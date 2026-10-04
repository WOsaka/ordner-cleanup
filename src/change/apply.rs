//! Ausführung eines Plans: prüfen, Journal schreiben (Write-ahead), in die Quarantäne verschieben.
//!
//! Pro Aktion gilt: Schutzregeln, Platzhalter/Link, Größe, Zeit, Hash von Quelle und
//! behaltener Datei werden **vor** dem Journal-`intent` geprüft. Stale-Aktionen erzeugen
//! deshalb nur einen `skip`-Eintrag. Dann folgen `intent` (mit `fsync`), Move und `done`.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::archive::is_in_archive;
use super::fsops::{FileMeta, FsOps};
use super::journal::{Dest, EndStatus, Entry, JournalWriter};
use super::plan::{hex, ActionType, Plan, PlanError, PlanKind, PlannedAction, ARCHIVE_DIR};
use super::protect::Protector;
use super::quarantine::{self, MAX_TARGET_LEN};
use super::{ActionCounts, RunId, SkipReason};
use crate::analysis::problems::MAX_PATH_CHARS;
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
    pub kind: ActionType,
    /// `move` einer Regel (Einsortieren oder Umbenennen).
    pub sorted: bool,
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

    /// Ausgeführte Aktionen nach Typ.
    pub fn counts(&self) -> ActionCounts {
        let mut counts = ActionCounts::default();
        for r in self
            .results
            .iter()
            .filter(|r| r.status == ActionStatus::Done)
        {
            if r.sorted {
                counts.count_sorted();
            } else {
                counts.count(r.kind);
            }
        }
        counts
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

/// Vorprüfung von `remove-dir`: Schutz, Existenz, Link/Platzhalter, wirklich ein leerer Ordner.
/// Der Ordner-mtime wird bewusst nicht verglichen: Das Entfernen von Kindern ändert ihn selbst.
fn verify_remove_dir(a: &PlannedAction, env: &ApplyEnv) -> Result<FileMeta, Verdict> {
    let path = Path::new(&a.path);
    if env.protector.check(path).is_some() {
        return Err(Verdict::Skip(SkipReason::Protected));
    }
    let meta = match env.fs.metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(Verdict::Skip(SkipReason::AlreadyDone))
        }
        Err(e) => return Err(Verdict::Fail(io_message(&e))),
    };
    if meta.is_cloud_only() {
        return Err(Verdict::Skip(SkipReason::CloudPlaceholder));
    }
    if meta.is_link || meta.is_reparse_point() {
        return Err(Verdict::Skip(SkipReason::Link));
    }
    if !meta.is_dir {
        return Err(Verdict::Skip(SkipReason::Stale));
    }
    match env.fs.read_dir(path) {
        Ok(entries) if entries.is_empty() => Ok(meta),
        Ok(_) => Err(Verdict::Skip(SkipReason::Stale)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Err(Verdict::Skip(SkipReason::AlreadyDone))
        }
        Err(e) => Err(Verdict::Fail(io_message(&e))),
    }
}

/// Zustand der `remove-dir`-Ordner vor Beginn des Laufs, nach Aktions-ID.
type DirSnapshot = HashMap<u32, FileMeta>;

fn snapshot_dirs(plan: &Plan, env: &ApplyEnv) -> DirSnapshot {
    plan.actions
        .iter()
        .filter(|a| a.action == ActionType::RemoveDir)
        .filter_map(|a| Some((a.id, env.fs.metadata(Path::new(&a.path)).ok()?)))
        .collect()
}

fn process_remove_dir(
    a: &PlannedAction,
    env: &ApplyEnv,
    journal: &mut JournalWriter,
    before: &DirSnapshot,
) -> Result<ActionStatus, ApplyError> {
    let run = env.run.clone();
    let meta = match verify_remove_dir(a, env) {
        Ok(meta) => meta,
        Err(verdict) => return record_verdict(verdict, run, a.id, journal),
    };
    // Write-ahead. Attribute und Zeiten stammen vom Zustand beim Apply, aber vor der ersten
    // Änderung des Laufs: Das Entfernen leerer Unterordner ändert die mtime ihrer Eltern, und
    // Undo soll den ursprünglichen Wert wiederherstellen.
    let saved = before.get(&a.id).unwrap_or(&meta);
    journal.append(&Entry::IntentRemoveDir {
        run: run.clone(),
        action: a.id,
        path: a.path.clone(),
        attrs: saved.attrs.0,
        mtime_ticks: saved.mtime_ticks,
        ctime_ticks: saved.ctime_ticks,
    })?;
    match env.fs.remove_dir(Path::new(&a.path)) {
        Ok(()) => {
            journal.append(&Entry::Done { run, action: a.id })?;
            Ok(ActionStatus::Done)
        }
        // Zwischen Vorprüfung und Entfernen wurde etwas hineingelegt: nichts ist passiert.
        Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => {
            journal.append(&Entry::Skip {
                run,
                action: a.id,
                reason: SkipReason::Stale,
            })?;
            Ok(ActionStatus::Skipped(SkipReason::Stale))
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

/// Dateianzahl, Summe der Größen und jüngste mtime eines Ordners per Metadaten-Walk (es wird
/// nichts geöffnet, kein Cloud-Recall). Links, Platzhalter und Geschütztes darin verbieten das
/// Verschieben als Ganzes.
struct DirSummary {
    files: u64,
    bytes: u64,
    newest: Option<i64>,
}

fn summarize_dir(a: &PlannedAction, env: &ApplyEnv) -> Result<DirSummary, Verdict> {
    let mut summary = DirSummary {
        files: 0,
        bytes: 0,
        newest: None,
    };
    let mut stack = vec![std::path::PathBuf::from(&a.path)];
    while let Some(dir) = stack.pop() {
        let entries = match env.fs.read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(Verdict::Skip(SkipReason::Stale))
            }
            Err(e) => return Err(Verdict::Fail(io_message(&e))),
        };
        for (path, meta) in entries {
            if meta.is_link || meta.is_reparse_point() {
                return Err(Verdict::Skip(SkipReason::Link));
            }
            if meta.is_cloud_only() {
                return Err(Verdict::Skip(SkipReason::CloudPlaceholder));
            }
            if meta.is_dir {
                if env.protector.check_inside(&path).is_some() {
                    return Err(Verdict::Skip(SkipReason::Protected));
                }
                stack.push(path);
            } else {
                if env.protector.check(&path).is_some() {
                    return Err(Verdict::Skip(SkipReason::Protected));
                }
                summary.files += 1;
                summary.bytes += meta.size;
                summary.newest = summary.newest.max(Some(meta.mtime_ticks));
            }
        }
    }
    Ok(summary)
}

/// Ein vorhandener Pfad muss ein echter Ordner sein (kein Link, keine Datei).
fn ensure_plain_dir(env: &ApplyEnv, dir: &Path) -> Result<(), Verdict> {
    match env.fs.metadata(dir) {
        Ok(m) if m.is_reparse_point() || !m.is_dir => Err(Verdict::Fail(format!(
            "{} ist ein Link oder kein Ordner; es wird nichts verschoben",
            paths::display(dir)
        ))),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Verdict::Fail(io_message(&e))),
    }
}

/// Legt den Zielordner unter `_Archiv` an. `_Archiv` und jede Ebene darunter müssen echte
/// Ordner sein: Ein Link würde das Ziel aus der Wurzel hinaus umleiten.
fn prepare_archive_parent(root: &Path, parent: &Path, env: &ApplyEnv) -> Result<(), Verdict> {
    let archive = root.join(ARCHIVE_DIR);
    ensure_plain_dir(env, &archive)?;
    env.fs
        .create_dir_all(parent)
        .map_err(|e| Verdict::Fail(io_message(&e)))?;
    let archive_key = paths::path_key(&archive);
    let mut dir = parent;
    loop {
        ensure_plain_dir(env, dir)?;
        if paths::path_key(dir) == archive_key {
            return Ok(());
        }
        match dir.parent() {
            Some(up) => dir = up,
            None => return Ok(()),
        }
    }
}

/// `versions`: Die behaltene jüngste Datei muss noch da, lokal und nicht älter geworden sein als
/// die verschobene. Es wird nichts gelesen oder gehasht.
fn verify_keep_present(a: &PlannedAction, keep: &str, env: &ApplyEnv) -> Result<(), Verdict> {
    let meta = match env.fs.metadata(Path::new(keep)) {
        Ok(m) if !m.is_dir && !m.is_link && !m.is_cloud_only() => m,
        _ => return Err(Verdict::Skip(SkipReason::KeepMissing)),
    };
    if meta.mtime_ticks < a.mtime_ticks {
        return Err(Verdict::Skip(SkipReason::Stale));
    }
    Ok(())
}

/// Alle Prüfungen vor dem Journal. `Ok` liefert das freie Ziel unter `_Archiv`.
fn verify_move(
    a: &PlannedAction,
    root: &Path,
    env: &ApplyEnv,
    kind: PlanKind,
) -> Result<std::path::PathBuf, Verdict> {
    let src = Path::new(&a.path);
    let Some(target) = a.target.as_deref().map(std::path::PathBuf::from) else {
        return Err(Verdict::Fail("move ohne Ziel".into()));
    };
    if env.protector.check_move(src, &target).is_some() {
        return Err(Verdict::Skip(SkipReason::Protected));
    }
    if is_in_archive(root, src) {
        return Err(Verdict::Skip(SkipReason::InArchive));
    }
    if a.is_dir {
        let meta = match env.fs.metadata(src) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(Verdict::Skip(SkipReason::AlreadyDone))
            }
            Err(e) => return Err(Verdict::Fail(io_message(&e))),
        };
        if meta.is_cloud_only() {
            return Err(Verdict::Skip(SkipReason::CloudPlaceholder));
        }
        if meta.is_link || meta.is_reparse_point() {
            return Err(Verdict::Skip(SkipReason::Link));
        }
        if !meta.is_dir {
            return Err(Verdict::Skip(SkipReason::Stale));
        }
        if env.protector.check_inside(src).is_some() {
            return Err(Verdict::Skip(SkipReason::Protected));
        }
        let summary = summarize_dir(a, env)?;
        if summary.files != a.files.unwrap_or(0)
            || summary.bytes != a.size
            || summary.newest != Some(a.mtime_ticks)
        {
            return Err(Verdict::Skip(SkipReason::Stale));
        }
    } else {
        verify_source(a, env)?;
        if let Some(keep) = &a.keep {
            verify_keep_present(a, keep, env)?;
        }
    }

    if kind == PlanKind::Rules {
        return verify_rules_target(a, src, target, env);
    }
    if paths::display(&target).chars().count() > MAX_TARGET_LEN {
        return Err(Verdict::Skip(SkipReason::TooLong));
    }
    if env.fs.exists(&target) {
        return Err(Verdict::Skip(SkipReason::TargetExists));
    }
    let parent = target.parent().unwrap_or(root);
    prepare_archive_parent(root, parent, env)?;
    let same_volume = match (env.fs.volume_serial(src), env.fs.volume_serial(parent)) {
        (Ok(x), Ok(y)) => x == y,
        (Err(e), _) | (_, Err(e)) => return Err(Verdict::Fail(io_message(&e))),
    };
    if !same_volume {
        return Err(Verdict::Skip(SkipReason::DifferentVolume));
    }
    Ok(target)
}

/// Ziel eines Regel-Moves: Länge, Schreibweisen-Umbenennen oder freies Ziel. Der Zielordner
/// wird erst danach angelegt ([`prepare_rules_parent`]), weil das ins Journal gehört.
fn verify_rules_target(
    a: &PlannedAction,
    src: &Path,
    target: std::path::PathBuf,
    env: &ApplyEnv,
) -> Result<std::path::PathBuf, Verdict> {
    if paths::display(&target).chars().count() > MAX_PATH_CHARS {
        return Err(Verdict::Skip(SkipReason::PathTooLong));
    }
    if is_case_only_rename(src, &target) {
        // Das Ziel „existiert“, weil es die Quelle selbst ist: nur dann erlaubt.
        return match env.fs.same_file(src, &target) {
            Ok(true) if paths::display(&target) != a.path => Ok(target),
            Ok(_) => Err(Verdict::Skip(SkipReason::TargetExists)),
            Err(e) => Err(Verdict::Fail(io_message(&e))),
        };
    }
    if env.fs.exists(&target) {
        return Err(Verdict::Skip(SkipReason::TargetExists));
    }
    Ok(target)
}

/// Quelle und Ziel unterscheiden sich höchstens in der Groß-/Kleinschreibung.
fn is_case_only_rename(src: &Path, target: &Path) -> bool {
    paths::path_key(src) == paths::path_key(target)
}

enum PrepareError {
    Verdict(Verdict),
    Apply(ApplyError),
}

impl From<Verdict> for PrepareError {
    fn from(v: Verdict) -> Self {
        Self::Verdict(v)
    }
}

impl From<io::Error> for PrepareError {
    fn from(e: io::Error) -> Self {
        Self::Apply(ApplyError::Journal(e))
    }
}

/// Legt fehlende Zielordner von oben nach unten an. Jeder neue Ordner wird **vorher** als
/// `created_dir` ins Journal geschrieben (Undo entfernt genau diese). Jede Ebene muss ein
/// echter Ordner sein: Ein Link würde das Ziel aus der Wurzel hinaus umleiten.
fn prepare_rules_parent(
    a: &PlannedAction,
    root: &Path,
    parent: &Path,
    env: &ApplyEnv,
    journal: &mut JournalWriter,
) -> Result<(), PrepareError> {
    let root_key = paths::path_key(root);
    let mut missing = Vec::new();
    let mut dir = parent;
    while paths::path_key(dir) != root_key {
        match env.fs.metadata(dir) {
            Ok(_) => ensure_plain_dir(env, dir)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => missing.push(dir),
            Err(e) => return Err(Verdict::Fail(io_message(&e)).into()),
        }
        match dir.parent() {
            Some(up) if paths::is_under(&paths::path_key(up), &root_key) => dir = up,
            _ => {
                return Err(Verdict::Fail("Ziel liegt nicht in der Wurzel".into()).into());
            }
        }
    }
    for dir in missing.into_iter().rev() {
        journal.append(&Entry::CreatedDir {
            run: env.run.clone(),
            action: a.id,
            path: paths::display(dir),
        })?;
        match env.fs.create_dir(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(Verdict::Fail(io_message(&e)).into()),
        }
        ensure_plain_dir(env, dir)?;
    }
    Ok(())
}

fn process_move(
    a: &PlannedAction,
    root: &Path,
    env: &ApplyEnv,
    journal: &mut JournalWriter,
    kind: PlanKind,
) -> Result<ActionStatus, ApplyError> {
    let target = match verify_move(a, root, env, kind) {
        Ok(target) => target,
        Err(verdict) => return record_verdict(verdict, env.run.clone(), a.id, journal),
    };
    if kind == PlanKind::Rules && !is_case_only_rename(Path::new(&a.path), &target) {
        let parent = target.parent().unwrap_or(root);
        let prepared = prepare_rules_parent(a, root, parent, env, journal).and_then(|()| {
            let same_volume = match (
                env.fs.volume_serial(Path::new(&a.path)),
                env.fs.volume_serial(parent),
            ) {
                (Ok(x), Ok(y)) => x == y,
                (Err(e), _) | (_, Err(e)) => return Err(Verdict::Fail(io_message(&e)).into()),
            };
            if same_volume {
                Ok(())
            } else {
                Err(Verdict::Skip(SkipReason::DifferentVolume).into())
            }
        });
        match prepared {
            Ok(()) => {}
            Err(PrepareError::Verdict(verdict)) => {
                return record_verdict(verdict, env.run.clone(), a.id, journal)
            }
            Err(PrepareError::Apply(e)) => return Err(e),
        }
    }
    // Write-ahead: Ohne gesicherten `intent` wird nichts verschoben.
    journal.append(&Entry::Intent {
        run: env.run.clone(),
        action: a.id,
        from: a.path.clone(),
        to: paths::display(&target),
        size: a.size,
        hash: None,
        dest: if kind == PlanKind::Rules {
            Dest::Rules
        } else {
            Dest::Archive
        },
        is_dir: a.is_dir,
    })?;
    finish_rename(env, journal, a, &target)
}

fn process(
    a: &PlannedAction,
    root: &Path,
    env: &ApplyEnv,
    journal: &mut JournalWriter,
    keeps: &mut HashMap<String, Option<SkipReason>>,
    before: &DirSnapshot,
    kind: PlanKind,
) -> Result<ActionStatus, ApplyError> {
    match a.action {
        ActionType::Quarantine => process_quarantine(a, root, env, journal, keeps),
        ActionType::RemoveDir => process_remove_dir(a, env, journal, before),
        ActionType::Move => process_move(a, root, env, journal, kind),
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
    let before = snapshot_dirs(plan, env);
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
        let status = process(action, root, env, &mut journal, &mut keeps, &before, plan.kind)?;
        if status == ActionStatus::Done {
            outcome.moved_bytes += action.size;
        }
        let result = ActionResult {
            id: action.id,
            path: action.path.clone(),
            kind: action.action,
            sorted: plan.kind == PlanKind::Rules && action.action == ActionType::Move,
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
        assert_eq!(
            out.counts(),
            ActionCounts {
                quarantined: 2,
                ..ActionCounts::default()
            }
        );
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

    fn remove_dir_intents(entries: &[Entry]) -> Vec<(String, u32, i64)> {
        entries
            .iter()
            .filter_map(|e| match e {
                Entry::IntentRemoveDir {
                    path,
                    attrs,
                    mtime_ticks,
                    ..
                } => Some((path.clone(), *attrs, *mtime_ticks)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn leere_ordner_werden_von_unten_nach_oben_entfernt_und_journalisiert() {
        let fx = fx();
        fx.mkdir("a/b/c");
        fx.mkdir("d");
        let plan = fx.dir_plan(&["a/b/c", "a/b", "a", "d"]);

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.exit_code()), (4, 0));
        assert_eq!(
            out.counts(),
            ActionCounts {
                dirs_removed: 4,
                ..ActionCounts::default()
            }
        );
        assert!(!fx.exists("a") && !fx.exists("d"));
        assert!(fx.root.exists(), "die Wurzel bleibt");
        let entries = fx.journal(RUN);
        assert_eq!(remove_dir_intents(&entries).len(), 4);
        let order: Vec<bool> = entries
            .iter()
            .filter(|e| matches!(e, Entry::IntentRemoveDir { .. } | Entry::Done { .. }))
            .map(|e| matches!(e, Entry::IntentRemoveDir { .. }))
            .collect();
        assert_eq!(
            order,
            [true, false, true, false, true, false, true, false],
            "intent immer vor done"
        );
        assert!(matches!(
            entries.last(),
            Some(Entry::RunEnd {
                status: EndStatus::Complete,
                ..
            })
        ));
    }

    #[test]
    fn journal_haelt_attribute_und_zeiten_vom_zustand_vor_dem_lauf_fest() {
        let fx = fx();
        let a = fx.mkdir("a");
        let b = fx.mkdir("a/b");
        let long_ago = 1_600_000_000 * crate::scan::source::TICKS_PER_SEC;
        let created = 1_500_000_000 * crate::scan::source::TICKS_PER_SEC;
        RealFs.set_dir_meta(&b, 0x10, long_ago, created).unwrap();
        RealFs
            .set_dir_meta(&a, 0x10 | 0x2, long_ago, created)
            .unwrap();
        let plan = fx.dir_plan(&["a/b", "a"]);
        // Plan und Dateisystem weichen ab: Maßgeblich ist der Zustand beim Apply, nicht der Plan.
        let mut plan = plan;
        plan.actions[1].mtime_ticks = 42;

        apply(&fx, &plan);

        let intents = remove_dir_intents(&fx.journal(RUN));
        assert_eq!(intents.len(), 2);
        let (b_path, b_attrs, b_mtime) = &intents[0];
        assert!(b_path.ends_with(r"a\b"), "{b_path}");
        assert_eq!(*b_mtime, long_ago);
        assert_eq!(b_attrs & 0x2, 0, "b ist nicht versteckt");
        let (a_path, a_attrs, a_mtime) = &intents[1];
        assert!(a_path.ends_with(r"\a"), "{a_path}");
        assert_ne!(a_attrs & 0x2, 0, "a ist versteckt");
        assert_eq!(
            *a_mtime, long_ago,
            "das Entfernen von a\\b ändert die mtime von a; festgehalten wird der Zustand \
             vor dem Lauf, damit Undo den ursprünglichen Wert wiederherstellt"
        );
        let Some(Entry::IntentRemoveDir { ctime_ticks, .. }) = fx
            .journal(RUN)
            .into_iter()
            .find(|e| matches!(e, Entry::IntentRemoveDir { .. }))
        else {
            panic!("kein Intent");
        };
        assert_eq!(ctime_ticks, created);
    }

    #[test]
    fn ordner_der_inzwischen_nicht_mehr_leer_ist_gilt_als_stale() {
        let fx = fx();
        fx.mkdir("a");
        fx.mkdir("b");
        let plan = fx.dir_plan(&["a", "b"]);
        fx.write("a/neu.txt", "inhalt");

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.stale(), out.exit_code()), (1, 1, 2));
        assert!(fx.exists("a/neu.txt") && !fx.exists("b"));
        assert!(
            remove_dir_intents(&fx.journal(RUN)).len() == 1,
            "stale schreibt kein intent"
        );
    }

    #[test]
    fn versteckte_und_system_dateien_zaehlen_als_inhalt() {
        let fx = fx();
        fx.mkdir("a");
        let plan = fx.dir_plan(&["a"]);
        let hidden = fx.write("a/versteckt.txt", "x");
        std::process::Command::new("attrib")
            .args(["+H", "+S"])
            .arg(&hidden)
            .output()
            .unwrap();

        let out = apply(&fx, &plan);

        assert_eq!(out.stale(), 1);
        assert!(fx.exists("a"));
    }

    #[test]
    fn nicht_mehr_vorhandener_ordner_ist_bereits_erledigt_und_zweiter_lauf_aendert_nichts() {
        let fx = fx();
        fx.mkdir("a");
        let plan = fx.dir_plan(&["a"]);
        assert_eq!(apply(&fx, &plan).executed(), 1);

        let second =
            run_with(&fx, &plan, &RealFs, &fx.protector(), "20261003-130000-cd34").unwrap();

        assert_eq!((second.executed(), second.already_done()), (0, 1));
        assert_eq!(second.exit_code(), 0);
        assert!(remove_dir_intents(&fx.journal("20261003-130000-cd34")).is_empty());
    }

    #[test]
    fn datei_statt_ordner_ist_stale_und_bleibt() {
        let fx = fx();
        fx.mkdir("a");
        let plan = fx.dir_plan(&["a"]);
        std::fs::remove_dir(fx.root.join("a")).unwrap();
        fx.write("a", "jetzt eine datei");

        let out = apply(&fx, &plan);

        assert_eq!(out.stale(), 1);
        assert_eq!(fx.read("a"), "jetzt eine datei");
    }

    #[test]
    fn geschuetzter_ordner_wird_nie_entfernt() {
        let fx = fx();
        fx.mkdir("wichtig");
        let plan = fx.dir_plan(&["wichtig"]);
        let config = Config {
            protected_paths: vec![crate::paths::display(&fx.root.join("wichtig"))],
            ..Config::default()
        };
        let protector =
            crate::change::protect::Protector::new(&fx.root, &config, &ProtectPaths::default());

        let out = run_with(&fx, &plan, &RealFs, &protector, RUN).unwrap();

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::Protected)
        );
        assert!(fx.exists("wichtig"));
    }

    #[test]
    fn junction_wird_nie_entfernt() {
        let fx = fx();
        let target = fx.mkdir("ziel");
        let link = fx.root.join("verweis");
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
        let plan = fx.dir_plan(&["verweis"]);

        let out = apply(&fx, &plan);

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::Link)
        );
        assert!(link.exists() && target.exists());
    }

    #[test]
    fn cloud_platzhalter_ordner_wird_uebersprungen() {
        let fx = fx();
        let dir = fx.mkdir("wolke");
        let plan = fx.dir_plan(&["wolke"]);
        let faulty = FaultyFs::new().cloud_only(&dir);

        let out = run_with(&fx, &plan, &faulty, &fx.protector(), RUN).unwrap();

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::CloudPlaceholder)
        );
        assert!(fx.exists("wolke"));
    }

    #[test]
    fn fehler_beim_entfernen_wird_gemeldet_und_der_lauf_geht_weiter() {
        let fx = fx();
        let gesperrt = fx.mkdir("gesperrt");
        fx.mkdir("frei");
        let plan = fx.dir_plan(&["gesperrt", "frei"]);
        let faulty = FaultyFs::new().fail(Op::RemoveDir, &gesperrt);

        let out = run_with(&fx, &plan, &faulty, &fx.protector(), RUN).unwrap();

        assert_eq!((out.executed(), out.failed(), out.exit_code()), (1, 1, 2));
        assert!(fx.exists("gesperrt") && !fx.exists("frei"));
        let entries = fx.journal(RUN);
        assert!(entries
            .iter()
            .any(|e| matches!(e, Entry::Fail { action: 1, .. })));
    }

    #[test]
    fn wird_der_ordner_erst_nach_der_vorpruefung_befuellt_ist_das_ergebnis_stale() {
        let fx = fx();
        let a = fx.mkdir("a");
        let plan = fx.dir_plan(&["a"]);
        let faulty = FaultyFs::new().fail(Op::RemoveDirNotEmpty, &a);

        let out = run_with(&fx, &plan, &faulty, &fx.protector(), RUN).unwrap();

        assert_eq!((out.executed(), out.stale()), (0, 1));
        let entries = fx.journal(RUN);
        assert_eq!(remove_dir_intents(&entries).len(), 1);
        assert!(entries.iter().any(|e| matches!(
            e,
            Entry::Skip {
                action: 1,
                reason: SkipReason::Stale,
                ..
            }
        )));
    }

    fn archive_tree(fx: &Fx) {
        fx.write("Projekt/a.txt", "alpha");
        fx.write("Projekt/sub/b.txt", "bravo bravo");
        fx.write("Projekt/sub/tief/c.txt", "charlie");
    }

    fn archive_intents(entries: &[Entry]) -> Vec<(String, String, bool, Dest, Option<String>)> {
        entries
            .iter()
            .filter_map(|e| match e {
                Entry::Intent {
                    from,
                    to,
                    is_dir,
                    dest,
                    hash,
                    ..
                } => Some((from.clone(), to.clone(), *is_dir, *dest, hash.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ordner_wird_als_ganzes_ins_archiv_verschoben_und_journalisiert() {
        let fx = fx();
        archive_tree(&fx);
        let plan = fx.archive_plan(&["Projekt"]);
        let target = plan.actions[0].target.clone().unwrap();
        let mtime = RealFs
            .metadata(&fx.root.join("Projekt/sub/b.txt"))
            .unwrap()
            .mtime_ticks;

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.exit_code()), (1, 0));
        assert_eq!(
            out.counts(),
            ActionCounts {
                archived: 1,
                ..ActionCounts::default()
            }
        );
        assert!(!fx.exists("Projekt"));
        let moved = std::path::Path::new(&target);
        assert_eq!(
            std::fs::read_to_string(moved.join("sub").join("b.txt")).unwrap(),
            "bravo bravo"
        );
        assert_eq!(
            RealFs
                .metadata(&moved.join("sub").join("b.txt"))
                .unwrap()
                .mtime_ticks,
            mtime
        );
        let entries = fx.journal(RUN);
        assert_eq!(
            archive_intents(&entries),
            [(
                plan.actions[0].path.clone(),
                target,
                true,
                Dest::Archive,
                None
            )]
        );
        let order: Vec<bool> = entries
            .iter()
            .filter(|e| matches!(e, Entry::Intent { .. } | Entry::Done { .. }))
            .map(|e| matches!(e, Entry::Intent { .. }))
            .collect();
        assert_eq!(order, [true, false], "intent vor done");
        assert!(matches!(
            entries.last(),
            Some(Entry::RunEnd {
                status: EndStatus::Complete,
                ..
            })
        ));
        assert!(
            !fx.quarantined(RUN, "Projekt").exists(),
            "nichts in der Quarantäne"
        );
    }

    #[test]
    fn veraenderter_ordner_ist_stale() {
        // (Veränderung, Beschreibung)
        type Change = fn(&Fx);
        let changes: [(Change, &str); 4] = [
            (
                |fx| drop(fx.write("Projekt/neu.txt", "x")),
                "Datei hinzugekommen",
            ),
            (
                |fx| std::fs::remove_file(fx.root.join("Projekt/a.txt")).unwrap(),
                "Datei entfernt",
            ),
            (
                |fx| drop(fx.write("Projekt/sub/b.txt", "anders und länger")),
                "Größe geändert",
            ),
            (
                |fx| {
                    let f = std::fs::File::options()
                        .write(true)
                        .open(fx.root.join("Projekt/sub/tief/c.txt"))
                        .unwrap();
                    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
                    f.set_modified(later).unwrap();
                },
                "jüngste mtime geändert",
            ),
        ];
        for (change, what) in changes {
            let fx = fx();
            archive_tree(&fx);
            let plan = fx.archive_plan(&["Projekt"]);
            change(&fx);

            let out = apply(&fx, &plan);

            assert_eq!(
                (out.executed(), out.stale(), out.exit_code()),
                (0, 1, 2),
                "{what}"
            );
            assert!(fx.exists("Projekt"), "{what}");
            assert!(
                archive_intents(&fx.journal(RUN)).is_empty(),
                "{what}: kein intent"
            );
        }
    }

    #[test]
    fn vorhandenes_ziel_wird_nie_ueberschrieben_oder_zusammengefuehrt() {
        let fx = fx();
        archive_tree(&fx);
        let plan = fx.archive_plan(&["Projekt"]);
        let target = plan.actions[0].target.clone().unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(std::path::Path::new(&target).join("alt.txt"), "bestand").unwrap();

        let out = apply(&fx, &plan);

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::TargetExists)
        );
        assert!(fx.exists("Projekt/a.txt"));
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(&target).join("alt.txt")).unwrap(),
            "bestand"
        );
    }

    #[test]
    fn fehlende_quelle_ist_bereits_erledigt_und_zweiter_lauf_aendert_nichts() {
        let fx = fx();
        archive_tree(&fx);
        let plan = fx.archive_plan(&["Projekt"]);
        assert_eq!(apply(&fx, &plan).executed(), 1);

        let second =
            run_with(&fx, &plan, &RealFs, &fx.protector(), "20261003-130000-cd34").unwrap();

        assert_eq!((second.executed(), second.already_done()), (0, 1));
        assert_eq!(second.exit_code(), 0);
    }

    #[test]
    fn cloud_platzhalter_im_ordner_verhindert_das_verschieben() {
        let fx = fx();
        archive_tree(&fx);
        let plan = fx.archive_plan(&["Projekt"]);
        let faulty = FaultyFs::new().cloud_only(&fx.root.join("Projekt/sub/b.txt"));

        let out = run_with(&fx, &plan, &faulty, &fx.protector(), RUN).unwrap();

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::CloudPlaceholder)
        );
        assert!(fx.exists("Projekt/sub/b.txt"));
        assert!(faulty.hashed().is_empty());
    }

    #[test]
    fn junction_im_ordner_verhindert_das_verschieben() {
        let fx = fx();
        archive_tree(&fx);
        let outside = fx.mkdir("woanders");
        let link = fx.root.join("Projekt").join("verweis");
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("Junction nicht erzeugbar, Test übersprungen");
            return;
        }
        let mut plan = fx.archive_plan(&["Projekt"]);
        // Der Plan stammt von vor dem Link; Zähler wie im Plan, der Link kommt neu hinzu.
        plan.actions[0].files = Some(3);

        let out = apply(&fx, &plan);

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::Link)
        );
        assert!(fx.exists("Projekt/a.txt") && outside.exists());
    }

    #[test]
    fn geschuetzter_inhalt_verhindert_das_verschieben() {
        let fx = fx();
        archive_tree(&fx);
        fx.write("Projekt/sub/wichtig/x.txt", "x");
        let plan = fx.archive_plan(&["Projekt"]);
        let config = Config {
            protected_paths: vec![crate::paths::display(&fx.root.join("Projekt/sub/wichtig"))],
            ..Config::default()
        };
        let protector =
            crate::change::protect::Protector::new(&fx.root, &config, &ProtectPaths::default());

        let out = run_with(&fx, &plan, &RealFs, &protector, RUN).unwrap();

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::Protected)
        );
        assert!(fx.exists("Projekt/sub/wichtig/x.txt"));
    }

    #[test]
    fn geschuetztes_ziel_verhindert_das_verschieben() {
        let fx = fx();
        archive_tree(&fx);
        let plan = fx.archive_plan(&["Projekt"]);
        let config = Config {
            protected_paths: vec![crate::paths::display(&fx.root.join("_Archiv"))],
            ..Config::default()
        };
        let protector =
            crate::change::protect::Protector::new(&fx.root, &config, &ProtectPaths::default());

        let out = run_with(&fx, &plan, &RealFs, &protector, RUN).unwrap();

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::Protected)
        );
        assert!(fx.exists("Projekt"));
        assert!(!fx.exists("_Archiv"), "es wird nichts angelegt");
    }

    #[test]
    fn archiv_als_junction_wird_nicht_betreten() {
        let fx = fx();
        archive_tree(&fx);
        let outside = tempfile::tempdir().unwrap();
        let link = fx.root.join("_Archiv");
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("Junction nicht erzeugbar, Test übersprungen");
            return;
        }
        let plan = fx.archive_plan(&["Projekt"]);

        let out = apply(&fx, &plan);

        assert_eq!(out.executed(), 0);
        assert!(matches!(out.results[0].status, ActionStatus::Failed(_)));
        assert!(fx.exists("Projekt/a.txt"));
        assert_eq!(
            std::fs::read_dir(outside.path()).unwrap().count(),
            0,
            "außerhalb der Wurzel darf nichts landen"
        );
    }

    #[test]
    fn quelle_unterhalb_von_archiv_wird_nicht_erneut_verschoben() {
        let fx = fx();
        fx.write("_Archiv/2019/Alt/a.txt", "x");
        let mut plan = fx.archive_plan(&["_Archiv/2019/Alt"]);
        plan.actions[0].target = Some(crate::paths::display(&fx.root.join("_Archiv/2020/Alt")));

        let out = apply(&fx, &plan);

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::InArchive)
        );
        assert!(fx.exists("_Archiv/2019/Alt/a.txt"));
    }

    #[test]
    fn gesperrter_ordner_scheitert_ohne_teilverschiebung_und_der_lauf_geht_weiter() {
        let fx = fx();
        archive_tree(&fx);
        fx.write("Zweiter/z.txt", "zz");
        let plan = fx.archive_plan(&["Projekt", "Zweiter"]);
        let faulty = FaultyFs::new().fail(Op::Rename, &fx.root.join("Projekt"));

        let out = run_with(&fx, &plan, &faulty, &fx.protector(), RUN).unwrap();

        assert_eq!((out.executed(), out.failed(), out.exit_code()), (1, 1, 2));
        assert!(fx.exists("Projekt/sub/tief/c.txt"), "kein Teilverschieben");
        assert!(!fx.exists("Zweiter"));
        let entries = fx.journal(RUN);
        assert!(entries
            .iter()
            .any(|e| matches!(e, Entry::Fail { action: 1, .. })));
    }

    #[test]
    fn einzelne_datei_wird_mit_groesse_und_mtime_geprueft_und_verschoben() {
        let fx = fx();
        fx.write("Bericht_v1.docx", "version eins");
        fx.write("Bericht_v2.docx", "version zwei");
        let plan = fx.move_file_plan(&[("Bericht_v1.docx", r"Versionen\Bericht_v1.docx")]);

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.exit_code()), (1, 0));
        assert!(!fx.exists("Bericht_v1.docx") && fx.exists("Bericht_v2.docx"));
        assert_eq!(
            fx.read(r"_Archiv\Versionen\Bericht_v1.docx"),
            "version eins"
        );
        let intents = archive_intents(&fx.journal(RUN));
        assert_eq!(intents.len(), 1);
        assert!(!intents[0].2, "keine Ordner-Aktion");
        assert_eq!(intents[0].3, Dest::Archive);
    }

    #[test]
    fn geaenderte_einzeldatei_ist_stale() {
        let fx = fx();
        fx.write("a.docx", "kurz");
        let plan = fx.move_file_plan(&[("a.docx", "Versionen/a.docx")]);
        fx.write("a.docx", "jetzt länger");

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.stale()), (0, 1));
        assert!(fx.exists("a.docx") && !fx.exists("_Archiv"));
    }

    /// Zwei ältere Versionen und die behaltene jüngste; Plan wie `plan versions`.
    fn versions_fixture() -> (Fx, Plan) {
        let fx = fx();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(400 * 86_400);
        let mid = std::time::SystemTime::now() - std::time::Duration::from_secs(200 * 86_400);
        for (name, time) in [("Bericht_v1.docx", old), ("Bericht_v2.docx", mid)] {
            let path = fx.write(name, name);
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(time)
                .unwrap();
        }
        fx.write("Bericht final.docx", "final");
        let mut plan = fx.move_file_plan(&[
            ("Bericht_v1.docx", r"Versionen\Bericht_v1.docx"),
            ("Bericht_v2.docx", r"Versionen\Bericht_v2.docx"),
        ]);
        let keep = crate::paths::display(&fx.root.join("Bericht final.docx"));
        for action in &mut plan.actions {
            action.keep = Some(keep.clone());
        }
        (fx, plan)
    }

    #[test]
    fn aeltere_versionen_gehen_ins_archiv_die_behaltene_bleibt() {
        let (fx, plan) = versions_fixture();

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.exit_code()), (2, 0));
        assert!(!fx.exists("Bericht_v1.docx") && !fx.exists("Bericht_v2.docx"));
        assert_eq!(fx.read("Bericht final.docx"), "final");
        assert_eq!(
            fx.read(r"_Archiv\Versionen\Bericht_v2.docx"),
            "Bericht_v2.docx"
        );
    }

    #[test]
    fn fehlende_behaltene_datei_ueberspringt_die_ganze_gruppe() {
        let (fx, plan) = versions_fixture();
        std::fs::remove_file(fx.root.join("Bericht final.docx")).unwrap();

        let out = apply(&fx, &plan);

        assert_eq!(out.executed(), 0);
        assert!(out
            .results
            .iter()
            .all(|r| r.status == ActionStatus::Skipped(SkipReason::KeepMissing)));
        assert!(fx.exists("Bericht_v1.docx") && fx.exists("Bericht_v2.docx"));
        assert!(!fx.exists("_Archiv"));
    }

    #[test]
    fn behaltene_datei_die_aelter_als_die_verschobene_geworden_ist_gilt_als_stale() {
        let (fx, plan) = versions_fixture();
        let ancient = std::time::SystemTime::now() - std::time::Duration::from_secs(900 * 86_400);
        std::fs::File::options()
            .write(true)
            .open(fx.root.join("Bericht final.docx"))
            .unwrap()
            .set_modified(ancient)
            .unwrap();

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.stale()), (0, 2));
        assert!(fx.exists("Bericht_v1.docx"));
    }

    #[test]
    fn behaltene_datei_als_cloud_platzhalter_oder_ordner_ist_nicht_nutzbar_und_wird_nie_gelesen() {
        let (fx, plan) = versions_fixture();
        let keep = fx.root.join("Bericht final.docx");
        let faulty = FaultyFs::new().cloud_only(&keep);

        let out = run_with(&fx, &plan, &faulty, &fx.protector(), RUN).unwrap();

        assert!(out
            .results
            .iter()
            .all(|r| r.status == ActionStatus::Skipped(SkipReason::KeepMissing)));
        assert!(faulty.hashed().is_empty(), "keine Inhalte lesen");

        // Ein Ordner an der Stelle der behaltenen Datei ist ebenfalls unbrauchbar.
        let (fx, plan) = versions_fixture();
        std::fs::remove_file(fx.root.join("Bericht final.docx")).unwrap();
        fx.mkdir("Bericht final.docx");
        let out = apply(&fx, &plan);
        assert_eq!(out.executed(), 0);
        assert!(fx.exists("Bericht_v1.docx"));
    }

    #[test]
    fn behaltene_datei_mit_gleicher_mtime_ist_zulaessig() {
        let fx = fx();
        fx.write("a_v1.doc", "x");
        fx.write("a_v2.doc", "y");
        let mtime = std::fs::metadata(fx.root.join("a_v1.doc"))
            .unwrap()
            .modified()
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(fx.root.join("a_v2.doc"))
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        let mut plan = fx.move_file_plan(&[("a_v1.doc", "Versionen/a_v1.doc")]);
        plan.actions[0].keep = Some(crate::paths::display(&fx.root.join("a_v2.doc")));

        let out = apply(&fx, &plan);

        assert_eq!(out.executed(), 1);
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
                    kind: ActionType::Quarantine,
                    sorted: false,
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

    // --- Phase 4: Regel-Moves (`rules`) ---

    fn created_dirs(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .filter_map(|e| match e {
                Entry::CreatedDir { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    fn rel(fx: &Fx, path: &str) -> String {
        path.trim_start_matches(&crate::paths::display(&fx.root))
            .trim_start_matches('\\')
            .to_string()
    }

    #[test]
    fn regel_move_legt_zielordner_an_und_protokolliert_sie_vor_dem_intent() {
        let fx = fx();
        fx.write("Eingang/x.txt", "inhalt");
        let mtime = RealFs
            .metadata(&fx.root.join("Eingang/x.txt"))
            .unwrap()
            .mtime_ticks;
        let plan = fx.rules_plan(&[("Eingang/x.txt", r"Neu\Sub\x.txt")]);

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.exit_code()), (1, 0));
        assert_eq!(
            out.counts(),
            ActionCounts {
                sorted: 1,
                ..ActionCounts::default()
            }
        );
        assert_eq!(fx.read("Neu/Sub/x.txt"), "inhalt");
        assert!(!fx.exists("Eingang/x.txt"));
        assert_eq!(
            RealFs
                .metadata(&fx.root.join("Neu/Sub/x.txt"))
                .unwrap()
                .mtime_ticks,
            mtime
        );
        let entries = fx.journal(RUN);
        let dirs: Vec<_> = created_dirs(&entries)
            .iter()
            .map(|p| rel(&fx, p))
            .collect();
        assert_eq!(dirs, ["Neu", r"Neu\Sub"], "von oben nach unten");
        let first_created = entries
            .iter()
            .position(|e| matches!(e, Entry::CreatedDir { .. }))
            .unwrap();
        let intent = entries
            .iter()
            .position(|e| matches!(e, Entry::Intent { .. }))
            .unwrap();
        assert!(first_created < intent, "write-ahead: Ordner vor dem Move");
        assert!(matches!(
            &entries[intent],
            Entry::Intent {
                dest: Dest::Rules,
                is_dir: false,
                ..
            }
        ));
    }

    #[test]
    fn vorhandener_zielordner_bekommt_keinen_created_dir_eintrag() {
        let fx = fx();
        fx.write("x.txt", "a");
        fx.mkdir("Neu");
        let plan = fx.rules_plan(&[("x.txt", r"Neu\Sub\x.txt")]);

        apply(&fx, &plan);

        let dirs: Vec<_> = created_dirs(&fx.journal(RUN))
            .iter()
            .map(|p| rel(&fx, p))
            .collect();
        assert_eq!(dirs, [r"Neu\Sub"]);
    }

    #[test]
    fn zielordner_der_ein_link_ist_wird_abgelehnt() {
        let fx = fx();
        fx.write("x.txt", "a");
        let outside = tempfile::tempdir().unwrap();
        let link = fx.root.join("Neu");
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("Junction nicht erzeugbar, Test übersprungen");
            return;
        }
        let plan = fx.rules_plan(&[("x.txt", r"Neu\x.txt")]);

        let out = apply(&fx, &plan);

        assert_eq!(out.executed(), 0);
        assert!(matches!(out.results[0].status, ActionStatus::Failed(_)));
        assert!(fx.exists("x.txt"));
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn regel_ziel_das_inzwischen_belegt_ist_wird_nicht_ueberschrieben() {
        let fx = fx();
        fx.write("x.txt", "neu");
        let plan = fx.rules_plan(&[("x.txt", r"Neu\x.txt")]);
        fx.write("Neu/x.txt", "schon da");

        let out = apply(&fx, &plan);

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::TargetExists)
        );
        assert_eq!(fx.read("Neu/x.txt"), "schon da");
        assert_eq!(fx.read("x.txt"), "neu");
        assert!(created_dirs(&fx.journal(RUN)).is_empty());
    }

    #[test]
    fn regel_move_einer_geaenderten_quelle_ist_stale() {
        let fx = fx();
        fx.write("x.txt", "alt");
        let plan = fx.rules_plan(&[("x.txt", r"Neu\x.txt")]);
        fx.write("x.txt", "ganz anders und laenger");

        let out = apply(&fx, &plan);

        assert_eq!(out.stale(), 1);
        assert!(fx.exists("x.txt") && !fx.exists("Neu"));
    }

    #[test]
    fn umbenennen_nur_in_der_schreibweise_klappt_ohne_target_exists() {
        let fx = fx();
        fx.write("Fotos/foto.JPG", "bild");
        let plan = fx.rules_plan(&[("Fotos/foto.JPG", r"Fotos\foto.jpg")]);

        let out = apply(&fx, &plan);

        assert_eq!((out.executed(), out.exit_code()), (1, 0));
        let names: Vec<_> = std::fs::read_dir(fx.root.join("Fotos"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["foto.jpg"]);
        assert_eq!(fx.read("Fotos/foto.jpg"), "bild");
        assert!(created_dirs(&fx.journal(RUN)).is_empty());
    }

    #[test]
    fn umbenennen_auf_den_namen_einer_anderen_datei_wird_nicht_als_schreibweise_behandelt() {
        let fx = fx();
        fx.write("a.txt", "eins");
        fx.write("b.txt", "zwei");
        let plan = fx.rules_plan(&[("a.txt", "b.txt")]);

        let out = apply(&fx, &plan);

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::TargetExists)
        );
        assert_eq!(fx.read("b.txt"), "zwei");
    }

    #[test]
    fn gesperrte_datei_ist_ein_fehler_und_der_lauf_geht_weiter() {
        let fx = fx();
        fx.write("a.txt", "a");
        fx.write("b.txt", "b");
        let plan = fx.rules_plan(&[("a.txt", r"Neu\a.txt"), ("b.txt", r"Neu\b.txt")]);
        let fs = FaultyFs::new().fail(Op::Rename, &fx.root.join("a.txt"));

        let out = run_with(&fx, &plan, &fs, &fx.protector(), RUN).unwrap();

        assert!(matches!(out.results[0].status, ActionStatus::Failed(_)));
        assert_eq!(out.results[1].status, ActionStatus::Done);
        assert_eq!(out.exit_code(), 2);
        assert!(fx.exists("a.txt") && fx.exists("Neu/b.txt"));
    }

    #[test]
    fn absturz_vor_create_dir_hinterlaesst_created_dir_ohne_ordner() {
        let fx = fx();
        fx.write("x.txt", "a");
        let plan = fx.rules_plan(&[("x.txt", r"Neu\x.txt")]);
        let fs = FaultyFs::new().crash_before_create_dir(1);

        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_with(&fx, &plan, &fs, &fx.protector(), RUN)
        }));

        assert!(crashed.is_err());
        let entries = fx.journal(RUN);
        assert_eq!(created_dirs(&entries).len(), 1);
        assert!(!fx.exists("Neu"), "Ordner wurde nie angelegt");
        assert_eq!(intents(&entries), 0);
        assert!(fx.exists("x.txt"));
    }

    #[test]
    fn absturz_nach_dem_rename_hinterlaesst_intent_ohne_done() {
        let fx = fx();
        fx.write("x.txt", "a");
        let plan = fx.rules_plan(&[("x.txt", r"Neu\x.txt")]);
        let fs = FaultyFs::new().crash_after_rename(1);

        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_with(&fx, &plan, &fs, &fx.protector(), RUN)
        }));

        assert!(crashed.is_err());
        let entries = fx.journal(RUN);
        assert_eq!(intents(&entries), 1);
        assert!(!entries.iter().any(|e| matches!(e, Entry::Done { .. })));
        assert!(fx.exists("Neu/x.txt") && !fx.exists("x.txt"));
    }

    #[test]
    fn regel_ziel_ueber_260_zeichen_wird_uebersprungen_und_legt_nichts_an() {
        let fx = fx();
        fx.write("x.txt", "a");
        let long = "l".repeat(200);
        let plan = fx.rules_plan(&[("x.txt", &format!(r"{long}\{long}\x.txt"))]);

        let out = apply(&fx, &plan);

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::PathTooLong)
        );
        assert!(fx.exists("x.txt") && !fx.root.join(&long).exists());
    }

    #[test]
    fn geschuetztes_regel_ziel_wird_beim_apply_erneut_geprueft() {
        let fx = fx();
        fx.write("x.txt", "a");
        let plan = fx.rules_plan(&[("x.txt", r"Tabu\x.txt")]);
        let config = Config {
            protected_paths: vec![crate::paths::display(&fx.root.join("Tabu"))],
            ..Config::default()
        };
        let protector = Protector::new(&fx.root, &config, &ProtectPaths::default());

        let out = run_with(&fx, &plan, &RealFs, &protector, RUN).unwrap();

        assert_eq!(
            out.results[0].status,
            ActionStatus::Skipped(SkipReason::Protected)
        );
        assert!(fx.exists("x.txt") && !fx.exists("Tabu"));
    }
}
