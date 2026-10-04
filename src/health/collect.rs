//! Kennzahlen direkt aus dem Index: lädt dieselben Zeilen wie der Bericht und ruft [`compute`] auf.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{compute, MetricsCtx, Rows, Snapshot};
use crate::analysis::age::parse_old_after;
use crate::analysis::problems::ProblemCtx;
use crate::analysis::structure::Thresholds;
use crate::change::junk::{JunkClassifier, JunkOptions};
use crate::config::Config;
use crate::index::Index;
use crate::report::rows::{load_dirs, load_errors, load_files_with};
use crate::report::ReportRoot;
use crate::template::check::{check, CheckResult};
use crate::template::Template;

/// Alles außer Index und Wurzel, was die Kennzahlen bestimmt.
pub struct CollectEnv<'a> {
    pub config: &'a Config,
    /// Ordner, in denen die Müll-Kategorie `installer` greift
    pub downloads_dirs: &'a [PathBuf],
    pub now_ticks: i64,
    pub template: Option<&'a Template>,
}

/// Momentaufnahme und, falls eine Vorlage gesetzt ist, der Soll/Ist-Abgleich dazu.
pub struct Collected {
    pub snapshot: Snapshot,
    pub template: Option<CheckResult>,
}

pub fn collect(index: &Index, root: &ReportRoot, env: &CollectEnv) -> Result<Collected> {
    let config = env.config;
    let files = load_files_with(index, &root.dir_key, false)?;
    let dirs = load_dirs(index, &root.dir_key)?;
    let errors = load_errors(index, &root.dir_key)?;
    let dups = index.exact_duplicate_groups(&root.dir_key)?;
    let junk = JunkClassifier::new(
        &JunkOptions {
            categories: &config.junk_categories,
            rules: &config.junk_rules,
            downloads_dirs: env.downloads_dirs,
            installer_min_age_days: config.installer_min_age_days(),
            now_ticks: env.now_ticks,
        },
        Path::new(&root.path),
    );
    let problem_ctx = ProblemCtx::from_env(&config.onedrive_conflict_hostnames);
    let thresholds = Thresholds {
        max_depth: config.max_depth_warning,
        huge_entries: config.huge_dir_entries,
    };
    let old_after_days = parse_old_after(&config.old_after).map_err(anyhow::Error::msg)?;
    let template = env.template.map(|t| check(&files, &dirs, root, t));
    let snapshot = compute(
        &Rows {
            files: &files,
            dirs: &dirs,
            errors: &errors,
        },
        &dups,
        root,
        &MetricsCtx {
            junk: &junk,
            problem_ctx: &problem_ctx,
            thresholds: &thresholds,
            old_after_days,
            now_ticks: env.now_ticks,
            weights: &config.health.weights,
            template: template.as_ref().map(|t| &t.by_bucket),
        },
    );
    Ok(Collected { snapshot, template })
}
