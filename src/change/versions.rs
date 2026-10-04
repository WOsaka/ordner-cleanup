//! Planer für `plan versions`: behält pro Versionsgruppe die jüngste Datei und verschiebt die
//! älteren nach `<Wurzel>\_Archiv\Versionen\<relativer Pfad>`.
//!
//! Eine Gruppe bilden Dateien **desselben Ordners** mit gleichem normalisierten Namen und
//! gleicher Endung (`analysis::similar::group_similar`), mindestens zwei. Die Datei mit der
//! jüngsten mtime bleibt (`keep`); bei Gleichstand gewinnt der lexikografisch letzte Pfad. Ältere
//! Versionen müssen mindestens `min_age` alt sein. Liest nur den Index und verändert nichts.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use super::archive::archive_root;
use super::plan::{
    ticks_to_rfc3339, ActionType, CleanupPlan, Plan, PlanKind, PlannedAction, Skipped, PLAN_VERSION,
};
use super::protect::Protector;
use super::quarantine::MAX_TARGET_LEN;
use super::SkipReason;
use crate::analysis::age::age_days;
use crate::analysis::similar::group_similar;
use crate::index::{FileRow, Index, IndexError};
use crate::paths;

/// Unterordner von `_Archiv` für ältere Versionen.
pub const VERSIONS_DIR: &str = "Versionen";

/// `<Wurzel>\_Archiv\Versionen\<relativer Pfad>`
pub fn versions_target(root: &Path, relative: &Path) -> PathBuf {
    archive_root(root).join(VERSIONS_DIR).join(relative)
}

pub struct VersionsOptions<'a> {
    /// Mindestalter älterer Versionen wie angegeben (z. B. `30d`), für den Plan.
    pub min_age: &'a str,
    pub min_age_days: i64,
    pub now_ticks: i64,
}

/// Die Datei mit der jüngsten mtime bleibt; bei Gleichstand der lexikografisch letzte Pfad.
fn newest<'a>(group: &[&'a FileRow]) -> &'a FileRow {
    group
        .iter()
        .copied()
        .max_by(|a, b| a.mtime.cmp(&b.mtime).then_with(|| a.key.cmp(&b.key)))
        .expect("Gruppen haben mindestens zwei Mitglieder")
}

pub fn plan_versions(
    index: &Index,
    root: &Path,
    protector: &Protector,
    created: &str,
    options: &VersionsOptions,
) -> Result<CleanupPlan, IndexError> {
    let files = index.files_under(&paths::dir_key(root))?;
    let existing: HashSet<&str> = files.iter().map(|f| f.key.as_str()).collect();
    let archive_key = paths::path_key(&archive_root(root));

    // Nur Dateien außerhalb von `_Archiv`, je Ordner (die Gruppenbildung gilt nur dort).
    let mut by_dir: BTreeMap<&str, Vec<&FileRow>> = BTreeMap::new();
    for file in files
        .iter()
        .filter(|f| !paths::is_under(&f.key, &archive_key))
    {
        by_dir.entry(&file.dir_key).or_default().push(file);
    }

    let mut actions: Vec<(String, PlannedAction)> = Vec::new();
    let mut skipped: Vec<(String, Skipped)> = Vec::new();
    for dir_files in by_dir.values() {
        let names: Vec<&str> = dir_files.iter().map(|f| f.name.as_str()).collect();
        for indices in group_similar(&names) {
            let group: Vec<&FileRow> = indices.iter().map(|&i| dir_files[i]).collect();
            let keep = newest(&group);
            let mut note = |file: &FileRow, reason| {
                skipped.push((
                    file.key.clone(),
                    Skipped {
                        path: file.path.clone(),
                        reason,
                        detail: None,
                    },
                ));
            };
            // Ohne lokalen Inhalt ist die mtime der jüngsten Datei nicht vertrauenswürdig.
            let keep_usable = !keep.cloud_only && !keep.is_link;
            for file in group.iter().filter(|f| f.key != keep.key) {
                let path = Path::new(&file.path);
                let relative = paths::relative_to(root, path).unwrap_or_default();
                let target = versions_target(root, &relative);
                let blocked = if !keep_usable {
                    Some(SkipReason::GroupIncomplete)
                } else if protector.check(path).is_some() {
                    Some(SkipReason::Protected)
                } else if file.cloud_only {
                    Some(SkipReason::CloudPlaceholder)
                } else if file.is_link {
                    Some(SkipReason::Link)
                } else if age_days(file.mtime, options.now_ticks) < options.min_age_days {
                    Some(SkipReason::TooRecent)
                } else if existing.contains(paths::path_key(&target).as_str()) {
                    Some(SkipReason::TargetExists)
                } else if paths::display(&target).chars().count() > MAX_TARGET_LEN {
                    Some(SkipReason::TooLong)
                } else {
                    None
                };
                if let Some(reason) = blocked {
                    note(file, reason);
                    continue;
                }
                actions.push((
                    file.key.clone(),
                    PlannedAction {
                        id: 0,
                        action: ActionType::Move,
                        path: file.path.clone(),
                        size: file.size.max(0) as u64,
                        mtime_ticks: file.mtime,
                        mtime: ticks_to_rfc3339(file.mtime),
                        hash: None,
                        keep: Some(keep.path.clone()),
                        keep_hash: None,
                        reason: "older-version".into(),
                        target: Some(paths::display(&target)),
                        is_dir: false,
                        files: None,
                        rule: None,
                    },
                ));
            }
        }
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
    params.insert("min_age".to_string(), options.min_age.to_string());
    Ok(CleanupPlan {
        plan: Plan {
            version: PLAN_VERSION,
            created: created.to_string(),
            kind: PlanKind::Versions,
            root: paths::display(root),
            keep_strategy: None,
            params,
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
    use crate::index::FileRecord;
    use crate::scan::source::TICKS_PER_SEC;

    const ROOT: &str = r"Z:\Root";
    const DAY: i64 = 86_400 * TICKS_PER_SEC;
    const NOW: i64 = 1_790_985_600 * TICKS_PER_SEC;

    struct F {
        path: &'static str,
        age_days: i64,
        cloud: bool,
        link: bool,
    }

    fn f(path: &'static str, age_days: i64) -> F {
        F {
            path,
            age_days,
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

    fn seed(files: &[F]) -> Index {
        let mut index = Index::open_in_memory().unwrap();
        let dir_key = paths::dir_key(Path::new(ROOT));
        let run = index.begin_root(ROOT, &dir_key, "t").unwrap();
        let records: Vec<FileRecord> = files
            .iter()
            .map(|s| {
                let p = Path::new(s.path);
                FileRecord {
                    dir_key: paths::dir_key(p.parent().unwrap()),
                    path: s.path.into(),
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

    fn protector_with(config: &Config) -> Protector {
        Protector::new(Path::new(ROOT), config, &ProtectPaths::default())
    }

    fn run(index: &Index) -> CleanupPlan {
        run_with(index, &protector_with(&Config::default()), 30)
    }

    fn run_with(index: &Index, protector: &Protector, min_age_days: i64) -> CleanupPlan {
        let result = plan_versions(
            index,
            Path::new(ROOT),
            protector,
            "t",
            &VersionsOptions {
                min_age: &format!("{min_age_days}d"),
                min_age_days,
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
    fn hilfsfunktion_bildet_das_ziel_unter_archiv_versionen() {
        assert_eq!(
            versions_target(Path::new(ROOT), Path::new(r"Sub\Bericht_v1.docx")),
            PathBuf::from(r"Z:\Root\_Archiv\Versionen\Sub\Bericht_v1.docx")
        );
    }

    #[test]
    fn spec_beispiel_juengste_bleibt_aeltere_gehen_nach_archiv_versionen() {
        let index = seed(&[
            f(r"Z:\Root\Bericht_v1.docx", 400),
            f(r"Z:\Root\Bericht_v2.docx", 200),
            f(r"Z:\Root\Bericht final.docx", 100),
        ]);
        let result = run(&index);
        assert_eq!(
            paths_of(&result.plan),
            [r"Z:\Root\Bericht_v1.docx", r"Z:\Root\Bericht_v2.docx"]
        );
        for a in &result.plan.actions {
            assert_eq!(a.action, ActionType::Move);
            assert!(!a.is_dir && a.files.is_none());
            assert_eq!(a.keep.as_deref(), Some(r"Z:\Root\Bericht final.docx"));
            assert!(a.hash.is_none() && a.keep_hash.is_none());
            assert_eq!(a.reason, "older-version");
            assert_eq!((a.size, result.bytes), (100, 200));
        }
        assert_eq!(
            result.plan.actions[0].target.as_deref(),
            Some(r"Z:\Root\_Archiv\Versionen\Bericht_v1.docx")
        );
        assert_eq!(result.plan.actions[0].mtime_ticks, NOW - 400 * DAY);
    }

    #[test]
    fn unterordner_behalten_ihren_relativen_pfad_im_ziel() {
        let index = seed(&[
            f(r"Z:\Root\Sub\Tief\Plan_v1.xlsx", 90),
            f(r"Z:\Root\Sub\Tief\Plan_v2.xlsx", 40),
        ]);
        let result = run(&index);
        assert_eq!(
            result.plan.actions[0].target.as_deref(),
            Some(r"Z:\Root\_Archiv\Versionen\Sub\Tief\Plan_v1.xlsx")
        );
    }

    #[test]
    fn juengere_aeltere_version_bleibt_und_wird_als_too_recent_gemeldet() {
        let index = seed(&[
            f(r"Z:\Root\a_v1.doc", 300),
            f(r"Z:\Root\a_v2.doc", 10),
            f(r"Z:\Root\a_v3.doc", 2),
        ]);
        let result = run(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\a_v1.doc"]);
        assert_eq!(
            skipped(&result.plan),
            [(r"Z:\Root\a_v2.doc", SkipReason::TooRecent)]
        );
    }

    #[test]
    fn mindestalter_gilt_inklusive() {
        let index = seed(&[
            f(r"Z:\Root\genau_v1.doc", 30),
            f(r"Z:\Root\genau_v2.doc", 1),
            f(r"Z:\Root\knapp_v1.doc", 29),
            f(r"Z:\Root\knapp_v2.doc", 1),
        ]);
        let result = run(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\genau_v1.doc"]);
        assert_eq!(
            skipped(&result.plan),
            [(r"Z:\Root\knapp_v1.doc", SkipReason::TooRecent)]
        );
    }

    #[test]
    fn andere_ordner_oder_endungen_bilden_keine_gruppe() {
        let index = seed(&[
            f(r"Z:\Root\a\Bericht_v1.docx", 300),
            f(r"Z:\Root\b\Bericht_v2.docx", 200),
            f(r"Z:\Root\c\Plan_v1.docx", 300),
            f(r"Z:\Root\c\Plan_v2.pdf", 200),
            f(r"Z:\Root\c\Einzeln.txt", 300),
        ]);
        let result = run(&index);
        assert!(
            result.plan.actions.is_empty(),
            "{:?}",
            paths_of(&result.plan)
        );
        assert!(result.plan.skipped.is_empty());
    }

    #[test]
    fn gleiche_mtime_waehlt_den_lexikografisch_letzten_pfad_deterministisch() {
        let index = seed(&[
            f(r"Z:\Root\a_v1.doc", 100),
            f(r"Z:\Root\a_v3.doc", 100),
            f(r"Z:\Root\a_v2.doc", 100),
        ]);
        let first = run(&index);
        assert_eq!(first.plan, run(&index).plan);
        assert_eq!(
            paths_of(&first.plan),
            [r"Z:\Root\a_v1.doc", r"Z:\Root\a_v2.doc"]
        );
        assert!(first
            .plan
            .actions
            .iter()
            .all(|a| a.keep.as_deref() == Some(r"Z:\Root\a_v3.doc")));
    }

    #[test]
    fn juengste_datei_als_cloud_platzhalter_ueberspringt_die_ganze_gruppe() {
        let index = seed(&[
            f(r"Z:\Root\a_v1.doc", 300),
            f(r"Z:\Root\a_v2.doc", 200),
            f(r"Z:\Root\a_v3.doc", 5).cloud(),
        ]);
        let result = run(&index);
        assert!(result.plan.actions.is_empty());
        assert_eq!(
            skipped(&result.plan),
            [
                (r"Z:\Root\a_v1.doc", SkipReason::GroupIncomplete),
                (r"Z:\Root\a_v2.doc", SkipReason::GroupIncomplete)
            ]
        );
    }

    #[test]
    fn juengste_datei_als_link_ueberspringt_die_ganze_gruppe() {
        let index = seed(&[
            f(r"Z:\Root\a_v1.doc", 300),
            f(r"Z:\Root\a_v2.doc", 5).link(),
        ]);
        let result = run(&index);
        assert!(result.plan.actions.is_empty());
        assert_eq!(result.plan.skipped.len(), 1);
    }

    #[test]
    fn aeltere_cloud_platzhalter_und_links_zaehlen_zur_gruppe_werden_aber_nie_verschoben() {
        let index = seed(&[
            f(r"Z:\Root\a_v1.doc", 300).cloud(),
            f(r"Z:\Root\a_v2.doc", 250).link(),
            f(r"Z:\Root\a_v3.doc", 200),
            f(r"Z:\Root\a_v4.doc", 100),
        ]);
        let result = run(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\a_v3.doc"]);
        assert_eq!(
            skipped(&result.plan),
            [
                (r"Z:\Root\a_v1.doc", SkipReason::CloudPlaceholder),
                (r"Z:\Root\a_v2.doc", SkipReason::Link)
            ]
        );
    }

    #[test]
    fn geschuetzte_aeltere_version_wird_uebersprungen() {
        let config = Config {
            protected_paths: vec![r"Z:\Root\wichtig".into()],
            ..Config::default()
        };
        let index = seed(&[
            f(r"Z:\Root\wichtig\a_v1.doc", 300),
            f(r"Z:\Root\wichtig\a_v2.doc", 200),
            f(r"Z:\Root\frei\b_v1.doc", 300),
            f(r"Z:\Root\frei\b_v2.doc", 200),
        ]);
        let result = run_with(&index, &protector_with(&config), 30);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\frei\b_v1.doc"]);
        assert_eq!(
            skipped(&result.plan),
            [(r"Z:\Root\wichtig\a_v1.doc", SkipReason::Protected)]
        );
    }

    #[test]
    fn dateien_unterhalb_von_archiv_sind_keine_kandidaten_und_bilden_keine_gruppe() {
        let index = seed(&[
            f(r"Z:\Root\Bericht_v1.docx", 300),
            f(r"Z:\Root\_Archiv\Versionen\Bericht_v2.docx", 200),
            f(r"Z:\Root\_Archiv\2019\Bericht_v3.docx", 100),
        ]);
        let result = run(&index);
        assert!(result.plan.actions.is_empty());
        assert!(result.plan.skipped.is_empty());
    }

    #[test]
    fn vorhandenes_ziel_wird_nie_ueberschrieben() {
        let index = seed(&[
            f(r"Z:\Root\a_v1.doc", 300),
            f(r"Z:\Root\a_v2.doc", 200),
            // Ziel einer früheren Archivierung, im Index vorhanden (aber eine andere Gruppe,
            // weil unter _Archiv)
            f(r"Z:\Root\_Archiv\Versionen\a_v1.doc", 500),
        ]);
        let result = run(&index);
        assert!(result.plan.actions.is_empty());
        assert_eq!(
            skipped(&result.plan),
            [(r"Z:\Root\a_v1.doc", SkipReason::TargetExists)]
        );
    }

    #[test]
    fn zu_langes_ziel_wird_uebersprungen() {
        let long: &'static str =
            Box::leak(format!(r"Z:\Root\{}\a_v1.doc", "x".repeat(32_000)).into_boxed_str());
        let long2: &'static str =
            Box::leak(format!(r"Z:\Root\{}\a_v2.doc", "x".repeat(32_000)).into_boxed_str());
        let index = seed(&[f(long, 300), f(long2, 200)]);
        let result = run(&index);
        assert!(result.plan.actions.is_empty());
        assert_eq!(result.plan.skipped[0].reason, SkipReason::TooLong);
    }

    #[test]
    fn mehrere_gruppen_in_mehreren_ordnern_sind_sortiert_und_nummeriert() {
        let index = seed(&[
            f(r"Z:\Root\z\Zeta_v1.txt", 300),
            f(r"Z:\Root\z\Zeta_v2.txt", 200),
            f(r"Z:\Root\a\Alpha_v1.txt", 300),
            f(r"Z:\Root\a\Alpha_v2.txt", 250),
            f(r"Z:\Root\a\Alpha_v3.txt", 200),
        ]);
        let result = run(&index);
        assert_eq!(
            paths_of(&result.plan),
            [
                r"Z:\Root\a\Alpha_v1.txt",
                r"Z:\Root\a\Alpha_v2.txt",
                r"Z:\Root\z\Zeta_v1.txt"
            ]
        );
        let ids: Vec<u32> = result.plan.actions.iter().map(|a| a.id).collect();
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn nach_dem_archivieren_ist_der_naechste_plan_leer() {
        // Zustand nach `apply` + `scan`: nur noch die jüngste Datei, ältere unter _Archiv.
        let index = seed(&[
            f(r"Z:\Root\Bericht final.docx", 100),
            f(r"Z:\Root\_Archiv\Versionen\Bericht_v1.docx", 400),
            f(r"Z:\Root\_Archiv\Versionen\Bericht_v2.docx", 200),
        ]);
        assert!(run(&index).plan.actions.is_empty());
    }

    #[test]
    fn nur_dateien_unterhalb_der_wurzel_zaehlen() {
        let index = seed(&[
            f(r"Z:\Root\a_v1.doc", 300),
            f(r"Z:\Root\a_v2.doc", 200),
            f(r"Z:\Root2\b_v1.doc", 300),
            f(r"Z:\Root2\b_v2.doc", 200),
        ]);
        assert_eq!(paths_of(&run(&index).plan), [r"Z:\Root\a_v1.doc"]);
    }

    #[test]
    fn plan_beschreibt_sich_selbst() {
        let index = seed(&[f(r"Z:\Root\a_v1.doc", 300), f(r"Z:\Root\a_v2.doc", 200)]);
        let plan = run(&index).plan;
        assert_eq!(plan.version, PLAN_VERSION);
        assert_eq!(plan.kind, PlanKind::Versions);
        assert_eq!(plan.root, ROOT);
        assert_eq!(plan.params.get("min_age").map(String::as_str), Some("30d"));
        assert!(plan.keep_strategy.is_none());
        assert!(!plan.actions[0].mtime.is_empty());
    }

    #[test]
    fn leerer_index_ergibt_leeren_plan() {
        let index = seed(&[]);
        assert!(run(&index).plan.actions.is_empty());
    }
}
