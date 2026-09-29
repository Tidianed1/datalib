//! What the media page says, from what [`parse`](super::parse) read.
//! It counts files — the rows of `media_files` — so a song kept in two
//! folders is two.

use datalib_etl_render::text::{human_bytes, thousands};
use datalib_etl_summary_render::{Breakdown, Order, Summary};

use super::parse::Parsed;

pub fn summarize(p: &Parsed) -> Summary {
    let mut lede = format!(
        "{} media file{}, {} in all.",
        thousands(p.files),
        if p.files == 1 { "" } else { "s" },
        human_bytes(p.bytes),
    );
    if let (Some(a), Some(b)) = (&p.earliest, &p.latest) {
        lede.push_str(&format!(
            " Photos and videos taken {} to {}.",
            day(a),
            day(b)
        ));
    }
    let mut facts = vec![
        ("Scanned".to_string(), p.roots.join(", ")),
        ("Files".to_string(), thousands(p.files)),
        (
            "Distinct contents".to_string(),
            format!("{} (a copy in two places counts once)", thousands(p.items)),
        ),
        ("Size".to_string(), human_bytes(p.bytes)),
    ];
    if p.playlists > 0 {
        facts.push(("Playlists".to_string(), thousands(p.playlists)));
    }
    Summary {
        item_count: p.files,
        lede,
        facts,
        breakdowns: vec![
            Breakdown {
                heading: "By kind",
                key: "Kind",
                noun: "Files",
                tally: p.by_class.clone(),
                bytes: true,
                order: Order::ByCount,
                limit: None,
            },
            Breakdown {
                heading: "By format",
                key: "Format",
                noun: "Files",
                tally: p.by_format.clone(),
                bytes: true,
                order: Order::ByCount,
                limit: Some(20),
            },
            Breakdown {
                heading: "Photos and videos by year taken",
                key: "Year",
                noun: "Files",
                tally: p.by_year.clone(),
                bytes: false,
                order: Order::ByLabel,
                limit: None,
            },
            Breakdown {
                heading: "Cameras",
                key: "Camera",
                noun: "Files",
                tally: p.cameras.clone(),
                bytes: false,
                order: Order::ByCount,
                limit: Some(15),
            },
            Breakdown {
                heading: "Artists",
                key: "Artist",
                noun: "Files",
                tally: p.artists.clone(),
                bytes: false,
                order: Order::ByCount,
                limit: Some(15),
            },
        ],
        earliest: p.earliest.clone(),
        latest: p.latest.clone(),
        ..Default::default()
    }
}

/// The date part of a stamp, as the file wrote it.
fn day(stamp: &str) -> &str {
    stamp.get(..10).unwrap_or(stamp)
}
