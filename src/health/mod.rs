//! Kennzahlen je Wurzel und je Ordner der ersten Ebene sowie der Health-Score darüber.
//!
//! Alles ist rein und deterministisch: dieselben Zeilen und dieselbe Config ergeben dieselben
//! Zahlen. Die Zeilen sind dieselben wie im Bericht (`report::rows`).

pub mod collect;
pub mod fingerprint;
pub mod score;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::analysis::age;
use crate::analysis::problems::{self, ProblemCtx};
use crate::analysis::similar;
use crate::analysis::structure::{self, DirFacts, Thresholds};
use crate::change::junk::JunkClassifier;
use crate::index::DupGroup;
use crate::paths;
use crate::report::rows::{DRow, FRow};
use crate::report::{ErrorItem, ReportRoot};
pub use collect::{collect, CollectEnv, Collected};
pub use fingerprint::fingerprint;
pub use score::{score, Deduction, Part, Score, Weights, METRICS_VERSION};

/// Ab so vielen Ordnern der ersten Ebene gibt es nur Zeilen für die größten und eine für „Sonstige“.
pub const MAX_FOLDERS: usize = 200;
/// Name der Sammelzeile für alle weiteren Ordner.
pub const OTHER_FOLDER: &str = "*";

/// Kennzahlen eines Bereichs (Wurzel oder Ordner der ersten Ebene).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metrics {
    /// Größe in Bytes (Hardlink-Geschwister einmal, „Nur Summe“-Ordner mit ihrer Summe)
    pub size: u64,
    /// Davon lokal gespeicherte Bytes (ohne Cloud-Platzhalter)
    pub local_bytes: u64,
    pub files: u64,
    pub dirs: u64,
    pub cloud_files: u64,
    pub cloud_bytes: u64,
    pub junk_files: u64,
    pub junk_bytes: u64,
    pub dup_groups: u64,
    pub dup_wasted: u64,
    pub problem_files: u64,
    pub problems_by_kind: BTreeMap<String, u64>,
    pub empty_dirs: u64,
    pub structure_issues: u64,
    pub version_excess: u64,
    /// Bytes älter als `old_after` außerhalb von `_Archiv` (nur Kennzahl, kein Abzug)
    pub old_bytes: u64,
    pub template_checked: Option<u64>,
    pub template_deviations: Option<u64>,
}

impl Metrics {
    fn add(&mut self, o: &Metrics) {
        self.size += o.size;
        self.local_bytes += o.local_bytes;
        self.files += o.files;
        self.dirs += o.dirs;
        self.cloud_files += o.cloud_files;
        self.cloud_bytes += o.cloud_bytes;
        self.junk_files += o.junk_files;
        self.junk_bytes += o.junk_bytes;
        self.dup_groups += o.dup_groups;
        self.dup_wasted += o.dup_wasted;
        self.problem_files += o.problem_files;
        for (k, v) in &o.problems_by_kind {
            *self.problems_by_kind.entry(k.clone()).or_default() += v;
        }
        self.empty_dirs += o.empty_dirs;
        self.structure_issues += o.structure_issues;
        self.version_excess += o.version_excess;
        self.old_bytes += o.old_bytes;
        if o.template_checked.is_some() || self.template_checked.is_some() {
            self.template_checked =
                Some(self.template_checked.unwrap_or(0) + o.template_checked.unwrap_or(0));
            self.template_deviations =
                Some(self.template_deviations.unwrap_or(0) + o.template_deviations.unwrap_or(0));
        }
    }
}

/// Kennzahlen und Score eines Bereichs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Area {
    /// `""` = Wurzel, `"*"` = Sonstige, sonst der Name des Ordners der ersten Ebene
    pub folder: String,
    pub metrics: Metrics,
    pub score: Score,
}

/// Eine Momentaufnahme: die Wurzel und ihre Ordner der ersten Ebene (größte zuerst).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub root: Area,
    pub folders: Vec<Area>,
}

/// Die Zeilen, aus denen die Kennzahlen entstehen.
pub struct Rows<'a> {
    pub files: &'a [FRow],
    pub dirs: &'a [DRow],
    pub errors: &'a [ErrorItem],
}

pub struct MetricsCtx<'a> {
    pub junk: &'a JunkClassifier,
    pub problem_ctx: &'a ProblemCtx,
    pub thresholds: &'a Thresholds,
    pub old_after_days: i64,
    pub now_ticks: i64,
    pub weights: &'a Weights,
    /// Ergebnis des Soll/Ist-Abgleichs je Bereich (geprüft, Abweichungen); `None` = keine Vorlage
    pub template: Option<&'a BTreeMap<String, (u64, u64)>>,
}

/// Schlüssel des Ordners der ersten Ebene, zu dem ein Schlüssel gehört (`""` = direkt in der Wurzel).
pub(crate) fn bucket_of(root_key: &str, key: &str) -> String {
    let Some(rest) = key.strip_prefix(root_key) else {
        return String::new();
    };
    match rest.split_once('\\') {
        Some((first, _)) if !first.is_empty() => format!("{root_key}{first}\\"),
        _ => String::new(),
    }
}

fn name_of(path: &str) -> String {
    path.rsplit('\\')
        .find(|p| !p.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn archive_key(root_key: &str) -> String {
    format!("{root_key}_archiv\\")
}

/// Berechnet die Momentaufnahme. `dups` sind die exakten Duplikatgruppen der Wurzel.
pub fn compute(rows: &Rows, dups: &[DupGroup], root: &ReportRoot, ctx: &MetricsCtx) -> Snapshot {
    let root_key = root.dir_key.as_str();
    let archive = archive_key(root_key);
    let mut acc: BTreeMap<String, Metrics> = BTreeMap::new();
    acc.entry(String::new()).or_default();

    // Namen der Ordner der ersten Ebene (Schreibweise aus dem Index).
    let names: HashMap<&str, String> = rows
        .dirs
        .iter()
        .filter(|d| d.parent.as_deref() == Some(root_key))
        .map(|d| (d.key.as_str(), name_of(&d.path)))
        .collect();

    let real_files: Vec<&FRow> = rows.files.iter().filter(|f| !f.is_link).collect();

    // --- Dateien (parallel: je Thread eine Teilsumme, danach zusammenführen) ---
    let file_acc: BTreeMap<String, Metrics> = real_files
        .par_iter()
        .fold(BTreeMap::new, |mut acc: BTreeMap<String, Metrics>, f| {
            let m = acc.entry(bucket_of(root_key, &f.dir_key)).or_default();
            m.files += 1;
            if f.counted {
                m.size += f.size;
                if !f.cloud {
                    m.local_bytes += f.size;
                }
            }
            if f.cloud {
                m.cloud_files += 1;
                if f.counted {
                    m.cloud_bytes += f.size;
                }
            } else if ctx
                .junk
                .classify(&f.name, &f.dir_key, f.mtime, ctx.now_ticks)
                .is_some()
            {
                m.junk_files += 1;
                if f.counted {
                    m.junk_bytes += f.size;
                }
            }
            let found = problems::check_file(&f.name, &f.path, f.size, ctx.problem_ctx);
            if !found.is_empty() {
                m.problem_files += 1;
                for p in &found {
                    *m.problems_by_kind.entry(p.key().to_string()).or_default() += 1;
                }
            }
            if f.counted
                && !paths::is_under(&f.dir_key, &archive)
                && age::is_old(f.mtime, ctx.now_ticks, ctx.old_after_days)
            {
                m.old_bytes += f.size;
            }
            acc
        })
        .reduce(BTreeMap::new, |mut a, b| {
            for (bucket, m) in b {
                a.entry(bucket).or_default().add(&m);
            }
            a
        });
    for (bucket, m) in file_acc {
        acc.entry(bucket).or_default().add(&m);
    }

    // --- Ordner ---
    let root_depth = rows.dirs.iter().find(|d| d.key == root_key).map_or_else(
        || rows.dirs.iter().map(|d| d.depth).min().unwrap_or(0),
        |d| d.depth,
    );
    let error_keys: HashSet<String> = rows
        .errors
        .iter()
        .map(|e| paths::dir_key(Path::new(&e.path)))
        .collect();
    let mut file_counts: HashMap<&str, usize> = HashMap::new();
    for f in &real_files {
        *file_counts.entry(f.dir_key.as_str()).or_default() += 1;
    }
    for d in rows.dirs {
        let m = acc.entry(bucket_of(root_key, &d.key)).or_default();
        m.dirs += 1;
        if d.summary {
            m.files += d.summary_files;
            m.size += d.summary_size;
            m.local_bytes += d.summary_size;
        }
        let facts = DirFacts {
            rel_depth: (d.depth - root_depth).max(0) as usize,
            direct_entries: d.direct_entries as usize,
            direct_files: file_counts.get(d.key.as_str()).copied().unwrap_or(0),
            skipped: d.is_link || d.summary || error_keys.contains(&d.key),
        };
        for issue in structure::check_dir(&facts, ctx.thresholds) {
            m.structure_issues += 1;
            if issue == structure::StructureIssue::Empty {
                m.empty_dirs += 1;
            }
        }
    }

    // --- Versionen: Gruppen ähnlicher Dateien je Ordner, ohne `_Archiv` ---
    let mut per_dir: BTreeMap<&str, Vec<&FRow>> = BTreeMap::new();
    for f in &real_files {
        if !paths::is_under(&f.dir_key, &archive) {
            per_dir.entry(f.dir_key.as_str()).or_default().push(f);
        }
    }
    let excess: Vec<(String, u64)> = per_dir
        .par_iter()
        .filter_map(|(dir_key, members)| {
            let names: Vec<&str> = members.iter().map(|f| f.name.as_str()).collect();
            let excess: u64 = similar::group_similar(&names)
                .iter()
                .map(|g| g.len().saturating_sub(1) as u64)
                .sum();
            (excess > 0).then(|| (bucket_of(root_key, dir_key), excess))
        })
        .collect();
    for (bucket, excess) in excess {
        acc.entry(bucket).or_default().version_excess += excess;
    }

    // --- Duplikate: jede weitere Instanz zählt in ihrem Ordner als verschwendet ---
    let mut root_groups = 0;
    for group in dups {
        let mut files: Vec<&crate::index::DupFile> = group.files.iter().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let mut seen: HashSet<(i64, i64)> = HashSet::new();
        let mut touched: HashSet<String> = HashSet::new();
        for (i, file) in files.iter().enumerate() {
            let first = i == 0;
            let is_new = file.identity.is_none_or(|id| seen.insert(id));
            if first {
                continue;
            }
            if !is_new {
                continue;
            }
            let bucket = bucket_of(root_key, &paths::path_key(Path::new(&file.path)));
            acc.entry(bucket.clone()).or_default().dup_wasted += group.size.max(0) as u64;
            touched.insert(bucket);
        }
        root_groups += u64::from(!touched.is_empty());
        for bucket in touched {
            acc.entry(bucket).or_default().dup_groups += 1;
        }
    }

    if let Some(stats) = ctx.template {
        for (bucket, (checked, deviations)) in stats {
            let m = acc.entry(bucket.clone()).or_default();
            m.template_checked = Some(*checked);
            m.template_deviations = Some(*deviations);
        }
        for m in acc.values_mut() {
            m.template_checked.get_or_insert(0);
            m.template_deviations.get_or_insert(0);
        }
    }

    finish(acc, root_groups, &names, ctx.weights)
}

/// Fasst die Bereiche zusammen und berechnet die Scores.
fn finish(
    acc: BTreeMap<String, Metrics>,
    root_dup_groups: u64,
    names: &HashMap<&str, String>,
    weights: &Weights,
) -> Snapshot {
    let mut root_metrics = Metrics::default();
    for m in acc.values() {
        root_metrics.add(m);
    }
    // Eine Gruppe über mehrere Ordner zählt in jedem Ordner, in der Wurzel aber nur einmal.
    root_metrics.dup_groups = root_dup_groups;
    let mut folders: Vec<(String, Metrics)> = acc
        .into_iter()
        .filter(|(key, _)| !key.is_empty())
        .map(|(key, m)| {
            let name = names
                .get(key.as_str())
                .cloned()
                .unwrap_or_else(|| name_of(&key));
            (name, m)
        })
        .collect();
    folders.sort_by(|a, b| b.1.size.cmp(&a.1.size).then_with(|| a.0.cmp(&b.0)));
    if folders.len() > MAX_FOLDERS {
        let mut rest = Metrics::default();
        for (_, m) in folders.drain(MAX_FOLDERS..) {
            rest.add(&m);
        }
        folders.push((OTHER_FOLDER.to_string(), rest));
    }
    let area = |folder: String, metrics: Metrics| {
        let score = score::score(&metrics, weights);
        Area {
            folder,
            metrics,
            score,
        }
    };
    Snapshot {
        root: area(String::new(), root_metrics),
        folders: folders.into_iter().map(|(n, m)| area(n, m)).collect(),
    }
}

#[cfg(test)]
mod tests;
