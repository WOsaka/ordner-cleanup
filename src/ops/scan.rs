//! `scan`: in den Index scannen und danach (bei vollständigem Lauf) die Momentaufnahme
//! schreiben. Fortschritt über den `TaskProgress`, Abbruch über das Flag im `OpCtx`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;

use super::snapshot::{self, Recorded};
use super::{
    acquire_scan_lock, now_rfc3339, resolve_root, target::load_template, target::TargetSpec, Notes,
    OpCtx,
};
use crate::index::Index;
use crate::paths::{self, index_path};
use crate::report;
use crate::scan::classify::{Classifier, DefaultPaths};
use crate::scan::source::StdDirSource;
use crate::scan::walker::Progress;
use crate::scan::{scan as scan_tree, ScanEnv, ScanOutcome};
use crate::template::Loaded;

/// Was ein Scan liefert: das Ergebnis, der Index und, bei vollständigem Lauf, die Momentaufnahme.
pub struct ScanResult {
    pub outcome: ScanOutcome,
    pub index: Index,
    pub index_file: PathBuf,
    pub recorded: Option<Result<Recorded>>,
}

impl ScanResult {
    /// Ergebnis für die Anzeige; `root` ist die gescannte Wurzel.
    pub fn report(&self, root: &Path, notes: Notes) -> ScanReport {
        let (score_line, history_warning) = match &self.recorded {
            Some(Ok(recorded)) => (
                Some(report::history::score_line(
                    recorded.score(),
                    recorded.comparison().as_ref(),
                )),
                None,
            ),
            Some(Err(e)) => (None, Some(format!("{e:#}"))),
            None => (None, None),
        };
        let score = match &self.recorded {
            Some(Ok(recorded)) => Some(recorded.score()),
            _ => None,
        };
        ScanReport {
            root: root.to_path_buf(),
            outcome: self.outcome.clone(),
            index_file: self.index_file.clone(),
            score,
            score_line,
            history_warning,
            notes,
        }
    }
}

pub struct ScanJob<'a> {
    pub root: &'a Path,
    pub config: &'a crate::config::Config,
    pub reset_index: bool,
    pub profile: Option<&'a str>,
    pub template: Option<&'a Loaded>,
}

/// Scannt in den Index und schreibt danach bei vollständigem Lauf die Momentaufnahme. Die
/// Scan-Sperre hält der Aufrufer.
pub fn run_scan(job: &ScanJob, ctx: &OpCtx) -> Result<ScanResult> {
    let default_paths = DefaultPaths::from_env();
    let index_file = index_path()?;
    if job.reset_index {
        Index::reset(&index_file)?;
    }
    let mut index = Index::open(&index_file)?;

    let progress = Progress::default();
    let env = ScanEnv {
        source: &StdDirSource,
        config: job.config,
        default_paths: &default_paths,
        cancel: &ctx.cancel,
        progress: &progress,
        find_duplicates: true,
        now: &now_rfc3339(),
    };
    ctx.progress.reset("Scannen", 0);
    let finished = AtomicBool::new(false);
    let outcome = std::thread::scope(|s| {
        s.spawn(|| mirror_progress(&progress, ctx, &finished));
        let result = scan_tree(&mut index, job.root, &env);
        finished.store(true, Ordering::Relaxed);
        result
    })?;
    let recorded = (!outcome.aborted).then(|| {
        snapshot::record(
            &index,
            job.root,
            job.config,
            job.template,
            job.profile,
            outcome.errors,
        )
    });
    Ok(ScanResult {
        outcome,
        index,
        index_file,
        recorded,
    })
}

/// Spiegelt den Fortschritt des Walkers in den `TaskProgress` (der Walker bleibt unverändert).
fn mirror_progress(progress: &Progress, ctx: &OpCtx, finished: &AtomicBool) {
    let target = &ctx.progress;
    loop {
        target
            .done
            .store(progress.files.load(Ordering::Relaxed), Ordering::Relaxed);
        target
            .dirs
            .store(progress.dirs.load(Ordering::Relaxed), Ordering::Relaxed);
        target
            .bytes
            .store(progress.bytes.load(Ordering::Relaxed), Ordering::Relaxed);
        target
            .errors
            .store(progress.errors.load(Ordering::Relaxed), Ordering::Relaxed);
        target.set_current(
            progress
                .current
                .lock()
                .map(|c| c.clone())
                .unwrap_or_default(),
        );
        if finished.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Anfrage für `scan`.
#[derive(Debug, Clone)]
pub struct ScanRequest {
    pub target: TargetSpec,
    pub exclude: Vec<String>,
    pub summary_only: Vec<String>,
    pub no_default_excludes: bool,
    pub reset_index: bool,
    pub threads: Option<usize>,
    /// Vorlage für Soll/Ist (Name oder Pfad)
    pub template: Option<String>,
}

/// Ergebnis eines Scans für die Anzeige.
#[derive(Debug)]
pub struct ScanReport {
    pub root: PathBuf,
    pub outcome: ScanOutcome,
    pub index_file: PathBuf,
    /// Health-Score (0–100), nur bei vollständigem Lauf und geschriebener Momentaufnahme
    pub score: Option<u8>,
    /// „Health-Score 82 (…)“, wie `score`
    pub score_line: Option<String>,
    /// Die Momentaufnahme konnte nicht geschrieben werden
    pub history_warning: Option<String>,
    pub notes: Notes,
}

impl ScanReport {
    pub fn complete(&self) -> bool {
        !self.outcome.aborted
    }
}

/// Scannt eine Wurzel. Nimmt die Scan-Sperre; ein belegtes Lock wird zu `Error::Busy`.
pub fn scan(req: &ScanRequest, ctx: &OpCtx) -> Result<ScanReport> {
    let target = req.target.resolve()?;
    let root = resolve_root(&target.root, req.target.force(&target))?;
    let profile_name = target.profile_name().map(String::from);
    let template = load_template(req.template.as_deref(), &target)?;
    let mut config = target.config;
    config.apply_scan_options(
        &req.exclude,
        &req.summary_only,
        req.no_default_excludes,
        req.threads,
    );
    let mut notes = Notes::default();
    if !config.no_default_excludes {
        if let Some(area) = Classifier::root_in_default_area(&root, &DefaultPaths::from_env()) {
            notes.hint(format!(
                "Hinweis: {} liegt im Standard-Ausschlussbereich {}; der angegebene Pfad hat Vorrang.",
                paths::display(&root),
                paths::display(&area)
            ));
        }
    }

    let _lock = acquire_scan_lock()?;
    let res = run_scan(
        &ScanJob {
            root: &root,
            config: &config,
            reset_index: req.reset_index,
            profile: profile_name.as_deref(),
            template: template.as_ref(),
        },
        ctx,
    )?;
    Ok(res.report(&root, notes))
}
