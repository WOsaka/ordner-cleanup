//! Planer für leere Ordner (`plan empty-dirs`): liest nur den Index und verändert nichts.
//!
//! Ein Ordner ist nur dann leer, wenn der Index ihn vollständig kennt (siehe `tree`) und er
//! rekursiv keine Datei enthält, auch keine versteckte, System- oder Cloud-Datei. Die Wurzel
//! und der Archivordner `_Archiv` selbst bleiben immer stehen. Der Plan läuft von unten nach
//! oben, damit ein Ordner erst nach seinen leeren Unterordnern entfernt wird.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::junk::is_builtin_junk_name;
use super::plan::{
    ticks_to_rfc3339, ActionType, CleanupPlan, Plan, PlanKind, PlannedAction, Skipped, ARCHIVE_DIR,
    PLAN_VERSION,
};
use super::protect::Protector;
use super::tree::Tree;
use super::SkipReason;
use crate::index::{DirRow, Index, IndexError};
use crate::paths;
use crate::platform::FileAttrs;

/// Tiefste Ordner zuerst, bei gleicher Tiefe alphabetisch.
fn bottom_up(tree: &Tree) -> Vec<String> {
    let mut keys: Vec<String> = tree.dir_keys().map(String::from).collect();
    keys.sort_by(|a, b| {
        b.matches('\\')
            .count()
            .cmp(&a.matches('\\').count())
            .then_with(|| a.cmp(b))
    });
    keys
}

/// Der Index kennt den Ordner selbst nicht vollständig (nicht nur etwas darunter).
fn locally_incomplete(tree: &Tree, row: &DirRow) -> bool {
    let known = (tree.files(&row.key).len() + tree.children(&row.key).len()) as i64;
    row.read_error || (!row.summary && !row.is_link && row.direct_entries != known)
}

/// Anzahl der obersten Ordner, die nur eingebauten Müll enthalten (und deshalb nicht leer sind).
/// Die Wurzel zählt nie selbst (sie bleibt immer stehen); gezählt werden ihre Müll-Unterordner.
fn count_junk_only_dirs(tree: &Tree, order: &[String], root_key: &str) -> usize {
    // (enthält Dateien, enthält nur Müll)
    let mut state: HashMap<&str, (bool, bool)> = HashMap::new();
    for key in order {
        let files = tree.files(key);
        let child_states: Vec<(bool, bool)> = tree
            .children(key)
            .iter()
            .filter_map(|c| state.get(c.as_str()).copied())
            .collect();
        let has_files = !files.is_empty() || child_states.iter().any(|(has, _)| *has);
        let junk_only = tree.complete(key)
            && files.iter().all(|f| is_builtin_junk_name(&f.name))
            && child_states.iter().all(|(has, junk)| !has || *junk);
        state.insert(key, (has_files, junk_only));
    }
    let is_junk_dir = |key: &str| state.get(key).is_some_and(|(has, junk)| *has && *junk);
    order
        .iter()
        .filter(|key| key.as_str() != root_key && is_junk_dir(key))
        .filter(|key| {
            !tree
                .row(key)
                .and_then(|r| r.parent_key.as_deref())
                .is_some_and(|parent| parent != root_key && is_junk_dir(parent))
        })
        .count()
}

pub fn plan_empty_dirs(
    index: &Index,
    root: &Path,
    protector: &Protector,
    created: &str,
) -> Result<CleanupPlan, IndexError> {
    let root_key = paths::dir_key(root);
    let archive_key = paths::dir_key(&root.join(ARCHIVE_DIR));
    let tree = Tree::new(index.dirs_under(&root_key)?, index.files_under(&root_key)?);
    let order = bottom_up(&tree);

    let mut actions: Vec<PlannedAction> = Vec::new();
    let mut skipped: Vec<(String, Skipped)> = Vec::new();
    let mut removable: HashSet<&str> = HashSet::new();
    for key in &order {
        let Some(row) = tree.row(key) else { continue };
        if *key == root_key || *key == archive_key {
            continue;
        }
        let mut skip = |reason| {
            skipped.push((
                key.clone(),
                Skipped {
                    path: row.path.clone(),
                    reason,
                },
            ));
        };
        if !tree.is_empty(key) {
            if tree.stats(key).files == 0
                && locally_incomplete(&tree, row)
                && !row.summary
                && !row.is_link
            {
                skip(SkipReason::IncompleteIndex);
            }
            continue;
        }
        if protector.check(Path::new(&row.path)).is_some() {
            skip(SkipReason::Protected);
        } else if FileAttrs(row.attrs).is_cloud_only() {
            skip(SkipReason::CloudPlaceholder);
        } else if !tree
            .children(key)
            .iter()
            .all(|c| removable.contains(c.as_str()))
        {
            // Ein leerer Unterordner bleibt stehen (geschützt o. Ä.): dieser wäre danach nicht leer.
            skip(SkipReason::NotEmpty);
        } else {
            removable.insert(key);
            let mtime = row.mtime.unwrap_or(0);
            actions.push(PlannedAction {
                id: actions.len() as u32 + 1,
                action: ActionType::RemoveDir,
                path: row.path.clone(),
                size: 0,
                mtime_ticks: mtime,
                mtime: ticks_to_rfc3339(mtime),
                hash: None,
                keep: None,
                keep_hash: None,
                reason: "empty-dir".into(),
                target: None,
                is_dir: true,
                files: None,
                rule: None,
            });
        }
    }
    skipped.sort_by(|a, b| a.0.cmp(&b.0));

    let mut notes = Vec::new();
    let junk_dirs = count_junk_only_dirs(&tree, &order, &root_key);
    if junk_dirs > 0 {
        let (verb, rule) = if junk_dirs == 1 {
            ("enthält", "gilt")
        } else {
            ("enthalten", "gelten")
        };
        notes.push(format!(
            "{junk_dirs} Ordner {verb} nur Müll (z. B. Thumbs.db) und {rule} deshalb nicht als \
             leer. Erst `plan junk` und `apply` ausführen, danach neu scannen."
        ));
    }
    Ok(CleanupPlan {
        plan: Plan {
            version: PLAN_VERSION,
            created: created.to_string(),
            kind: PlanKind::EmptyDirs,
            root: paths::display(root),
            keep_strategy: None,
            params: Default::default(),
            actions,
            skipped: skipped.into_iter().map(|(_, s)| s).collect(),
        },
        bytes: 0,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::plan::ActionType;
    use crate::change::protect::ProtectPaths;
    use crate::change::SkipReason;
    use crate::config::Config;
    use crate::index::{DirRecord, FileRecord, ScanErrorRecord};
    use crate::paths;

    const ROOT: &str = r"Z:\Root";

    /// Ordner mit selbst berechnetem `direct_entries` (Unterordner + Dateien); `tweak` darf
    /// einzelne Ordner-Datensätze für Sonderfälle ändern.
    fn seed_with(dirs: &[&str], files: &[&str], tweak: impl Fn(&mut DirRecord)) -> Index {
        let mut index = Index::open_in_memory().unwrap();
        let root_key = paths::dir_key(Path::new(ROOT));
        let run = index.begin_root(ROOT, &root_key, "t").unwrap();
        let mut records: Vec<DirRecord> = std::iter::once(ROOT)
            .chain(dirs.iter().copied())
            .map(|d| {
                let p = Path::new(d);
                let key = paths::dir_key(p);
                let entries = dirs
                    .iter()
                    .filter(|c| Path::new(c).parent() == Some(p))
                    .count()
                    + files
                        .iter()
                        .filter(|f| Path::new(f).parent() == Some(p))
                        .count();
                DirRecord {
                    path: d.to_string(),
                    parent_key: (d != ROOT).then(|| paths::dir_key(p.parent().unwrap())),
                    path_key: key,
                    depth: 0,
                    mode: "full".into(),
                    attrs: 0x10,
                    mtime: Some(1_700_000_000 * 10_000_000),
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
                let p = Path::new(f);
                FileRecord {
                    dir_key: paths::dir_key(p.parent().unwrap()),
                    path: f.to_string(),
                    path_key: paths::path_key(p),
                    name: p.file_name().unwrap().to_string_lossy().into_owned(),
                    size: 10,
                    mtime: 5,
                    attrs: 0x20,
                    ..FileRecord::default()
                }
            })
            .collect();
        index.upsert_files(&file_records, run.generation).unwrap();
        index
    }

    fn seed(dirs: &[&str], files: &[&str]) -> Index {
        seed_with(dirs, files, |_| {})
    }

    fn protector_with(config: &Config) -> Protector {
        Protector::new(Path::new(ROOT), config, &ProtectPaths::default())
    }

    fn plan_of(index: &Index) -> CleanupPlan {
        plan_with(index, &protector_with(&Config::default()))
    }

    fn plan_with(index: &Index, protector: &Protector) -> CleanupPlan {
        let result = plan_empty_dirs(index, Path::new(ROOT), protector, "t").unwrap();
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
    fn verschachtelte_leere_ordner_werden_von_unten_nach_oben_geplant() {
        let index = seed(
            &[r"Z:\Root\a", r"Z:\Root\a\b", r"Z:\Root\a\b\c", r"Z:\Root\d"],
            &[],
        );
        let result = plan_of(&index);
        assert_eq!(
            paths_of(&result.plan),
            [r"Z:\Root\a\b\c", r"Z:\Root\a\b", r"Z:\Root\a", r"Z:\Root\d"]
        );
        let ids: Vec<u32> = result.plan.actions.iter().map(|a| a.id).collect();
        assert_eq!(ids, [1, 2, 3, 4]);
    }

    #[test]
    fn die_wurzel_wird_nie_entfernt_auch_wenn_sie_leer_ist() {
        let index = seed(&[], &[]);
        let result = plan_of(&index);
        assert!(result.plan.actions.is_empty());
        assert!(result.plan.skipped.is_empty());
    }

    #[test]
    fn datei_macht_ordner_und_vorfahren_nicht_leer() {
        let index = seed(
            &[r"Z:\Root\a", r"Z:\Root\a\b", r"Z:\Root\a\b\c", r"Z:\Root\d"],
            &[r"Z:\Root\a\b\datei.txt"],
        );
        let result = plan_of(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\a\b\c", r"Z:\Root\d"]);
    }

    #[test]
    fn ordner_mit_nur_muell_ist_nicht_leer_und_erzeugt_einen_hinweis() {
        let index = seed(
            &[r"Z:\Root\muell", r"Z:\Root\gemischt", r"Z:\Root\leer"],
            &[
                r"Z:\Root\muell\Thumbs.db",
                r"Z:\Root\muell\cache.tmp",
                r"Z:\Root\gemischt\Thumbs.db",
                r"Z:\Root\gemischt\bericht.docx",
            ],
        );
        let result = plan_of(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\leer"]);
        assert_eq!(result.notes.len(), 1, "{:?}", result.notes);
        assert!(
            result.notes[0].starts_with("1 Ordner"),
            "{:?}",
            result.notes
        );
        assert!(result.notes[0].contains("plan junk"), "{:?}", result.notes);
    }

    #[test]
    fn hinweistext_beachtet_den_singular_und_plural() {
        let one = seed(&[r"Z:\Root\m"], &[r"Z:\Root\m\Thumbs.db"]);
        let note = &plan_of(&one).notes[0];
        assert!(
            note.starts_with("1 Ordner enthält nur Müll")
                && note.contains("gilt deshalb nicht als leer"),
            "{note}"
        );

        let two = seed(
            &[r"Z:\Root\m", r"Z:\Root\n"],
            &[r"Z:\Root\m\Thumbs.db", r"Z:\Root\n\a.tmp"],
        );
        let note = &plan_of(&two).notes[0];
        assert!(
            note.starts_with("2 Ordner enthalten nur Müll")
                && note.contains("gelten deshalb nicht als leer"),
            "{note}"
        );
    }

    #[test]
    fn hinweis_zaehlt_nur_den_obersten_muell_ordner() {
        let index = seed(
            &[r"Z:\Root\k", r"Z:\Root\k\sub"],
            &[r"Z:\Root\k\sub\Thumbs.db"],
        );
        let result = plan_of(&index);
        assert!(result.plan.actions.is_empty());
        assert!(
            result.notes[0].starts_with("1 Ordner"),
            "{:?}",
            result.notes
        );
    }

    #[test]
    fn kein_hinweis_ohne_muell_ordner() {
        let index = seed(&[r"Z:\Root\a"], &[r"Z:\Root\a\bericht.docx"]);
        assert!(plan_of(&index).notes.is_empty());
    }

    #[test]
    fn archiv_wurzel_bleibt_aber_leere_ordner_darunter_gehen() {
        let index = seed(
            &[
                r"Z:\Root\_Archiv",
                r"Z:\Root\_Archiv\2020",
                r"Z:\Root\_Archiv\2020\leer",
            ],
            &[],
        );
        let result = plan_of(&index);
        assert_eq!(
            paths_of(&result.plan),
            [r"Z:\Root\_Archiv\2020\leer", r"Z:\Root\_Archiv\2020"]
        );
    }

    #[test]
    fn nur_der_archivordner_direkt_unter_der_wurzel_ist_ausgenommen() {
        let index = seed(&[r"Z:\Root\x", r"Z:\Root\x\_Archiv"], &[]);
        let result = plan_of(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\x\_Archiv", r"Z:\Root\x"]);
    }

    #[test]
    fn geschuetzter_ordner_wird_uebersprungen_und_sperrt_seinen_elternordner() {
        let config = Config {
            protected_paths: vec![r"Z:\Root\x\wichtig".into()],
            ..Config::default()
        };
        let index = seed(&[r"Z:\Root\x", r"Z:\Root\x\wichtig", r"Z:\Root\y"], &[]);
        let result = plan_with(&index, &protector_with(&config));
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\y"]);
        assert_eq!(
            skipped(&result.plan),
            [
                (r"Z:\Root\x", SkipReason::NotEmpty),
                (r"Z:\Root\x\wichtig", SkipReason::Protected)
            ]
        );
    }

    #[test]
    fn unvollstaendiger_ordner_ist_kein_kandidat_und_wird_gemeldet() {
        // Der Scan sah 2 Einträge, bekannt ist keiner (ausgeschlossen).
        let index = seed_with(&[r"Z:\Root\p", r"Z:\Root\p\q"], &[], |d| {
            if d.path == r"Z:\Root\p\q" {
                d.direct_entries = 2;
            }
        });
        let result = plan_of(&index);
        assert!(result.plan.actions.is_empty());
        assert_eq!(
            skipped(&result.plan),
            [(r"Z:\Root\p\q", SkipReason::IncompleteIndex)]
        );
    }

    #[test]
    fn lesefehler_macht_den_ordner_zum_unvollstaendigen_kandidaten() {
        let mut index = seed(&[r"Z:\Root\gesperrt", r"Z:\Root\leer"], &[]);
        let run = crate::index::RootRun {
            path_key: paths::dir_key(Path::new(ROOT)),
            generation: 1,
        };
        index
            .record_error(
                &run,
                &ScanErrorRecord {
                    path: r"Z:\Root\gesperrt".into(),
                    kind: "access_denied".into(),
                    message: "Zugriff verweigert".into(),
                },
            )
            .unwrap();
        let result = plan_of(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\leer"]);
        assert_eq!(
            skipped(&result.plan),
            [(r"Z:\Root\gesperrt", SkipReason::IncompleteIndex)]
        );
    }

    #[test]
    fn aufsummierter_ordner_und_links_machen_den_elternordner_nicht_leer() {
        let index = seed_with(
            &[
                r"Z:\Root\p",
                r"Z:\Root\p\node_modules",
                r"Z:\Root\l",
                r"Z:\Root\l\verweis",
            ],
            &[],
            |d| {
                if d.path.ends_with("node_modules") {
                    d.mode = "summary".into();
                }
                if d.path.ends_with("verweis") {
                    d.is_link = true;
                }
            },
        );
        let result = plan_of(&index);
        assert!(
            result.plan.actions.is_empty(),
            "{:?}",
            paths_of(&result.plan)
        );
        assert!(
            result.plan.skipped.is_empty(),
            "Summary-Ordner und Links selbst sind keine Meldung wert: {:?}",
            skipped(&result.plan)
        );
    }

    #[test]
    fn leerer_ordner_mit_cloud_attribut_wird_uebersprungen() {
        let index = seed_with(&[r"Z:\Root\wolke", r"Z:\Root\leer"], &[], |d| {
            if d.path.ends_with("wolke") {
                d.attrs = 0x10 | 0x40_0000;
            }
        });
        let result = plan_of(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\leer"]);
        assert_eq!(
            skipped(&result.plan),
            [(r"Z:\Root\wolke", SkipReason::CloudPlaceholder)]
        );
    }

    #[test]
    fn nur_ordner_unterhalb_der_wurzel_zaehlen() {
        let mut index = seed(&[r"Z:\Root\a"], &[]);
        let other = paths::dir_key(Path::new(r"Z:\Root2"));
        let run = index.begin_root(r"Z:\Root2", &other, "t").unwrap();
        index
            .upsert_dirs(
                &[DirRecord {
                    path: r"Z:\Root2\fremd".into(),
                    path_key: paths::dir_key(Path::new(r"Z:\Root2\fremd")),
                    parent_key: Some(other),
                    mode: "full".into(),
                    attrs: 0x10,
                    ..DirRecord::default()
                }],
                run.generation,
            )
            .unwrap();
        let result = plan_of(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\a"]);
    }

    #[test]
    fn plan_beschreibt_sich_selbst_und_ist_deterministisch() {
        let index = seed(&[r"Z:\Root\b", r"Z:\Root\a", r"Z:\Root\a\x"], &[]);
        let first = plan_of(&index);
        let second = plan_of(&index);
        assert_eq!(first.plan, second.plan);

        let plan = &first.plan;
        assert_eq!(plan.version, PLAN_VERSION);
        assert_eq!(plan.kind, PlanKind::EmptyDirs);
        assert_eq!(plan.root, ROOT);
        assert!(plan.keep_strategy.is_none() && plan.params.is_empty());
        assert_eq!(first.bytes, 0);
        assert_eq!(paths_of(plan), [r"Z:\Root\a\x", r"Z:\Root\a", r"Z:\Root\b"]);
        let action = &plan.actions[0];
        assert_eq!(action.action, ActionType::RemoveDir);
        assert_eq!(action.reason, "empty-dir");
        assert!(action.is_dir);
        assert_eq!(action.size, 0);
        assert_eq!(action.mtime_ticks, 1_700_000_000 * 10_000_000);
        assert!(!action.mtime.is_empty());
        assert!(action.hash.is_none() && action.keep.is_none() && action.target.is_none());
    }
}
