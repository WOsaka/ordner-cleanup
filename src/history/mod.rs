//! Verlauf der Kennzahlen: eine eigene SQLite-Datei (`history.db`) neben dem Index.
//!
//! Sie ist unabhängig vom Index (`--reset-index` lässt sie stehen) und hat ein eigenes Schema mit
//! eigener Version. Eine beschädigte Datei oder ein neueres Schema ist ein Fehler; die Datei wird
//! dann weder überschrieben noch neu angelegt.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::health::{Area, Metrics, Snapshot, METRICS_VERSION};

pub const SCHEMA_VERSION: i64 = 1;
const SCHEMA_SQL: &str = include_str!("schema.sql");

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("Verlauf hat Schema-Version {found}, erwartet wird {expected}; die Datei bleibt unverändert")]
    NewerSchema { found: i64, expected: i64 },
    #[error("Verlauf ist beschädigt oder keine Datenbank ({0}); die Datei bleibt unverändert")]
    Unreadable(String),
    #[error("Datenbankfehler im Verlauf: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Verlauf enthält ungültige Daten: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Dateifehler: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, HistoryError>;

/// Alles zu einer Momentaufnahme außer den Zahlen selbst.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotMeta {
    pub root_key: String,
    pub root_path: String,
    /// UTC, RFC 3339
    pub taken_at: String,
    pub scan_finished_at: Option<String>,
    pub tool_version: String,
    pub metrics_version: i64,
    pub config_fp: String,
    pub template: Option<String>,
    pub profile: Option<String>,
    pub scan_errors: i64,
}

/// Eine gespeicherte Momentaufnahme.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub id: i64,
    pub meta: SnapshotMeta,
    pub snapshot: Snapshot,
}

/// Wie gut ein früherer Lauf mit dem jetzigen vergleichbar ist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparable {
    /// Gleiche Wurzel, gleicher Config-Fingerabdruck, gleiche Formeln
    Full,
    /// Config oder Formeln haben sich geändert; der Vergleich ist nur ein Anhaltspunkt
    Limited,
}

/// Ein Punkt einer Reihe (Score und Kennzahlen eines Bereichs zu einem Zeitpunkt).
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub id: i64,
    pub taken_at: String,
    pub config_fp: String,
    pub metrics_version: i64,
    pub score: u8,
    pub metrics: Metrics,
}

pub struct History {
    conn: Connection,
}

impl History {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let existed = path.exists();
        let conn = Connection::open(path).map_err(|e| HistoryError::Unreadable(e.to_string()))?;
        // GUI-Worker und ein geplanter CLI-Lauf teilen sich die Datei.
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| HistoryError::Unreadable(e.to_string()))?;
        Self::init(conn, existed)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, false)
    }

    fn init(conn: Connection, existed: bool) -> Result<Self> {
        let unreadable = |e: rusqlite::Error| HistoryError::Unreadable(e.to_string());
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
                .map_err(|_| HistoryError::Unreadable("schema_version ungültig".into()))?;
            if found > SCHEMA_VERSION {
                return Err(HistoryError::NewerSchema {
                    found,
                    expected: SCHEMA_VERSION,
                });
            }
            // Platz für Migrationen älterer Versionen; Version 1 ist die erste.
        } else {
            if existed {
                // Eine vorhandene Datei ohne unser Schema gehört uns nicht.
                let tables: i64 = conn
                    .query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))
                    .map_err(unreadable)?;
                if tables > 0 {
                    return Err(HistoryError::Unreadable("enthält fremde Tabellen".into()));
                }
            }
            conn.execute_batch(SCHEMA_SQL)?;
            conn.execute(
                "INSERT INTO meta(key, value) VALUES('schema_version', ?1)",
                [SCHEMA_VERSION.to_string()],
            )?;
        }
        Ok(Self { conn })
    }

    /// Schreibt eine Momentaufnahme (Wurzel und Ordner) in einer Transaktion.
    pub fn write(&mut self, snapshot: &Snapshot, meta: &SnapshotMeta) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots(root_key, root_path, taken_at, scan_finished_at, tool_version,
                                   metrics_version, config_fp, template, profile, scan_errors)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                meta.root_key,
                meta.root_path,
                meta.taken_at,
                meta.scan_finished_at,
                meta.tool_version,
                meta.metrics_version,
                meta.config_fp,
                meta.template,
                meta.profile,
                meta.scan_errors
            ],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                "INSERT INTO folder_metrics(snapshot_id, folder, score, metrics)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for area in std::iter::once(&snapshot.root).chain(&snapshot.folders) {
                stmt.execute(params![
                    id,
                    area.folder,
                    i64::from(area.score.total),
                    serde_json::to_string(area)?
                ])?;
            }
        }
        tx.commit()?;
        Ok(id)
    }

    fn load(&self, id: i64) -> Result<Option<Stored>> {
        let meta = self
            .conn
            .query_row(
                "SELECT root_key, root_path, taken_at, scan_finished_at, tool_version,
                        metrics_version, config_fp, template, profile, scan_errors
                 FROM snapshots WHERE id = ?1",
                [id],
                |r| {
                    Ok(SnapshotMeta {
                        root_key: r.get(0)?,
                        root_path: r.get(1)?,
                        taken_at: r.get(2)?,
                        scan_finished_at: r.get(3)?,
                        tool_version: r.get(4)?,
                        metrics_version: r.get(5)?,
                        config_fp: r.get(6)?,
                        template: r.get(7)?,
                        profile: r.get(8)?,
                        scan_errors: r.get(9)?,
                    })
                },
            )
            .optional()?;
        let Some(meta) = meta else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare("SELECT metrics FROM folder_metrics WHERE snapshot_id = ?1 ORDER BY rowid")?;
        let texts: Vec<String> = stmt
            .query_map([id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut root = None;
        let mut folders = Vec::new();
        for text in texts {
            let area: Area = serde_json::from_str(&text)?;
            if area.folder.is_empty() {
                root = Some(area);
            } else {
                folders.push(area);
            }
        }
        let root = root.ok_or_else(|| {
            HistoryError::Unreadable(format!("Momentaufnahme {id} ohne Wurzelzeile"))
        })?;
        Ok(Some(Stored {
            id,
            meta,
            snapshot: Snapshot { root, folders },
        }))
    }

    /// Der zuletzt geschriebene Lauf derselben Wurzel, mit dem verglichen wird: bevorzugt einer
    /// mit gleichem Fingerabdruck und gleichen Formeln (`Full`), sonst der letzte überhaupt
    /// (`Limited`). `before` schließt diese Id und alle späteren aus.
    pub fn latest_comparable(
        &self,
        root_key: &str,
        config_fp: &str,
        before: Option<i64>,
    ) -> Result<Option<(Stored, Comparable)>> {
        let before = before.unwrap_or(i64::MAX);
        let full: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM snapshots
                 WHERE root_key = ?1 AND config_fp = ?2 AND metrics_version = ?3 AND id < ?4
                 ORDER BY id DESC LIMIT 1",
                params![root_key, config_fp, METRICS_VERSION, before],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = full {
            return Ok(self.load(id)?.map(|s| (s, Comparable::Full)));
        }
        let any: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM snapshots WHERE root_key = ?1 AND id < ?2
                 ORDER BY id DESC LIMIT 1",
                params![root_key, before],
                |r| r.get(0),
            )
            .optional()?;
        match any {
            Some(id) => Ok(self.load(id)?.map(|s| (s, Comparable::Limited))),
            None => Ok(None),
        }
    }

    /// Die letzten `limit` Punkte eines Bereichs (`""` = Wurzel), älteste zuerst.
    pub fn series(&self, root_key: &str, folder: &str, limit: usize) -> Result<Vec<Point>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.taken_at, s.config_fp, s.metrics_version, f.score, f.metrics
             FROM snapshots s JOIN folder_metrics f ON f.snapshot_id = s.id
             WHERE s.root_key = ?1 AND f.folder = ?2
             ORDER BY s.taken_at DESC, s.id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![root_key, folder, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        let mut points = Vec::new();
        for row in rows {
            let (id, taken_at, config_fp, metrics_version, score, json) = row?;
            let area: Area = serde_json::from_str(&json)?;
            points.push(Point {
                id,
                taken_at,
                config_fp,
                metrics_version,
                score: score.clamp(0, 100) as u8,
                metrics: area.metrics,
            });
        }
        points.reverse();
        Ok(points)
    }

    /// Id der Momentaufnahme, die zu einem bestimmten Scan gehört (neueste zuerst gefunden).
    pub fn snapshot_for_scan(&self, root_key: &str, scan_finished_at: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM snapshots WHERE root_key = ?1 AND scan_finished_at = ?2
                 ORDER BY id DESC LIMIT 1",
                params![root_key, scan_finished_at],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Wurzeln mit Momentaufnahmen als `(Schlüssel, Pfad)`, alphabetisch.
    pub fn roots(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT root_key, MAX(root_path) FROM snapshots GROUP BY root_key ORDER BY root_key",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// Namen der Ordner der ersten Ebene, die im Verlauf einer Wurzel vorkommen.
    pub fn folders(&self, root_key: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT f.folder FROM snapshots s JOIN folder_metrics f ON f.snapshot_id = s.id
             WHERE s.root_key = ?1 AND f.folder <> '' ORDER BY f.folder",
        )?;
        let names = stmt
            .query_map([root_key], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(names)
    }
}

#[cfg(test)]
mod tests;
