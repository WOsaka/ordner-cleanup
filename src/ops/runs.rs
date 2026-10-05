//! Läufe: `runs`, `undo` und `purge`. Rückfragen stellt der Aufrufer zwischen `*_check` und
//! `*_execute`.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use anyhow::{Context, Result};

use super::{find_run_root, known_roots, load_config, normalize, now_rfc3339, OpCtx};
use crate::analysis::age::parse_old_after;
use crate::change::fsops::RealFs;
use crate::change::undo::{
    expired_runs, list_runs, purge_run, undo_run, RunStatus, RunSummary, UndoEnv, UndoError,
    UndoOutcome,
};
use crate::change::RunId;

/// Läufe je Wurzel (Wurzeln ohne Läufe fehlen). Ohne `root` alle bekannten Wurzeln.
pub fn runs(root: Option<&Path>) -> Result<Vec<(PathBuf, Vec<RunSummary>)>> {
    let config = load_config()?;
    let roots = match root {
        Some(path) => vec![normalize(path)],
        None => known_roots()?,
    };
    let mut out = Vec::new();
    for root in roots {
        let runs = list_runs(&root, config.quarantine_days)?;
        if !runs.is_empty() {
            out.push((root, runs));
        }
    }
    Ok(out)
}

/// Was ein Undo vorfindet.
#[derive(Debug)]
pub struct UndoCheck {
    pub root: PathBuf,
    pub summary: RunSummary,
}

impl UndoCheck {
    pub fn already_undone(&self) -> bool {
        self.summary.status == RunStatus::Undone
    }

    /// Die Quarantäne wurde gelöscht: nichts mehr wiederherstellbar.
    pub fn purged(&self) -> bool {
        self.summary.status == RunStatus::Purged
    }
}

/// Sucht den Lauf (Wurzel ausdrücklich oder aus dem Register); verändert nichts.
pub fn undo_check(run: &RunId, root: Option<&Path>) -> Result<UndoCheck> {
    let config = load_config()?;
    let root = find_run_root(run, root)?;
    let summary = list_runs(&root, config.quarantine_days)?
        .into_iter()
        .find(|s| s.run == *run)
        .ok_or_else(|| UndoError::NotFound(run.clone()))?;
    Ok(UndoCheck { root, summary })
}

/// Dreht den Lauf zurück; ein Abbruch hinterlässt `PartiallyUndone`, ein weiteres `undo` setzt
/// fort.
pub fn undo_execute(run: &RunId, root: &Path, ctx: &OpCtx) -> Result<UndoOutcome> {
    ctx.progress.reset("Rückgängig", 0);
    Ok(undo_run(
        root,
        run,
        &UndoEnv {
            fs: &RealFs,
            now: &now_rfc3339(),
            cancel: &ctx.cancel,
        },
    )?)
}

/// Ein abgelaufener Lauf, dessen Quarantäne gelöscht werden kann.
#[derive(Debug, Clone)]
pub struct PurgeCandidate {
    pub root: PathBuf,
    pub run: RunSummary,
}

#[derive(Debug)]
pub struct PurgeCheck {
    /// Mindestalter in Tagen
    pub days: u32,
    pub candidates: Vec<PurgeCandidate>,
}

impl PurgeCheck {
    pub fn total_bytes(&self) -> u64 {
        self.candidates.iter().map(|c| c.run.bytes).sum()
    }
}

/// Abgelaufene Läufe (Mindestalter `older_than`, sonst `quarantine_days` der Config).
pub fn purge_candidates(older_than: Option<&str>, root: Option<&Path>) -> Result<PurgeCheck> {
    let config = load_config()?;
    let days = match older_than {
        Some(text) => u32::try_from(parse_old_after(text).map_err(anyhow::Error::msg)?)
            .context("Dauer ist zu groß")?,
        None => config.quarantine_days,
    };
    let roots = match root {
        Some(path) => vec![normalize(path)],
        None => known_roots()?,
    };
    let now = chrono::Utc::now();
    let mut candidates = Vec::new();
    for root in roots {
        for run in expired_runs(&root, days, now)? {
            candidates.push(PurgeCandidate {
                root: root.clone(),
                run,
            });
        }
    }
    Ok(PurgeCheck { days, candidates })
}

/// Löscht die Quarantäne der Läufe endgültig. Ein Abbruch beendet die Liste nach dem laufenden
/// Lauf; die übrigen kommen nicht im Ergebnis vor.
pub fn purge_execute(
    candidates: &[PurgeCandidate],
    ctx: &OpCtx,
) -> Result<Vec<(RunId, Result<(), UndoError>)>> {
    ctx.progress
        .reset("Quarantäne leeren", candidates.len() as u64);
    let stamp = now_rfc3339();
    let env = UndoEnv {
        fs: &RealFs,
        now: &stamp,
        cancel: &ctx.cancel,
    };
    let mut results = Vec::new();
    for c in candidates {
        if ctx.cancel.load(Ordering::Relaxed) {
            break;
        }
        ctx.progress.set_current(c.run.run.to_string());
        results.push((c.run.run.clone(), purge_run(&c.root, &c.run.run, &env)));
        ctx.progress.inc();
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::ActionCounts;

    fn candidate(n: u32) -> PurgeCandidate {
        let id = format!("20261003-12000{n}-ab12");
        PurgeCandidate {
            root: PathBuf::from(r"Z:\gibt-es-nicht"),
            run: RunSummary {
                run: RunId::parse(&id).unwrap(),
                started: None,
                moved: 0,
                counts: ActionCounts::default(),
                bytes: 7,
                status: RunStatus::Complete,
                expires: None,
            },
        }
    }

    #[test]
    fn purge_summiert_bytes_der_kandidaten() {
        let check = PurgeCheck {
            days: 30,
            candidates: vec![candidate(1), candidate(2)],
        };
        assert_eq!(check.total_bytes(), 14);
    }

    #[test]
    fn purge_stoppt_bei_gesetztem_abbruch_vor_dem_ersten_lauf() {
        let ctx = OpCtx::default();
        ctx.cancel.store(true, Ordering::Relaxed);
        let results = purge_execute(&[candidate(1), candidate(2)], &ctx).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn purge_meldet_fehler_je_lauf_statt_abzubrechen() {
        let ctx = OpCtx::default();
        let results = purge_execute(&[candidate(1), candidate(2)], &ctx).unwrap();
        assert_eq!(results.len(), 2, "beide Läufe wurden versucht");
        assert!(results.iter().all(|(_, r)| r.is_err()));
    }
}
