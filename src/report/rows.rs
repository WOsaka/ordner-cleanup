//! Zeilen aus dem Index, wie Bericht und Kennzahlen sie gemeinsam nutzen.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use rusqlite::params;

use super::ErrorItem;
use crate::index::Index;
use crate::paths;
use crate::platform::FileAttrs;

pub struct FRow {
    pub path: String,
    pub dir_key: String,
    pub name: String,
    pub ext: Option<String>,
    pub size: u64,
    pub mtime: i64,
    pub attrs: FileAttrs,
    pub cloud: bool,
    pub is_link: bool,
    pub full_hash: Option<Vec<u8>>,
    /// Größe zählt in Summen (Hardlink-Geschwister zählen nur einmal)
    pub counted: bool,
}

pub struct DRow {
    pub path: String,
    pub key: String,
    pub parent: Option<String>,
    pub depth: i64,
    pub summary: bool,
    pub is_link: bool,
    pub direct_entries: u64,
    pub summary_size: u64,
    pub summary_files: u64,
}

pub fn load_files(index: &Index, dir_key: &str) -> Result<Vec<FRow>> {
    let (lo, hi) = paths::prefix_range(dir_key);
    let mut stmt = index.conn().prepare(
        "SELECT path, dir_key, name, ext, size, mtime, attrs, cloud_only, is_link,
                volume_serial, file_index, full_hash
         FROM files WHERE path_key >= ?1 AND path_key < ?2 ORDER BY path_key",
    )?;
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
    let rows = stmt.query_map(params![lo, hi], |r| {
        let volume: Option<i64> = r.get(9)?;
        let findex: Option<i64> = r.get(10)?;
        let is_link: bool = r.get(8)?;
        Ok((
            FRow {
                path: r.get(0)?,
                dir_key: r.get(1)?,
                name: r.get(2)?,
                ext: r.get(3)?,
                size: r.get::<_, i64>(4)? as u64,
                mtime: r.get(5)?,
                attrs: FileAttrs(r.get(6)?),
                cloud: r.get(7)?,
                is_link,
                full_hash: r.get(11)?,
                counted: true,
            },
            volume.zip(findex),
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (mut f, ident) = row?;
        if let Some(id) = ident {
            f.counted = seen.insert(id);
        }
        out.push(f);
    }
    Ok(out)
}

pub fn load_dirs(index: &Index, dir_key: &str) -> Result<Vec<DRow>> {
    let (lo, hi) = paths::prefix_range(dir_key);
    let mut stmt = index.conn().prepare(
        "SELECT path, path_key, parent_key, depth, mode, is_link, direct_entries,
                summary_size, summary_files
         FROM dirs WHERE path_key >= ?1 AND path_key < ?2 ORDER BY path_key",
    )?;
    let rows = stmt.query_map(params![lo, hi], |r| {
        Ok(DRow {
            path: r.get(0)?,
            key: r.get(1)?,
            parent: r.get(2)?,
            depth: r.get(3)?,
            summary: r.get::<_, String>(4)? == "summary",
            is_link: r.get(5)?,
            direct_entries: r.get::<_, i64>(6)? as u64,
            summary_size: r.get::<_, Option<i64>>(7)?.unwrap_or(0) as u64,
            summary_files: r.get::<_, Option<i64>>(8)?.unwrap_or(0) as u64,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn load_errors(index: &Index, dir_key: &str) -> Result<Vec<ErrorItem>> {
    let (lo, hi) = paths::prefix_range(dir_key);
    let mut stmt = index
        .conn()
        .prepare("SELECT path, kind, message FROM errors ORDER BY path")?;
    let rows = stmt.query_map(params![], |r| {
        Ok(ErrorItem {
            path: r.get(0)?,
            kind: r.get(1)?,
            message: r.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        let e = row?;
        let key = paths::path_key(Path::new(&e.path));
        // Fehlerpfade von Ordnern enden ohne `\`; als Ordner und als Datei prüfen.
        let in_range = (key.as_str() >= lo.as_str() && key.as_str() < hi.as_str())
            || format!("{key}\\") == dir_key;
        if in_range {
            out.push(e);
        }
    }
    Ok(out)
}
