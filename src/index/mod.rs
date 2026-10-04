mod dups;
mod exif_cache;
mod listing;
mod store;

use std::path::Path;

use rusqlite::Connection;

pub use dups::{DupFile, DupGroup, HashCandidate, HashUpdate};
pub use exif_cache::ExifEntry;
pub use listing::{DirRow, FileRow};
pub use store::{DirRecord, FileRecord, PrevFile, RootInfo, RootRun, RootStatus, ScanErrorRecord};

pub const SCHEMA_VERSION: i64 = 2;
const SCHEMA_SQL: &str = include_str!("schema.sql");

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("Index hat Schema-Version {found}, erwartet wird {expected}; mit --reset-index neu aufbauen")]
    SchemaMismatch { found: i64, expected: i64 },
    #[error("Index ist beschädigt oder keine Datenbank ({0}); mit --reset-index neu aufbauen")]
    Unreadable(String),
    #[error("Datenbankfehler: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Dateifehler: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, IndexError>;

pub struct Index {
    conn: Connection,
}

impl Index {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).map_err(|e| IndexError::Unreadable(e.to_string()))?;
        Self::init(conn, true)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, false)
    }

    /// Löscht eine vorhandene Index-Datei samt WAL-Dateien (`--reset-index`).
    pub fn reset(path: &Path) -> Result<()> {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            match std::fs::remove_file(&name) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    fn init(conn: Connection, file_backed: bool) -> Result<Self> {
        let unreadable = |e: rusqlite::Error| IndexError::Unreadable(e.to_string());
        if file_backed {
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(unreadable)?;
        }
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let has_meta: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta')",
                [],
                |r| r.get(0),
            )
            .map_err(unreadable)?;
        if has_meta {
            let found: i64 = conn
                .query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .map_err(unreadable)?
                .parse()
                .map_err(|_| IndexError::Unreadable("schema_version ungültig".into()))?;
            if found == 1 {
                Self::migrate_v1_to_v2(&conn)?;
            } else if found != SCHEMA_VERSION {
                return Err(IndexError::SchemaMismatch {
                    found,
                    expected: SCHEMA_VERSION,
                });
            }
        } else {
            conn.execute_batch(SCHEMA_SQL)?;
            conn.execute(
                "INSERT INTO meta(key, value) VALUES('schema_version', ?1)",
                [SCHEMA_VERSION.to_string()],
            )?;
        }
        Ok(Self { conn })
    }

    /// v1 (Phase 1 bis 3) kennt nur die EXIF-Tabelle nicht; alles andere bleibt unverändert.
    fn migrate_v1_to_v2(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS exif_cache (
                 path_key TEXT PRIMARY KEY,
                 size INTEGER NOT NULL,
                 mtime INTEGER NOT NULL,
                 taken INTEGER
             );
             UPDATE meta SET value = '2' WHERE key = 'schema_version';
             COMMIT;",
        )?;
        Ok(())
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neuer_index_hat_schema_version() {
        let idx = Index::open_in_memory().unwrap();
        let v: String = idx
            .conn()
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION.to_string());
    }

    #[test]
    fn datei_index_laesst_sich_erneut_oeffnen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("index.db");
        drop(Index::open(&path).unwrap());
        assert!(Index::open(&path).is_ok());
    }

    #[test]
    fn falsche_schema_version_wird_erkannt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let idx = Index::open(&path).unwrap();
        idx.conn()
            .execute("UPDATE meta SET value='99' WHERE key='schema_version'", [])
            .unwrap();
        drop(idx);
        match Index::open(&path) {
            Err(IndexError::SchemaMismatch {
                found: 99,
                expected,
            }) => {
                assert_eq!(expected, SCHEMA_VERSION)
            }
            other => panic!("unerwartet: {:?}", other.err()),
        }
    }

    #[test]
    fn keine_datenbank_ist_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        std::fs::write(
            &path,
            "das ist keine sqlite-datei, aber lang genug ".repeat(50),
        )
        .unwrap();
        assert!(matches!(Index::open(&path), Err(IndexError::Unreadable(_))));
    }

    #[test]
    fn reset_loescht_und_ermoeglicht_neuaufbau() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        std::fs::write(&path, "kaputt".repeat(500)).unwrap();
        Index::reset(&path).unwrap();
        assert!(!path.exists());
        assert!(Index::open(&path).is_ok());
        Index::reset(&dir.path().join("fehlt.db")).unwrap();
    }
}
