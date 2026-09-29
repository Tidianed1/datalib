//! Scan the TNG media corpus, render it, and pin the summary page: its
//! count is files, so the archived copy of a song counts beside the
//! original.

use std::path::{Path, PathBuf};

use datalib_etl::fingerprint_cache::FingerprintCache;
use datalib_etl::progress::Progress;
use datalib_etl_media::ingest::{self, FetchOptions, RawDb};
use datalib_etl_media_render::render::{document_uuid, parse, render_all};
use datalib_etl_render::grid_index::RenderedMarkdown;
use datalib_etl_render::inputs::RawRange;

const SOURCE: &str = "media";

async fn ingest(tree: &Path, raw: &Path) {
    let db = RawDb::open(&ingest::db_path_for(raw)).await.unwrap();
    let cache = FingerprintCache::open(&raw.join("fingerprints.sqlite"))
        .await
        .unwrap();
    ingest::fetch(FetchOptions {
        db: db.clone(),
        source_id: SOURCE.to_string(),
        root: tree.to_path_buf(),
        ignore: vec![],
        cache,
        max_bytes: None,
        payload_max_bytes: None,
        playlists: true,
        skip_dataless: true,
        now: "2364-04-13T08:45:00-07:00".to_string(),
        progress: Progress::noop(),
    })
    .await
    .unwrap();
    datalib_etl::doltlite_raw::commit_run(db.pool(), "test ingest")
        .await
        .unwrap();
    db.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tng_corpus_renders_one_page_counting_its_files() {
    let td = tempfile::tempdir().unwrap();
    let tree = PathBuf::from(std::env::var("MEDIA_FIXTURE_DIR").unwrap());
    let raw = td.path().join(SOURCE).join("ingest");
    std::fs::create_dir_all(&raw).unwrap();
    ingest(&tree, &raw).await;

    let parsed = parse::parse(&raw, RawRange::cold())
        .unwrap()
        .expect("a committed store");
    let mut emitted: Vec<RenderedMarkdown> = Vec::new();
    render_all(&parsed, td.path(), SOURCE, &mut |md| {
        emitted.push(md);
        Ok(())
    })
    .unwrap();

    let [doc] = emitted.as_slice() else {
        panic!("one page, got {}", emitted.len())
    };
    assert_eq!(doc.markdown_uuid, document_uuid(SOURCE));
    let [row] = doc.rows.as_slice() else {
        panic!("one row, got {:?}", doc.rows)
    };
    assert!(row.is_document);
    // Every media file under the corpus, the archived copy of
    // ode_to_spot.mp3 included; the playlists and readme.txt are not
    // media files.
    assert_eq!(row.item_count, Some(parsed.files));
    assert!(parsed.items < parsed.files, "{parsed:?}");
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);

    let mut md =
        std::fs::read_to_string(td.path().join(SOURCE).join("render_markdown/index.md")).unwrap();
    let abs = std::path::absolute(&tree).unwrap();
    for spelling in [tree.canonicalize().unwrap(), abs] {
        md = md.replace(spelling.to_str().unwrap(), "<scan root>");
    }
    insta::assert_snapshot!("tng_page", md);
}
