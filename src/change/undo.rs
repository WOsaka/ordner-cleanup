//! Undo, `runs` und `purge`. Undo vertraut nicht blind dem Journal, sondern prüft je Eintrag
//! den echten Dateizustand; es überschreibt nie und ist selbst journaliert und wiederholbar.

use std::io;
use std::path::Path;

use chrono::{DateTime, Utc};

use super::fsops::FsOps;
use super::journal::{self, EndStatus, Entry, JournalError, JournalWriter};
use super::plan::has_dot_component;
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

/// Ein Move aus dem Journal mit seinem bekannten Ausgang.
struct Moved {
    action: u32,
    from: String,
    to: String,
    size: u64,
    done: bool,
    failed: bool,
    undone: bool,
}

fn collect_moves(entries: &[Entry]) -> Vec<Moved> {
    let mut moves: Vec<Moved> = Vec::new();
    for entry in entries {
        if let Entry::Intent {
            action,
            from,
            to,
            size,
            ..
        } = entry
        {
            moves.push(Moved {
                action: *action,
                from: from.clone(),
                to: to.clone(),
                size: *size,
                done: false,
                failed: false,
                undone: false,
            });
            continue;
        }
        let (action, mark): (u32, fn(&mut Moved)) = match entry {
            Entry::Done { action, .. } => (*action, |m| m.done = true),
            Entry::Fail { action, .. } => (*action, |m| m.failed = true),
            Entry::UndoDone { action, .. } => (*action, |m| m.undone = true),
            _ => continue,
        };
        if let Some(m) = moves.iter_mut().rev().find(|m| m.action == action) {
            mark(m);
        }
    }
    moves
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

fn restore(
    m: &Moved,
    root_key: &str,
    quarantine_key: &str,
    env: &UndoEnv,
    journal: &mut JournalWriter,
    run: &RunId,
) -> Result<RestoreStatus, UndoError> {
    let (from, to) = (Path::new(&m.from), Path::new(&m.to));
    let (from_key, to_key) = (paths::path_key(from), paths::path_key(to));
    let sane = !has_dot_component(&m.from)
        && !has_dot_component(&m.to)
        && from_key != root_key
        && paths::is_under(&from_key, root_key)
        && to_key != quarantine_key
        && paths::is_under(&to_key, quarantine_key);
    if !sane {
        return Ok(RestoreStatus::Failed(
            "Journal-Eintrag verweist außerhalb von Wurzel oder Quarantäne".into(),
        ));
    }
    let (in_quarantine, at_origin) = (env.fs.exists(to), env.fs.exists(from));
    let mut conflict = |reason: &str| -> Result<(), UndoError> {
        journal.append(&Entry::UndoConflict {
            run: run.clone(),
            action: m.action,
            reason: reason.to_string(),
        })?;
        Ok(())
    };
    match (in_quarantine, at_origin) {
        (true, false) => {
            if let Some(parent) = from.parent() {
                if let Err(e) = env.fs.create_dir_all(parent) {
                    return Ok(RestoreStatus::Failed(e.to_string()));
                }
            }
            match env.fs.rename(to, from) {
                Ok(()) => {
                    journal.append(&Entry::UndoDone {
                        run: run.clone(),
                        action: m.action,
                    })?;
                    Ok(RestoreStatus::Restored)
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    conflict("Ursprungspfad ist belegt")?;
                    Ok(RestoreStatus::Conflict("Ursprungspfad ist belegt".into()))
                }
                Err(e) => Ok(RestoreStatus::Failed(e.to_string())),
            }
        }
        (true, true) => {
            conflict("Ursprungspfad ist belegt")?;
            Ok(RestoreStatus::Conflict("Ursprungspfad ist belegt".into()))
        }
        (false, true) => Ok(RestoreStatus::NothingToDo),
        (false, false) => {
            conflict("nicht mehr wiederherstellbar")?;
            Ok(RestoreStatus::Missing)
        }
    }
}

pub fn undo_run(root: &Path, run: &RunId, env: &UndoEnv) -> Result<UndoOutcome, UndoError> {
    let entries = load(root, run)?;
    let moves = collect_moves(&entries);
    let outcome = |results, already_undone, purged| UndoOutcome {
        run: run.clone(),
        results,
        already_undone,
        purged,
    };

    if entries.iter().any(|e| matches!(e, Entry::Purged { .. })) {
        let results = moves
            .iter()
            .filter(|m| m.done && !m.failed && !m.undone)
            .map(|m| RestoreResult {
                action: m.action,
                path: m.from.clone(),
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
    let quarantine_key = paths::path_key(&quarantine::run_dir(root, run));
    let mut results = Vec::new();
    // Rückwärts: zuletzt Verschobenes zuerst zurück.
    for m in moves.iter().rev().filter(|m| !m.failed) {
        let status = if m.undone {
            RestoreStatus::NothingToDo
        } else {
            restore(m, &root_key, &quarantine_key, env, &mut journal, run)?
        };
        results.push(RestoreResult {
            action: m.action,
            path: m.from.clone(),
            status,
        });
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
    let moves = collect_moves(entries);
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
        moved: moves.iter().filter(|m| m.done && !m.failed).count(),
        bytes: if purged {
            0
        } else {
            moves
                .iter()
                .filter(|m| m.done && !m.failed && !m.undone)
                .map(|m| m.size)
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
