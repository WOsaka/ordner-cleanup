//! Planer für doppelte Ordner (`plan dedupe-dirs`): liest nur den Index und verändert nichts.
//!
//! Die Erkennung liefert `analysis::folder_dups`. Hier wird je Gruppe der behaltene Ordner
//! gewählt (Namens-Heuristik, dann `--keep`), von oben nach unten gearbeitet (Gruppen mit
//! größerer Teilbaumhöhe zuerst), damit Unterordner bereits entfernter Ordner entfallen und nichts
//! innerhalb eines behaltenen Ordners entfernt wird, und je entferntem Ordner eine
//! `quarantine`-Aktion mit `is_dir` erzeugt.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use super::dedupe::{DedupePlan, KeepStrategy};
use super::plan::{
    ticks_to_rfc3339, ActionType, Plan, PlanKind, PlannedAction, Skipped, ARCHIVE_DIR, PLAN_VERSION,
};
use super::protect::Protector;
use super::quarantine;
use super::tree::{contains_protected, subtree_dirs, Tree};
use super::SkipReason;
use crate::analysis::folder_dups::{self, within, ExactGroup, FolderDupOptions};
use crate::index::{FileHash, FileRow, Index, IndexError};
use crate::paths;

#[derive(Debug, Clone, Default)]
pub struct DedupeDirsOptions {
    /// Gruppen mit weniger Bytes (ohne Junk) entfallen ohne Skip-Eintrag.
    pub min_size: Option<u64>,
    /// Zusätzliche Kopie-Muster der Namens-Heuristik (`dedupe_dirs_copy_patterns`).
    pub copy_patterns: Vec<String>,
}

fn dir_name(path: &str) -> &str {
    path.trim_end_matches('\\')
        .rsplit('\\')
        .next()
        .unwrap_or(path)
}

/// Alle Dateien im Teilbaum, samt ignorierter.
fn subtree_files<'a>(tree: &'a Tree, key: &'a str) -> impl Iterator<Item = &'a FileRow> {
    subtree_dirs(tree, key)
        .into_iter()
        .flat_map(|dir| tree.files(dir).iter())
}

/// Metadaten-Fingerabdruck eines Ordners aus dem Index (wie ihn `apply` aus dem Dateisystem
/// bildet).
fn meta_print(tree: &Tree, key: &str) -> u128 {
    folder_dups::meta_fingerprint(
        subtree_files(tree, key)
            .filter(|f| !folder_dups::is_ignored_name(&f.name))
            .map(|f| (f.key[key.len()..].to_string(), f.size, f.mtime)),
    )
}

fn hex128(value: u128) -> String {
    format!("{value:032x}")
}

struct Chooser<'a> {
    strategy: &'a KeepStrategy,
    prefix_key: Option<String>,
    patterns: &'a [String],
    tree: &'a Tree,
}

impl Chooser<'_> {
    fn name_score(&self, key: &str) -> u8 {
        let path = self.tree.row(key).map_or("", |r| r.path.as_str());
        folder_dups::copy_name_score(dir_name(path), self.patterns)
    }

    fn newest(&self, key: &str) -> i64 {
        self.tree.stats(key).newest_mtime.unwrap_or(0)
    }

    /// Kleiner ist besser: bereits behaltener Ordner, Heuristik, `--keep`, kürzester Pfad.
    fn order(&self, a: (&str, bool), b: (&str, bool)) -> Ordering {
        let by_time = match self.strategy {
            KeepStrategy::Newest => self.newest(b.0).cmp(&self.newest(a.0)),
            _ => self.newest(a.0).cmp(&self.newest(b.0)),
        };
        b.1.cmp(&a.1)
            .then_with(|| self.name_score(a.0).cmp(&self.name_score(b.0)))
            .then(by_time)
            .then_with(|| a.0.len().cmp(&b.0.len()))
            .then_with(|| a.0.cmp(b.0))
    }

    fn allowed(&self, key: &str) -> bool {
        self.prefix_key
            .as_ref()
            .is_none_or(|p| paths::is_under(key, p))
    }
}

pub fn plan_dedupe_dirs(
    index: &Index,
    root: &Path,
    strategy: &KeepStrategy,
    protector: &Protector,
    created: &str,
    options: &DedupeDirsOptions,
) -> Result<DedupePlan, IndexError> {
    let root_key = paths::dir_key(root);
    let tree = Tree::new(index.dirs_under(&root_key)?, index.files_under(&root_key)?);
    let hashes = index.file_hashes_under(&root_key)?;
    let analysis = folder_dups::analyze(
        &tree,
        &hashes,
        &FolderDupOptions {
            root_key: root_key.clone(),
        },
    );

    let chooser = Chooser {
        strategy,
        prefix_key: match strategy {
            KeepStrategy::PathPrefix(p) => Some(paths::path_key(Path::new(p))),
            _ => None,
        },
        patterns: &options.copy_patterns,
        tree: &tree,
    };
    // Bei `path:<prefix>` entscheidet das Präfix, nicht der Name.
    let name_decides = !matches!(strategy, KeepStrategy::PathPrefix(_));

    // Oben zuerst: Gruppen mit Vorfahren-Mitgliedern vor denen mit deren Nachfahren.
    let mut groups: Vec<&ExactGroup> = analysis.groups.iter().collect();
    groups.sort_by(|a, b| folder_dups::cmp_groups(a, b));

    let archive_key = paths::dir_key(&root.join(ARCHIVE_DIR));
    let mut removed: HashSet<String> = HashSet::new();
    let mut kept: HashSet<String> = HashSet::new();
    let mut keep_prints: HashMap<String, String> = HashMap::new();
    let mut actions: Vec<(String, PlannedAction)> = Vec::new();
    let mut skipped: Vec<(String, Skipped)> = Vec::new();
    let mut removed_dirs: Vec<String> = Vec::new();

    for group in groups {
        if options.min_size.is_some_and(|min| group.bytes < min) {
            continue;
        }
        let mut note = |key: &str, reason| {
            let path = tree
                .row(key)
                .map_or_else(|| key.to_string(), |r| r.path.clone());
            skipped.push((
                key.to_string(),
                Skipped {
                    path,
                    reason,
                    detail: None,
                },
            ));
        };

        let mut eligible: Vec<&str> = Vec::new();
        let mut had_protected = false;
        for key in group.members.iter().map(String::as_str) {
            if within(&removed, key) || paths::is_under(key, &archive_key) {
                continue;
            }
            if contains_protected(&tree, protector, key) {
                had_protected = true;
                note(key, SkipReason::Protected);
            } else {
                eligible.push(key);
            }
        }
        if eligible.len() < 2 {
            if had_protected {
                for key in &eligible {
                    note(key, SkipReason::GroupIncomplete);
                }
            }
            continue;
        }

        // Gesperrt: liegt in einem behaltenen Ordner. Weil Gruppen mit Vorfahren zuerst laufen,
        // kann ein Mitglied hier keinen schon behaltenen Ordner enthalten.
        let locked = |key: &str| within(&kept, key);
        let Some(keep) = eligible
            .iter()
            .copied()
            .filter(|k| chooser.allowed(k))
            .min_by(|a, b| chooser.order((a, locked(a)), (b, locked(b))))
        else {
            for key in &eligible {
                note(key, SkipReason::GroupIncomplete);
            }
            continue;
        };

        let mut to_remove: Vec<&str> = Vec::new();
        for key in eligible
            .iter()
            .copied()
            .filter(|k| *k != keep && !locked(k))
        {
            let Some(row) = tree.row(key) else { continue };
            let longest = subtree_files(&tree, key)
                .map(|f| {
                    f.path
                        .chars()
                        .count()
                        .saturating_sub(row.path.chars().count())
                })
                .max()
                .unwrap_or(0);
            if let Err(reason) = quarantine::fits_dir(root, Path::new(&row.path), longest) {
                note(key, reason);
                continue;
            }
            let all_linked = subtree_files(&tree, key)
                .filter(|f| !folder_dups::is_ignored_name(&f.name))
                .all(|f| {
                    let twin = format!("{keep}{}", &f.key[key.len()..]);
                    match (hashes.get(&f.key), hashes.get(&twin)) {
                        (Some(a), Some(b)) => a.identity.is_some() && a.identity == b.identity,
                        _ => false,
                    }
                });
            if all_linked {
                note(key, SkipReason::Hardlink);
                continue;
            }
            to_remove.push(key);
        }
        if to_remove.is_empty() {
            continue;
        }

        let keep_row_path = tree.row(keep).map(|r| r.path.clone()).unwrap_or_default();
        let keep_print = keep_prints
            .entry(keep.to_string())
            .or_insert_with(|| hex128(meta_print(&tree, keep)))
            .clone();
        let keep_score = chooser.name_score(keep);
        let fingerprint = hex128(group.fingerprint);
        kept.insert(keep.to_string());
        for key in to_remove {
            removed.insert(key.to_string());
            removed_dirs.push(key.to_string());
            let Some(row) = tree.row(key) else { continue };
            let stats = tree.stats(key);
            let decided_by_name = name_decides && chooser.name_score(key) > keep_score;
            actions.push((
                key.to_string(),
                PlannedAction {
                    id: 0,
                    action: ActionType::Quarantine,
                    path: row.path.clone(),
                    size: stats.bytes,
                    mtime_ticks: stats.newest_mtime.unwrap_or(0),
                    mtime: ticks_to_rfc3339(stats.newest_mtime.unwrap_or(0)),
                    hash: Some(fingerprint.clone()),
                    keep: Some(keep_row_path.clone()),
                    keep_hash: Some(fingerprint.clone()),
                    reason: if decided_by_name {
                        "exact-duplicate-dir:copy-name".into()
                    } else {
                        "exact-duplicate-dir".into()
                    },
                    target: None,
                    is_dir: true,
                    keep_fingerprint: Some(keep_print.clone()),
                    source_fingerprint: Some(hex128(meta_print(&tree, key))),
                    files: Some(stats.files),
                    rule: None,
                },
            ));
        }
    }

    let freed_bytes = freed_bytes(&tree, &hashes, &removed_dirs);

    actions.sort_by(|a, b| a.0.cmp(&b.0));
    skipped.sort_by(|a, b| a.0.cmp(&b.0));
    let actions = actions
        .into_iter()
        .zip(1u32..)
        .map(|((_, mut a), id)| {
            a.id = id;
            a
        })
        .collect();
    let mut params = BTreeMap::new();
    if let Some(min) = options.min_size {
        params.insert("min_size".to_string(), min.to_string());
    }
    Ok(DedupePlan {
        plan: Plan {
            version: PLAN_VERSION,
            created: created.to_string(),
            kind: PlanKind::DedupeDirs,
            root: paths::display(root),
            keep_strategy: Some(strategy.to_string()),
            params,
            protected_paths: Vec::new(),
            actions,
            skipped: skipped.into_iter().map(|(_, s)| s).collect(),
        },
        freed_bytes,
    })
}

/// Platzgewinn: Dateien ohne bekannte Identität zählen einzeln, Hardlinks nur, wenn alle Namen
/// der Identität entfernt werden.
fn freed_bytes(tree: &Tree, hashes: &HashMap<String, FileHash>, removed: &[String]) -> u64 {
    let mut freed = 0u64;
    // Identität → (entfernte Namen, Namen insgesamt, Größe)
    let mut by_identity: HashMap<(i64, i64), (i64, i64, u64)> = HashMap::new();
    for key in removed {
        for file in subtree_files(tree, key) {
            let size = file.size.max(0) as u64;
            match hashes
                .get(&file.key)
                .and_then(|h| h.identity.map(|id| (id, h.nlinks)))
            {
                Some((id, nlinks)) => {
                    by_identity.entry(id).or_insert((0, nlinks.max(1), size)).0 += 1;
                }
                None => freed += size,
            }
        }
    }
    freed
        + by_identity
            .values()
            .filter(|(count, nlinks, _)| count >= nlinks)
            .map(|(_, _, size)| size)
            .sum::<u64>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::plan::{Plan, PlanKind};
    use crate::change::protect::ProtectPaths;
    use crate::change::SkipReason;
    use crate::config::Config;
    use crate::index::{DirRecord, FileRecord};
    use crate::paths;

    const ROOT: &str = r"Z:\Root";

    /// (Pfad, mtime, Hash-Byte); Größe immer 100.
    type Seed<'a> = (&'a str, i64, u8);

    fn seed(files: &[Seed]) -> Index {
        seed_with(files, &[], |_| {})
    }

    /// `empty_dirs`: zusätzliche leere Ordner; `tweak` ändert Dateizeilen (Cloud, Identität …).
    fn seed_with(files: &[Seed], empty_dirs: &[&str], tweak: impl Fn(&mut FileRecord)) -> Index {
        let mut index = Index::open_in_memory().unwrap();
        let root_key = paths::dir_key(Path::new(ROOT));
        let run = index.begin_root(ROOT, &root_key, "t").unwrap();
        let mut dirs: std::collections::BTreeSet<String> = std::iter::once(ROOT.to_string())
            .chain(empty_dirs.iter().map(|s| s.to_string()))
            .collect();
        for (path, _, _) in files {
            let mut cur = Path::new(path).parent();
            while let Some(p) = cur {
                if p.as_os_str().len() < ROOT.len() {
                    break;
                }
                dirs.insert(p.to_string_lossy().into_owned());
                cur = p.parent();
            }
        }
        let records: Vec<DirRecord> = dirs
            .iter()
            .map(|d| {
                let p = Path::new(d);
                let entries = dirs
                    .iter()
                    .filter(|c| Path::new(c).parent() == Some(p))
                    .count()
                    + files
                        .iter()
                        .filter(|(f, _, _)| Path::new(f).parent() == Some(p))
                        .count();
                DirRecord {
                    path: d.clone(),
                    path_key: paths::dir_key(p),
                    parent_key: (d != ROOT).then(|| paths::dir_key(p.parent().unwrap())),
                    mode: "full".into(),
                    attrs: 0x10,
                    direct_entries: entries as i64,
                    ..DirRecord::default()
                }
            })
            .collect();
        index.upsert_dirs(&records, run.generation).unwrap();
        let file_records: Vec<FileRecord> = files
            .iter()
            .map(|(path, mtime, hash)| {
                let p = Path::new(path);
                let mut record = FileRecord {
                    dir_key: paths::dir_key(p.parent().unwrap()),
                    path: (*path).into(),
                    path_key: paths::path_key(p),
                    name: p.file_name().unwrap().to_string_lossy().into_owned(),
                    size: 100,
                    mtime: *mtime,
                    attrs: 0x20,
                    partial_hash: Some(vec![*hash; 16]),
                    full_hash: Some(vec![*hash; 16]),
                    hash_status: Some("ok".into()),
                    ..FileRecord::default()
                };
                tweak(&mut record);
                record
            })
            .collect();
        index.upsert_files(&file_records, run.generation).unwrap();
        index
    }

    fn set_identity(index: &Index, path: &str, volume: i64, file_index: i64, nlinks: i64) {
        index
            .conn()
            .execute(
                "UPDATE files SET volume_serial=?1, file_index=?2, nlinks=?3 WHERE path_key=?4",
                rusqlite::params![volume, file_index, nlinks, paths::path_key(Path::new(path))],
            )
            .unwrap();
    }

    fn protector_with(config: &Config) -> Protector {
        Protector::new(Path::new(ROOT), config, &ProtectPaths::default())
    }

    fn plan_full(
        index: &Index,
        strategy: KeepStrategy,
        protector: &Protector,
        options: &DedupeDirsOptions,
    ) -> DedupePlan {
        let result =
            plan_dedupe_dirs(index, Path::new(ROOT), &strategy, protector, "t", options).unwrap();
        result.plan.validate().unwrap();
        result
    }

    fn plan_with(index: &Index, strategy: KeepStrategy) -> DedupePlan {
        plan_full(
            index,
            strategy,
            &protector_with(&Config::default()),
            &DedupeDirsOptions::default(),
        )
    }

    fn plan_of(index: &Index) -> DedupePlan {
        plan_with(index, KeepStrategy::Oldest)
    }

    fn paths_of(plan: &Plan) -> Vec<&str> {
        plan.actions.iter().map(|a| a.path.as_str()).collect()
    }

    fn skip_reasons(plan: &Plan) -> Vec<(String, SkipReason)> {
        plan.skipped
            .iter()
            .map(|s| (s.path.clone(), s.reason))
            .collect()
    }

    #[test]
    fn kopie_von_projekt_wird_als_ordner_in_die_quarantaene_geplant() {
        let index = seed(&[
            (r"Z:\Root\Projekt\a.txt", 100, 1),
            (r"Z:\Root\Projekt\sub\b.txt", 100, 2),
            (r"Z:\Root\Kopie von Projekt\a.txt", 200, 1),
            (r"Z:\Root\Kopie von Projekt\sub\b.txt", 300, 2),
        ]);
        let result = plan_of(&index);
        let plan = &result.plan;
        assert_eq!(plan.kind, PlanKind::DedupeDirs);
        assert_eq!(paths_of(plan), [r"Z:\Root\Kopie von Projekt"]);
        let a = &plan.actions[0];
        assert!(a.is_dir);
        assert_eq!(a.keep.as_deref(), Some(r"Z:\Root\Projekt"));
        assert_eq!((a.files, a.size, a.mtime_ticks), (Some(2), 200, 300));
        assert_eq!(a.reason, "exact-duplicate-dir:copy-name");
        assert_eq!(a.hash, a.keep_hash);
        assert_eq!(a.hash.as_deref().map(str::len), Some(32));
        assert_eq!(a.keep_fingerprint.as_deref().map(str::len), Some(32));
        assert_eq!(plan.keep_strategy.as_deref(), Some("oldest"));
        assert_eq!(result.freed_bytes, 200);
    }

    #[test]
    fn ohne_namens_hinweis_entscheidet_keep_und_der_grund_bleibt_schlicht() {
        let index = seed(&[(r"Z:\Root\A\a.txt", 300, 1), (r"Z:\Root\B\a.txt", 100, 1)]);
        let plan = plan_with(&index, KeepStrategy::Oldest).plan;
        assert_eq!(paths_of(&plan), [r"Z:\Root\A"]);
        assert_eq!(plan.actions[0].reason, "exact-duplicate-dir");
        let plan = plan_with(&index, KeepStrategy::Newest).plan;
        assert_eq!(paths_of(&plan), [r"Z:\Root\B"]);
    }

    #[test]
    fn namens_heuristik_geht_vor_alter() {
        let index = seed(&[
            (r"Z:\Root\Projekt\a.txt", 500, 1),
            (r"Z:\Root\Projekt (2)\a.txt", 100, 1),
            (r"Z:\Root\Backup Projekt\a.txt", 200, 1),
        ]);
        let plan = plan_of(&index).plan;
        assert_eq!(
            paths_of(&plan),
            [r"Z:\Root\Backup Projekt", r"Z:\Root\Projekt (2)"]
        );
        assert!(plan
            .actions
            .iter()
            .all(|a| a.keep.as_deref() == Some(r"Z:\Root\Projekt")));
    }

    #[test]
    fn eigene_kopie_muster_aus_der_config_zaehlen() {
        let index = seed(&[
            (r"Z:\Root\Entwurf\a.txt", 500, 1),
            (r"Z:\Root\Archiv Entwurf\a.txt", 100, 1),
        ]);
        let options = DedupeDirsOptions {
            copy_patterns: vec!["archiv".into()],
            ..DedupeDirsOptions::default()
        };
        let plan = plan_full(
            &index,
            KeepStrategy::Oldest,
            &protector_with(&Config::default()),
            &options,
        )
        .plan;
        assert_eq!(paths_of(&plan), [r"Z:\Root\Archiv Entwurf"]);
    }

    #[test]
    fn drei_gleiche_ordner_behalten_genau_einen() {
        let index = seed(&[
            (r"Z:\Root\A\a.txt", 100, 1),
            (r"Z:\Root\B\a.txt", 200, 1),
            (r"Z:\Root\C\a.txt", 300, 1),
        ]);
        let result = plan_of(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\B", r"Z:\Root\C"]);
        assert_eq!(result.freed_bytes, 200);
        assert_eq!(result.plan.actions[0].id, 1);
        assert_eq!(result.plan.actions[1].id, 2);
    }

    #[test]
    fn nur_die_oberste_ebene_wird_geplant() {
        let index = seed(&[
            (r"Z:\Root\A\sub\b.txt", 100, 1),
            (r"Z:\Root\B\sub\b.txt", 200, 1),
        ]);
        let plan = plan_of(&index).plan;
        assert_eq!(paths_of(&plan), [r"Z:\Root\B"]);
        assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
    }

    #[test]
    fn mitglied_im_behaltenen_ordner_wird_nie_entfernt() {
        // P und Q sind gleich; P\sub, Q\sub und R\sub sind es ebenfalls. P bleibt, Q geht.
        // P\sub liegt im behaltenen P und bleibt, R\sub ist die einzige weitere Kopie.
        let index = seed(&[
            (r"Z:\Root\P\sub\b.txt", 100, 1),
            (r"Z:\Root\P\x.txt", 100, 9),
            (r"Z:\Root\Q\sub\b.txt", 200, 1),
            (r"Z:\Root\Q\x.txt", 200, 9),
            (r"Z:\Root\R\sub\b.txt", 50, 1),
        ]);
        let plan = plan_of(&index).plan;
        let removed = paths_of(&plan);
        assert!(removed.contains(&r"Z:\Root\Q"));
        assert!(
            !removed.iter().any(|p| p.starts_with(r"Z:\Root\P")),
            "{removed:?}"
        );
        assert!(
            !removed.iter().any(|p| p.starts_with(r"Z:\Root\Q\")),
            "Unterordner eines entfernten Ordners entfallen: {removed:?}"
        );
        // R\sub ist eine weitere Kopie von P\sub (im behaltenen Ordner): R\sub wird entfernt.
        assert!(removed.contains(&r"Z:\Root\R\sub"), "{removed:?}");
        let r = plan
            .actions
            .iter()
            .find(|a| a.path.ends_with(r"R\sub"))
            .unwrap();
        assert_eq!(r.keep.as_deref(), Some(r"Z:\Root\P\sub"));
    }

    #[test]
    fn geschuetzter_ordner_wird_weder_entfernt_noch_behalten() {
        let index = seed(&[
            (r"Z:\Root\A\a.txt", 100, 1),
            (r"Z:\Root\B\a.txt", 200, 1),
            (r"Z:\Root\Geschuetzt\a.txt", 50, 1),
        ]);
        let config = Config {
            protected_paths: vec![r"Z:\Root\Geschuetzt".into()],
            ..Config::default()
        };
        let plan = plan_full(
            &index,
            KeepStrategy::Oldest,
            &protector_with(&config),
            &DedupeDirsOptions::default(),
        )
        .plan;
        assert_eq!(paths_of(&plan), [r"Z:\Root\B"]);
        assert_eq!(plan.actions[0].keep.as_deref(), Some(r"Z:\Root\A"));
        assert!(skip_reasons(&plan)
            .contains(&(r"Z:\Root\Geschuetzt".to_string(), SkipReason::Protected)));
    }

    #[test]
    fn bleibt_kein_zweiter_zulaessiger_ordner_gibt_es_keine_aktion() {
        let index = seed(&[
            (r"Z:\Root\A\a.txt", 100, 1),
            (r"Z:\Root\Geschuetzt\a.txt", 50, 1),
        ]);
        let config = Config {
            protected_paths: vec![r"Z:\Root\Geschuetzt".into()],
            ..Config::default()
        };
        let plan = plan_full(
            &index,
            KeepStrategy::Oldest,
            &protector_with(&config),
            &DedupeDirsOptions::default(),
        )
        .plan;
        assert!(plan.actions.is_empty());
        let reasons = skip_reasons(&plan);
        assert!(reasons.contains(&(r"Z:\Root\Geschuetzt".into(), SkipReason::Protected)));
        assert!(reasons.contains(&(r"Z:\Root\A".into(), SkipReason::GroupIncomplete)));
    }

    #[test]
    fn path_strategie_ohne_treffer_laesst_die_gruppe_unberuehrt() {
        let index = seed(&[(r"Z:\Root\A\a.txt", 100, 1), (r"Z:\Root\B\a.txt", 200, 1)]);
        let plan = plan_with(&index, KeepStrategy::PathPrefix(r"Z:\Root\Anders".into())).plan;
        assert!(plan.actions.is_empty());
        assert_eq!(
            skip_reasons(&plan)
                .iter()
                .filter(|(_, r)| *r == SkipReason::GroupIncomplete)
                .count(),
            2
        );
    }

    #[test]
    fn path_strategie_hat_vorrang_vor_der_namens_heuristik() {
        let index = seed(&[
            (r"Z:\Root\A\a.txt", 100, 1),
            (r"Z:\Root\Backup\Kopie von A\a.txt", 200, 1),
        ]);
        let plan = plan_with(&index, KeepStrategy::PathPrefix(r"Z:\Root\Backup".into())).plan;
        assert_eq!(paths_of(&plan), [r"Z:\Root\A"]);
    }

    #[test]
    fn hardlinks_auf_den_behaltenen_ordner_werden_uebersprungen() {
        let index = seed(&[(r"Z:\Root\A\a.txt", 100, 1), (r"Z:\Root\B\a.txt", 200, 1)]);
        set_identity(&index, r"Z:\Root\A\a.txt", 1, 77, 2);
        set_identity(&index, r"Z:\Root\B\a.txt", 1, 77, 2);
        let result = plan_of(&index);
        assert!(result.plan.actions.is_empty());
        assert_eq!(
            skip_reasons(&result.plan),
            [(r"Z:\Root\B".to_string(), SkipReason::Hardlink)]
        );
        assert_eq!(result.freed_bytes, 0);
    }

    #[test]
    fn teilweise_hardlinks_werden_geplant_und_zaehlen_nur_echten_platzgewinn() {
        let index = seed(&[
            (r"Z:\Root\A\a.txt", 100, 1),
            (r"Z:\Root\A\b.txt", 100, 2),
            (r"Z:\Root\B\a.txt", 200, 1),
            (r"Z:\Root\B\b.txt", 200, 2),
        ]);
        set_identity(&index, r"Z:\Root\A\a.txt", 1, 77, 2);
        set_identity(&index, r"Z:\Root\B\a.txt", 1, 77, 2);
        let result = plan_of(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\B"]);
        assert_eq!(result.freed_bytes, 100, "nur b.txt wird wirklich frei");
    }

    #[test]
    fn cloud_datei_macht_den_ordner_zum_nicht_kandidaten() {
        let index = seed_with(
            &[(r"Z:\Root\A\a.txt", 100, 1), (r"Z:\Root\B\a.txt", 200, 1)],
            &[],
            |r| {
                if r.path.contains(r"\B\") {
                    r.cloud_only = true;
                }
            },
        );
        assert!(plan_of(&index).plan.actions.is_empty());
    }

    #[test]
    fn min_size_laesst_kleine_gruppen_ohne_skip_eintrag_weg() {
        let index = seed(&[(r"Z:\Root\A\a.txt", 100, 1), (r"Z:\Root\B\a.txt", 200, 1)]);
        let options = DedupeDirsOptions {
            min_size: Some(101),
            ..DedupeDirsOptions::default()
        };
        let plan = plan_full(
            &index,
            KeepStrategy::Oldest,
            &protector_with(&Config::default()),
            &options,
        )
        .plan;
        assert!(plan.actions.is_empty());
        assert!(plan.skipped.is_empty());
        let options = DedupeDirsOptions {
            min_size: Some(100),
            ..DedupeDirsOptions::default()
        };
        let plan = plan_full(
            &index,
            KeepStrategy::Oldest,
            &protector_with(&Config::default()),
            &options,
        )
        .plan;
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.params.get("min_size").map(String::as_str), Some("100"));
    }

    #[test]
    fn junk_unterschiede_zaehlen_nicht_und_der_ordner_wandert_komplett() {
        let index = seed(&[
            (r"Z:\Root\A\a.txt", 100, 1),
            (r"Z:\Root\B\a.txt", 200, 1),
            (r"Z:\Root\B\Thumbs.db", 200, 5),
        ]);
        let result = plan_of(&index);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\B"]);
        let a = &result.plan.actions[0];
        assert_eq!((a.files, a.size), (Some(2), 200), "inkl. Thumbs.db");
        assert_eq!(result.freed_bytes, 200);
    }

    #[test]
    fn unterschiedliche_ordner_ergeben_einen_leeren_plan() {
        let index = seed(&[(r"Z:\Root\A\a.txt", 100, 1), (r"Z:\Root\B\a.txt", 200, 2)]);
        let plan = plan_of(&index).plan;
        assert!(plan.actions.is_empty());
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn ordner_unter_archiv_nehmen_nicht_teil() {
        let index = seed(&[
            (r"Z:\Root\Projekt\a.txt", 100, 1),
            (r"Z:\Root\_Archiv\2019\Projekt\a.txt", 200, 1),
        ]);
        let plan = plan_of(&index).plan;
        assert!(plan.actions.is_empty(), "{:?}", paths_of(&plan));
        assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
    }

    #[test]
    fn archiv_ordner_ist_weder_zu_entfernen_noch_behaltener_ordner() {
        let index = seed(&[
            (r"Z:\Root\Projekt\a.txt", 100, 1),
            (r"Z:\Root\Kopie von Projekt\a.txt", 200, 1),
            (r"Z:\Root\_Archiv\2019\Projekt\a.txt", 300, 1),
        ]);
        let plan = plan_with(&index, KeepStrategy::Newest).plan;
        assert_eq!(paths_of(&plan), [r"Z:\Root\Kopie von Projekt"]);
        // Das Archiv wäre das neueste, ist aber nie der behaltene Ordner.
        assert_eq!(plan.actions[0].keep.as_deref(), Some(r"Z:\Root\Projekt"));
        assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
    }

    #[test]
    fn der_archivordner_selbst_nimmt_nicht_teil() {
        let index = seed(&[
            (r"Z:\Root\X\2019\Projekt\a.txt", 100, 1),
            (r"Z:\Root\_Archiv\2019\Projekt\a.txt", 200, 1),
        ]);
        let plan = plan_of(&index).plan;
        assert!(plan.actions.is_empty(), "{:?}", paths_of(&plan));
    }

    /// Kein behaltener Ordner liegt auf oder unter der Quelle einer anderen Aktion.
    fn keeps_are_outside_removed(plan: &Plan) -> Result<(), String> {
        for keeper in &plan.actions {
            let keep = paths::path_key(Path::new(keeper.keep.as_deref().unwrap()));
            for other in &plan.actions {
                if paths::is_under(&keep, &paths::path_key(Path::new(&other.path))) {
                    return Err(format!("{} behalten, aber {} entfernt", keep, other.path));
                }
            }
        }
        Ok(())
    }

    /// G1 = { Z, A\B\k, A\B\k2, M\N\k, M\N\k2 } behält `A\B\k` (älteste Datei); G2 = { A\B, M\N }
    /// behielte `M\N` und entfernte damit `A\B`, das den behaltenen `A\B\k` enthält.
    fn nested_groups() -> Index {
        seed(&[
            (r"Z:\Root\Z\f.txt", 300, 1),
            (r"Z:\Root\A\B\k\f.txt", 1, 1),
            (r"Z:\Root\A\B\k2\f.txt", 500, 1),
            (r"Z:\Root\M\N\k\f.txt", 100, 1),
            (r"Z:\Root\M\N\k2\f.txt", 100, 1),
        ])
    }

    /// Keine Quelle liegt unter einer anderen Quelle oder unter dem `keep` einer anderen Aktion.
    fn no_overlap(plan: &Plan) -> Result<(), String> {
        for a in &plan.actions {
            let own = paths::path_key(Path::new(&a.path));
            let keep = paths::path_key(Path::new(a.keep.as_deref().unwrap()));
            for o in plan.actions.iter().filter(|o| o.id != a.id) {
                let other = paths::path_key(Path::new(&o.path));
                if paths::is_under(&other, &own) {
                    return Err(format!("{} liegt in der Quelle {}", o.path, a.path));
                }
                if paths::is_under(&other, &keep) {
                    return Err(format!("{} liegt im behaltenen {keep}", o.path));
                }
            }
        }
        Ok(())
    }

    /// W = P\Y\x = P\Z\x (Gruppe 1, flacher Treffer außerhalb); P\Y = P\Z (Gruppe 2) enthält
    /// je eine Kopie. Gruppe 2 steht höher im Baum und muss zuerst entschieden werden.
    fn wrapped_copies() -> Index {
        seed(&[
            (r"Z:\Root\W\f.txt", 1, 1),
            (r"Z:\Root\P\Y\x\f.txt", 5, 1),
            (r"Z:\Root\P\Z\x\f.txt", 10, 1),
        ])
    }

    #[test]
    fn behaltener_ordner_enthaelt_keinen_entfernten_ordner() {
        let plan = plan_of(&wrapped_copies()).plan;
        assert_eq!(no_overlap(&plan), Ok(()), "{:?}", paths_of(&plan));
        assert_eq!(paths_of(&plan), [r"Z:\Root\P\Z", r"Z:\Root\W"]);
        assert_eq!(plan.actions[0].keep.as_deref(), Some(r"Z:\Root\P\Y"));
        assert_eq!(plan.actions[1].keep.as_deref(), Some(r"Z:\Root\P\Y\x"));
    }

    #[test]
    fn jede_aktion_traegt_den_fingerabdruck_ihrer_quelle() {
        let index = seed(&[
            (r"Z:\Root\Projekt\a.txt", 100, 1),
            (r"Z:\Root\Projekt\sub\b.txt", 100, 2),
            (r"Z:\Root\Kopie von Projekt\a.txt", 200, 1),
            (r"Z:\Root\Kopie von Projekt\sub\b.txt", 200, 2),
        ]);
        let root_key = paths::dir_key(Path::new(ROOT));
        let tree = Tree::new(
            index.dirs_under(&root_key).unwrap(),
            index.files_under(&root_key).unwrap(),
        );
        let plan = plan_of(&index).plan;
        assert_eq!(plan.actions.len(), 1);
        let action = &plan.actions[0];
        let key = paths::dir_key(Path::new(&action.path));
        assert_eq!(
            action.source_fingerprint.as_deref(),
            Some(hex128(meta_print(&tree, &key)).as_str())
        );
        // Die Quelle hat andere mtimes als der behaltene Ordner.
        assert_ne!(action.source_fingerprint, action.keep_fingerprint);
    }

    #[test]
    fn verschachtelte_ordner_zaehlen_beim_platzgewinn_nicht_doppelt() {
        // P\Z (eine Datei) und W (eine Datei) gehen: 2 x 100 Bytes.
        assert_eq!(plan_of(&wrapped_copies()).freed_bytes, 200);
    }

    #[test]
    fn mitglied_mit_behaltenem_unterordner_wird_nie_entfernt() {
        let plan = plan_of(&nested_groups()).plan;
        assert_eq!(
            keeps_are_outside_removed(&plan),
            Ok(()),
            "{:?}",
            paths_of(&plan)
        );
        assert_eq!(no_overlap(&plan), Ok(()), "{:?}", paths_of(&plan));
        // {A\B, M\N} steht höher und wird zuerst entschieden; A\B\k und A\B\k2 entfallen
        // dadurch einzeln.
        let removed = paths_of(&plan);
        assert!(!removed.contains(&r"Z:\Root\A\B\k"), "{removed:?}");
        assert!(!removed.contains(&r"Z:\Root\A\B\k2"), "{removed:?}");
    }

    #[test]
    fn aktionen_sind_nach_pfad_sortiert_und_fortlaufend_nummeriert() {
        let index = seed(&[
            (r"Z:\Root\Z1\a.txt", 100, 1),
            (r"Z:\Root\Z2\a.txt", 200, 1),
            (r"Z:\Root\M1\b.txt", 100, 2),
            (r"Z:\Root\M2\b.txt", 200, 2),
        ]);
        let plan = plan_of(&index).plan;
        assert_eq!(paths_of(&plan), [r"Z:\Root\M2", r"Z:\Root\Z2"]);
        assert_eq!(
            plan.actions.iter().map(|a| a.id).collect::<Vec<_>>(),
            [1, 2]
        );
    }
}
