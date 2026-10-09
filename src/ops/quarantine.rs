//! Quarantäne-Ansicht: Liste über alle Wurzeln, Vorschau und Zurückholen einzelner Einträge.
//! Rückfragen stellt der Aufrufer zwischen `restore_check` und `restore_execute`.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{known_roots, load_config, normalize, now_rfc3339, OpCtx};
use crate::change::fsops::RealFs;
use crate::change::restore::{
    list_quarantine, preview_restore, restore_items, ItemOutcome, ItemResult, QuarantineItem,
    RestorePreview,
};
use crate::change::undo::UndoEnv;
use crate::change::RunId;
use crate::paths;

/// Die Quarantäne-Einträge einer Wurzel.
#[derive(Debug)]
pub struct RootItems {
    pub root: PathBuf,
    pub items: Vec<QuarantineItem>,
}

#[derive(Debug, Default)]
pub struct QuarantineList {
    pub roots: Vec<RootItems>,
    /// Journale, die sich nicht lesen ließen (alle Wurzeln zusammen).
    pub unreadable: usize,
    /// Wurzeln, die nicht erreichbar sind (z. B. getrenntes Laufwerk).
    pub unreachable: Vec<PathBuf>,
}

impl QuarantineList {
    pub fn item_count(&self) -> usize {
        self.roots.iter().map(|r| r.items.len()).sum()
    }
}

/// Ein ausgewählter Eintrag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    pub root: PathBuf,
    pub run: RunId,
    pub action: u32,
}

/// Alle Quarantäne-Einträge; ohne `root` über alle bekannten Wurzeln. Nicht erreichbare
/// Wurzeln werden gemeldet statt als Fehler gewertet.
pub fn quarantine_list(root: Option<&Path>) -> Result<QuarantineList> {
    let config = load_config()?;
    let roots = match root {
        Some(path) => vec![normalize(path)],
        None => known_roots()?,
    };
    Ok(list_roots(&roots, config.quarantine_days))
}

fn list_roots(roots: &[PathBuf], quarantine_days: u32) -> QuarantineList {
    let mut out = QuarantineList::default();
    for root in roots {
        if std::fs::metadata(paths::extended(root)).is_err() {
            out.unreachable.push(root.clone());
            continue;
        }
        match list_quarantine(root, quarantine_days, &RealFs) {
            Ok(listing) => {
                out.unreadable += listing.unreadable;
                if !listing.items.is_empty() {
                    out.roots.push(RootItems {
                        root: root.clone(),
                        items: listing.items,
                    });
                }
            }
            Err(_) => out.unreadable += 1,
        }
    }
    out
}

/// Gruppiert eine Auswahl nach Wurzel und Lauf (Reihenfolge des ersten Auftretens).
fn group(selection: &[Selected]) -> Vec<(PathBuf, RunId, Vec<u32>)> {
    let mut groups: Vec<(String, PathBuf, RunId, Vec<u32>)> = Vec::new();
    for item in selection {
        let key = paths::path_key(&item.root);
        match groups
            .iter_mut()
            .find(|(k, _, run, _)| *k == key && *run == item.run)
        {
            Some((_, _, _, actions)) => actions.push(item.action),
            None => groups.push((key, item.root.clone(), item.run.clone(), vec![item.action])),
        }
    }
    groups
        .into_iter()
        .map(|(_, root, run, actions)| (root, run, actions))
        .collect()
}

/// Zeigt für die Auswahl Ziel und Hash-Prüfung; verändert nichts.
pub fn restore_check(selection: &[Selected]) -> Result<Vec<(PathBuf, RestorePreview)>> {
    let mut out = Vec::new();
    for (root, run, actions) in group(selection) {
        let picked: Vec<(RunId, u32)> = actions.iter().map(|&a| (run.clone(), a)).collect();
        for preview in preview_restore(&root, &picked, &RealFs)? {
            out.push((root.clone(), preview));
        }
    }
    Ok(out)
}

/// Holt die Auswahl zurück. Ein Abbruch beendet nach dem laufenden Eintrag; ein Fehler in
/// einem Lauf wird zum Ergebnis seiner Einträge, die übrigen Läufe laufen weiter.
pub fn restore_execute(
    selection: &[Selected],
    ctx: &OpCtx,
) -> Result<Vec<(PathBuf, RunId, ItemResult)>> {
    ctx.progress.reset("Zurückholen", selection.len() as u64);
    let stamp = now_rfc3339();
    let env = UndoEnv {
        fs: &RealFs,
        now: &stamp,
        cancel: &ctx.cancel,
    };
    let mut out = Vec::new();
    for (root, run, actions) in group(selection) {
        if ctx.is_cancelled() {
            break;
        }
        ctx.progress.set_current(run.to_string());
        let results = restore_items(&root, &run, &actions, &env).unwrap_or_else(|e| {
            actions
                .iter()
                .map(|&action| ItemResult {
                    action,
                    origin: String::new(),
                    outcome: ItemOutcome::Failed(e.to_string()),
                })
                .collect()
        });
        for result in results {
            ctx.progress.inc();
            out.push((root.clone(), run.clone(), result));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::restore::ItemOutcome;
    use std::sync::atomic::Ordering;

    const RUN_A: &str = "20261003-120000-ab12";
    const RUN_B: &str = "20261004-120000-cd34";

    fn sel(root: &str, run: &str, action: u32) -> Selected {
        Selected {
            root: PathBuf::from(root),
            run: RunId::parse(run).unwrap(),
            action,
        }
    }

    #[test]
    fn auswahl_wird_nach_wurzel_und_lauf_gruppiert() {
        let selection = [
            sel(r"D:\a", RUN_A, 1),
            sel(r"D:\b", RUN_A, 1),
            sel(r"D:\a", RUN_A, 3),
            sel(r"D:\a", RUN_B, 2),
            sel(r"d:\A", RUN_A, 5),
        ];
        let groups = group(&selection);
        let shape: Vec<_> = groups
            .iter()
            .map(|(root, run, actions)| {
                (
                    root.to_string_lossy().to_string(),
                    run.to_string(),
                    actions.clone(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (r"D:\a".to_string(), RUN_A.to_string(), vec![1, 3, 5]),
                (r"D:\b".to_string(), RUN_A.to_string(), vec![1]),
                (r"D:\a".to_string(), RUN_B.to_string(), vec![2]),
            ]
        );
    }

    #[test]
    fn nicht_erreichbare_wurzel_ist_ein_hinweis_und_kein_fehler() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gibt-es-nicht");
        let list = list_roots(&[dir.path().to_path_buf(), missing.clone()], 30);
        assert_eq!(list.unreachable, [missing]);
        assert_eq!(list.item_count(), 0);
        assert_eq!(list.unreadable, 0);
    }

    #[test]
    fn abbruch_vor_dem_ersten_eintrag_liefert_leeres_ergebnis() {
        let ctx = OpCtx::default();
        ctx.cancel.store(true, Ordering::Relaxed);
        let results = restore_execute(&[sel(r"Z:\gibt-es-nicht", RUN_A, 1)], &ctx).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn fehler_in_einem_lauf_wird_zum_ergebnis_seiner_eintraege() {
        let ctx = OpCtx::default();
        let selection = [
            sel(r"Z:\gibt-es-nicht", RUN_A, 1),
            sel(r"Z:\gibt-es-nicht", RUN_A, 2),
            sel(r"Z:\gibt-es-nicht", RUN_B, 3),
        ];
        let results = restore_execute(&selection, &ctx).unwrap();
        assert_eq!(results.len(), 3, "beide Läufe wurden versucht");
        assert!(results
            .iter()
            .all(|(_, _, r)| matches!(r.outcome, ItemOutcome::Failed(_))));
        let actions: Vec<_> = results.iter().map(|(_, _, r)| r.action).collect();
        assert_eq!(actions, [1, 2, 3]);
    }

    #[test]
    fn vorschau_fuer_unbekannten_lauf_ist_ein_fehler() {
        assert!(restore_check(&[sel(r"Z:\gibt-es-nicht", RUN_A, 1)]).is_err());
    }
}
