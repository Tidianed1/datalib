//! The FIT files and wellness bundles: each edge points at its own
//! record's bytes, a file that failed is fetched again however far
//! behind the walk it lies, and an edge an earlier build pointed at
//! another record's file is fetched again too.

use datalib_etl::blob_cas::blake3_hex;

use crate::prune_gate::{bytes, status, Account, PLAYBACK};

const FIT_13: &str = "/download-service/files/activity/17010413001";

fn hash(b: &str) -> Option<String> {
    Some(blake3_hex(b.as_bytes()))
}

fn fit_edges() -> Vec<(String, Option<String>)> {
    vec![
        ("17010413001".into(), hash(".FIT synthetic 17010413001")),
        ("17010414002".into(), hash(".FIT ride of 2369-04-14")),
    ]
}

const FIT_EDGES_SQL: &str =
    "SELECT activity_id, blake3 FROM garmin_activity_files ORDER BY activity_id";

const SHARE_THE_RIDES_FIT: &str = "UPDATE garmin_activity_files SET blake3 = \
     (SELECT blake3 FROM garmin_activity_files WHERE activity_id = '17010414002')";

/// What a store written before the repair looks like: no record of it.
const NOT_YET_REPAIRED: &str =
    "DELETE FROM sync_scope_state WHERE scope LIKE 'garmin:shared_hash_repair:%'";

/// The CAS bundle is keyed by ref, and every FIT went in under the one
/// ref `fit`: each activity's edge got the hash of the last file of
/// the batch, and the earlier files never reached the CAS. A store
/// written that way is mended by fetching the shared ones again — once:
/// a hash two activities share after that is theirs, and fetching it
/// every run would only get the same bytes back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_activitys_fit_edge_points_at_its_own_file() {
    let _serial = PLAYBACK.lock().await;
    let a = Account::tng();
    let s = a.run().await;
    assert_eq!(s.activity_files, 2, "{}", s.line());
    assert_eq!(a.pairs(FIT_EDGES_SQL).await, fit_edges());

    a.exec(SHARE_THE_RIDES_FIT).await;
    a.exec(NOT_YET_REPAIRED).await;
    let s = a.run().await;
    assert_eq!(s.errors, 0, "{}", s.line());
    assert_eq!(
        s.activity_files,
        2,
        "both edges of the shared hash are fetched again: {}",
        s.line()
    );
    assert_eq!(a.pairs(FIT_EDGES_SQL).await, fit_edges());
    assert!(a.problems().await.is_empty(), "{:?}", a.problems().await);

    a.exec(SHARE_THE_RIDES_FIT).await;
    let s = a.run().await;
    assert_eq!(
        s.activity_files,
        0,
        "a store already repaired is not repaired again: {}",
        s.line()
    );
    assert!(a.problems().await.is_empty(), "{:?}", a.problems().await);
}

/// The repair stamps edges failed so a walk fetches them again; with that
/// walk off, nothing would, and the rows would stand for good. So it
/// waits until the walk is on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_repair_waits_for_the_walk_that_would_refetch() {
    let _serial = PLAYBACK.lock().await;
    let mut a = Account::tng();
    a.run().await;
    a.exec(SHARE_THE_RIDES_FIT).await;
    a.exec(NOT_YET_REPAIRED).await;

    a.api.activity_files = Some(false);
    let s = a.run().await;
    assert_eq!(s.errors, 0, "{}", s.line());
    assert!(a.problems().await.is_empty(), "{:?}", a.problems().await);

    a.api.activity_files = None;
    let s = a.run().await;
    assert_eq!(s.activity_files, 2, "{}", s.line());
    assert_eq!(a.pairs(FIT_EDGES_SQL).await, fit_edges());
    assert!(a.problems().await.is_empty(), "{:?}", a.problems().await);
}

/// A FIT was sought only for an activity the run's listing named, so
/// one that failed and then fell behind the listing window was never
/// asked for again, and its `problems` row stood for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_fit_behind_the_listing_window_is_fetched_again() {
    let _serial = PLAYBACK.lock().await;
    let a = Account::tng();
    a.answer_bytes(FIT_13, status(500, "upstream fell over"));
    let s1 = a.run().await;
    assert_eq!(s1.errors, 1, "{}", s1.line());
    assert_eq!(
        a.problems().await.keys().collect::<Vec<_>>(),
        ["garmin_activity_files:17010413001#fit"]
    );

    // The next listing starts the day after that activity.
    a.resynthesize();
    a.set_cursor("garmin:activities", "2369-04-21").await;
    let later = serde_json::Value::Array(vec![a.spec["activities"][1]["listing"].clone()]);
    a.answer(
        "/activitylist-service/activities/search/activities?start=0&limit=100&startDate=2369-04-14",
        status(200, &later.to_string()),
    );
    let s2 = a.run().await;
    assert_eq!(s2.errors, 0, "{}", s2.line());
    assert_eq!(s2.activities_listed, 1, "{}", s2.line());
    assert_eq!(s2.activity_files, 1, "{}", s2.line());
    assert!(a.problems().await.is_empty(), "{:?}", a.problems().await);
    assert_eq!(a.pairs(FIT_EDGES_SQL).await, fit_edges());
}

fn wellness(day: &str) -> String {
    format!("/download-service/files/wellness/{day}")
}

const WELLNESS_EDGES_SQL: &str =
    "SELECT calendar_date, blake3 FROM garmin_wellness_files ORDER BY calendar_date";

/// The wellness cursor walked past a day whose bundle failed; past the
/// refresh window nothing asked for it again. And, as with the FIT
/// files, every bundle of a batch shared one ref.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_wellness_day_behind_the_cursor_is_fetched_again() {
    let _serial = PLAYBACK.lock().await;
    let mut a = Account::tng();
    a.api.wellness_files = Some(true);
    for day in 1..=15 {
        a.answer_bytes(&wellness(&format!("2369-04-{day:02}")), status(404, ""));
    }
    a.answer_bytes(&wellness("2369-04-02"), bytes(b"bundle of 2369-04-02"));
    a.answer_bytes(&wellness("2369-04-03"), status(500, "upstream fell over"));
    a.answer_bytes(&wellness("2369-04-04"), bytes(b"bundle of 2369-04-04"));
    let s1 = a.run().await;
    assert_eq!(s1.errors, 1, "{}", s1.line());
    assert_eq!(s1.wellness_files, 2, "{}", s1.line());
    assert_eq!(
        a.problems().await.keys().collect::<Vec<_>>(),
        ["garmin_wellness_files:2369-04-03#wellness_zip"]
    );
    assert_eq!(
        a.pairs(WELLNESS_EDGES_SQL).await,
        [
            ("2369-04-02".to_string(), hash("bundle of 2369-04-02")),
            ("2369-04-03".to_string(), None),
            ("2369-04-04".to_string(), hash("bundle of 2369-04-04")),
        ]
    );

    a.answer_bytes(&wellness("2369-04-03"), bytes(b"bundle of 2369-04-03"));
    let s2 = a.run().await;
    assert_eq!(s2.errors, 0, "{}", s2.line());
    assert_eq!(s2.wellness_files, 1, "{}", s2.line());
    assert!(a.problems().await.is_empty(), "{:?}", a.problems().await);
    assert_eq!(
        a.pairs(WELLNESS_EDGES_SQL).await[1],
        ("2369-04-03".to_string(), hash("bundle of 2369-04-03"))
    );
}

/// A download that is not a zip comes back the same every time, so it is
/// not asked for again run after run; an edit to the activity is the
/// one thing that could change it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreadable_fit_is_fetched_again_only_when_its_activity_changes() {
    let _serial = PLAYBACK.lock().await;
    let mut a = Account::tng();
    a.answer_bytes(FIT_13, bytes(b"not a zip, captain"));
    let s1 = a.run().await;
    assert_eq!(s1.errors, 1, "{}", s1.line());
    let key = "garmin_activity_files:17010413001#fit";
    assert_eq!(a.problems().await.keys().collect::<Vec<_>>(), [key]);

    let s2 = a.run().await;
    assert_eq!(
        (s2.errors, s2.activity_files),
        (0, 0),
        "not asked for again: {}",
        s2.line()
    );
    assert_eq!(a.problems().await.keys().collect::<Vec<_>>(), [key]);

    a.spec["activities"][0]["listing"]["activityName"] = "Holodeck run: Dixon Hill, again".into();
    a.resynthesize();
    let s3 = a.run().await;
    assert_eq!((s3.errors, s3.activity_files), (0, 1), "{}", s3.line());
    assert!(a.problems().await.is_empty(), "{:?}", a.problems().await);
}
