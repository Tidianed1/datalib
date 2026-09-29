//! What the fsindex page says, from what [`parse`](super::parse) read.
//! The page counts files and symlinks, the rows of `files`, and never
//! directories.

use datalib_etl_render::text::{human_bytes, thousands};
use datalib_etl_summary_render::{Breakdown, Order, Summary, Tally};

use super::parse::Parsed;

pub fn summarize(p: &Parsed) -> Summary {
    let symlinks = match p.symlinks {
        0 => String::new(),
        1 => ", 1 symlink".to_string(),
        n => format!(", {} symlinks", thousands(n)),
    };
    let lede = format!(
        "{} file{}{symlinks} in {} folder{}, {} in all.",
        thousands(p.files),
        plural(p.files),
        thousands(p.folders),
        plural(p.folders),
        human_bytes(p.file_bytes),
    );
    let mut facts = vec![
        ("Scanned".to_string(), p.roots.join(", ")),
        ("Files".to_string(), thousands(p.files)),
    ];
    if p.symlinks > 0 {
        facts.push(("Symlinks".to_string(), thousands(p.symlinks)));
    }
    facts.push(("Folders".to_string(), thousands(p.folders)));
    facts.push(("Size".to_string(), human_bytes(p.file_bytes)));

    let mut folders = Tally::default();
    for f in &p.top_folders {
        folders.add(format!("{}/", f.name), f.entries, f.size);
    }
    Summary {
        item_count: p.files + p.symlinks,
        lede,
        facts,
        breakdowns: vec![
            Breakdown {
                heading: "By extension",
                key: "Extension",
                noun: "Files",
                tally: p.by_extension.clone(),
                bytes: true,
                order: Order::ByCount,
                limit: Some(20),
            },
            Breakdown {
                heading: "Top-level folders",
                key: "Folder",
                noun: "Entries",
                tally: folders,
                bytes: true,
                order: Order::ByCount,
                limit: Some(20),
            },
        ],
        ..Default::default()
    }
}

fn plural(n: i64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Directories are rows of their own table, and the page's count is
    /// what `files` holds: files and symlinks, never folders.
    #[test]
    fn the_item_count_is_files_and_symlinks_not_folders() {
        let s = summarize(&Parsed {
            files: 4,
            symlinks: 1,
            folders: 2,
            ..Default::default()
        });
        assert_eq!(s.item_count, 5);
        assert!(
            s.lede.starts_with("4 files, 1 symlink in 2 folders"),
            "{}",
            s.lede
        );
    }
}
