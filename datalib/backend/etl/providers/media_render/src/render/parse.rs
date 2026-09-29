//! Read a media raw store, pinned, into the numbers its page shows.
//!
//! The page counts paths (`media_files`), not content (`media_items`):
//! two copies of one song are two files a person has, and an item whose
//! last path was deleted stays in `media_items` without being in the
//! library any more. Each file is tallied through the item it points
//! at, so the per-item tables are read once into memory — at item
//! scale, not fsindex's — and `media_files` is streamed.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use datalib_etl::doltlite_raw::Reader;
use datalib_etl_render::inputs::{Input, RawRange};
use datalib_etl_summary_render::dates::year_of;
use datalib_etl_summary_render::Tally;
use futures::TryStreamExt;
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    /// The commit everything was read at.
    pub head: String,
    /// `media_scan_meta.abs_root`: where the tree was scanned from.
    pub roots: Vec<String>,
    pub files: i64,
    /// Distinct contents behind those files.
    pub items: i64,
    pub bytes: i64,
    pub by_class: Tally,
    pub by_format: Tally,
    /// Files with a capture date, by the year it names.
    pub by_year: Tally,
    pub cameras: Tally,
    pub artists: Tally,
    /// The earliest and latest capture dates, as the files wrote them.
    pub earliest: Option<String>,
    pub latest: Option<String>,
    pub playlists: i64,
}

/// The tables the page reads, whole: any row of any of them moving
/// re-renders it.
pub fn inputs() -> Vec<Input> {
    [
        "media_files",
        "media_items",
        "media_visual",
        "media_audio",
        "media_playlists",
        "media_scan_meta",
    ]
    .into_iter()
    .map(Input::whole_table)
    .collect()
}

pub fn parse(raw_path: &Path, range: RawRange<'_>) -> Result<Option<Parsed>> {
    datalib_etl_summary_render::read::pinned(raw_path, range, async |reader: &Reader| {
        read(reader).await
    })
}

struct Item {
    size: i64,
    class: String,
    container: String,
}

#[derive(Default)]
struct Visual {
    captured_at: Option<String>,
    camera: Option<String>,
}

async fn read(reader: &Reader) -> Result<Parsed> {
    let pool = reader.pool();
    let items = load_items(pool).await?;
    let visual = load_visual(pool).await?;
    let artists = load_artists(pool).await?;
    let mut parsed = Parsed {
        head: reader.pin().commit().to_string(),
        roots: sqlx::query_scalar(
            "SELECT abs_root FROM pinned_media_scan_meta media_scan_meta ORDER BY abs_root",
        )
        .fetch_all(pool)
        .await
        .context("read media_scan_meta")?,
        playlists: sqlx::query_scalar(
            "SELECT COUNT(*) FROM pinned_media_playlists media_playlists",
        )
        .fetch_one(pool)
        .await
        .context("count media_playlists")?,
        ..Default::default()
    };

    let mut seen: HashSet<String> = HashSet::new();
    let mut rows = sqlx::query("SELECT blake3 FROM pinned_media_files media_files").fetch(pool);
    while let Some(r) = rows.try_next().await.context("read media_files")? {
        let blake3: String = r.try_get("blake3")?;
        parsed.files += 1;
        if let Some(item) = items.get(&blake3) {
            parsed.bytes += item.size;
            parsed.by_class.add(item.class.as_str(), 1, item.size);
            parsed.by_format.add(item.container.as_str(), 1, item.size);
        }
        if let Some(v) = visual.get(&blake3) {
            if let Some(at) = &v.captured_at {
                parsed.by_year.add(year_of(Some(at)), 1, 0);
                widen(&mut parsed.earliest, &mut parsed.latest, at);
            }
            if let Some(camera) = &v.camera {
                parsed.cameras.add(camera.as_str(), 1, 0);
            }
        }
        if let Some(artist) = artists.get(&blake3) {
            parsed.artists.add(artist.as_str(), 1, 0);
        }
        seen.insert(blake3);
    }
    parsed.items = seen.len() as i64;
    Ok(parsed)
}

async fn load_items(pool: &SqlitePool) -> Result<HashMap<String, Item>> {
    sqlx::query("SELECT blake3, size, media_class, container FROM pinned_media_items media_items")
        .fetch_all(pool)
        .await
        .context("read media_items")?
        .into_iter()
        .map(|r| {
            Ok((
                r.try_get("blake3")?,
                Item {
                    size: r.try_get("size")?,
                    class: r.try_get("media_class")?,
                    container: r.try_get("container")?,
                },
            ))
        })
        .collect()
}

async fn load_visual(pool: &SqlitePool) -> Result<HashMap<String, Visual>> {
    sqlx::query(
        "SELECT blake3, captured_at, camera_make, camera_model \
           FROM pinned_media_visual media_visual",
    )
    .fetch_all(pool)
    .await
    .context("read media_visual")?
    .into_iter()
    .map(|r| {
        Ok((
            r.try_get("blake3")?,
            Visual {
                captured_at: r.try_get("captured_at")?,
                camera: camera_label(r.try_get("camera_make")?, r.try_get("camera_model")?),
            },
        ))
    })
    .collect()
}

/// An item's artist: the album artist where the tags name one, since
/// that is who a library is browsed by.
async fn load_artists(pool: &SqlitePool) -> Result<HashMap<String, String>> {
    let rows = sqlx::query(
        "SELECT blake3, coalesce(nullif(album_artist, ''), nullif(artist, '')) AS who \
           FROM pinned_media_audio media_audio",
    )
    .fetch_all(pool)
    .await
    .context("read media_audio")?;
    let mut out = HashMap::new();
    for r in rows {
        if let Some(who) = r.try_get::<Option<String>, _>("who")? {
            out.insert(r.try_get("blake3")?, who);
        }
    }
    Ok(out)
}

/// `Canon` + `Canon EOS R5` is `Canon EOS R5`; a model that does not
/// repeat its make gets it in front.
pub fn camera_label(make: Option<String>, model: Option<String>) -> Option<String> {
    let make = make.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let model = model
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    match (make, model) {
        (Some(make), Some(model)) if model.to_lowercase().starts_with(&make.to_lowercase()) => {
            Some(model)
        }
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (make, model) => make.or(model),
    }
}

/// Stretch `[earliest, latest]` to cover `at`, comparing as text: the
/// files write ISO 8601, some with an offset and some without, so this
/// is the order a person reading them would give.
pub fn widen(earliest: &mut Option<String>, latest: &mut Option<String>, at: &str) {
    if earliest.as_deref().is_none_or(|e| at < e) {
        *earliest = Some(at.to_string());
    }
    if latest.as_deref().is_none_or(|l| at > l) {
        *latest = Some(at.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_camera_is_named_once() {
        let label = |make: &str, model: &str| {
            camera_label(
                Some(make.to_string()).filter(|s| !s.is_empty()),
                Some(model.to_string()).filter(|s| !s.is_empty()),
            )
        };
        assert_eq!(
            label("Canon", "Canon EOS R5").as_deref(),
            Some("Canon EOS R5")
        );
        assert_eq!(label("SONY", "ILCE-7M3").as_deref(), Some("SONY ILCE-7M3"));
        assert_eq!(label("", "iPhone 15").as_deref(), Some("iPhone 15"));
        assert_eq!(label("", ""), None);
    }
}
