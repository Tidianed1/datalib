//! Read a Lightroom mirror, pinned, into the numbers its page shows.
//!
//! The mirror is the catalog's own tables under their own names
//! (`lightroom/INGEST.md`). Every read is a `GROUP BY` over one table:
//! a `dolt_at_` read uses no secondary index, so a join between two
//! of them would be a nested loop over the catalog, and the joins the
//! page needs (a keyword's name, a camera's) are done here instead,
//! over the grouped counts.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use datalib_etl::doltlite_raw::Reader;
use datalib_etl_render::inputs::{Input, RawRange};
use datalib_etl_summary_render::dates::year_of;
use datalib_etl_summary_render::read::Mirror;
use datalib_etl_summary_render::Tally;
use sqlx::sqlite::SqliteRow;
use sqlx::Row;

/// The table whose rows are the catalog's photos.
pub const IMAGES: &str = "Adobe_images";

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    /// The commit everything was read at.
    pub head: String,
    /// The tables the page read, whole.
    pub inputs: Vec<Input>,
    /// Rows of [`IMAGES`]; `None` when the mirror has no such table.
    pub images: Option<i64>,
    pub by_format: Tally,
    pub by_year: Tally,
    pub by_rating: Tally,
    pub by_flag: Tally,
    /// `captureTime`'s range, as the catalog wrote it.
    pub earliest: Option<String>,
    pub latest: Option<String>,
    pub folders: Option<i64>,
    pub collections: Option<i64>,
    pub keywords: Tally,
    pub cameras: Tally,
}

pub fn parse(raw_path: &Path, range: RawRange<'_>) -> Result<Option<Parsed>> {
    datalib_etl_summary_render::read::pinned(raw_path, range, async |reader: &Reader| {
        read(reader).await
    })
}

async fn read(reader: &Reader) -> Result<Parsed> {
    let mut m = Mirror::new(reader);
    let mut parsed = Parsed {
        head: reader.pin().commit().to_string(),
        ..Default::default()
    };
    if let Some(rows) = m
        .rows(IMAGES, |t| {
            format!(
                "SELECT COUNT(*), CAST(MIN(NULLIF(captureTime, '')) AS TEXT), \
                        CAST(MAX(NULLIF(captureTime, '')) AS TEXT) FROM {t}"
            )
        })
        .await?
    {
        let r = &rows[0];
        parsed.images = Some(r.try_get(0)?);
        parsed.earliest = r.try_get(1)?;
        parsed.latest = r.try_get(2)?;
    }
    parsed.by_format = tally(
        m.rows(IMAGES, |t| {
            format!("SELECT CAST(fileFormat AS TEXT) AS k, COUNT(*) FROM {t} GROUP BY k")
        })
        .await?,
        |k| k.unwrap_or_else(|| "(unknown)".to_string()),
    )?;
    parsed.by_year = tally(
        m.rows(IMAGES, |t| {
            format!(
                "SELECT substr(CAST(captureTime AS TEXT), 1, 4) AS k, COUNT(*) FROM {t} GROUP BY k"
            )
        })
        .await?,
        |k| year_of(k.as_deref()),
    )?;
    parsed.by_rating = tally(
        m.rows(IMAGES, |t| {
            format!(
                "SELECT CAST(CAST(rating AS INTEGER) AS TEXT) AS k, COUNT(*) FROM {t} GROUP BY k"
            )
        })
        .await?,
        |k| rating_label(k.as_deref()),
    )?;
    parsed.by_flag = tally(
        m.rows(IMAGES, |t| {
            format!("SELECT CAST(CAST(pick AS INTEGER) AS TEXT) AS k, COUNT(*) FROM {t} GROUP BY k")
        })
        .await?,
        |k| flag_label(k.as_deref()),
    )?;
    parsed.folders = count(&mut m, "AgLibraryFolder").await?;
    parsed.collections = count(&mut m, "AgLibraryCollection").await?;
    parsed.keywords = named_tally(
        m.rows("AgLibraryKeywordImage", |t| {
            format!("SELECT CAST(tag AS TEXT) AS k, COUNT(*) FROM {t} GROUP BY k")
        })
        .await?,
        m.rows("AgLibraryKeyword", |t| {
            format!("SELECT CAST(id_local AS TEXT), CAST(name AS TEXT) FROM {t}")
        })
        .await?,
    )?;
    parsed.cameras = named_tally(
        m.rows("AgHarvestedExifMetadata", |t| {
            format!("SELECT CAST(cameraModelRef AS TEXT) AS k, COUNT(*) FROM {t} GROUP BY k")
        })
        .await?,
        m.rows("AgInternedExifCameraModel", |t| {
            format!("SELECT CAST(id_local AS TEXT), CAST(value AS TEXT) FROM {t}")
        })
        .await?,
    )?;
    parsed.inputs = m.inputs();
    Ok(parsed)
}

async fn count(m: &mut Mirror<'_>, table: &'static str) -> Result<Option<i64>> {
    match m
        .rows(table, |t| format!("SELECT COUNT(*) FROM {t}"))
        .await?
    {
        Some(rows) => Ok(Some(rows[0].try_get(0)?)),
        None => Ok(None),
    }
}

/// `(key, count)` rows, each key through `label`.
fn tally(rows: Option<Vec<SqliteRow>>, label: impl Fn(Option<String>) -> String) -> Result<Tally> {
    let mut t = Tally::default();
    for r in rows.unwrap_or_default() {
        t.add(label(r.try_get(0)?), r.try_get(1)?, 0);
    }
    Ok(t)
}

/// `(id, count)` rows named through `(id, name)` rows; an id with no
/// name (no row, or a NULL key) is left out rather than shown as a
/// number nobody can read.
fn named_tally(counts: Option<Vec<SqliteRow>>, names: Option<Vec<SqliteRow>>) -> Result<Tally> {
    let mut by_id: HashMap<String, String> = HashMap::new();
    for r in names.unwrap_or_default() {
        if let (Some(id), Some(name)) = (r.try_get::<Option<String>, _>(0)?, r.try_get(1)?) {
            by_id.insert(id, name);
        }
    }
    let mut t = Tally::default();
    for r in counts.unwrap_or_default() {
        let id: Option<String> = r.try_get(0)?;
        if let Some(name) = id.and_then(|id| by_id.get(&id)) {
            t.add(name.as_str(), r.try_get(1)?, 0);
        }
    }
    Ok(t)
}

/// Lightroom's stars; 0 and none are both "no rating" in its own UI.
pub fn rating_label(rating: Option<&str>) -> String {
    match rating {
        Some("1") => "1 star".to_string(),
        Some(n @ ("2" | "3" | "4" | "5")) => format!("{n} stars"),
        _ => "Unrated".to_string(),
    }
}

/// `pick`: 1 is a pick, -1 a reject, anything else unflagged.
pub fn flag_label(pick: Option<&str>) -> String {
    match pick {
        Some("1") => "Picked",
        Some("-1") => "Rejected",
        _ => "Unflagged",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratings_and_flags_read_the_way_lightroom_shows_them() {
        assert_eq!(rating_label(Some("5")), "5 stars");
        assert_eq!(rating_label(Some("1")), "1 star");
        assert_eq!(rating_label(Some("0")), "Unrated");
        assert_eq!(rating_label(None), "Unrated");
        assert_eq!(flag_label(Some("-1")), "Rejected");
        assert_eq!(flag_label(Some("0")), "Unflagged");
    }
}
