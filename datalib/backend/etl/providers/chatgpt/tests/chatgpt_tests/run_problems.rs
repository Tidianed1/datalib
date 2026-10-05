//! Part of a sync that fails is a `problems` row, not a failed step, and
//! the row clears only once the same thing has been tried again and
//! worked.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use datalib_etl::http::{fixture_key, HttpRequest, HttpResponse, HttpService, PLAYBACK_ENV};
use datalib_etl::retry::{self, RetryGuard};
use datalib_etl::store_handle::RawStoreHandle;
use datalib_etl::synthesize::{write_fixture, Synthesizer};
use datalib_etl_chatgpt::ingest::{db_path_for, fetch, FetchOptions, FetchSummary, RawDb};
use datalib_etl_chatgpt::synthesize::ChatgptSynth;
use serde_json::{json, Value};
use tempfile::TempDir;

const BASE: &str = "https://chatgpt.com";

fn write_json(path: &Path, v: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(v).unwrap()).unwrap();
}

fn get(path: &str) -> HttpRequest {
    HttpRequest::get(HttpService::Chatgpt, format!("{BASE}{path}"))
        .header("Accept", "application/json")
}

fn listing_page(offset: usize) -> HttpRequest {
    get(&format!(
        "/backend-api/conversations?offset={offset}&limit=100&order=updated"
    ))
}

fn conversation_request(id: &str) -> HttpRequest {
    get(&format!("/backend-api/conversation/{id}"))
}

fn conversation(id: &str, update_time: f64) -> Value {
    json!({"id": id, "update_time": update_time, "mapping": {}, "title": id})
}

/// One account's input snapshot, its playback tape, and the store a run
/// writes.
struct Account {
    _dir: TempDir,
    api: PathBuf,
    playback: PathBuf,
    raw: PathBuf,
}

impl Account {
    fn new(convs: &[Value]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let a = Account {
            api: dir.path().join("api"),
            playback: dir.path().join("playback"),
            raw: dir.path().join("raw"),
            _dir: dir,
        };
        fs::create_dir_all(&a.raw).unwrap();
        write_json(&a.api.join("me.json"), &json!({"id": "u-1"}));
        a.list(convs);
        a
    }

    /// Rewrite the listing (and each conversation's detail) to `convs`.
    fn list(&self, convs: &[Value]) {
        let items: Vec<Value> = convs
            .iter()
            .map(|c| json!({"id": c["id"], "update_time": c["update_time"], "title": c["title"]}))
            .collect();
        write_json(&self.api.join("conversations.json"), &Value::Array(items));
        for c in convs {
            let id = c["id"].as_str().unwrap();
            write_json(&self.api.join(format!("conversations/{id}.json")), c);
        }
        ChatgptSynth::new(&self.api)
            .synthesize(&self.playback)
            .unwrap();
    }

    fn answer(&self, req: &HttpRequest, status: u16) {
        let resp = HttpResponse {
            status,
            headers: Default::default(),
            body: b"{\"detail\":\"no\"}".to_vec(),
            duration_ms: 0,
        };
        write_fixture(&self.playback, req, &resp).unwrap();
    }

    fn forget(&self, req: &HttpRequest) {
        let path = self
            .playback
            .join(HttpService::Chatgpt.as_str())
            .join(fixture_key(req));
        fs::remove_file(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }

    async fn run(&self, conv_uuids: &[&str]) -> anyhow::Result<FetchSummary> {
        std::env::set_var(PLAYBACK_ENV, &self.playback);
        let db = RawDb::open(&db_path_for(&self.raw)).await.unwrap();
        let control = datalib_etl::control::DownloadControl::default();
        // One failure is the give-up: a 429 ends as `RateLimited` at once.
        let fast = Duration::from_millis(1);
        let guard = RetryGuard::new(
            Duration::from_secs(3600),
            1,
            fast,
            fast,
            control.stop.clone(),
        );
        let s = retry::scope(
            guard,
            fetch(FetchOptions {
                conv_uuids: conv_uuids.iter().map(|s| s.to_string()).collect(),
                control,
                ..FetchOptions::new(db.clone())
            }),
        )
        .await;
        db.commit_all("test").await.unwrap();
        db.close().await;
        std::env::remove_var(PLAYBACK_ENV);
        s
    }

    /// `(scope_key, detail)` of every `problems` row.
    async fn problems(&self) -> Vec<(String, String)> {
        self.query("SELECT scope_key, sample FROM problems ORDER BY scope_key")
            .await
    }

    async fn query(&self, sql: &'static str) -> Vec<(String, String)> {
        let db = RawDb::open(&db_path_for(&self.raw)).await.unwrap();
        let rows = sqlx::query_as(sql).fetch_all(db.pool()).await.unwrap();
        db.close().await;
        rows
    }

    async fn keys(&self) -> Vec<String> {
        self.problems().await.into_iter().map(|(k, _)| k).collect()
    }
}

/// One test, several scenarios, run in sequence: `PLAYBACK_ENV` is
/// process-global, so as separate tests they would race.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn part_of_a_sync_that_fails_is_a_problem_row() {
    a_rate_limit_is_a_phase_row_until_the_rest_is_fetched().await;
    a_failed_listing_page_keeps_the_pages_before_it_and_prunes_nothing().await;
    a_first_listing_page_that_fails_with_nothing_stored_fails_the_step().await;
    a_named_conversation_that_fails_costs_only_itself().await;
    a_pruned_conversation_takes_its_problem_with_it().await;
    a_failed_attachment_says_why_and_is_tried_again_while_unchanged().await;
}

/// The rate limit used to end the walk with a `warn!` and a run that read
/// as clean.
async fn a_rate_limit_is_a_phase_row_until_the_rest_is_fetched() {
    let convs = [
        conversation("c-a", 3.0),
        conversation("c-b", 2.0),
        conversation("c-c", 1.0),
    ];
    let acct = Account::new(&convs);
    acct.answer(&conversation_request("c-b"), 429);

    let s = acct
        .run(&[])
        .await
        .expect("a rate limit is not a failed step");
    assert_eq!(s.fetched, 1, "c-a landed before the limit: {s:?}");
    let problems = acct.problems().await;
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].0, "phase:conversations");
    assert!(
        problems[0]
            .1
            .contains("rate-limited after 1 fetched; 2 left"),
        "{problems:?}"
    );

    acct.list(&convs);
    let s = acct.run(&[]).await.unwrap();
    assert_eq!(s.fetched, 2, "the skip-check queued the rest again: {s:?}");
    assert_eq!(acct.keys().await, Vec::<String>::new());
}

/// A listing page failing after the first used to fail the step and
/// throw away the pages it had.
async fn a_failed_listing_page_keeps_the_pages_before_it_and_prunes_nothing() {
    let convs: Vec<Value> = (0..101)
        .map(|i| conversation(&format!("c-{i:03}"), 1000.0 - i as f64))
        .collect();
    let acct = Account::new(&convs);
    let first = acct.run(&[]).await.unwrap();
    assert_eq!(first.fetched, 101);

    acct.forget(&listing_page(100));
    let s = acct.run(&[]).await.expect("the first page listed");
    assert_eq!(s.listing, 100, "page 1 is kept: {s:?}");
    assert_eq!(s.pruned, 0, "an incomplete listing deletes nothing");
    let stored = acct
        .query("SELECT id, id FROM conversations WHERE id = 'c-100'")
        .await;
    assert_eq!(stored.len(), 1, "the conversation on the lost page stays");
    assert_eq!(acct.keys().await, ["listing:conversations"]);

    acct.list(&convs);
    acct.run(&[]).await.unwrap();
    assert_eq!(acct.keys().await, Vec::<String>::new());
}

/// With nothing listed and nothing stored the run has done nothing, which
/// is a failed step.
async fn a_first_listing_page_that_fails_with_nothing_stored_fails_the_step() {
    let acct = Account::new(&[conversation("c-a", 1.0)]);
    acct.forget(&listing_page(0));
    let err = acct.run(&[]).await.expect_err("nothing to fall back on");
    assert!(format!("{err:#}").contains("list conversations"), "{err:#}");
}

/// One named conversation that would not fetch used to fail the step and
/// skip the rest.
async fn a_named_conversation_that_fails_costs_only_itself() {
    let acct = Account::new(&[
        conversation("c-a", 1.0),
        conversation("c-gone", 1.0),
        conversation("c-err", 1.0),
    ]);
    acct.answer(&conversation_request("c-gone"), 404);
    acct.answer(&conversation_request("c-err"), 500);

    let s = acct
        .run(&["c-gone", "c-err", "c-a"])
        .await
        .expect("one failure is not the run's");
    assert_eq!((s.fetched, s.errors), (1, 1), "{s:?}");
    assert_eq!(
        acct.keys().await,
        ["config:conv_uuids:c-gone", "conversations:c-err"]
    );

    acct.list(&[
        conversation("c-a", 1.0),
        conversation("c-gone", 1.0),
        conversation("c-err", 1.0),
    ]);
    acct.run(&["c-gone", "c-err", "c-a"]).await.unwrap();
    assert_eq!(acct.keys().await, Vec::<String>::new());
}

/// The prune deleted the row and left its problem standing for good.
async fn a_pruned_conversation_takes_its_problem_with_it() {
    let acct = Account::new(&[conversation("c-a", 1.0), conversation("c-x", 1.0)]);
    acct.answer(&conversation_request("c-x"), 500);
    acct.run(&[]).await.unwrap();
    assert_eq!(acct.keys().await, ["conversations:c-x"]);

    acct.list(&[conversation("c-a", 1.0)]);
    let s = acct.run(&[]).await.unwrap();
    assert_eq!(s.pruned, 1, "{s:?}");
    assert_eq!(acct.keys().await, Vec::<String>::new());
}

/// An attachment's row said "no bytes" whatever went wrong, and was tried
/// again only when its conversation changed.
async fn a_failed_attachment_says_why_and_is_tried_again_while_unchanged() {
    let with_file = json!({
        "id": "c-a",
        "update_time": 1.0,
        "title": "Holodeck log",
        "mapping": {"n1": {"message": {"metadata": {"attachments": [
            {"id": "file-1", "name": "program.txt", "mime_type": "text/plain"}
        ]}}}},
    });
    let acct = Account::new(&[with_file]);
    let s = acct.run(&[]).await.unwrap();
    assert_eq!(s.failed_blobs, 1, "{s:?}");
    let problems = acct.problems().await;
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].0, "chatgpt_attachments:c-a#file-1");
    assert!(
        problems[0].1.starts_with("file metadata: "),
        "the real reason, not 'no bytes': {problems:?}"
    );

    let s = acct.run(&[]).await.unwrap();
    assert_eq!(s.skipped, 1, "the conversation is unchanged: {s:?}");
    assert_eq!(
        s.failed_blobs, 1,
        "but its attachment is tried again: {s:?}"
    );
    let attempts = acct
        .query("SELECT id, CAST(attempt_count AS TEXT) FROM chatgpt_attachments_bookkeeping")
        .await;
    assert_eq!(attempts, [("c-a#file-1".to_string(), "2".to_string())]);
}
