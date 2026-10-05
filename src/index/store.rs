use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};

use super::{Index, Result};
use crate::paths;

#[derive(Debug, Clone, Default)]
pub struct DirRecord {
    pub path: String,
    pub path_key: String,
    pub parent_key: Option<String>,
    pub depth: i64,
    /// `full` oder `summary`
    pub mode: String,
    pub attrs: u32,
    pub mtime: Option<i64>,
    pub is_link: bool,
    pub link_target: Option<String>,
    pub direct_entries: i64,
    pub summary_size: Option<i64>,
    pub summary_files: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct FileRecord {
    pub dir_key: String,
    pub path: String,
    pub path_key: String,
    pub name: String,
    pub ext: Option<String>,
    pub size: i64,
    pub mtime: i64,
    pub ctime: Option<i64>,
    /// Nur beim ersten Einfügen gesetzt (Schema v3); `None` = schon beim ersten Scan da.
    pub first_seen: Option<i64>,
    pub attrs: u32,
    pub cloud_only: bool,
    pub is_link: bool,
    pub link_target: Option<String>,
    pub partial_hash: Option<Vec<u8>>,
    pub full_hash: Option<Vec<u8>>,
    pub hash_status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScanErrorRecord {
    pub path: String,
    pub kind: String,
    pub message: String,
}

/// Vorzustand einer Datei für den inkrementellen Scan.
#[derive(Debug, Clone, PartialEq)]
pub struct PrevFile {
    pub size: i64,
    pub mtime: i64,
    pub partial_hash: Option<Vec<u8>>,
    pub full_hash: Option<Vec<u8>>,
    pub hash_status: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootStatus {
    Running,
    Complete,
    Aborted,
}

impl RootStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Complete => "complete",
            Self::Aborted => "aborted",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "complete" => Self::Complete,
            "aborted" => Self::Aborted,
            _ => Self::Running,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RootInfo {
    pub path: String,
    /// Ordner-Schlüssel (endet auf `\`).
    pub path_key: String,
    pub generation: i64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub status: RootStatus,
    pub error_count: i64,
}

/// Ein laufender Scan einer Wurzel.
#[derive(Debug, Clone)]
pub struct RootRun {
    pub path_key: String,
    pub generation: i64,
}

impl Index {
    /// Startet einen Scan: vergibt eine neue, global steigende Generation.
    pub fn begin_root(&mut self, path: &str, dir_key: &str, now: &str) -> Result<RootRun> {
        let tx = self.conn_mut().transaction()?;
        let current: i64 = tx
            .query_row("SELECT value FROM meta WHERE key='generation'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let generation = current + 1;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES('generation', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [generation.to_string()],
        )?;
        tx.execute(
            "INSERT INTO roots(path, path_key, generation, started_at, finished_at, status, error_count)
             VALUES(?1, ?2, ?3, ?4, NULL, 'running', 0)
             ON CONFLICT(path_key) DO UPDATE SET
               path = excluded.path, generation = excluded.generation,
               started_at = excluded.started_at, finished_at = NULL,
               status = 'running', error_count = 0",
            params![path, dir_key, generation, now],
        )?;
        tx.commit()?;
        Ok(RootRun {
            path_key: dir_key.to_string(),
            generation,
        })
    }

    /// Vorzustand aller Dateien unterhalb der Wurzel, nach `path_key`.
    pub fn load_previous(&self, dir_key: &str) -> Result<HashMap<String, PrevFile>> {
        let (lo, hi) = paths::prefix_range(dir_key);
        let mut stmt = self.conn().prepare(
            "SELECT path_key, size, mtime, partial_hash, full_hash, hash_status
             FROM files WHERE path_key >= ?1 AND path_key < ?2",
        )?;
        let rows = stmt.query_map(params![lo, hi], |r| {
            Ok((
                r.get::<_, String>(0)?,
                PrevFile {
                    size: r.get(1)?,
                    mtime: r.get(2)?,
                    partial_hash: r.get(3)?,
                    full_hash: r.get(4)?,
                    hash_status: r.get(5)?,
                },
            ))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn upsert_dirs(&mut self, dirs: &[DirRecord], generation: i64) -> Result<()> {
        let tx = self.conn_mut().transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO dirs(path, path_key, parent_key, depth, mode, attrs, mtime, is_link,
                                  link_target, direct_entries, summary_size, summary_files, generation)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
                 ON CONFLICT(path_key) DO UPDATE SET
                   path=excluded.path, parent_key=excluded.parent_key, depth=excluded.depth,
                   mode=excluded.mode, attrs=excluded.attrs, mtime=excluded.mtime,
                   is_link=excluded.is_link, link_target=excluded.link_target,
                   direct_entries=excluded.direct_entries, summary_size=excluded.summary_size,
                   summary_files=excluded.summary_files, generation=excluded.generation",
            )?;
            for d in dirs {
                stmt.execute(params![
                    d.path,
                    d.path_key,
                    d.parent_key,
                    d.depth,
                    d.mode,
                    d.attrs,
                    d.mtime,
                    d.is_link,
                    d.link_target,
                    d.direct_entries,
                    d.summary_size,
                    d.summary_files,
                    generation
                ])?;
            }
        }
        Ok(tx.commit()?)
    }

    pub fn upsert_files(&mut self, files: &[FileRecord], generation: i64) -> Result<()> {
        let tx = self.conn_mut().transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO files(dir_key, path, path_key, name, ext, size, mtime, ctime, attrs,
                                   cloud_only, is_link, link_target, partial_hash, full_hash,
                                   hash_status, generation, first_seen)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
                 ON CONFLICT(path_key) DO UPDATE SET
                   dir_key=excluded.dir_key, path=excluded.path, name=excluded.name,
                   ext=excluded.ext, size=excluded.size, mtime=excluded.mtime,
                   ctime=excluded.ctime, attrs=excluded.attrs, cloud_only=excluded.cloud_only,
                   is_link=excluded.is_link, link_target=excluded.link_target,
                   partial_hash=excluded.partial_hash, full_hash=excluded.full_hash,
                   hash_status=excluded.hash_status, generation=excluded.generation",
            )?;
            for f in files {
                stmt.execute(params![
                    f.dir_key,
                    f.path,
                    f.path_key,
                    f.name,
                    f.ext,
                    f.size,
                    f.mtime,
                    f.ctime,
                    f.attrs,
                    f.cloud_only,
                    f.is_link,
                    f.link_target,
                    f.partial_hash,
                    f.full_hash,
                    f.hash_status,
                    generation,
                    f.first_seen
                ])?;
            }
        }
        Ok(tx.commit()?)
    }

    /// War die Wurzel schon einmal vollständig gescannt? (Grundlage für `first_seen`.)
    pub fn root_scanned_before(&self, dir_key: &str) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM meta WHERE key = ?1",
                [format!("root_complete:{dir_key}")],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn record_error(&mut self, run: &RootRun, err: &ScanErrorRecord) -> Result<()> {
        self.conn().execute(
            "INSERT INTO errors(root_key, path, kind, message, generation) VALUES(?1,?2,?3,?4,?5)",
            params![
                run.path_key,
                err.path,
                err.kind,
                err.message,
                run.generation
            ],
        )?;
        Ok(())
    }

    /// Schließt den Scan ab. Nur bei `Complete` werden veraltete Einträge (ältere
    /// Generation) im Bereich der Wurzel gelöscht; nach einem Abbruch bleibt alles stehen.
    pub fn finish_root(
        &mut self,
        run: &RootRun,
        status: RootStatus,
        error_count: i64,
        now: &str,
    ) -> Result<()> {
        let (lo, hi) = paths::prefix_range(&run.path_key);
        let tx = self.conn_mut().transaction()?;
        if status == RootStatus::Complete {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES(?1, '1')
                 ON CONFLICT(key) DO UPDATE SET value = '1'",
                [format!("root_complete:{}", run.path_key)],
            )?;
            for table in ["files", "dirs"] {
                tx.execute(
                    &format!(
                        "DELETE FROM {table} WHERE path_key >= ?1 AND path_key < ?2 AND generation < ?3"
                    ),
                    params![lo, hi, run.generation],
                )?;
            }
            tx.execute(
                "DELETE FROM errors WHERE root_key = ?1 AND generation < ?2",
                params![run.path_key, run.generation],
            )?;
        }
        tx.execute(
            "UPDATE roots SET status = ?1, finished_at = ?2, error_count = ?3 WHERE path_key = ?4",
            params![status.as_str(), now, error_count, run.path_key],
        )?;
        Ok(tx.commit()?)
    }

    pub fn roots(&self) -> Result<Vec<RootInfo>> {
        let mut stmt = self.conn().prepare(
            "SELECT path, path_key, generation, started_at, finished_at, status, error_count
             FROM roots ORDER BY path_key",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(RootInfo {
                path: r.get(0)?,
                path_key: r.get(1)?,
                generation: r.get(2)?,
                started_at: r.get(3)?,
                finished_at: r.get(4)?,
                status: RootStatus::parse(&r.get::<_, String>(5)?),
                error_count: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Entfernt eine Wurzel samt ihrer Einträge, außer Einträgen, die in einer
    /// anderen (verschachtelten) Wurzel liegen. Liefert `false`, wenn es die Wurzel nicht gab.
    pub fn remove_root(&mut self, dir_key: &str) -> Result<bool> {
        let (lo, hi) = paths::prefix_range(dir_key);
        let tx = self.conn_mut().transaction()?;
        let removed = tx.execute("DELETE FROM roots WHERE path_key = ?1", [dir_key])?;
        if removed == 0 {
            return Ok(false);
        }
        for table in ["files", "dirs"] {
            tx.execute(
                &format!(
                    "DELETE FROM {table} WHERE path_key >= ?1 AND path_key < ?2 AND NOT EXISTS (
                       SELECT 1 FROM roots r
                       WHERE substr({table}.path_key, 1, length(r.path_key)) = r.path_key)"
                ),
                params![lo, hi],
            )?;
        }
        tx.execute("DELETE FROM errors WHERE root_key = ?1", [dir_key])?;
        tx.execute(
            "DELETE FROM meta WHERE key = ?1",
            [format!("root_complete:{dir_key}")],
        )?;
        tx.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn key(p: &str) -> String {
        paths::path_key(Path::new(p))
    }

    fn file(path: &str, size: i64) -> FileRecord {
        let p = Path::new(path);
        FileRecord {
            dir_key: paths::dir_key(p.parent().unwrap()),
            path: path.to_string(),
            path_key: key(path),
            name: p.file_name().unwrap().to_string_lossy().into_owned(),
            size,
            mtime: 100,
            ..FileRecord::default()
        }
    }

    fn dir(path: &str) -> DirRecord {
        DirRecord {
            path: path.to_string(),
            path_key: paths::dir_key(Path::new(path)),
            mode: "full".into(),
            ..DirRecord::default()
        }
    }

    fn count(idx: &Index, table: &str) -> i64 {
        idx.conn()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn begin(idx: &mut Index, path: &str) -> RootRun {
        idx.begin_root(path, &paths::dir_key(Path::new(path)), "t")
            .unwrap()
    }

    #[test]
    fn generationen_steigen_global() {
        let mut idx = Index::open_in_memory().unwrap();
        let a = begin(&mut idx, r"C:\A");
        let b = begin(&mut idx, r"C:\B");
        let a2 = begin(&mut idx, r"C:\A");
        assert!(a.generation < b.generation && b.generation < a2.generation);
        assert_eq!(idx.roots().unwrap().len(), 2);
    }

    #[test]
    fn upsert_ersetzt_statt_zu_verdoppeln() {
        let mut idx = Index::open_in_memory().unwrap();
        let run = begin(&mut idx, r"C:\A");
        idx.upsert_files(&[file(r"C:\A\x.txt", 1)], run.generation)
            .unwrap();
        let run2 = begin(&mut idx, r"C:\A");
        idx.upsert_files(&[file(r"C:\a\X.TXT", 5)], run2.generation)
            .unwrap();
        idx.upsert_dirs(&[dir(r"C:\A")], run2.generation).unwrap();
        idx.upsert_dirs(&[dir(r"C:\A")], run2.generation).unwrap();
        assert_eq!(count(&idx, "files"), 1);
        assert_eq!(count(&idx, "dirs"), 1);
        let (size, generation): (i64, i64) = idx
            .conn()
            .query_row("SELECT size, generation FROM files", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((size, generation), (5, run2.generation));
    }

    #[test]
    fn prune_entfernt_verschwundenes_nur_im_praefix() {
        let mut idx = Index::open_in_memory().unwrap();
        let a = begin(&mut idx, r"C:\A");
        idx.upsert_files(
            &[file(r"C:\A\alt.txt", 1), file(r"C:\A\bleibt.txt", 1)],
            a.generation,
        )
        .unwrap();
        let b = begin(&mut idx, r"C:\B");
        idx.upsert_files(
            &[file(r"C:\B\b.txt", 1), file(r"C:\AB\c.txt", 1)],
            b.generation,
        )
        .unwrap();
        idx.finish_root(&b, RootStatus::Complete, 0, "t").unwrap();

        let a2 = begin(&mut idx, r"C:\A");
        idx.upsert_files(
            &[file(r"C:\A\bleibt.txt", 1), file(r"C:\A\neu.txt", 1)],
            a2.generation,
        )
        .unwrap();
        idx.finish_root(&a2, RootStatus::Complete, 0, "t").unwrap();

        let prev = idx
            .load_previous(&paths::dir_key(Path::new(r"C:\")))
            .unwrap();
        let mut keys: Vec<_> = prev.keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                key(r"C:\A\bleibt.txt"),
                key(r"C:\A\neu.txt"),
                key(r"C:\AB\c.txt"),
                key(r"C:\B\b.txt")
            ]
        );
    }

    #[test]
    fn abbruch_prunt_nicht() {
        let mut idx = Index::open_in_memory().unwrap();
        let a = begin(&mut idx, r"C:\A");
        idx.upsert_files(&[file(r"C:\A\alt.txt", 1)], a.generation)
            .unwrap();
        idx.finish_root(&a, RootStatus::Complete, 0, "t").unwrap();
        let a2 = begin(&mut idx, r"C:\A");
        idx.finish_root(&a2, RootStatus::Aborted, 0, "t").unwrap();
        assert_eq!(count(&idx, "files"), 1);
        assert_eq!(idx.roots().unwrap()[0].status, RootStatus::Aborted);
    }

    #[test]
    fn fehler_werden_pro_lauf_ersetzt() {
        let mut idx = Index::open_in_memory().unwrap();
        let a = begin(&mut idx, r"C:\A");
        let e = ScanErrorRecord {
            path: r"C:\A\x".into(),
            kind: "access_denied".into(),
            message: "Zugriff verweigert".into(),
        };
        idx.record_error(&a, &e).unwrap();
        idx.finish_root(&a, RootStatus::Complete, 1, "t").unwrap();
        let a2 = begin(&mut idx, r"C:\A");
        idx.finish_root(&a2, RootStatus::Complete, 0, "t").unwrap();
        assert_eq!(count(&idx, "errors"), 0);
    }

    #[test]
    fn vorzustand_liefert_hashes() {
        let mut idx = Index::open_in_memory().unwrap();
        let a = begin(&mut idx, r"C:\A");
        let mut f = file(r"C:\A\x.bin", 10);
        f.partial_hash = Some(vec![1, 2]);
        f.full_hash = Some(vec![3, 4]);
        f.hash_status = Some("ok".into());
        idx.upsert_files(&[f], a.generation).unwrap();
        let prev = idx
            .load_previous(&paths::dir_key(Path::new(r"C:\A")))
            .unwrap();
        let p = &prev[&key(r"C:\A\x.bin")];
        assert_eq!((p.size, p.mtime), (10, 100));
        assert_eq!(p.full_hash, Some(vec![3, 4]));
        assert_eq!(p.hash_status.as_deref(), Some("ok"));
    }

    #[test]
    fn verschachtelte_wurzeln_ohne_doppelungen() {
        let mut idx = Index::open_in_memory().unwrap();
        let p = begin(&mut idx, r"C:\P");
        idx.upsert_files(
            &[file(r"C:\P\a.txt", 1), file(r"C:\P\sub\b.txt", 1)],
            p.generation,
        )
        .unwrap();
        idx.finish_root(&p, RootStatus::Complete, 0, "t").unwrap();
        let s = begin(&mut idx, r"C:\P\sub");
        idx.upsert_files(&[file(r"C:\P\sub\b.txt", 1)], s.generation)
            .unwrap();
        idx.finish_root(&s, RootStatus::Complete, 0, "t").unwrap();
        assert_eq!(count(&idx, "files"), 2);
        // Prune der Unterwurzel lässt die Datei der Oberwurzel stehen.
        assert!(idx
            .load_previous(&paths::dir_key(Path::new(r"C:\P")))
            .unwrap()
            .contains_key(&key(r"C:\P\a.txt")));
    }

    #[test]
    fn remove_root_behaelt_eintraege_verschachtelter_wurzel() {
        let mut idx = Index::open_in_memory().unwrap();
        let p = begin(&mut idx, r"C:\P");
        idx.upsert_files(
            &[file(r"C:\P\a.txt", 1), file(r"C:\P\sub\b.txt", 1)],
            p.generation,
        )
        .unwrap();
        let s = begin(&mut idx, r"C:\P\sub");
        idx.upsert_files(&[file(r"C:\P\sub\b.txt", 1)], s.generation)
            .unwrap();
        assert!(idx
            .remove_root(&paths::dir_key(Path::new(r"C:\P")))
            .unwrap());
        assert_eq!(count(&idx, "files"), 1);
        assert_eq!(idx.roots().unwrap().len(), 1);
        assert!(!idx
            .remove_root(&paths::dir_key(Path::new(r"C:\P")))
            .unwrap());
    }
}
