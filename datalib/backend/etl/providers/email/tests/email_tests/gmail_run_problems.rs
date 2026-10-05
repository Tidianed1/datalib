//! When part of a Gmail sync fails: what failed is a `problems` row that
//! clears only once the same thing is tried again and works, and a
//! failure every later request would share ends the run.
//!
//! Driven through the HTTP playback layer: no credential, no network.

use std::collections::{BTreeMap, BTreeSet};

use datalib_etl::http::HttpResponse;
use datalib_etl_email::ingest::gmail_api::{self, FetchOptions, FetchSummary};
use datalib_etl_email::ingest::RawDb;
use serde_json::json;

use crate::support::{
    gmail_get_url, gmail_history_url, gmail_list_url, gmail_message, inbox_label, put_gmail,
    put_gmail_account, put_gmail_response, Mirror,
};

const PICARD: &str = "18c9f2a1b2c3d701";
const RIKER: &str = "18c9f2a1b2c3d702";

/// A message that fetched but would not store is behind the cursor the
/// run then stores, so no later listing names it. The next run asks for
/// it by id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_that_would_not_store_is_fetched_again() {
    let m = Mirror::new();
    put_gmail_account(&m.playback, "9001", json!([inbox_label()]));
    put_gmail(
        &m.playback,
        &gmail_list_url(&[]),
        &json!({ "messages": [{ "id": PICARD }, { "id": RIKER }] }),
    );
    put_gmail(
        &m.playback,
        &gmail_get_url(PICARD),
        &gmail_message(PICARD, &["INBOX"], "Engage"),
    );
    let mut unusable = gmail_message(RIKER, &["INBOX"], "Number One");
    unusable["raw"] = json!("");
    put_gmail(&m.playback, &gmail_get_url(RIKER), &unusable);
    run(&m, |_| {}).await.expect("first run");
    assert_eq!(
        problems(&m).await,
        [format!("record:gmail_messages:{RIKER}")]
    );

    // Nothing changed upstream since; the message now stores.
    put_gmail(
        &m.playback,
        &gmail_history_url("9001"),
        &json!({ "historyId": "9001" }),
    );
    put_gmail(
        &m.playback,
        &gmail_get_url(RIKER),
        &gmail_message(RIKER, &["INBOX"], "Number One"),
    );
    run(&m, |_| {}).await.expect("second run");
    assert_eq!(m.gmail_ids().await, ids(&[PICARD, RIKER]));
    assert!(problems(&m).await.is_empty());
}

/// A message over `blob_size_limit_bytes` is a warning on its `.eml`,
/// and raising the limit fetches it, although nothing lists it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversize_message_is_fetched_once_the_limit_allows() {
    let m = Mirror::new();
    put_gmail_account(&m.playback, "9001", json!([inbox_label()]));
    put_gmail(
        &m.playback,
        &gmail_list_url(&[]),
        &json!({ "messages": [{ "id": PICARD }] }),
    );
    put_gmail(
        &m.playback,
        &gmail_get_url(PICARD),
        &gmail_message(PICARD, &["INBOX"], "Engage"),
    );
    run(&m, |o| o.blob_size_limit_bytes = Some(10))
        .await
        .expect("first run");
    let rows = severities(&m).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].0.starts_with("email_blobs:"), "{rows:?}");
    assert_eq!(
        (rows[0].1.as_str(), rows[0].2.as_str()),
        ("warning", "over_size_limit")
    );

    put_gmail(
        &m.playback,
        &gmail_history_url("9001"),
        &json!({ "historyId": "9001" }),
    );
    run(&m, |_| {}).await.expect("second run");
    assert!(problems(&m).await.is_empty(), "the .eml landed");
}

/// A refused credential refuses every message after it: the run ends
/// on the first instead of writing one failure each.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_credential_ends_the_run() {
    let m = Mirror::new();
    put_gmail_account(&m.playback, "9001", json!([inbox_label()]));
    put_gmail(
        &m.playback,
        &gmail_list_url(&[]),
        &json!({ "messages": [{ "id": RIKER }, { "id": PICARD }] }),
    );
    put_gmail_response(&m.playback, &gmail_get_url(RIKER), &status(401));
    put_gmail(
        &m.playback,
        &gmail_get_url(PICARD),
        &gmail_message(PICARD, &["INBOX"], "Engage"),
    );
    run(&m, |_| {})
        .await
        .expect_err("a refused credential ends the run");
    assert!(
        m.gmail_ids().await.is_empty(),
        "nothing after it was fetched"
    );
    assert!(problems(&m).await.is_empty());
    assert_eq!(cursor(&m).await, None);
}

/// One label's walk that fails costs that label: the others are walked,
/// the cursor is held, and the next run that walks it clears the row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_walk_that_fails_costs_only_that_label() {
    let m = Mirror::new();
    put_gmail_account(
        &m.playback,
        "9001",
        json!([
            inbox_label(),
            { "id": "Label_7", "name": "away team", "type": "user" },
            { "id": "Label_9", "name": "holodeck", "type": "user" },
        ]),
    );
    put_gmail(
        &m.playback,
        &gmail_list_url(&["Label_7"]),
        &json!({ "messages": [{ "id": PICARD }] }),
    );
    put_gmail_response(&m.playback, &gmail_list_url(&["Label_9"]), &status(400));
    put_gmail(
        &m.playback,
        &gmail_get_url(PICARD),
        &gmail_message(PICARD, &["Label_7"], "Engage"),
    );
    put_gmail(
        &m.playback,
        &gmail_get_url(RIKER),
        &gmail_message(RIKER, &["Label_9"], "Number One"),
    );
    let labels = |o: &mut FetchOptions| {
        o.only_labels = vec!["away team".into(), "holodeck".into()];
    };
    run(&m, labels)
        .await
        .expect("one walk that fails does not fail the run");
    assert_eq!(m.gmail_ids().await, ids(&[PICARD]));
    assert_eq!(problems(&m).await, ["listing:messages.list holodeck"]);
    assert_eq!(cursor(&m).await, None, "the walk that failed is owed");

    put_gmail(
        &m.playback,
        &gmail_list_url(&["Label_9"]),
        &json!({ "messages": [{ "id": RIKER }] }),
    );
    run(&m, labels).await.expect("second run");
    assert_eq!(m.gmail_ids().await, ids(&[PICARD, RIKER]));
    assert!(problems(&m).await.is_empty());
}

/// A run that stops before it gets to an earlier failure has not tried
/// it, so the failure's row stands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_earlier_failure_the_run_never_reached_keeps_its_row() {
    let m = Mirror::new();
    put_gmail_account(&m.playback, "9001", json!([inbox_label()]));
    put_gmail(
        &m.playback,
        &gmail_list_url(&[]),
        &json!({ "messages": [{ "id": RIKER }] }),
    );
    put_gmail_response(&m.playback, &gmail_get_url(RIKER), &status(400));
    run(&m, |_| {}).await.expect("first run");
    let failed = [format!("record:gmail_messages:{RIKER}")];
    assert_eq!(problems(&m).await, failed);

    let second = run(&m, |o| o.config.message_budget = Some(0))
        .await
        .expect("second run");
    assert!(second.budget_exhausted, "{second:?}");
    assert_eq!(problems(&m).await, failed);
}

// ── helpers ─────────────────────────────────────────────────────────

async fn run(m: &Mirror, tweak: impl FnOnce(&mut FetchOptions)) -> anyhow::Result<FetchSummary> {
    m.run(|db| {
        let mut opts = FetchOptions::new(db);
        tweak(&mut opts);
        gmail_api::fetch(opts)
    })
    .await
}

fn ids(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn status(code: u16) -> HttpResponse {
    HttpResponse {
        status: code,
        headers: BTreeMap::new(),
        body: b"{\"error\":{\"message\":\"nope\"}}".to_vec(),
        duration_ms: 0,
    }
}

async fn problems(m: &Mirror) -> Vec<String> {
    severities(m).await.into_iter().map(|r| r.0).collect()
}

/// Each `problems` row: its key, severity and reason.
async fn severities(m: &Mirror) -> Vec<(String, String, String)> {
    m.read(|db: RawDb| async move {
        sqlx::query_as("SELECT scope_key, severity, reason FROM problems ORDER BY scope_key")
            .fetch_all(db.pool())
            .await
            .unwrap()
    })
    .await
}

async fn cursor(m: &Mirror) -> Option<String> {
    m.read(|db: RawDb| async move {
        sqlx::query_scalar("SELECT last_seen_at_utc FROM sync_scope_state WHERE scope = ?")
            .bind("gmail:t@example.test:historyId")
            .fetch_optional(db.pool())
            .await
            .unwrap()
    })
    .await
}
