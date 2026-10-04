//! Index-Abfragen und -Updates für die Duplikat-Pipeline.

use std::collections::HashMap;

use rusqlite::params;

use super::{Index, Result};
use crate::paths;

/// Datei, die in einer Größengruppe mit mindestens einer weiteren liegt.
#[derive(Debug, Clone)]
pub struct HashCandidate {
    pub id: i64,
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub partial_hash: Option<Vec<u8>>,
    pub full_hash: Option<Vec<u8>>,
}

/// Ergebnis der Pipeline für eine Datei (ersetzt die Spalten vollständig).
#[derive(Debug, Clone, Default)]
pub struct HashUpdate {
    pub id: i64,
    pub volume_serial: Option<i64>,
    pub file_index: Option<i64>,
    pub nlinks: Option<i64>,
    pub partial_hash: Option<Vec<u8>>,
    pub full_hash: Option<Vec<u8>>,
    pub hash_status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DupFile {
    pub path: String,
    pub mtime: i64,
    pub nlinks: i64,
    /// `(volume_serial, file_index)`; gleich bei Hardlinks derselben Datei.
    pub identity: Option<(i64, i64)>,
}

/// Gruppe identischer Dateien (gleicher Voll-Hash). Hardlinks zählen als eine Instanz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DupGroup {
    pub size: i64,
    pub hash: Vec<u8>,
    pub instances: usize,
    pub wasted: i64,
    pub files: Vec<DupFile>,
}

const LOCAL_FILE: &str = "cloud_only = 0 AND is_link = 0 AND size > 0";

impl Index {
    /// Anzahl lokal lesbarer, nicht leerer Dateien unterhalb der Wurzel.
    pub fn hashable_count(&self, dir_key: &str) -> Result<i64> {
        let (lo, hi) = paths::prefix_range(dir_key);
        Ok(self.conn().query_row(
            &format!(
                "SELECT COUNT(*) FROM files WHERE path_key >= ?1 AND path_key < ?2 AND {LOCAL_FILE}"
            ),
            params![lo, hi],
            |r| r.get(0),
        )?)
    }

    /// Dateien mit mindestens einer weiteren Datei gleicher Größe (nur lokal verfügbare).
    pub fn hash_candidates(&self, dir_key: &str) -> Result<Vec<HashCandidate>> {
        let (lo, hi) = paths::prefix_range(dir_key);
        let mut stmt = self.conn().prepare(&format!(
            "SELECT id, path, size, mtime, partial_hash, full_hash FROM files
             WHERE path_key >= ?1 AND path_key < ?2 AND {LOCAL_FILE}
               AND size IN (SELECT size FROM files
                            WHERE path_key >= ?1 AND path_key < ?2 AND {LOCAL_FILE}
                            GROUP BY size HAVING COUNT(*) > 1)"
        ))?;
        let rows = stmt.query_map(params![lo, hi], |r| {
            Ok(HashCandidate {
                id: r.get(0)?,
                path: r.get(1)?,
                size: r.get(2)?,
                mtime: r.get(3)?,
                partial_hash: r.get(4)?,
                full_hash: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn store_hash_updates(&mut self, updates: &[HashUpdate]) -> Result<()> {
        let tx = self.conn_mut().transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "UPDATE files SET volume_serial = ?2, file_index = ?3, nlinks = ?4,
                                  partial_hash = ?5, full_hash = ?6, hash_status = ?7
                 WHERE id = ?1",
            )?;
            for u in updates {
                stmt.execute(params![
                    u.id,
                    u.volume_serial,
                    u.file_index,
                    u.nlinks,
                    u.partial_hash,
                    u.full_hash,
                    u.hash_status
                ])?;
            }
        }
        Ok(tx.commit()?)
    }

    /// Ergänzt Fehler, die nach `finish_root` entstanden sind (Hash-Phase).
    pub fn add_root_errors(&mut self, dir_key: &str, count: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE roots SET error_count = error_count + ?1 WHERE path_key = ?2",
            params![count, dir_key],
        )?;
        Ok(())
    }

    /// Exakte Duplikatgruppen (Voll-Hash) unterhalb der Wurzel, nach Verschwendung sortiert.
    pub fn exact_duplicate_groups(&self, dir_key: &str) -> Result<Vec<DupGroup>> {
        let (lo, hi) = paths::prefix_range(dir_key);
        let mut stmt = self.conn().prepare(
            "SELECT path, size, mtime, full_hash, volume_serial, file_index, nlinks FROM files
             WHERE path_key >= ?1 AND path_key < ?2 AND full_hash IS NOT NULL
               AND hash_status = 'ok' AND cloud_only = 0 AND is_link = 0
               AND (size, full_hash) IN (
                   SELECT size, full_hash FROM files
                   WHERE path_key >= ?1 AND path_key < ?2 AND full_hash IS NOT NULL
                     AND hash_status = 'ok' AND cloud_only = 0 AND is_link = 0
                   GROUP BY size, full_hash HAVING COUNT(*) > 1)
             ORDER BY path_key",
        )?;
        let mut groups: HashMap<(i64, Vec<u8>), Vec<DupFile>> = HashMap::new();
        let rows = stmt.query_map(params![lo, hi], |r| {
            let size: i64 = r.get(1)?;
            let hash: Vec<u8> = r.get(3)?;
            let volume: Option<i64> = r.get(4)?;
            let index: Option<i64> = r.get(5)?;
            Ok((
                (size, hash),
                DupFile {
                    path: r.get(0)?,
                    mtime: r.get(2)?,
                    nlinks: r.get::<_, Option<i64>>(6)?.unwrap_or(1),
                    identity: volume.zip(index),
                },
            ))
        })?;
        for row in rows {
            let (key, value) = row?;
            groups.entry(key).or_default().push(value);
        }
        let mut result = Vec::new();
        for ((size, hash), members) in groups {
            let mut identities = std::collections::HashSet::new();
            let mut instances = 0;
            for file in &members {
                // Ohne bekannte Identität zählt jede Datei einzeln.
                if file.identity.is_none_or(|id| identities.insert(id)) {
                    instances += 1;
                }
            }
            if instances >= 2 {
                result.push(DupGroup {
                    size,
                    hash,
                    instances,
                    wasted: size * (instances as i64 - 1),
                    files: members,
                });
            }
        }
        result.sort_by(|a, b| {
            b.wasted
                .cmp(&a.wasted)
                .then_with(|| a.files[0].path.cmp(&b.files[0].path))
        });
        Ok(result)
    }
}
