//! `plan …`: Vorbereitung (Wurzel, Index, Schutzregeln), Planer, Speichern und Zusammenfassung
//! sind für alle Pläne gleich. Das Ergebnis ist ein strukturierter `PlanOutcome`; Ausgabe und
//! Rückfragen liegen bei CLI bzw. GUI.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use bytesize::ByteSize;

use super::classify;
use super::target::TargetSpec;
use super::{
    downloads_dirs, index_age_note, index_path, now_rfc3339, now_ticks, onedrive_roots_from_env,
    onedrive_warning, resolve_root, Notes, OpCtx,
};
use crate::analysis::age::parse_old_after;
use crate::change::archive::{plan_archive, ArchiveOptions};
use crate::change::dedupe::{plan_dedupe, KeepStrategy};
use crate::change::empty_dirs::plan_empty_dirs;
use crate::change::junk::{plan_junk, JunkOptions};
use crate::change::limits;
use crate::change::plan::Plan;
use crate::change::protect::{ProtectPaths, Protector};
use crate::change::rules::{
    plan_rules, CachedExif, ContentLookup, LiveContent, NoContent, RuleStats, RulesEnv, RulesPlan,
};
use crate::change::versions::{plan_versions, VersionsOptions};
use crate::config::{Config, BUILTIN_JUNK_CATEGORIES};
use crate::content::classify::Classifier;
use crate::index::Index;
use crate::paths;
use crate::report;
use crate::rules::{KnownCategories, RuleSet};

/// Art des Plans samt Optionen, wie die CLI-Variante sie kennt.
#[derive(Debug, Clone)]
pub enum PlanKindRequest {
    Dedupe {
        keep: KeepStrategy,
    },
    Junk {
        /// Leer = `junk_categories` der Config
        categories: Vec<String>,
    },
    EmptyDirs,
    Archive {
        older_than: Option<String>,
    },
    Versions {
        min_age: Option<String>,
    },
    Rules {
        /// Regeldatei (sonst `rules_file` der Config bzw. `rules.toml` im Config-Ordner)
        rules_file: Option<PathBuf>,
        /// Regeltext statt Datei (Entwurf des Editors); hat Vorrang vor `rules_file`
        rules_text: Option<String>,
        /// Nur diese Regeln
        only: Vec<String>,
        /// Fehlende Inhalte nicht nachklassifizieren
        no_classify: bool,
    },
}

/// Wohin der Plan geschrieben wird.
#[derive(Debug, Clone)]
pub enum PlanOut {
    /// `plan-<Zeitstempel>.json` im aktuellen Ordner (CLI-Default)
    Cwd,
    /// Im GUI-Planordner (`[gui] plans_dir` bzw. `plans\_gui`)
    GuiDir,
    Path(PathBuf),
    /// Nicht speichern (Vorschau)
    DontSave,
}

#[derive(Debug, Clone)]
pub struct PlanRequest {
    pub target: TargetSpec,
    pub kind: PlanKindRequest,
    pub out: PlanOut,
}

/// Kennzahlen eines Regel-Plans.
#[derive(Debug, Clone)]
pub struct RulesSummary {
    pub per_rule: Vec<RuleStats>,
    pub unmatched: usize,
    pub review: usize,
    pub not_classified: usize,
}

#[derive(Debug)]
pub struct PlanOutcome {
    pub plan: Plan,
    /// Pfad der Plan-Datei (nicht bei `PlanOut::DontSave`)
    pub saved: Option<PathBuf>,
    /// Betroffene Bytes (`dedupe`: freiwerdende; bei `remove-dir` 0)
    pub bytes: u64,
    /// Kopfzeile der Zusammenfassung (bei Regeln mehrzeilig)
    pub headline: String,
    /// Übersprungene Einträge je Grund
    pub skipped_by_reason: BTreeMap<String, usize>,
    pub rules: Option<RulesSummary>,
    /// Hinweise des Planers („Hinweis: …“ in der Zusammenfassung)
    pub notes: Vec<String>,
    /// Meldungen der Vorbereitung (Index-Alter, OneDrive) und des Nachklassifizierens
    pub prep: Notes,
    /// Meldung, wenn der Plan die OneDrive-Obergrenze überschreitet
    pub limit: Option<String>,
}

/// Alles, was jeder Planer braucht.
struct Prepared {
    root: PathBuf,
    config: Config,
    index: Index,
    protector: Protector,
}

/// Wurzel und Config, ohne den Index zu öffnen.
fn start(target: &TargetSpec) -> Result<(PathBuf, Config)> {
    let resolved = target.resolve()?;
    Ok((
        resolve_root(&resolved.root, target.force(&resolved))?,
        resolved.config,
    ))
}

fn open_prepared(root: PathBuf, config: Config, prep: &mut Notes) -> Result<Prepared> {
    let index = Index::open(&index_path()?)?;
    let scanned = report::select_root(&index, Some(&root))?;
    if let Some(note) = index_age_note(&scanned, chrono::Utc::now()) {
        prep.hint(note);
    }
    if let Some(warning) = onedrive_warning(&root, &onedrive_roots_from_env()) {
        prep.warn(warning);
    }
    let protector = Protector::new(&root, &config, &ProtectPaths::from_env());
    Ok(Prepared {
        root,
        config,
        index,
        protector,
    })
}

/// Speichert den Plan (je nach `out`) und fasst ihn zusammen.
fn finish(
    plan: Plan,
    config: &Config,
    out: &PlanOut,
    headline: String,
    bytes: u64,
    notes: Vec<String>,
    prep: Notes,
) -> Result<PlanOutcome> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let file = match out {
        PlanOut::Cwd => Some(PathBuf::from(format!("plan-{stamp}.json"))),
        PlanOut::GuiDir => {
            let dir = paths::gui_plans_dir(config.gui.plans_dir.as_deref())?;
            std::fs::create_dir_all(paths::extended(&dir))
                .with_context(|| format!("Plan-Ordner {} nicht anlegbar", paths::display(&dir)))?;
            let kind = serde_json::to_value(plan.kind)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();
            Some(dir.join(format!("plan-{stamp}-{kind}.json")))
        }
        PlanOut::Path(path) => Some(path.clone()),
        PlanOut::DontSave => None,
    };
    let saved = match file {
        Some(file) => {
            plan.save(&file).with_context(|| {
                format!("Plan-Datei {} nicht schreibbar", paths::display(&file))
            })?;
            Some(std::path::absolute(&file).unwrap_or(file))
        }
        None => None,
    };
    let mut skipped_by_reason = BTreeMap::new();
    for s in &plan.skipped {
        *skipped_by_reason
            .entry(s.reason.to_string())
            .or_insert(0usize) += 1;
    }
    let limit = limits::exceeds(&plan, &onedrive_roots_from_env(), config);
    Ok(PlanOutcome {
        plan,
        saved,
        bytes,
        headline,
        skipped_by_reason,
        rules: None,
        notes,
        prep,
        limit,
    })
}

/// Erzeugt einen Plan. Verändert nichts im Ordner; schreibt höchstens die Plan-Datei.
pub fn plan(req: &PlanRequest, ctx: &OpCtx) -> Result<PlanOutcome> {
    let mut prep = Notes::default();
    match &req.kind {
        PlanKindRequest::Rules {
            rules_file,
            rules_text,
            only,
            no_classify,
        } => plan_rules_kind(
            req,
            rules_file.as_deref(),
            rules_text.as_deref(),
            only,
            *no_classify,
            ctx,
            prep,
        ),
        kind => {
            let (root, config) = start(&req.target)?;
            let p = open_prepared(root, config, &mut prep)?;
            plan_cleanup(req, kind, p, prep)
        }
    }
}

fn plan_cleanup(
    req: &PlanRequest,
    kind: &PlanKindRequest,
    p: Prepared,
    prep: Notes,
) -> Result<PlanOutcome> {
    match kind {
        PlanKindRequest::Dedupe { keep } => {
            let result = plan_dedupe(&p.index, &p.root, keep, &p.protector, &now_rfc3339())?;
            let plan = result.plan;
            let headline = format!(
                "{} Aktionen, {} freiwerdend, {} übersprungen (Strategie: {})",
                plan.actions.len(),
                ByteSize::b(result.freed_bytes),
                plan.skipped.len(),
                plan.keep_strategy.as_deref().unwrap_or("-")
            );
            finish(
                plan,
                &p.config,
                &req.out,
                headline,
                result.freed_bytes,
                Vec::new(),
                prep,
            )
        }
        PlanKindRequest::Junk { categories } => {
            let categories = resolve_categories(categories, &p.config)?;
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
            let headline = format!(
                "{} Aktionen, {} betroffen, {} übersprungen (Kategorien: {})",
                result.plan.actions.len(),
                ByteSize::b(result.bytes),
                result.plan.skipped.len(),
                categories.join(", ")
            );
            finish(
                result.plan,
                &p.config,
                &req.out,
                headline,
                result.bytes,
                result.notes,
                prep,
            )
        }
        PlanKindRequest::EmptyDirs => {
            let result = plan_empty_dirs(&p.index, &p.root, &p.protector, &now_rfc3339())?;
            let headline = format!(
                "{} leere Ordner, {} übersprungen",
                result.plan.actions.len(),
                result.plan.skipped.len()
            );
            finish(
                result.plan,
                &p.config,
                &req.out,
                headline,
                result.bytes,
                result.notes,
                prep,
            )
        }
        PlanKindRequest::Archive { older_than } => {
            let older_than = older_than
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
            let headline = format!(
                "{} Ordner zum Archivieren, {} betroffen, {} übersprungen (älter als {older_than})",
                result.plan.actions.len(),
                ByteSize::b(result.bytes),
                result.plan.skipped.len(),
            );
            finish(
                result.plan,
                &p.config,
                &req.out,
                headline,
                result.bytes,
                result.notes,
                prep,
            )
        }
        PlanKindRequest::Versions { min_age } => {
            let min_age = min_age
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
            let headline = format!(
                "{} ältere Versionen, {} betroffen, {} übersprungen (Mindestalter {min_age})",
                result.plan.actions.len(),
                ByteSize::b(result.bytes),
                result.plan.skipped.len(),
            );
            finish(
                result.plan,
                &p.config,
                &req.out,
                headline,
                result.bytes,
                result.notes,
                prep,
            )
        }
        PlanKindRequest::Rules { .. } => unreachable!("Regeln laufen über plan_rules_kind"),
    }
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
    if result.review > 0 {
        lines.push(format!(
            "  zum Prüfen (Kategorie zu unsicher, keine Aktion): {} Dateien",
            result.review
        ));
    }
    if result.not_classified > 0 {
        lines.push(format!(
            "  nicht klassifiziert (keine Aktion): {} Dateien",
            result.not_classified
        ));
    }
    lines.join("\n")
}

/// `plan rules`: Regeldatei bzw. -entwurf, Kategorien und Nachklassifizieren.
fn plan_rules_kind(
    req: &PlanRequest,
    rules_file: Option<&Path>,
    rules_text: Option<&str>,
    only: &[String],
    no_classify: bool,
    ctx: &OpCtx,
    mut prep: Notes,
) -> Result<PlanOutcome> {
    // Die Regeldatei wird vollständig geprüft, bevor der Index geöffnet wird.
    let (root, config) = start(&req.target)?;
    let rules_path = match rules_text {
        Some(_) => PathBuf::from("Regel-Entwurf"),
        None => {
            let path = resolve_rules_path(rules_file, &config, paths::config_dir().as_deref())?;
            if !path.is_file() {
                bail!("{}", missing_rules_message(&path));
            }
            path
        }
    };
    // Kategorien zuerst: Fehler in der categories.toml brechen vor den Regeln ab, aber nur,
    // wenn Regeln Inhalte brauchen (Regeln ohne Inhaltsbedingung kommen ohne sie aus).
    let defs = classify::load_defs(&config);
    let known = match &defs {
        Ok(d) => KnownCategories::Names(d.names()),
        Err(e) => KnownCategories::Unavailable(format!("{e:#}")),
    };
    let mut rules = match rules_text {
        Some(text) => RuleSet::parse(text, &rules_path, &known)?,
        None => RuleSet::load(&rules_path, &known)?,
    };
    if !only.is_empty() {
        rules = rules.select(only)?;
    }
    let uses_content = rules
        .rules
        .iter()
        .any(|r| r.needs_content() || r.needs_text());
    let classifier = match defs {
        Ok(d) => Some(Classifier::new(d)),
        Err(e) if uses_content => return Err(e),
        Err(_) => None,
    };
    let mut p = open_prepared(root, config, &mut prep)?;
    let created = now_rfc3339();
    let content = LiveContent::default();
    let lookup = classifier.as_ref().map(|c| classify::CachedLookup {
        root: &p.root,
        config: &p.config,
        classifier: c,
        classify: !no_classify,
        ctx,
        notes: Default::default(),
        log: Default::default(),
    });
    let live_text = classify::LiveText { config: &p.config };
    let no_content = NoContent;
    let content_lookup: &dyn ContentLookup = match &lookup {
        Some(l) => l,
        None => &no_content,
    };
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
            content_lookup,
            text: &live_text,
            min_confidence: p.config.classify.min_confidence as f32,
        },
    )?;
    if let Some(l) = &lookup {
        result
            .notes
            .extend(l.notes.lock().map(|n| n.clone()).unwrap_or_default());
        if let Ok(log) = l.log.lock() {
            prep.hints.extend(log.hints.iter().cloned());
            prep.warnings.extend(log.warnings.iter().cloned());
        }
    }
    // Der Cache behält nur Einträge zu Dateien, die der Index noch kennt.
    let _ = p.index.exif_prune(&paths::dir_key(&p.root));
    if !only.is_empty() {
        let names: Vec<&str> = rules.rules.iter().map(|r| r.name.as_str()).collect();
        result.plan.params.insert("rules".into(), names.join(","));
    }
    let headline = rules_headline(&result);
    let summary = RulesSummary {
        per_rule: result.per_rule.clone(),
        unmatched: result.unmatched,
        review: result.review,
        not_classified: result.not_classified,
    };
    let mut outcome = finish(
        result.plan,
        &p.config,
        &req.out,
        headline,
        result.bytes,
        result.notes,
        prep,
    )?;
    outcome.rules = Some(summary);
    Ok(outcome)
}

/// Teilplan neben dem Original speichern: `<stem>-auswahl-<Zeitstempel>.json`, sonst im
/// GUI-Planordner. Das Original bleibt unverändert; `params` nennt Herkunft und Anzahl.
pub fn save_subset(
    original: &Path,
    plan: &Plan,
    keep: &HashSet<u32>,
    config: &Config,
) -> Result<PathBuf> {
    let mut subset = plan.subset(keep);
    let name = original
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    subset.params.insert("auswahl_von".into(), name);
    subset.params.insert(
        "auswahl".into(),
        format!("{}/{}", subset.actions.len(), plan.actions.len()),
    );
    subset.validate()?;
    let stem = original
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "plan".into());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let file_name = format!("{stem}-auswahl-{stamp}.json");
    let dir = match original.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) if dir.is_dir() => dir.to_path_buf(),
        _ => {
            let dir = paths::gui_plans_dir(config.gui.plans_dir.as_deref())?;
            std::fs::create_dir_all(paths::extended(&dir))
                .with_context(|| format!("Plan-Ordner {} nicht anlegbar", paths::display(&dir)))?;
            dir
        }
    };
    let file = dir.join(file_name);
    subset
        .save(&file)
        .with_context(|| format!("Plan-Datei {} nicht schreibbar", paths::display(&file)))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::downloads_dirs;

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
        RuleSet::parse(
            &example,
            Path::new("beispiel.toml"),
            &KnownCategories::builtin(),
        )
        .unwrap();
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
            review: 0,
            not_classified: 0,
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

    fn sample_plan(root: &Path) -> Plan {
        use crate::change::plan::{ActionType, PlanKind, PlannedAction, PLAN_VERSION};
        let action = |id: u32| PlannedAction {
            id,
            action: ActionType::Quarantine,
            path: root.join(format!("x{id}.tmp")).display().to_string(),
            size: 1,
            mtime_ticks: 0,
            mtime: String::new(),
            hash: None,
            keep: None,
            keep_hash: None,
            reason: "junk:temp".into(),
            target: None,
            is_dir: false,
            files: None,
            rule: None,
        };
        Plan {
            version: PLAN_VERSION,
            created: "2026-10-05T10:00:00+02:00".into(),
            kind: PlanKind::Junk,
            root: root.display().to_string(),
            keep_strategy: None,
            params: Default::default(),
            actions: (1..=5).map(action).collect(),
            skipped: vec![],
        }
    }

    #[test]
    fn teilplan_liegt_neben_dem_original_und_das_original_bleibt_bytegleich() {
        let dir = tempfile::tempdir().unwrap();
        let plan = sample_plan(dir.path());
        let original = dir.path().join("plan-20261005-101500.json");
        plan.save(&original).unwrap();
        let before = std::fs::read(&original).unwrap();

        let keep: HashSet<u32> = [1, 2, 3].into();
        let file = save_subset(&original, &plan, &keep, &Config::default()).unwrap();

        assert_eq!(file.parent(), original.parent());
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("plan-20261005-101500-auswahl-") && name.ends_with(".json"),
            "{name}"
        );
        let loaded = Plan::load(&file).unwrap();
        assert_eq!(loaded.actions.len(), 3);
        assert_eq!(loaded.params["auswahl"], "3/5");
        assert_eq!(loaded.params["auswahl_von"], "plan-20261005-101500.json");
        assert_eq!(std::fs::read(&original).unwrap(), before);
    }

    #[test]
    fn teilplan_ohne_gueltigen_ordner_geht_in_den_gui_planordner() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gui-plans");
        let config = config(&format!(
            "[gui]
plans_dir = '{}'
",
            target.display()
        ));
        let plan = sample_plan(dir.path());
        let keep: HashSet<u32> = [1].into();
        let file = save_subset(Path::new("plan.json"), &plan, &keep, &config).unwrap();
        assert_eq!(file.parent(), Some(target.as_path()));
        assert_eq!(Plan::load(&file).unwrap().actions.len(), 1);
    }
}
