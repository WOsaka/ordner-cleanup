//! Gemeinsame Test-Hilfen für Apply- und Undo-Tests (echte Temp-Ordner).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use super::apply::{apply_plan, ApplyEnv, ApplyError, ApplyOutcome};
use super::fsops::{FsOps, RealFs};
use super::journal::{self, Entry};
use super::plan::{hex, ActionType, Plan, PlanKind, PlannedAction, PLAN_VERSION};
use super::protect::{ProtectPaths, Protector};
use super::{quarantine, RunId};
use crate::config::Config;
use crate::paths;
use crate::scan::source::TICKS_PER_SEC;

pub const RUN: &str = "20261003-120000-ab12";

pub struct Fx {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
}

pub fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("wurzel");
    std::fs::create_dir_all(&root).unwrap();
    Fx { _dir: dir, root }
}

impl Fx {
    pub fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).unwrap()
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }

    /// Plan aus `(Duplikat, behaltene Datei)`-Paaren, Werte wie der Planer sie schreibt.
    pub fn plan(&self, pairs: &[(&str, &str)]) -> Plan {
        let actions = pairs
            .iter()
            .zip(1u32..)
            .map(|((dup, keep), id)| {
                let (dup, keep) = (self.root.join(dup), self.root.join(keep));
                let meta = RealFs.metadata(&dup).unwrap();
                let hash = hex(&RealFs.hash(&dup, meta.size).unwrap());
                PlannedAction {
                    id,
                    action: ActionType::Quarantine,
                    path: paths::display(&dup),
                    size: meta.size,
                    mtime_ticks: meta.mtime_ticks,
                    mtime: String::new(),
                    hash: Some(hash.clone()),
                    keep: Some(paths::display(&keep)),
                    keep_hash: Some(hash),
                    reason: "exact-duplicate".into(),
                    target: None,
                    is_dir: false,
                    files: None,
                }
            })
            .collect();
        Plan {
            version: PLAN_VERSION,
            created: "t".into(),
            kind: PlanKind::Dedupe,
            root: paths::display(&self.root),
            keep_strategy: Some("oldest".into()),
            params: Default::default(),
            actions,
            skipped: vec![],
        }
    }

    /// Quarantäne-Plan ohne Hash (wie `plan junk` ihn schreibt): Größe und mtime aus dem
    /// aktuellen Dateizustand.
    pub fn junk_plan(&self, rels: &[&str]) -> Plan {
        let actions = rels
            .iter()
            .zip(1u32..)
            .map(|(rel, id)| {
                let path = self.root.join(rel);
                let meta = RealFs.metadata(&path).unwrap();
                PlannedAction {
                    id,
                    action: ActionType::Quarantine,
                    path: paths::display(&path),
                    size: meta.size,
                    mtime_ticks: meta.mtime_ticks,
                    mtime: String::new(),
                    hash: None,
                    keep: None,
                    keep_hash: None,
                    reason: "junk:temp".into(),
                    target: None,
                    is_dir: false,
                    files: None,
                }
            })
            .collect();
        Plan {
            version: PLAN_VERSION,
            created: "t".into(),
            kind: PlanKind::Junk,
            root: paths::display(&self.root),
            keep_strategy: None,
            params: Default::default(),
            actions,
            skipped: vec![],
        }
    }

    pub fn mkdir(&self, rel: &str) -> PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// `remove-dir`-Plan in der angegebenen Reihenfolge (wie `plan empty-dirs`: tiefste zuerst).
    pub fn dir_plan(&self, rels: &[&str]) -> Plan {
        let actions = rels
            .iter()
            .zip(1u32..)
            .map(|(rel, id)| {
                let path = self.root.join(rel);
                let meta = RealFs.metadata(&path).unwrap();
                PlannedAction {
                    id,
                    action: ActionType::RemoveDir,
                    path: paths::display(&path),
                    size: 0,
                    mtime_ticks: meta.mtime_ticks,
                    mtime: String::new(),
                    hash: None,
                    keep: None,
                    keep_hash: None,
                    reason: "empty-dir".into(),
                    target: None,
                    is_dir: true,
                    files: None,
                }
            })
            .collect();
        Plan {
            version: PLAN_VERSION,
            created: "t".into(),
            kind: PlanKind::EmptyDirs,
            root: paths::display(&self.root),
            keep_strategy: None,
            params: Default::default(),
            actions,
            skipped: vec![],
        }
    }

    /// Archiv-Plan für ganze Ordner (wie `plan archive`): Dateianzahl, Summe und jüngste mtime
    /// kommen aus dem aktuellen Dateisystem, das Ziel ist `_Archiv\<Jahr>\<rel>`.
    pub fn archive_plan(&self, rels: &[&str]) -> Plan {
        let actions = rels
            .iter()
            .zip(1u32..)
            .map(|(rel, id)| {
                let dir = self.root.join(rel);
                let (files, bytes, newest) = walk_stats(&dir);
                let year = chrono::DateTime::from_timestamp(newest / TICKS_PER_SEC, 0)
                    .map_or(1970, |t| chrono::Datelike::year(&t));
                let target = self.root.join("_Archiv").join(year.to_string()).join(rel);
                PlannedAction {
                    id,
                    action: ActionType::Move,
                    path: paths::display(&dir),
                    size: bytes,
                    mtime_ticks: newest,
                    mtime: String::new(),
                    hash: None,
                    keep: None,
                    keep_hash: None,
                    reason: "archive:older-than-2y".into(),
                    target: Some(paths::display(&target)),
                    is_dir: true,
                    files: Some(files),
                }
            })
            .collect();
        Plan {
            version: PLAN_VERSION,
            created: "t".into(),
            kind: PlanKind::Archive,
            root: paths::display(&self.root),
            keep_strategy: None,
            params: Default::default(),
            actions,
            skipped: vec![],
        }
    }

    /// `move`-Plan für einzelne Dateien nach `_Archiv\<ziel>` (wie `plan versions`).
    pub fn move_file_plan(&self, pairs: &[(&str, &str)]) -> Plan {
        let actions = pairs
            .iter()
            .zip(1u32..)
            .map(|((rel, target), id)| {
                let path = self.root.join(rel);
                let meta = RealFs.metadata(&path).unwrap();
                PlannedAction {
                    id,
                    action: ActionType::Move,
                    path: paths::display(&path),
                    size: meta.size,
                    mtime_ticks: meta.mtime_ticks,
                    mtime: String::new(),
                    hash: None,
                    keep: None,
                    keep_hash: None,
                    reason: "older-version".into(),
                    target: Some(paths::display(&self.root.join("_Archiv").join(target))),
                    is_dir: false,
                    files: None,
                }
            })
            .collect();
        Plan {
            version: PLAN_VERSION,
            created: "t".into(),
            kind: PlanKind::Versions,
            root: paths::display(&self.root),
            keep_strategy: None,
            params: Default::default(),
            actions,
            skipped: vec![],
        }
    }

    pub fn protector(&self) -> Protector {
        Protector::new(&self.root, &Config::default(), &ProtectPaths::default())
    }

    pub fn quarantined(&self, run: &str, rel: &str) -> PathBuf {
        quarantine::run_dir(&self.root, &RunId::parse(run).unwrap()).join(rel)
    }

    pub fn journal(&self, run: &str) -> Vec<Entry> {
        journal::read(&quarantine::journal_path(
            &self.root,
            &RunId::parse(run).unwrap(),
        ))
        .unwrap()
    }
}

pub fn run_with(
    _fx: &Fx,
    plan: &Plan,
    fs: &dyn FsOps,
    protector: &Protector,
    run: &str,
) -> Result<ApplyOutcome, ApplyError> {
    let cancel = AtomicBool::new(false);
    apply_plan(
        plan,
        &ApplyEnv {
            fs,
            protector,
            cancel: &cancel,
            progress: &|_| {},
            run: RunId::parse(run).unwrap(),
            plan_name: "plan.json",
            now: "2026-10-03T10:00:00Z",
        },
    )
}

/// Dateianzahl, Summe der Größen und jüngste mtime (Ticks) aller Dateien unterhalb von `dir`.
fn walk_stats(dir: &std::path::Path) -> (u64, u64, i64) {
    let (mut files, mut bytes, mut newest) = (0u64, 0u64, 0i64);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).unwrap() {
            let path = entry.unwrap().path();
            let meta = RealFs.metadata(&path).unwrap();
            if meta.is_dir {
                stack.push(path);
            } else {
                files += 1;
                bytes += meta.size;
                newest = newest.max(meta.mtime_ticks);
            }
        }
    }
    (files, bytes, newest)
}
