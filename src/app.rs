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
use crate::cli::{Cli, Command, IndexCommand, ReportArgs, ScanArgs};
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
    }
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

/// Prüft die Wurzel und liefert den normalisierten Pfad.
fn resolve_root(path: &Path, force: bool) -> Result<PathBuf> {
    let meta = std::fs::metadata(paths::extended(path))
        .with_context(|| format!("Pfad nicht lesbar: {}", paths::display(path)))?;
    if !meta.is_dir() {
        bail!("{} ist kein Ordner", paths::display(path));
    }
    let absolute = std::fs::canonicalize(path)
        .map(|p| PathBuf::from(paths::display(&p)))
        .unwrap_or_else(|_| path.to_path_buf());
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
    let requested = args.path.as_deref().map(|p| {
        std::fs::canonicalize(p)
            .map(|c| PathBuf::from(paths::display(&c)))
            .unwrap_or_else(|_| p.to_path_buf())
    });
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
            if index.remove_root(&paths::dir_key(path))? {
                println!("Entfernt: {}", paths::display(path));
                Ok(0)
            } else {
                bail!("{} ist nicht im Index", paths::display(path))
            }
        }
    }
}
