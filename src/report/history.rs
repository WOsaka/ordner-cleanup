//! Abschnitt „Verlauf“ des Berichts: Score, größte Abzüge, Veränderung zum letzten vergleichbaren
//! Lauf und die Reihen für die Trendgrafiken. Das Modell ist rein; Index und Verlauf liest der
//! Aufrufer.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::health::{Deduction, Metrics, Part, Snapshot};
use crate::history::{Comparable, Point, Stored};

/// Wie viele Abzüge der Bericht nennt.
pub const TOP_DEDUCTIONS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Count,
    Bytes,
}

struct MetricDef {
    key: &'static str,
    label: &'static str,
    unit: Unit,
    get: fn(&Metrics) -> u64,
}

const METRICS: [MetricDef; 11] = [
    MetricDef {
        key: "files",
        label: "Dateien",
        unit: Unit::Count,
        get: |m| m.files,
    },
    MetricDef {
        key: "size",
        label: "Größe",
        unit: Unit::Bytes,
        get: |m| m.size,
    },
    MetricDef {
        key: "junk_files",
        label: "Müll-Dateien",
        unit: Unit::Count,
        get: |m| m.junk_files,
    },
    MetricDef {
        key: "junk_bytes",
        label: "Müll",
        unit: Unit::Bytes,
        get: |m| m.junk_bytes,
    },
    MetricDef {
        key: "dup_wasted",
        label: "Duplikate (verschwendet)",
        unit: Unit::Bytes,
        get: |m| m.dup_wasted,
    },
    MetricDef {
        key: "problem_files",
        label: "Problemdateien",
        unit: Unit::Count,
        get: |m| m.problem_files,
    },
    MetricDef {
        key: "structure_issues",
        label: "Strukturbefunde",
        unit: Unit::Count,
        get: |m| m.structure_issues,
    },
    MetricDef {
        key: "empty_dirs",
        label: "Leere Ordner",
        unit: Unit::Count,
        get: |m| m.empty_dirs,
    },
    MetricDef {
        key: "version_excess",
        label: "Überzählige Versionen",
        unit: Unit::Count,
        get: |m| m.version_excess,
    },
    MetricDef {
        key: "old_bytes",
        label: "Alte Daten (nur Kennzahl)",
        unit: Unit::Bytes,
        get: |m| m.old_bytes,
    },
    MetricDef {
        key: "cloud_bytes",
        label: "Nur in der Cloud",
        unit: Unit::Bytes,
        get: |m| m.cloud_bytes,
    },
];

const TEMPLATE_DEVIATIONS: MetricDef = MetricDef {
    key: "template_deviations",
    label: "Vorlagen-Abweichungen",
    unit: Unit::Count,
    get: |m| m.template_deviations.unwrap_or(0),
};

#[derive(Debug, Clone, Serialize)]
pub struct PartScore {
    pub key: String,
    pub label: String,
    /// `None` = nicht anwendbar oder abgeschaltet
    pub value: Option<f64>,
}

/// Vergleich mit dem letzten vergleichbaren Lauf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Comparison {
    /// UTC, RFC 3339
    pub previous_at: String,
    /// `full` oder `limited` (Config oder Formeln haben sich geändert)
    pub kind: String,
    pub previous_score: u8,
    pub delta: i32,
}

impl Comparison {
    pub fn new(score: u8, previous: &Stored, kind: Comparable) -> Self {
        let previous_score = previous.snapshot.root.score.total;
        Self {
            previous_at: previous.meta.taken_at.clone(),
            kind: match kind {
                Comparable::Full => "full",
                Comparable::Limited => "limited",
            }
            .to_string(),
            previous_score,
            delta: i32::from(score) - i32::from(previous_score),
        }
    }

    pub fn is_limited(&self) -> bool {
        self.kind == "limited"
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricRow {
    pub key: String,
    pub label: String,
    pub unit: Unit,
    pub now: u64,
    pub previous: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderRow {
    pub folder: String,
    pub score: u8,
    pub previous_score: Option<u8>,
    pub size: u64,
    pub files: u64,
    pub top_deduction: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesPoint {
    /// UTC, RFC 3339
    pub taken_at: String,
    pub score: u8,
    pub values: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistorySection {
    pub score: u8,
    pub parts: Vec<PartScore>,
    pub deductions: Vec<Deduction>,
    /// `None` = erster Lauf
    pub comparison: Option<Comparison>,
    pub metrics: Vec<MetricRow>,
    pub folders: Vec<FolderRow>,
    pub series: Vec<SeriesPoint>,
    /// Hinweise, z. B. warum kein Verlauf vorliegt oder dass ein Lauf ausgefallen ist
    pub notes: Vec<String>,
}

pub fn format_delta(delta: i32) -> String {
    match delta {
        0 => "±0".to_string(),
        d if d > 0 => format!("+{d}"),
        d => format!("−{}", -d),
    }
}

/// „27.09.“ in Ortszeit aus einem RFC-3339-Zeitpunkt.
pub fn short_date(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| t.with_timezone(&chrono::Local).format("%d.%m.").to_string())
        .unwrap_or_else(|_| rfc3339.to_string())
}

/// „Health-Score 72 (−5 seit 27.09.)“
pub fn score_line(score: u8, comparison: Option<&Comparison>) -> String {
    match comparison {
        None => format!("Health-Score {score} (erster Lauf)"),
        Some(c) => format!(
            "Health-Score {score} ({} seit {}{})",
            format_delta(c.delta),
            short_date(&c.previous_at),
            if c.is_limited() {
                ", eingeschränkt vergleichbar"
            } else {
                ""
            }
        ),
    }
}

fn part_scores(score: &crate::health::Score) -> Vec<PartScore> {
    Part::ALL
        .iter()
        .zip(&score.parts)
        .map(|(part, (_, value))| PartScore {
            key: part.key().to_string(),
            label: part.label().to_string(),
            value: *value,
        })
        .collect()
}

/// Baut den Abschnitt. `now` ist die frisch berechnete Momentaufnahme, `previous` der Lauf, mit
/// dem verglichen wird, `series` die Punkte der Wurzel (älteste zuerst).
pub fn section(
    now: &Snapshot,
    previous: Option<(&Stored, Comparable)>,
    series: &[Point],
    notes: Vec<String>,
) -> HistorySection {
    let score = now.root.score.total;
    let comparison = previous.map(|(p, kind)| Comparison::new(score, p, kind));
    let prev_root = previous.map(|(p, _)| &p.snapshot.root.metrics);

    let mut defs: Vec<&MetricDef> = METRICS.iter().collect();
    if now.root.metrics.template_checked.is_some() {
        defs.push(&TEMPLATE_DEVIATIONS);
    }
    let metrics = defs
        .iter()
        .map(|d| MetricRow {
            key: d.key.to_string(),
            label: d.label.to_string(),
            unit: d.unit,
            now: (d.get)(&now.root.metrics),
            previous: prev_root.map(d.get),
        })
        .collect();

    let folders = now
        .folders
        .iter()
        .map(|f| FolderRow {
            folder: f.folder.clone(),
            score: f.score.total,
            previous_score: previous.and_then(|(p, _)| {
                p.snapshot
                    .folders
                    .iter()
                    .find(|o| o.folder == f.folder)
                    .map(|o| o.score.total)
            }),
            size: f.metrics.size,
            files: f.metrics.files,
            top_deduction: f.score.deductions.first().map(|d| d.text.clone()),
        })
        .collect();

    let series = series
        .iter()
        .map(|p| SeriesPoint {
            taken_at: p.taken_at.clone(),
            score: p.score,
            values: METRICS
                .iter()
                .chain(std::iter::once(&TEMPLATE_DEVIATIONS))
                .map(|d| (d.key.to_string(), (d.get)(&p.metrics)))
                .collect(),
        })
        .collect();

    HistorySection {
        score,
        parts: part_scores(&now.root.score),
        deductions: now.root.score.top_deductions(TOP_DEDUCTIONS).to_vec(),
        comparison,
        metrics,
        folders,
        series,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::score::{score, Weights, METRICS_VERSION};
    use crate::health::Area;
    use crate::history::SnapshotMeta;

    fn area(folder: &str, junk: u64, dup: u64) -> Area {
        let metrics = Metrics {
            files: 1000,
            size: 1_000_000,
            local_bytes: 1_000_000,
            junk_files: junk,
            dup_wasted: dup,
            ..Metrics::default()
        };
        Area {
            folder: folder.to_string(),
            score: score(&metrics, &Weights::default()),
            metrics,
        }
    }

    fn snap(junk: u64, dup: u64) -> Snapshot {
        Snapshot {
            root: area("", junk, dup),
            folders: vec![area("Docs", junk, dup), area("Neu", 0, 0)],
        }
    }

    fn stored(snapshot: Snapshot) -> Stored {
        Stored {
            id: 1,
            meta: SnapshotMeta {
                root_key: "d:\\".into(),
                root_path: "D:\\".into(),
                taken_at: "2026-09-27T12:00:00Z".into(),
                scan_finished_at: None,
                tool_version: "0".into(),
                metrics_version: METRICS_VERSION,
                config_fp: "fp".into(),
                template: None,
                profile: None,
                scan_errors: 0,
            },
            snapshot,
        }
    }

    #[test]
    fn erster_lauf_hat_keinen_vergleich() {
        let s = section(&snap(0, 0), None, &[], vec![]);
        assert_eq!(s.score, 100);
        assert!(s.comparison.is_none());
        assert!(s.metrics.iter().all(|m| m.previous.is_none()));
        assert_eq!(
            score_line(s.score, s.comparison.as_ref()),
            "Health-Score 100 (erster Lauf)"
        );
    }

    #[test]
    fn zweiter_lauf_zeigt_veraenderung() {
        let prev = stored(snap(0, 0));
        let now = snap(25, 0);
        let s = section(&now, Some((&prev, Comparable::Full)), &[], vec![]);
        let c = s.comparison.as_ref().unwrap();
        assert!(c.delta < 0);
        assert_eq!(c.kind, "full");
        assert_eq!(c.previous_score, 100);
        let junk = s.metrics.iter().find(|m| m.key == "junk_files").unwrap();
        assert_eq!((junk.now, junk.previous), (25, Some(0)));
        let line = score_line(s.score, s.comparison.as_ref());
        assert!(line.contains(&format!("−{}", -c.delta)), "{line}");
    }

    #[test]
    fn eingeschraenkter_vergleich_ist_markiert() {
        let prev = stored(snap(0, 0));
        let s = section(&snap(0, 0), Some((&prev, Comparable::Limited)), &[], vec![]);
        let c = s.comparison.unwrap();
        assert!(c.is_limited());
        assert!(score_line(100, Some(&c)).ends_with(", eingeschränkt vergleichbar)"));
    }

    #[test]
    fn nur_die_drei_groessten_abzuege() {
        let metrics = Metrics {
            files: 1000,
            dirs: 100,
            size: 1_000_000,
            local_bytes: 1_000_000,
            junk_files: 50,
            dup_wasted: 250_000,
            problem_files: 50,
            structure_issues: 30,
            version_excess: 50,
            ..Metrics::default()
        };
        let area = Area {
            folder: String::new(),
            score: score(&metrics, &Weights::default()),
            metrics,
        };
        let now = Snapshot {
            root: area,
            folders: vec![],
        };
        let s = section(&now, None, &[], vec![]);
        assert_eq!(s.deductions.len(), TOP_DEDUCTIONS);
        assert_eq!(s.deductions[0].part, "duplicates");
        assert_eq!(s.parts.len(), 6);
        assert_eq!(s.parts[5].value, None, "ohne Vorlage keine Strukturtreue");
    }

    #[test]
    fn ordner_zeilen_kennen_den_vorigen_score_nur_wenn_es_ihn_gab() {
        let prev = stored(snap(0, 0));
        let s = section(&snap(25, 0), Some((&prev, Comparable::Full)), &[], vec![]);
        let docs = s.folders.iter().find(|f| f.folder == "Docs").unwrap();
        assert_eq!(docs.previous_score, Some(100));
        assert!(docs.score < 100);
        assert!(docs.top_deduction.as_deref().unwrap().contains("Müll"));
        let neu = s.folders.iter().find(|f| f.folder == "Neu").unwrap();
        assert_eq!(neu.previous_score, Some(100));
        let prev_without = stored(Snapshot {
            root: area("", 0, 0),
            folders: vec![],
        });
        let s = section(
            &snap(0, 0),
            Some((&prev_without, Comparable::Full)),
            &[],
            vec![],
        );
        assert!(s.folders.iter().all(|f| f.previous_score.is_none()));
    }

    #[test]
    fn reihen_enthalten_alle_kennzahlen_je_punkt() {
        let points: Vec<Point> = (0..3)
            .map(|i| Point {
                id: i,
                taken_at: format!("2026-10-0{}T10:00:00Z", i + 1),
                config_fp: "fp".into(),
                metrics_version: METRICS_VERSION,
                score: 90 - i as u8,
                metrics: Metrics {
                    junk_files: i as u64 * 10,
                    ..Metrics::default()
                },
            })
            .collect();
        let s = section(&snap(0, 0), None, &points, vec!["Hinweis".into()]);
        assert_eq!(s.series.len(), 3);
        assert_eq!(s.series[2].values["junk_files"], 20);
        assert_eq!(s.series[2].score, 88);
        assert!(s.series[0].values.contains_key("dup_wasted"));
        assert_eq!(s.notes, ["Hinweis"]);
    }

    #[test]
    fn alte_daten_sind_nur_eine_kennzahl() {
        let mut now = snap(0, 0);
        now.root.metrics.old_bytes = 500;
        let s = section(&now, None, &[], vec![]);
        assert_eq!(
            s.metrics.iter().find(|m| m.key == "old_bytes").unwrap().now,
            500
        );
        assert_eq!(s.score, 100);
    }
}
