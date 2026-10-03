//! Undo, `runs` und `purge`. Undo vertraut nicht blind dem Journal, sondern prüft je Eintrag
//! den echten Dateizustand; es überschreibt nie und ist selbst journaliert und wiederholbar.

use std::io;
use std::path::Path;

use chrono::{DateTime, Utc};

use super::fsops::FsOps;
use super::journal::{self, Dest, EndStatus, Entry, JournalError, JournalWriter};
use super::plan::{has_dot_component, ARCHIVE_DIR};
use super::{quarantine, RunId};
use crate::paths;

#[derive(Debug, thiserror::Error)]
pub enum UndoError {
    #[error("Lauf {0} wurde nicht gefunden")]
    NotFound(RunId),
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error("Journal ist ungültig: {0}")]
    Invalid(String),
    #[error("Dateifehler: {0}")]
    Io(#[from] io::Error),
}

pub struct UndoEnv<'a> {
    pub fs: &'a dyn FsOps,
    pub now: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreStatus {
    Restored,
    /// Der Move hat nie stattgefunden oder die Datei liegt schon wieder am Ursprungsort.
    NothingToDo,
    /// Der Ursprungspfad ist belegt; die Datei bleibt in der Quarantäne.
    Conflict(String),
    /// Weder Quarantäne-Datei noch Ursprungsdatei vorhanden (z. B. nach `purge`).
    Missing,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreResult {
    pub action: u32,
    pub path: String,
    pub status: RestoreStatus,
}

#[derive(Debug)]
pub struct UndoOutcome {
    pub run: RunId,
    pub results: Vec<RestoreResult>,
    /// Der Lauf war bereits vollständig zurückgedreht; nichts wurde geändert.
    pub already_undone: bool,
    /// Die Quarantäne wurde per `purge` gelöscht.
    pub purged: bool,
}

impl UndoOutcome {
    fn count(&self, f: impl Fn(&RestoreStatus) -> bool) -> usize {
        self.results.iter().filter(|r| f(&r.status)).count()
    }

    pub fn restored(&self) -> usize {
        self.count(|s| *s == RestoreStatus::Restored)
    }

    pub fn conflicts(&self) -> usize {
        self.count(|s| matches!(s, RestoreStatus::Conflict(_)))
    }

    pub fn missing(&self) -> usize {
        self.count(|s| *s == RestoreStatus::Missing)
    }

    pub fn failed(&self) -> usize {
        self.count(|s| matches!(s, RestoreStatus::Failed(_)))
    }

    /// 0 alles zurück, 2 mit Kollisionen/fehlenden Dateien/Fehlern.
    pub fn exit_code(&self) -> i32 {
        i32::from(self.conflicts() + self.missing() + self.failed() > 0) * 2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Complete,
    Partial,
    /// Kein `run_end`: Prozess wurde beendet.
    Incomplete,
    Undone,
    PartiallyUndone,
    Purged,
    Unreadable,
}

#[derive(Debug, Clone)]
pub struct RunSummary {
    pub run: RunId,
    pub started: Option<String>,
    /// Verschobene Dateien.
    pub moved: usize,
    /// Bytes, die noch in der Quarantäne liegen.
    pub bytes: u64,
    pub status: RunStatus,
    pub expires: Option<DateTime<Utc>>,
}

/// Was eine Aktion laut Journal bewirkt hat.
enum OpKind {
    Move {
        from: String,
        to: String,
        size: u64,
        dest: Dest,
    },
    RemoveDir {
        path: String,
        attrs: u32,
        mtime_ticks: i64,
        ctime_ticks: i64,
    },
}

/// Eine Aktion aus dem Journal mit ihrem bekannten Ausgang.
struct Op {
    action: u32,
    kind: OpKind,
    done: bool,
    /// Fehlgeschlagen oder nach dem `intent` übersprungen: es wurde nichts verändert.
    failed: bool,
    undone: bool,
}

impl Op {
    /// Ursprungspfad: dorthin führt Undo zurück.
    fn origin(&self) -> &str {
        match &self.kind {
            OpKind::Move { from, .. } => from,
            OpKind::RemoveDir { path, .. } => path,
        }
    }

    /// Nach `purge` ist nur der Inhalt der Quarantäne verloren; Archiv-Moves und entfernte
    /// leere Ordner bleiben wiederherstellbar.
    fn lives_in_quarantine(&self) -> bool {
        matches!(
            self.kind,
            OpKind::Move {
                dest: Dest::Quarantine,
                ..
            }
        )
    }
}

fn collect_ops(entries: &[Entry]) -> Vec<Op> {
    let mut ops: Vec<Op> = Vec::new();
    for entry in entries {
        let started = match entry {
            Entry::Intent {
                action,
                from,
                to,
                size,
                dest,
                ..
            } => Some((
                *action,
                OpKind::Move {
                    from: from.clone(),
                    to: to.clone(),
                    size: *size,
                    dest: *dest,
                },
            )),
            Entry::IntentRemoveDir {
                action,
                path,
                attrs,
                mtime_ticks,
                ctime_ticks,
                ..
            } => Some((
                *action,
                OpKind::RemoveDir {
                    path: path.clone(),
                    attrs: *attrs,
                    mtime_ticks: *mtime_ticks,
                    ctime_ticks: *ctime_ticks,
                },
            )),
            _ => None,
        };
        if let Some((action, kind)) = started {
            ops.push(Op {
                action,
                kind,
                done: false,
                failed: false,
                undone: false,
            });
            continue;
        }
        let (action, mark): (u32, fn(&mut Op)) = match entry {
            Entry::Done { action, .. } => (*action, |o| o.done = true),
            Entry::Fail { action, .. } | Entry::Skip { action, .. } => {
                (*action, |o| o.failed = true)
            }
            Entry::UndoDone { action, .. } => (*action, |o| o.undone = true),
            _ => continue,
        };
        if let Some(op) = ops.iter_mut().rev().find(|o| o.action == action) {
            mark(op);
        }
    }
    ops
}

/// Lädt und prüft das Journal: gehört es zu dieser Wurzel und zu diesem Lauf?
fn load(root: &Path, run: &RunId) -> Result<Vec<Entry>, UndoError> {
    let path = quarantine::journal_path(root, run);
    if std::fs::metadata(paths::extended(&path)).is_err() {
        return Err(UndoError::NotFound(run.clone()));
    }
    let entries = journal::read(&path)?;
    match entries.first() {
        Some(Entry::RunStart { root: recorded, .. })
            if paths::path_key(Path::new(recorded)) == paths::path_key(root) => {}
        Some(Entry::RunStart { root: recorded, .. }) => {
            return Err(UndoError::Invalid(format!(
                "gehört zur Wurzel {recorded}, nicht zu {}",
                paths::display(root)
            )))
        }
        _ => return Err(UndoError::Invalid("run_start fehlt".into())),
    }
    if entries.iter().any(|e| e.run() != run) {
        return Err(UndoError::Invalid(
            "enthält Einträge eines anderen Laufs".into(),
        ));
    }
    Ok(entries)
}

/// Zustand der letzten Undo-Runde: `None` nie gestartet, `Some(None)` nicht beendet.
fn undo_state(entries: &[Entry]) -> Option<Option<EndStatus>> {
    entries.iter().fold(None, |state, e| match e {
        Entry::UndoStart { .. } => Some(None),
        Entry::UndoEnd { status, .. } => Some(Some(*status)),
        _ => state,
    })
}

/// Gemeinsame Angaben für die Wiederherstellung einer Aktion.
struct Restore<'a> {
    env: &'a UndoEnv<'a>,
    journal: &'a mut JournalWriter,
    run: &'a RunId,
    root_key: &'a str,
}

impl Restore<'_> {
    fn conflict(&mut self, action: u32, reason: &str) -> Result<RestoreStatus, UndoError> {
        self.journal.append(&Entry::UndoConflict {
            run: self.run.clone(),
            action,
            reason: reason.to_string(),
        })?;
        Ok(RestoreStatus::Conflict(reason.to_string()))
    }

    /// Verschobene Datei oder Ordner zurück an den Ursprungsort. `stop` ist der Bereich
    /// (Quarantäne-Lauf oder `_Archiv`), in dem `to` liegen muss; danach werden leer gewordene
    /// Ordner darin bis einschließlich `stop` aufgeräumt.
    fn move_back(
        &mut self,
        action: u32,
        from: &str,
        to: &str,
        stop: &Path,
    ) -> Result<RestoreStatus, UndoError> {
        let allowed_key = paths::path_key(stop);
        let allowed_key = allowed_key.as_str();
        let (from, to) = (Path::new(from), Path::new(to));
        let (from_key, to_key) = (paths::path_key(from), paths::path_key(to));
        let sane = !has_dot_component(&paths::display(from))
            && !has_dot_component(&paths::display(to))
            && from_key != self.root_key
            && paths::is_under(&from_key, self.root_key)
            && to_key != allowed_key
            && paths::is_under(&to_key, allowed_key);
        if !sane {
            return Ok(RestoreStatus::Failed(
                "Journal-Eintrag verweist außerhalb von Wurzel oder Quarantäne".into(),
            ));
        }
        let (in_quarantine, at_origin) = (self.env.fs.exists(to), self.env.fs.exists(from));
        match (in_quarantine, at_origin) {
            (true, false) => {
                if let Some(parent) = from.parent() {
                    if let Err(e) = self.env.fs.create_dir_all(parent) {
                        return Ok(RestoreStatus::Failed(e.to_string()));
                    }
                }
                match self.env.fs.rename(to, from) {
                    Ok(()) => {
                        self.journal.append(&Entry::UndoDone {
                            run: self.run.clone(),
                            action,
                        })?;
                        if let Some(parent) = to.parent() {
                            quarantine::cleanup_empty_parents(self.env.fs, parent, stop);
                        }
                        Ok(RestoreStatus::Restored)
                    }
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        self.conflict(action, "Ursprungspfad ist belegt")
                    }
                    Err(e) => Ok(RestoreStatus::Failed(e.to_string())),
                }
            }
            (true, true) => self.conflict(action, "Ursprungspfad ist belegt"),
            (false, true) => Ok(RestoreStatus::NothingToDo),
            (false, false) => {
                self.conflict(action, "nicht mehr wiederherstellbar")?;
                Ok(RestoreStatus::Missing)
            }
        }
    }

    /// Legt einen entfernten Ordner wieder an. Attribute und Zeiten setzt erst ein zweiter
    /// Durchlauf, weil jedes angelegte Kind die Zeit seines Elternordners ändert.
    fn recreate_dir(&mut self, action: u32, path: &str) -> Result<RestoreStatus, UndoError> {
        let key = paths::path_key(Path::new(path));
        let sane = !has_dot_component(path)
            && key != self.root_key
            && paths::is_under(&key, self.root_key);
        if !sane {
            return Ok(RestoreStatus::Failed(
                "Journal-Eintrag verweist außerhalb der Wurzel".into(),
            ));
        }
        let dir = Path::new(path);
        match self.env.fs.metadata(dir) {
            Ok(meta) if meta.is_link || meta.is_reparse_point() => {
                self.conflict(action, "Pfad ist inzwischen ein Link")
            }
            Ok(meta) if meta.is_dir => Ok(RestoreStatus::NothingToDo),
            Ok(_) => self.conflict(action, "Pfad ist inzwischen eine Datei"),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Some(parent) = dir.parent() {
                    if let Err(e) = self.env.fs.create_dir_all(parent) {
                        return Ok(RestoreStatus::Failed(e.to_string()));
                    }
                }
                match self.env.fs.create_dir(dir) {
                    Ok(()) => Ok(RestoreStatus::Restored),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        Ok(RestoreStatus::NothingToDo)
                    }
                    Err(e) => Ok(RestoreStatus::Failed(e.to_string())),
                }
            }
            Err(e) => Ok(RestoreStatus::Failed(e.to_string())),
        }
    }
}

pub fn undo_run(root: &Path, run: &RunId, env: &UndoEnv) -> Result<UndoOutcome, UndoError> {
    let entries = load(root, run)?;
    let ops = collect_ops(&entries);
    let outcome = |results, already_undone, purged| UndoOutcome {
        run: run.clone(),
        results,
        already_undone,
        purged,
    };

    let purged = entries.iter().any(|e| matches!(e, Entry::Purged { .. }));
    let restorable =
        |o: &Op| o.done && !o.failed && !o.undone && !(purged && o.lives_in_quarantine());
    if purged && !ops.iter().any(restorable) {
        let results = ops
            .iter()
            .filter(|o| o.done && !o.failed && !o.undone)
            .map(|o| RestoreResult {
                action: o.action,
                path: o.origin().to_string(),
                status: RestoreStatus::Missing,
            })
            .collect();
        return Ok(outcome(results, false, true));
    }
    if undo_state(&entries) == Some(Some(EndStatus::Complete)) {
        return Ok(outcome(Vec::new(), true, false));
    }

    let mut journal = JournalWriter::open_append(&quarantine::journal_path(root, run))?;
    journal.append(&Entry::UndoStart {
        run: run.clone(),
        started: env.now.to_string(),
    })?;
    let root_key = paths::path_key(root);
    let quarantine_dir = quarantine::run_dir(root, run);
    let archive_dir = root.join(ARCHIVE_DIR);
    let mut restore = Restore {
        env,
        journal: &mut journal,
        run,
        root_key: &root_key,
    };

    let mut results: Vec<RestoreResult> = Vec::new();
    // Neu angelegte Ordner: Index ihres Ergebnisses; Zeiten und Attribute folgen unten.
    let mut created: Vec<(usize, &Op)> = Vec::new();
    // Rückwärts: zuletzt Veränderes zuerst zurück (bei Ordnern also von oben nach unten).
    for op in ops.iter().rev().filter(|o| !o.failed) {
        let status = if op.undone {
            RestoreStatus::NothingToDo
        } else if purged && op.lives_in_quarantine() {
            RestoreStatus::Missing
        } else {
            match &op.kind {
                OpKind::Move { from, to, dest, .. } => {
                    let stop = match dest {
                        Dest::Quarantine => &quarantine_dir,
                        Dest::Archive => &archive_dir,
                    };
                    restore.move_back(op.action, from, to, stop)?
                }
                OpKind::RemoveDir { path, .. } => {
                    let status = restore.recreate_dir(op.action, path)?;
                    if status == RestoreStatus::Restored {
                        created.push((results.len(), op));
                    }
                    status
                }
            }
        };
        results.push(RestoreResult {
            action: op.action,
            path: op.origin().to_string(),
            status,
        });
    }
    // Von unten nach oben: Ein Kind ändert die Zeit seines Elternordners nur beim Anlegen,
    // nicht beim Setzen seiner eigenen Attribute.
    for (index, op) in created.iter().rev() {
        let OpKind::RemoveDir {
            path,
            attrs,
            mtime_ticks,
            ctime_ticks,
        } = &op.kind
        else {
            continue;
        };
        match env
            .fs
            .set_dir_meta(Path::new(path), *attrs, *mtime_ticks, *ctime_ticks)
        {
            Ok(()) => journal.append(&Entry::UndoDone {
                run: run.clone(),
                action: op.action,
            })?,
            Err(e) => {
                results[*index].status = RestoreStatus::Failed(format!(
                    "Ordner angelegt, Attribute und Zeiten nicht wiederhergestellt: {e}"
                ))
            }
        }
    }
    let result = outcome(results, false, false);
    journal.append(&Entry::UndoEnd {
        run: run.clone(),
        status: if result.exit_code() == 0 {
            EndStatus::Complete
        } else {
            EndStatus::Partial
        },
    })?;
    Ok(result)
}

fn summarize(run: RunId, entries: &[Entry], quarantine_days: u32) -> RunSummary {
    let ops = collect_ops(entries);
    let started = entries.iter().find_map(|e| match e {
        Entry::RunStart { started, .. } => Some(started.clone()),
        _ => None,
    });
    let purged = entries.iter().any(|e| matches!(e, Entry::Purged { .. }));
    let ended = entries.iter().rev().find_map(|e| match e {
        Entry::RunEnd { status, .. } => Some(*status),
        _ => None,
    });
    let status = if purged {
        RunStatus::Purged
    } else {
        match (undo_state(entries), ended) {
            (Some(Some(EndStatus::Complete)), _) => RunStatus::Undone,
            (Some(_), _) => RunStatus::PartiallyUndone,
            (None, Some(EndStatus::Complete)) => RunStatus::Complete,
            (None, Some(EndStatus::Partial)) => RunStatus::Partial,
            (None, None) => RunStatus::Incomplete,
        }
    };
    RunSummary {
        run,
        moved: ops.iter().filter(|o| o.done && !o.failed).count(),
        bytes: if purged {
            0
        } else {
            ops.iter()
                .filter(|o| o.done && !o.failed && !o.undone)
                .filter_map(|o| match o.kind {
                    OpKind::Move {
                        size,
                        dest: Dest::Quarantine,
                        ..
                    } => Some(size),
                    _ => None,
                })
                .sum()
        },
        expires: started
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc) + chrono::Duration::days(i64::from(quarantine_days))),
        started,
        status,
    }
}

pub fn list_runs(root: &Path, quarantine_days: u32) -> Result<Vec<RunSummary>, UndoError> {
    let dir = paths::extended(&quarantine::journal_dir(root));
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut runs = Vec::new();
    for entry in listing.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(run) = name
            .strip_suffix(".jsonl")
            .and_then(|stem| RunId::parse(stem).ok())
        else {
            continue;
        };
        runs.push(match journal::read(&entry.path()) {
            Ok(entries) => summarize(run, &entries, quarantine_days),
            Err(_) => RunSummary {
                run,
                started: None,
                moved: 0,
                bytes: 0,
                status: RunStatus::Unreadable,
                expires: None,
            },
        });
    }
    runs.sort_by(|a, b| a.run.cmp(&b.run));
    Ok(runs)
}

/// Läufe, deren Aufbewahrungsfrist abgelaufen ist und die noch nicht gelöscht wurden.
pub fn expired_runs(
    root: &Path,
    quarantine_days: u32,
    now: DateTime<Utc>,
) -> Result<Vec<RunSummary>, UndoError> {
    Ok(list_runs(root, quarantine_days)?
        .into_iter()
        .filter(|r| r.status != RunStatus::Purged)
        .filter(|r| r.expires.is_some_and(|expires| expires <= now))
        .collect())
}

/// Löscht die Quarantäne eines Laufs endgültig (einzige harte Löschung). Das Journal bleibt.
pub fn purge_run(root: &Path, run: &RunId, env: &UndoEnv) -> Result<(), UndoError> {
    let entries = load(root, run)?;
    let dir = quarantine::run_dir(root, run);
    match env.fs.metadata(&dir) {
        Ok(meta) if meta.is_reparse_point() || !meta.is_dir => {
            return Err(UndoError::Invalid(format!(
                "{} ist ein Link oder kein Ordner; nichts gelöscht",
                paths::display(&dir)
            )))
        }
        Ok(_) => env.fs.remove_dir_all(&dir)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    // Erst löschen, dann vermerken: Ein Absturz dazwischen lässt höchstens den Vermerk fehlen.
    if !entries.iter().any(|e| matches!(e, Entry::Purged { .. })) {
        JournalWriter::open_append(&quarantine::journal_path(root, run))?.append(
            &Entry::Purged {
                run: run.clone(),
                at: env.now.to_string(),
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::fsops::testing::FaultyFs;
    use crate::change::fsops::RealFs;
    use crate::change::journal;
    use crate::change::test_support::{fx, run_with, Fx, RUN};

    const NOW: &str = "2026-10-03T11:00:00Z";

    fn env() -> UndoEnv<'static> {
        UndoEnv {
            fs: &RealFs,
            now: NOW,
        }
    }

    fn run_id() -> RunId {
        RunId::parse(RUN).unwrap()
    }

    fn undo(fx: &Fx) -> UndoOutcome {
        undo_run(&fx.root, &run_id(), &env()).unwrap()
    }

    /// Wurzel mit zwei Duplikaten in Unterordnern; Apply ist gelaufen.
    fn applied() -> Fx {
        let fx = fx();
        fx.write("a/orig.txt", "gleicher inhalt");
        fx.write("b/kopie.txt", "gleicher inhalt");
        fx.write("c/sub/kopie2.txt", "gleicher inhalt");
        let plan = fx.plan(&[
            ("b/kopie.txt", "a/orig.txt"),
            ("c/sub/kopie2.txt", "a/orig.txt"),
        ]);
        let out = run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        assert_eq!(out.executed(), 2);
        fx
    }

    fn mtime(fx: &Fx, rel: &str) -> i64 {
        RealFs.metadata(&fx.root.join(rel)).unwrap().mtime_ticks
    }

    const KNOWN_MTIME: i64 = 1_600_000_000 * crate::scan::source::TICKS_PER_SEC;
    const KNOWN_CTIME: i64 = 1_500_000_000 * crate::scan::source::TICKS_PER_SEC;

    /// Wurzel mit `a\b\c` (b versteckt, alle mit bekannten Zeiten) und `d`; `remove-dir`-Apply
    /// ist gelaufen.
    fn applied_dirs() -> Fx {
        let fx = fx();
        for rel in ["a/b/c", "d"] {
            fx.mkdir(rel);
        }
        // Erst alle Ordner anlegen (ändert die mtime der Eltern), dann Zeiten von unten nach oben.
        for rel in ["a/b/c", "a/b", "a", "d"] {
            let attrs = if rel == "a/b" { 0x10 | 0x2 } else { 0x10 };
            RealFs
                .set_dir_meta(&fx.root.join(rel), attrs, KNOWN_MTIME, KNOWN_CTIME)
                .unwrap();
        }
        let plan = fx.dir_plan(&["a/b/c", "a/b", "a", "d"]);
        let out = run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        assert_eq!(out.executed(), 4);
        assert!(!fx.exists("a") && !fx.exists("d"));
        fx
    }

    /// `(Pfad, attrs, mtime, ctime)` aller `intent_remove_dir`-Einträge.
    fn journaled_dirs(fx: &Fx) -> Vec<(String, u32, i64, i64)> {
        fx.journal(RUN)
            .into_iter()
            .filter_map(|e| match e {
                Entry::IntentRemoveDir {
                    path,
                    attrs,
                    mtime_ticks,
                    ctime_ticks,
                    ..
                } => Some((path, attrs, mtime_ticks, ctime_ticks)),
                _ => None,
            })
            .collect()
    }

    /// Schreibt das Journal ohne `done`-Einträge und ohne `run_end` neu (Absturz nach dem
    /// Entfernen, aber vor dem Vermerk).
    fn drop_done_entries(fx: &Fx) {
        let path = quarantine::journal_path(&fx.root, &run_id());
        let kept: Vec<String> = fx
            .journal(RUN)
            .iter()
            .filter(|e| !matches!(e, Entry::Done { .. } | Entry::RunEnd { .. }))
            .map(|e| serde_json::to_string(e).unwrap())
            .collect();
        std::fs::write(path, kept.join("\n") + "\n").unwrap();
    }

    #[test]
    fn undo_legt_ordner_von_oben_nach_unten_mit_attributen_und_zeiten_wieder_an() {
        let fx = applied_dirs();
        let journaled = journaled_dirs(&fx);
        assert_eq!(journaled.len(), 4);

        let out = undo(&fx);

        assert_eq!((out.restored(), out.exit_code()), (4, 0));
        for rel in ["a", "a/b", "a/b/c", "d"] {
            assert!(fx.root.join(rel).is_dir(), "{rel}");
        }
        for (path, attrs, mtime, ctime) in &journaled {
            let meta = RealFs.metadata(Path::new(path)).unwrap();
            assert_eq!(meta.mtime_ticks, *mtime, "mtime von {path}");
            assert_eq!(meta.ctime_ticks, *ctime, "ctime von {path}");
            assert_eq!(
                meta.attrs.is_hidden(),
                attrs & 0x2 != 0,
                "versteckt-Attribut von {path}"
            );
        }
        assert!(RealFs
            .metadata(&fx.root.join("a/b"))
            .unwrap()
            .attrs
            .is_hidden());
        // Auch der Elternordner trägt wieder die ursprüngliche Zeit, nicht die vom Entfernen
        // seiner Kinder veränderte.
        for rel in ["a", "a/b", "a/b/c", "d"] {
            assert_eq!(
                mtime(&fx, rel),
                KNOWN_MTIME,
                "ursprüngliche mtime von {rel}"
            );
        }

        let entries = fx.journal(RUN);
        assert_eq!(
            entries
                .iter()
                .filter(|e| matches!(e, Entry::UndoDone { .. }))
                .count(),
            4
        );
        assert!(matches!(
            entries.last(),
            Some(Entry::UndoEnd {
                status: EndStatus::Complete,
                ..
            })
        ));
    }

    #[test]
    fn zweites_undo_nach_remove_dir_aendert_nichts() {
        let fx = applied_dirs();
        undo(&fx);
        let journal_len = fx.journal(RUN).len();

        let again = undo(&fx);

        assert!(again.already_undone && again.results.is_empty());
        assert_eq!(fx.journal(RUN).len(), journal_len);
    }

    #[test]
    fn datei_an_stelle_des_ordners_ist_ein_konflikt_und_bleibt_unberuehrt() {
        let fx = applied_dirs();
        fx.write("d", "ich bin jetzt eine datei");

        let out = undo(&fx);

        assert_eq!(
            (out.restored(), out.conflicts(), out.exit_code()),
            (3, 1, 2)
        );
        assert_eq!(fx.read("d"), "ich bin jetzt eine datei");
        assert!(fx.root.join("a/b/c").is_dir());
        assert!(fx
            .journal(RUN)
            .iter()
            .any(|e| matches!(e, Entry::UndoConflict { .. })));
    }

    #[test]
    fn vorhandener_ordner_wird_nicht_angefasst() {
        let fx = applied_dirs();
        let d = fx.mkdir("d");
        let before = RealFs.metadata(&d).unwrap();

        let out = undo(&fx);

        assert_eq!((out.restored(), out.exit_code()), (3, 0));
        let after = RealFs.metadata(&d).unwrap();
        assert_eq!(
            (before.mtime_ticks, before.ctime_ticks, before.attrs),
            (after.mtime_ticks, after.ctime_ticks, after.attrs),
            "vorhandenen Ordner nicht überschreiben"
        );
        assert!(out
            .results
            .iter()
            .any(|r| r.status == RestoreStatus::NothingToDo));
    }

    #[test]
    fn absturz_nach_dem_entfernen_ohne_done_stellt_den_ordner_trotzdem_wieder_her() {
        let fx = applied_dirs();
        drop_done_entries(&fx);

        let out = undo(&fx);

        assert_eq!((out.restored(), out.exit_code()), (4, 0));
        assert!(fx.root.join("a/b/c").is_dir() && fx.root.join("d").is_dir());
    }

    #[test]
    fn absturz_vor_dem_entfernen_intent_ohne_done_und_ordner_noch_da_ist_nichts_zu_tun() {
        let fx = fx();
        fx.mkdir("d");
        let plan = fx.dir_plan(&["d"]);
        let faulty = FaultyFs::new().fail(
            crate::change::fsops::testing::Op::RemoveDir,
            &fx.root.join("d"),
        );
        run_with(&fx, &plan, &faulty, &fx.protector(), RUN).unwrap();
        // Ein Fail-Eintrag würde den Move ausblenden; für den Absturz-Fall entfernen wir ihn.
        let path = quarantine::journal_path(&fx.root, &run_id());
        let kept: Vec<String> = fx
            .journal(RUN)
            .iter()
            .filter(|e| !matches!(e, Entry::Fail { .. } | Entry::RunEnd { .. }))
            .map(|e| serde_json::to_string(e).unwrap())
            .collect();
        std::fs::write(path, kept.join("\n") + "\n").unwrap();
        let before = RealFs.metadata(&fx.root.join("d")).unwrap();

        let out = undo(&fx);

        assert_eq!(out.restored(), 0);
        assert_eq!(out.exit_code(), 0);
        assert_eq!(out.results[0].status, RestoreStatus::NothingToDo);
        assert_eq!(
            RealFs.metadata(&fx.root.join("d")).unwrap().mtime_ticks,
            before.mtime_ticks
        );
    }

    #[test]
    fn fehlgeschlagene_und_uebersprungene_aktionen_werden_nicht_zurueckgedreht() {
        let fx = fx();
        let a = fx.mkdir("a");
        fx.mkdir("b");
        let plan = fx.dir_plan(&["a", "b"]);
        let faulty = FaultyFs::new().fail(crate::change::fsops::testing::Op::RemoveDirNotEmpty, &a);
        run_with(&fx, &plan, &faulty, &fx.protector(), RUN).unwrap();
        assert!(fx.exists("a") && !fx.exists("b"));

        let out = undo(&fx);

        assert_eq!(out.results.len(), 1, "nur b wurde wirklich entfernt");
        assert_eq!((out.restored(), out.exit_code()), (1, 0));
        assert!(fx.root.join("b").is_dir());
    }

    #[test]
    fn manipuliertes_journal_kann_keine_ordner_ausserhalb_der_wurzel_anlegen() {
        let fx = fx();
        let outside = fx.root.parent().unwrap().join("ausserhalb");
        let mut w = JournalWriter::create(&quarantine::journal_path(&fx.root, &run_id())).unwrap();
        let bad_paths = [
            paths::display(&outside),
            paths::display(&fx.root),
            format!(r"{}\..\ausserhalb2", paths::display(&fx.root)),
        ];
        w.append(&Entry::RunStart {
            run: run_id(),
            plan: "p".into(),
            root: paths::display(&fx.root),
            started: "2026-10-03T10:00:00Z".into(),
        })
        .unwrap();
        for (action, path) in (1u32..).zip(&bad_paths) {
            w.append(&Entry::IntentRemoveDir {
                run: run_id(),
                action,
                path: path.clone(),
                attrs: 0x10,
                mtime_ticks: 0,
                ctime_ticks: 0,
            })
            .unwrap();
            w.append(&Entry::Done {
                run: run_id(),
                action,
            })
            .unwrap();
        }

        let out = undo(&fx);

        assert_eq!(out.failed(), 3, "{:?}", out.results);
        assert!(!outside.exists());
        assert!(!fx.root.parent().unwrap().join("ausserhalb2").exists());
    }

    #[test]
    fn purge_beruehrt_remove_dir_laeufe_nicht_undo_geht_weiter() {
        let fx = applied_dirs();
        purge_run(&fx.root, &run_id(), &env()).unwrap();

        let out = undo(&fx);

        assert!(!out.purged);
        assert_eq!((out.restored(), out.missing(), out.exit_code()), (4, 0, 0));
        assert!(fx.root.join("a/b/c").is_dir());
    }

    #[test]
    fn runs_zaehlt_entfernte_ordner_ohne_quarantaene_bytes() {
        let fx = applied_dirs();
        let runs = list_runs(&fx.root, 30).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].moved, 4);
        assert_eq!(runs[0].bytes, 0);
        assert_eq!(runs[0].status, RunStatus::Complete);

        undo(&fx);
        assert_eq!(
            list_runs(&fx.root, 30).unwrap()[0].status,
            RunStatus::Undone
        );
    }

    /// Wurzel mit `Projekt` (verschachtelt); der Archiv-Apply ist gelaufen.
    fn archived() -> (Fx, String) {
        let fx = fx();
        fx.write("Projekt/a.txt", "alpha");
        fx.write("Projekt/sub/b.txt", "bravo bravo");
        fx.write("Projekt/sub/tief/c.txt", "charlie");
        let plan = fx.archive_plan(&["Projekt"]);
        let target = plan.actions[0].target.clone().unwrap();
        let out = run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        assert_eq!(out.executed(), 1);
        assert!(!fx.exists("Projekt"));
        (fx, target)
    }

    #[test]
    fn undo_stellt_archivierten_ordner_her_und_raeumt_das_archiv_auf() {
        let fx = fx();
        fx.write("Projekt/a.txt", "alpha");
        fx.write("Projekt/sub/b.txt", "bravo bravo");
        let m_a = mtime(&fx, "Projekt/a.txt");
        let m_b = mtime(&fx, "Projekt/sub/b.txt");
        let plan = fx.archive_plan(&["Projekt"]);
        run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        assert!(fx.exists("_Archiv"));

        let out = undo(&fx);

        assert_eq!((out.restored(), out.exit_code()), (1, 0));
        assert_eq!(fx.read("Projekt/a.txt"), "alpha");
        assert_eq!(fx.read("Projekt/sub/b.txt"), "bravo bravo");
        assert_eq!(
            (mtime(&fx, "Projekt/a.txt"), mtime(&fx, "Projekt/sub/b.txt")),
            (m_a, m_b)
        );
        assert!(
            !fx.exists("_Archiv"),
            "Jahresordner und leeres _Archiv werden aufgeräumt"
        );
        let entries = fx.journal(RUN);
        assert_eq!(
            entries
                .iter()
                .filter(|e| matches!(e, Entry::UndoDone { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn undo_laesst_ein_archiv_mit_anderem_inhalt_stehen() {
        let (fx, target) = archived();
        fx.write("_Archiv/2010/Anderes/x.txt", "fremd");

        let out = undo(&fx);

        assert_eq!(out.exit_code(), 0);
        assert!(fx.exists("Projekt/a.txt"));
        assert!(
            !std::path::Path::new(&target).exists()
                && !std::path::Path::new(&target).parent().unwrap().exists(),
            "der eigene Jahresordner ist weg"
        );
        assert_eq!(fx.read("_Archiv/2010/Anderes/x.txt"), "fremd");
    }

    #[test]
    fn belegter_ursprungspfad_ist_ein_konflikt_und_das_archiv_bleibt_unberuehrt() {
        let (fx, target) = archived();
        fx.write("Projekt/neu.txt", "inzwischen angelegt");

        let out = undo(&fx);

        assert_eq!(
            (out.restored(), out.conflicts(), out.exit_code()),
            (0, 1, 2)
        );
        assert_eq!(fx.read("Projekt/neu.txt"), "inzwischen angelegt");
        assert!(std::path::Path::new(&target).join("a.txt").exists());

        // Nach Auflösen der Kollision lässt sich Undo wiederholen.
        std::fs::remove_dir_all(fx.root.join("Projekt")).unwrap();
        let retry = undo(&fx);
        assert_eq!((retry.restored(), retry.exit_code()), (1, 0));
        assert!(fx.exists("Projekt/sub/tief/c.txt"));
    }

    #[test]
    fn absturz_nach_dem_rename_ohne_done_wird_anhand_des_zustands_zurueckgedreht() {
        let (fx, _) = archived();
        drop_done_entries(&fx);

        let out = undo(&fx);

        assert_eq!((out.restored(), out.exit_code()), (1, 0));
        assert!(fx.exists("Projekt/a.txt"));
    }

    #[test]
    fn absturz_vor_dem_rename_intent_ohne_done_und_ordner_noch_am_ursprung_ist_nichts_zu_tun() {
        let fx = fx();
        fx.write("Projekt/a.txt", "alpha");
        let plan = fx.archive_plan(&["Projekt"]);
        let faulty = FaultyFs::new().crash_before_rename(1);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_with(&fx, &plan, &faulty, &fx.protector(), RUN)
        }));
        assert!(result.is_err(), "simulierter Absturz");
        assert!(fx.exists("Projekt/a.txt"));

        let out = undo(&fx);

        assert_eq!(out.results[0].status, RestoreStatus::NothingToDo);
        assert_eq!(out.exit_code(), 0);
        assert!(fx.exists("Projekt/a.txt"));
    }

    #[test]
    fn purge_macht_archiv_moves_nicht_unwiederbringlich() {
        let (fx, _) = archived();
        purge_run(&fx.root, &run_id(), &env()).unwrap();

        let out = undo(&fx);

        assert!(!out.purged);
        assert_eq!((out.restored(), out.missing(), out.exit_code()), (1, 0, 0));
        assert!(fx.exists("Projekt/sub/tief/c.txt"));
    }

    #[test]
    fn purge_trifft_nur_quarantaene_aktionen_eines_gemischten_laufs() {
        // Gemischter Lauf (nur über ein handgebautes Journal möglich): ein Quarantäne-Move
        // und ein Archiv-Move; nach `purge` fehlt nur der erste.
        let fx = fx();
        fx.write("lose.txt", "quarantäne");
        fx.write("Projekt/a.txt", "archiv");
        let junk = fx.junk_plan(&["lose.txt"]);
        let archive = fx.archive_plan(&["Projekt"]);
        let mut plan = junk;
        plan.kind = crate::change::plan::PlanKind::Junk;
        let mut moved = archive.actions[0].clone();
        moved.id = 2;
        plan.actions.push(moved);
        // Das Plan-Format erlaubt gemischte Aktionstypen; `apply` führt beide aus.
        let out = run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        assert_eq!(out.executed(), 2);
        purge_run(&fx.root, &run_id(), &env()).unwrap();

        let result = undo(&fx);

        assert!(!result.purged, "ein Teil ist noch wiederherstellbar");
        assert_eq!((result.restored(), result.missing()), (1, 1));
        assert!(fx.exists("Projekt/a.txt") && !fx.exists("lose.txt"));
    }

    #[test]
    fn manipuliertes_journal_kann_kein_archiv_ziel_ausserhalb_von_archiv_zurueckholen() {
        let fx = fx();
        let victim = fx.write("opfer.txt", "x");
        let elsewhere = fx.write("woanders.txt", "y");
        let in_quarantine = fx.quarantined(RUN, "q.txt");
        std::fs::create_dir_all(in_quarantine.parent().unwrap()).unwrap();
        std::fs::write(&in_quarantine, "q").unwrap();
        let mut w = JournalWriter::create(&quarantine::journal_path(&fx.root, &run_id())).unwrap();
        w.append(&Entry::RunStart {
            run: run_id(),
            plan: "p".into(),
            root: paths::display(&fx.root),
            started: "2026-10-03T10:00:00Z".into(),
        })
        .unwrap();
        // 1: dest archive, aber `to` liegt irgendwo in der Wurzel
        // 2: dest archive, aber `to` liegt in der Quarantäne
        // 3: dest quarantine, aber `to` liegt unter _Archiv
        let cases = [
            (
                paths::display(&elsewhere),
                paths::display(&victim),
                Dest::Archive,
            ),
            (
                paths::display(&fx.root.join("ziel2.txt")),
                paths::display(&in_quarantine),
                Dest::Archive,
            ),
            (
                paths::display(&fx.root.join("ziel3.txt")),
                paths::display(&fx.root.join("_Archiv").join("x.txt")),
                Dest::Quarantine,
            ),
        ];
        for ((from, to, dest), action) in cases.into_iter().zip(1u32..) {
            w.append(&Entry::Intent {
                run: run_id(),
                action,
                from,
                to,
                size: 1,
                hash: None,
                dest,
                is_dir: false,
            })
            .unwrap();
            w.append(&Entry::Done {
                run: run_id(),
                action,
            })
            .unwrap();
        }

        let out = undo(&fx);

        assert_eq!(out.failed(), 3, "{:?}", out.results);
        assert_eq!(fx.read("opfer.txt"), "x");
        assert_eq!(fx.read("woanders.txt"), "y");
        assert!(in_quarantine.exists());
    }

    #[test]
    fn archivierte_einzeldatei_wird_zurueckgeholt_und_der_ordner_aufgeraeumt() {
        let fx = fx();
        fx.write("Bericht_v1.docx", "eins");
        fx.write("Bericht_v2.docx", "zwei");
        let m = mtime(&fx, "Bericht_v1.docx");
        let plan = fx.move_file_plan(&[("Bericht_v1.docx", r"Versionen\Bericht_v1.docx")]);
        run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        assert!(!fx.exists("Bericht_v1.docx"));

        let out = undo(&fx);

        assert_eq!((out.restored(), out.exit_code()), (1, 0));
        assert_eq!(fx.read("Bericht_v1.docx"), "eins");
        assert_eq!(mtime(&fx, "Bericht_v1.docx"), m);
        assert!(!fx.exists("_Archiv"));
    }

    #[test]
    fn runs_zaehlt_archiv_moves_ohne_quarantaene_bytes() {
        let (fx, _) = archived();
        let runs = list_runs(&fx.root, 30).unwrap();
        assert_eq!((runs[0].moved, runs[0].bytes), (1, 0));
        assert_eq!(runs[0].status, RunStatus::Complete);
        undo(&fx);
        assert_eq!(
            list_runs(&fx.root, 30).unwrap()[0].status,
            RunStatus::Undone
        );
    }

    #[test]
    fn apply_und_undo_stellen_pfad_inhalt_und_zeit_wieder_her() {
        let fx = fx();
        fx.write("a/orig.txt", "gleicher inhalt");
        fx.write("b/kopie.txt", "gleicher inhalt");
        fx.write("c/sub/kopie2.txt", "gleicher inhalt");
        let (m1, m2) = (mtime(&fx, "b/kopie.txt"), mtime(&fx, "c/sub/kopie2.txt"));
        let plan = fx.plan(&[
            ("b/kopie.txt", "a/orig.txt"),
            ("c/sub/kopie2.txt", "a/orig.txt"),
        ]);
        run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        assert!(!fx.exists("b/kopie.txt"));

        let out = undo(&fx);

        assert_eq!((out.restored(), out.exit_code()), (2, 0));
        assert_eq!(fx.read("b/kopie.txt"), "gleicher inhalt");
        assert_eq!(fx.read("c/sub/kopie2.txt"), "gleicher inhalt");
        assert_eq!(
            (mtime(&fx, "b/kopie.txt"), mtime(&fx, "c/sub/kopie2.txt")),
            (m1, m2)
        );
        assert!(!fx.quarantined(RUN, r"b\kopie.txt").exists());
        assert_eq!(fx.read("a/orig.txt"), "gleicher inhalt");

        let entries = fx.journal(RUN);
        assert!(entries.iter().any(|e| matches!(e, Entry::UndoStart { .. })));
        assert_eq!(
            entries
                .iter()
                .filter(|e| matches!(e, Entry::UndoDone { .. }))
                .count(),
            2
        );
        assert!(matches!(
            entries.last(),
            Some(Entry::UndoEnd {
                status: EndStatus::Complete,
                ..
            })
        ));
    }

    #[test]
    fn undo_raeumt_leere_ordner_in_der_quarantaene_auf() {
        let fx = applied();
        assert!(quarantine::run_dir(&fx.root, &run_id()).exists());

        let out = undo(&fx);

        assert_eq!(out.exit_code(), 0);
        assert!(
            !quarantine::run_dir(&fx.root, &run_id()).exists(),
            "keine leeren Ordner unter quarantine\\<run-id>, auch der Laufordner nicht"
        );
        assert!(quarantine::journal_path(&fx.root, &run_id()).exists());
    }

    #[test]
    fn undo_laesst_ordner_mit_verbleibenden_dateien_stehen_und_raeumt_den_rest() {
        let fx = applied();
        fx.write("b/kopie.txt", "belegt");

        undo(&fx);

        assert!(
            fx.quarantined(RUN, r"b\kopie.txt").exists(),
            "Konflikt: bleibt"
        );
        assert!(
            !fx.quarantined(RUN, "c").exists(),
            "der erfolgreich zurückgeholte Zweig ist weg"
        );
    }

    #[test]
    fn zweites_undo_meldet_bereits_zurueckgedreht_und_aendert_nichts() {
        let fx = applied();
        undo(&fx);
        let journal_len = fx.journal(RUN).len();

        let again = undo(&fx);

        assert!(again.already_undone && again.results.is_empty());
        assert_eq!(again.exit_code(), 0);
        assert_eq!(fx.journal(RUN).len(), journal_len);
    }

    #[test]
    fn belegter_ursprungspfad_wird_nie_ueberschrieben() {
        let fx = applied();
        fx.write("b/kopie.txt", "neue datei, andere");

        let out = undo(&fx);

        assert_eq!(
            (out.restored(), out.conflicts(), out.exit_code()),
            (1, 1, 2)
        );
        assert_eq!(fx.read("b/kopie.txt"), "neue datei, andere");
        assert!(fx.quarantined(RUN, r"b\kopie.txt").exists());
        assert_eq!(fx.read("c/sub/kopie2.txt"), "gleicher inhalt");
        assert!(fx
            .journal(RUN)
            .iter()
            .any(|e| matches!(e, Entry::UndoConflict { .. })));
        assert!(matches!(
            fx.journal(RUN).last(),
            Some(Entry::UndoEnd {
                status: EndStatus::Partial,
                ..
            })
        ));

        // Nach Auflösen der Kollision kann Undo wiederholt werden.
        std::fs::remove_file(fx.root.join("b/kopie.txt")).unwrap();
        let retry = undo(&fx);
        assert!(!retry.already_undone);
        assert_eq!(retry.restored(), 1);
        assert_eq!(retry.exit_code(), 0);
        assert_eq!(fx.read("b/kopie.txt"), "gleicher inhalt");
    }

    #[test]
    fn fehlende_quarantaene_datei_wird_gemeldet() {
        let fx = applied();
        std::fs::remove_file(fx.quarantined(RUN, r"b\kopie.txt")).unwrap();

        let out = undo(&fx);

        assert_eq!((out.restored(), out.missing(), out.exit_code()), (1, 1, 2));
    }

    #[test]
    fn undo_legt_fehlende_ursprungsordner_wieder_an() {
        let fx = applied();
        std::fs::remove_dir_all(fx.root.join("c")).unwrap();

        let out = undo(&fx);

        assert_eq!(out.restored(), 2);
        assert_eq!(fx.read("c/sub/kopie2.txt"), "gleicher inhalt");
    }

    #[test]
    fn absturz_vor_dem_zweiten_move_stellt_nur_den_ersten_wieder_her() {
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

        let out = undo(&fx);

        assert_eq!((out.restored(), out.exit_code()), (1, 0));
        assert!(out
            .results
            .iter()
            .any(|r| r.status == RestoreStatus::NothingToDo));
        assert_eq!(fx.read("b.txt"), "x");
        assert_eq!(fx.read("c.txt"), "x");
    }

    #[test]
    fn absturz_nach_dem_move_wird_anhand_des_dateizustands_zurueckgedreht() {
        let fx = fx();
        fx.write("a.txt", "x");
        fx.write("b.txt", "x");
        let plan = fx.plan(&[("b.txt", "a.txt")]);
        let fs = FaultyFs::new().crash_after_rename(1);
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_with(&fx, &plan, &fs, &fx.protector(), RUN)
        }));
        assert!(crashed.is_err());
        assert!(!fx.exists("b.txt"));

        let out = undo(&fx);

        assert_eq!(out.restored(), 1);
        assert_eq!(fx.read("b.txt"), "x");
    }

    #[test]
    fn unbekannter_lauf_ist_ein_klarer_fehler() {
        let fx = fx();
        assert!(matches!(
            undo_run(&fx.root, &run_id(), &env()),
            Err(UndoError::NotFound(_))
        ));
    }

    #[test]
    fn manipuliertes_journal_kann_keine_fremden_dateien_verschieben() {
        let fx = fx();
        let victim = fx.write("opfer.txt", "x");
        let elsewhere = fx.write("woanders.txt", "y");
        let mut w = JournalWriter::create(&quarantine::journal_path(&fx.root, &run_id())).unwrap();
        for e in [
            Entry::RunStart {
                run: run_id(),
                plan: "p".into(),
                root: paths::display(&fx.root),
                started: "2026-10-03T10:00:00Z".into(),
            },
            Entry::Intent {
                run: run_id(),
                action: 1,
                from: paths::display(&elsewhere),
                to: paths::display(&victim),
                size: 1,
                hash: Some("00".repeat(16)),
                dest: crate::change::journal::Dest::Quarantine,
                is_dir: false,
            },
            Entry::Done {
                run: run_id(),
                action: 1,
            },
        ] {
            w.append(&e).unwrap();
        }

        let out = undo(&fx);

        assert_eq!(out.failed(), 1);
        assert_eq!(fx.read("opfer.txt"), "x");
        assert_eq!(fx.read("woanders.txt"), "y");
    }

    #[test]
    fn journal_einer_anderen_wurzel_wird_abgelehnt() {
        let fx = fx();
        let mut w = JournalWriter::create(&quarantine::journal_path(&fx.root, &run_id())).unwrap();
        w.append(&Entry::RunStart {
            run: run_id(),
            plan: "p".into(),
            root: r"D:\Andere".into(),
            started: "t".into(),
        })
        .unwrap();
        assert!(matches!(
            undo_run(&fx.root, &run_id(), &env()),
            Err(UndoError::Invalid(_))
        ));
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn runs_zeigt_status_groesse_und_ablaufdatum() {
        let fx = applied();
        let runs = list_runs(&fx.root, 30).unwrap();
        assert_eq!(runs.len(), 1);
        let r = &runs[0];
        assert_eq!(
            (r.run.clone(), r.status, r.moved),
            (run_id(), RunStatus::Complete, 2)
        );
        assert_eq!(r.bytes, 30);
        assert_eq!(r.expires, Some(at("2026-11-02T10:00:00Z")));

        undo(&fx);
        let r = &list_runs(&fx.root, 30).unwrap()[0];
        assert_eq!((r.status, r.bytes), (RunStatus::Undone, 0));
    }

    #[test]
    fn runs_erkennt_teilweise_unvollstaendige_und_teilweise_zurueckgedrehte_laeufe() {
        // unvollständig: Absturz
        let crashed = fx();
        crashed.write("a.txt", "x");
        crashed.write("b.txt", "x");
        let plan = crashed.plan(&[("b.txt", "a.txt")]);
        let fs = FaultyFs::new().crash_after_rename(1);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_with(&crashed, &plan, &fs, &crashed.protector(), RUN)
        }));
        assert_eq!(
            list_runs(&crashed.root, 30).unwrap()[0].status,
            RunStatus::Incomplete
        );

        // teilweise: eine Aktion stale
        let partial = fx();
        partial.write("a.txt", "x");
        partial.write("b.txt", "x");
        let plan = partial.plan(&[("b.txt", "a.txt")]);
        partial.write("b.txt", "länger");
        run_with(&partial, &plan, &RealFs, &partial.protector(), RUN).unwrap();
        assert_eq!(
            list_runs(&partial.root, 30).unwrap()[0].status,
            RunStatus::Partial
        );

        // teilweise zurückgedreht: Kollision
        let conflict = applied();
        conflict.write("b/kopie.txt", "neu");
        undo(&conflict);
        assert_eq!(
            list_runs(&conflict.root, 30).unwrap()[0].status,
            RunStatus::PartiallyUndone
        );
    }

    #[test]
    fn runs_ohne_journal_ordner_ist_leer() {
        assert!(list_runs(&fx().root, 30).unwrap().is_empty());
    }

    #[test]
    fn beschaedigtes_journal_wird_als_unlesbar_gelistet() {
        let fx = fx();
        let path = quarantine::journal_path(&fx.root, &run_id());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "kaputt\nnoch kaputter\n").unwrap();
        assert_eq!(
            list_runs(&fx.root, 30).unwrap()[0].status,
            RunStatus::Unreadable
        );
    }

    #[test]
    fn abgelaufene_laeufe_werden_nach_frist_erkannt() {
        let fx = applied();
        let within = expired_runs(&fx.root, 30, at("2026-11-01T10:00:00Z")).unwrap();
        assert!(within.is_empty());
        let after = expired_runs(&fx.root, 30, at("2026-11-02T10:00:00Z")).unwrap();
        assert_eq!(after.len(), 1);
        let custom = expired_runs(&fx.root, 1, at("2026-10-05T00:00:00Z")).unwrap();
        assert_eq!(custom.len(), 1);
    }

    #[test]
    fn purge_loescht_nur_die_quarantaene_und_laesst_das_journal() {
        let fx = applied();
        purge_run(&fx.root, &run_id(), &env()).unwrap();

        assert!(!quarantine::run_dir(&fx.root, &run_id()).exists());
        assert_eq!(fx.read("a/orig.txt"), "gleicher inhalt");
        assert!(quarantine::journal_path(&fx.root, &run_id()).exists());
        assert!(fx
            .journal(RUN)
            .iter()
            .any(|e| matches!(e, Entry::Purged { .. })));
        let r = &list_runs(&fx.root, 30).unwrap()[0];
        assert_eq!((r.status, r.bytes), (RunStatus::Purged, 0));
        assert!(expired_runs(&fx.root, 30, at("2030-01-01T00:00:00Z"))
            .unwrap()
            .is_empty());
        // Wiederholung ist harmlos.
        purge_run(&fx.root, &run_id(), &env()).unwrap();
    }

    #[test]
    fn undo_nach_purge_meldet_nicht_mehr_wiederherstellbar() {
        let fx = applied();
        purge_run(&fx.root, &run_id(), &env()).unwrap();

        let out = undo(&fx);

        assert!(out.purged);
        assert_eq!((out.restored(), out.missing(), out.exit_code()), (0, 2, 2));
        assert!(!fx.exists("b/kopie.txt"));
    }

    #[test]
    fn journal_bleibt_nach_dem_zuruecksetzen_lesbar_fuer_runs() {
        let fx = applied();
        undo(&fx);
        let entries = journal::read(&quarantine::journal_path(&fx.root, &run_id())).unwrap();
        assert!(matches!(entries[0], Entry::RunStart { .. }));
    }
}
