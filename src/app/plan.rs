//! `plan …`-Befehle: Vorbereitung (Wurzel, Index, Schutzregeln), Speichern und Zusammenfassung
//! sind für alle Pläne gleich.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use bytesize::ByteSize;

use super::profile;
use super::{
    index_age_note, index_path, normalize, now_rfc3339, now_ticks, onedrive_roots_from_env,
    onedrive_warning, resolve_root,
};
use crate::analysis::age::parse_old_after;
use crate::change::archive::{plan_archive, ArchiveOptions};
use crate::change::dedupe::plan_dedupe;
use crate::change::empty_dirs::plan_empty_dirs;
use crate::change::junk::{plan_junk, JunkOptions};
use crate::change::limits;
use crate::change::plan::Plan;
use crate::change::protect::{ProtectPaths, Protector};
use crate::change::rules::{plan_rules, CachedExif, LiveContent, RulesEnv, RulesPlan};
use crate::change::versions::{plan_versions, VersionsOptions};
use crate::cli::{
    PlanArchiveArgs, PlanDedupeArgs, PlanEmptyDirsArgs, PlanJunkArgs, PlanRulesArgs,
    PlanVersionsArgs,
};
use crate::config::{Config, BUILTIN_JUNK_CATEGORIES};
use crate::index::Index;
use crate::paths;
use crate::platform::windows::downloads_dir;
use crate::report;
use crate::rules::RuleSet;

/// Alles, was jeder Planer braucht.
struct Prepared {
    root: PathBuf,
    config: Config,
    index: Index,
    protector: Protector,
}

/// Wurzel auflösen, Index öffnen und die Hinweise zu Index-Alter und OneDrive ausgeben.
fn prepare(path: Option<&Path>, profile: Option<&str>) -> Result<Prepared> {
    let (root, config) = start(path, profile)?;
    open_prepared(root, config)
}

/// Erster Teil von [`prepare`]: Wurzel und Config, ohne den Index zu öffnen.
fn start(path: Option<&Path>, profile: Option<&str>) -> Result<(PathBuf, Config)> {
    let target = profile::target(path, profile)?;
    Ok((resolve_root(&target.root, target.force())?, target.config))
}

fn open_prepared(root: PathBuf, config: Config) -> Result<Prepared> {
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
    let p = prepare(args.path.as_deref(), args.profile.as_deref())?;
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
pub(super) fn downloads_dirs(config: &Config) -> Vec<PathBuf> {
    if config.downloads_dirs.is_empty() {
        downloads_dir().into_iter().collect()
    } else {
        config
            .downloads_dirs
            .iter()
            .map(|d| normalize(Path::new(d)))
            .collect()
    }
}

pub(super) fn plan_junk_command(args: &PlanJunkArgs) -> Result<i32> {
    let p = prepare(args.path.as_deref(), args.profile.as_deref())?;
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

pub(super) fn plan_empty_dirs_command(args: &PlanEmptyDirsArgs) -> Result<i32> {
    let p = prepare(args.path.as_deref(), args.profile.as_deref())?;
    let result = plan_empty_dirs(&p.index, &p.root, &p.protector, &now_rfc3339())?;
    let plan = &result.plan;
    let headline = format!(
        "{} leere Ordner, {} übersprungen",
        plan.actions.len(),
        plan.skipped.len()
    );
    finish(plan, &p.config, args.out.as_ref(), &headline, &result.notes)
}

pub(super) fn plan_archive_command(args: &PlanArchiveArgs) -> Result<i32> {
    let p = prepare(args.path.as_deref(), args.profile.as_deref())?;
    let older_than = args
        .older_than
        .clone()
        .unwrap_or_else(|| p.config.archive_older_than.clone());
    let older_than_days = parse_old_after(&older_than).map_err(anyhow::Error::msg)?;
    let result = plan_archive(
        &p.index,
        &p.root,
        &p.protector,
        &now_rfc3339(),
        &ArchiveOptions {
            older_than: &older_than,
            older_than_days,
            now_ticks: now_ticks(),
        },
    )?;
    let plan = &result.plan;
    let headline = format!(
        "{} Ordner zum Archivieren, {} betroffen, {} übersprungen (älter als {older_than})",
        plan.actions.len(),
        ByteSize::b(result.bytes),
        plan.skipped.len(),
    );
    finish(plan, &p.config, args.out.as_ref(), &headline, &result.notes)
}

pub(super) fn plan_versions_command(args: &PlanVersionsArgs) -> Result<i32> {
    let p = prepare(args.path.as_deref(), args.profile.as_deref())?;
    let min_age = args
        .min_age
        .clone()
        .unwrap_or_else(|| p.config.versions_min_age.clone());
    let min_age_days = parse_old_after(&min_age).map_err(anyhow::Error::msg)?;
    let result = plan_versions(
        &p.index,
        &p.root,
        &p.protector,
        &now_rfc3339(),
        &VersionsOptions {
            min_age: &min_age,
            min_age_days,
            now_ticks: now_ticks(),
        },
    )?;
    let plan = &result.plan;
    let headline = format!(
        "{} ältere Versionen, {} betroffen, {} übersprungen (Mindestalter {min_age})",
        plan.actions.len(),
        ByteSize::b(result.bytes),
        plan.skipped.len(),
    );
    finish(plan, &p.config, args.out.as_ref(), &headline, &result.notes)
}

/// Pfad der Regeldatei: `--rules` vor `rules_file` der Config vor `rules.toml` im Config-Ordner.
fn resolve_rules_path(
    arg: Option<&Path>,
    config: &Config,
    config_dir: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(path) = arg {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = &config.rules_file {
        return Ok(PathBuf::from(path));
    }
    let dir = config_dir.context("Config-Ordner (%APPDATA%) nicht ermittelbar")?;
    Ok(dir.join("rules.toml"))
}

fn missing_rules_message(path: &Path) -> String {
    format!(
        "Regeldatei {} nicht gefunden. Lege sie an oder gib mit --rules eine andere an, \
         zum Beispiel:\n\n\
         [[rules]]\n\
         name   = \"pdf\"\n\
         ext    = [\"pdf\"]\n\
         target = \"Dokumente/\"\n\n\
         [[rules]]\n\
         name   = \"fotos\"\n\
         ext    = [\"jpg\", \"jpeg\"]\n\
         target = \"Fotos/{{exif.date:%Y}}/\"",
        paths::display(path)
    )
}

/// Kopfzeilen der Zusammenfassung: Summe, je Regel Anzahl und Beispiele, Dateien ohne Regel.
fn rules_headline(result: &RulesPlan) -> String {
    let plan = &result.plan;
    let mut lines = vec![format!(
        "{} Aktionen, {} betroffen, {} übersprungen",
        plan.actions.len(),
        ByteSize::b(result.bytes),
        plan.skipped.len()
    )];
    for rule in &result.per_rule {
        lines.push(format!(
            "  Regel „{}“: {} Dateien ({})",
            rule.name,
            rule.actions,
            ByteSize::b(rule.bytes)
        ));
        for (old, new) in &rule.examples {
            lines.push(format!("      {old} → {new}"));
        }
    }
    if result.unmatched > 0 {
        lines.push(format!(
            "  ohne passende Regel: {} Dateien",
            result.unmatched
        ));
    }
    lines.join("\n")
}

pub(super) fn plan_rules_command(args: &PlanRulesArgs) -> Result<i32> {
    // Die Regeldatei wird vollständig geprüft, bevor der Index geöffnet wird.
    let (root, config) = start(args.path.as_deref(), args.profile.as_deref())?;
    let rules_path = resolve_rules_path(
        args.rules.as_deref(),
        &config,
        paths::config_dir().as_deref(),
    )?;
    if !rules_path.is_file() {
        bail!("{}", missing_rules_message(&rules_path));
    }
    let mut rules = RuleSet::load(&rules_path)?;
    if !args.rule.is_empty() {
        rules = rules.select(&args.rule)?;
    }
    let mut p = open_prepared(root, config)?;
    let created = now_rfc3339();
    let content = LiveContent::default();
    let mut result = plan_rules(
        &mut p.index,
        &p.root,
        &rules,
        &RulesEnv {
            protector: &p.protector,
            exif: &CachedExif,
            content: &content,
            created: &created,
            now_ticks: now_ticks(),
        },
    )?;
    // Der Cache behält nur Einträge zu Dateien, die der Index noch kennt.
    let _ = p.index.exif_prune(&paths::dir_key(&p.root));
    if !args.rule.is_empty() {
        let names: Vec<&str> = rules.rules.iter().map(|r| r.name.as_str()).collect();
        result.plan.params.insert("rules".into(), names.join(","));
    }
    let headline = rules_headline(&result);
    finish(
        &result.plan,
        &p.config,
        args.out.as_ref(),
        &headline,
        &result.notes,
    )
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

    #[test]
    fn regeldatei_pfad_cli_vor_config_vor_default() {
        let with_file = config("rules_file = \"D:\\\\Regeln\\\\r.toml\"");
        let dir = Path::new(r"C:\Cfg");
        assert_eq!(
            resolve_rules_path(Some(Path::new(r"X:\a.toml")), &with_file, Some(dir)).unwrap(),
            PathBuf::from(r"X:\a.toml")
        );
        assert_eq!(
            resolve_rules_path(None, &with_file, Some(dir)).unwrap(),
            PathBuf::from(r"D:\Regeln\r.toml")
        );
        assert_eq!(
            resolve_rules_path(None, &Config::default(), Some(dir)).unwrap(),
            PathBuf::from(r"C:\Cfg\rules.toml")
        );
        assert!(resolve_rules_path(None, &Config::default(), None).is_err());
    }

    #[test]
    fn fehlende_regeldatei_nennt_den_pfad_und_ein_beispiel() {
        let text = missing_rules_message(Path::new(r"C:\Cfg\rules.toml"));
        assert!(text.contains(r"C:\Cfg\rules.toml"), "{text}");
        assert!(
            text.contains("[[rules]]") && text.contains("--rules"),
            "{text}"
        );
        assert!(text.contains("{exif.date:%Y}"), "{text}");
        // Das Beispiel muss selbst eine gültige Regeldatei sein.
        let example = text.split("\n\n").skip(1).collect::<Vec<_>>().join("\n\n");
        RuleSet::parse(&example, Path::new("beispiel.toml")).unwrap();
    }

    #[test]
    fn zusammenfassung_zeigt_regeln_beispiele_und_dateien_ohne_regel() {
        use crate::change::plan::{Plan, PlanKind, PLAN_VERSION};
        use crate::change::rules::RuleStats;
        let result = RulesPlan {
            plan: Plan {
                version: PLAN_VERSION,
                created: String::new(),
                kind: PlanKind::Rules,
                root: r"D:\Daten".into(),
                keep_strategy: None,
                params: Default::default(),
                actions: vec![],
                skipped: vec![],
            },
            bytes: 2048,
            per_rule: vec![
                RuleStats {
                    name: "fotos".into(),
                    actions: 2,
                    bytes: 2048,
                    examples: vec![("a.jpg".into(), "Fotos/a.jpg".into())],
                },
                RuleStats {
                    name: "leer".into(),
                    actions: 0,
                    bytes: 0,
                    examples: vec![],
                },
            ],
            unmatched: 5,
            notes: vec![],
        };
        let text = rules_headline(&result);
        assert!(
            text.contains("Regel „fotos“: 2 Dateien (2.0 KiB)"),
            "{text}"
        );
        assert!(text.contains("a.jpg → Fotos/a.jpg"), "{text}");
        assert!(text.contains("Regel „leer“: 0 Dateien"), "{text}");
        assert!(text.contains("ohne passende Regel: 5 Dateien"), "{text}");
    }
}
