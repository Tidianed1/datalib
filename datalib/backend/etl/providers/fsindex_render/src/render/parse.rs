//! Read an fsindex raw store, pinned, into the numbers its page shows.
//! `files` is read once, streamed, because at the scale fsindex is
//! built for (tens of millions of paths) holding it would not fit.

use std::path::Path;

use anyhow::{Context, Result};
use datalib_etl::doltlite_raw::Reader;
use datalib_etl_render::inputs::{Input, RawRange};
use datalib_etl_summary_render::Tally;
use futures::TryStreamExt;
use sqlx::Row;

/// A top-level directory, with its whole subtree rolled up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    pub size: i64,
    pub entries: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    /// The commit everything was read at.
    pub head: String,
    /// `scan_meta.abs_path`: where the tree was scanned from.
    pub roots: Vec<String>,
    pub files: i64,
    pub symlinks: i64,
    /// Content bytes of the regular files.
    pub file_bytes: i64,
    /// Regular files by extension, with their bytes.
    pub by_extension: Tally,
    /// Directories under the scan root, the root not counted.
    pub folders: i64,
    pub top_folders: Vec<Folder>,
}

/// The tables the page reads, whole: any row of any of them moving
/// re-renders it.
pub fn inputs() -> Vec<Input> {
    ["files", "dirs", "scan_meta"]
        .into_iter()
        .map(Input::whole_table)
        .collect()
}

pub fn parse(raw_path: &Path, range: RawRange<'_>) -> Result<Option<Parsed>> {
    datalib_etl_summary_render::read::pinned(raw_path, range, async |reader: &Reader| {
        read(reader).await
    })
}

async fn read(reader: &Reader) -> Result<Parsed> {
    let pool = reader.pool();
    let mut parsed = Parsed {
        head: reader.pin().commit().to_string(),
        ..Default::default()
    };
    parsed.roots =
        sqlx::query_scalar("SELECT abs_path FROM pinned_scan_meta scan_meta ORDER BY abs_path")
            .fetch_all(pool)
            .await
            .context("read scan_meta")?;

    let mut rows = sqlx::query("SELECT id, kind, size FROM pinned_files files").fetch(pool);
    while let Some(r) = rows.try_next().await.context("read files")? {
        let size: i64 = r.try_get("size")?;
        match r.try_get::<String, _>("kind")?.as_str() {
            "symlink" => parsed.symlinks += 1,
            _ => {
                parsed.files += 1;
                parsed.file_bytes += size;
                let path: String = r.try_get("id")?;
                parsed.by_extension.add(extension_of(&path), 1, size);
            }
        }
    }
    drop(rows);

    parsed.folders = sqlx::query_scalar("SELECT COUNT(*) FROM pinned_dirs dirs WHERE id != ''")
        .fetch_one(pool)
        .await
        .context("count dirs")?;
    parsed.top_folders = sqlx::query(
        "SELECT id, size, entries FROM pinned_dirs dirs \
          WHERE id != '' AND instr(id, '/') = 0 ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .context("read top-level dirs")?
    .into_iter()
    .map(|r| {
        Ok(Folder {
            name: r.try_get("id")?,
            size: r.try_get("size")?,
            entries: r.try_get("entries")?,
        })
    })
    .collect::<Result<_>>()?;
    Ok(parsed)
}

/// A file's extension as the page groups it: lowercased, with its dot.
/// A dotfile's leading dot does not start one.
pub fn extension_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            format!(".{}", ext.to_lowercase())
        }
        _ => "(none)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_are_lowercased_and_a_dotfile_has_none() {
        assert_eq!(extension_of("bridge/Viewscreen.TXT"), ".txt");
        assert_eq!(extension_of("archive.tar.gz"), ".gz");
        assert_eq!(extension_of("holodeck/.fsindex.yaml"), ".yaml");
        assert_eq!(extension_of(".profile"), "(none)");
        assert_eq!(extension_of("Makefile"), "(none)");
        assert_eq!(extension_of("v1.2/README"), "(none)");
    }
}
