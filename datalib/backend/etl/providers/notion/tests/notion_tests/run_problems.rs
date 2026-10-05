//! Part of a sync that fails is a `problems` row, not a failed step or a
//! log line: the rest of the run goes on, and the row goes once the same
//! thing fetches — which means a later run has to try it again, though
//! upstream has not moved it.

use std::path::Path;

use datalib_etl::http::{HttpRequest, HttpResponse, HttpService, PLAYBACK_ENV};
use datalib_etl::store_handle::RawStoreHandle;
use datalib_etl::synthesize::{json_response, write_fixture};
use datalib_etl_notion::ingest::official::{BASE, PAGE_SIZE};
use datalib_etl_notion::ingest::{fetch, FetchOptions, FetchSummary, RawDb};
use serde_json::{json, Value};
use tempfile::tempdir;

const BRIDGE: &str = "1701d000-0000-4000-8000-000000000001";
const SICKBAY: &str = "1701d000-0000-4000-8000-000000000002";
const HOLODECK: &str = "1701d000-0000-4000-8000-000000000003";
const TEN_FORWARD: &str = "1701d000-0000-4000-8000-000000000004";
const EDITED: &str = "2026-09-01T00:00:00.000Z";

fn get(url: &str) -> HttpRequest {
    HttpRequest::get(HttpService::Notion, url).header("Accept", "application/json")
}

fn serve(tape: &Path, url: &str, body: Value) {
    write_fixture(tape, &get(url), &json_response(&body)).unwrap();
}

fn page(id: &str, edited: &str) -> Value {
    json!({
        "object": "page",
        "id": id,
        "last_edited_time": edited,
        "parent": {"type": "workspace", "workspace": true},
    })
}

fn serve_object(tape: &Path, id: &str, edited: &str) {
    serve(tape, &format!("{BASE}/pages/{id}"), page(id, edited));
}

fn serve_body(tape: &Path, id: &str, markdown: &str, truncated: bool) {
    serve(
        tape,
        &format!("{BASE}/pages/{id}/markdown"),
        json!({"object": "page_markdown", "id": id, "markdown": markdown, "truncated": truncated}),
    );
}

fn serve_comments(tape: &Path, id: &str) {
    serve(
        tape,
        &format!("{BASE}/comments?block_id={id}&page_size={PAGE_SIZE}"),
        json!({"object": "list", "results": [], "has_more": false, "next_cursor": null}),
    );
}

fn serve_page(tape: &Path, id: &str, edited: &str, markdown: &str) {
    serve_object(tape, id, edited);
    serve_body(tape, id, markdown, false);
    serve_comments(tape, id);
}

fn serve_search(tape: &Path, cursor: Option<&str>, results: Value, next: Option<&str>) {
    let mut body = json!({
        "page_size": PAGE_SIZE,
        "sort": { "timestamp": "last_edited_time", "direction": "descending" },
    });
    if let Some(c) = cursor {
        body["start_cursor"] = json!(c);
    }
    let req = HttpRequest::post_json(
        HttpService::Notion,
        format!("{BASE}/search"),
        body.to_string().into_bytes(),
    )
    .header("Accept", "application/json");
    let resp = json!({
        "object": "list",
        "results": results,
        "has_more": next.is_some(),
        "next_cursor": next,
    });
    write_fixture(tape, &req, &json_response(&resp)).unwrap();
}

async fn run(tape: &Path, store: &Path, roots: &[&str]) -> anyhow::Result<FetchSummary> {
    std::env::set_var(PLAYBACK_ENV, tape);
    let db = RawDb::open(store).await.unwrap();
    let summary = fetch(FetchOptions {
        subtree_pages: roots.iter().map(|r| r.to_string()).collect(),
        ..FetchOptions::new(db.clone())
    })
    .await;
    db.commit_all("test").await.unwrap();
    db.close().await;
    summary
}

async fn problems(store: &Path) -> Vec<(String, String)> {
    let db = RawDb::open(store).await.unwrap();
    let rows = sqlx::query_as("SELECT scope_key, severity FROM problems ORDER BY scope_key")
        .fetch_all(db.pool())
        .await
        .unwrap();
    db.close().await;
    rows
}

fn row(key: &str, severity: &str) -> (String, String) {
    (key.to_string(), severity.to_string())
}

/// A body that would not fetch used to be a log line, and the page was
/// stored with its new `last_edited_time`, so no later run asked again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_body_that_did_not_fetch_is_a_problem_until_it_does() {
    let d = tempdir().unwrap();
    let (tape, store) = (d.path().join("tape"), d.path().join("s.doltlite_db"));
    serve_object(&tape, BRIDGE, EDITED);
    serve_comments(&tape, BRIDGE);

    run(&tape, &store, &[BRIDGE]).await.unwrap();
    assert_eq!(
        problems(&store).await,
        vec![row(&format!("page_markdown:{BRIDGE}"), "error")]
    );

    serve_body(&tape, BRIDGE, "Captain's log.\n", false);
    run(&tape, &store, &[BRIDGE]).await.unwrap();
    assert!(problems(&store).await.is_empty());
    let db = RawDb::open(&store).await.unwrap();
    let bodies = db.load_page_markdown().await.unwrap();
    db.close().await;
    assert_eq!(
        bodies,
        vec![(BRIDGE.to_string(), "Captain's log.\n".into())]
    );
}

/// An attachment whose bytes did not come back was stamped as fetched.
/// Its signed URL lives only in the response that named it, so the page
/// has to be fetched again for the retry, though it has not moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attachment_that_did_not_fetch_is_retried_through_its_page() {
    let d = tempdir().unwrap();
    let (tape, store) = (d.path().join("tape"), d.path().join("s.doltlite_db"));
    let signed =
        "https://prod-files-secure.s3.us-west-2.amazonaws.com/ws/sickbay.png?X-Amz-Signature=abc";
    let slot = "https://prod-files-secure.s3.us-west-2.amazonaws.com/ws/sickbay.png";
    serve_page(&tape, SICKBAY, EDITED, &format!("![chart]({signed})\n"));

    run(&tape, &store, &[SICKBAY]).await.unwrap();
    assert_eq!(
        problems(&store).await,
        vec![row(
            &format!("notion_attachments:{SICKBAY}#{slot}"),
            "error"
        )]
    );

    let bytes = HttpResponse {
        status: 200,
        headers: [("content-type".to_string(), "image/png".to_string())].into(),
        body: b"\x89PNG".to_vec(),
        duration_ms: 0,
    };
    write_fixture(
        &tape,
        &HttpRequest::get(HttpService::Notion, signed).plain(),
        &bytes,
    )
    .unwrap();
    run(&tape, &store, &[SICKBAY]).await.unwrap();
    assert!(problems(&store).await.is_empty());
    let db = RawDb::open(&store).await.unwrap();
    assert!(db.blob_exists(slot).await.unwrap());
    db.close().await;
}

/// A comments listing that failed was swallowed as "no comments".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn comments_that_did_not_list_are_a_warning_on_the_page_until_they_do() {
    let d = tempdir().unwrap();
    let (tape, store) = (d.path().join("tape"), d.path().join("s.doltlite_db"));
    serve_object(&tape, BRIDGE, EDITED);
    serve_body(&tape, BRIDGE, "Captain's log.\n", false);

    run(&tape, &store, &[BRIDGE]).await.unwrap();
    assert_eq!(
        problems(&store).await,
        vec![row(&format!("pages:{BRIDGE}"), "warning")]
    );

    serve_comments(&tape, BRIDGE);
    run(&tape, &store, &[BRIDGE]).await.unwrap();
    assert!(problems(&store).await.is_empty());
}

/// A truncated subtree whose follow-up failed left the body incomplete
/// for good, with nothing but a log line to say so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subtree_that_did_not_fetch_is_a_problem_until_it_does() {
    let d = tempdir().unwrap();
    let (tape, store) = (d.path().join("tape"), d.path().join("s.doltlite_db"));
    let hole = "1701d000-0000-4000-8000-0000000000aa";
    let marker = format!(
        "<unknown url=\"https://www.notion.so/x#{}\"/>",
        hole.replace('-', "")
    );
    serve_object(&tape, HOLODECK, EDITED);
    serve_body(&tape, HOLODECK, &format!("Program list\n{marker}\n"), true);
    serve_comments(&tape, HOLODECK);

    run(&tape, &store, &[HOLODECK]).await.unwrap();
    assert_eq!(
        problems(&store).await,
        vec![row(&format!("page_markdown:{HOLODECK}"), "warning")]
    );

    serve_body(&tape, hole, "Dixon Hill\n", false);
    run(&tape, &store, &[HOLODECK]).await.unwrap();
    assert!(problems(&store).await.is_empty());
    let db = RawDb::open(&store).await.unwrap();
    let bodies = db.load_page_markdown().await.unwrap();
    db.close().await;
    assert!(bodies[0].1.contains("Dixon Hill"), "{bodies:?}");
}

/// A user that could not be read was only logged, and never asked for
/// again unless a page naming it changed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_user_that_did_not_fetch_is_retried_every_run() {
    let d = tempdir().unwrap();
    let (tape, store) = (d.path().join("tape"), d.path().join("s.doltlite_db"));
    let riker = "1701d000-0000-4000-8000-0000000000bb";
    let mut obj = page(BRIDGE, EDITED);
    obj["created_by"] = json!({"object": "user", "id": riker});
    serve(&tape, &format!("{BASE}/pages/{BRIDGE}"), obj);
    serve_body(&tape, BRIDGE, "", false);
    serve_comments(&tape, BRIDGE);

    run(&tape, &store, &[BRIDGE]).await.unwrap();
    assert_eq!(
        problems(&store).await,
        vec![row(&format!("users:{riker}"), "error")]
    );

    serve(
        &tape,
        &format!("{BASE}/users/{riker}"),
        json!({"object": "user", "id": riker, "name": "William Riker"}),
    );
    run(&tape, &store, &[BRIDGE]).await.unwrap();
    assert!(problems(&store).await.is_empty());
}

/// A configured root Notion does not have was only a `pages:` row, which
/// does not say the config is what needs fixing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_root_notion_does_not_have_is_a_config_problem() {
    let d = tempdir().unwrap();
    let (tape, store) = (d.path().join("tape"), d.path().join("s.doltlite_db"));
    serve_page(&tape, BRIDGE, EDITED, "");
    let missing = HttpResponse {
        status: 404,
        body: br#"{"object":"error","status":404,"code":"object_not_found"}"#.to_vec(),
        ..json_response(&json!({}))
    };
    write_fixture(&tape, &get(&format!("{BASE}/pages/{SICKBAY}")), &missing).unwrap();

    run(&tape, &store, &[BRIDGE, SICKBAY]).await.unwrap();
    assert_eq!(
        problems(&store).await,
        vec![
            row(&format!("config:roots:{SICKBAY}"), "warning"),
            row(&format!("pages:{SICKBAY}"), "error"),
        ]
    );

    run(&tape, &store, &[BRIDGE]).await.unwrap();
    assert_eq!(
        problems(&store).await,
        vec![row(&format!("pages:{SICKBAY}"), "error")],
        "the config row goes with the entry; the page's own row stands until it fetches"
    );
}

/// A search that failed past its first page failed the whole step, and
/// would have moved the resume cursor past what it never read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_search_cut_short_keeps_its_pages_and_holds_the_cursor() {
    let d = tempdir().unwrap();
    let (tape, store) = (d.path().join("tape"), d.path().join("s.doltlite_db"));
    serve_page(&tape, BRIDGE, EDITED, "");
    serve_search(&tape, None, json!([page(BRIDGE, EDITED)]), Some("page-2"));

    let summary = run(&tape, &store, &[]).await.unwrap();
    assert_eq!(summary.new_pages, 1);
    assert_eq!(problems(&store).await, vec![row("listing:search", "error")]);
    let db = RawDb::open(&store).await.unwrap();
    let cursor = datalib_etl::doltlite_raw::load_scope_state(db.pool())
        .await
        .unwrap();
    db.close().await;
    assert!(cursor.is_empty(), "the cursor moved: {cursor:?}");

    serve_search(&tape, Some("page-2"), json!([]), None);
    run(&tape, &store, &[]).await.unwrap();
    assert!(problems(&store).await.is_empty());
}

/// Search names only what moved since the resume cursor, so a page that
/// failed and has not moved since was never asked for again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_mode_retries_a_page_that_failed() {
    let d = tempdir().unwrap();
    let (tape, store) = (d.path().join("tape"), d.path().join("s.doltlite_db"));
    let earlier = "2026-08-01T00:00:00.000Z";
    serve_page(&tape, BRIDGE, EDITED, "");
    serve_search(
        &tape,
        None,
        json!([page(BRIDGE, EDITED), page(SICKBAY, earlier)]),
        None,
    );
    run(&tape, &store, &[]).await.unwrap();
    assert_eq!(
        problems(&store).await,
        vec![row(&format!("pages:{SICKBAY}"), "error")]
    );

    // Upstream: one newer edit, then only what the cursor already covers.
    let later = "2026-09-02T00:00:00.000Z";
    serve_page(&tape, TEN_FORWARD, later, "");
    serve_search(
        &tape,
        None,
        json!([page(TEN_FORWARD, later), page(SICKBAY, earlier)]),
        None,
    );
    serve_page(&tape, SICKBAY, earlier, "");
    run(&tape, &store, &[]).await.unwrap();
    assert!(problems(&store).await.is_empty());
}
