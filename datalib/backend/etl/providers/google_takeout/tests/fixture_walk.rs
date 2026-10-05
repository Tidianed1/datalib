//! End-to-end fixture walk: point the extractor at the checked-in
//! TNG-themed Takeout tree and assert each feed lands the rows
//! the provider's INGEST.md promises.

use std::path::{Path, PathBuf};

use datalib_etl::fingerprint_cache::FingerprintCache;
use datalib_etl::progress::Progress;
use datalib_etl::store_handle::RawStoreHandle;
use datalib_etl_google_takeout::ingest::{self, FetchOptions, RawDb, SyncFlags};

fn fixture_root() -> PathBuf {
    let rel =
        std::env::var("TAKEOUT_FIXTURE_DIR").expect("TAKEOUT_FIXTURE_DIR must be set by the build");
    // Under `bazel test` the runfiles root is CWD; under `cargo test`
    // the env var is repo-relative from the workspace root.
    let p = PathBuf::from(&rel);
    if p.is_dir() {
        return p;
    }
    let up = PathBuf::from("../../../../..").join(&rel);
    assert!(up.is_dir(), "fixture dir not found: {rel}");
    up
}

/// A temp cache per run: tests must never touch this host's real one.
async fn opts(work: &Path, db: &RawDb, sync: SyncFlags) -> FetchOptions {
    FetchOptions {
        db: db.clone(),
        input_path: fixture_root(),
        cache: FingerprintCache::open(&work.join("fingerprints.sqlite"))
            .await
            .unwrap(),
        sync,
        progress: Progress::noop(),
        control: Default::default(),
    }
}

async fn run_all() -> (tempfile::TempDir, ingest::FetchSummary, PathBuf) {
    let work = tempfile::tempdir().unwrap();
    let db_path = work.path().join("gt.doltlite_db");
    let db = RawDb::open(&db_path).await.unwrap();
    let summary = ingest::fetch(opts(work.path(), &db, SyncFlags::all()).await)
        .await
        .unwrap();
    // Closed, not dropped: every caller reopens this store, and a
    // dropped pool is still a live connection for a moment.
    db.commit_all("test").await.unwrap();
    db.close().await;
    (work, summary, db_path)
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_reviews_lands_two_rows() {
    let (_work, summary, db_path) = run_all().await;
    assert_eq!(summary.maps_reviews, 2);
    let db = RawDb::open(&db_path).await.unwrap();
    let rows = db.load_payloads("maps_reviews").await.unwrap();
    assert_eq!(rows.len(), 2);
    let names: Vec<String> = rows
        .iter()
        .filter_map(|v| {
            v.get("properties")
                .and_then(|p| p.get("location"))
                .and_then(|l| l.get("name"))
                .and_then(|n| n.as_str())
                .map(str::to_string)
        })
        .collect();
    assert!(names.iter().any(|n| n == "Ten Forward"));
    assert!(names.iter().any(|n| n == "Resort Lounge"));
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_saved_places_handles_ftid_and_cid() {
    let (_work, summary, _db_path) = run_all().await;
    assert_eq!(summary.maps_saved_places, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_photo_lands_row_and_blob() {
    let (_work, summary, db_path) = run_all().await;
    assert_eq!(summary.maps_photos, 1);
    let db = RawDb::open(&db_path).await.unwrap();
    let rows = db.load_payloads("maps_photos").await.unwrap();
    assert_eq!(rows.len(), 1);
    // blake3 column populated from JPEG bytes.
    let blake3: Option<String> = sqlx::query_scalar("SELECT blake3 FROM maps_photos WHERE id = ?")
        .bind("2026-06-04-tenfwd")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let blake3 = blake3.expect("blake3 populated");
    assert_eq!(blake3.len(), 64);
    // CAS has the bytes.
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM cas_objects WHERE blake3 = ?)")
            .bind(&blake3)
            .fetch_one(db.cas().pool())
            .await
            .unwrap();
    assert!(exists, "photo bytes in CAS");
}

#[tokio::test(flavor = "multi_thread")]
async fn youtube_subscriptions_handles_quoted_titles() {
    let (_work, summary, db_path) = run_all().await;
    assert_eq!(summary.youtube_subscriptions, 3);
    let db = RawDb::open(&db_path).await.unwrap();
    let title: String = sqlx::query_scalar(
        "SELECT channel_title FROM youtube_subscriptions WHERE id = 'UCriker002'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(title, "Riker, William T.");
}

#[tokio::test(flavor = "multi_thread")]
async fn youtube_watch_history_parses_cells_and_timestamps() {
    let (_work, summary, db_path) = run_all().await;
    assert_eq!(summary.youtube_watch_history, 3);
    let db = RawDb::open(&db_path).await.unwrap();
    // video_id promoted column populated for each row.
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM youtube_watch_history WHERE video_id IS NOT NULL")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 3);
    let when: Option<String> = sqlx::query_scalar(
        "SELECT when_ts FROM youtube_watch_history WHERE video_id = 'trekS01E01'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    // Parsed PDT-relative; the rfc3339 wallclock is the local 11:48
    // turned into a -07:00 offset.
    assert!(when.unwrap().starts_with("2026-06-04T11:48:37"));
}

/// The fixture's third entry has a multi-byte character just where the
/// timestamp look-back starts, which once panicked the whole ingest.
#[tokio::test(flavor = "multi_thread")]
async fn a_multibyte_char_before_the_timestamp_lands_with_it() {
    let html = std::fs::read_to_string(fixture_root().join(WATCH_HISTORY)).unwrap();
    let cell = ingest::mdl_html::iter_cells(&html)
        .find(|c| c.contains("trekS04E02"))
        .unwrap();
    let text = ingest::mdl_html::strip_tags(cell);
    assert!(
        !text.is_char_boundary(text.find(" AM ").unwrap() - 30),
        "the fixture must put the look-back inside the 'ü'"
    );

    let (_work, _summary, db_path) = run_all().await;
    let db = RawDb::open(&db_path).await.unwrap();
    let when: Option<String> = sqlx::query_scalar(
        "SELECT when_ts FROM youtube_watch_history WHERE video_id = 'trekS04E02'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(when.unwrap().starts_with("2026-06-06T09:00:00"));
}

#[tokio::test(flavor = "multi_thread")]
async fn google_chat_lands_groups_users_messages_and_attachments() {
    let (_work, summary, db_path) = run_all().await;
    assert_eq!(summary.chat_groups, 1);
    assert_eq!(summary.chat_users, 1);
    assert_eq!(summary.chat_messages, 2);
    // The second is named by the message and absent from the export.
    assert_eq!(summary.chat_attachments, 2);
    let db = RawDb::open(&db_path).await.unwrap();
    // The DM group key is the takeout directory name verbatim.
    let group_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM chat_groups ORDER BY id")
        .fetch_all(db.pool())
        .await
        .unwrap();
    assert_eq!(group_ids, vec!["DM TNG-BRIDGE"]);
    // The fetched attachment's edge row has the CAS blake3 set.
    let blake3: Option<String> = sqlx::query_scalar(
        "SELECT blake3 FROM chat_attachments WHERE export_name = 'course-laid-in.txt'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let blake3 = blake3.expect("blake3 set");
    let bytes: Vec<u8> = sqlx::query_scalar("SELECT bytes FROM cas_objects WHERE blake3 = ?")
        .bind(&blake3)
        .fetch_one(db.cas().pool())
        .await
        .unwrap();
    let s = String::from_utf8(bytes).unwrap();
    assert!(s.contains("Course 314"));
}

#[tokio::test(flavor = "multi_thread")]
async fn gemini_apps_lands_two_cells_and_one_attachment() {
    let (_work, summary, db_path) = run_all().await;
    assert_eq!(summary.gemini_activity, 2);
    assert_eq!(summary.gemini_attachments, 1);
    let db = RawDb::open(&db_path).await.unwrap();
    let when_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM gemini_activity WHERE when_ts IS NOT NULL")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(when_count, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn second_run_skips_via_file_checkpoint() {
    let (work, summary1, db_path) = run_all().await;
    assert!(summary1.maps_reviews > 0);
    assert!(summary1.youtube_watch_history > 0);

    // Same fixture root, freshly opened db pool — cursor rows from
    // the first run mean every file's fingerprint matches and the
    // walkers short-circuit.
    let db = RawDb::open(&db_path).await.unwrap();
    let summary2 = ingest::fetch(opts(work.path(), &db, SyncFlags::all()).await)
        .await
        .unwrap();
    db.commit_all("test").await.unwrap();
    db.close().await;
    let _ = work; // keep temp dir alive

    assert_eq!(summary2.maps_reviews, 0);
    assert_eq!(summary2.maps_saved_places, 0);
    assert_eq!(summary2.youtube_watch_history, 0);
    assert_eq!(summary2.youtube_subscriptions, 0);
    assert_eq!(summary2.chat_messages, 0);
    assert_eq!(summary2.gemini_activity, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_flags_default_disables_everything() {
    let work = tempfile::tempdir().unwrap();
    let db_path = work.path().join("gt.doltlite_db");
    let db = RawDb::open(&db_path).await.unwrap();
    // Default SyncFlags has every feed off.
    let summary = ingest::fetch(opts(work.path(), &db, SyncFlags::default()).await)
        .await
        .unwrap();
    db.commit_all("test").await.unwrap();
    db.close().await;
    assert_eq!(summary.maps_reviews, 0);
    assert_eq!(summary.youtube_subscriptions, 0);
    assert_eq!(summary.chat_messages, 0);
    assert_eq!(summary.gemini_activity, 0);
}

/// Voice's `Bills.html` sits at `Voice/Bills.html` under the export root;
/// matching it as a bare `Bills.html` never found it.
#[tokio::test(flavor = "multi_thread")]
async fn google_voice_lands_the_bills() {
    let (_work, summary, _db_path) = run_all().await;
    assert!(summary.voice_bills > 0, "{summary:?}");
}

// ── #898: a file that is gone takes its records with it ─────────────

fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&dest).unwrap();
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

/// A private copy of the fixture export, a store, and a cache, so a test
/// can delete files between syncs.
struct Export {
    work: tempfile::TempDir,
    root: PathBuf,
    db_path: PathBuf,
}

impl Export {
    fn new() -> Self {
        let work = tempfile::tempdir().unwrap();
        let root = work.path().join("Takeout");
        std::fs::create_dir_all(&root).unwrap();
        copy_tree(&fixture_root(), &root);
        let db_path = work.path().join("gt.doltlite_db");
        Self {
            work,
            root,
            db_path,
        }
    }

    async fn sync(&self) -> ingest::FetchSummary {
        let db = RawDb::open(&self.db_path).await.unwrap();
        let summary = ingest::fetch(FetchOptions {
            input_path: self.root.clone(),
            ..opts(self.work.path(), &db, SyncFlags::all()).await
        })
        .await
        .unwrap();
        db.commit_all("test").await.unwrap();
        db.close().await;
        summary
    }

    fn remove(&self, rel: &str) {
        std::fs::remove_file(self.root.join(rel)).unwrap();
    }

    async fn count(&self, table: &str) -> i64 {
        let db = RawDb::open(&self.db_path).await.unwrap();
        let n = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(db.pool())
            .await
            .unwrap();
        db.close().await;
        n
    }
}

const MESSAGES: &str = "Google Chat/Groups/DM TNG-BRIDGE/messages.json";

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_chat_file_takes_its_records() {
    let e = Export::new();
    e.sync().await;
    assert_eq!(e.count("chat_messages").await, 2);
    assert_eq!(e.count("chat_attachments").await, 2);

    e.remove(MESSAGES);
    let s = e.sync().await;
    assert_eq!(s.removed, 2);
    assert_eq!(e.count("chat_messages").await, 0);
    assert_eq!(e.count("chat_attachments").await, 0);
    assert_eq!(
        e.count("chat_groups").await,
        1,
        "group_info.json is still there"
    );

    e.remove("Google Chat/Groups/DM TNG-BRIDGE/group_info.json");
    e.remove("Google Chat/Users/User 1234567890/user_info.json");
    e.sync().await;
    assert_eq!(e.count("chat_groups").await, 0);
    assert_eq!(e.count("chat_users").await, 0);
}

/// A group's `messages.json` is all of its messages, so one a re-read
/// file no longer carries is gone, attachment edge and all.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_dropped_from_a_reread_file_is_gone() {
    let e = Export::new();
    e.sync().await;

    let path = e.root.join(MESSAGES);
    let mut doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    doc["messages"].as_array_mut().unwrap().pop();
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let s = e.sync().await;
    assert_eq!(s.removed, 1);
    assert_eq!(e.count("chat_messages").await, 1);
    assert_eq!(
        e.count("chat_attachments").await,
        0,
        "T2 carried the attachment"
    );
}

/// A `messages.json` with no `messages` array lists nothing, which is not
/// the same as listing no messages.
#[tokio::test(flavor = "multi_thread")]
async fn a_messages_file_without_a_list_deletes_nothing() {
    let e = Export::new();
    e.sync().await;

    std::fs::write(e.root.join(MESSAGES), b"{}").unwrap();
    let s = e.sync().await;
    assert_eq!(s.removed, 0);
    assert_eq!(e.count("chat_messages").await, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_maps_photo_sidecar_takes_its_row() {
    let e = Export::new();
    e.sync().await;
    assert_eq!(e.count("maps_photos").await, 1);

    e.remove("Maps/Photos and videos/2026-06-04-tenfwd.json");
    let s = e.sync().await;
    assert_eq!(s.removed, 1);
    assert_eq!(e.count("maps_photos").await, 0);
}

/// Voice records are keyed by content, so a gone file costs a read of the
/// rest, and only what no remaining file holds goes.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_voice_file_takes_only_its_records() {
    let e = Export::new();
    e.sync().await;
    let before = e.count("voice_messages").await;
    let bills = e.count("voice_bills").await;
    assert!(bills > 0);

    e.remove("Voice/Calls/Wesley Crusher - Missed - 2364-03-03T11_00_00Z.html");
    let s = e.sync().await;
    assert_eq!(s.removed, 1, "{s:?}");
    assert_eq!(e.count("voice_messages").await, before - 1);
    assert_eq!(e.count("voice_bills").await, bills);

    e.remove("Voice/Bills.html");
    e.sync().await;
    assert_eq!(e.count("voice_bills").await, 0);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_walk_error_deletes_nothing() {
    let e = Export::new();
    e.sync().await;

    e.remove(MESSAGES);
    e.remove("Voice/Calls/Wesley Crusher - Missed - 2364-03-03T11_00_00Z.html");
    std::os::unix::fs::symlink(e.root.join("nowhere"), e.root.join("dangling.json")).unwrap();
    let s = e.sync().await;
    assert_eq!(s.removed, 0);
    assert_eq!(e.count("chat_messages").await, 2);
}

// ── A single-file feed's file is the whole of its table ─────────────

const REVIEWS: &str = "Maps (your places)/Reviews.json";
const SAVED: &str = "Maps (your places)/Saved Places.json";
const SUBSCRIPTIONS: &str = "YouTube and YouTube Music/subscriptions/subscriptions.csv";
const WATCH_HISTORY: &str = "YouTube and YouTube Music/history/watch-history.html";
const GEMINI: &str = "My Activity/Gemini Apps/MyActivity.html";

impl Export {
    fn rewrite(&self, rel: &str, edit: impl FnOnce(String) -> String) {
        let path = self.root.join(rel);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, edit(text)).unwrap();
    }
}

fn drop_first_feature(json: String) -> String {
    let mut doc: serde_json::Value = serde_json::from_str(&json).unwrap();
    doc["features"].as_array_mut().unwrap().remove(0);
    doc.to_string()
}

fn drop_last_line(csv: String) -> String {
    let mut lines: Vec<&str> = csv.lines().collect();
    lines.pop();
    lines.join("\n") + "\n"
}

/// Cut the first of the MDL activity cells Takeout's HTML feeds are made of.
fn drop_first_cell(html: String) -> String {
    let marker = "<div class=\"outer-cell";
    let first = html.find(marker).unwrap();
    let second = first + marker.len() + html[first + marker.len()..].find(marker).unwrap();
    format!("{}{}", &html[..first], &html[second..])
}

fn drop_last_cell(html: String) -> String {
    let last = html.rfind("<div class=\"outer-cell").unwrap();
    let end = html.rfind("</body>").unwrap();
    format!("{}{}", &html[..last], &html[end..])
}

#[tokio::test(flavor = "multi_thread")]
async fn a_review_dropped_from_a_newer_export_is_gone() {
    let e = Export::new();
    e.sync().await;
    assert_eq!(e.count("maps_reviews").await, 2);

    e.rewrite(REVIEWS, drop_first_feature);
    let s = e.sync().await;
    assert_eq!(s.removed, 1, "{s:?}");
    assert_eq!(e.count("maps_reviews").await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_saved_place_dropped_from_a_newer_export_is_gone() {
    let e = Export::new();
    e.sync().await;
    assert_eq!(e.count("maps_saved_places").await, 2);

    e.rewrite(SAVED, drop_first_feature);
    let s = e.sync().await;
    assert_eq!(s.removed, 1, "{s:?}");
    assert_eq!(e.count("maps_saved_places").await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_dropped_from_a_newer_export_is_gone() {
    let e = Export::new();
    e.sync().await;
    assert_eq!(e.count("youtube_subscriptions").await, 3);

    e.rewrite(SUBSCRIPTIONS, drop_last_line);
    let s = e.sync().await;
    assert_eq!(s.removed, 1, "{s:?}");
    assert_eq!(e.count("youtube_subscriptions").await, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watch_dropped_from_a_newer_export_is_gone() {
    let e = Export::new();
    e.sync().await;
    assert_eq!(e.count("youtube_watch_history").await, 3);

    e.rewrite(WATCH_HISTORY, drop_first_cell);
    let s = e.sync().await;
    assert_eq!(s.removed, 1, "{s:?}");
    assert_eq!(e.count("youtube_watch_history").await, 2);
}

/// The dropped cell is the one with the attachment, so its CAS edge must
/// go with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_gemini_activity_dropped_from_a_newer_export_is_gone_with_its_attachment() {
    let e = Export::new();
    e.sync().await;
    assert_eq!(e.count("gemini_activity").await, 2);
    assert_eq!(e.count("gemini_attachments").await, 1);

    e.rewrite(GEMINI, drop_first_cell);
    let s = e.sync().await;
    assert_eq!(s.removed, 1, "{s:?}");
    assert_eq!(e.count("gemini_activity").await, 1);
    assert_eq!(e.count("gemini_attachments").await, 0);
}

/// A reviews file with no `features` list says nothing about which reviews
/// exist, which is not the same as listing none.
#[tokio::test(flavor = "multi_thread")]
async fn a_reviews_file_without_a_list_deletes_nothing() {
    let e = Export::new();
    e.sync().await;

    e.rewrite(REVIEWS, |_| "{}".to_string());
    let s = e.sync().await;
    assert_eq!(s.removed, 0, "{s:?}");
    assert_eq!(e.count("maps_reviews").await, 2);
}

/// A single-file feed whose file is missing deletes nothing: an export
/// requested without that product looks exactly like this.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_single_file_feed_deletes_nothing() {
    let e = Export::new();
    e.sync().await;

    for rel in [REVIEWS, SAVED, SUBSCRIPTIONS, WATCH_HISTORY, GEMINI] {
        e.remove(rel);
    }
    let s = e.sync().await;
    assert_eq!(s.removed, 0, "{s:?}");
    assert_eq!(e.count("maps_reviews").await, 2);
    assert_eq!(e.count("maps_saved_places").await, 2);
    assert_eq!(e.count("youtube_subscriptions").await, 3);
    assert_eq!(e.count("youtube_watch_history").await, 3);
    assert_eq!(e.count("gemini_activity").await, 2);
}

// ── A product missing from the export deletes nothing ───────────────

const PRODUCTS: [&str; 3] = ["Google Chat", "Voice", "Maps/Photos and videos"];

impl Export {
    /// Move a product's folder out of the export, as a Takeout requested
    /// without it would be; returns where it went.
    fn set_aside(&self, rel: &str) -> PathBuf {
        let aside = self.work.path().join("aside").join(rel);
        std::fs::create_dir_all(aside.parent().unwrap()).unwrap();
        std::fs::rename(self.root.join(rel), &aside).unwrap();
        aside
    }

    fn put_back(&self, rel: &str, aside: &Path) {
        std::fs::rename(aside, self.root.join(rel)).unwrap();
    }
}

/// An export requested without Chat, Voice or Maps photos looks exactly
/// like one whose product was emptied, so a missing product folder is
/// read as "not exported", never as "deleted".
#[tokio::test(flavor = "multi_thread")]
async fn a_product_missing_from_the_export_deletes_nothing() {
    let e = Export::new();
    e.sync().await;
    let tables = [
        "chat_users",
        "chat_groups",
        "chat_messages",
        "chat_attachments",
        "voice_messages",
        "voice_bills",
        "maps_photos",
    ];
    let mut before = Vec::new();
    for t in tables {
        before.push(e.count(t).await);
    }

    for p in PRODUCTS {
        e.set_aside(p);
    }
    let s = e.sync().await;
    assert_eq!(s.removed, 0, "{s:?}");
    for (t, n) in tables.iter().zip(before) {
        assert_eq!(e.count(t).await, n, "{t}");
    }
}

/// Holding the deletions back keeps the cursor, so a product that comes
/// back smaller still loses what it dropped.
#[tokio::test(flavor = "multi_thread")]
async fn a_product_that_returns_smaller_loses_what_it_dropped() {
    let e = Export::new();
    e.sync().await;
    assert_eq!(e.count("chat_messages").await, 2);

    let aside = e.set_aside("Google Chat");
    e.sync().await;
    std::fs::remove_file(aside.join("Groups/DM TNG-BRIDGE/messages.json")).unwrap();
    e.put_back("Google Chat", &aside);
    let s = e.sync().await;
    assert_eq!(s.removed, 2, "{s:?}");
    assert_eq!(e.count("chat_messages").await, 0);
    assert_eq!(e.count("chat_groups").await, 1);
}

// ── one entry the parser trips on costs only itself ─────────────────

/// A feed that fails costs that feed and nothing else, and says so where
/// the Manage row reads it rather than only in the log.
#[tokio::test(flavor = "multi_thread")]
async fn a_feed_that_fails_is_a_problem_row_and_the_rest_land() {
    let e = Export::new();
    e.rewrite(SAVED, |_| "{ not json".to_string());
    let s = e.sync().await;
    assert_eq!(s.feeds_failed, 1, "{s:?}");
    assert_eq!(s.maps_saved_places, 0, "{s:?}");
    assert_eq!(s.maps_reviews, 2, "{s:?}");
    assert_eq!(s.youtube_watch_history, 3, "{s:?}");

    let db = RawDb::open(&e.db_path).await.unwrap();
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT scope_key, severity, sample FROM problems WHERE scope_key LIKE 'phase:%'",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    db.close().await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "phase:maps_saved_places");
    assert_eq!(rows[0].1, "error");
    assert!(rows[0].2.starts_with("parse Saved Places.json"), "{rows:?}");
}

/// An entry read and not stored once reached only the log, one identical
/// `warn!` per entry: 321 saved places with no key, 36 watch-history
/// entries that are not videos. Each is now a `problems` row.
#[tokio::test(flavor = "multi_thread")]
async fn entries_read_and_not_stored_are_problem_rows() {
    let (_work, summary, db_path) = run_all().await;
    assert_eq!(summary.maps_saved_places, 2);
    assert_eq!(summary.youtube_watch_history, 3);
    let db = RawDb::open(&db_path).await.unwrap();
    let rows: Vec<(String, String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT scope_key, severity, reason, rule, sample FROM problems \
         WHERE scope_key LIKE 'skipped:%' ORDER BY scope_key",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    db.close().await;
    let [place, watch] = rows.as_slice() else {
        panic!("one row per skipped entry: {rows:?}");
    };
    assert!(
        place.0.starts_with("skipped:maps_saved_places:"),
        "{place:?}"
    );
    assert_eq!(
        (place.1.as_str(), place.2.as_str()),
        ("error", "no_identity")
    );
    assert!(place.4.contains("?q=Quark"), "{place:?}");
    assert!(
        watch.0.starts_with("skipped:youtube_watch_history:"),
        "{watch:?}"
    );
    assert_eq!(watch.1, "warning");
    assert_eq!(watch.3.as_deref(), Some("youtube_watch_not_a_video"));
    assert!(watch.4.contains("/post/"), "{watch:?}");
}

/// A feed whose file is unchanged reads nothing and reports nothing, so
/// what it skipped last time must still be a row.
#[tokio::test(flavor = "multi_thread")]
async fn an_unchanged_file_keeps_its_skipped_rows() {
    let e = Export::new();
    e.sync().await;
    let skipped = || async {
        let db = RawDb::open(&e.db_path).await.unwrap();
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM problems WHERE scope_key LIKE 'skipped:%'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        db.close().await;
        n
    };
    assert_eq!(skipped().await, 2);
    e.sync().await;
    assert_eq!(skipped().await, 2);

    e.rewrite(WATCH_HISTORY, drop_last_cell);
    e.sync().await;
    assert_eq!(
        skipped().await,
        1,
        "the post left the export, so its row goes"
    );
}
