//! What the Lightroom page says, from what [`parse`](super::parse)
//! read. It counts the catalog's photos: the rows of `Adobe_images`,
//! virtual copies included, as Lightroom's own "All Photographs" does.

use datalib_etl_render::text::thousands;
use datalib_etl_summary_render::{Breakdown, Order, Summary};
use datalib_schema::problems::{Problem, Reason, Severity};

use super::parse::{Parsed, IMAGES};

pub fn summarize(p: &Parsed) -> Summary {
    let Some(images) = p.images else {
        return Summary {
            lede: format!(
                "This mirror has no `{IMAGES}` table, so there are no photos to count. \
                 Is the ingest pointed at a Lightroom catalog, and is the table left out \
                 by `include_tables` or `exclude_tables`?"
            ),
            problems: vec![Problem::field(
                IMAGES,
                Reason::NotFound,
                "the mirror has no such table",
            )
            .severity(Severity::Warning)],
            ..Default::default()
        };
    };
    let mut lede = format!(
        "{} photo{} in the catalog.",
        thousands(images),
        if images == 1 { "" } else { "s" }
    );
    if let (Some(a), Some(b)) = (&p.earliest, &p.latest) {
        lede.push_str(&format!(" Captured {} to {}.", day(a), day(b)));
    }
    let mut facts = vec![("Photos".to_string(), thousands(images))];
    for (label, n) in [("Folders", p.folders), ("Collections", p.collections)] {
        if let Some(n) = n {
            facts.push((label.to_string(), thousands(n)));
        }
    }
    let table = |heading, key, tally: &datalib_etl_summary_render::Tally, order, limit| Breakdown {
        heading,
        key,
        noun: "Photos",
        tally: tally.clone(),
        bytes: false,
        order,
        limit,
    };
    Summary {
        item_count: images,
        lede,
        facts,
        breakdowns: vec![
            table("By year captured", "Year", &p.by_year, Order::ByLabel, None),
            table(
                "By file format",
                "Format",
                &p.by_format,
                Order::ByCount,
                None,
            ),
            table("By rating", "Rating", &p.by_rating, Order::ByLabel, None),
            table("By flag", "Flag", &p.by_flag, Order::ByCount, None),
            table("Keywords", "Keyword", &p.keywords, Order::ByCount, Some(20)),
            table("Cameras", "Camera", &p.cameras, Order::ByCount, Some(15)),
        ],
        earliest: p.earliest.clone(),
        latest: p.latest.clone(),
        ..Default::default()
    }
}

/// The date part of a stamp, as the catalog wrote it.
fn day(stamp: &str) -> &str {
    stamp.get(..10).unwrap_or(stamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mirror without the images table is not an empty catalog: the
    /// page says so, and a problem row says so where the Manage screen
    /// counts them.
    #[test]
    fn a_mirror_with_no_images_table_says_so_and_reports_it() {
        let s = summarize(&Parsed::default());
        assert_eq!(s.item_count, 0);
        assert!(s.lede.contains("no `Adobe_images` table"), "{}", s.lede);
        let [p] = s.problems.as_slice() else {
            panic!("{:?}", s.problems)
        };
        assert_eq!(p.reason, Reason::NotFound);
        assert_eq!(p.field.as_deref(), Some(IMAGES));
    }
}
