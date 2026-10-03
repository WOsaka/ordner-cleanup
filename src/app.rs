use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use bytesize::ByteSize;
use indicatif::{ProgressBar, ProgressStyle};

use crate::analysis::age::parse_old_after;
use crate::analysis::problems::ProblemCtx;
use crate::analysis::structure::Thresholds;
use crate::change::dedupe::plan_dedupe;
use crate::change::protect::{ProtectPaths, Protector};
use crate::cli::{Cli, Command, IndexCommand, PlanCommand, PlanDedupeArgs, ReportArgs, ScanArgs};
use crate::config::Config;
use crate::index::{Index, RootStatus};
use crate::paths;
use crate::platform::windows::{drive_kind, DriveKind};
use crate::report::{self, Format, ReportParams};
use crate::scan::classify::{Classifier, DefaultPaths};
use crate::scan::source::{StdDirSource, TICKS_PER_SEC};
use crate::scan::walker::Progress;
use crate::scan::{scan, ScanEnv};

/// Führt den Befehl aus und liefert den Exit-Code (0 OK, 2 OK mit Teilfehlern).
pub fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Scan(args) => scan_command(&args),
        Command::Report(args) => report_command(&args),
        Command::Index(cmd) => index_command(&cmd),
        Command::Plan(PlanCommand::Dedupe(args)) => plan_dedupe_command(&args),
    }
}

/// Ab diesem Alter des letzten Scans weist `plan` auf einen möglicherweise veralteten Index hin.
const STALE_SCAN_DAYS: i64 = 7;

/// Hinweis, wenn die Wurzel unter einem OneDrive-Ordner liegt: Die Quarantäne läge dann im
/// synchronisierten Bereich und erzeugt Sync-Traffic.
fn onedrive_warning(root: &Path, onedrive_roots: &[PathBuf]) -> Option<String> {
    let key = paths::path_key(root);
    onedrive_roots
        .iter()
        .any(|r| paths::is_under(&key, &paths::path_key(r)))
        .then(|| {
            "Warnung: Der Ordner liegt in OneDrive. Die Quarantäne (.ordner-cleanup) wird \
             mitsynchronisiert und erzeugt Sync-Traffic; Dateien lassen sich später mit \
             `purge` endgültig entsorgen."
                .to_string()
        })
}

fn onedrive_roots_from_env() -> Vec<PathBuf> {
    ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
        .collect()
}

/// Hinweis zum Index-Zustand der Wurzel (kein Abbruch).
fn index_age_note(root: &report::ReportRoot, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    if root.status != RootStatus::Complete {
        return Some(
            "Hinweis: Der letzte Scan dieses Ordners war nicht vollständig; der Plan kann \
             unvollständig sein. Bitte neu scannen."
                .to_string(),
        );
    }
    let scanned = chrono::DateTime::parse_from_rfc3339(root.scanned_at.as_deref()?).ok()?;
    let age = now.signed_duration_since(scanned).num_days();
    (age >= STALE_SCAN_DAYS).then(|| {
        format!("Hinweis: Der letzte Scan ist {age} Tage alt. Bitte neu scannen, falls sich viel geändert hat.")
    })
}

fn plan_dedupe_command(args: &PlanDedupeArgs) -> Result<i32> {
    let root = resolve_root(&args.path, false)?;
    let config = load_config()?;
    let index = Index::open(&index_path()?)?;
    let scanned = report::select_root(&index, Some(&root))?;
    if let Some(note) = index_age_note(&scanned, chrono::Utc::now()) {
        eprintln!("{note}");
    }
    if let Some(warning) = onedrive_warning(&root, &onedrive_roots_from_env()) {
        eprintln!("{warning}");
    }

    let protector = Protector::new(&root, &config, &ProtectPaths::from_env());
    let result = plan_dedupe(&index, &root, &args.keep, &protector, &now_rfc3339())?;
    let out = match &args.out {
        Some(out) => out.clone(),
        None => PathBuf::from(format!(
            "plan-{}.json",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        )),
    };
    result
        .plan
        .save(&out)
        .with_context(|| format!("Plan-Datei {} nicht schreibbar", paths::display(&out)))?;

    let plan = &result.plan;
    println!(
        "{} Aktionen, {} freiwerdend, {} übersprungen (Strategie: {})",
        plan.actions.len(),
        ByteSize::b(result.freed_bytes),
        plan.skipped.len(),
        plan.keep_strategy
    );
    let mut reasons = std::collections::BTreeMap::new();
    for s in &plan.skipped {
        *reasons.entry(s.reason.to_string()).or_insert(0usize) += 1;
    }
    for (reason, count) in reasons {
        println!("  übersprungen: {count} × {reason}");
    }
    let shown = std::path::absolute(&out).unwrap_or(out);
    println!("Plan: {}", paths::display(&shown));
    println!("Es wurde nichts verändert. Plan prüfen, danach mit `apply` ausführen.");
    Ok(0)
}

fn index_path() -> Result<PathBuf> {
    let dir = paths::data_dir().context("Datenordner (%LOCALAPPDATA%) nicht ermittelbar")?;
    Ok(dir.join("index.db"))
}

fn load_config() -> Result<Config> {
    match paths::config_dir() {
        Some(dir) => Config::load(&dir.join("config.toml")),
        None => Ok(Config::default()),
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Löst Kurznamen (8.3), Symlinks und relative Pfade auf, damit Scan, Report und
/// `index remove` denselben Schlüssel verwenden. Nicht existierende Pfade bleiben unverändert.
fn normalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|p| PathBuf::from(paths::display(&p)))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Prüft die Wurzel und liefert den normalisierten Pfad.
fn resolve_root(path: &Path, force: bool) -> Result<PathBuf> {
    let meta = std::fs::metadata(paths::extended(path))
        .with_context(|| format!("Pfad nicht lesbar: {}", paths::display(path)))?;
    if !meta.is_dir() {
        bail!("{} ist kein Ordner", paths::display(path));
    }
    let absolute = normalize(path);
    if drive_kind(&absolute) != DriveKind::Local && !force {
        bail!(
            "{} liegt auf einem Netzlaufwerk bzw. UNC-Pfad. Das wird in Phase 1 nicht unterstützt; \
             mit --force trotzdem scannen",
            paths::display(&absolute)
        );
    }
    Ok(absolute)
}

fn scan_command(args: &ScanArgs) -> Result<i32> {
    let root = resolve_root(&args.path, args.force)?;
    let mut config = load_config()?;
    config.apply_scan_args(args);
    let default_paths = DefaultPaths::from_env();
    if !config.no_default_excludes {
        if let Some(area) = Classifier::root_in_default_area(&root, &default_paths) {
            eprintln!(
                "Hinweis: {} liegt im Standard-Ausschlussbereich {}; der angegebene Pfad hat Vorrang.",
                paths::display(&root),
                paths::display(&area)
            );
        }
    }

    let index_file = index_path()?;
    if args.reset_index {
        Index::reset(&index_file)?;
    }
    let mut index = Index::open(&index_file)?;

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let cancel = Arc::clone(&cancel);
        ctrlc::set_handler(move || cancel.store(true, Ordering::Relaxed))
            .context("Strg+C-Handler konnte nicht gesetzt werden")?;
    }

    let progress = Progress::default();
    let env = ScanEnv {
        source: &StdDirSource,
        config: &config,
        default_paths: &default_paths,
        cancel: &cancel,
        progress: &progress,
        find_duplicates: true,
        now: &now_rfc3339(),
    };
    let finished = AtomicBool::new(false);
    let outcome = std::thread::scope(|s| {
        s.spawn(|| show_progress(&progress, &finished));
        let result = scan(&mut index, &root, &env);
        finished.store(true, Ordering::Relaxed);
        result
    })?;

    println!(
        "{} Dateien, {} Ordner, {} ({} Fehler/Warnungen)",
        outcome.files,
        outcome.dirs,
        ByteSize::b(outcome.bytes),
        outcome.errors
    );
    if outcome.aborted {
        eprintln!("Scan abgebrochen; der Index bleibt konsistent, aber unvollständig.");
        return Ok(1);
    }
    println!("Index: {}", paths::display(&index_file));
    Ok(if outcome.errors > 0 { 2 } else { 0 })
}

fn show_progress(progress: &Progress, finished: &AtomicBool) {
    let bar = ProgressBar::new_spinner();
    bar.set_style(
        ProgressStyle::with_template("{spinner} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    while !finished.load(Ordering::Relaxed) {
        let current = progress
            .current
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default();
        bar.set_message(format!(
            "{} Dateien, {} Ordner, {}, {} Fehler – {}",
            progress.files.load(Ordering::Relaxed),
            progress.dirs.load(Ordering::Relaxed),
            ByteSize::b(progress.bytes.load(Ordering::Relaxed)),
            progress.errors.load(Ordering::Relaxed),
            current
        ));
        bar.tick();
        std::thread::sleep(Duration::from_millis(200));
    }
    bar.finish_and_clear();
}

fn report_command(args: &ReportArgs) -> Result<i32> {
    let mut config = load_config()?;
    config.apply_report_args(args);
    let old_after_days = parse_old_after(&config.old_after).map_err(anyhow::Error::msg)?;
    let formats = Format::parse_list(&args.format)?;

    let index = Index::open(&index_path()?)?;
    let requested = args.path.as_deref().map(normalize);
    let root = report::select_root(&index, requested.as_deref())?;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .context("Systemzeit liegt vor 1970")?
        .as_secs() as i64;
    let params = ReportParams {
        generated_at: now_rfc3339(),
        now_ticks: now * TICKS_PER_SEC,
        top: config.top,
        old_after_days,
        thresholds: Thresholds {
            max_depth: config.max_depth_warning,
            huge_entries: config.huge_dir_entries,
        },
        problem_ctx: ProblemCtx::from_env(&config.onedrive_conflict_hostnames),
    };
    let model = report::build(&index, &root, &params)?;

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let dir = report::prepare_out_dir(args.out.as_deref(), &timestamp)?;
    let written = report::write_all(&model, &formats, &dir)?;

    print!("{}", report::terminal::render(&model));
    println!();
    for file in &written {
        println!("Geschrieben: {}", paths::display(file));
    }
    Ok(if root.status == RootStatus::Complete {
        0
    } else {
        2
    })
}

fn index_command(cmd: &IndexCommand) -> Result<i32> {
    let mut index = Index::open(&index_path()?)?;
    match cmd {
        IndexCommand::List => {
            let roots = index.roots()?;
            if roots.is_empty() {
                println!("Keine gescannten Wurzeln im Index.");
            }
            for r in roots {
                println!(
                    "{}  ({:?}, {}, {} Fehler)",
                    r.path,
                    r.status,
                    r.finished_at.or(r.started_at).unwrap_or_default(),
                    r.error_count
                );
            }
            Ok(0)
        }
        IndexCommand::Remove { path } => {
            let path = normalize(path);
            if index.remove_root(&paths::dir_key(&path))? {
                println!("Entfernt: {}", paths::display(&path));
                Ok(0)
            } else {
                bail!("{} ist nicht im Index", paths::display(&path))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(status: RootStatus, scanned_at: Option<&str>) -> report::ReportRoot {
        report::ReportRoot {
            path: r"D:\Daten".into(),
            dir_key: r"d:\daten\".into(),
            scanned_at: scanned_at.map(String::from),
            status,
        }
    }

    fn at(text: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn onedrive_warnung_nur_unterhalb_der_onedrive_wurzel() {
        let roots = [PathBuf::from(r"C:\Users\me\OneDrive")];
        assert!(onedrive_warning(Path::new(r"C:\Users\me\onedrive\Doku"), &roots).is_some());
        assert!(onedrive_warning(Path::new(r"C:\Users\me\OneDrive"), &roots).is_some());
        assert!(onedrive_warning(Path::new(r"C:\Users\me\OneDrive2\x"), &roots).is_none());
        assert!(onedrive_warning(Path::new(r"D:\Daten"), &roots).is_none());
        assert!(onedrive_warning(Path::new(r"D:\Daten"), &[]).is_none());
    }

    #[test]
    fn unvollstaendiger_scan_wird_gemeldet() {
        let r = root(RootStatus::Aborted, Some("2026-10-03T10:00:00Z"));
        assert!(index_age_note(&r, at("2026-10-03T12:00:00Z"))
            .unwrap()
            .contains("nicht vollständig"));
    }

    #[test]
    fn alter_scan_wird_ab_sieben_tagen_gemeldet() {
        let r = root(RootStatus::Complete, Some("2026-09-26T12:00:00Z"));
        assert!(index_age_note(&r, at("2026-10-03T12:00:00Z")).is_some());
        assert!(index_age_note(&r, at("2026-10-03T11:00:00Z")).is_none());
    }

    #[test]
    fn frischer_oder_undatierter_scan_ist_still() {
        let fresh = root(RootStatus::Complete, Some("2026-10-03T10:00:00Z"));
        assert!(index_age_note(&fresh, at("2026-10-03T12:00:00Z")).is_none());
        let undated = root(RootStatus::Complete, None);
        assert!(index_age_note(&undated, at("2026-10-03T12:00:00Z")).is_none());
    }
}
