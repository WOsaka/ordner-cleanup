//! Befehl `classify` (Phase 6a): Terminal-Frontend über `ops::classify`.

use anyhow::Result;

use super::global_cancel_flag;
use super::progress::with_bar;
use crate::cli::ClassifyArgs;
use crate::ops::classify::{classify, render_summary, ClassifyOutcome, ClassifyRequest};
use crate::ops::target::TargetSpec;
use crate::ops::OpCtx;
use crate::paths;

pub(super) fn classify_command(args: &ClassifyArgs) -> Result<i32> {
    let target = match (&args.path, &args.profile) {
        (_, Some(name)) => TargetSpec::Profile(name.clone()),
        (Some(path), None) => TargetSpec::Path {
            path: path.clone(),
            force: false,
        },
        (None, None) => anyhow::bail!("Pfad oder --profile angeben"),
    };
    let ctx = OpCtx::new(global_cancel_flag()?);
    let outcome = with_bar(&ctx.progress, || {
        classify(
            &ClassifyRequest {
                target,
                no_llm: args.no_llm,
                force: args.force,
                ext: args.ext.clone(),
                only: args.only.clone(),
                clear: args.clear,
            },
            &ctx,
        )
    })?;
    match outcome {
        ClassifyOutcome::Cleared { root, entries } => {
            println!(
                "Inhalts- und OCR-Text-Cache von {} geleert ({entries} Einträge).",
                paths::display(&root)
            );
            Ok(0)
        }
        ClassifyOutcome::Classified { root, run, summary } => {
            print!("{}", render_summary(&paths::display(&root), &run, &summary));
            if run.aborted {
                eprintln!("Abgebrochen; bis dahin Analysiertes bleibt im Cache.");
                return Ok(1);
            }
            Ok(0)
        }
    }
}
