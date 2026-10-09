//! `apply`: prüfen (`apply_check`) und ausführen (`apply_execute`). Die Rückfrage stellt der
//! Aufrufer dazwischen: die CLI auf der Konsole, die GUI per Dialog.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use super::{onedrive_roots_from_env, onedrive_warning, Error, Notes, OpCtx};
use crate::change::apply::{apply_plan, ApplyEnv, ApplyOutcome};
use crate::change::fsops::RealFs;
use crate::change::limits;
use crate::change::plan::{Plan, PlanError, PLAN_VERSION};
use crate::change::protect::{ProtectPaths, Protector};
use crate::change::registry::{self, RunRecord};
use crate::change::{ActionCounts, RunId};
use crate::config::Config;
use crate::paths::registry_path;

/// Was ein Plan vor dem Ausführen zu sagen hat.
#[derive(Debug, Clone)]
pub struct ApplyCheck {
    pub counts: ActionCounts,
    pub actions: usize,
    pub bytes: u64,
    /// Meldung, wenn die OneDrive-Obergrenze überschritten ist (`allow_large` nötig)
    pub limit: Option<String>,
    /// OneDrive-Warnung, Warnung bei Plänen ohne Prüfsumme (Format 1 und 2)
    pub notes: Notes,
    pub empty: bool,
    /// Wie lange die Quarantäne Läufe vorhält (`quarantine_days` der Config), für den Dialog
    pub quarantine_days: u32,
}

/// Ergebnis von [`apply_execute`].
#[derive(Debug)]
pub struct ApplyResult {
    pub outcome: ApplyOutcome,
    /// Der Lauf konnte nicht im Register vermerkt werden (für `undo` dann `--root` angeben)
    pub register_warning: Option<String>,
}

/// Warnung für Pläne ohne Prüfsumme (Format 1 und 2).
pub fn unprotected_note(version: u32) -> Option<String> {
    (version < PLAN_VERSION).then(|| {
        format!(
            "Ungeschützter Plan (Format {version}): Änderungen an der Datei werden nicht erkannt."
        )
    })
}

/// Prüft den Plan, ohne etwas zu verändern.
pub fn apply_check(plan: &Plan) -> Result<ApplyCheck> {
    check_with(plan, &super::load_config()?, &onedrive_roots_from_env())
}

fn check_with(plan: &Plan, config: &Config, onedrive_roots: &[PathBuf]) -> Result<ApplyCheck> {
    let root = PathBuf::from(&plan.root);
    if !root.is_dir() {
        bail!("Wurzel {} des Plans existiert nicht", plan.root);
    }
    let mut notes = Notes::default();
    if let Some(note) = unprotected_note(plan.version) {
        notes.warn(note);
    }
    if let Some(warning) = onedrive_warning(&root, onedrive_roots) {
        notes.warn(warning);
    }
    Ok(ApplyCheck {
        counts: ActionCounts::from_plan(plan),
        actions: plan.actions.len(),
        bytes: plan.total_bytes(),
        limit: limits::exceeds(plan, onedrive_roots, config),
        notes,
        empty: plan.actions.is_empty(),
        quarantine_days: config.quarantine_days,
    })
}

/// Führt den Plan aus. Prüft die harten Bedingungen (Wurzel, Obergrenze) selbst noch einmal,
/// ein vorheriges `apply_check` ist keine Voraussetzung.
pub fn apply_execute(
    plan: &Plan,
    plan_name: &str,
    allow_large: bool,
    ctx: &OpCtx,
) -> Result<ApplyResult> {
    let config = super::load_config()?;
    let check = check_with(plan, &config, &onedrive_roots_from_env())?;
    if let Some(message) = check.limit {
        if !allow_large {
            return Err(Error::LimitExceeded(message).into());
        }
    }
    let root = PathBuf::from(&plan.root);
    let protector = protector_for(plan, &root, &config);

    let run = RunId::generate(chrono::Local::now());
    let now = super::now_rfc3339();
    let register_warning = register_run(&run, &plan.root, &now)
        .err()
        .map(|e| format!("Lauf nicht im Register vermerkt ({e}); für undo --root angeben."));

    ctx.progress.reset("Anwenden", plan.actions.len() as u64);
    let outcome = apply_plan(
        plan,
        &ApplyEnv {
            fs: &RealFs,
            protector: &protector,
            cancel: &ctx.cancel,
            progress: &|r| {
                ctx.progress.set_current(r.path.clone());
                ctx.progress.inc();
            },
            run,
            plan_name,
            now: &now,
        },
    )?;
    Ok(ApplyResult {
        outcome,
        register_warning,
    })
}

/// Schutz aus der Config und den `protected_paths`, die der Plan beim Planen mitbekommen hat
/// (Profil). Der Plan kann den Schutz nur erweitern, nie die Config abschwächen.
fn protector_for(plan: &Plan, root: &Path, config: &Config) -> Protector {
    let mut config = config.clone();
    for path in &plan.protected_paths {
        if !config.protected_paths.contains(path) {
            config.protected_paths.push(path.clone());
        }
    }
    Protector::new(root, &config, &ProtectPaths::from_env())
}

fn register_run(run: &RunId, root: &str, at: &str) -> Result<()> {
    let file: PathBuf = registry_path()?;
    registry::append(
        &file,
        &RunRecord {
            run: run.clone(),
            root: root.to_string(),
            at: at.to_string(),
        },
    )?;
    Ok(())
}

/// Lädt einen Plan aus einer Datei (für `apply` und „Plan öffnen“).
pub fn load_plan(path: &Path) -> Result<Plan> {
    Ok(Plan::load(path)?)
}

/// Ist die Plan-Datei nicht mehr der geladene Plan? Vergleicht die Prüfsumme des Inhalts auf der
/// Platte mit der des geladenen Plans (für die GUI vor dem Anwenden); reine Formatierung zählt
/// nicht, ein veränderter Inhalt mit alter Prüfsumme schon.
pub fn plan_file_changed(path: &Path, loaded: &Plan) -> Result<bool> {
    match Plan::load(path) {
        Ok(on_disk) => Ok(on_disk.integrity() != loaded.integrity()),
        Err(PlanError::Tampered { .. }) => Ok(true),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::plan::{ActionType, PlanKind, PlannedAction};

    fn plan_in(root: &Path, files: usize, size: u64) -> Plan {
        let action = |id: u32| PlannedAction {
            id,
            action: ActionType::Quarantine,
            path: root.join(format!("x{id}.tmp")).display().to_string(),
            size,
            mtime_ticks: 0,
            mtime: String::new(),
            hash: None,
            keep: None,
            keep_hash: None,
            reason: "junk:temp".into(),
            target: None,
            is_dir: false,
            keep_fingerprint: None,
            source_fingerprint: None,
            files: None,
            rule: None,
        };
        Plan {
            version: PLAN_VERSION,
            created: "2026-10-05T10:00:00+02:00".into(),
            kind: PlanKind::Junk,
            root: root.display().to_string(),
            keep_strategy: None,
            params: Default::default(),
            protected_paths: Vec::new(),
            actions: (1..=files as u32).map(action).collect(),
            skipped: vec![],
        }
    }

    #[test]
    fn plan_schutz_ergaenzt_die_config_und_ersetzt_sie_nicht() {
        let dir = tempfile::tempdir().unwrap();
        let in_plan = dir.path().join("im-plan");
        let in_config = dir.path().join("in-config");
        let free = dir.path().join("frei");
        let config = Config {
            protected_paths: vec![in_config.display().to_string()],
            ..Config::default()
        };
        let mut plan = plan_in(dir.path(), 1, 1);

        let without = protector_for(&plan, dir.path(), &config);
        assert!(without.check(&in_plan.join("x.txt")).is_none());
        assert!(without.check(&in_config.join("x.txt")).is_some());

        plan.protected_paths = vec![in_plan.display().to_string()];
        let with = protector_for(&plan, dir.path(), &config);
        assert!(with.check(&in_plan.join("x.txt")).is_some());
        assert!(with.check(&in_config.join("x.txt")).is_some());
        assert!(with.check(&free.join("x.txt")).is_none());
    }

    #[test]
    fn pruefung_zaehlt_aktionen_und_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let check = check_with(&plan_in(dir.path(), 3, 10), &Config::default(), &[]).unwrap();
        assert_eq!((check.actions, check.bytes, check.empty), (3, 30, false));
        assert_eq!(check.counts.quarantined, 3);
        assert!(check.limit.is_none() && check.notes.is_empty());
    }

    #[test]
    fn leerer_plan_wird_als_leer_gemeldet() {
        let dir = tempfile::tempdir().unwrap();
        let check = check_with(&plan_in(dir.path(), 0, 0), &Config::default(), &[]).unwrap();
        assert!(check.empty);
    }

    #[test]
    fn fehlende_wurzel_ist_ein_fehler() {
        let plan = plan_in(Path::new(r"Z:\gibt-es-nicht"), 1, 1);
        let err = check_with(&plan, &Config::default(), &[]).unwrap_err();
        assert!(err.to_string().contains("existiert nicht"), "{err}");
    }

    #[test]
    fn onedrive_warnung_und_obergrenze_kommen_als_daten() {
        let dir = tempfile::tempdir().unwrap();
        let onedrive = [dir.path().to_path_buf()];
        let config = Config::parse("onedrive_max_move_files = 2").unwrap();
        let ok = check_with(&plan_in(dir.path(), 2, 1), &config, &onedrive).unwrap();
        assert_eq!(ok.notes.warnings.len(), 1);
        assert!(ok.limit.is_none());
        let big = check_with(&plan_in(dir.path(), 3, 1), &config, &onedrive).unwrap();
        assert!(big.limit.unwrap().contains("Obergrenze"));
    }

    #[test]
    fn plan_ohne_pruefsumme_bekommt_eine_warnung() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = plan_in(dir.path(), 1, 1);
        assert!(check_with(&plan, &Config::default(), &[])
            .unwrap()
            .notes
            .is_empty());
        for version in [1, 2] {
            plan.version = version;
            let check = check_with(&plan, &Config::default(), &[]).unwrap();
            assert_eq!(
                check.notes.warnings,
                [format!(
                    "Ungeschützter Plan (Format {version}): Änderungen an der Datei werden nicht erkannt."
                )]
            );
        }
    }
}
