//! A full Fastmail (JMAP) re-list is the one moment the mirror can see
//! what was destroyed while no cursor was replaying: a mailbox the full
//! `Mailbox/get` does not name comes off every email and goes, and an
//! email the finished, unfiltered `Email/query` does not name goes.

use datalib_etl::http::{HttpRequest, HttpResponse, HttpService};
use datalib_etl::synthesize::{json_response, write_fixture};
use datalib_etl_email::ingest::session::Session;
use datalib_etl_email::ingest::{api, FetchOptions, FetchSummary, RawDb};
use serde_json::{json, Value};

use crate::support::Mirror;

const HOST: &str = "jmap.example.test";
const ACCOUNT: &str = "A1";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_resync_drops_what_upstream_destroyed() {
    let m = Mirror::new();
    write_run(
        &m.playback,
        &[("MB1", "Inbox"), ("MB2", "Work")],
        &[("M1", &["MB1", "MB2"]), ("M2", &["MB2"])],
    );
    let first = run(&m).await;
    assert_eq!(first.emails_upserted, 2, "{first:?}");

    // `Work` was destroyed, and `M2` with it; `M1` is only in the Inbox.
    write_run(&m.playback, &[("MB1", "Inbox")], &[("M1", &["MB1"])]);
    let second = run(&m).await;
    assert_eq!(second.mailboxes_destroyed, 1, "{second:?}");
    assert_eq!(second.emails_destroyed, 1, "{second:?}");

    let (mailboxes, emails, joins) = m
        .read(|db: RawDb| async move {
            let mailboxes: Vec<String> = db
                .mailbox_names(ACCOUNT)
                .await
                .unwrap()
                .into_keys()
                .collect();
            let emails: Vec<String> = sqlx::query_scalar("SELECT id FROM emails ORDER BY id")
                .fetch_all(db.pool())
                .await
                .unwrap();
            let joins = db.load_email_joins().await.unwrap().mailboxes;
            (mailboxes, emails, joins)
        })
        .await;
    assert_eq!(mailboxes, vec!["MB1"]);
    assert_eq!(emails, vec!["M1"]);
    assert_eq!(joins["M1"], vec!["MB1"]);
}

async fn run(m: &Mirror) -> FetchSummary {
    m.run(|db| {
        let mut opts = FetchOptions::new(db);
        opts.hostname = HOST.to_string();
        opts.full_resync = true;
        datalib_etl_email::ingest::fetch(opts)
    })
    .await
    .expect("jmap fetch under playback")
}

fn session_json() -> Value {
    json!({
        "apiUrl": "https://jmap.example.test/jmap/api/",
        "downloadUrl": "https://jmap.example.test/jmap/download/{accountId}/{blobId}/{name}?type={type}",
        "uploadUrl": "https://jmap.example.test/jmap/upload/{accountId}/",
        "primaryAccounts": { "urn:ietf:params:jmap:mail": ACCOUNT },
        "accounts": { ACCOUNT: { "name": "t@example.test", "isPersonal": true } },
    })
}

/// The fixtures one full run reads: these mailboxes, these emails.
fn write_run(out: &std::path::Path, mailboxes: &[(&str, &str)], emails: &[(&str, &[&str])]) {
    let session = Session::from_value(session_json()).expect("parse the fixture session");
    let put_call = |method: &str, args: Value, result: &Value| {
        let req = api::method_request(&session, method, args).expect("build the JMAP request");
        let body = json!({ "methodResponses": [[method, result, "a"]] });
        write_fixture(out, &req, &json_response(&body)).expect("write fixture");
    };
    write_fixture(
        out,
        &HttpRequest::get(
            HttpService::Jmap,
            format!("https://{HOST}/.well-known/jmap"),
        ),
        &json_response(&session_json()),
    )
    .expect("write the session fixture");

    put_call(
        "Mailbox/get",
        json!({ "accountId": ACCOUNT, "ids": null }),
        &json!({
            "state": "mbox-1",
            "list": mailboxes
                .iter()
                .map(|(id, name)| json!({ "id": id, "name": name, "parentId": null }))
                .collect::<Vec<_>>(),
        }),
    );

    let ids: Vec<&str> = emails.iter().map(|(id, _)| *id).collect();
    put_call(
        "Email/query",
        json!({
            "accountId": ACCOUNT,
            "sort": [{ "property": "receivedAt", "isAscending": false }],
            "limit": 500,
            "position": 0,
            "calculateTotal": true,
        }),
        &json!({ "ids": ids, "queryState": "q-1", "total": ids.len() }),
    );
    put_call(
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
        &json!({
            "state": "email-1",
            "list": emails.iter().map(|(id, filed)| email(id, filed)).collect::<Vec<_>>(),
        }),
    );
    let thread_ids: Vec<String> = ids.iter().map(|id| format!("T{id}")).collect();
    put_call(
        "Thread/get",
        json!({ "accountId": ACCOUNT, "ids": thread_ids }),
        &json!({
            "state": "thread-1",
            "list": ids
                .iter()
                .map(|id| json!({ "id": format!("T{id}"), "emailIds": [id] }))
                .collect::<Vec<_>>(),
        }),
    );
    for id in ids {
        let url =
            session.download_url_for(ACCOUNT, &format!("B{id}"), "message.eml", "message/rfc822");
        write_fixture(out, &HttpRequest::get(HttpService::Jmap, url), &eml(id))
            .expect("write a blob fixture");
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
        "subject": format!("message {id}"),
        "receivedAt": "2026-09-01T10:00:00Z",
        "size": 256,
        "hasAttachment": false,
    })
}

fn eml(id: &str) -> HttpResponse {
    let body = format!(
        "Message-ID: <{id}@example.test>\r\n\
         Date: Tue, 1 Sep 2026 10:00:00 +0000\r\n\
         From: sender@example.test\r\n\
         Subject: message {id}\r\n\
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
