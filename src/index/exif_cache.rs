//! Zwischenspeicher für das EXIF-Aufnahmedatum (Schema v2). Ein Eintrag gilt, solange Größe
//! und mtime der Datei unverändert sind. Die Tabelle hängt nicht an `files.generation` und
//! überdauert deshalb Re-Scans.

use std::collections::HashMap;

use rusqlite::params;

use super::{Index, Result};
use crate::paths;

/// Eintrag zum Speichern: `taken` = Aufnahmezeit als lokale Sekunden seit der Epoche,
/// `None` = kein (lesbares) EXIF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExifEntry {
    pub path_key: String,
    pub size: i64,
    pub mtime: i64,
    pub taken: Option<i64>,
}

impl Index {
    /// Gültige Treffer zu `(path_key, size, mtime)`; ungültige oder fehlende fehlen im Ergebnis.
    pub fn exif_lookup(&self, files: &[(&str, i64, i64)]) -> Result<HashMap<String, Option<i64>>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT size, mtime, taken FROM exif_cache WHERE path_key = ?1")?;
        let mut found = HashMap::new();
        for (key, size, mtime) in files {
            let row = stmt
                .query_row([key], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get(2)?))
                })
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    e => Err(e),
                })?;
            if let Some((cached_size, cached_mtime, taken)) = row {
                if cached_size == *size && cached_mtime == *mtime {
                    found.insert((*key).to_string(), taken);
                }
            }
        }
        Ok(found)
    }

    /// Schreibt alle Einträge in **einer** Transaktion.
    pub fn exif_store(&mut self, entries: &[ExifEntry]) -> Result<()> {
        let tx = self.conn_mut().transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO exif_cache(path_key, size, mtime, taken) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(path_key) DO UPDATE SET
                   size = excluded.size, mtime = excluded.mtime, taken = excluded.taken",
            )?;
            for e in entries {
                stmt.execute(params![e.path_key, e.size, e.mtime, e.taken])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Entfernt Einträge unter der Wurzel, zu denen es keine Datei im Index mehr gibt.
    pub fn exif_prune(&mut self, root_dir_key: &str) -> Result<usize> {
        let (lo, hi) = paths::prefix_range(root_dir_key);
        Ok(self.conn().execute(
            "DELETE FROM exif_cache WHERE path_key >= ?1 AND path_key < ?2
             AND NOT EXISTS (SELECT 1 FROM files WHERE files.path_key = exif_cache.path_key)",
            params![lo, hi],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::FileRecord;
    use std::path::Path;

    fn entry(key: &str, size: i64, mtime: i64, taken: Option<i64>) -> ExifEntry {
        ExifEntry {
            path_key: key.into(),
            size,
            mtime,
            taken,
        }
    }

    #[test]
    fn treffer_gilt_nur_bei_gleicher_groesse_und_mtime() {
        let mut index = Index::open_in_memory().unwrap();
        index
            .exif_store(&[
                entry(r"Z:\a.jpg", 10, 100, Some(5)),
                entry(r"Z:\b.jpg", 10, 100, None),
            ])
            .unwrap();
        let found = index
            .exif_lookup(&[
                (r"Z:\a.jpg", 10, 100),
                (r"Z:\b.jpg", 10, 100),
                (r"Z:\c.jpg", 10, 100),
            ])
            .unwrap();
        assert_eq!(found.get(r"Z:\a.jpg"), Some(&Some(5)));
        assert_eq!(
            found.get(r"Z:\b.jpg"),
            Some(&None),
            "kein EXIF ist ein Treffer"
        );
        assert!(!found.contains_key(r"Z:\c.jpg"));

        let stale = index
            .exif_lookup(&[(r"Z:\a.jpg", 11, 100), (r"Z:\b.jpg", 10, 101)])
            .unwrap();
        assert!(stale.is_empty(), "Größe oder mtime geändert: {stale:?}");
    }

    #[test]
    fn speichern_ersetzt_vorhandene_eintraege() {
        let mut index = Index::open_in_memory().unwrap();
        index
            .exif_store(&[entry(r"Z:\a.jpg", 10, 100, None)])
            .unwrap();
        index
            .exif_store(&[entry(r"Z:\a.jpg", 20, 200, Some(7))])
            .unwrap();
        let found = index.exif_lookup(&[(r"Z:\a.jpg", 20, 200)]).unwrap();
        assert_eq!(found.get(r"Z:\a.jpg"), Some(&Some(7)));
    }

    #[test]
    fn prune_entfernt_nur_eintraege_ohne_datei_unter_der_wurzel() {
        let mut index = Index::open_in_memory().unwrap();
        let root = Path::new(r"Z:\Root");
        let run = index
            .begin_root(r"Z:\Root", &paths::dir_key(root), "t")
            .unwrap();
        index
            .upsert_files(
                &[FileRecord {
                    dir_key: paths::dir_key(root),
                    path: r"Z:\Root\da.jpg".into(),
                    path_key: r"Z:\root\da.jpg".into(),
                    name: "da.jpg".into(),
                    ..FileRecord::default()
                }],
                run.generation,
            )
            .unwrap();
        index
            .exif_store(&[
                entry(r"Z:\root\da.jpg", 1, 1, None),
                entry(r"Z:\root\weg.jpg", 1, 1, None),
                entry(r"Z:\anders\weg.jpg", 1, 1, None),
            ])
            .unwrap();
        assert_eq!(index.exif_prune(&paths::dir_key(root)).unwrap(), 1);
        let found = index
            .exif_lookup(&[
                (r"Z:\root\da.jpg", 1, 1),
                (r"Z:\root\weg.jpg", 1, 1),
                (r"Z:\anders\weg.jpg", 1, 1),
            ])
            .unwrap();
        assert!(found.contains_key(r"Z:\root\da.jpg"));
        assert!(!found.contains_key(r"Z:\root\weg.jpg"));
        assert!(
            found.contains_key(r"Z:\anders\weg.jpg"),
            "andere Wurzel bleibt"
        );
    }

    #[test]
    fn schema_v1_wird_beim_oeffnen_migriert_und_behaelt_die_daten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let root = Path::new(r"Z:\Root");
        {
            let mut index = Index::open(&path).unwrap();
            let run = index
                .begin_root(r"Z:\Root", &paths::dir_key(root), "t")
                .unwrap();
            index
                .upsert_files(
                    &[FileRecord {
                        dir_key: paths::dir_key(root),
                        path: r"Z:\Root\a.txt".into(),
                        path_key: r"Z:\root\a.txt".into(),
                        name: "a.txt".into(),
                        full_hash: Some(vec![1, 2, 3]),
                        ..FileRecord::default()
                    }],
                    run.generation,
                )
                .unwrap();
            // Zustand von Phase 1 bis 3 nachstellen.
            for sql in [
                "DROP TABLE exif_cache",
                "DROP TABLE content_cache",
                "DROP TABLE ocr_text",
                "ALTER TABLE files DROP COLUMN first_seen",
            ] {
                index.conn().execute(sql, []).unwrap();
            }
            index
                .conn()
                .execute("UPDATE meta SET value='1' WHERE key='schema_version'", [])
                .unwrap();
        }
        let index = Index::open(&path).unwrap();
        let files = index.files_under(&paths::dir_key(root)).unwrap();
        assert_eq!(files.len(), 1, "Dateien bleiben erhalten");
        assert_eq!(index.roots().unwrap().len(), 1, "Wurzeln bleiben erhalten");
        assert!(index
            .exif_lookup(&[(r"Z:\root\a.txt", 0, 0)])
            .unwrap()
            .is_empty());
        let version: String = index
            .conn()
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, "3");
    }
}
