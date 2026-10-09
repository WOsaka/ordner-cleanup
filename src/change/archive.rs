//! Planer für `plan archive`: verschiebt lange unberührte Ordner nach `<Wurzel>\_Archiv\<Jahr>\…`.
//!
//! Ein Ordner gilt als alt, wenn die jüngste mtime aller Dateien darin älter als die Schwelle
//! ist (nur mtime, keine atime). Es wird immer nur der oberste passende Ordner geplant. Der
//! Index muss den Ordner vollständig kennen; Cloud-Platzhalter, Links und Geschütztes darin
//! verhindern das Verschieben als Ganzes. Liest nur den Index und verändert nichts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::Datelike;

use super::plan::{
    ticks_to_rfc3339, ActionType, CleanupPlan, Plan, PlanKind, PlannedAction, Skipped, ARCHIVE_DIR,
    PLAN_VERSION,
};
use super::protect::Protector;
use super::quarantine::MAX_TARGET_LEN;
use super::tree::{contains_protected, Tree};
use super::SkipReason;
use crate::analysis::age::is_old;
use crate::index::{Index, IndexError};
use crate::paths;
use crate::scan::source::TICKS_PER_SEC;

/// `<Wurzel>\_Archiv`
pub fn archive_root(root: &Path) -> PathBuf {
    root.join(ARCHIVE_DIR)
}

/// Ziel eines Archiv-Ordners: `<Wurzel>\_Archiv\<Jahr>\<relativer Pfad>`.
pub fn archive_target(root: &Path, year: i32, relative: &Path) -> PathBuf {
    archive_root(root).join(year.to_string()).join(relative)
}

/// Der Pfad liegt unterhalb von (oder ist) `<Wurzel>\_Archiv`.
pub fn is_in_archive(root: &Path, path: &Path) -> bool {
    paths::is_under(
        &paths::path_key(path),
        &paths::path_key(&archive_root(root)),
    )
}

pub struct ArchiveOptions<'a> {
    /// Schwelle wie angegeben (z. B. `2y`), für Grund und Parameter im Plan.
    pub older_than: &'a str,
    pub older_than_days: i64,
    pub now_ticks: i64,
}

fn year_of(ticks: i64) -> i32 {
    chrono::DateTime::from_timestamp(ticks.div_euclid(TICKS_PER_SEC), 0).map_or(1970, |t| t.year())
}

pub fn plan_archive(
    index: &Index,
    root: &Path,
    protector: &Protector,
    created: &str,
    options: &ArchiveOptions,
) -> Result<CleanupPlan, IndexError> {
    let root_key = paths::dir_key(root);
    let archive_key = paths::dir_key(&archive_root(root));
    let tree = Tree::new(index.dirs_under(&root_key)?, index.files_under(&root_key)?);

    let mut actions: Vec<(String, PlannedAction)> = Vec::new();
    let mut skipped: Vec<(String, Skipped)> = Vec::new();
    // Von oben nach unten: Der oberste alte Ordner gewinnt, darunter wird nicht mehr gesucht.
    let mut stack: Vec<&str> = tree
        .children(&root_key)
        .iter()
        .map(String::as_str)
        .collect();
    while let Some(key) = stack.pop() {
        let (Some(row), true) = (tree.row(key), key != archive_key) else {
            continue;
        };
        let stats = tree.stats(key);
        let Some(newest) = stats.newest_mtime.filter(|_| stats.files > 0) else {
            continue;
        };
        if !is_old(newest, options.now_ticks, options.older_than_days) {
            stack.extend(tree.children(key).iter().map(String::as_str));
            continue;
        }

        let relative = paths::relative_to(root, Path::new(&row.path)).unwrap_or_default();
        let target = archive_target(root, year_of(newest), &relative);
        let blocked = if stats.has_cloud {
            Some(SkipReason::CloudPlaceholder)
        } else if stats.has_link {
            Some(SkipReason::Link)
        } else if contains_protected(&tree, protector, key) {
            Some(SkipReason::Protected)
        } else if !tree.complete(key) {
            Some(SkipReason::IncompleteIndex)
        } else if tree.row(&paths::dir_key(&target)).is_some() {
            Some(SkipReason::TargetExists)
        } else if paths::display(&target).chars().count() > MAX_TARGET_LEN {
            Some(SkipReason::TooLong)
        } else {
            None
        };
        if let Some(reason) = blocked {
            skipped.push((
                key.to_string(),
                Skipped {
                    path: row.path.clone(),
                    reason,
                    detail: None,
                },
            ));
            continue;
        }
        actions.push((
            key.to_string(),
            PlannedAction {
                id: 0,
                action: ActionType::Move,
                path: row.path.clone(),
                size: stats.bytes,
                mtime_ticks: newest,
                mtime: ticks_to_rfc3339(newest),
                hash: None,
                keep: None,
                keep_hash: None,
                reason: format!("archive:older-than-{}", options.older_than),
                target: Some(paths::display(&target)),
                is_dir: true,
                keep_fingerprint: None,
                source_fingerprint: None,
                files: Some(stats.files),
                rule: None,
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
    params.insert("older_than".to_string(), options.older_than.to_string());
    Ok(CleanupPlan {
        plan: Plan {
            version: PLAN_VERSION,
            created: created.to_string(),
            kind: PlanKind::Archive,
            root: paths::display(root),
            keep_strategy: None,
            params,
            protected_paths: Vec::new(),
            actions,
            skipped: skipped.into_iter().map(|(_, s)| s).collect(),
        },
        bytes,
        notes: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::plan::ActionType;
    use crate::change::protect::ProtectPaths;
    use crate::change::SkipReason;
    use crate::config::Config;
    use crate::index::{DirRecord, FileRecord};
    use crate::scan::source::TICKS_PER_SEC;

    const ROOT: &str = r"Z:\Root";
    const DAY: i64 = 86_400 * TICKS_PER_SEC;
    /// 2026-10-03 00:00:00 UTC
    const NOW: i64 = 1_790_985_600 * TICKS_PER_SEC;

    fn ticks(year: i32, month: u32, day: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp()
            * TICKS_PER_SEC
    }

    struct F {
        path: &'static str,
        mtime: i64,
        cloud: bool,
        link: bool,
    }

    /// Datei, `days` Tage vor `NOW` geändert.
    fn aged(path: &'static str, days: i64) -> F {
        F {
            path,
            mtime: NOW - days * DAY,
            cloud: false,
            link: false,
        }
    }

    fn dated(path: &'static str, year: i32, month: u32, day: u32) -> F {
        F {
            path,
            mtime: ticks(year, month, day),
            cloud: false,
            link: false,
        }
    }

    impl F {
        fn cloud(mut self) -> Self {
            self.cloud = true;
            self
        }
        fn link(mut self) -> Self {
            self.link = true;
            self
        }
    }

    /// Ordner (ohne Wurzel) und Dateien; `direct_entries` wird aus dem Bekannten berechnet,
    /// `tweak` darf Ordner-Datensätze für Sonderfälle ändern.
    fn seed_with(dirs: &[&str], files: &[F], tweak: impl Fn(&mut DirRecord)) -> Index {
        let mut index = Index::open_in_memory().unwrap();
        let root_key = paths::dir_key(Path::new(ROOT));
        let run = index.begin_root(ROOT, &root_key, "t").unwrap();
        let mut records: Vec<DirRecord> = std::iter::once(ROOT)
            .chain(dirs.iter().copied())
            .map(|d| {
                let p = Path::new(d);
                let entries = dirs
                    .iter()
                    .filter(|c| Path::new(c).parent() == Some(p))
                    .count()
                    + files
                        .iter()
                        .filter(|f| Path::new(f.path).parent() == Some(p))
                        .count();
                DirRecord {
                    path: d.to_string(),
                    path_key: paths::dir_key(p),
                    parent_key: (d != ROOT).then(|| paths::dir_key(p.parent().unwrap())),
                    mode: "full".into(),
                    attrs: 0x10,
                    direct_entries: entries as i64,
                    ..DirRecord::default()
                }
            })
            .collect();
        for record in &mut records {
            tweak(record);
        }
        index.upsert_dirs(&records, run.generation).unwrap();
        let file_records: Vec<FileRecord> = files
            .iter()
            .map(|f| {
                let p = Path::new(f.path);
                FileRecord {
                    dir_key: paths::dir_key(p.parent().unwrap()),
                    path: f.path.to_string(),
                    path_key: paths::path_key(p),
                    name: p.file_name().unwrap().to_string_lossy().into_owned(),
                    size: 100,
                    mtime: f.mtime,
                    attrs: if f.cloud { 0x40_0000 } else { 0x20 },
                    cloud_only: f.cloud,
                    is_link: f.link,
                    ..FileRecord::default()
                }
            })
            .collect();
        index.upsert_files(&file_records, run.generation).unwrap();
        index
    }

    fn seed(dirs: &[&str], files: &[F]) -> Index {
        seed_with(dirs, files, |_| {})
    }

    fn protector_with(config: &Config) -> Protector {
        Protector::new(Path::new(ROOT), config, &ProtectPaths::default())
    }

    fn run(index: &Index) -> CleanupPlan {
        run_with(index, &protector_with(&Config::default()))
    }

    fn run_with(index: &Index, protector: &Protector) -> CleanupPlan {
        let result = plan_archive(
            index,
            Path::new(ROOT),
            protector,
            "t",
            &ArchiveOptions {
                older_than: "2y",
                older_than_days: 730,
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

    fn skipped(plan: &Plan) -> Vec<(&str, SkipReason)> {
        plan.skipped
            .iter()
            .map(|s| (s.path.as_str(), s.reason))
            .collect()
    }

    #[test]
    fn hilfsfunktionen_bilden_pfade_unter_dem_archiv() {
        let root = Path::new(ROOT);
        assert_eq!(archive_root(root), PathBuf::from(r"Z:\Root\_Archiv"));
        assert_eq!(
            archive_target(root, 2019, Path::new(r"Projekt\2019")),
            PathBuf::from(r"Z:\Root\_Archiv\2019\Projekt\2019")
        );
        assert!(is_in_archive(root, Path::new(r"z:\root\_ARCHIV\2019\x")));
        assert!(is_in_archive(root, Path::new(r"Z:\Root\_Archiv")));
        assert!(!is_in_archive(root, Path::new(r"Z:\Root\_Archiv2\x")));
        assert!(!is_in_archive(root, Path::new(r"Z:\Root\x\_Archiv")));
    }

    #[test]
    fn alter_ordner_wird_nach_jahr_der_juengsten_datei_archiviert() {
        let index = seed(
            &[r"Z:\Root\Projekt2019"],
            &[
                dated(r"Z:\Root\Projekt2019\a.txt", 2018, 3, 1),
                dated(r"Z:\Root\Projekt2019\b.txt", 2019, 6, 15),
            ],
        );
        let result = run(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\Projekt2019"]);
        let a = &result.plan.actions[0];
        assert_eq!(a.action, ActionType::Move);
        assert_eq!(
            a.target.as_deref(),
            Some(r"Z:\Root\_Archiv\2019\Projekt2019")
        );
        assert!(a.is_dir);
        assert_eq!(a.files, Some(2));
        assert_eq!(a.size, 200);
        assert_eq!(a.mtime_ticks, ticks(2019, 6, 15));
        assert_eq!(a.reason, "archive:older-than-2y");
        assert!(a.hash.is_none() && a.keep.is_none());
        assert_eq!(result.bytes, 200);
    }

    #[test]
    fn die_juengste_datei_entscheidet_nicht_die_aelteste() {
        let index = seed(
            &[r"Z:\Root\gemischt"],
            &[
                dated(r"Z:\Root\gemischt\alt.txt", 2015, 1, 1),
                aged(r"Z:\Root\gemischt\neu.txt", 10),
            ],
        );
        assert!(run(&index).plan.actions.is_empty());
    }

    #[test]
    fn schwelle_ist_ein_echtes_groesser_als() {
        let index = seed(
            &[r"Z:\Root\genau", r"Z:\Root\knapp"],
            &[
                aged(r"Z:\Root\genau\x.txt", 730),
                aged(r"Z:\Root\knapp\x.txt", 731),
            ],
        );
        assert_eq!(paths_of(&run(&index).plan), [r"Z:\Root\knapp"]);
    }

    #[test]
    fn nur_der_oberste_passende_ordner_wird_geplant() {
        let index = seed(
            &[r"Z:\Root\a", r"Z:\Root\a\sub", r"Z:\Root\a\sub\tief"],
            &[
                dated(r"Z:\Root\a\x.txt", 2018, 1, 1),
                dated(r"Z:\Root\a\sub\y.txt", 2017, 1, 1),
                dated(r"Z:\Root\a\sub\tief\z.txt", 2016, 1, 1),
            ],
        );
        let result = run(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\a"]);
        assert_eq!(result.plan.actions[0].files, Some(3));
    }

    #[test]
    fn alter_unterordner_in_aktivem_ordner_wird_einzeln_archiviert() {
        let index = seed(
            &[r"Z:\Root\aktiv", r"Z:\Root\aktiv\alt"],
            &[
                aged(r"Z:\Root\aktiv\neu.txt", 3),
                dated(r"Z:\Root\aktiv\alt\x.txt", 2018, 8, 8),
            ],
        );
        let result = run(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\aktiv\alt"]);
        assert_eq!(
            result.plan.actions[0].target.as_deref(),
            Some(r"Z:\Root\_Archiv\2018\aktiv\alt")
        );
    }

    #[test]
    fn einzeldateien_und_ordner_ohne_dateien_werden_nicht_archiviert() {
        let index = seed(
            &[
                r"Z:\Root\leer",
                r"Z:\Root\nur_leere",
                r"Z:\Root\nur_leere\auch",
            ],
            &[dated(r"Z:\Root\lose.txt", 2010, 1, 1)],
        );
        let result = run(&index);
        assert!(
            result.plan.actions.is_empty(),
            "{:?}",
            paths_of(&result.plan)
        );
    }

    #[test]
    fn wurzel_und_archiv_sind_nie_kandidat_und_inhalt_darunter_auch_nicht() {
        let index = seed(
            &[
                r"Z:\Root\_Archiv",
                r"Z:\Root\_Archiv\2019",
                r"Z:\Root\_Archiv\2019\Projekt",
                r"Z:\Root\x",
                r"Z:\Root\x\_Archiv",
            ],
            &[
                dated(r"Z:\Root\_Archiv\2019\Projekt\a.txt", 2019, 1, 1),
                dated(r"Z:\Root\x\_Archiv\b.txt", 2015, 1, 1),
            ],
        );
        let result = run(&index);
        // Nur das tiefe `_Archiv` (nicht unter der Wurzel) ist ein gewöhnlicher Ordner; `x`
        // enthält es und ist als Ganzes alt.
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\x"]);
    }

    #[test]
    fn cloud_platzhalter_und_links_verhindern_das_verschieben_als_ganzes() {
        let index = seed(
            &[r"Z:\Root\wolke", r"Z:\Root\verweis", r"Z:\Root\ok"],
            &[
                dated(r"Z:\Root\wolke\a.txt", 2015, 1, 1),
                dated(r"Z:\Root\wolke\b.txt", 2015, 1, 1).cloud(),
                dated(r"Z:\Root\verweis\a.txt", 2015, 1, 1),
                dated(r"Z:\Root\verweis\l.lnk", 2015, 1, 1).link(),
                dated(r"Z:\Root\ok\a.txt", 2015, 1, 1),
            ],
        );
        let result = run(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\ok"]);
        assert_eq!(
            skipped(&result.plan),
            [
                (r"Z:\Root\verweis", SkipReason::Link),
                (r"Z:\Root\wolke", SkipReason::CloudPlaceholder)
            ]
        );
    }

    #[test]
    fn link_ordner_darin_verhindert_das_verschieben() {
        let index = seed_with(
            &[r"Z:\Root\p", r"Z:\Root\p\junction"],
            &[dated(r"Z:\Root\p\a.txt", 2015, 1, 1)],
            |d| {
                if d.path.ends_with("junction") {
                    d.is_link = true;
                }
            },
        );
        let result = run(&index);
        assert!(result.plan.actions.is_empty());
        assert_eq!(skipped(&result.plan), [(r"Z:\Root\p", SkipReason::Link)]);
    }

    #[test]
    fn geschuetzter_unterordner_oder_name_verhindert_das_verschieben_des_elternordners() {
        let config = Config {
            protected_paths: vec![r"Z:\Root\p\wichtig".into()],
            ..Config::default()
        };
        let index = seed(
            &[
                r"Z:\Root\p",
                r"Z:\Root\p\wichtig",
                r"Z:\Root\q",
                r"Z:\Root\q\.git",
                r"Z:\Root\frei",
            ],
            &[
                dated(r"Z:\Root\p\wichtig\a.txt", 2015, 1, 1),
                dated(r"Z:\Root\q\.git\config", 2015, 1, 1),
                dated(r"Z:\Root\frei\a.txt", 2015, 1, 1),
            ],
        );
        let result = run_with(&index, &protector_with(&config));
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\frei"]);
        assert_eq!(
            skipped(&result.plan),
            [
                (r"Z:\Root\p", SkipReason::Protected),
                (r"Z:\Root\q", SkipReason::Protected)
            ]
        );
    }

    #[test]
    fn unvollstaendiger_ordner_wird_nicht_archiviert() {
        // Der Scan sah 2 Einträge, bekannt ist nur einer (z. B. ausgeschlossene Datei).
        let index = seed_with(
            &[r"Z:\Root\halb"],
            &[dated(r"Z:\Root\halb\a.txt", 2015, 1, 1)],
            |d| {
                if d.path.ends_with("halb") {
                    d.direct_entries = 2;
                }
            },
        );
        let result = run(&index);
        assert!(result.plan.actions.is_empty());
        assert_eq!(
            skipped(&result.plan),
            [(r"Z:\Root\halb", SkipReason::IncompleteIndex)]
        );
    }

    #[test]
    fn vorhandenes_ziel_im_archiv_wird_nie_ueberschrieben() {
        let index = seed(
            &[
                r"Z:\Root\Projekt",
                r"Z:\Root\_Archiv",
                r"Z:\Root\_Archiv\2015",
                r"Z:\Root\_Archiv\2015\Projekt",
            ],
            &[
                dated(r"Z:\Root\Projekt\a.txt", 2015, 5, 5),
                dated(r"Z:\Root\_Archiv\2015\Projekt\alt.txt", 2015, 1, 1),
            ],
        );
        let result = run(&index);
        assert!(result.plan.actions.is_empty());
        assert_eq!(
            skipped(&result.plan),
            [(r"Z:\Root\Projekt", SkipReason::TargetExists)]
        );
    }

    #[test]
    fn zu_langes_ziel_wird_uebersprungen() {
        let long = format!(r"Z:\Root\{}", "x".repeat(32_000));
        let long_file = format!(r"{long}\a.txt");
        let dir: &'static str = Box::leak(long.into_boxed_str());
        let file: &'static str = Box::leak(long_file.into_boxed_str());
        let index = seed(&[dir], &[dated(file, 2015, 1, 1)]);
        let result = run(&index);
        assert!(result.plan.actions.is_empty());
        assert_eq!(result.plan.skipped[0].reason, SkipReason::TooLong);
    }

    #[test]
    fn nur_ordner_unterhalb_der_wurzel_zaehlen() {
        let mut index = seed(&[r"Z:\Root\a"], &[dated(r"Z:\Root\a\x.txt", 2015, 1, 1)]);
        let other = paths::dir_key(Path::new(r"Z:\Root2"));
        let run_id = index.begin_root(r"Z:\Root2", &other, "t").unwrap();
        index
            .upsert_dirs(
                &[DirRecord {
                    path: r"Z:\Root2\fremd".into(),
                    path_key: paths::dir_key(Path::new(r"Z:\Root2\fremd")),
                    parent_key: Some(other.clone()),
                    mode: "full".into(),
                    attrs: 0x10,
                    direct_entries: 1,
                    ..DirRecord::default()
                }],
                run_id.generation,
            )
            .unwrap();
        index
            .upsert_files(
                &[FileRecord {
                    dir_key: paths::dir_key(Path::new(r"Z:\Root2\fremd")),
                    path: r"Z:\Root2\fremd\x.txt".into(),
                    path_key: paths::path_key(Path::new(r"Z:\Root2\fremd\x.txt")),
                    name: "x.txt".into(),
                    size: 1,
                    mtime: ticks(2010, 1, 1),
                    attrs: 0x20,
                    ..FileRecord::default()
                }],
                run_id.generation,
            )
            .unwrap();
        assert_eq!(paths_of(&run(&index).plan), [r"Z:\Root\a"]);
    }

    #[test]
    fn plan_beschreibt_sich_selbst_und_ist_deterministisch() {
        let index = seed(
            &[r"Z:\Root\b", r"Z:\Root\a"],
            &[
                dated(r"Z:\Root\b\x.txt", 2014, 1, 1),
                dated(r"Z:\Root\a\x.txt", 2016, 1, 1),
            ],
        );
        let first = run(&index);
        let second = run(&index);
        assert_eq!(first.plan, second.plan);

        let plan = &first.plan;
        assert_eq!(plan.version, PLAN_VERSION);
        assert_eq!(plan.kind, PlanKind::Archive);
        assert_eq!(plan.root, ROOT);
        assert_eq!(
            plan.params.get("older_than").map(String::as_str),
            Some("2y")
        );
        assert_eq!(paths_of(plan), [r"Z:\Root\a", r"Z:\Root\b"]);
        let ids: Vec<u32> = plan.actions.iter().map(|a| a.id).collect();
        assert_eq!(ids, [1, 2]);
        assert!(plan.actions.iter().all(|a| !a.mtime.is_empty()));
    }

    #[test]
    fn leerer_index_ergibt_leeren_plan() {
        let index = seed(&[], &[]);
        let result = run(&index);
        assert!(result.plan.actions.is_empty() && result.plan.skipped.is_empty());
    }
}
