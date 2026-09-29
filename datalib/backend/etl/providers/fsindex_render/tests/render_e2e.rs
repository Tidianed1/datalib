//! Scan the TNG tree, render it, and pin the summary page: its count is
//! the tree's files, not its folders.

use std::path::{Path, PathBuf};

use datalib_etl::control::DownloadControl;
use datalib_etl::fingerprint_cache::FingerprintCache;
use datalib_etl::progress::Progress;
use datalib_etl_fsindex::ingest::{self, FetchOptions, RawDb};
use datalib_etl_fsindex_render::render::{document_uuid, parse, render_all};
use datalib_etl_render::grid_index::RenderedMarkdown;
use datalib_etl_render::inputs::RawRange;

const SOURCE: &str = "fsindex";

/// Bazel stages runfiles as symlinks, and fsindex tells a symlink from
/// a file, so the tree is copied out as real files first.
fn copy_deref(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let from = entry.unwrap().path();
        let to = dst.join(from.file_name().unwrap());
        if std::fs::metadata(&from).unwrap().is_dir() {
            copy_deref(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

async fn ingest(tree: &Path, raw: &Path) {
    let db = RawDb::open(&datalib_etl::raw_layout::entities_db(raw))
        .await
        .unwrap();
    let cache = FingerprintCache::open(&raw.join("fingerprints.sqlite"))
        .await
        .unwrap();
    let s = ingest::fetch(FetchOptions {
        db: db.clone(),
        source_id: SOURCE.to_string(),
        root: tree.to_path_buf(),
        target_doltlite_branch: None,
        cache,
        no_stamp: true,
        progress: Progress::noop(),
        control: DownloadControl::default(),
    })
    .await
    .unwrap();
    assert_eq!(s.errors, 0);
    datalib_etl::doltlite_raw::commit_run(db.pool(), "test ingest")
        .await
        .unwrap();
    db.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tng_tree_renders_one_page_counting_its_files() {
    let td = tempfile::tempdir().unwrap();
    let tree = td.path().join("tree");
    copy_deref(
        &PathBuf::from(std::env::var("FSINDEX_TNG_DIR").unwrap()),
        &tree,
    );
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
    // captains_log, crew_manifest, bridge/viewscreen and
    // holodeck/program_picard; helm.tmp is ignored and the three
    // directories are not files.
    assert_eq!(row.item_count, Some(4));
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);

    let mut md =
        std::fs::read_to_string(td.path().join(SOURCE).join("render_markdown/index.md")).unwrap();
    // The canonical spelling first: on macOS it is the other one with
    // `/private` in front.
    for spelling in [tree.canonicalize().unwrap(), tree] {
        md = md.replace(spelling.to_str().unwrap(), "<scan root>");
    }
    insta::assert_snapshot!("tng_page", md);
}
