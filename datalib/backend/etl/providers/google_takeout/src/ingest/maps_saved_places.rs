//! `Maps (your places)/Saved Places.json` walker.
//!
//! GeoJSON `FeatureCollection`; one feature per saved/starred place.
//! PK recipe: `uuidv5(NS, "maps_saved:{ftid_or_cid}:{date}")`.

use datalib_etl::fsscan;

use anyhow::{Context, Result};
use datalib_etl::file_checkpoint::{self, SnapshotCounts};
use datalib_etl::progress::Progress;
use datalib_problems::Reason;
use serde_json::Value;

use super::db::RawDb;
use super::schema_raw::{ns_id, MapsSavedPlaceRow};
use datalib_etl::doltlite_raw::WirePayload;

const FILE_REL: &str = "Maps (your places)/Saved Places.json";
const SCOPE: &str = "google_takeout/maps_saved_places";

pub async fn ingest(
    db: &RawDb,
    scan: &fsscan::Scan,
    progress: &Progress,
) -> Result<SnapshotCounts> {
    let file = scan.file(FILE_REL);
    let mut unusable = None;
    let n = file_checkpoint::ingest_snapshot(db.pool(), SCOPE, file, |bytes| {
        let geo: Value = serde_json::from_slice(bytes).context("parse Saved Places.json")?;
        let Some(features) = geo.get("features").and_then(|v| v.as_array()) else {
            unusable = Some((
                Reason::Undeserializable,
                "the file has no features list, so nothing was ingested or deleted".to_string(),
            ));
            return Ok(None);
        };
        let mut rows: Vec<MapsSavedPlaceRow> = Vec::with_capacity(features.len());
        let mut skipped: Vec<String> = Vec::new();
        for (i, f) in features.iter().enumerate() {
            let Some(props) = f.get("properties") else {
                skipped.push(format!("place {i} has no properties"));
                continue;
            };
            let date = props.get("date").and_then(|v| v.as_str()).unwrap_or("");
            let url = props
                .get("google_maps_url")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let key = extract_ftid_or_cid(url).unwrap_or("");
            if key.is_empty() || date.is_empty() {
                skipped.push(format!("place {i} has no place id or no date"));
                continue;
            }
            let id = ns_id(&format!("maps_saved:{key}:{date}"));
            let payload = serde_json::to_string(f).context("serialize saved-place feature")?;
            rows.push(MapsSavedPlaceRow {
                id_and_payload: WirePayload { id, payload },
                when_ts: Some(date.to_string()),
            });
        }
        unusable = super::skipped_records(&skipped);
        Ok(Some(rows))
    })
    .await?;
    super::record_unusable(db, SCOPE, file, unusable).await?;
    progress.set_message(&format!("maps_saved_places: {}", n.written));
    Ok(n)
}

fn extract_ftid_or_cid(url: &str) -> Option<&str> {
    if let Some(rest) = url.find("!1s").map(|i| &url[i + 3..]) {
        let end = rest.find('!').unwrap_or(rest.len());
        let ftid = &rest[..end];
        if !ftid.is_empty() {
            return Some(ftid);
        }
    }
    if let Some(rest) = url.find("cid=").map(|i| &url[i + 4..]) {
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let cid = &rest[..end];
        if !cid.is_empty() {
            return Some(cid);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ftid_wins_over_cid() {
        assert_eq!(
            extract_ftid_or_cid("https://maps.google.com/?cid=42&data=!1sabc!8m"),
            Some("abc"),
        );
    }

    #[test]
    fn falls_back_to_cid() {
        assert_eq!(
            extract_ftid_or_cid("https://maps.google.com/?cid=12345"),
            Some("12345"),
        );
    }
}
