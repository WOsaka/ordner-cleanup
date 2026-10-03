//! Abfragen für die Planer der Aufräumaktionen: alle Ordner und Dateien unterhalb eines
//! Ordner-Schlüssels, jeweils mit den Spalten, die `change::tree` braucht.

use std::collections::HashSet;

use rusqlite::params;

use super::{Index, Result};
use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirRow {
    pub path: String,
    /// Ordner-Schlüssel (endet auf `\`).
    pub key: String,
    pub parent_key: Option<String>,
    /// Nur aufsummiert gescannt (Inhalt im Index unbekannt).
    pub summary: bool,
    pub attrs: u32,
    pub is_link: bool,
    /// Einträge, die der Scan im Ordner gesehen hat (inklusive ausgeschlossener und fehlerhafter).
    pub direct_entries: i64,
    /// Für den Ordner selbst wurde beim Scan ein Lesefehler gemeldet.
    pub read_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub path: String,
    pub key: String,
    pub dir_key: String,
    pub name: String,
    pub size: i64,
    pub mtime: i64,
    pub attrs: u32,
    pub cloud_only: bool,
    pub is_link: bool,
}

impl Index {
    /// Der Ordner selbst und alle Ordner darunter (eine Abfrage plus eine für die Fehler).
    pub fn dirs_under(&self, dir_key: &str) -> Result<Vec<DirRow>> {
        let error_dirs = self.error_dir_keys(dir_key)?;
        let (lo, hi) = paths::prefix_range(dir_key);
        let mut stmt = self.conn().prepare(
            "SELECT path, path_key, parent_key, mode, attrs, is_link, direct_entries
             FROM dirs WHERE path_key >= ?1 AND path_key < ?2 ORDER BY path_key",
        )?;
        let rows = stmt.query_map(params![lo, hi], |r| {
            let key: String = r.get(1)?;
            Ok(DirRow {
                path: r.get(0)?,
                read_error: error_dirs.contains(&key),
                key,
                parent_key: r.get(2)?,
                summary: r.get::<_, String>(3)? == "summary",
                attrs: r.get(4)?,
                is_link: r.get(5)?,
                direct_entries: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Alle Dateien unterhalb des Ordners (eine Abfrage).
    pub fn files_under(&self, dir_key: &str) -> Result<Vec<FileRow>> {
        let (lo, hi) = paths::prefix_range(dir_key);
        let mut stmt = self.conn().prepare(
            "SELECT path, path_key, dir_key, name, size, mtime, attrs, cloud_only, is_link
             FROM files WHERE path_key >= ?1 AND path_key < ?2 ORDER BY path_key",
        )?;
        let rows = stmt.query_map(params![lo, hi], |r| {
            Ok(FileRow {
                path: r.get(0)?,
                key: r.get(1)?,
                dir_key: r.get(2)?,
                name: r.get(3)?,
                size: r.get(4)?,
                mtime: r.get(5)?,
                attrs: r.get(6)?,
                cloud_only: r.get(7)?,
                is_link: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Schlüssel der Ordner unterhalb von `dir_key`, für die der Scan einen Fehler meldete.
    /// Ein Ordner, dessen Lesen scheiterte, sieht im Index leer aus (`direct_entries` = 0).
    fn error_dir_keys(&self, dir_key: &str) -> Result<HashSet<String>> {
        let mut stmt = self.conn().prepare("SELECT path FROM errors")?;
        let paths: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(paths
            .iter()
            .map(|p| paths::dir_key(std::path::Path::new(p)))
            .filter(|k| paths::is_under(k, dir_key))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::index::{DirRecord, FileRecord, ScanErrorRecord};

    fn dir(path: &str) -> DirRecord {
        let key = paths::dir_key(Path::new(path));
        let parent = Path::new(path).parent().map(paths::dir_key);
        DirRecord {
            path: path.into(),
            path_key: key,
            parent_key: parent,
            mode: "full".into(),
            ..DirRecord::default()
        }
    }

    fn file(path: &str, size: i64) -> FileRecord {
        let p = Path::new(path);
        FileRecord {
            dir_key: paths::dir_key(p.parent().unwrap()),
            path: path.into(),
            path_key: paths::path_key(p),
            name: p.file_name().unwrap().to_string_lossy().into_owned(),
            size,
            mtime: 42,
            ..FileRecord::default()
        }
    }

    fn seeded() -> Index {
        let mut idx = Index::open_in_memory().unwrap();
        let root = paths::dir_key(Path::new(r"D:\Daten"));
        let run = idx.begin_root(r"D:\Daten", &root, "t").unwrap();
        idx.upsert_dirs(
            &[
                DirRecord {
                    direct_entries: 2,
                    ..dir(r"D:\Daten")
                },
                DirRecord {
                    direct_entries: 1,
                    ..dir(r"D:\Daten\a")
                },
                dir(r"D:\Daten\a\tief"),
                DirRecord {
                    mode: "summary".into(),
                    attrs: 0x2,
                    ..dir(r"D:\Daten\node_modules")
                },
                DirRecord {
                    is_link: true,
                    ..dir(r"D:\Daten\link")
                },
                dir(r"D:\Daten2"),
                dir(r"D:\Daten2\x"),
            ],
            run.generation,
        )
        .unwrap();
        idx.upsert_files(
            &[
                file(r"D:\Daten\a\x.txt", 10),
                file(r"D:\Daten\a\tief\y.txt", 20),
                FileRecord {
                    cloud_only: true,
                    attrs: 0x40_0000,
                    ..file(r"D:\Daten\wolke.docx", 30)
                },
                FileRecord {
                    is_link: true,
                    ..file(r"D:\Daten\verweis.lnk", 0)
                },
                file(r"D:\Daten2\fremd.txt", 99),
            ],
            run.generation,
        )
        .unwrap();
        idx
    }

    fn keys(rows: &[DirRow]) -> Vec<String> {
        let mut k: Vec<String> = rows.iter().map(|r| r.key.clone()).collect();
        k.sort();
        k
    }

    #[test]
    fn dirs_under_liefert_den_ordner_und_alle_unterordner_aber_keine_nachbarn() {
        let idx = seeded();
        let rows = idx
            .dirs_under(&paths::dir_key(Path::new(r"D:\Daten")))
            .unwrap();
        assert_eq!(
            keys(&rows),
            [
                r"D:\daten\",
                r"D:\daten\a\",
                r"D:\daten\a\tief\",
                r"D:\daten\link\",
                r"D:\daten\node_modules\"
            ]
        );
    }

    #[test]
    fn dirs_under_fuer_unterordner_beschraenkt_sich_auf_diesen() {
        let idx = seeded();
        let rows = idx
            .dirs_under(&paths::dir_key(Path::new(r"D:\Daten\a")))
            .unwrap();
        assert_eq!(keys(&rows), [r"D:\daten\a\", r"D:\daten\a\tief\"]);
    }

    #[test]
    fn dir_row_traegt_die_spalten_der_planer() {
        let idx = seeded();
        let rows = idx
            .dirs_under(&paths::dir_key(Path::new(r"D:\Daten")))
            .unwrap();
        let by_key = |k: &str| rows.iter().find(|r| r.key == k).unwrap();

        let root = by_key(r"D:\daten\");
        assert_eq!(root.path, r"D:\Daten");
        assert_eq!(root.parent_key.as_deref(), Some(r"D:\"));
        assert_eq!(root.direct_entries, 2);
        assert!(!root.summary && !root.is_link && !root.read_error);

        let summary = by_key(r"D:\daten\node_modules\");
        assert!(summary.summary);
        assert_eq!(summary.attrs, 0x2);
        assert!(by_key(r"D:\daten\link\").is_link);
    }

    #[test]
    fn files_under_liefert_nur_dateien_unterhalb() {
        let idx = seeded();
        let mut rows = idx
            .files_under(&paths::dir_key(Path::new(r"D:\Daten")))
            .unwrap();
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["y.txt", "x.txt", "verweis.lnk", "wolke.docx"]);
        let sub = idx
            .files_under(&paths::dir_key(Path::new(r"D:\Daten\a")))
            .unwrap();
        assert_eq!(sub.len(), 2);
    }

    #[test]
    fn file_row_traegt_die_spalten_der_planer() {
        let idx = seeded();
        let rows = idx
            .files_under(&paths::dir_key(Path::new(r"D:\Daten")))
            .unwrap();
        let find = |n: &str| rows.iter().find(|r| r.name == n).unwrap();

        let x = find("x.txt");
        assert_eq!(x.path, r"D:\Daten\a\x.txt");
        assert_eq!(x.key, r"D:\daten\a\x.txt");
        assert_eq!(x.dir_key, r"D:\daten\a\");
        assert_eq!((x.size, x.mtime), (10, 42));
        assert!(!x.cloud_only && !x.is_link);

        let wolke = find("wolke.docx");
        assert!(wolke.cloud_only);
        assert_eq!(wolke.attrs, 0x40_0000);
        assert!(find("verweis.lnk").is_link);
    }

    #[test]
    fn lesefehler_eines_ordners_wird_markiert_unabhaengig_von_der_schreibweise() {
        let mut idx = seeded();
        let run = crate::index::RootRun {
            path_key: paths::dir_key(Path::new(r"D:\Daten")),
            generation: 1,
        };
        idx.record_error(
            &run,
            &ScanErrorRecord {
                path: r"D:\DATEN\a\tief".into(),
                kind: "access_denied".into(),
                message: "Zugriff verweigert".into(),
            },
        )
        .unwrap();
        let rows = idx
            .dirs_under(&paths::dir_key(Path::new(r"D:\Daten")))
            .unwrap();
        let flagged: Vec<&str> = rows
            .iter()
            .filter(|r| r.read_error)
            .map(|r| r.key.as_str())
            .collect();
        assert_eq!(flagged, [r"D:\daten\a\tief\"]);
    }
}
