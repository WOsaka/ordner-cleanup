use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use bytesize::ByteSize;
use indicatif::{ProgressBar, ProgressStyle};

use crate::analysis::age::parse_old_after;
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
    ApplyArgs, Cli, Command, IndexCommand, PlanCommand, PurgeArgs, ReportArgs, RunsArgs, ScanArgs,
    UndoArgs,
};
use crate::index::RootStatus;
use crate::ops::admin::{index_remove, index_roots};
use crate::ops::report::{export_report, report_model, ReportRequest};
use crate::ops::scan::{scan, ScanReport, ScanRequest};
use crate::ops::target::TargetSpec;
use crate::ops::{
    find_run_root, index_age_note, known_roots, load_config, local_time, normalize, now_rfc3339,
    now_ticks, onedrive_roots_from_env, onedrive_warning, resolve_root, status_label, OpCtx,
};
use crate::paths::{self, index_path, registry_path};
use crate::report::{self, Format};

mod classify;
mod history;
mod plan;
mod profile;
mod progress;
mod run;
mod schedule;

/// Führt den Befehl aus und liefert den Exit-Code (0 OK, 2 OK mit Teilfehlern).
pub fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Scan(args) => scan_command(&args),
        Command::Classify(args) => classify::classify_command(&args),
        Command::Report(args) => report_command(&args),
        Command::History(args) => history::history_command(&args),
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
        Command::OcrWorker(args) => crate::platform::ocr::worker_main(
            &args.path,
            args.max_pages,
            args.langs
                .split(',')
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
        ),
    }
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

/// Das Abbruch-Flag des Prozesses (Strg+C). Der Handler wird nur einmal installiert, deshalb
/// teilen sich Scan, `classify` und `apply` dasselbe Flag (ein geplanter Lauf nutzt mehrere).
fn global_cancel_flag() -> Result<Arc<AtomicBool>> {
    static FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
    if let Some(flag) = FLAG.get() {
        return Ok(Arc::clone(flag));
    }
    let flag = Arc::new(AtomicBool::new(false));
    let handler_flag = Arc::clone(&flag);
    ctrlc::set_handler(move || handler_flag.store(true, Ordering::Relaxed))
        .context("Strg+C-Handler konnte nicht gesetzt werden")?;
    Ok(Arc::clone(FLAG.get_or_init(|| flag)))
}

fn install_cancel_flag() -> Result<Arc<AtomicBool>> {
    global_cancel_flag()
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
    let cancel = install_cancel_flag()?;
    let outcome = undo_run(
        &root,
        &args.run_id,
        &UndoEnv {
            fs: &RealFs,
            now: &now,
            cancel: &cancel,
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
    let cancel = install_cancel_flag()?;
    let env = UndoEnv {
        fs: &RealFs,
        now: &stamp,
        cancel: &cancel,
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

fn print_scan_result(res: &ScanReport) -> bool {
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
    if let Some(line) = &res.score_line {
        println!("{line}");
    }
    if let Some(e) = &res.history_warning {
        eprintln!("Warnung: Verlauf nicht aktualisiert: {e}");
    }
    true
}

fn scan_command(args: &ScanArgs) -> Result<i32> {
    let target = match (&args.path, &args.profile) {
        (_, Some(name)) => TargetSpec::Profile(name.clone()),
        (Some(path), None) => TargetSpec::Path {
            path: path.clone(),
            force: args.force,
        },
        (None, None) => bail!("Pfad oder --profile angeben"),
    };
    let ctx = OpCtx::new(global_cancel_flag()?);
    let res = progress::with_spinner(&ctx.progress, || {
        scan(
            &ScanRequest {
                target,
                exclude: args.exclude.clone(),
                summary_only: args.summary_only.clone(),
                no_default_excludes: args.no_default_excludes,
                reset_index: args.reset_index,
                threads: args.threads,
                template: args.template.clone(),
            },
            &ctx,
        )
    })?;
    for hint in &res.notes.hints {
        eprintln!("{hint}");
    }
    if !print_scan_result(&res) {
        return Ok(1);
    }
    Ok(if res.outcome.errors > 0 { 2 } else { 0 })
}

fn report_command(args: &ReportArgs) -> Result<i32> {
    let formats = Format::parse_list(&args.format)?;
    let ctx = OpCtx::new(global_cancel_flag()?);
    let view = report_model(
        &ReportRequest {
            path: args.path.clone(),
            profile: args.profile.clone(),
            template: args.template.clone(),
            old_after: args.old_after.clone(),
            top: args.top,
        },
        &ctx,
    )?;
    for warning in &view.notes.warnings {
        eprintln!("{warning}");
    }
    let written = export_report(&view, &formats, args.out.as_deref())?;

    print!("{}", report::terminal::render(&view.model));
    println!();
    for file in &written {
        println!("Geschrieben: {}", paths::display(file));
    }
    Ok(if view.root.status == RootStatus::Complete {
        0
    } else {
        2
    })
}

fn index_command(cmd: &IndexCommand) -> Result<i32> {
    match cmd {
        IndexCommand::List => {
            let roots = index_roots()?;
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
            if index_remove(&path)? {
                println!("Entfernt: {}", paths::display(&path));
                Ok(0)
            } else {
                bail!("{} ist nicht im Index", paths::display(&path))
            }
        }
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
