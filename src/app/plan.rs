//! `plan …`-Befehle: Terminal-Frontend über `ops::plan`. Die Zusammenfassung ist für alle Pläne
//! gleich.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{confirm, global_cancel_flag};
use crate::change::plan::{short_integrity, Seal, PLAN_VERSION};
use crate::cli::{
    PlanArchiveArgs, PlanDedupeArgs, PlanDedupeDirsArgs, PlanEmptyDirsArgs, PlanJunkArgs,
    PlanRulesArgs, PlanSealArgs, PlanVersionsArgs,
};
use crate::ops::plan::{
    plan, seal_preview, seal_write, PlanKindRequest, PlanOut, PlanOutcome, PlanRequest,
};
use crate::ops::target::TargetSpec;
use crate::ops::OpCtx;
use crate::paths;

fn target_spec(path: Option<&Path>, profile: Option<&str>) -> Result<TargetSpec> {
    match (profile, path) {
        (Some(name), None) => Ok(TargetSpec::Profile(name.to_string())),
        (Some(_), Some(_)) => anyhow::bail!("Pfad und --profile schließen sich aus"),
        (None, Some(path)) => Ok(TargetSpec::Path {
            path: path.to_path_buf(),
            force: false,
        }),
        (None, None) => anyhow::bail!("Pfad oder --profile angeben"),
    }
}

/// Erzeugt den Plan und gibt Zusammenfassung, Hinweise und Pfad aus.
fn run_plan(
    path: Option<&Path>,
    profile: Option<&str>,
    kind: PlanKindRequest,
    out: Option<&PathBuf>,
) -> Result<i32> {
    let ctx = OpCtx::new(global_cancel_flag()?);
    let outcome = plan(
        &PlanRequest {
            target: target_spec(path, profile)?,
            kind,
            out: match out {
                Some(path) => PlanOut::Path(path.clone()),
                None => PlanOut::Cwd,
            },
        },
        &ctx,
    )?;
    print_outcome(&outcome);
    Ok(0)
}

fn print_outcome(outcome: &PlanOutcome) {
    for line in outcome.prep.hints.iter().chain(&outcome.prep.warnings) {
        eprintln!("{line}");
    }
    println!("{}", outcome.headline);
    for (reason, count) in &outcome.skipped_by_reason {
        println!("  übersprungen: {count} × {reason}");
    }
    for note in &outcome.notes {
        println!("Hinweis: {note}");
    }
    if outcome.limit.is_some() {
        println!(
            "Hinweis: Der Plan überschreitet die OneDrive-Obergrenze; `apply` braucht dafür \
             `--allow-large`."
        );
    }
    if let Some(saved) = &outcome.saved {
        println!("Plan: {}", paths::display(saved));
    }
    if let Some(integrity) = &outcome.integrity {
        println!("Prüfsumme: sha256:{}…", short_integrity(integrity));
    }
    println!("Es wurde nichts verändert. Plan prüfen, danach mit `apply` ausführen.");
}

pub(super) fn plan_dedupe_command(args: &PlanDedupeArgs) -> Result<i32> {
    run_plan(
        args.path.as_deref(),
        args.profile.as_deref(),
        PlanKindRequest::Dedupe {
            keep: args.keep.clone(),
        },
        args.out.as_ref(),
    )
}

pub(super) fn plan_dedupe_dirs_command(args: &PlanDedupeDirsArgs) -> Result<i32> {
    run_plan(
        args.path.as_deref(),
        args.profile.as_deref(),
        PlanKindRequest::DedupeDirs {
            keep: args.keep.clone(),
            min_size: args.min_size,
        },
        args.out.as_ref(),
    )
}

pub(super) fn plan_junk_command(args: &PlanJunkArgs) -> Result<i32> {
    run_plan(
        args.path.as_deref(),
        args.profile.as_deref(),
        PlanKindRequest::Junk {
            categories: args.category.clone(),
        },
        args.out.as_ref(),
    )
}

pub(super) fn plan_empty_dirs_command(args: &PlanEmptyDirsArgs) -> Result<i32> {
    run_plan(
        args.path.as_deref(),
        args.profile.as_deref(),
        PlanKindRequest::EmptyDirs,
        args.out.as_ref(),
    )
}

pub(super) fn plan_archive_command(args: &PlanArchiveArgs) -> Result<i32> {
    run_plan(
        args.path.as_deref(),
        args.profile.as_deref(),
        PlanKindRequest::Archive {
            older_than: args.older_than.clone(),
        },
        args.out.as_ref(),
    )
}

pub(super) fn plan_versions_command(args: &PlanVersionsArgs) -> Result<i32> {
    run_plan(
        args.path.as_deref(),
        args.profile.as_deref(),
        PlanKindRequest::Versions {
            min_age: args.min_age.clone(),
        },
        args.out.as_ref(),
    )
}

pub(super) fn plan_rules_command(args: &PlanRulesArgs) -> Result<i32> {
    plan_rules_with(args, false)
}

/// `lock_held`: nur `run`, das die Scan-Sperre für den ganzen Lauf hält.
pub(super) fn plan_rules_with(args: &PlanRulesArgs, lock_held: bool) -> Result<i32> {
    run_plan(
        args.path.as_deref(),
        args.profile.as_deref(),
        PlanKindRequest::Rules {
            rules_file: args.rules.clone(),
            rules_text: None,
            only: args.rule.clone(),
            no_classify: args.no_classify,
            lock_held,
        },
        args.out.as_ref(),
    )
}

/// `plan seal`: zeigt, was versiegelt wird, fragt nach und schreibt die neue Prüfsumme.
pub(super) fn plan_seal_command(args: &PlanSealArgs) -> Result<i32> {
    let preview = seal_preview(&args.plan)?;
    let new = short_integrity(&preview.integrity);
    let before = match &preview.seal {
        Seal::Valid => {
            println!("Plan ist bereits versiegelt (sha256:{new}…).");
            return Ok(0);
        }
        Seal::Unprotected { version } => {
            format!("keine Prüfsumme (Format {version}), wird auf Format {PLAN_VERSION} gehoben")
        }
        Seal::Missing => "Prüfsumme fehlt".into(),
        Seal::Mismatch { stored, .. } => format!(
            "sha256:{}… (passt nicht zum Inhalt)",
            short_integrity(stored)
        ),
    };
    let plan = &preview.plan;
    let kind = serde_json::to_value(plan.kind)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    println!("Plan {}", paths::display(&args.plan));
    println!("  Art {kind}, Wurzel {}", plan.root);
    println!(
        "  {} Aktionen, {} übersprungen",
        plan.actions.len(),
        plan.skipped.len()
    );
    println!("  Bisher: {before}");
    println!("  Neu: sha256:{new}…");
    if !confirm("Plan mit diesem Inhalt versiegeln? [j/N] ", args.yes)? {
        println!("Abgebrochen. Die Datei wurde nicht verändert.");
        return Ok(1);
    }
    seal_write(&args.plan, plan)?;
    println!("Plan versiegelt: {}", paths::display(&args.plan));
    Ok(0)
}
