use std::io::{BufRead, IsTerminal, Write};
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
use crate::change::apply::{apply_plan, ActionResult, ActionStatus, ApplyEnv, ApplyOutcome};
use crate::change::fsops::RealFs;
use crate::change::limits;
use crate::change::plan::Plan;
use crate::change::protect::{ProtectPaths, Protector};
use crate::change::registry::{self, RunRecord};
use crate::change::undo::{
    expired_runs, list_runs, purge_run, undo_run, RestoreStatus, RunStatus, RunSummary, UndoEnv,
    UndoError,
};
use crate::change::{ActionCounts, RunId};
use crate::cli::{
    ApplyArgs, Cli, Command, HistoryArgs, IndexCommand, PlanCommand, PurgeArgs, ReportArgs,
    RunsArgs, ScanArgs, UndoArgs,
};
use crate::config::Config;
use crate::index::{Index, RootStatus};
use crate::paths::{self, index_path, registry_path};
use crate::platform::windows::{drive_kind, DriveKind};
use crate::report::{self, Format, ReportParams};
use crate::scan::classify::{Classifier, DefaultPaths};
use crate::scan::lock::ScanLock;
use crate::scan::source::{StdDirSource, TICKS_PER_SEC};
use crate::scan::walker::Progress;
use crate::scan::{scan, ScanEnv, ScanOutcome};

mod history;
mod plan;
mod profile;
mod run;
mod schedule;
mod snapshot;

/// Führt den Befehl aus und liefert den Exit-Code (0 OK, 2 OK mit Teilfehlern).
pub fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Scan(args) => scan_command(&args),
        Command::Report(args) => report_command(&args),
        Command::History(args) => history_command(&args),
        Command::Profiles => profile::profiles_command(),
        Command::Run(args) => run::run_command(&args),
        Command::Schedule(cmd) => schedule::schedule_command(&cmd),
        Command::Index(cmd) => index_command(&cmd),
        Command::Plan(PlanCommand::Dedupe(args)) => plan::plan_dedupe_command(&args),
        Command::Plan(PlanCommand::Junk(args)) => plan::plan_junk_command(&args),
        Command::Plan(PlanCommand::EmptyDirs(args)) => plan::plan_empty_dirs_command(&args),
        Command::Plan(PlanCommand::Archive(args)) => plan::plan_archive_command(&args),
        Command::Plan(PlanCommand::Versions(args)) => plan::plan_versions_command(&args),
        Command::Plan(PlanCommand::Rules(args)) => plan::plan_rules_command(&args),
        Command::Apply(args) => apply_command(&args),
        Command::Undo(args) => undo_command(&args),
        Command::Runs(args) => runs_command(&args),
        Command::Purge(args) => purge_command(&args),
    }
}

/// Ab diesem Alter des letzten Scans weist `plan` auf einen möglicherweise veralteten Index hin.
const STALE_SCAN_DAYS: i64 = 7;

/// Hinweis, wenn die Wurzel unter einem OneDrive-Ordner liegt: Die Quarantäne läge dann im
/// synchronisierten Bereich und erzeugt Sync-Traffic.
fn onedrive_warning(root: &Path, onedrive_roots: &[PathBuf]) -> Option<String> {
    limits::under_onedrive(root, onedrive_roots).then(|| {
        "Warnung: Der Ordner liegt in OneDrive. Die Quarantäne (.ordner-cleanup) wird \
             mitsynchronisiert und erzeugt Sync-Traffic; Dateien lassen sich später mit \
             `purge` endgültig entsorgen."
            .to_string()
    })
}

fn onedrive_roots_from_env() -> Vec<PathBuf> {
    ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .filter_map(|v| std::env::var_os(v).map(|p| normalize(Path::new(&p))))
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

/// Fragt einmal j/N. Ohne `--yes` bricht eine nicht interaktive Sitzung hart ab, damit ein
/// Skript nie versehentlich Dateien verschiebt oder löscht.
fn confirm_with(
    prompt: &str,
    yes: bool,
    interactive: bool,
    input: &mut dyn BufRead,
) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !interactive {
        bail!(
            "Keine interaktive Sitzung: ohne Bestätigung wird nichts verändert. \
             Mit --yes bestätigen."
        );
    }
    eprint!("{prompt}");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_lowercase().as_str(),
        "j" | "ja" | "y" | "yes"
    ))
}

fn confirm(prompt: &str, yes: bool) -> Result<bool> {
    let stdin = std::io::stdin();
    confirm_with(prompt, yes, stdin.is_terminal(), &mut stdin.lock())
}

fn install_cancel_flag() -> Result<Arc<AtomicBool>> {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))
        .context("Strg+C-Handler konnte nicht gesetzt werden")?;
    Ok(cancel)
}

fn status_line(result: &ActionResult) -> Option<String> {
    let text = match &result.status {
        ActionStatus::Done | ActionStatus::AlreadyDone => return None,
        ActionStatus::Skipped(reason) => reason.to_string(),
        ActionStatus::Failed(error) => format!("Fehler: {error}"),
    };
    Some(format!("  {text}: {}", result.path))
}

/// Rückfrage vor `apply`: nennt die Aktionen je Typ.
fn apply_question(plan: &Plan) -> String {
    format!("{}? [j/N] ", ActionCounts::from_plan(plan).plan_text())
}

/// Rückfrage vor `undo`: nennt die Aktionen des Laufs und die Bytes in der Quarantäne.
fn undo_question(run: &RunId, summary: &RunSummary) -> String {
    let bytes = if summary.bytes > 0 {
        format!(", {} in der Quarantäne", ByteSize::b(summary.bytes))
    } else {
        String::new()
    };
    format!(
        "Lauf {run} ({}{bytes}) zurückdrehen? [j/N] ",
        summary.counts.done_text()
    )
}

fn print_apply_summary(outcome: &ApplyOutcome) {
    let moved = if outcome.moved_bytes > 0 {
        format!(" ({})", ByteSize::b(outcome.moved_bytes))
    } else {
        String::new()
    };
    println!(
        "Lauf {}: {}{moved}, {} bereits erledigt, {} stale, {} übersprungen, {} Fehler",
        outcome.run,
        outcome.counts().done_text(),
        outcome.already_done(),
        outcome.stale(),
        outcome.skipped(),
        outcome.failed()
    );
    const MAX_LINES: usize = 20;
    let lines: Vec<String> = outcome.results.iter().filter_map(status_line).collect();
    for line in lines.iter().take(MAX_LINES) {
        println!("{line}");
    }
    if lines.len() > MAX_LINES {
        println!("  … und {} weitere", lines.len() - MAX_LINES);
    }
    if outcome.aborted {
        eprintln!("Abgebrochen; der Lauf ist teilweise ausgeführt und lässt sich zurückdrehen.");
    }
    if outcome.executed() > 0 {
        println!("Rückgängig machen: ordner-cleanup undo {}", outcome.run);
    }
}

fn apply_command(args: &ApplyArgs) -> Result<i32> {
    let plan = Plan::load(&args.plan)?;
    let root = PathBuf::from(&plan.root);
    if !root.is_dir() {
        bail!("Wurzel {} des Plans existiert nicht", plan.root);
    }
    let config = load_config()?;
    let protector = Protector::new(&root, &config, &ProtectPaths::from_env());

    println!(
        "Plan vom {}: {} Aktionen, {}, Wurzel {}",
        plan.created,
        plan.actions.len(),
        ByteSize::b(plan.total_bytes()),
        plan.root
    );
    let onedrive_roots = onedrive_roots_from_env();
    if let Some(warning) = onedrive_warning(&root, &onedrive_roots) {
        eprintln!("{warning}");
    }
    if let Some(message) = limits::exceeds(&plan, &onedrive_roots, &config) {
        if !args.allow_large {
            bail!("{message}");
        }
        eprintln!("Hinweis: Obergrenze mit --allow-large aufgehoben.");
    }
    if plan.actions.is_empty() {
        println!("Der Plan enthält keine Aktionen.");
        return Ok(0);
    }
    if !confirm(&apply_question(&plan), args.yes)? {
        println!("Abgebrochen. Es wurde nichts verändert.");
        return Ok(1);
    }

    let cancel = install_cancel_flag()?;
    let run = RunId::generate(chrono::Local::now());
    let now = now_rfc3339();
    let registered = registry_path().and_then(|file| {
        registry::append(
            &file,
            &RunRecord {
                run: run.clone(),
                root: plan.root.clone(),
                at: now.clone(),
            },
        )
        .map_err(Into::into)
    });
    if let Err(e) = registered {
        eprintln!("Hinweis: Lauf nicht im Register vermerkt ({e}); für undo --root angeben.");
    }

    let bar = ProgressBar::new(plan.actions.len() as u64);
    bar.set_style(
        ProgressStyle::with_template("{bar:30} {pos}/{len} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_bar()),
    );
    let plan_name = args
        .plan
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let outcome = apply_plan(
        &plan,
        &ApplyEnv {
            fs: &RealFs,
            protector: &protector,
            cancel: &cancel,
            progress: &|r| {
                bar.set_message(r.path.clone());
                bar.inc(1);
            },
            run,
            plan_name: &plan_name,
            now: &now,
        },
    );
    bar.finish_and_clear();
    let outcome = outcome?;
    print_apply_summary(&outcome);
    Ok(outcome.exit_code())
}

/// Wurzel eines Laufs: ausdrücklich angegeben oder aus dem Register.
fn find_run_root(run: &RunId, explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(normalize(path));
    }
    match registry::find_root(&registry_path()?, run) {
        Some(root) => Ok(PathBuf::from(root)),
        None => bail!("Lauf {run} ist im Register unbekannt; Wurzel mit --root angeben"),
    }
}

fn status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Complete => "vollständig",
        RunStatus::Partial => "teilweise",
        RunStatus::Incomplete => "unvollständig",
        RunStatus::Undone => "zurückgedreht",
        RunStatus::PartiallyUndone => "teilweise zurückgedreht",
        RunStatus::Purged => "Quarantäne gelöscht",
        RunStatus::Unreadable => "Journal unlesbar",
    }
}

fn local_time(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_string())
}

fn undo_command(args: &UndoArgs) -> Result<i32> {
    let config = load_config()?;
    let root = find_run_root(&args.run_id, args.root.as_deref())?;
    let summary = list_runs(&root, config.quarantine_days)?
        .into_iter()
        .find(|s| s.run == args.run_id)
        .ok_or_else(|| UndoError::NotFound(args.run_id.clone()))?;
    match summary.status {
        RunStatus::Undone => {
            println!("Lauf {} wurde bereits zurückgedreht.", args.run_id);
            return Ok(0);
        }
        RunStatus::Purged => {
            println!(
                "Lauf {}: Quarantäne wurde gelöscht, nicht mehr wiederherstellbar.",
                args.run_id
            );
            return Ok(2);
        }
        _ => {}
    }
    if !confirm(&undo_question(&args.run_id, &summary), args.yes)? {
        println!("Abgebrochen. Es wurde nichts verändert.");
        return Ok(1);
    }

    let now = now_rfc3339();
    let outcome = undo_run(
        &root,
        &args.run_id,
        &UndoEnv {
            fs: &RealFs,
            now: &now,
        },
    )?;
    println!(
        "Lauf {}: {} wiederhergestellt, {} Kollisionen, {} nicht mehr vorhanden, {} Fehler",
        outcome.run,
        outcome.restored(),
        outcome.conflicts(),
        outcome.missing(),
        outcome.failed()
    );
    for r in &outcome.results {
        match &r.status {
            RestoreStatus::Conflict(reason) => println!("  Kollision ({reason}): {}", r.path),
            RestoreStatus::Missing => println!("  nicht mehr wiederherstellbar: {}", r.path),
            RestoreStatus::Failed(error) => println!("  Fehler ({error}): {}", r.path),
            RestoreStatus::Restored | RestoreStatus::NothingToDo => {}
        }
    }
    if outcome.conflicts() > 0 {
        println!(
            "Kollidierende Einträge bleiben, wo sie sind (Quarantäne bzw. _Archiv); nach dem \
             Auflösen erneut `undo` ausführen."
        );
    }
    Ok(outcome.exit_code())
}

/// Alle Wurzeln mit möglichen Läufen: Register plus gescannte Wurzeln, ohne Duplikate.
fn known_roots() -> Result<Vec<PathBuf>> {
    let mut roots = registry::known_roots(&registry_path()?);
    let index_file = index_path()?;
    if index_file.exists() {
        roots.extend(
            Index::open(&index_file)?
                .roots()?
                .into_iter()
                .map(|r| r.path),
        );
    }
    let mut seen = std::collections::HashSet::new();
    Ok(roots
        .into_iter()
        .filter(|r| seen.insert(paths::path_key(Path::new(r))))
        .map(PathBuf::from)
        .collect())
}

fn runs_command(args: &RunsArgs) -> Result<i32> {
    let config = load_config()?;
    let roots = match &args.path {
        Some(path) => vec![normalize(path)],
        None => known_roots()?,
    };
    let mut any = false;
    for root in roots {
        let runs = list_runs(&root, config.quarantine_days)?;
        if runs.is_empty() {
            continue;
        }
        any = true;
        println!("Wurzel: {}", paths::display(&root));
        for r in runs {
            println!(
                "  {}  {}  {}  {}{}",
                r.run,
                r.started.as_deref().map(local_time).unwrap_or_default(),
                r.counts.short_text(r.bytes),
                status_label(r.status),
                r.expires
                    .filter(|_| r.status != RunStatus::Purged)
                    .map(|e| format!(
                        ", läuft ab {}",
                        e.with_timezone(&chrono::Local).format("%Y-%m-%d")
                    ))
                    .unwrap_or_default()
            );
        }
    }
    if !any {
        println!("Keine Läufe gefunden.");
    }
    Ok(0)
}

fn purge_command(args: &PurgeArgs) -> Result<i32> {
    let config = load_config()?;
    let days = match &args.older_than {
        Some(text) => u32::try_from(parse_old_after(text).map_err(anyhow::Error::msg)?)
            .context("Dauer ist zu groß")?,
        None => config.quarantine_days,
    };
    let roots = match &args.root {
        Some(path) => vec![normalize(path)],
        None => known_roots()?,
    };
    let now = chrono::Utc::now();
    let mut candidates = Vec::new();
    for root in roots {
        for run in expired_runs(&root, days, now)? {
            candidates.push((root.clone(), run));
        }
    }
    if candidates.is_empty() {
        println!("Keine abgelaufenen Läufe (älter als {days} Tage).");
        return Ok(0);
    }
    let total: u64 = candidates.iter().map(|(_, r)| r.bytes).sum();
    for (root, r) in &candidates {
        println!(
            "  {}  {}  {}  ({})",
            r.run,
            ByteSize::b(r.bytes),
            status_label(r.status),
            paths::display(root)
        );
    }
    let question = format!(
        "{} Läufe ({}) endgültig löschen? Das lässt sich nicht rückgängig machen. [j/N] ",
        candidates.len(),
        ByteSize::b(total)
    );
    if !confirm(&question, args.yes)? {
        println!("Abgebrochen. Es wurde nichts gelöscht.");
        return Ok(1);
    }

    let stamp = now_rfc3339();
    let env = UndoEnv {
        fs: &RealFs,
        now: &stamp,
    };
    let mut failures = 0;
    for (root, r) in &candidates {
        match purge_run(root, &r.run, &env) {
            Ok(()) => println!("Gelöscht: {} ({})", r.run, ByteSize::b(r.bytes)),
            Err(e) => {
                failures += 1;
                eprintln!("Fehler bei {}: {e}", r.run);
            }
        }
    }
    Ok(if failures > 0 { 2 } else { 0 })
}

fn load_config() -> Result<Config> {
    match paths::config_dir() {
        Some(dir) => Config::load(&dir.join("config.toml")),
        None => Ok(Config::default()),
    }
}

fn now_ticks() -> i64 {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    secs * TICKS_PER_SEC
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

fn history_command(args: &HistoryArgs) -> Result<i32> {
    let path = match &args.profile {
        Some(name) => Some(profile::target(None, Some(name))?.root),
        None => args.path.clone(),
    };
    history::history_command(args, path.as_deref())
}

/// Was ein Scan liefert: das Ergebnis, der Index und, bei vollständigem Lauf, die Momentaufnahme.
pub(super) struct ScanResult {
    pub outcome: ScanOutcome,
    pub index: Index,
    pub index_file: PathBuf,
    pub recorded: Option<Result<snapshot::Recorded>>,
}

pub(super) struct ScanJob<'a> {
    pub root: &'a Path,
    pub config: &'a Config,
    pub reset_index: bool,
    /// Fortschrittsanzeige im Terminal (aus für geplante Läufe)
    pub spinner: bool,
    pub profile: Option<&'a str>,
}

/// Scannt in den Index und schreibt danach bei vollständigem Lauf die Momentaufnahme. Die
/// Scan-Sperre hält der Aufrufer.
pub(super) fn run_scan(job: &ScanJob) -> Result<ScanResult> {
    let default_paths = DefaultPaths::from_env();
    let index_file = index_path()?;
    if job.reset_index {
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
        config: job.config,
        default_paths: &default_paths,
        cancel: &cancel,
        progress: &progress,
        find_duplicates: true,
        now: &now_rfc3339(),
    };
    let finished = AtomicBool::new(false);
    let outcome = std::thread::scope(|s| {
        if job.spinner {
            s.spawn(|| show_progress(&progress, &finished));
        }
        let result = scan(&mut index, job.root, &env);
        finished.store(true, Ordering::Relaxed);
        result
    })?;
    let recorded = (!outcome.aborted)
        .then(|| snapshot::record(&index, job.root, job.config, job.profile, outcome.errors));
    Ok(ScanResult {
        outcome,
        index,
        index_file,
        recorded,
    })
}

/// Zusammenfassung eines Scans; `true`, wenn er vollständig war.
fn print_scan_result(res: &ScanResult) -> bool {
    let outcome = &res.outcome;
    println!(
        "{} Dateien, {} Ordner, {} ({} Fehler/Warnungen)",
        outcome.files,
        outcome.dirs,
        ByteSize::b(outcome.bytes),
        outcome.errors
    );
    if outcome.aborted {
        eprintln!("Scan abgebrochen; der Index bleibt konsistent, aber unvollständig.");
        return false;
    }
    println!("Index: {}", paths::display(&res.index_file));
    match &res.recorded {
        Some(Ok(recorded)) => println!(
            "{}",
            report::history::score_line(recorded.score(), recorded.comparison().as_ref())
        ),
        Some(Err(e)) => eprintln!("Warnung: Verlauf nicht aktualisiert: {e:#}"),
        None => {}
    }
    true
}

fn scan_command(args: &ScanArgs) -> Result<i32> {
    let target = profile::target(args.path.as_deref(), args.profile.as_deref())?;
    let root = resolve_root(&target.root, args.force || target.force())?;
    let profile_name = target.profile_name().map(String::from);
    let mut config = target.config;
    config.apply_scan_args(args);
    if !config.no_default_excludes {
        if let Some(area) = Classifier::root_in_default_area(&root, &DefaultPaths::from_env()) {
            eprintln!(
                "Hinweis: {} liegt im Standard-Ausschlussbereich {}; der angegebene Pfad hat Vorrang.",
                paths::display(&root),
                paths::display(&area)
            );
        }
    }

    let _lock = ScanLock::acquire(&paths::scan_lock_path()?)?;
    let res = run_scan(&ScanJob {
        root: &root,
        config: &config,
        reset_index: args.reset_index,
        spinner: true,
        profile: profile_name.as_deref(),
    })?;
    if !print_scan_result(&res) {
        return Ok(1);
    }
    Ok(if res.outcome.errors > 0 { 2 } else { 0 })
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

/// Baut das Berichtsmodell samt Abschnitt „Verlauf“ aus dem Index. `notes` landen im Verlauf.
pub(super) fn build_report(
    index: &Index,
    config: &Config,
    requested: Option<&Path>,
    notes: Vec<String>,
) -> Result<(report::Report, report::ReportRoot)> {
    let old_after_days = parse_old_after(&config.old_after).map_err(anyhow::Error::msg)?;
    let requested = requested.map(normalize);
    let root = report::select_root(index, requested.as_deref())?;
    let params = ReportParams {
        generated_at: now_rfc3339(),
        now_ticks: now_ticks(),
        top: config.top,
        old_after_days,
        thresholds: Thresholds {
            max_depth: config.max_depth_warning,
            huge_entries: config.huge_dir_entries,
        },
        problem_ctx: ProblemCtx::from_env(&config.onedrive_conflict_hostnames),
    };
    let mut model = report::build(index, &root, &params)?;
    if let Err(e) = snapshot::attach_history(&mut model, index, &root, config, notes) {
        eprintln!("Warnung: Abschnitt Verlauf ausgelassen: {e:#}");
    }
    Ok((model, root))
}

fn report_command(args: &ReportArgs) -> Result<i32> {
    let (mut config, requested, notes) = match &args.profile {
        Some(name) => {
            let target = profile::target(None, Some(name))?;
            let notes = run::missed_runs_notes(name);
            (target.config, Some(target.root), notes)
        }
        None => (load_config()?, args.path.clone(), Vec::new()),
    };
    config.apply_report_args(args);
    let formats = Format::parse_list(&args.format)?;

    let index = Index::open(&index_path()?)?;
    let (model, root) = build_report(&index, &config, requested.as_deref(), notes)?;

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

#[cfg(test)]
mod confirm_tests {
    use super::*;
    use rstest::rstest;
    use std::io::Cursor;

    fn ask(input: &str, yes: bool, interactive: bool) -> Result<bool> {
        confirm_with("?", yes, interactive, &mut Cursor::new(input.to_string()))
    }

    #[rstest]
    #[case("j\n", true)]
    #[case("J\n", true)]
    #[case("ja\n", true)]
    #[case(" Yes \n", true)]
    #[case("n\n", false)]
    #[case("nein\n", false)]
    #[case("\n", false)]
    #[case("", false)]
    #[case("vielleicht\n", false)]
    fn antworten_werden_ausgewertet(#[case] input: &str, #[case] expected: bool) {
        assert_eq!(ask(input, false, true).unwrap(), expected);
    }

    #[test]
    fn yes_ueberspringt_die_frage_auch_ohne_terminal() {
        assert!(ask("", true, false).unwrap());
    }

    #[test]
    fn ohne_terminal_und_ohne_yes_bricht_es_hart_ab() {
        let err = ask("j\n", false, false).unwrap_err();
        assert!(err.to_string().contains("--yes"));
    }

    #[test]
    fn apply_frage_nennt_die_aktionen_je_typ() {
        use crate::change::plan::{ActionType, Plan, PlanKind, PlannedAction, PLAN_VERSION};
        let action = |id, action| PlannedAction {
            id,
            action,
            path: format!(r"D:\Daten\x{id}"),
            size: 0,
            mtime_ticks: 0,
            mtime: String::new(),
            hash: None,
            keep: None,
            keep_hash: None,
            reason: String::new(),
            target: None,
            is_dir: false,
            files: None,
            rule: None,
        };
        let plan = Plan {
            version: PLAN_VERSION,
            created: String::new(),
            kind: PlanKind::EmptyDirs,
            root: r"D:\Daten".into(),
            keep_strategy: None,
            params: Default::default(),
            actions: vec![
                action(1, ActionType::RemoveDir),
                action(2, ActionType::RemoveDir),
                action(3, ActionType::Move),
            ],
            skipped: vec![],
        };
        assert_eq!(
            apply_question(&plan),
            "1 Elemente nach _Archiv verschieben, 2 leere Ordner entfernen? [j/N] "
        );
    }

    #[test]
    fn undo_frage_nennt_aktionen_und_nur_quarantaene_bytes() {
        let run = RunId::parse("20261003-120000-ab12").unwrap();
        let summary = |counts: ActionCounts, bytes| RunSummary {
            run: run.clone(),
            started: None,
            moved: counts.total(),
            counts,
            bytes,
            status: RunStatus::Complete,
            expires: None,
        };
        let quarantine = ActionCounts {
            quarantined: 2,
            ..ActionCounts::default()
        };
        assert_eq!(
            undo_question(&run, &summary(quarantine, 30)),
            "Lauf 20261003-120000-ab12 (2 in die Quarantäne verschoben, 30 B in der Quarantäne) zurückdrehen? [j/N] "
        );
        let dirs = ActionCounts {
            dirs_removed: 4,
            ..ActionCounts::default()
        };
        assert_eq!(
            undo_question(&run, &summary(dirs, 0)),
            "Lauf 20261003-120000-ab12 (4 leere Ordner entfernt) zurückdrehen? [j/N] "
        );
    }

    #[test]
    fn statuszeilen_nennen_nur_nicht_erledigtes() {
        use crate::change::SkipReason;
        let result = |status| ActionResult {
            id: 1,
            path: r"D:\x.txt".into(),
            kind: crate::change::plan::ActionType::Quarantine,
            sorted: false,
            status,
        };
        assert!(status_line(&result(ActionStatus::Done)).is_none());
        assert!(status_line(&result(ActionStatus::AlreadyDone)).is_none());
        assert!(
            status_line(&result(ActionStatus::Skipped(SkipReason::Stale)))
                .unwrap()
                .contains("stale")
        );
        assert!(
            status_line(&result(ActionStatus::Failed("gesperrt".into())))
                .unwrap()
                .contains("Fehler: gesperrt")
        );
    }
}
