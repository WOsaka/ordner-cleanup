//! `plan …`-Befehle: Vorbereitung (Wurzel, Index, Schutzregeln), Speichern und Zusammenfassung
//! sind für alle Pläne gleich.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use bytesize::ByteSize;

use super::{
    index_age_note, index_path, load_config, now_rfc3339, onedrive_roots_from_env,
    onedrive_warning, resolve_root,
};
use crate::change::dedupe::plan_dedupe;
use crate::change::junk::{plan_junk, JunkOptions};
use crate::change::limits;
use crate::change::plan::Plan;
use crate::change::protect::{ProtectPaths, Protector};
use crate::cli::{PlanDedupeArgs, PlanJunkArgs};
use crate::config::{Config, BUILTIN_JUNK_CATEGORIES};
use crate::index::Index;
use crate::paths;
use crate::platform::windows::downloads_dir;
use crate::report;
use crate::scan::source::TICKS_PER_SEC;

/// Alles, was jeder Planer braucht.
struct Prepared {
    root: PathBuf,
    config: Config,
    index: Index,
    protector: Protector,
}

/// Wurzel auflösen, Index öffnen und die Hinweise zu Index-Alter und OneDrive ausgeben.
fn prepare(path: &Path) -> Result<Prepared> {
    let root = resolve_root(path, false)?;
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
    Ok(Prepared {
        root,
        config,
        index,
        protector,
    })
}

fn now_ticks() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    secs * TICKS_PER_SEC
}

/// Speichert den Plan und gibt Zusammenfassung, Hinweise und Pfad aus.
fn finish(
    plan: &Plan,
    config: &Config,
    out: Option<&PathBuf>,
    headline: &str,
    notes: &[String],
) -> Result<i32> {
    let out = match out {
        Some(out) => out.clone(),
        None => PathBuf::from(format!(
            "plan-{}.json",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        )),
    };
    plan.save(&out)
        .with_context(|| format!("Plan-Datei {} nicht schreibbar", paths::display(&out)))?;

    println!("{headline}");
    let mut reasons = BTreeMap::new();
    for s in &plan.skipped {
        *reasons.entry(s.reason.to_string()).or_insert(0usize) += 1;
    }
    for (reason, count) in reasons {
        println!("  übersprungen: {count} × {reason}");
    }
    for note in notes {
        println!("Hinweis: {note}");
    }
    if limits::exceeds(plan, &onedrive_roots_from_env(), config).is_some() {
        println!(
            "Hinweis: Der Plan überschreitet die OneDrive-Obergrenze; `apply` braucht dafür \
             `--allow-large`."
        );
    }
    let shown = std::path::absolute(&out).unwrap_or(out);
    println!("Plan: {}", paths::display(&shown));
    println!("Es wurde nichts verändert. Plan prüfen, danach mit `apply` ausführen.");
    Ok(0)
}

pub(super) fn plan_dedupe_command(args: &PlanDedupeArgs) -> Result<i32> {
    let p = prepare(&args.path)?;
    let result = plan_dedupe(&p.index, &p.root, &args.keep, &p.protector, &now_rfc3339())?;
    let plan = &result.plan;
    let headline = format!(
        "{} Aktionen, {} freiwerdend, {} übersprungen (Strategie: {})",
        plan.actions.len(),
        ByteSize::b(result.freed_bytes),
        plan.skipped.len(),
        plan.keep_strategy.as_deref().unwrap_or("-")
    );
    finish(plan, &p.config, args.out.as_ref(), &headline, &[])
}

/// `--category` prüfen (eingebaut oder eigene Regel); ohne Angabe gilt die Config.
fn resolve_categories(requested: &[String], config: &Config) -> Result<Vec<String>> {
    if requested.is_empty() {
        return Ok(config.junk_categories.clone());
    }
    let custom: Vec<&str> = config.junk_rules.iter().map(|r| r.name.as_str()).collect();
    let mut categories: Vec<String> = Vec::new();
    for name in requested {
        let name = name.trim();
        if !BUILTIN_JUNK_CATEGORIES.contains(&name) && !custom.contains(&name) {
            bail!(
                "Unbekannte Kategorie '{name}' (eingebaut: {}; eigene: {})",
                BUILTIN_JUNK_CATEGORIES.join(", "),
                if custom.is_empty() {
                    "keine".to_string()
                } else {
                    custom.join(", ")
                }
            );
        }
        if !categories.iter().any(|c| c == name) {
            categories.push(name.to_string());
        }
    }
    Ok(categories)
}

/// Ordner, in denen `installer` greift: die Config ersetzt den Known Folder.
fn downloads_dirs(config: &Config) -> Vec<PathBuf> {
    if config.downloads_dirs.is_empty() {
        downloads_dir().into_iter().collect()
    } else {
        config.downloads_dirs.iter().map(PathBuf::from).collect()
    }
}

pub(super) fn plan_junk_command(args: &PlanJunkArgs) -> Result<i32> {
    let p = prepare(&args.path)?;
    let categories = resolve_categories(&args.category, &p.config)?;
    let result = plan_junk(
        &p.index,
        &p.root,
        &p.protector,
        &now_rfc3339(),
        &JunkOptions {
            categories: &categories,
            rules: &p.config.junk_rules,
            downloads_dirs: &downloads_dirs(&p.config),
            installer_min_age_days: p.config.installer_min_age_days(),
            now_ticks: now_ticks(),
        },
    )?;
    let plan = &result.plan;
    let headline = format!(
        "{} Aktionen, {} betroffen, {} übersprungen (Kategorien: {})",
        plan.actions.len(),
        ByteSize::b(result.bytes),
        plan.skipped.len(),
        categories.join(", ")
    );
    finish(plan, &p.config, args.out.as_ref(), &headline, &result.notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    #[test]
    fn ohne_category_gilt_die_config() {
        let c = config("junk_categories = [\"temp\"]");
        assert_eq!(resolve_categories(&[], &c).unwrap(), ["temp"]);
        assert_eq!(
            resolve_categories(&[], &Config::default()).unwrap(),
            ["system", "temp", "downloads", "installer"]
        );
    }

    #[test]
    fn category_akzeptiert_eingebaute_und_eigene_ohne_doppelte() {
        let c = config("[[junk_rules]]\nname = \"logs\"\npatterns = [\"*.log\"]");
        let requested: Vec<String> = ["temp", " logs ", "temp"].map(String::from).to_vec();
        assert_eq!(
            resolve_categories(&requested, &c).unwrap(),
            ["temp", "logs"]
        );
    }

    #[test]
    fn unbekannte_category_nennt_name_und_alternativen() {
        let c = config("[[junk_rules]]\nname = \"logs\"\npatterns = [\"*.log\"]");
        let err = resolve_categories(&["gibtsnicht".to_string()], &c).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("gibtsnicht"), "{text}");
        assert!(text.contains("system") && text.contains("logs"), "{text}");
    }

    #[test]
    fn downloads_ordner_aus_der_config_ersetzt_den_known_folder() {
        let c = config("downloads_dirs = [\"D:\\\\Eingang\", \"E:\\\\Mehr\"]");
        assert_eq!(
            downloads_dirs(&c),
            [PathBuf::from(r"D:\Eingang"), PathBuf::from(r"E:\Mehr")]
        );
        assert!(downloads_dirs(&Config::default()).len() <= 1);
    }
}
