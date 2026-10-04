//! Soll/Ist-Abgleich: vergleicht den Index mit einer Vorlage. Rein; liest nur Zeilen.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;

use super::Template;
use crate::health::bucket_of;
use crate::report::rows::{DRow, FRow};
use crate::report::ReportRoot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviationKind {
    /// Ein Pflicht-Ordner fehlt
    MissingRequired,
    /// Ein Ordner, den die Ebene nicht vorsieht
    UnexpectedDir,
    /// Ein Ordner, dessen Name keinem Muster der Ebene entspricht
    BadName,
    /// Eine Datei auf einer Ebene ohne Dateien
    FileNotAllowed,
    /// Ein Ordner unterhalb von `max_depth`
    TooDeep,
}

impl DeviationKind {
    pub const ALL: [DeviationKind; 5] = [
        Self::MissingRequired,
        Self::UnexpectedDir,
        Self::BadName,
        Self::FileNotAllowed,
        Self::TooDeep,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::MissingRequired => "missing_required",
            Self::UnexpectedDir => "unexpected_dir",
            Self::BadName => "bad_name",
            Self::FileNotAllowed => "file_not_allowed",
            Self::TooDeep => "too_deep",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::MissingRequired => "Pflicht-Ordner fehlt",
            Self::UnexpectedDir => "Unerwarteter Ordner",
            Self::BadName => "Name passt zu keinem Muster",
            Self::FileNotAllowed => "Datei auf einer Ebene ohne Dateien",
            Self::TooDeep => "Zu tief verschachtelt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Deviation {
    pub kind: DeviationKind,
    pub path: String,
    /// Ebene unter der Wurzel (1 = direkt darunter)
    pub level: usize,
}

#[derive(Debug, Clone, Default)]
pub struct CheckResult {
    /// Sortiert nach Art, dann Pfad
    pub deviations: Vec<Deviation>,
    /// Geprüfte Einträge: Ordner und Dateien auf Ebenen mit Regeln plus Pflicht-Plätze
    pub checked: u64,
    /// Je Ordner der ersten Ebene (`""` = Wurzel): (geprüft, Abweichungen)
    pub by_bucket: BTreeMap<String, (u64, u64)>,
}

fn name_of(path: &str) -> &str {
    path.rsplit('\\').find(|p| !p.is_empty()).unwrap_or(path)
}

fn segments<'a>(root_key: &str, key: &'a str) -> Vec<&'a str> {
    key.strip_prefix(root_key)
        .unwrap_or("")
        .split('\\')
        .filter(|s| !s.is_empty())
        .collect()
}

/// `_Archiv` und `.ordner-cleanup` sind überall erlaubt und werden samt Inhalt nie geprüft.
fn exempt(segments: &[&str]) -> bool {
    segments
        .iter()
        .any(|s| *s == "_archiv" || *s == ".ordner-cleanup")
}

pub fn check(files: &[FRow], dirs: &[DRow], root: &ReportRoot, t: &Template) -> CheckResult {
    let root_key = root.dir_key.as_str();
    let mut result = CheckResult::default();
    let mut record = |kind: Option<(DeviationKind, String, usize)>, bucket: String| {
        result.checked += 1;
        let stats = result.by_bucket.entry(bucket).or_default();
        stats.0 += 1;
        if let Some((kind, path, level)) = kind {
            stats.1 += 1;
            result.deviations.push(Deviation { kind, path, level });
        }
    };

    // Kinder je Ordner (Namen kleingeschrieben) für die Pflicht-Prüfung
    let mut children: HashMap<&str, HashSet<String>> = HashMap::new();
    for d in dirs {
        if let Some(parent) = &d.parent {
            children
                .entry(parent.as_str())
                .or_default()
                .insert(name_of(&d.path).to_lowercase());
        }
    }
    // Ordner je Ebene (für Pflicht-Ordner der Folgeebene)
    let mut by_level: BTreeMap<usize, Vec<&DRow>> = BTreeMap::new();

    for d in dirs {
        if d.key == root_key || d.is_link {
            continue;
        }
        let segs = segments(root_key, &d.key);
        if segs.is_empty() || exempt(&segs) {
            continue;
        }
        let level = segs.len();
        let bucket = bucket_of(root_key, &d.key);
        if let Some(max) = t.max_depth {
            if level > max {
                // Nur der erste zu tiefe Ordner je Zweig zählt, nicht sein ganzer Unterbaum.
                if level == max + 1 {
                    record(
                        Some((DeviationKind::TooDeep, d.path.clone(), level)),
                        bucket,
                    );
                }
                continue;
            }
        }
        by_level.entry(level).or_default().push(d);
        let Some(rule) = t.levels.get(level - 1) else {
            continue;
        };
        let name = name_of(&d.path);
        let allowed = rule.allow_other
            || rule.required.iter().any(|r| r.eq_ignore_ascii_case(name))
            || rule.allowed.iter().any(|a| a.eq_ignore_ascii_case(name))
            || rule.patterns.iter().any(|p| p.is_match(name));
        let kind = (!allowed).then(|| {
            let kind = if rule.patterns.is_empty() {
                DeviationKind::UnexpectedDir
            } else {
                DeviationKind::BadName
            };
            (kind, d.path.clone(), level)
        });
        record(kind, bucket);
    }

    for f in files {
        if f.is_link {
            continue;
        }
        let segs = segments(root_key, &f.dir_key);
        if exempt(&segs) {
            continue;
        }
        if t.max_depth.is_some_and(|max| segs.len() > max) {
            continue;
        }
        let level = segs.len() + 1;
        let Some(rule) = t.levels.get(level - 1) else {
            continue;
        };
        let kind = (!rule.files).then(|| (DeviationKind::FileNotAllowed, f.path.clone(), level));
        record(kind, bucket_of(root_key, &f.dir_key));
    }

    // Pflicht-Ordner: in jedem Ordner der Ebene davor (Ebene 1: die Wurzel)
    for (i, rule) in t.levels.iter().enumerate() {
        if rule.required.is_empty() {
            continue;
        }
        let level = i + 1;
        let parents: Vec<(String, String, String)> = if level == 1 {
            vec![(root.dir_key.clone(), root.path.clone(), String::new())]
        } else {
            by_level
                .get(&(level - 1))
                .into_iter()
                .flatten()
                .filter(|d| !d.summary)
                .map(|d| (d.key.clone(), d.path.clone(), bucket_of(root_key, &d.key)))
                .collect()
        };
        for (key, path, bucket) in parents {
            let present = children.get(key.as_str());
            for name in &rule.required {
                if present.is_some_and(|set| set.contains(&name.to_lowercase())) {
                    continue;
                }
                let missing = format!("{}\\{name}", path.trim_end_matches('\\'));
                record(
                    Some((DeviationKind::MissingRequired, missing, level)),
                    bucket.clone(),
                );
            }
        }
    }

    result.deviations.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });
    result
}
