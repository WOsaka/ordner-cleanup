use std::path::PathBuf;

use anyhow::{Context, Result};
use bytesize::ByteSize;

use super::{
    index_age_note, index_path, load_config, now_rfc3339, onedrive_roots_from_env,
    onedrive_warning, resolve_root,
};
use crate::change::dedupe::plan_dedupe;
use crate::change::protect::{ProtectPaths, Protector};
use crate::cli::PlanDedupeArgs;
use crate::index::Index;
use crate::paths;
use crate::report;

pub(super) fn plan_dedupe_command(args: &PlanDedupeArgs) -> Result<i32> {
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
