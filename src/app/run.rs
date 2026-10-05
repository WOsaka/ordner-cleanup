//! Befehl `run`: ein geplanter Lauf für ein Profil. Scan, Momentaufnahme, Bericht und die im
//! Profil genannten Pläne – nie `apply`. Alles landet unter dem Datenordner des Profils.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::profile::{self, Target};
use super::{build_report, now_rfc3339, plan, print_scan_result, resolve_root, run_scan, ScanJob};
use crate::change::dedupe::KeepStrategy;
use crate::change::plan::Plan;
use crate::cli::{
    PlanArchiveArgs, PlanDedupeArgs, PlanEmptyDirsArgs, PlanJunkArgs, PlanRulesArgs,
    PlanVersionsArgs, RunArgs,
};
use crate::notify::{self, OpenTarget};
use crate::paths;
use crate::platform::toast;
use crate::runlog::{self, PlanRecord, RunRecord, RunStatus};
use crate::scan::lock::{LockError, ScanLock};

/// Exit-Codes von `run`.
const EXIT_PARTIAL: i32 = 2;
const EXIT_FAILED: i32 = 1;
const EXIT_SKIPPED: i32 = 3;

/// Hinweise auf ausgefallene Läufe für den nächsten Bericht eines Profils.
pub(super) fn missed_runs_notes(profile: &str) -> Vec<String> {
    paths::runs_log(profile)
        .ok()
        .and_then(|log| runlog::missed_runs_note(&runlog::read_all(&log)))
        .into_iter()
        .collect()
}

pub(super) fn run_command(args: &RunArgs) -> Result<i32> {
    let name = args.profile.as_str();
    let target = profile::target(None, Some(name))?;
    let log = paths::runs_log(name)?;
    let mut record = RunRecord {
        started: now_rfc3339(),
        ended: String::new(),
        status: RunStatus::Failed,
        score: None,
        score_delta: None,
        report: None,
        plans: Vec::new(),
        errors: Vec::new(),
        notified: false,
        review: None,
    };
    let code = match execute(name, &target, &mut record) {
        Ok(code) => code,
        Err(e) => {
            record.status = RunStatus::Failed;
            record.errors.push(format!("{e:#}"));
            eprintln!("Fehler: {e:#}");
            EXIT_FAILED
        }
    };
    record.ended = now_rfc3339();
    if args.notify {
        notify_if_relevant(name, &target, &log, &mut record);
    }
    if let Err(e) = runlog::append(&log, &record) {
        eprintln!(
            "Warnung: Lauf-Protokoll {} nicht geschrieben: {e}",
            paths::display(&log)
        );
    }
    Ok(code)
}

/// Zeigt bei relevanter Veränderung eine Benachrichtigung; ein Fehler dabei kippt den Lauf nicht
/// (Benachrichtigungen können systemweit abgeschaltet sein, das Ergebnis steht im Bericht).
fn notify_if_relevant(name: &str, target: &Target, log: &Path, record: &mut RunRecord) {
    let previous = runlog::read_all(log).pop();
    let Some(n) = notify::decide(name, record, previous.as_ref(), &target.config.notify) else {
        return;
    };
    let open = match (n.open, &record.report) {
        (OpenTarget::Report, Some(report)) => PathBuf::from(report),
        _ => log.to_path_buf(),
    };
    match toast::system_notifier().notify(&n.title, &n.body, &open) {
        Ok(()) => record.notified = true,
        Err(e) => eprintln!("Warnung: Benachrichtigung nicht angezeigt: {e}"),
    }
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn execute(name: &str, target: &Target, record: &mut RunRecord) -> Result<i32> {
    let _lock = match ScanLock::acquire(&paths::scan_lock_path()?) {
        Ok(lock) => lock,
        Err(LockError::Busy(path)) => {
            record.status = RunStatus::SkippedLocked;
            record.errors.push(format!(
                "Ein anderer Scan läuft (Sperre {})",
                paths::display(&path)
            ));
            println!("Lauf übersprungen: ein anderer Scan läuft.");
            return Ok(EXIT_SKIPPED);
        }
        Err(e) => return Err(e.into()),
    };

    if !paths::extended(&target.root).is_dir() {
        record.status = RunStatus::RootMissing;
        record.errors.push(format!(
            "Wurzel {} nicht erreichbar",
            paths::display(&target.root)
        ));
        eprintln!("Fehler: {}", record.errors[0]);
        return Ok(EXIT_FAILED);
    }
    let root = resolve_root(&target.root, target.force())?;

    // Eine unbrauchbare Vorlage kippt den Lauf nicht: Scan und Bericht entstehen ohne sie.
    let template = match profile::load_template(None, target) {
        Ok(template) => template,
        Err(e) => {
            record.errors.push(format!("Vorlage: {e:#}"));
            None
        }
    };
    let mut res = run_scan(&ScanJob {
        root: &root,
        config: &target.config,
        reset_index: false,
        spinner: false,
        profile: Some(name),
        template: template.as_ref(),
    })?;
    let complete = print_scan_result(&res);
    if !complete {
        record.errors.push("Scan abgebrochen".into());
        return Ok(EXIT_FAILED);
    }
    let mut partial = res.outcome.errors > 0 || !record.errors.is_empty();
    match &res.recorded {
        Some(Ok(recorded)) => {
            record.score = Some(recorded.score());
            record.score_delta = recorded.comparison().map(|c| c.delta);
        }
        Some(Err(e)) => {
            partial = true;
            record.errors.push(format!("Verlauf: {e:#}"));
        }
        None => {}
    }

    if target
        .profile
        .as_ref()
        .is_some_and(|p| p.profile.classify == Some(true))
    {
        match super::classify::classify_for_run(&mut res.index, &root, &target.config) {
            Ok(review) => record.review = Some(review),
            Err(e) => {
                partial = true;
                record.errors.push(format!("classify: {e:#}"));
            }
        }
    }

    let stamp = stamp();
    match write_report(name, target, template.as_ref(), &res.index, &root, &stamp) {
        Ok(latest) => record.report = Some(paths::display(&latest)),
        Err(e) => {
            partial = true;
            record.errors.push(format!("Bericht: {e:#}"));
        }
    }
    // Die Pläne öffnen den Index selbst.
    drop(res);

    if let Some(profile) = &target.profile {
        for kind in &profile.profile.plans {
            let rec = make_plan(name, kind, &stamp);
            if let Some(error) = &rec.error {
                partial = true;
                record.errors.push(format!("Plan {kind}: {error}"));
            }
            record.plans.push(rec);
        }
    }

    prune(&paths::reports_dir(name)?, target.config.reports_keep);
    prune(&paths::plans_dir(name)?, target.config.reports_keep);

    record.status = if partial {
        RunStatus::Partial
    } else {
        RunStatus::Ok
    };
    Ok(if partial { EXIT_PARTIAL } else { 0 })
}

/// Schreibt `<Zeitstempel>.html` und kopiert ihn nach `latest.html`.
fn write_report(
    name: &str,
    target: &Target,
    template: Option<&crate::template::Loaded>,
    index: &crate::index::Index,
    root: &Path,
    stamp: &str,
) -> Result<PathBuf> {
    let notes = missed_runs_notes(name);
    let (model, _) = build_report(index, &target.config, Some(root), template, notes)?;
    let dir = paths::reports_dir(name)?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Berichtsordner {} nicht anlegbar", paths::display(&dir)))?;
    let file = dir.join(format!("{stamp}.html"));
    std::fs::write(&file, crate::report::html::render(&model)?)
        .with_context(|| format!("{} nicht schreibbar", paths::display(&file)))?;
    let latest = dir.join("latest.html");
    std::fs::copy(&file, &latest)
        .with_context(|| format!("{} nicht schreibbar", paths::display(&latest)))?;
    Ok(latest)
}

/// Erzeugt einen Plan des Profils nach `plans\<profil>\<Zeitstempel>-<art>.json`.
fn make_plan(name: &str, kind: &str, stamp: &str) -> PlanRecord {
    let fail = |error: String| PlanRecord {
        kind: kind.to_string(),
        actions: 0,
        path: None,
        error: Some(error),
    };
    let dir = match paths::plans_dir(name) {
        Ok(dir) => dir,
        Err(e) => return fail(format!("{e:#}")),
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail(format!("Planordner nicht anlegbar: {e}"));
    }
    let out = dir.join(format!("{stamp}-{kind}.json"));
    let profile = Some(name.to_string());
    let out_arg = Some(out.clone());
    let result = match kind {
        "rules" => plan::plan_rules_command(&PlanRulesArgs {
            path: None,
            profile,
            rules: None,
            rule: Vec::new(),
            out: out_arg,
            no_classify: false,
        }),
        "junk" => plan::plan_junk_command(&PlanJunkArgs {
            path: None,
            profile,
            category: Vec::new(),
            out: out_arg,
        }),
        "empty-dirs" => plan::plan_empty_dirs_command(&PlanEmptyDirsArgs {
            path: None,
            profile,
            out: out_arg,
        }),
        "archive" => plan::plan_archive_command(&PlanArchiveArgs {
            path: None,
            profile,
            older_than: None,
            out: out_arg,
        }),
        "versions" => plan::plan_versions_command(&PlanVersionsArgs {
            path: None,
            profile,
            min_age: None,
            out: out_arg,
        }),
        "dedupe" => plan::plan_dedupe_command(&PlanDedupeArgs {
            path: None,
            profile,
            keep: KeepStrategy::Oldest,
            out: out_arg,
        }),
        other => return fail(format!("unbekannter Plan '{other}'")),
    };
    match result {
        Ok(_) => match Plan::load(&out) {
            Ok(plan) => PlanRecord {
                kind: kind.to_string(),
                actions: plan.actions.len() as u64,
                path: Some(paths::display(&out)),
                error: None,
            },
            Err(e) => fail(format!("Plan nicht lesbar: {e}")),
        },
        Err(e) => fail(format!("{e:#}")),
    }
}

/// Zeitstempel-Präfix `JJJJMMTT-HHMMSS` eines Dateinamens.
fn stamp_of(file_name: &str) -> Option<&str> {
    let prefix = file_name.get(..15)?;
    let (date, time) = prefix.split_once('-')?;
    (date.len() == 8
        && time.len() == 6
        && date.bytes().chain(time.bytes()).all(|b| b.is_ascii_digit()))
    .then_some(prefix)
}

/// Behält die Dateien der letzten `keep` Läufe (nach Zeitstempel im Namen). Andere Dateien
/// wie `latest.html` bleiben unberührt. Fehler beim Aufräumen sind kein Grund, den Lauf zu kippen.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let files: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            stamp_of(&name).map(|s| (s.to_string(), e.path()))
        })
        .collect();
    let stamps: BTreeSet<&str> = files.iter().map(|(s, _)| s.as_str()).collect();
    let keep_stamps: BTreeSet<&str> = stamps.iter().rev().take(keep).copied().collect();
    for (stamp, path) in &files {
        if !keep_stamps.contains(stamp.as_str()) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), "x").unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn zeitstempel_wird_erkannt() {
        assert_eq!(stamp_of("20261004-093000.html"), Some("20261004-093000"));
        assert_eq!(
            stamp_of("20261004-093000-junk.json"),
            Some("20261004-093000")
        );
        assert_eq!(stamp_of("latest.html"), None);
        assert_eq!(stamp_of("2026100-0930000.html"), None);
        assert_eq!(stamp_of("kurz"), None);
        assert_eq!(stamp_of("ä0261004-093000"), None);
    }

    #[test]
    fn aufraeumen_behaelt_die_juengsten_laeufe_und_alle_dateien_eines_laufs() {
        let dir = tempfile::tempdir().unwrap();
        for day in 1..=5 {
            touch(dir.path(), &format!("2026100{day}-080000-junk.json"));
            touch(dir.path(), &format!("2026100{day}-080000-rules.json"));
        }
        touch(dir.path(), "latest.html");
        touch(dir.path(), "notiz.txt");
        prune(dir.path(), 2);
        assert_eq!(
            names(dir.path()),
            [
                "20261004-080000-junk.json",
                "20261004-080000-rules.json",
                "20261005-080000-junk.json",
                "20261005-080000-rules.json",
                "latest.html",
                "notiz.txt"
            ]
        );
    }

    #[test]
    fn aufraeumen_ohne_ordner_oder_mit_wenigen_dateien_aendert_nichts() {
        prune(Path::new(r"C:\gibt\es\nicht"), 3);
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "20261001-080000.html");
        prune(dir.path(), 12);
        assert_eq!(names(dir.path()).len(), 1);
    }
}
