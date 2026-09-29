//! Read an Apple Photos mirror, pinned, into the numbers its page
//! shows.
//!
//! The mirror is `Photos.sqlite`'s Core Data tables under their own
//! names (`apple_photos/INGEST.md`). As in the Lightroom render, every
//! read is one table grouped: a `dolt_at_` read uses no secondary
//! index, so a join would be a nested loop over the library.

use std::path::Path;

use anyhow::Result;
use datalib_etl::doltlite_raw::Reader;
use datalib_etl_render::inputs::{Input, RawRange};
use datalib_etl_summary_render::dates::year_of;
use datalib_etl_summary_render::read::Mirror;
use datalib_etl_summary_render::Tally;
use sqlx::sqlite::SqliteRow;
use sqlx::Row;

/// The table whose rows are the library's photos and videos.
pub const ASSETS: &str = "ZASSET";

/// Core Data dates are seconds since 2001-01-01 00:00:00 UTC.
const CORE_DATA_EPOCH_S: f64 = 978_307_200.0;

/// An asset in the Recently Deleted album has a non-zero
/// `ZTRASHEDSTATE`; everything else is in the library.
const IN_LIBRARY: &str = "coalesce(ZTRASHEDSTATE, 0) = 0";

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    /// The commit everything was read at.
    pub head: String,
    /// The tables the page read, whole.
    pub inputs: Vec<Input>,
    /// Assets in the library, the Recently Deleted album not counted;
    /// `None` when the mirror has no [`ASSETS`] table.
    pub assets: Option<i64>,
    pub trashed: i64,
    pub favorites: i64,
    pub hidden: i64,
    pub located: i64,
    pub by_kind: Tally,
    pub by_type: Tally,
    /// By the UTC year each was taken in.
    pub by_year: Tally,
    /// `ZDATECREATED`'s range, in unix milliseconds.
    pub earliest_ms: Option<i64>,
    pub latest_ms: Option<i64>,
    /// The titles of the albums a person made, sorted.
    pub albums: Vec<String>,
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
        .rows(ASSETS, |t| {
            format!(
                "SELECT COUNT(*), CAST(MIN(ZDATECREATED) AS REAL), \
                        CAST(MAX(ZDATECREATED) AS REAL), \
                        coalesce(SUM(ZFAVORITE = 1), 0), coalesce(SUM(ZHIDDEN = 1), 0), \
                        coalesce(SUM(ZLATITUDE IS NOT NULL AND ZLATITUDE != -180.0), 0) \
                   FROM {t} WHERE {IN_LIBRARY}"
            )
        })
        .await?
    {
        let r = &rows[0];
        parsed.assets = Some(r.try_get(0)?);
        parsed.earliest_ms = r.try_get::<Option<f64>, _>(1)?.map(core_data_ms);
        parsed.latest_ms = r.try_get::<Option<f64>, _>(2)?.map(core_data_ms);
        parsed.favorites = r.try_get(3)?;
        parsed.hidden = r.try_get(4)?;
        parsed.located = r.try_get(5)?;
    }
    if let Some(rows) = m
        .rows(ASSETS, |t| {
            format!("SELECT COUNT(*) FROM {t} WHERE NOT ({IN_LIBRARY})")
        })
        .await?
    {
        parsed.trashed = rows[0].try_get(0)?;
    }
    parsed.by_kind = tally(
        m.rows(ASSETS, |t| {
            format!(
                "SELECT CAST(ZKIND AS TEXT) AS k, COUNT(*) FROM {t} WHERE {IN_LIBRARY} GROUP BY k"
            )
        })
        .await?,
        |k| kind_label(k.as_deref()),
    )?;
    parsed.by_type = tally(
        m.rows(ASSETS, |t| {
            format!(
                "SELECT CAST(ZUNIFORMTYPEIDENTIFIER AS TEXT) AS k, COUNT(*) FROM {t} \
                  WHERE {IN_LIBRARY} GROUP BY k"
            )
        })
        .await?,
        |k| type_label(k.as_deref()),
    )?;
    parsed.by_year = tally(
        m.rows(ASSETS, |t| {
            format!(
                "SELECT strftime('%Y', ZDATECREATED + {CORE_DATA_EPOCH_S}, 'unixepoch') AS k, \
                        COUNT(*) FROM {t} WHERE {IN_LIBRARY} GROUP BY k"
            )
        })
        .await?,
        |k| year_of(k.as_deref()),
    )?;
    // Kind 2 is an album a person made; the others are Photos' own
    // (smart albums, folders, the shared-album roots).
    if let Some(rows) = m
        .rows("ZGENERICALBUM", |t| {
            format!(
                "SELECT CAST(ZTITLE AS TEXT) FROM {t} \
                  WHERE ZKIND = 2 AND {IN_LIBRARY} ORDER BY 1"
            )
        })
        .await?
    {
        for r in rows {
            if let Some(title) = r.try_get::<Option<String>, _>(0)? {
                parsed.albums.push(title);
            }
        }
    }
    parsed.inputs = m.inputs();
    Ok(parsed)
}

fn core_data_ms(seconds: f64) -> i64 {
    ((seconds + CORE_DATA_EPOCH_S) * 1000.0).round() as i64
}

/// `(key, count)` rows, each key through `label`.
fn tally(rows: Option<Vec<SqliteRow>>, label: impl Fn(Option<String>) -> String) -> Result<Tally> {
    let mut t = Tally::default();
    for r in rows.unwrap_or_default() {
        t.add(label(r.try_get(0)?), r.try_get(1)?, 0);
    }
    Ok(t)
}

/// `ZKIND`: 0 is a photo, 1 a video.
pub fn kind_label(kind: Option<&str>) -> String {
    match kind {
        Some("0") => "Photo".to_string(),
        Some("1") => "Video".to_string(),
        Some(other) => format!("Kind {other}"),
        None => "(unknown)".to_string(),
    }
}

/// A uniform type identifier's last part: `public.heic` is `heic`,
/// `com.apple.quicktime-movie` is `quicktime-movie`.
pub fn type_label(uti: Option<&str>) -> String {
    match uti.and_then(|u| u.rsplit('.').next()) {
        Some(last) if !last.is_empty() => last.to_string(),
        _ => "(unknown)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_is_its_uti_s_last_part() {
        assert_eq!(type_label(Some("public.heic")), "heic");
        assert_eq!(
            type_label(Some("com.apple.quicktime-movie")),
            "quicktime-movie"
        );
        assert_eq!(type_label(None), "(unknown)");
    }

    #[test]
    fn a_core_data_date_is_seconds_after_2001() {
        assert_eq!(core_data_ms(0.0), 978_307_200_000);
    }
}
