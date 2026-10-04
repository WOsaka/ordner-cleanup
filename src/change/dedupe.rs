//! Planer für exakte Duplikate: liest nur den Index und verändert nichts.
//!
//! Sicherheitsregeln: Pro Gruppe bleibt immer mindestens eine Datei unberührt; geschützte
//! Dateien werden weder entfernt noch als „keep“ gewählt; Hardlinks auf die behaltene Datei
//! zählen nicht; Cloud-Platzhalter und Links kennt der Index in Gruppen gar nicht (sie werden
//! nie gehasht).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use super::plan::{
    hex, ticks_to_rfc3339, ActionType, Plan, PlanKind, PlannedAction, Skipped, PLAN_VERSION,
};
use super::protect::Protector;
use super::quarantine;
use super::SkipReason;
use crate::index::{DupFile, Index, IndexError};
use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepStrategy {
    Oldest,
    Newest,
    /// Behalte die älteste Kopie unterhalb dieses (absoluten) Ordners.
    PathPrefix(String),
}

impl FromStr for KeepStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "oldest" => Ok(Self::Oldest),
            "newest" => Ok(Self::Newest),
            _ => match s.strip_prefix("path:") {
                Some(prefix) if Path::new(prefix).is_absolute() => {
                    Ok(Self::PathPrefix(prefix.to_string()))
                }
                Some(_) => Err("path:<prefix> braucht einen absoluten Ordnerpfad".into()),
                None => Err(format!(
                    "Unbekannte Strategie „{s}“ (erlaubt: oldest, newest, path:<prefix>)"
                )),
            },
        }
    }
}

impl fmt::Display for KeepStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oldest => f.write_str("oldest"),
            Self::Newest => f.write_str("newest"),
            Self::PathPrefix(p) => write!(f, "path:{p}"),
        }
    }
}

#[derive(Debug)]
pub struct DedupePlan {
    pub plan: Plan,
    /// Tatsächlich freiwerdende Bytes (Hardlinks, die noch anderswo verlinkt sind, zählen nicht).
    pub freed_bytes: u64,
}

struct Candidate<'a> {
    file: &'a DupFile,
    key: String,
}

/// Reihenfolge der Wahl: Strategie, dann kürzester Pfad, dann alphabetisch.
fn preference(strategy: &KeepStrategy, a: &Candidate, b: &Candidate) -> Ordering {
    let by_time = match strategy {
        KeepStrategy::Newest => b.file.mtime.cmp(&a.file.mtime),
        _ => a.file.mtime.cmp(&b.file.mtime),
    };
    by_time
        .then_with(|| a.key.len().cmp(&b.key.len()))
        .then_with(|| a.key.cmp(&b.key))
}

pub fn plan_dedupe(
    index: &Index,
    root: &Path,
    strategy: &KeepStrategy,
    protector: &Protector,
    created: &str,
) -> Result<DedupePlan, IndexError> {
    let mut actions: Vec<(String, PlannedAction)> = Vec::new();
    let mut skipped: Vec<(String, Skipped)> = Vec::new();
    let mut freed_bytes = 0u64;
    let prefix_key = match strategy {
        KeepStrategy::PathPrefix(p) => Some(paths::path_key(Path::new(p))),
        _ => None,
    };

    for group in index.exact_duplicate_groups(&paths::dir_key(root))? {
        let size = group.size.max(0) as u64;
        let hash = hex(&group.hash);
        let mut note = |file: &DupFile, reason| {
            skipped.push((
                paths::path_key(Path::new(&file.path)),
                Skipped {
                    path: file.path.clone(),
                    reason,
                },
            ));
        };

        let mut eligible = Vec::new();
        for file in &group.files {
            if protector.check(Path::new(&file.path)).is_some() {
                note(file, SkipReason::Protected);
            } else {
                eligible.push(Candidate {
                    file,
                    key: paths::path_key(Path::new(&file.path)),
                });
            }
        }

        let keep = eligible
            .iter()
            .filter(|c| {
                prefix_key
                    .as_ref()
                    .is_none_or(|p| paths::is_under(&c.key, p))
            })
            .min_by(|a, b| preference(strategy, a, b));
        let Some(keep) = keep.filter(|_| eligible.len() >= 2) else {
            for c in &eligible {
                note(c.file, SkipReason::GroupIncomplete);
            }
            continue;
        };

        let mut removed: Vec<&Candidate> = Vec::new();
        for c in eligible.iter().filter(|c| c.key != keep.key) {
            if c.file.identity.is_some() && c.file.identity == keep.file.identity {
                note(c.file, SkipReason::Hardlink);
            } else if let Err(reason) = quarantine::fits(root, Path::new(&c.file.path)) {
                note(c.file, reason);
            } else {
                removed.push(c);
            }
        }

        // Platzgewinn nur, wenn alle Namen einer Datei-Identität entfernt werden.
        let mut by_identity: HashMap<(i64, i64), (i64, i64)> = HashMap::new();
        for c in &removed {
            match c.file.identity {
                Some(id) => by_identity.entry(id).or_insert((0, c.file.nlinks.max(1))).0 += 1,
                None => freed_bytes += size,
            }
        }
        freed_bytes += by_identity
            .values()
            .filter(|(count, nlinks)| count >= nlinks)
            .count() as u64
            * size;

        for c in removed {
            actions.push((
                c.key.clone(),
                PlannedAction {
                    id: 0,
                    action: ActionType::Quarantine,
                    path: c.file.path.clone(),
                    size,
                    mtime_ticks: c.file.mtime,
                    mtime: ticks_to_rfc3339(c.file.mtime),
                    hash: Some(hash.clone()),
                    keep: Some(keep.file.path.clone()),
                    keep_hash: Some(hash.clone()),
                    reason: "exact-duplicate".into(),
                    target: None,
                    is_dir: false,
                    files: None,
                    rule: None,
                },
            ));
        }
    }

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
    Ok(DedupePlan {
        plan: Plan {
            version: PLAN_VERSION,
            created: created.to_string(),
            kind: PlanKind::Dedupe,
            root: paths::display(root),
            keep_strategy: Some(strategy.to_string()),
            params: Default::default(),
            actions,
            skipped: skipped.into_iter().map(|(_, s)| s).collect(),
        },
        freed_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::protect::ProtectPaths;
    use crate::config::Config;
    use crate::index::FileRecord;
    use rstest::rstest;

    const ROOT: &str = r"Z:\Root";

    /// (Pfad, mtime, Hash-Byte, Identität `(volume, file_index, nlinks)`)
    type Seed<'a> = (&'a str, i64, u8, Option<(i64, i64, i64)>);

    fn seed(files: &[Seed]) -> Index {
        let mut index = Index::open_in_memory().unwrap();
        let dir_key = paths::dir_key(Path::new(ROOT));
        let run = index.begin_root(ROOT, &dir_key, "t").unwrap();
        let records: Vec<FileRecord> = files
            .iter()
            .map(|(path, mtime, hash, _)| {
                let p = Path::new(path);
                FileRecord {
                    dir_key: paths::dir_key(p.parent().unwrap()),
                    path: (*path).into(),
                    path_key: paths::path_key(p),
                    name: p.file_name().unwrap().to_string_lossy().into_owned(),
                    ext: None,
                    size: 100,
                    mtime: *mtime,
                    ctime: None,
                    first_seen: None,
                    attrs: 0x20,
                    cloud_only: false,
                    is_link: false,
                    link_target: None,
                    partial_hash: Some(vec![*hash; 16]),
                    full_hash: Some(vec![*hash; 16]),
                    hash_status: Some("ok".into()),
                }
            })
            .collect();
        index.upsert_files(&records, run.generation).unwrap();
        for (path, _, _, identity) in files {
            if let Some((volume, file_index, nlinks)) = identity {
                index
                    .conn()
                    .execute(
                        "UPDATE files SET volume_serial=?1, file_index=?2, nlinks=?3 WHERE path_key=?4",
                        rusqlite::params![volume, file_index, nlinks, paths::path_key(Path::new(path))],
                    )
                    .unwrap();
            }
        }
        index
    }

    fn protector() -> Protector {
        Protector::new(
            Path::new(ROOT),
            &Config::default(),
            &ProtectPaths::default(),
        )
    }

    fn plan_with(index: &Index, strategy: KeepStrategy) -> DedupePlan {
        let result = plan_dedupe(index, Path::new(ROOT), &strategy, &protector(), "t").unwrap();
        result.plan.validate().unwrap();
        result
    }

    fn paths_of(plan: &Plan) -> Vec<&str> {
        plan.actions.iter().map(|a| a.path.as_str()).collect()
    }

    #[test]
    fn oldest_behaelt_die_aelteste_und_plant_den_rest_sortiert() {
        let index = seed(&[
            (r"Z:\Root\c\kopie.txt", 300, 1, None),
            (r"Z:\Root\a\orig.txt", 100, 1, None),
            (r"Z:\Root\b\kopie.txt", 200, 1, None),
        ]);
        let result = plan_with(&index, KeepStrategy::Oldest);
        let plan = &result.plan;
        assert_eq!(
            paths_of(plan),
            [r"Z:\Root\b\kopie.txt", r"Z:\Root\c\kopie.txt"]
        );
        assert_eq!(plan.actions[0].id, 1);
        assert_eq!(plan.actions[1].id, 2);
        assert!(plan
            .actions
            .iter()
            .all(|a| a.keep.as_deref() == Some(r"Z:\Root\a\orig.txt")));
        assert_eq!(
            plan.actions[0].hash.as_deref(),
            Some("01".repeat(16).as_str())
        );
        assert_eq!(plan.actions[0].keep_hash, plan.actions[0].hash);
        assert_eq!(plan.actions[0].mtime_ticks, 200);
        assert_eq!(plan.keep_strategy.as_deref(), Some("oldest"));
        assert_eq!(result.freed_bytes, 200);
    }

    #[test]
    fn newest_behaelt_die_neueste() {
        let index = seed(&[
            (r"Z:\Root\a.txt", 100, 1, None),
            (r"Z:\Root\b.txt", 300, 1, None),
            (r"Z:\Root\c.txt", 200, 1, None),
        ]);
        let plan = plan_with(&index, KeepStrategy::Newest).plan;
        assert_eq!(paths_of(&plan), [r"Z:\Root\a.txt", r"Z:\Root\c.txt"]);
        assert_eq!(plan.actions[0].keep.as_deref(), Some(r"Z:\Root\b.txt"));
    }

    #[test]
    fn gleichstand_entscheidet_kuerzester_pfad_dann_alphabet() {
        let index = seed(&[
            (r"Z:\Root\sub\lang\x.txt", 100, 1, None),
            (r"Z:\Root\b.txt", 100, 1, None),
            (r"Z:\Root\a.txt", 100, 1, None),
        ]);
        let plan = plan_with(&index, KeepStrategy::Oldest).plan;
        assert!(plan
            .actions
            .iter()
            .all(|a| a.keep.as_deref() == Some(r"Z:\Root\a.txt")));
    }

    #[test]
    fn path_strategie_behaelt_kopie_im_praefix() {
        let index = seed(&[
            (r"Z:\Root\alt\x.txt", 100, 1, None),
            (r"Z:\Root\Archiv\x.txt", 300, 1, None),
            (r"Z:\Root\Archiv2\x.txt", 50, 1, None),
        ]);
        // Kleinschreibung im Präfix: die Wahl ist unabhängig von der Schreibweise.
        let strategy: KeepStrategy = r"path:Z:\root\archiv".parse().unwrap();
        let plan = plan_with(&index, strategy).plan;
        assert_eq!(plan.actions.len(), 2);
        assert!(plan
            .actions
            .iter()
            .all(|a| a.keep.as_deref() == Some(r"Z:\Root\Archiv\x.txt")));
    }

    #[test]
    fn path_strategie_ohne_treffer_laesst_die_gruppe_unberuehrt() {
        let index = seed(&[
            (r"Z:\Root\a.txt", 100, 1, None),
            (r"Z:\Root\b.txt", 200, 1, None),
        ]);
        let plan = plan_with(&index, KeepStrategy::PathPrefix(r"Z:\Root\leer".into())).plan;
        assert!(plan.actions.is_empty());
        assert_eq!(plan.skipped.len(), 2);
        assert!(plan
            .skipped
            .iter()
            .all(|s| s.reason == SkipReason::GroupIncomplete));
    }

    #[test]
    fn geschuetzte_datei_wird_weder_entfernt_noch_behalten() {
        let index = seed(&[
            (r"Z:\Root\proj\.git\obj.txt", 10, 1, None),
            (r"Z:\Root\a.txt", 100, 1, None),
            (r"Z:\Root\b.txt", 200, 1, None),
        ]);
        let plan = plan_with(&index, KeepStrategy::Oldest).plan;
        // Die älteste ist geschützt und wird nicht gewählt; a.txt bleibt, b.txt geht.
        assert_eq!(paths_of(&plan), [r"Z:\Root\b.txt"]);
        assert_eq!(plan.actions[0].keep.as_deref(), Some(r"Z:\Root\a.txt"));
        assert_eq!(
            plan.skipped,
            [Skipped {
                path: r"Z:\Root\proj\.git\obj.txt".into(),
                reason: SkipReason::Protected
            }]
        );
    }

    #[test]
    fn nur_eine_zulaessige_datei_ergibt_keine_aktion() {
        let index = seed(&[
            (r"Z:\Root\node_modules\x.txt", 10, 1, None),
            (r"Z:\Root\a.txt", 100, 1, None),
        ]);
        let plan = plan_with(&index, KeepStrategy::Oldest).plan;
        assert!(plan.actions.is_empty());
        let reasons: Vec<_> = plan.skipped.iter().map(|s| s.reason).collect();
        assert_eq!(reasons.len(), 2);
        assert!(reasons.contains(&SkipReason::Protected));
        assert!(reasons.contains(&SkipReason::GroupIncomplete));
    }

    #[test]
    fn zu_lange_quarantaene_ziele_werden_uebersprungen() {
        let long = format!(r"Z:\Root\{}\x.txt", "a".repeat(32_000));
        let index = seed(&[
            (r"Z:\Root\a.txt", 100, 1, None),
            (long.as_str(), 200, 1, None),
        ]);
        let plan = plan_with(&index, KeepStrategy::Oldest).plan;
        assert!(plan.actions.is_empty());
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].reason, SkipReason::TooLong);
    }

    #[test]
    fn hardlinks_der_behaltenen_datei_werden_nicht_aufgenommen() {
        let index = seed(&[
            (r"Z:\Root\a.txt", 100, 1, Some((1, 10, 2))),
            (r"Z:\Root\a-link.txt", 100, 1, Some((1, 10, 2))),
            (r"Z:\Root\kopie.txt", 200, 1, Some((1, 11, 1))),
        ]);
        let result = plan_with(&index, KeepStrategy::Oldest);
        assert_eq!(paths_of(&result.plan), [r"Z:\Root\kopie.txt"]);
        assert_eq!(result.freed_bytes, 100);
        assert_eq!(
            result.plan.skipped,
            [Skipped {
                path: r"Z:\Root\a-link.txt".into(),
                reason: SkipReason::Hardlink
            }]
        );
    }

    #[test]
    fn hardlink_paar_ausserhalb_des_keep_zaehlt_den_platz_einmal() {
        let index = seed(&[
            (r"Z:\Root\a.txt", 100, 1, Some((1, 10, 1))),
            (r"Z:\Root\l1.txt", 200, 1, Some((1, 11, 2))),
            (r"Z:\Root\l2.txt", 200, 1, Some((1, 11, 2))),
        ]);
        let result = plan_with(&index, KeepStrategy::Oldest);
        assert_eq!(result.plan.actions.len(), 2);
        assert_eq!(result.freed_bytes, 100);
    }

    #[test]
    fn hardlink_mit_weiterem_link_ausserhalb_gibt_keinen_platz_frei() {
        let index = seed(&[
            (r"Z:\Root\a.txt", 100, 1, Some((1, 10, 1))),
            (r"Z:\Root\l1.txt", 200, 1, Some((1, 11, 3))),
            (r"Z:\Root\l2.txt", 200, 1, Some((1, 11, 3))),
        ]);
        assert_eq!(plan_with(&index, KeepStrategy::Oldest).freed_bytes, 0);
    }

    #[rstest]
    #[case(KeepStrategy::Oldest)]
    #[case(KeepStrategy::Newest)]
    #[case(KeepStrategy::PathPrefix(r"Z:\Root\b".into()))]
    fn pro_gruppe_bleibt_immer_mindestens_eine_datei(#[case] strategy: KeepStrategy) {
        let index = seed(&[
            (r"Z:\Root\a\1.txt", 100, 1, None),
            (r"Z:\Root\b\1.txt", 200, 1, None),
            (r"Z:\Root\c\1.txt", 300, 1, None),
            (r"Z:\Root\a\2.txt", 100, 2, None),
            (r"Z:\Root\b\2.txt", 100, 2, None),
            (r"Z:\Root\a\3.txt", 100, 3, None),
            (r"Z:\Root\.git\3.txt", 100, 3, None),
        ]);
        let plan = plan_with(&index, strategy).plan;
        // (Hash-Byte, Dateien der Gruppe im Index)
        for (byte, total) in [(1u8, 3), (2, 2), (3, 2)] {
            let hash = hex(&[byte; 16]);
            let planned = plan
                .actions
                .iter()
                .filter(|a| a.hash.as_deref() == Some(hash.as_str()))
                .count();
            assert!(planned < total, "Gruppe {hash} vollständig im Plan");
        }
    }

    #[test]
    fn dateien_ausserhalb_der_wurzel_bleiben_unberuehrt() {
        let mut index = seed(&[
            (r"Z:\Root\a.txt", 100, 1, None),
            (r"Z:\Root\b.txt", 200, 1, None),
        ]);
        let other = r"Z:\Anders";
        let run = index
            .begin_root(other, &paths::dir_key(Path::new(other)), "t")
            .unwrap();
        let rec = |name: &str| {
            let path = format!(r"Z:\Anders\{name}");
            FileRecord {
                dir_key: paths::dir_key(Path::new(other)),
                path_key: paths::path_key(Path::new(&path)),
                path,
                name: name.into(),
                ext: None,
                size: 100,
                mtime: 5,
                ctime: None,
                first_seen: None,
                attrs: 0x20,
                cloud_only: false,
                is_link: false,
                link_target: None,
                partial_hash: Some(vec![1; 16]),
                full_hash: Some(vec![1; 16]),
                hash_status: Some("ok".into()),
            }
        };
        index
            .upsert_files(&[rec("x.txt"), rec("y.txt")], run.generation)
            .unwrap();
        let plan = plan_with(&index, KeepStrategy::Oldest).plan;
        assert_eq!(plan.actions.len(), 1);
        assert!(plan.actions.iter().all(|a| a.path.starts_with(r"Z:\Root")));
    }

    #[test]
    fn ohne_duplikate_ist_der_plan_leer() {
        let index = seed(&[
            (r"Z:\Root\a.txt", 100, 1, None),
            (r"Z:\Root\b.txt", 100, 2, None),
        ]);
        let result = plan_with(&index, KeepStrategy::Oldest);
        assert!(result.plan.actions.is_empty() && result.plan.skipped.is_empty());
        assert_eq!(result.freed_bytes, 0);
    }

    #[rstest]
    #[case("oldest", Some(KeepStrategy::Oldest))]
    #[case("newest", Some(KeepStrategy::Newest))]
    #[case(r"path:D:\Archiv", Some(KeepStrategy::PathPrefix(r"D:\Archiv".into())))]
    #[case("path:Archiv", None)]
    #[case("path:", None)]
    #[case("zufällig", None)]
    fn strategie_parsen(#[case] text: &str, #[case] expected: Option<KeepStrategy>) {
        assert_eq!(text.parse::<KeepStrategy>().ok(), expected);
    }
}
