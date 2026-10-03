//! Ausführung eines Plans: prüfen, Journal schreiben (Write-ahead), in die Quarantäne verschieben.
//!
//! Pro Aktion gilt: Schutzregeln, Platzhalter/Link, Größe, Zeit, Hash von Quelle und
//! behaltener Datei werden **vor** dem Journal-`intent` geprüft. Stale-Aktionen erzeugen
//! deshalb nur einen `skip`-Eintrag. Dann folgen `intent` (mit `fsync`), Move und `done`.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::fsops::{FileMeta, FsOps};
use super::journal::{Dest, EndStatus, Entry, JournalWriter};
use super::plan::{hex, ActionType, Plan, PlanError, PlannedAction};
use super::protect::Protector;
use super::quarantine;
use super::{RunId, SkipReason};
use crate::paths;
use crate::scan::hasher;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error("Vorprüfung fehlgeschlagen: {0}")]
    Preflight(io::Error),
    #[error("Die Wurzel des Plans ist geschützt ({0}); es wird nichts angelegt oder verschoben")]
    ProtectedRoot(String),
    #[error("Journal nicht schreibbar, Lauf abgebrochen: {0}")]
    Journal(#[from] io::Error),
}

pub struct ApplyEnv<'a> {
    pub fs: &'a dyn FsOps,
    pub protector: &'a Protector,
    /// Wird nach jeder Aktion geprüft (Strg+C): Der Lauf endet sauber nach der laufenden Aktion.
    pub cancel: &'a AtomicBool,
    pub progress: &'a dyn Fn(&ActionResult),
    pub run: RunId,
    /// Name der Plan-Datei, nur für das Journal.
    pub plan_name: &'a str,
    pub now: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionStatus {
    Done,
    AlreadyDone,
    Skipped(SkipReason),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionResult {
    pub id: u32,
    pub path: String,
    pub status: ActionStatus,
}

#[derive(Debug)]
pub struct ApplyOutcome {
    pub run: RunId,
    pub results: Vec<ActionResult>,
    pub moved_bytes: u64,
    pub aborted: bool,
}

impl ApplyOutcome {
    fn count(&self, f: impl Fn(&ActionStatus) -> bool) -> usize {
        self.results.iter().filter(|r| f(&r.status)).count()
    }

    pub fn executed(&self) -> usize {
        self.count(|s| *s == ActionStatus::Done)
    }

    pub fn already_done(&self) -> usize {
        self.count(|s| *s == ActionStatus::AlreadyDone)
    }

    pub fn stale(&self) -> usize {
        self.count(|s| *s == ActionStatus::Skipped(SkipReason::Stale))
    }

    /// Übersprungen aus anderen Gründen als stale oder bereits erledigt.
    pub fn skipped(&self) -> usize {
        self.count(|s| matches!(s, ActionStatus::Skipped(r) if *r != SkipReason::Stale))
    }

    pub fn failed(&self) -> usize {
        self.count(|s| matches!(s, ActionStatus::Failed(_)))
    }

    /// 0 alles erledigt, 2 Teilerfolg (stale/übersprungen/Fehler), 1 Abbruch.
    pub fn exit_code(&self) -> i32 {
        if self.aborted {
            1
        } else if self.stale() + self.skipped() + self.failed() > 0 {
            2
        } else {
            0
        }
    }
}

/// Ergebnis der Vorprüfung einer Aktion.
enum Verdict {
    Skip(SkipReason),
    Fail(String),
}

fn io_message(e: &io::Error) -> String {
    if hasher::is_locked(e) {
        format!("Datei ist von einem anderen Programm geöffnet ({e})")
    } else {
        e.to_string()
    }
}

/// Behaltene Datei prüfen (einmal je Datei und Hash, Ergebnis wird gemerkt).
fn verify_keep(
    size: u64,
    keep_path: &str,
    keep_hash: &str,
    env: &ApplyEnv,
    cache: &mut HashMap<String, Option<SkipReason>>,
) -> Option<SkipReason> {
    let key = format!("{}|{}", paths::path_key(Path::new(keep_path)), keep_hash);
    *cache.entry(key).or_insert_with(|| {
        let keep = Path::new(keep_path);
        let meta = match env.fs.metadata(keep) {
            Ok(m) if !m.is_dir && !m.is_link && !m.is_cloud_only() => m,
            _ => return Some(SkipReason::KeepMissing),
        };
        if meta.size != size {
            return Some(SkipReason::Stale);
        }
        match env.fs.hash(keep, meta.size) {
            Ok(h) if hex(&h) == keep_hash => None,
            Ok(_) => Some(SkipReason::Stale),
            Err(_) => Some(SkipReason::KeepMissing),
        }
    })
}

/// Gemeinsame Prüfungen einer Datei-Quelle: Schutzregeln, Existenz, Platzhalter, Link,
/// Größe und Änderungszeit. Ohne Hash ist das die gesamte Stale-Prüfung (`junk`, `versions`,
/// `archive`).
fn verify_source(a: &PlannedAction, env: &ApplyEnv) -> Result<FileMeta, Verdict> {
    let src = Path::new(&a.path);
    if env.protector.check(src).is_some() {
        return Err(Verdict::Skip(SkipReason::Protected));
    }
    let meta = match env.fs.metadata(src) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(Verdict::Skip(SkipReason::AlreadyDone))
        }
        Err(e) => return Err(Verdict::Fail(io_message(&e))),
    };
    // Reihenfolge zählt: Platzhalter dürfen nie gelesen oder gehasht werden.
    if meta.is_cloud_only() {
        return Err(Verdict::Skip(SkipReason::CloudPlaceholder));
    }
    if meta.is_link {
        return Err(Verdict::Skip(SkipReason::Link));
    }
    if meta.is_dir || meta.size != a.size || meta.mtime_ticks != a.mtime_ticks {
        return Err(Verdict::Skip(SkipReason::Stale));
    }
    Ok(meta)
}

/// Nur `dedupe`: behaltene Datei und Inhalt der Quelle per Hash bestätigen.
fn verify_content(
    a: &PlannedAction,
    meta: &FileMeta,
    env: &ApplyEnv,
    keeps: &mut HashMap<String, Option<SkipReason>>,
) -> Result<(), Verdict> {
    let (Some(hash), Some(keep), Some(keep_hash)) = (&a.hash, &a.keep, &a.keep_hash) else {
        return Ok(());
    };
    if let Some(reason) = verify_keep(a.size, keep, keep_hash, env, keeps) {
        return Err(Verdict::Skip(reason));
    }
    match env.fs.hash(Path::new(&a.path), meta.size) {
        Ok(h) if hex(&h) == *hash => Ok(()),
        Ok(_) => Err(Verdict::Skip(SkipReason::Stale)),
        Err(e) => Err(Verdict::Fail(io_message(&e))),
    }
}

/// Freies Ziel in der Quarantäne; legt den Zielordner an und prüft das Volume.
fn quarantine_target(
    a: &PlannedAction,
    root: &Path,
    env: &ApplyEnv,
) -> Result<std::path::PathBuf, Verdict> {
    let src = Path::new(&a.path);
    let target = quarantine::target_for(root, &env.run, src).map_err(Verdict::Skip)?;
    let parent = target.parent().unwrap_or(root);
    env.fs
        .create_dir_all(parent)
        .map_err(|e| Verdict::Fail(io_message(&e)))?;
    let same_volume = match (env.fs.volume_serial(src), env.fs.volume_serial(parent)) {
        (Ok(a), Ok(b)) => a == b,
        (Err(e), _) | (_, Err(e)) => return Err(Verdict::Fail(io_message(&e))),
    };
    if !same_volume {
        return Err(Verdict::Skip(SkipReason::DifferentVolume));
    }
    quarantine::unique_target(env.fs, target)
        .ok_or_else(|| Verdict::Fail("kein freier Name in der Quarantäne".into()))
}

/// Alle Prüfungen vor dem Journal. `Ok` liefert das freie Ziel in der Quarantäne.
fn verify_quarantine(
    a: &PlannedAction,
    root: &Path,
    env: &ApplyEnv,
    keeps: &mut HashMap<String, Option<SkipReason>>,
) -> Result<std::path::PathBuf, Verdict> {
    let meta = verify_source(a, env)?;
    verify_content(a, &meta, env, keeps)?;
    quarantine_target(a, root, env)
}

/// Ergebnis der Vorprüfung in einen Journal-Eintrag und einen Status übersetzen.
fn record_verdict(
    verdict: Verdict,
    run: RunId,
    action: u32,
    journal: &mut JournalWriter,
) -> Result<ActionStatus, ApplyError> {
    match verdict {
        Verdict::Skip(reason) => {
            journal.append(&Entry::Skip {
                run,
                action,
                reason,
            })?;
            Ok(if reason == SkipReason::AlreadyDone {
                ActionStatus::AlreadyDone
            } else {
                ActionStatus::Skipped(reason)
            })
        }
        Verdict::Fail(error) => {
            journal.append(&Entry::Fail {
                run,
                action,
                error: error.clone(),
            })?;
            Ok(ActionStatus::Failed(error))
        }
    }
}

/// Führt den Rename aus und schließt die Aktion im Journal ab.
fn finish_rename(
    env: &ApplyEnv,
    journal: &mut JournalWriter,
    a: &PlannedAction,
    target: &Path,
) -> Result<ActionStatus, ApplyError> {
    let run = env.run.clone();
    match env.fs.rename(Path::new(&a.path), target) {
        Ok(()) => {
            journal.append(&Entry::Done { run, action: a.id })?;
            Ok(ActionStatus::Done)
        }
        Err(e) => {
            let error = io_message(&e);
            journal.append(&Entry::Fail {
                run,
                action: a.id,
                error: error.clone(),
            })?;
            Ok(ActionStatus::Failed(error))
        }
    }
}

fn process_quarantine(
    a: &PlannedAction,
    root: &Path,
    env: &ApplyEnv,
    journal: &mut JournalWriter,
    keeps: &mut HashMap<String, Option<SkipReason>>,
) -> Result<ActionStatus, ApplyError> {
    let target = match verify_quarantine(a, root, env, keeps) {
        Ok(target) => target,
        Err(verdict) => return record_verdict(verdict, env.run.clone(), a.id, journal),
    };
    // Write-ahead: Ohne gesicherten `intent` wird nichts verschoben.
    journal.append(&Entry::Intent {
        run: env.run.clone(),
        action: a.id,
        from: a.path.clone(),
        to: paths::display(&target),
        size: a.size,
        hash: a.hash.clone(),
        dest: Dest::Quarantine,
        is_dir: false,
    })?;
    finish_rename(env, journal, a, &target)
}

fn process(
    a: &PlannedAction,
    root: &Path,
    env: &ApplyEnv,
    journal: &mut JournalWriter,
    keeps: &mut HashMap<String, Option<SkipReason>>,
) -> Result<ActionStatus, ApplyError> {
    match a.action {
        ActionType::Quarantine => process_quarantine(a, root, env, journal, keeps),
        ActionType::RemoveDir | ActionType::Move => record_verdict(
            Verdict::Fail("Aktionstyp wird von dieser Version noch nicht ausgeführt".into()),
            env.run.clone(),
            a.id,
            journal,
        ),
    }
}

pub fn apply_plan(plan: &Plan, env: &ApplyEnv) -> Result<ApplyOutcome, ApplyError> {
    plan.validate()?;
    let root = Path::new(&plan.root);
    // Journal und Quarantäne entstehen in der Wurzel: Liegt sie in einem Systempfad (z. B. ein
    // manipulierter Plan mit `C:\Windows`), darf dort nichts angelegt werden.
    if let Some(reason) = env.protector.check(root) {
        return Err(ApplyError::ProtectedRoot(reason.to_string()));
    }
    quarantine::ensure_plain_dirs(env.fs, root, &env.run).map_err(ApplyError::Preflight)?;

    let mut journal = JournalWriter::create(&quarantine::journal_path(root, &env.run))?;
    journal.append(&Entry::RunStart {
        run: env.run.clone(),
        plan: env.plan_name.to_string(),
        root: plan.root.clone(),
        started: env.now.to_string(),
    })?;

    let mut keeps = HashMap::new();
    let mut outcome = ApplyOutcome {
        run: env.run.clone(),
        results: Vec::new(),
        moved_bytes: 0,
        aborted: false,
    };
    for action in &plan.actions {
        if env.cancel.load(Ordering::Relaxed) {
            outcome.aborted = true;
            break;
        }
        let status = process(action, root, env, &mut journal, &mut keeps)?;
        if status == ActionStatus::Done {
            outcome.moved_bytes += action.size;
        }
        let result = ActionResult {
            id: action.id,
            path: action.path.clone(),
            status,
        };
        (env.progress)(&result);
        outcome.results.push(result);
    }

    let complete = !outcome.aborted
        && outcome
            .results
            .iter()
            .all(|r| matches!(r.status, ActionStatus::Done | ActionStatus::AlreadyDone));
    journal.append(&Entry::RunEnd {
        run: env.run.clone(),
        status: if complete {
            EndStatus::Complete
        } else {
            EndStatus::Partial
        },
    })?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::fsops::testing::{FaultyFs, Op};
    use crate::change::fsops::RealFs;
    use crate::change::protect::ProtectPaths;
    use crate::change::test_support::{fx, run_with, Fx, RUN};
    use crate::config::Config;

    fn apply(fx: &Fx, plan: &Plan) -> ApplyOutcome {
        run_with(fx, plan, &RealFs, &fx.protector(), RUN).unwrap()
    }

    fn intents(entries: &[Entry]) -> usize {
        entries
            .iter()
            .filter(|e| matches!(e, Entry::Intent { .. }))
            .count()
    }

    #[test]
    fn happy_path_verschiebt_in_die_quarantaene_und_journalisiert() {
        let fx = fx();
        fx.write("a/orig.txt", "gleicher inhalt");
        fx.write("b/kopie.txt", "gleicher inhalt");
        fx.write("c/sub/kopie2.txt", "gleicher inhalt");
        let mtime = RealFs
            .metadata(&fx.root.join("b/kopie.txt"))
            .unwrap()
            .mtime_ticks;
        let plan = fx.plan(&[
            ("b/kopie.txt", "a/orig.txt"),
            ("c/sub/kopie2.txt", "a/orig.txt"),
        ]);

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.exit_code()), (2, 0));
        assert_eq!(out.moved_bytes, 30);
        assert!(!fx.exists("b/kopie.txt") && !fx.exists("c/sub/kopie2.txt"));
        assert_eq!(fx.read("a/orig.txt"), "gleicher inhalt");
        let moved = fx.quarantined(RUN, r"b\kopie.txt");
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "gleicher inhalt");
        assert_eq!(RealFs.metadata(&moved).unwrap().mtime_ticks, mtime);
        assert!(fx.quarantined(RUN, r"c\sub\kopie2.txt").exists());

        let entries = fx.journal(RUN);
        assert!(matches!(entries.first(), Some(Entry::RunStart { .. })));
        assert!(matches!(
            entries.last(),
            Some(Entry::RunEnd {
                status: EndStatus::Complete,
                ..
            })
        ));
        assert_eq!(intents(&entries), 2);
        let order: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e, Entry::Intent { .. } | Entry::Done { .. }))
            .map(|e| matches!(e, Entry::Intent { .. }))
            .collect();
        assert_eq!(order, [true, false, true, false], "intent immer vor done");
    }

    #[test]
    fn zweiter_apply_aendert_nichts_und_meldet_bereits_erledigt() {
        let fx = fx();
        fx.write("a.txt", "x");
        fx.write("b.txt", "x");
        let plan = fx.plan(&[("b.txt", "a.txt")]);
        assert_eq!(apply(&fx, &plan).executed(), 1);

        let second =
            run_with(&fx, &plan, &RealFs, &fx.protector(), "20261003-130000-cd34").unwrap();
        assert_eq!((second.executed(), second.already_done()), (0, 1));
        assert_eq!(second.exit_code(), 0);
        assert!(!fx.quarantined("20261003-130000-cd34", "b.txt").exists());
        assert!(fx.quarantined(RUN, "b.txt").exists());
        assert_eq!(intents(&fx.journal("20261003-130000-cd34")), 0);
    }

    #[test]
    fn junk_ohne_hash_wird_ohne_inhalt_zu_lesen_in_die_quarantaene_verschoben() {
        let fx = fx();
        fx.write("a/cache.tmp", "temporaer");
        fx.write("b/~$bericht.docx", "sperrdatei");
        let plan = fx.junk_plan(&["a/cache.tmp", "b/~$bericht.docx"]);
        let faulty = FaultyFs::new();

        let out = run_with(&fx, &plan, &faulty, &fx.protector(), RUN).unwrap();

        assert_eq!((out.executed(), out.exit_code()), (2, 0));
        assert!(!fx.exists("a/cache.tmp") && !fx.exists("b/~$bericht.docx"));
        assert!(fx.quarantined(RUN, r"a\cache.tmp").exists());
        assert!(faulty.hashed().is_empty(), "Junk wird nie gehasht");
        let entries = fx.journal(RUN);
        let intents: Vec<_> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Intent {
                    hash, dest, is_dir, ..
                } => Some((hash.clone(), *dest, *is_dir)),
                _ => None,
            })
            .collect();
        assert_eq!(
            intents,
            [
                (None, Dest::Quarantine, false),
                (None, Dest::Quarantine, false)
            ]
        );
    }

    #[test]
    fn junk_geaenderte_groesse_ist_stale() {
        let fx = fx();
        fx.write("a.tmp", "kurz");
        let plan = fx.junk_plan(&["a.tmp"]);
        fx.write("a.tmp", "jetzt deutlich länger");

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.stale(), out.exit_code()), (0, 1, 2));
        assert!(fx.exists("a.tmp"));
    }

    #[test]
    fn junk_geaenderte_mtime_bei_gleicher_groesse_ist_stale_weil_kein_hash_pruefung() {
        let fx = fx();
        let file = fx.write("a.tmp", "gleich");
        let plan = fx.junk_plan(&["a.tmp"]);
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(later)
            .unwrap();

        let out = apply(&fx, &plan);

        assert_eq!(out.stale(), 1);
        assert!(fx.exists("a.tmp") && !fx.quarantined(RUN, "a.tmp").exists());
        assert_eq!(intents(&fx.journal(RUN)), 0, "stale erzeugt kein intent");
    }

    #[test]
    fn junk_cloud_platzhalter_wird_uebersprungen_und_nie_gelesen() {
        let fx = fx();
        let wolke = fx.write("wolke.tmp", "x");
        let plan = fx.junk_plan(&["wolke.tmp"]);
        let faulty = FaultyFs::new().cloud_only(&wolke);

        let out = run_with(&fx, &plan, &faulty, &fx.protector(), RUN).unwrap();

        assert_eq!(out.skipped(), 1);
        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::CloudPlaceholder)
        );
        assert!(faulty.hashed().is_empty());
        assert!(fx.exists("wolke.tmp"));
    }

    #[test]
    fn junk_fehlende_datei_ist_bereits_erledigt() {
        let fx = fx();
        fx.write("a.tmp", "x");
        let plan = fx.junk_plan(&["a.tmp"]);
        std::fs::remove_file(fx.root.join("a.tmp")).unwrap();

        let out = apply(&fx, &plan);

        assert_eq!(
            (out.executed(), out.already_done(), out.exit_code()),
            (0, 1, 0)
        );
    }

    #[test]
    fn geaenderte_datei_ist_stale_der_rest_laeuft() {
        let fx = fx();
        fx.write("a.txt", "inhalt");
        fx.write("b.txt", "inhalt");
        fx.write("c.txt", "inhalt");
        let plan = fx.plan(&[("b.txt", "a.txt"), ("c.txt", "a.txt")]);
        fx.write("b.txt", "inhalt, aber länger");

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.stale(), out.exit_code()), (1, 1, 2));
        assert!(fx.exists("b.txt") && !fx.exists("c.txt"));
        assert!(fx.journal(RUN).iter().any(|e| matches!(
            e,
            Entry::Skip {
                action: 1,
                reason: SkipReason::Stale,
                ..
            }
        )));
        assert_eq!(intents(&fx.journal(RUN)), 1, "stale erzeugt kein intent");
    }

    #[test]
    fn gleiche_groesse_und_zeit_aber_anderer_inhalt_ist_stale() {
        let fx = fx();
        fx.write("a.txt", "aaaa");
        let b = fx.write("b.txt", "aaaa");
        let plan = fx.plan(&[("b.txt", "a.txt")]);
        let mtime = std::fs::metadata(&b).unwrap().modified().unwrap();
        std::fs::write(&b, "bbbb").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&b)
            .unwrap()
            .set_modified(mtime)
            .unwrap();

        let out = apply(&fx, &plan);

        assert_eq!(out.stale(), 1);
        assert_eq!(fx.read("b.txt"), "bbbb");
    }

    #[test]
    fn fehlende_behaltene_datei_ueberspringt_die_aktion() {
        let fx = fx();
        fx.write("a.txt", "x");
        fx.write("b.txt", "x");
        fx.write("c.txt", "x");
        let plan = fx.plan(&[("b.txt", "a.txt"), ("c.txt", "a.txt")]);
        std::fs::remove_file(fx.root.join("a.txt")).unwrap();

        let out = apply(&fx, &plan);

        assert_eq!(out.executed(), 0);
        assert!(out
            .results
            .iter()
            .all(|r| r.status == ActionStatus::Skipped(SkipReason::KeepMissing)));
        assert!(fx.exists("b.txt") && fx.exists("c.txt"));
    }

    #[test]
    fn veraenderte_behaltene_datei_ist_stale() {
        let fx = fx();
        fx.write("a.txt", "xxxx");
        fx.write("b.txt", "xxxx");
        let plan = fx.plan(&[("b.txt", "a.txt")]);
        fx.write("a.txt", "yyyy");

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.stale()), (0, 1));
        assert!(fx.exists("b.txt"));
    }

    #[test]
    fn geschuetzter_pfad_im_manipulierten_plan_wird_verweigert() {
        let fx = fx();
        fx.write("a.txt", "x");
        fx.write("system/b.txt", "x");
        let plan = fx.plan(&[("system/b.txt", "a.txt")]);
        let protect = ProtectPaths {
            system: vec![fx.root.join("system")],
            appdata: vec![],
        };
        let protector = Protector::new(&fx.root, &Config::default(), &protect);

        let out = run_with(&fx, &plan, &RealFs, &protector, RUN).unwrap();

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::Protected)
        );
        assert_eq!(out.exit_code(), 2);
        assert!(fx.exists("system/b.txt"));
    }

    #[test]
    fn cloud_platzhalter_wird_nie_gelesen_oder_verschoben() {
        let fx = fx();
        fx.write("a.txt", "x");
        let cloud = fx.write("wolke.txt", "x");
        let plan = fx.plan(&[("wolke.txt", "a.txt")]);
        let fs = FaultyFs::new().cloud_only(&cloud);

        let out = run_with(&fx, &plan, &fs, &fx.protector(), RUN).unwrap();

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::CloudPlaceholder)
        );
        assert!(fx.exists("wolke.txt"));
        assert!(!fs.hashed().contains(&paths::path_key(&cloud)));
    }

    #[test]
    fn link_als_quelle_wird_nie_verschoben() {
        let fx = fx();
        fx.write("a.txt", "x");
        let target = fx.root.join("zielordner");
        std::fs::create_dir_all(&target).unwrap();
        let link = fx.root.join("junction");
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("Junction nicht erzeugbar, Test übersprungen");
            return;
        }
        let mut plan = fx.plan(&[("a.txt", "zielordner/../a.txt")]);
        plan.actions[0].path = paths::display(&link);
        plan.actions[0].keep = Some(paths::display(&fx.root.join("a.txt")));
        plan.actions[0].size = 0;

        let out = apply(&fx, &plan);

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::Link)
        );
        assert!(link.exists());
    }

    #[test]
    fn gesperrte_datei_ist_fehler_und_der_lauf_geht_weiter() {
        let fx = fx();
        fx.write("a.txt", "x");
        let locked = fx.write("b.txt", "x");
        fx.write("c.txt", "x");
        let plan = fx.plan(&[("b.txt", "a.txt"), ("c.txt", "a.txt")]);
        let fs = FaultyFs::new().fail(Op::Hash, &locked);

        let out = run_with(&fx, &plan, &fs, &fx.protector(), RUN).unwrap();

        assert_eq!((out.executed(), out.failed(), out.exit_code()), (1, 1, 2));
        assert!(fx.exists("b.txt"));
        assert!(fx
            .journal(RUN)
            .iter()
            .any(|e| matches!(e, Entry::Fail { action: 1, .. })));
    }

    #[test]
    fn fehlgeschlagener_move_steht_als_intent_und_fail_im_journal() {
        let fx = fx();
        fx.write("a.txt", "x");
        let busy = fx.write("b.txt", "x");
        let plan = fx.plan(&[("b.txt", "a.txt")]);
        let fs = FaultyFs::new().fail(Op::Rename, &busy);

        let out = run_with(&fx, &plan, &fs, &fx.protector(), RUN).unwrap();

        assert_eq!((out.executed(), out.failed()), (0, 1));
        assert!(fx.exists("b.txt"));
        let entries = fx.journal(RUN);
        assert_eq!(intents(&entries), 1);
        assert!(entries
            .iter()
            .any(|e| matches!(e, Entry::Fail { action: 1, .. })));
        assert!(matches!(
            entries.last(),
            Some(Entry::RunEnd {
                status: EndStatus::Partial,
                ..
            })
        ));
    }

    #[test]
    fn belegtes_quarantaene_ziel_bekommt_eindeutigen_namen() {
        let fx = fx();
        fx.write("a.txt", "x");
        fx.write("b.txt", "x");
        let plan = fx.plan(&[("b.txt", "a.txt")]);
        let taken = fx.quarantined(RUN, "b.txt");
        std::fs::create_dir_all(taken.parent().unwrap()).unwrap();
        std::fs::write(&taken, "fremd").unwrap();

        let out = apply(&fx, &plan);

        assert_eq!(out.executed(), 1);
        assert_eq!(std::fs::read_to_string(&taken).unwrap(), "fremd");
        let second = fx.quarantined(RUN, "b (2).txt");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "x");
        assert!(fx.journal(RUN).iter().any(|e| matches!(
            e,
            Entry::Intent { to, .. } if to.ends_with("b (2).txt")
        )));
    }

    #[test]
    fn manipulierte_plaene_werden_vor_dem_ersten_move_abgelehnt() {
        let fx = fx();
        fx.write("a.txt", "x");
        fx.write("b.txt", "x");
        let mut plan = fx.plan(&[("b.txt", "a.txt")]);
        plan.actions[0].path = format!(r"{}\..\fremd.txt", paths::display(&fx.root));
        let err = run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap_err();
        assert!(matches!(err, ApplyError::Plan(_)));
        assert!(!quarantine::journal_path(&fx.root, &RunId::parse(RUN).unwrap()).exists());
        assert!(fx.exists("b.txt"));
    }

    #[test]
    fn abbruch_per_flag_beendet_vor_der_naechsten_aktion() {
        let fx = fx();
        fx.write("a.txt", "x");
        fx.write("b.txt", "x");
        let plan = fx.plan(&[("b.txt", "a.txt")]);
        let cancel = AtomicBool::new(true);
        let out = apply_plan(
            &plan,
            &ApplyEnv {
                fs: &RealFs,
                protector: &fx.protector(),
                cancel: &cancel,
                progress: &|_| {},
                run: RunId::parse(RUN).unwrap(),
                plan_name: "plan.json",
                now: "t",
            },
        )
        .unwrap();
        assert!(out.aborted && out.results.is_empty());
        assert_eq!(out.exit_code(), 1);
        assert!(fx.exists("b.txt"));
        assert!(matches!(
            fx.journal(RUN).last(),
            Some(Entry::RunEnd {
                status: EndStatus::Partial,
                ..
            })
        ));
    }

    #[test]
    fn absturz_vor_dem_move_hinterlaesst_intent_ohne_done() {
        let fx = fx();
        fx.write("a.txt", "x");
        fx.write("b.txt", "x");
        fx.write("c.txt", "x");
        let plan = fx.plan(&[("b.txt", "a.txt"), ("c.txt", "a.txt")]);
        let fs = FaultyFs::new().crash_before_rename(2);

        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_with(&fx, &plan, &fs, &fx.protector(), RUN)
        }));

        assert!(crashed.is_err());
        let entries = fx.journal(RUN);
        assert_eq!(intents(&entries), 2);
        assert_eq!(
            entries
                .iter()
                .filter(|e| matches!(e, Entry::Done { .. }))
                .count(),
            1
        );
        assert!(!entries.iter().any(|e| matches!(e, Entry::RunEnd { .. })));
        assert!(fx.exists("c.txt"), "zweiter Move hat nie stattgefunden");
        assert!(!fx.exists("b.txt"));
    }

    #[test]
    fn absturz_nach_dem_move_hinterlaesst_intent_ohne_done_aber_datei_ist_verschoben() {
        let fx = fx();
        fx.write("a.txt", "x");
        fx.write("b.txt", "x");
        let plan = fx.plan(&[("b.txt", "a.txt")]);
        let fs = FaultyFs::new().crash_after_rename(1);

        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_with(&fx, &plan, &fs, &fx.protector(), RUN)
        }));

        assert!(crashed.is_err());
        let entries = fx.journal(RUN);
        assert_eq!(intents(&entries), 1);
        assert!(!entries.iter().any(|e| matches!(e, Entry::Done { .. })));
        assert!(!fx.exists("b.txt") && fx.quarantined(RUN, "b.txt").exists());
    }

    #[test]
    fn geschuetzte_wurzel_wird_vor_dem_ersten_schreiben_abgelehnt() {
        let fx = fx();
        fx.write("a.txt", "x");
        fx.write("b.txt", "x");
        let plan = fx.plan(&[("b.txt", "a.txt")]);
        let protect = ProtectPaths {
            system: vec![fx.root.clone()],
            appdata: vec![],
        };
        let protector = Protector::new(&fx.root, &Config::default(), &protect);

        let err = run_with(&fx, &plan, &RealFs, &protector, RUN).unwrap_err();

        assert!(matches!(err, ApplyError::ProtectedRoot(_)));
        assert!(!quarantine::tool_dir(&fx.root).exists(), "nichts angelegt");
        assert!(fx.exists("b.txt"));
    }

    #[test]
    fn exit_code_je_ergebnis() {
        let out = |statuses: Vec<ActionStatus>, aborted| ApplyOutcome {
            run: RunId::parse(RUN).unwrap(),
            results: statuses
                .into_iter()
                .map(|status| ActionResult {
                    id: 1,
                    path: String::new(),
                    status,
                })
                .collect(),
            moved_bytes: 0,
            aborted,
        };
        assert_eq!(out(vec![], false).exit_code(), 0);
        assert_eq!(
            out(vec![ActionStatus::Done, ActionStatus::AlreadyDone], false).exit_code(),
            0
        );
        assert_eq!(
            out(vec![ActionStatus::Failed("x".into())], false).exit_code(),
            2
        );
        assert_eq!(out(vec![ActionStatus::Done], true).exit_code(), 1);
    }
}
