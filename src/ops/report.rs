//! Berichtsmodell aus dem Index (für `report`, `run` und die Analyse-Ansicht der GUI) und der
//! Export in Dateien.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::target::{load_template, target};
use super::{load_config, normalize, now_rfc3339, now_ticks, Notes, OpCtx};
use crate::analysis::age::parse_old_after;
use crate::analysis::problems::ProblemCtx;
use crate::analysis::structure::Thresholds;
use crate::config::Config;
use crate::index::Index;
use crate::paths::{self, index_path};
use crate::report::{self, Format, Report, ReportParams, ReportRoot};
use crate::template::{self, Loaded};

/// Hinweise auf ausgefallene Läufe für den nächsten Bericht eines Profils.
pub fn missed_runs_notes(profile: &str) -> Vec<String> {
    paths::runs_log(profile)
        .ok()
        .and_then(|log| crate::runlog::missed_runs_note(&crate::runlog::read_all(&log)))
        .into_iter()
        .collect()
}

/// Baut das Berichtsmodell samt Abschnitt „Verlauf“ aus dem Index. `notes` landen im Verlauf;
/// ausgelassene Abschnitte kommen als Warnung zurück.
pub fn build_report(
    index: &Index,
    config: &Config,
    requested: Option<&Path>,
    template: Option<&Loaded>,
    notes: Vec<String>,
) -> Result<(Report, ReportRoot, Notes)> {
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
        partial_threshold: config.dedupe_dirs_partial_threshold,
    };
    let mut warnings = Notes::default();
    let mut model = report::build(index, &root, &params)?;
    if let Err(e) =
        super::snapshot::attach_history(&mut model, index, &root, config, template, notes)
    {
        warnings.warn(format!("Warnung: Abschnitt Verlauf ausgelassen: {e:#}"));
    }
    match report::content::build(index, &root.dir_key, config.classify.min_confidence as f32) {
        Ok(section) => model.content = Some(section),
        Err(e) => warnings.warn(format!("Warnung: Abschnitt Inhalte ausgelassen: {e:#}")),
    }
    Ok((model, root, warnings))
}

/// Anfrage für das Berichtsmodell.
#[derive(Debug, Clone, Default)]
pub struct ReportRequest {
    /// Gescannte Wurzel (optional, wenn nur eine im Index ist)
    pub path: Option<PathBuf>,
    pub profile: Option<String>,
    pub template: Option<String>,
    pub old_after: Option<String>,
    pub top: Option<usize>,
}

/// Berichtsmodell einer Wurzel.
#[derive(Debug)]
pub struct ReportView {
    pub model: Report,
    pub root: ReportRoot,
    pub notes: Notes,
}

/// Modell aus dem Index; liest nur. Die Anfrage wird wie bei `report` aufgelöst (Profil: dessen
/// Config und Wurzel, sonst die globale Config).
pub fn report_model(req: &ReportRequest, _ctx: &OpCtx) -> Result<ReportView> {
    let (mut config, requested, notes, template) = match &req.profile {
        Some(name) => {
            let target = target(None, Some(name))?;
            let notes = missed_runs_notes(name);
            let template = load_template(req.template.as_deref(), &target)?;
            (target.config, Some(target.root), notes, template)
        }
        None => {
            let template = match req.template.as_deref() {
                Some(spec) => Some(template::resolve(spec, None)?),
                None => None,
            };
            (load_config()?, req.path.clone(), Vec::new(), template)
        }
    };
    if let Some(old_after) = &req.old_after {
        config.old_after = old_after.clone();
    }
    if let Some(top) = req.top {
        config.top = top;
    }
    let index = Index::open(&index_path()?)?;
    let (model, root, notes) = build_report(
        &index,
        &config,
        requested.as_deref(),
        template.as_ref(),
        notes,
    )?;
    Ok(ReportView { model, root, notes })
}

/// Schreibt das Modell in den Zielordner (`out` oder `reports\<Zeitstempel>`).
pub fn export_report(
    view: &ReportView,
    formats: &[Format],
    out: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let dir = report::prepare_out_dir(out, &timestamp)?;
    report::write_all(&view.model, formats, &dir)
}
