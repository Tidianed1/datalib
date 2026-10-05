//! When part of a Fastmail (JMAP) sync fails: a listing or a phase that
//! did not answer is a `problems` row and the rest of the run goes on; a
//! download the server refused ends the `.eml` phase, keeping what
//! landed; and each row clears only once the thing it is about is tried
//! again and works.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use datalib_etl::http::{HttpRequest, HttpResponse, HttpService};
use datalib_etl::synthesize::{json_response, write_fixture};
use datalib_etl_email::ingest::session::Session;
use datalib_etl_email::ingest::{api, FetchOptions, FetchSummary, RawDb};
use serde_json::{json, Value};

use crate::support::Mirror;

const HOST: &str = "jmap.example.test";
const ACCOUNT: &str = "A1";

/// A full re-list whose `Mailbox/get` fails keeps the mailboxes an
/// earlier run stored and still mirrors the mail.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mailbox_listing_that_fails_is_a_row_and_the_run_goes_on() {
    let m = Mirror::new();
    let tape = Tape::new(&m.playback);
    tape.account(&[("MB1", "Inbox")], &[("M1", &["MB1"])]);
    run(&m, |_| {}).await.expect("first run");

    tape.account(&[("MB1", "Inbox")], &[("M1", &["MB1"]), ("M2", &["MB1"])]);
    tape.refuse(
        "Mailbox/get",
        json!({ "accountId": ACCOUNT, "ids": null }),
        400,
    );
    let second = run(&m, |_| {})
        .await
        .expect("a mailbox listing that fails does not fail the run");
    assert_eq!(second.emails_upserted, 2, "{second:?}");
    assert_eq!(problems(&m).await, [row("listing:Mailbox/get", "error")]);
    assert_eq!(mailboxes(&m).await, ["MB1"]);

    tape.account(&[("MB1", "Inbox")], &[("M1", &["MB1"]), ("M2", &["MB1"])]);
    run(&m, |_| {}).await.expect("third run");
    assert!(
        problems(&m).await.is_empty(),
        "listing again clears the row"
    );
}

/// An `Email/query` walk that fails keeps every stored email: absence
/// from a walk that stopped means nothing. With nothing stored, there is
/// nothing to keep going for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_email_listing_that_fails_deletes_nothing() {
    let m = Mirror::new();
    let tape = Tape::new(&m.playback);
    tape.account(&[("MB1", "Inbox")], &[("M1", &["MB1"]), ("M2", &["MB1"])]);
    run(&m, |_| {}).await.expect("first run");

    tape.refuse("Email/query", query_args(), 400);
    run(&m, |_| {})
        .await
        .expect("an email listing that fails does not fail the run");
    assert_eq!(problems(&m).await, [row("listing:Email/query", "error")]);
    assert_eq!(emails(&m).await, ["M1", "M2"]);

    let fresh = Mirror::new();
    let tape = Tape::new(&fresh.playback);
    tape.account(&[("MB1", "Inbox")], &[]);
    tape.refuse("Email/query", query_args(), 400);
    run(&fresh, |_| {})
        .await
        .expect_err("with no email stored, a listing that fails fails the run");
}

/// A thread `Thread/get` did not answer for is asked for again by the
/// next run, although none of its emails changed to name it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thread_that_did_not_answer_is_asked_for_again() {
    let m = Mirror::new();
    let tape = Tape::new(&m.playback);
    tape.account(&[("MB1", "Inbox")], &[("M1", &["MB1"])]);
    tape.refuse(
        "Thread/get",
        json!({ "accountId": ACCOUNT, "ids": ["TM1"] }),
        400,
    );
    let first = run(&m, |_| {})
        .await
        .expect("a thread phase that fails does not fail the run");
    assert_eq!(first.blobs_downloaded, 1, "the blobs still download");
    assert_eq!(problems(&m).await, [row("phase:Thread/get", "error")]);
    assert!(threads(&m).await.is_empty());

    // Nothing changed upstream: the next run is incremental and replays
    // no email.
    tape.unchanged_since("mbox-1", "email-1");
    tape.threads(&["M1"]);
    run(&m, |o| o.full_resync = false)
        .await
        .expect("second run");
    assert_eq!(threads(&m).await, ["TM1"]);
    assert!(problems(&m).await.is_empty());
}

/// A refused credential fails every download after it the same way, so
/// it ends the phase rather than writing one failure per `.eml`. The
/// refusal was once the run's error, and a failed run commits nothing:
/// every body the run had downloaded went with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_download_stops_the_phase_and_keeps_what_downloaded() {
    let m = Mirror::new();
    let tape = Tape::new(&m.playback);
    let mail: [(&str, &[&str]); 3] = [("M1", &["MB1"]), ("M2", &["MB1"]), ("M3", &["MB1"])];
    tape.account(&[("MB1", "Inbox")], &mail);
    tape.blob("M2", status(401));
    let first = run(&m, |o| o.blob_download_concurrency = Some(1))
        .await
        .expect("a refused download keeps the run's work");
    assert_eq!(first.blobs_downloaded, 1, "{first:?}");
    assert_eq!(blobs(&m).await, [("M1".to_string(), true)]);
    assert_eq!(problems(&m).await, [row("phase:eml_download", "error")]);
    let said = sample(&m, "phase:eml_download").await;
    assert!(
        said.starts_with("the server refused the credential (HTTP 401); 1 downloaded, 2 left"),
        "{said}"
    );

    tape.account(&[("MB1", "Inbox")], &mail);
    let second = run(&m, |_| {}).await.expect("second run");
    assert_eq!(second.blobs_downloaded, 2, "{second:?}");
    assert_eq!(
        blobs(&m).await,
        ["M1", "M2", "M3"].map(|id| (id.to_string(), true))
    );
    assert!(problems(&m).await.is_empty(), "{:?}", problems(&m).await);
}

/// Twenty downloads failing in a row end the phase the same way: the
/// bodies before them are kept, each failure keeps its own row, and the
/// bodies the phase never asked for are downloaded by the next run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_of_failed_downloads_stops_the_phase_and_keeps_what_downloaded() {
    let m = Mirror::new();
    let tape = Tape::new(&m.playback);
    let ids: Vec<String> = (1..=25).map(|n| format!("M{n:02}")).collect();
    let mail: Vec<(&str, &[&str])> = ids.iter().map(|id| (id.as_str(), &["MB1"][..])).collect();
    tape.account(&[("MB1", "Inbox")], &mail);
    for id in &ids[3..23] {
        tape.blob(id, status(404));
    }
    let first = run(&m, |o| o.blob_download_concurrency = Some(1))
        .await
        .expect("a spent failure budget keeps the run's work");
    assert_eq!(first.blobs_downloaded, 3, "{first:?}");
    let stored = blobs(&m).await;
    assert_eq!(
        stored.iter().filter(|(_, landed)| *landed).count(),
        3,
        "{stored:?}"
    );
    assert_eq!(stored.len(), 23, "the last two were never asked for");
    let found = problems(&m).await;
    assert!(
        found.contains(&row("phase:eml_download", "error")),
        "{found:?}"
    );
    assert_eq!(found.len(), 21, "one row per failed .eml, and the phase's");
    let said = sample(&m, "phase:eml_download").await;
    assert!(
        said.starts_with("20 .eml downloads failed in a row; 3 downloaded, 22 left"),
        "{said}"
    );

    tape.account(&[("MB1", "Inbox")], &mail);
    let second = run(&m, |_| {}).await.expect("second run");
    assert_eq!(second.blobs_downloaded, 22, "{second:?}");
    assert!(blobs(&m).await.iter().all(|(_, landed)| *landed));
    assert!(problems(&m).await.is_empty(), "{:?}", problems(&m).await);
}

/// Every body once waited in memory for one write at the end of the
/// phase, so a kill lost them all and no seal could publish any. With
/// the bound at one byte each body is its own write, and each write is
/// a point the run may seal at: some commit holds part of the bodies.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bodies_are_written_and_sealed_as_they_land() {
    let m = Mirror::new();
    let tape = Tape::new(&m.playback);
    tape.account(
        &[("MB1", "Inbox")],
        &[("M1", &["MB1"]), ("M2", &["MB1"]), ("M3", &["MB1"])],
    );
    m.run_sealing(|db, sealer| {
        let mut opts = FetchOptions::new(db);
        opts.hostname = HOST.to_string();
        opts.full_resync = true;
        opts.sealer = Some(sealer);
        opts.blob_download_concurrency = Some(1);
        opts.blob_flush_bytes = Some(1);
        datalib_etl_email::ingest::fetch(opts)
    })
    .await
    .expect("run");

    let held = m
        .read(|db: RawDb| async move {
            let commits: Vec<String> = sqlx::query_scalar("SELECT commit_hash FROM dolt_log")
                .fetch_all(db.pool())
                .await
                .unwrap();
            let mut held = Vec::new();
            for commit in commits {
                // Audited for `AssertSqlSafe`: `commit` is a hash doltlite
                // just listed, and the table function takes a literal. The
                // store's first commit predates the table.
                let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                    "SELECT count(*) FROM dolt_at_email_blobs('{commit}') \
                     WHERE blake3 IS NOT NULL"
                )))
                .fetch_one(db.pool())
                .await
                .unwrap_or(0);
                held.push(n);
            }
            held
        })
        .await;
    assert!(
        held.contains(&3),
        "the run's end holds every body: {held:?}"
    );
    assert!(
        held.contains(&1) && held.contains(&2),
        "no commit holds only part of the bodies, so none was sealed mid-phase: {held:?}"
    );
}

/// An `.eml` over `blob_size_limit_bytes` was turned away on purpose: a
/// warning that the mirror lacks it, not a download that failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversize_eml_is_a_skip_not_a_failure() {
    let m = Mirror::new();
    let tape = Tape::new(&m.playback);
    tape.account(&[("MB1", "Inbox")], &[("M1", &["MB1"])]);
    run(&m, |o| o.blob_size_limit_bytes = Some(10))
        .await
        .expect("run");
    let rows = m
        .read(|db: RawDb| async move {
            sqlx::query_as::<_, (String, String, String)>(
                "SELECT scope_key, severity, reason FROM problems",
            )
            .fetch_all(db.pool())
            .await
            .unwrap()
        })
        .await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].0.starts_with("email_blobs:M1#"), "{rows:?}");
    assert_eq!(
        (rows[0].1.as_str(), rows[0].2.as_str()),
        ("warning", "over_size_limit")
    );
}

/// A label path that matched nothing is a row, and the run that no
/// longer names it clears the row, filter or no filter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unmatched_label_row_goes_when_the_filter_does() {
    let m = Mirror::new();
    let tape = Tape::new(&m.playback);
    tape.account(&[("MB1", "Inbox")], &[("M1", &["MB1"])]);
    run(&m, |o| o.only_mailbox_labels = vec!["Starbase".into()])
        .await
        .expect("first run");
    assert_eq!(
        problems(&m).await,
        [row("config:only_extract_labels:Starbase", "warning")]
    );

    run(&m, |_| {}).await.expect("second run");
    assert!(problems(&m).await.is_empty());
}

// ── helpers ─────────────────────────────────────────────────────────

/// A full re-list, as the tests above mostly want, unless `tweak` says
/// otherwise.
async fn run(m: &Mirror, tweak: impl FnOnce(&mut FetchOptions)) -> anyhow::Result<FetchSummary> {
    m.run(|db| {
        let mut opts = FetchOptions::new(db);
        opts.hostname = HOST.to_string();
        opts.full_resync = true;
        tweak(&mut opts);
        datalib_etl_email::ingest::fetch(opts)
    })
    .await
}

fn row(key: &str, severity: &str) -> (String, String) {
    (key.to_string(), severity.to_string())
}

async fn problems(m: &Mirror) -> Vec<(String, String)> {
    m.read(|db: RawDb| async move {
        sqlx::query_as("SELECT scope_key, severity FROM problems ORDER BY scope_key")
            .fetch_all(db.pool())
            .await
            .unwrap()
    })
    .await
}

async fn sample(m: &Mirror, key: &'static str) -> String {
    m.read(|db: RawDb| async move {
        sqlx::query_scalar("SELECT sample FROM problems WHERE scope_key = ?")
            .bind(key)
            .fetch_one(db.pool())
            .await
            .unwrap()
    })
    .await
}

async fn ids(m: &Mirror, sql: &'static str) -> Vec<String> {
    m.read(|db: RawDb| async move { sqlx::query_scalar(sql).fetch_all(db.pool()).await.unwrap() })
        .await
}

async fn mailboxes(m: &Mirror) -> Vec<String> {
    ids(m, "SELECT id FROM mailboxes ORDER BY id").await
}

async fn emails(m: &Mirror) -> Vec<String> {
    ids(m, "SELECT id FROM emails ORDER BY id").await
}

async fn threads(m: &Mirror) -> Vec<String> {
    ids(m, "SELECT id FROM threads ORDER BY id").await
}

/// Each `email_blobs` edge: its email, and whether its bytes landed.
async fn blobs(m: &Mirror) -> Vec<(String, bool)> {
    m.read(|db: RawDb| async move {
        sqlx::query_as("SELECT email_id, blake3 IS NOT NULL FROM email_blobs ORDER BY email_id")
            .fetch_all(db.pool())
            .await
            .unwrap()
    })
    .await
}

fn status(code: u16) -> HttpResponse {
    HttpResponse {
        status: code,
        headers: BTreeMap::new(),
        body: b"{}".to_vec(),
        duration_ms: 0,
    }
}

fn query_args() -> Value {
    json!({
        "accountId": ACCOUNT,
        "sort": [{ "property": "receivedAt", "isAscending": false }],
        "limit": 500,
        "position": 0,
        "calculateTotal": true,
    })
}

/// The fixtures a run reads. Each request is keyed on its exact bytes,
/// built by `api::method_request` as the provider builds it, and a later
/// write to the same request replaces the answer.
struct Tape {
    out: PathBuf,
    session: Session,
}

impl Tape {
    fn new(out: &Path) -> Self {
        let session_json = json!({
            "apiUrl": "https://jmap.example.test/jmap/api/",
            "downloadUrl": "https://jmap.example.test/jmap/download/{accountId}/{blobId}/{name}?type={type}",
            "uploadUrl": "https://jmap.example.test/jmap/upload/{accountId}/",
            "primaryAccounts": { "urn:ietf:params:jmap:mail": ACCOUNT },
            "accounts": { ACCOUNT: { "name": "t@example.test", "isPersonal": true } },
        });
        write_fixture(
            out,
            &HttpRequest::get(
                HttpService::Jmap,
                format!("https://{HOST}/.well-known/jmap"),
            ),
            &json_response(&session_json),
        )
        .expect("write the session fixture");
        Self {
            out: out.to_path_buf(),
            session: Session::from_value(session_json).expect("parse the fixture session"),
        }
    }

    fn answer(&self, method: &str, args: Value, response: &HttpResponse) {
        let req = api::method_request(&self.session, method, args).expect("build the request");
        write_fixture(&self.out, &req, response).expect("write fixture");
    }

    fn call(&self, method: &str, args: Value, result: Value) {
        let body = json!({ "methodResponses": [[method, result, "a"]] });
        self.answer(method, args, &json_response(&body));
    }

    fn refuse(&self, method: &str, args: Value, code: u16) {
        self.answer(method, args, &status(code));
    }

    /// A whole account: its mailboxes, its emails, their threads (one
    /// each, `T<id>`) and their `.eml`s.
    fn account(&self, mailboxes: &[(&str, &str)], emails: &[(&str, &[&str])]) {
        self.call(
            "Mailbox/get",
            json!({ "accountId": ACCOUNT, "ids": null }),
            json!({
                "state": "mbox-1",
                "list": mailboxes
                    .iter()
                    .map(|(id, name)| json!({ "id": id, "name": name, "parentId": null }))
                    .collect::<Vec<_>>(),
            }),
        );
        let ids: Vec<&str> = emails.iter().map(|(id, _)| *id).collect();
        self.call(
            "Email/query",
            query_args(),
            json!({ "ids": ids, "queryState": "q-1", "total": ids.len() }),
        );
        self.call(
            "Email/get",
            json!({
                "accountId": ACCOUNT,
                "ids": ids,
                "properties": [
                    "id", "blobId", "threadId", "mailboxIds", "keywords", "from",
                    "subject", "sentAt", "receivedAt", "size", "messageId",
                    "hasAttachment", "attachments",
                ],
            }),
            json!({
                "state": "email-1",
                "list": emails.iter().map(|(id, filed)| email(id, filed)).collect::<Vec<_>>(),
            }),
        );
        self.threads(&ids);
        for id in ids {
            self.blob(id, eml(id));
        }
    }

    fn threads(&self, email_ids: &[&str]) {
        let thread_ids: Vec<String> = email_ids.iter().map(|id| format!("T{id}")).collect();
        self.call(
            "Thread/get",
            json!({ "accountId": ACCOUNT, "ids": thread_ids }),
            json!({
                "state": "thread-1",
                "list": email_ids
                    .iter()
                    .map(|id| json!({ "id": format!("T{id}"), "emailIds": [id] }))
                    .collect::<Vec<_>>(),
            }),
        );
    }

    fn blob(&self, email_id: &str, response: HttpResponse) {
        let url = self.session.download_url_for(
            ACCOUNT,
            &format!("B{email_id}"),
            "message.eml",
            "message/rfc822",
        );
        write_fixture(
            &self.out,
            &HttpRequest::get(HttpService::Jmap, url),
            &response,
        )
        .expect("write a blob fixture");
    }

    /// An incremental run's answers when nothing changed.
    fn unchanged_since(&self, mailbox_state: &str, email_state: &str) {
        for (method, state) in [
            ("Mailbox/changes", mailbox_state),
            ("Email/changes", email_state),
        ] {
            self.call(
                method,
                json!({ "accountId": ACCOUNT, "sinceState": state, "maxChanges": 5000 }),
                json!({
                    "created": [], "updated": [], "destroyed": [],
                    "newState": state, "hasMoreChanges": false,
                }),
            );
        }
    }
}

fn email(id: &str, filed: &[&str]) -> Value {
    let mailbox_ids: serde_json::Map<String, Value> = filed
        .iter()
        .map(|m| (m.to_string(), Value::Bool(true)))
        .collect();
    json!({
        "id": id,
        "blobId": format!("B{id}"),
        "threadId": format!("T{id}"),
        "mailboxIds": mailbox_ids,
        "keywords": { "$seen": true },
        "subject": format!("Stardate log {id}"),
        "receivedAt": "2026-09-01T10:00:00Z",
        "size": 256,
        "hasAttachment": false,
    })
}

fn eml(id: &str) -> HttpResponse {
    let body = format!(
        "Message-ID: <{id}@enterprise.starfleet>\r\n\
         Date: Tue, 1 Sep 2026 10:00:00 +0000\r\n\
         From: data@enterprise.starfleet\r\n\
         Subject: Stardate log {id}\r\n\
         \r\n\
         body of {id}\r\n",
    );
    HttpResponse {
        status: 200,
        headers: [("content-type".to_string(), "message/rfc822".to_string())].into(),
        body: body.into_bytes(),
        duration_ms: 0,
    }
}
