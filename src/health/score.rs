//! Teilwerte und Gesamt-Score (0–100) aus den Kennzahlen eines Bereichs.
//!
//! Jeder Teilwert fällt linear von 100 (nichts gefunden) auf 0 (Schwelle „schlecht“ erreicht).
//! Der Gesamtwert ist der gewichtete Durchschnitt der anwendbaren Teilwerte. Die Schwellen sind
//! Konstanten; ändern sie sich, steigt [`METRICS_VERSION`], damit alte Verläufe nicht mit neuen
//! verglichen werden.

use bytesize::ByteSize;
use serde::{Deserialize, Serialize};

use super::Metrics;

/// Version der Formeln und Schwellen unten.
pub const METRICS_VERSION: i64 = 1;

/// Müll: 0 Punkte ab so vielen Müll-Dateien je 1.000 Dateien …
pub const JUNK_FILES_PER_1000_BAD: f64 = 50.0;
/// … bzw. diesem Anteil Müll-Bytes an der Größe.
pub const JUNK_BYTES_SHARE_BAD: f64 = 0.10;
/// Duplikate: verschwendete Bytes im Verhältnis zu den lokalen Bytes.
pub const DUP_SHARE_BAD: f64 = 0.25;
/// Problemdateien je 1.000 Dateien.
pub const PROBLEMS_PER_1000_BAD: f64 = 50.0;
/// Struktur-Befunde je 100 Ordner.
pub const STRUCTURE_PER_100_DIRS_BAD: f64 = 30.0;
/// Überzählige Versionen je 1.000 Dateien.
pub const VERSIONS_PER_1000_BAD: f64 = 50.0;
/// Anteil abweichender Einträge an den gegen die Vorlage geprüften.
pub const TEMPLATE_SHARE_BAD: f64 = 0.30;

/// Gewichte der Teilwerte (`[health.weights]` in der Config); 0 schaltet einen Teilwert ab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Weights {
    pub junk: u32,
    pub duplicates: u32,
    pub problems: u32,
    pub structure: u32,
    pub versions: u32,
    pub template: u32,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            junk: 20,
            duplicates: 25,
            problems: 20,
            structure: 20,
            versions: 15,
            template: 20,
        }
    }
}

/// Die Teilwerte in fester Reihenfolge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Junk,
    Duplicates,
    Problems,
    Structure,
    Versions,
    Template,
}

impl Part {
    pub const ALL: [Part; 6] = [
        Part::Junk,
        Part::Duplicates,
        Part::Problems,
        Part::Structure,
        Part::Versions,
        Part::Template,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Part::Junk => "junk",
            Part::Duplicates => "duplicates",
            Part::Problems => "problems",
            Part::Structure => "structure",
            Part::Versions => "versions",
            Part::Template => "template",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Part::Junk => "Müll",
            Part::Duplicates => "Duplikate",
            Part::Problems => "Problemdateien",
            Part::Structure => "Struktur",
            Part::Versions => "Versionen",
            Part::Template => "Strukturtreue",
        }
    }

    fn weight(self, w: &Weights) -> u32 {
        match self {
            Part::Junk => w.junk,
            Part::Duplicates => w.duplicates,
            Part::Problems => w.problems,
            Part::Structure => w.structure,
            Part::Versions => w.versions,
            Part::Template => w.template,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deduction {
    pub part: String,
    pub points: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub total: u8,
    /// Teilwerte 0–100 in der Reihenfolge von [`Part::ALL`]; `None` = nicht anwendbar/abgeschaltet.
    pub parts: Vec<(String, Option<f64>)>,
    /// Abzüge, größte zuerst; nur Teile mit mindestens einem halben Punkt.
    pub deductions: Vec<Deduction>,
}

impl Score {
    pub fn top_deductions(&self, n: usize) -> &[Deduction] {
        &self.deductions[..n.min(self.deductions.len())]
    }
}

/// 100 bei `bad_fraction == 0`, 0 ab 1; dazwischen linear.
fn linear(bad_fraction: f64) -> f64 {
    100.0 * (1.0 - bad_fraction.clamp(0.0, 1.0))
}

fn ratio(n: u64, d: u64) -> f64 {
    if d == 0 {
        0.0
    } else {
        n as f64 / d as f64
    }
}

fn percent(share: f64) -> String {
    format!("{:.0} %", share * 100.0)
}

/// Teilwert und Erklärung des Abzugs; `None`, wenn der Teil nicht anwendbar ist.
fn part_value(part: Part, m: &Metrics) -> Option<(f64, String)> {
    if m.files == 0 {
        // Eine leere Wurzel ist makellos; „Strukturtreue“ gibt es nur mit Vorlage.
        let applicable = part != Part::Template || m.template_checked.is_some();
        return applicable.then(|| (100.0, String::new()));
    }
    match part {
        Part::Junk => {
            let per_1000 = ratio(m.junk_files, m.files) * 1000.0 / JUNK_FILES_PER_1000_BAD;
            let by_bytes = ratio(m.junk_bytes, m.size) / JUNK_BYTES_SHARE_BAD;
            let text = format!(
                "{} Müll-Dateien ({}, {} der Daten)",
                m.junk_files,
                ByteSize::b(m.junk_bytes),
                percent(ratio(m.junk_bytes, m.size))
            );
            Some((linear(per_1000.max(by_bytes)), text))
        }
        Part::Duplicates => {
            let share = ratio(m.dup_wasted, m.local_bytes);
            let text = format!(
                "{} Duplikate ({} der Daten)",
                ByteSize::b(m.dup_wasted),
                percent(share)
            );
            Some((linear(share / DUP_SHARE_BAD), text))
        }
        Part::Problems => {
            let per_1000 = ratio(m.problem_files, m.files) * 1000.0;
            let text = format!("{} Dateien mit Problemen", m.problem_files);
            Some((linear(per_1000 / PROBLEMS_PER_1000_BAD), text))
        }
        Part::Structure => {
            let per_100 = ratio(m.structure_issues, m.dirs) * 100.0;
            let text = format!(
                "{} Strukturbefunde in {} Ordnern",
                m.structure_issues, m.dirs
            );
            Some((linear(per_100 / STRUCTURE_PER_100_DIRS_BAD), text))
        }
        Part::Versions => {
            let per_1000 = ratio(m.version_excess, m.files) * 1000.0;
            let text = format!("{} überzählige Versionen", m.version_excess);
            Some((linear(per_1000 / VERSIONS_PER_1000_BAD), text))
        }
        Part::Template => {
            let checked = m.template_checked?;
            let deviations = m.template_deviations.unwrap_or(0);
            let share = ratio(deviations, checked);
            let text = format!("{deviations} von {checked} Einträgen weichen von der Vorlage ab");
            Some((linear(share / TEMPLATE_SHARE_BAD), text))
        }
    }
}

/// Berechnet Teilwerte, Gesamtwert und erklärte Abzüge. Deterministisch und monoton.
pub fn score(m: &Metrics, weights: &Weights) -> Score {
    let mut parts = Vec::new();
    let mut applicable: Vec<(Part, f64, String, u32)> = Vec::new();
    for part in Part::ALL {
        let weight = part.weight(weights);
        let value = if weight == 0 {
            None
        } else {
            part_value(part, m)
        };
        parts.push((part.key().to_string(), value.as_ref().map(|v| v.0)));
        if let Some((v, text)) = value {
            applicable.push((part, v, text, weight));
        }
    }
    let weight_sum: u32 = applicable.iter().map(|a| a.3).sum();
    if weight_sum == 0 {
        return Score {
            total: 100,
            parts,
            deductions: Vec::new(),
        };
    }
    let weight_sum = f64::from(weight_sum);
    let total = applicable
        .iter()
        .map(|(_, v, _, w)| v * f64::from(*w))
        .sum::<f64>()
        / weight_sum;
    let mut deductions: Vec<Deduction> = applicable
        .iter()
        .map(|(part, v, text, w)| Deduction {
            part: part.key().to_string(),
            points: f64::from(*w) / weight_sum * (100.0 - v),
            text: text.clone(),
        })
        .filter(|d| d.points >= 0.5)
        .collect();
    deductions.sort_by(|a, b| b.points.total_cmp(&a.points));
    for d in &mut deductions {
        d.points = (d.points * 10.0).round() / 10.0;
    }
    Score {
        total: total.round().clamp(0.0, 100.0) as u8,
        parts,
        deductions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Metrics {
        Metrics {
            files: 1000,
            dirs: 100,
            size: 1_000_000,
            local_bytes: 1_000_000,
            ..Metrics::default()
        }
    }

    fn part(s: &Score, p: Part) -> Option<f64> {
        s.parts.iter().find(|(k, _)| k == p.key()).and_then(|x| x.1)
    }

    #[test]
    fn leere_wurzel_ergibt_100() {
        let s = score(&Metrics::default(), &Weights::default());
        assert_eq!(s.total, 100);
        assert!(s.deductions.is_empty());
        assert_eq!(part(&s, Part::Junk), Some(100.0));
        assert_eq!(part(&s, Part::Template), None);
    }

    #[test]
    fn makellose_wurzel_ergibt_100_ohne_abzuege() {
        let s = score(&base(), &Weights::default());
        assert_eq!(s.total, 100);
        assert!(s.deductions.is_empty());
    }

    #[test]
    fn jede_kurve_an_null_schwelle_und_mitte() {
        type Setter = Box<dyn Fn(&mut Metrics, f64)>;
        let cases: Vec<(Part, Setter)> = vec![
            (
                Part::Junk,
                Box::new(|m, f| m.junk_files = (f * 50.0) as u64), // 50 je 1000 bei 1000 Dateien
            ),
            (
                Part::Duplicates,
                Box::new(|m, f| m.dup_wasted = (f * 250_000.0) as u64),
            ),
            (
                Part::Problems,
                Box::new(|m, f| m.problem_files = (f * 50.0) as u64),
            ),
            (
                Part::Structure,
                Box::new(|m, f| m.structure_issues = (f * 30.0) as u64), // 30 je 100 bei 100 Ordnern
            ),
            (
                Part::Versions,
                Box::new(|m, f| m.version_excess = (f * 50.0) as u64),
            ),
            (
                Part::Template,
                Box::new(|m, f| {
                    m.template_checked = Some(100);
                    m.template_deviations = Some((f * 30.0) as u64);
                }),
            ),
        ];
        for (p, set) in cases {
            let mut zero = base();
            set(&mut zero, 0.0);
            assert_eq!(part(&score(&zero, &Weights::default()), p), Some(100.0));
            let mut half = base();
            set(&mut half, 0.5);
            let v = part(&score(&half, &Weights::default()), p).unwrap();
            assert!((v - 50.0).abs() < 1.0, "{p:?}: {v}");
            let mut bad = base();
            set(&mut bad, 1.0);
            assert_eq!(
                part(&score(&bad, &Weights::default()), p),
                Some(0.0),
                "{p:?}"
            );
            let mut worse = base();
            set(&mut worse, 3.0);
            assert_eq!(
                part(&score(&worse, &Weights::default()), p),
                Some(0.0),
                "{p:?}"
            );
        }
    }

    #[test]
    fn muell_bytes_koennen_allein_den_teilwert_senken() {
        let mut m = base();
        m.junk_files = 1;
        m.junk_bytes = 100_000; // 10 % der Größe
        assert_eq!(part(&score(&m, &Weights::default()), Part::Junk), Some(0.0));
    }

    #[test]
    fn gewicht_null_schaltet_den_teil_ab() {
        let mut m = base();
        m.junk_files = 50;
        let w = Weights {
            junk: 0,
            ..Weights::default()
        };
        let s = score(&m, &w);
        assert_eq!(part(&s, Part::Junk), None);
        assert_eq!(s.total, 100);
        assert!(s.deductions.is_empty());
    }

    #[test]
    fn nur_nullgewichte_ergeben_100() {
        let w = Weights {
            junk: 0,
            duplicates: 0,
            problems: 0,
            structure: 0,
            versions: 0,
            template: 0,
        };
        let mut m = base();
        m.junk_files = 500;
        assert_eq!(score(&m, &w).total, 100);
    }

    #[test]
    fn ohne_vorlage_faellt_strukturtreue_aus_der_gewichtung() {
        let mut m = base();
        m.junk_files = 50; // Müll 0 Punkte
                           // Ohne Vorlage: 20 von 100 Gewicht auf 0 -> 80
        assert_eq!(score(&m, &Weights::default()).total, 80);
        // Mit makelloser Vorlage zählt Strukturtreue (100) mit Gewicht 20: 80/120 Gewicht fehlen
        m.template_checked = Some(10);
        m.template_deviations = Some(0);
        assert_eq!(score(&m, &Weights::default()).total, 83);
    }

    #[test]
    fn abzuege_sind_sortiert_und_erklaert() {
        let mut m = base();
        m.dup_wasted = 250_000; // Duplikate 0
        m.junk_files = 25; // Müll 50
        let s = score(&m, &Weights::default());
        assert_eq!(s.deductions[0].part, "duplicates");
        assert!(s.deductions[0].text.contains("25 %"), "{:?}", s.deductions);
        assert_eq!(s.deductions[1].part, "junk");
        assert!(s.deductions[1].points < s.deductions[0].points);
        assert_eq!(s.top_deductions(1).len(), 1);
        assert_eq!(s.top_deductions(9).len(), 2);
    }

    #[test]
    fn alte_daten_senken_den_score_nicht() {
        let mut m = base();
        m.old_bytes = 1_000_000;
        assert_eq!(score(&m, &Weights::default()).total, 100);
    }

    #[test]
    fn deterministisch() {
        let mut m = base();
        m.junk_files = 17;
        m.dup_wasted = 12_345;
        m.problem_files = 3;
        assert_eq!(
            score(&m, &Weights::default()),
            score(&m, &Weights::default())
        );
    }

    #[test]
    fn monoton_mehr_schlechtes_senkt_nie_den_score() {
        type Set = fn(&mut Metrics, u64);
        let setters: [Set; 6] = [
            |m, n| m.junk_files = n,
            |m, n| m.dup_wasted = n * 1000,
            |m, n| m.problem_files = n,
            |m, n| m.structure_issues = n,
            |m, n| m.version_excess = n,
            |m, n| {
                m.template_checked = Some(100);
                m.template_deviations = Some(n.min(100));
            },
        ];
        for set in setters {
            let mut last = 100u8;
            for n in 0..400 {
                let mut m = base();
                set(&mut m, n);
                let total = score(&m, &Weights::default()).total;
                assert!(total <= last, "n={n}: {total} > {last}");
                last = total;
            }
        }
    }
}
