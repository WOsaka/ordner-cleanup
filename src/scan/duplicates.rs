//! Duplikat-Pipeline: Größe → Hardlinks zusammenfassen → Teil-Hash → Voll-Hash.

use std::collections::HashMap;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use rayon::prelude::*;

use super::hasher::{self, SMALL_MAX};
use super::source::filetime_to_ticks;
use crate::index::{HashUpdate, Index, RootRun, ScanErrorRecord};
use crate::paths;
use crate::platform::windows::{file_identity, FileIdentity};
use crate::platform::FileAttrs;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HashStats {
    /// Dateien in Größengruppen mit mindestens einer weiteren Datei.
    pub candidates: u64,
    /// Dateien, die nicht gehasht werden mussten (eindeutige Größe, nur Hardlinks).
    pub skipped_unique: u64,
    pub partial_hashed: u64,
    pub full_hashed: u64,
    pub locked: u64,
    pub changed: u64,
    pub errors: u64,
}

#[derive(Debug, Clone)]
enum Failure {
    /// Datei ist inzwischen ein Cloud-Platzhalter: nicht anfassen, kein Fehler.
    Cloud,
    Locked,
    Changed,
    Vanished,
    Io(String),
}

impl Failure {
    fn classify(e: &io::Error) -> Self {
        if hasher::is_locked(e) {
            Self::Locked
        } else if e.kind() == io::ErrorKind::NotFound {
            Self::Vanished
        } else if e.kind() == io::ErrorKind::InvalidData {
            Self::Changed
        } else {
            Self::Io(e.to_string())
        }
    }

    fn status(&self) -> Option<&'static str> {
        match self {
            Self::Cloud => None,
            Self::Locked => Some("locked"),
            Self::Changed => Some("changed"),
            Self::Vanished | Self::Io(_) => Some("error"),
        }
    }

    fn error_record(&self, path: &str) -> Option<ScanErrorRecord> {
        let (kind, message) = match self {
            Self::Cloud => return None,
            Self::Locked => (
                "locked",
                "Datei ist gesperrt, Hash nicht ermittelbar".to_string(),
            ),
            Self::Changed => (
                "changed",
                "Datei hat sich während des Scans geändert".to_string(),
            ),
            Self::Vanished => ("vanished", "Datei ist verschwunden".to_string()),
            Self::Io(m) => ("io", m.clone()),
        };
        Some(ScanErrorRecord {
            path: path.to_string(),
            kind: kind.to_string(),
            message,
        })
    }
}

struct Cand {
    id: i64,
    path: String,
    size: u64,
    mtime: i64,
    partial: Option<Vec<u8>>,
    full: Option<Vec<u8>>,
    identity: Option<FileIdentity>,
    failure: Option<Failure>,
    did_partial: bool,
    did_full: bool,
}

/// Prüft, dass die Datei noch so aussieht wie im Index und kein Cloud-Platzhalter ist.
/// `symlink_metadata` öffnet nur mit Attribut-Zugriff (kein Cloud-Recall).
fn verify(c: &Cand) -> Result<(), Failure> {
    let meta = std::fs::symlink_metadata(paths::extended(std::path::Path::new(&c.path)))
        .map_err(|e| Failure::classify(&e))?;
    if FileAttrs(meta.file_attributes()).is_cloud_only() {
        return Err(Failure::Cloud);
    }
    if meta.len() != c.size || filetime_to_ticks(meta.last_write_time()) != c.mtime {
        return Err(Failure::Changed);
    }
    Ok(())
}

/// (Teil-Hash, Voll-Hash) – je nach Stufe ist nur einer gesetzt.
type Digests = (Option<Vec<u8>>, Option<Vec<u8>>);

fn compute(c: &mut Cand, full: bool, cancel: &AtomicBool) {
    if c.failure.is_some() || cancel.load(Ordering::Relaxed) {
        return;
    }
    let result = (|| -> Result<Digests, Failure> {
        verify(c)?;
        let path = std::path::Path::new(&c.path);
        let mut file = hasher::open_read(path).map_err(|e| Failure::classify(&e))?;
        let hashes = if full {
            let h = hasher::full_hash(&mut file, c.size).map_err(|e| Failure::classify(&e))?;
            (None, Some(h))
        } else {
            let h = hasher::partial_hash(&mut file, c.size).map_err(|e| Failure::classify(&e))?;
            (Some(h), None)
        };
        // Nach dem Lesen erneut prüfen: war die Datei währenddessen in Bewegung?
        verify(c)?;
        Ok(hashes)
    })();
    match result {
        Ok((partial, full_hash)) => {
            if let Some(p) = partial {
                c.did_partial = true;
                if c.size <= SMALL_MAX {
                    c.full = Some(p.clone());
                }
                c.partial = Some(p);
            }
            if let Some(f) = full_hash {
                c.did_full = true;
                c.full = Some(f);
            }
        }
        Err(f) => {
            c.partial = None;
            c.full = None;
            c.failure = Some(f);
        }
    }
}

pub fn run(
    index: &mut Index,
    pool: &rayon::ThreadPool,
    root: &RootRun,
    cancel: &AtomicBool,
) -> Result<HashStats> {
    let candidates = index.hash_candidates(&root.path_key)?;
    let hashable = index.hashable_count(&root.path_key)? as u64;
    let mut stats = HashStats {
        candidates: candidates.len() as u64,
        skipped_unique: hashable - candidates.len() as u64,
        ..HashStats::default()
    };
    let mut cands: Vec<Cand> = candidates
        .into_iter()
        .map(|c| Cand {
            id: c.id,
            path: c.path,
            size: c.size as u64,
            mtime: c.mtime,
            partial: c.partial_hash,
            full: c.full_hash,
            identity: None,
            failure: None,
            did_partial: false,
            did_full: false,
        })
        .collect();

    // Stufe 1: Datei-Identität nur für Kandidaten (Hardlinks haben dieselbe Größe).
    pool.install(|| {
        cands
            .par_iter_mut()
            .for_each(|c| c.identity = file_identity(std::path::Path::new(&c.path)).ok());
    });

    // Stufe 2: Größengruppen, Hardlinks zu einem Vertreter zusammenfassen.
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, c) in cands.iter().enumerate() {
        by_size.entry(c.size).or_default().push(i);
    }
    let mut rep_of: Vec<usize> = (0..cands.len()).collect();
    let mut active: Vec<usize> = Vec::new();
    for idxs in by_size.values() {
        let mut seen: HashMap<(u32, u64), usize> = HashMap::new();
        let mut reps = Vec::new();
        for &i in idxs {
            match cands[i].identity {
                Some(id) => match seen.get(&(id.volume_serial, id.file_index)) {
                    Some(&rep) => rep_of[i] = rep,
                    None => {
                        seen.insert((id.volume_serial, id.file_index), i);
                        reps.push(i);
                    }
                },
                None => reps.push(i),
            }
        }
        if reps.len() >= 2 {
            active.extend(reps);
        } else {
            stats.skipped_unique += idxs.len() as u64;
        }
    }

    // Stufe 3: Teil-Hash.
    let active_set: std::collections::HashSet<usize> = active.iter().copied().collect();
    pool.install(|| {
        cands
            .par_iter_mut()
            .enumerate()
            .filter(|(i, c)| active_set.contains(i) && c.partial.is_none())
            .for_each(|(_, c)| compute(c, false, cancel));
    });

    // Stufe 4: Voll-Hash nur für Vertreter mit gleichem Teil-Hash.
    let mut by_partial: HashMap<(u64, Vec<u8>), Vec<usize>> = HashMap::new();
    for &i in &active {
        let c = &cands[i];
        if c.failure.is_none() {
            if let Some(p) = &c.partial {
                by_partial.entry((c.size, p.clone())).or_default().push(i);
            }
        }
    }
    let need_full: std::collections::HashSet<usize> = by_partial
        .values()
        .filter(|g| g.len() >= 2)
        .flatten()
        .copied()
        .filter(|&i| cands[i].full.is_none())
        .collect();
    pool.install(|| {
        cands
            .par_iter_mut()
            .enumerate()
            .filter(|(i, _)| need_full.contains(i))
            .for_each(|(_, c)| compute(c, true, cancel));
    });

    // Ergebnisse schreiben; Hardlink-Geschwister erben die Hashes ihres Vertreters.
    let mut updates = Vec::with_capacity(cands.len());
    for (i, c) in cands.iter().enumerate() {
        let src = &cands[rep_of[i]];
        let status = match &src.failure {
            Some(f) => f.status(),
            None if src.partial.is_some() || src.full.is_some() => Some("ok"),
            None => None,
        };
        updates.push(HashUpdate {
            id: c.id,
            volume_serial: c.identity.map(|x| i64::from(x.volume_serial)),
            // Der File-Index ist 64 Bit; als i64 gespeichert (Bitmuster bleibt erhalten).
            file_index: c.identity.map(|x| x.file_index as i64),
            nlinks: c.identity.map(|x| i64::from(x.nlinks)),
            partial_hash: src.partial.clone(),
            full_hash: src.full.clone(),
            hash_status: status.map(str::to_string),
        });
    }
    index.store_hash_updates(&updates)?;

    let mut failures = 0;
    for (i, c) in cands.iter().enumerate() {
        stats.partial_hashed += u64::from(c.did_partial);
        stats.full_hashed += u64::from(c.did_full);
        if rep_of[i] != i {
            continue;
        }
        if let Some(f) = &c.failure {
            match f {
                Failure::Locked => stats.locked += 1,
                Failure::Changed => stats.changed += 1,
                Failure::Vanished | Failure::Io(_) => stats.errors += 1,
                Failure::Cloud => {}
            }
            if let Some(record) = f.error_record(&c.path) {
                index.record_error(root, &record)?;
                failures += 1;
            }
        }
    }
    index.add_root_errors(&root.path_key, failures)?;
    Ok(stats)
}
