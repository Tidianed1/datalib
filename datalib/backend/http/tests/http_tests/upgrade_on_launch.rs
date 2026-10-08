//! A launch on a root another build wrote: the server migrates the raw
//! stores that build left, with no download, before its loop takes any
//! request (`docs/dev/plans/upgrade_on_launch.md`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::Request;
use datalib_http::{router, ApiToken, AppState};
use datalib_store_meta::{StoreKind, Versions};
use tower::ServiceExt;

const TOKEN: &str = "upgrade-on-launch-test-token";

/// Stands in for `datalib-step`: says which mode it was invoked in, and
/// writes its tree only when it syncs.
const FAKE_STEP: &str = r#"#!/bin/sh
if [ -n "$DATALIB_DAG_MIGRATE" ]; then mode=migrate; else mode=sync; fi
echo "$mode $DATALIB_DAG_STEP" >> "$DATALIB_DAG_DATA_ROOT/invoked"
if [ "$mode" = sync ]; then
    mkdir -p "$DATALIB_DAG_DATA_ROOT/$DATALIB_DAG_STEP"
    echo x > "$DATALIB_DAG_DATA_ROOT/$DATALIB_DAG_STEP/f"
fi
"#;

fn fake_step(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("datalib-step");
    std::fs::write(&path, FAKE_STEP).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn ingest_group(group: &str, step: &Path) -> String {
    format!(
        "[[groups]]\nid = \"{group}\"\n\n\
         [[steps]]\ngroup = \"{group}\"\nfunction = \"ingest\"\ncommand = \"{}\"\n\n",
        step.display()
    )
}

/// A raw store whose `_datalib_meta` names `version`, or this build.
async fn raw_store(root: &Path, group: &str, version: Option<&str>) {
    let dir = root.join(group).join("ingest");
    std::fs::create_dir_all(&dir).unwrap();
    let url = format!(
        "sqlite://{}?mode=rwc",
        dir.join("entities.doltlite_db").display()
    );
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect(&url)
        .await
        .unwrap();
    datalib_store_meta::write(&pool, StoreKind::Raw, "shape", Versions::default())
        .await
        .unwrap();
    if let Some(version) = version {
        sqlx::query("UPDATE _datalib_meta SET value = ? WHERE key = 'datalib_version'")
            .bind(version)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("SELECT dolt_commit('-Am', 'a raw store')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

fn invoked(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join("invoked"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

async fn config(state: &AppState) -> serde_json::Value {
    let req = Request::builder()
        .uri("/api/config")
        .header("x-datalib-token", TOKEN)
        .body(Body::empty())
        .unwrap();
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// The sync opened before the server came up runs only once `a`'s store,
/// which another build wrote, is migrated; `b`'s, which this build wrote,
/// is left alone. `/api/config` then reports the pass as done.
#[tokio::test]
async fn a_launch_migrates_another_builds_raw_stores_before_any_sync() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let step = fake_step(td.path());
    std::fs::write(
        root.join("config.toml"),
        ingest_group("a", &step) + &ingest_group("b", &step),
    )
    .unwrap();
    raw_store(&root, "a", Some("0.0.1")).await;
    raw_store(&root, "b", None).await;
    let mailbox = datalib_dag::supervisor::store::Store::open(&root)
        .await
        .unwrap();
    mailbox
        .open_request(&["a/ingest".to_string()], "ui")
        .await
        .unwrap();
    mailbox.close().await;

    let state =
        datalib_http::build_state(root.clone(), None, None, ApiToken::from_value(TOKEN, &root))
            .await
            .expect("the server boots");

    let deadline = Instant::now() + Duration::from_secs(30);
    while !invoked(&root).contains(&"sync a/ingest".to_string()) {
        assert!(
            Instant::now() < deadline,
            "the sync never ran: {:?}",
            invoked(&root)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(invoked(&root), ["migrate a/ingest", "sync a/ingest"]);
    let upgrade = &config(&state).await["upgrade"];
    assert_eq!(upgrade["migrating"], false, "{upgrade}");
    assert_eq!(
        upgrade["stores"],
        serde_json::json!([{ "step": "a/ingest", "state": "done", "error": null }])
    );
    state.sync.shutdown(Duration::from_secs(10)).await;
}
