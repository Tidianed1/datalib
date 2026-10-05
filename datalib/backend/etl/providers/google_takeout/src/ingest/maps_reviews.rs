//! `Maps (your places)/Reviews.json` walker.

use datalib_etl::fsscan;

use anyhow::{Context, Result};
use datalib_etl::file_checkpoint::{self, SnapshotCounts};
use datalib_etl::progress::Progress;
use datalib_problems::Reason;
use serde_json::Value;

use super::db::RawDb;
use super::schema_raw::{ns_id, MapsReviewRow};
use datalib_etl::doltlite_raw::WirePayload;

const FILE_REL: &str = "Maps (your places)/Reviews.json";
const SCOPE: &str = "google_takeout/maps_reviews";

pub async fn ingest(
    db: &RawDb,
    scan: &fsscan::Scan,
    progress: &Progress,
) -> Result<SnapshotCounts> {
    let file = scan.file(FILE_REL);
    let n = file_checkpoint::ingest_snapshot(db.pool(), SCOPE, file, |bytes| {
        let geo: Value = serde_json::from_slice(bytes).context("parse Reviews.json")?;
        let Some(features) = geo.get("features").and_then(|v| v.as_array()) else {
            return Ok((
                None,
                super::unusable(
                    Reason::Undeserializable,
                    "the file has no features list, so nothing was ingested or deleted".to_string(),
                ),
            ));
        };
        let mut rows: Vec<MapsReviewRow> = Vec::with_capacity(features.len());
        let mut skipped: Vec<String> = Vec::new();
        for (i, f) in features.iter().enumerate() {
            let Some(props) = f.get("properties") else {
                skipped.push(format!("review {i} has no properties"));
                continue;
            };
            let date = props.get("date").and_then(|v| v.as_str()).unwrap_or("");
            let ftid = props
                .get("google_maps_url")
                .and_then(|v| v.as_str())
                .and_then(extract_ftid)
                .unwrap_or("");
            if ftid.is_empty() || date.is_empty() {
                skipped.push(format!("review {i} has no place id or no date"));
                continue;
            }
            let id = ns_id(&format!("maps_review:{ftid}:{date}"));
            let payload = serde_json::to_string(f).context("serialize maps_review feature")?;
            rows.push(MapsReviewRow {
                id_and_payload: WirePayload { id, payload },
                when_ts: Some(date.to_string()),
            });
        }
        Ok((Some(rows), super::skipped_records(&skipped)))
    })
    .await?;
    progress.set_message(&format!("maps_reviews: {}", n.written));
    Ok(n)
}

fn extract_ftid(url: &str) -> Option<&str> {
    let key = "!1s";
    let after = &url[url.find(key)? + key.len()..];
    let end = after.find('!').unwrap_or(after.len());
    let ftid = &after[..end];
    if ftid.is_empty() {
        None
    } else {
        Some(ftid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_ftid_from_url() {
        assert_eq!(
            extract_ftid("https://www.google.com/maps/place/X/@1,2,15z/data=!4m1!1sabc123def!8m2"),
            Some("abc123def"),
        );
        assert_eq!(extract_ftid("https://www.google.com/maps/place/X"), None);
    }
}
