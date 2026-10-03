//! Planer für Müll (`plan junk`): liest nur den Index und verändert nichts.
//!
//! Kategorien: `system`, `temp`, `downloads`, `installer` (nur direkt in Downloads-Ordnern und
//! ab einem Mindestalter) sowie eigene Regeln aus der Config. Alles wird als `quarantine`
//! ohne Hash geplant; `apply` prüft Größe und mtime.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use super::plan::{
    ticks_to_rfc3339, ActionType, CleanupPlan, Plan, PlanKind, PlannedAction, Skipped, PLAN_VERSION,
};
use super::protect::Protector;
use super::quarantine;
use super::SkipReason;
use crate::analysis::age::is_old;
use crate::config::JunkRule;
use crate::index::{FileRow, Index, IndexError};
use crate::paths;

pub struct JunkOptions<'a> {
    /// Aktive Kategorien in Prüfreihenfolge (eingebaute und Namen eigener Regeln).
    pub categories: &'a [String],
    pub rules: &'a [JunkRule],
    /// Ordner, in denen `installer` greift (Known Folder oder Config).
    pub downloads_dirs: &'a [PathBuf],
    pub installer_min_age_days: i64,
    pub now_ticks: i64,
}

const SYSTEM_PATTERNS: [&str; 3] = ["Thumbs.db", "ehthumbs.db", ".DS_Store"];
const TEMP_PATTERNS: [&str; 2] = ["*.tmp", "~$*"];
const DOWNLOADS_PATTERNS: [&str; 3] = ["*.crdownload", "*.part", "*.partial"];

/// Der Name gehört zu einer eingebauten Müll-Kategorie (ohne `installer` und eigene Regeln).
/// `plan empty-dirs` nutzt das, um auf Ordner hinzuweisen, die nur Müll enthalten.
pub fn is_builtin_junk_name(name: &str) -> bool {
    static SET: std::sync::OnceLock<GlobSet> = std::sync::OnceLock::new();
    SET.get_or_init(|| {
        let all: Vec<&str> = SYSTEM_PATTERNS
            .iter()
            .chain(&TEMP_PATTERNS)
            .chain(&DOWNLOADS_PATTERNS)
            .copied()
            .collect();
        globs(&all)
    })
    .is_match(name)
}

/// Eine aktive Kategorie: Namensmuster, optional auf Ordner beschränkt, optional mit Mindestalter.
struct Category {
    name: String,
    names: GlobSet,
    /// Nur Dateien direkt in diesen Ordnern (Ordner-Schlüssel).
    only_in: Option<HashSet<String>>,
    min_age_days: Option<i64>,
}

fn globs(patterns: &[&str]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        // Eingebaute Muster sind gültig; eigene prüft die Config schon beim Laden, ein
        // trotzdem ungültiges Muster trifft schlicht nichts.
        if let Ok(glob) = GlobBuilder::new(pattern).case_insensitive(true).build() {
            builder.add(glob);
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

fn dir_keys(dirs: impl IntoIterator<Item = impl AsRef<Path>>) -> HashSet<String> {
    dirs.into_iter()
        .map(|d| paths::dir_key(d.as_ref()))
        .collect()
}

fn category(name: &str, options: &JunkOptions, downloads: &HashSet<String>) -> Option<Category> {
    let builtin = |patterns: &[&str]| Category {
        name: name.to_string(),
        names: globs(patterns),
        only_in: None,
        min_age_days: None,
    };
    match name {
        "system" => Some(builtin(&SYSTEM_PATTERNS)),
        "temp" => Some(builtin(&TEMP_PATTERNS)),
        "downloads" => Some(builtin(&DOWNLOADS_PATTERNS)),
        "installer" => Some(Category {
            only_in: Some(downloads.clone()),
            min_age_days: Some(options.installer_min_age_days),
            ..builtin(&["*.exe", "*.msi"])
        }),
        custom => {
            let rule = options.rules.iter().find(|r| r.name == custom)?;
            let patterns: Vec<&str> = rule.patterns.iter().map(String::as_str).collect();
            Some(Category {
                only_in: (!rule.dirs.is_empty()).then(|| dir_keys(&rule.dirs)),
                min_age_days: rule
                    .min_age
                    .as_deref()
                    .and_then(|a| crate::analysis::age::parse_old_after(a).ok()),
                ..builtin(&patterns)
            })
        }
    }
}

pub fn plan_junk(
    index: &Index,
    root: &Path,
    protector: &Protector,
    created: &str,
    options: &JunkOptions,
) -> Result<CleanupPlan, IndexError> {
    let root_key = paths::dir_key(root);
    let downloads: HashSet<String> = dir_keys(options.downloads_dirs)
        .into_iter()
        .filter(|k| paths::is_under(k, &root_key))
        .collect();
    let categories: Vec<Category> = options
        .categories
        .iter()
        .filter_map(|name| category(name, options, &downloads))
        .collect();

    let mut notes = Vec::new();
    let installer_active = options.categories.iter().any(|c| c == "installer");
    if installer_active && downloads.is_empty() {
        notes.push(format!(
            "Kategorie installer: kein Downloads-Ordner unter {} gefunden \
             (Known Folder oder downloads_dirs); es werden keine Installer geplant.",
            paths::display(root)
        ));
    }

    let mut actions: Vec<(String, PlannedAction)> = Vec::new();
    let mut skipped: Vec<(String, Skipped)> = Vec::new();
    for file in index.files_under(&root_key)? {
        let Some(cat) = match_category(&categories, &file) else {
            continue;
        };
        let path = Path::new(&file.path);
        let skip = if protector.check(path).is_some() {
            Some(SkipReason::Protected)
        } else if file.cloud_only {
            Some(SkipReason::CloudPlaceholder)
        } else if file.is_link {
            Some(SkipReason::Link)
        } else if cat
            .min_age_days
            .is_some_and(|days| !is_old(file.mtime, options.now_ticks, days))
        {
            Some(SkipReason::TooRecent)
        } else {
            quarantine::fits(root, path).err()
        };
        if let Some(reason) = skip {
            skipped.push((
                file.key.clone(),
                Skipped {
                    path: file.path.clone(),
                    reason,
                },
            ));
            continue;
        }
        actions.push((
            file.key.clone(),
            PlannedAction {
                id: 0,
                action: ActionType::Quarantine,
                path: file.path.clone(),
                size: file.size.max(0) as u64,
                mtime_ticks: file.mtime,
                mtime: ticks_to_rfc3339(file.mtime),
                hash: None,
                keep: None,
                keep_hash: None,
                reason: format!("junk:{}", cat.name),
                target: None,
                is_dir: false,
                files: None,
            },
        ));
    }

    actions.sort_by(|a, b| a.0.cmp(&b.0));
    skipped.sort_by(|a, b| a.0.cmp(&b.0));
    let actions: Vec<PlannedAction> = actions
        .into_iter()
        .zip(1u32..)
        .map(|((_, mut a), id)| {
            a.id = id;
            a
        })
        .collect();
    let bytes = actions.iter().map(|a| a.size).sum();

    let mut params = BTreeMap::new();
    params.insert("categories".to_string(), options.categories.join(","));
    if installer_active {
        params.insert(
            "installer_min_age".to_string(),
            format!("{}d", options.installer_min_age_days),
        );
    }
    Ok(CleanupPlan {
        plan: Plan {
            version: PLAN_VERSION,
            created: created.to_string(),
            kind: PlanKind::Junk,
            root: paths::display(root),
            keep_strategy: None,
            params,
            actions,
            skipped: skipped.into_iter().map(|(_, s)| s).collect(),
        },
        bytes,
        notes,
    })
}

/// Erste aktive Kategorie, deren Muster und Ordner auf die Datei passen.
fn match_category<'a>(categories: &'a [Category], file: &FileRow) -> Option<&'a Category> {
    categories.iter().find(|c| {
        c.names.is_match(&file.name)
            && c.only_in
                .as_ref()
                .is_none_or(|dirs| dirs.contains(&file.dir_key))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::plan::ActionType;
    use crate::change::protect::ProtectPaths;
    use crate::change::SkipReason;
    use crate::config::Config;
    use crate::index::FileRecord;
    use crate::paths;
    use crate::scan::source::TICKS_PER_SEC;

    const ROOT: &str = r"Z:\Root";
    const DAY: i64 = 86_400 * TICKS_PER_SEC;
    const NOW: i64 = 4_000 * DAY;

    struct Seed {
        path: String,
        age_days: i64,
        cloud: bool,
        link: bool,
    }

    fn f(path: &str) -> Seed {
        Seed {
            path: path.into(),
            age_days: 400,
            cloud: false,
            link: false,
        }
    }

    impl Seed {
        fn age(mut self, days: i64) -> Self {
            self.age_days = days;
            self
        }
        fn cloud(mut self) -> Self {
            self.cloud = true;
            self
        }
        fn link(mut self) -> Self {
            self.link = true;
            self
        }
    }

    fn seed(files: &[Seed]) -> Index {
        let mut index = Index::open_in_memory().unwrap();
        let dir_key = paths::dir_key(Path::new(ROOT));
        let run = index.begin_root(ROOT, &dir_key, "t").unwrap();
        let records: Vec<FileRecord> = files
            .iter()
            .map(|s| {
                let p = Path::new(&s.path);
                FileRecord {
                    dir_key: paths::dir_key(p.parent().unwrap()),
                    path: s.path.clone(),
                    path_key: paths::path_key(p),
                    name: p.file_name().unwrap().to_string_lossy().into_owned(),
                    size: 100,
                    mtime: NOW - s.age_days * DAY,
                    attrs: if s.cloud { 0x40_0000 } else { 0x20 },
                    cloud_only: s.cloud,
                    is_link: s.link,
                    ..FileRecord::default()
                }
            })
            .collect();
        index.upsert_files(&records, run.generation).unwrap();
        index
    }

    fn protector() -> Protector {
        protector_with(&Config::default())
    }

    fn protector_with(config: &Config) -> Protector {
        Protector::new(Path::new(ROOT), config, &ProtectPaths::default())
    }

    fn all() -> Vec<String> {
        Config::default().junk_categories
    }

    fn cats(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn run(index: &Index, categories: &[String]) -> CleanupPlan {
        run_with(index, categories, &[], &[], &protector())
    }

    fn run_with(
        index: &Index,
        categories: &[String],
        rules: &[JunkRule],
        downloads: &[PathBuf],
        protector: &Protector,
    ) -> CleanupPlan {
        let result = plan_junk(
            index,
            Path::new(ROOT),
            protector,
            "t",
            &JunkOptions {
                categories,
                rules,
                downloads_dirs: downloads,
                installer_min_age_days: 90,
                now_ticks: NOW,
            },
        )
        .unwrap();
        result.plan.validate().unwrap();
        result
    }

    fn paths_of(plan: &Plan) -> Vec<&str> {
        plan.actions.iter().map(|a| a.path.as_str()).collect()
    }

    fn skipped_reasons(plan: &Plan) -> Vec<(&str, SkipReason)> {
        plan.skipped
            .iter()
            .map(|s| (s.path.as_str(), s.reason))
            .collect()
    }

    fn rule(name: &str, patterns: &[&str], min_age: Option<&str>, dirs: &[&str]) -> JunkRule {
        JunkRule {
            name: name.into(),
            patterns: patterns.iter().map(|s| s.to_string()).collect(),
            min_age: min_age.map(String::from),
            dirs: dirs.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn eingebaute_kategorien_erkennen_ihre_dateien_mit_grund() {
        let index = seed(&[
            f(r"Z:\Root\a\Thumbs.db"),
            f(r"Z:\Root\a\EHTHUMBS.DB"),
            f(r"Z:\Root\a\.DS_Store"),
            f(r"Z:\Root\b\cache.tmp"),
            f(r"Z:\Root\b\SCRATCH.TMP"),
            f(r"Z:\Root\b\~$bericht.docx"),
            f(r"Z:\Root\c\film.crdownload"),
            f(r"Z:\Root\c\daten.part"),
            f(r"Z:\Root\c\daten2.partial"),
        ]);
        let result = run(&index, &all());
        let reasons: Vec<(&str, &str)> = result
            .plan
            .actions
            .iter()
            .map(|a| (a.path.rsplit('\\').next().unwrap(), a.reason.as_str()))
            .collect();
        assert_eq!(
            reasons,
            [
                (".DS_Store", "junk:system"),
                ("EHTHUMBS.DB", "junk:system"),
                ("Thumbs.db", "junk:system"),
                ("cache.tmp", "junk:temp"),
                ("SCRATCH.TMP", "junk:temp"),
                ("~$bericht.docx", "junk:temp"),
                ("daten.part", "junk:downloads"),
                ("daten2.partial", "junk:downloads"),
                ("film.crdownload", "junk:downloads"),
            ]
        );
        assert!(result
            .plan
            .actions
            .iter()
            .all(|a| a.action == ActionType::Quarantine && a.hash.is_none() && a.keep.is_none()));
    }

    #[test]
    fn eingebaute_muellnamen_ohne_installer_und_eigene_regeln() {
        for name in [
            "Thumbs.db",
            "EHTHUMBS.DB",
            ".DS_Store",
            "a.tmp",
            "~$x.docx",
            "f.crdownload",
            "d.part",
            "d.PARTIAL",
        ] {
            assert!(is_builtin_junk_name(name), "{name}");
        }
        for name in [
            "setup.exe",
            "notizen.txt",
            "thumbs.db.bak",
            "~x.docx",
            "tmp",
        ] {
            assert!(!is_builtin_junk_name(name), "{name}");
        }
    }

    #[test]
    fn aehnliche_namen_sind_kein_muell() {
        let index = seed(&[
            f(r"Z:\Root\notizen.txt"),
            f(r"Z:\Root\thumbs.db.bak"),
            f(r"Z:\Root\mythumbs.db"),
            f(r"Z:\Root\tmp"),
            f(r"Z:\Root\datei.tmp.txt"),
            f(r"Z:\Root\~bericht.docx"),
            f(r"Z:\Root\daten.parts"),
            f(r"Z:\Root\setup.exe"),
        ]);
        let result = run(&index, &all());
        assert!(
            result.plan.actions.is_empty(),
            "{:?}",
            paths_of(&result.plan)
        );
    }

    #[test]
    fn category_beschraenkt_auf_die_gewaehlten_kategorien() {
        let index = seed(&[
            f(r"Z:\Root\Thumbs.db"),
            f(r"Z:\Root\a.tmp"),
            f(r"Z:\Root\b.part"),
        ]);
        let only_temp = run(&index, &cats(&["temp"]));
        assert_eq!(paths_of(&only_temp.plan), [r"Z:\Root\a.tmp"]);
        let two = run(&index, &cats(&["system", "downloads"]));
        assert_eq!(
            paths_of(&two.plan),
            [r"Z:\Root\b.part", r"Z:\Root\Thumbs.db"]
        );
        assert!(run(&index, &[]).plan.actions.is_empty());
    }

    #[test]
    fn datei_wird_nur_einmal_und_von_der_ersten_passenden_kategorie_geplant() {
        let index = seed(&[f(r"Z:\Root\a.tmp")]);
        let rules = [rule("alles-tmp", &["*.tmp"], None, &[])];
        let result = run_with(
            &index,
            &cats(&["temp", "alles-tmp"]),
            &rules,
            &[],
            &protector(),
        );
        assert_eq!(result.plan.actions.len(), 1);
        assert_eq!(result.plan.actions[0].reason, "junk:temp");
    }

    #[test]
    fn installer_nur_direkt_im_downloads_ordner_und_ab_mindestalter() {
        let downloads = [PathBuf::from(r"Z:\Root\Downloads")];
        let index = seed(&[
            f(r"Z:\Root\Downloads\alt.exe").age(120),
            f(r"Z:\Root\Downloads\ALT.MSI").age(91),
            f(r"Z:\Root\Downloads\genau.exe").age(90),
            f(r"Z:\Root\Downloads\neu.exe").age(10),
            f(r"Z:\Root\Downloads\unter\tief.exe").age(500),
            f(r"Z:\Root\Anderswo\alt.exe").age(500),
            f(r"Z:\Root\Downloads\doku.pdf").age(500),
        ]);
        let result = run_with(&index, &cats(&["installer"]), &[], &downloads, &protector());
        assert_eq!(
            paths_of(&result.plan),
            [r"Z:\Root\Downloads\alt.exe", r"Z:\Root\Downloads\ALT.MSI"]
        );
        assert!(result
            .plan
            .actions
            .iter()
            .all(|a| a.reason == "junk:installer"));
        assert_eq!(
            skipped_reasons(&result.plan),
            [
                (r"Z:\Root\Downloads\genau.exe", SkipReason::TooRecent),
                (r"Z:\Root\Downloads\neu.exe", SkipReason::TooRecent)
            ]
        );
        assert!(result.notes.is_empty(), "{:?}", result.notes);
    }

    #[test]
    fn installer_ohne_downloads_ordner_unter_der_wurzel_liefert_nichts_und_einen_hinweis() {
        let index = seed(&[f(r"Z:\Root\Downloads\alt.exe").age(500)]);
        for downloads in [vec![], vec![PathBuf::from(r"Y:\Fremd\Downloads")]] {
            let result = run_with(&index, &cats(&["installer"]), &[], &downloads, &protector());
            assert!(result.plan.actions.is_empty());
            assert_eq!(result.notes.len(), 1, "{:?}", result.notes);
            assert!(result.notes[0].contains("installer"), "{:?}", result.notes);
        }
        let without = run_with(&index, &cats(&["temp"]), &[], &[], &protector());
        assert!(without.notes.is_empty());
    }

    #[test]
    fn downloads_ordner_ausserhalb_der_wurzel_zaehlt_nicht_der_innerhalb_schon() {
        let downloads = [
            PathBuf::from(r"Y:\Fremd\Downloads"),
            PathBuf::from(r"z:\root\EINGANG"),
        ];
        let index = seed(&[f(r"Z:\Root\Eingang\setup.exe").age(200)]);
        let result = run_with(&index, &cats(&["installer"]), &[], &downloads, &protector());
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\Eingang\setup.exe"]);
        assert!(result.notes.is_empty());
    }

    #[test]
    fn eigene_regel_mit_muster_mindestalter_und_ordnern() {
        let rules = [rule("logs", &["*.log"], Some("30d"), &[r"Z:\Root\Logs"])];
        let index = seed(&[
            f(r"Z:\Root\Logs\alt.log").age(60),
            f(r"Z:\Root\Logs\NEU.LOG").age(5),
            f(r"Z:\Root\Logs\sub\tief.log").age(60),
            f(r"Z:\Root\Anderswo\alt.log").age(60),
            f(r"Z:\Root\Logs\alt.txt").age(60),
        ]);
        let result = run_with(&index, &cats(&["logs"]), &rules, &[], &protector());
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\Logs\alt.log"]);
        assert_eq!(result.plan.actions[0].reason, "junk:logs");
        assert_eq!(
            skipped_reasons(&result.plan),
            [(r"Z:\Root\Logs\NEU.LOG", SkipReason::TooRecent)]
        );
    }

    #[test]
    fn eigene_regel_ohne_ordner_und_alter_gilt_ueberall_aber_nur_wenn_aktiv() {
        let rules = [rule("dumps", &["*.dmp", "crash-*.txt"], None, &[])];
        let index = seed(&[
            f(r"Z:\Root\a\x.dmp").age(0),
            f(r"Z:\Root\b\Crash-1.TXT").age(0),
            f(r"Z:\Root\b\andere.txt").age(0),
        ]);
        let active = run_with(&index, &cats(&["dumps"]), &rules, &[], &protector());
        assert_eq!(
            paths_of(&active.plan),
            [r"Z:\Root\a\x.dmp", r"Z:\Root\b\Crash-1.TXT"]
        );
        let inactive = run_with(&index, &cats(&["temp"]), &rules, &[], &protector());
        assert!(inactive.plan.actions.is_empty());
    }

    #[test]
    fn geschuetztes_platzhalter_und_links_werden_uebersprungen() {
        let config = Config {
            protected_paths: vec![r"Z:\Root\wichtig".into()],
            ..Config::default()
        };
        let index = seed(&[
            f(r"Z:\Root\wichtig\a.tmp"),
            f(r"Z:\Root\proj\node_modules\b.tmp"),
            f(r"Z:\Root\wolke.tmp").cloud(),
            f(r"Z:\Root\verweis.tmp").link(),
            f(r"Z:\Root\frei.tmp"),
        ]);
        let result = run_with(&index, &all(), &[], &[], &protector_with(&config));
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\frei.tmp"]);
        assert_eq!(
            skipped_reasons(&result.plan),
            [
                (r"Z:\Root\proj\node_modules\b.tmp", SkipReason::Protected),
                (r"Z:\Root\verweis.tmp", SkipReason::Link),
                (r"Z:\Root\wichtig\a.tmp", SkipReason::Protected),
                (r"Z:\Root\wolke.tmp", SkipReason::CloudPlaceholder),
            ]
        );
    }

    #[test]
    fn zu_langer_quarantaene_pfad_wird_uebersprungen() {
        let long = format!(r"Z:\Root\{}.tmp", "x".repeat(32_100));
        let index = seed(&[f(&long)]);
        let result = run(&index, &all());
        assert!(result.plan.actions.is_empty());
        assert_eq!(result.plan.skipped[0].reason, SkipReason::TooLong);
    }

    #[test]
    fn nur_dateien_unterhalb_der_wurzel_zaehlen() {
        let mut index = seed(&[f(r"Z:\Root\a.tmp")]);
        let other = paths::dir_key(Path::new(r"Z:\Root2"));
        let run_id = index.begin_root(r"Z:\Root2", &other, "t").unwrap();
        index
            .upsert_files(
                &[FileRecord {
                    dir_key: other.clone(),
                    path: r"Z:\Root2\b.tmp".into(),
                    path_key: paths::path_key(Path::new(r"Z:\Root2\b.tmp")),
                    name: "b.tmp".into(),
                    size: 1,
                    mtime: NOW,
                    attrs: 0x20,
                    ..FileRecord::default()
                }],
                run_id.generation,
            )
            .unwrap();
        let result = run(&index, &all());
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\a.tmp"]);
    }

    #[test]
    fn plan_ist_deterministisch_nummeriert_und_beschreibt_sich_selbst() {
        let index = seed(&[
            f(r"Z:\Root\z\b.tmp"),
            f(r"Z:\Root\a\a.tmp"),
            f(r"Z:\Root\m\Thumbs.db"),
        ]);
        let first = run(&index, &all());
        let second = run(&index, &all());
        assert_eq!(first.plan, second.plan);

        let plan = &first.plan;
        assert_eq!(plan.version, PLAN_VERSION);
        assert_eq!(plan.kind, PlanKind::Junk);
        assert_eq!(plan.root, ROOT);
        assert_eq!(plan.created, "t");
        assert!(plan.keep_strategy.is_none());
        assert_eq!(
            paths_of(plan),
            [
                r"Z:\Root\a\a.tmp",
                r"Z:\Root\m\Thumbs.db",
                r"Z:\Root\z\b.tmp"
            ]
        );
        let ids: Vec<u32> = plan.actions.iter().map(|a| a.id).collect();
        assert_eq!(ids, [1, 2, 3]);
        assert_eq!(first.bytes, 300);
        assert_eq!(
            plan.params.get("categories").map(String::as_str),
            Some("system,temp,downloads,installer")
        );
        assert_eq!(
            plan.params.get("installer_min_age").map(String::as_str),
            Some("90d")
        );
        let action = &plan.actions[0];
        assert_eq!((action.size, action.mtime_ticks), (100, NOW - 400 * DAY));
        assert!(!action.mtime.is_empty());
    }

    #[test]
    fn leerer_index_ergibt_leeren_plan() {
        let index = seed(&[]);
        let result = run(&index, &all());
        assert!(result.plan.actions.is_empty() && result.plan.skipped.is_empty());
        assert_eq!(result.bytes, 0);
    }
}
