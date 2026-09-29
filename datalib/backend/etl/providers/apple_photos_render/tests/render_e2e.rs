//! Mirror the TNG library, render it, and pin the summary page: its
//! count is the library's photos and videos.

use std::path::Path;
use std::time::Instant;

use datalib_etl_apple_photos::processor::mirror_options;
use datalib_etl_apple_photos_config::ApplePhotosConfig;
use datalib_etl_apple_photos_render::render::{document_uuid, parse, render_all};
use datalib_etl_render::grid_index::RenderedMarkdown;
use datalib_etl_render::inputs::RawRange;

const SOURCE: &str = "apple_photos";

async fn ingest(library: &str, raw: &Path) {
    let config: ApplePhotosConfig =
        serde_json::from_value(serde_json::json!({ "library": { "path": library } })).unwrap();
    datalib_etl_apple_photos::ingest::fetch_and_commit(
        &datalib_etl::raw_layout::entities_db(raw),
        mirror_options(&config).unwrap(),
        SOURCE,
        Instant::now(),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tng_library_renders_one_page_counting_its_assets() {
    let td = tempfile::tempdir().unwrap();
    let raw = td.path().join(SOURCE).join("ingest");
    std::fs::create_dir_all(&raw).unwrap();
    ingest(&std::env::var("APPLE_PHOTOS_TNG_DB").unwrap(), &raw).await;

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
    // The four ZASSET rows `make_apple_photos_library.py` writes, none
    // of them in the Recently Deleted album.
    assert_eq!(row.item_count, Some(4));
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);
    let inputs: Vec<&str> = parsed.inputs.iter().map(|i| i.table.as_str()).collect();
    assert_eq!(inputs, ["ZASSET", "ZGENERICALBUM"]);

    let md =
        std::fs::read_to_string(td.path().join(SOURCE).join("render_markdown/index.md")).unwrap();
    insta::assert_snapshot!("tng_page", md);
}
