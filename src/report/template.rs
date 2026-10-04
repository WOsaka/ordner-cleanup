//! Abschnitt „Soll/Ist“ des Berichts: Abweichungen von der Vorlage.

use serde::Serialize;

use crate::template::check::{CheckResult, DeviationKind};

/// Höchstens so viele Abweichungen je Art stehen im Bericht; die Anzahl nennt alle.
pub const MAX_PER_KIND: usize = 100;

#[derive(Debug, Clone, Serialize)]
pub struct KindCount {
    pub key: String,
    pub label: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviationItem {
    pub kind: String,
    pub label: String,
    pub path: String,
    pub level: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateSection {
    pub name: String,
    /// Eingebauter Name oder Pfad der Vorlage
    pub source: String,
    /// Geprüfte Einträge
    pub checked: u64,
    /// Abweichungen insgesamt (auch die über der Anzeigegrenze)
    pub total: u64,
    /// Anteil der Abweichungen an den geprüften Einträgen (0.0 bis 1.0)
    pub share: f64,
    pub counts: Vec<KindCount>,
    pub items: Vec<DeviationItem>,
}

pub fn section(name: &str, source: &str, result: &CheckResult) -> TemplateSection {
    let mut counts = Vec::new();
    let mut items = Vec::new();
    for kind in DeviationKind::ALL {
        let of_kind: Vec<_> = result
            .deviations
            .iter()
            .filter(|d| d.kind == kind)
            .collect();
        counts.push(KindCount {
            key: kind.key().to_string(),
            label: kind.label().to_string(),
            count: of_kind.len() as u64,
        });
        items.extend(
            of_kind
                .into_iter()
                .take(MAX_PER_KIND)
                .map(|d| DeviationItem {
                    kind: kind.key().to_string(),
                    label: kind.label().to_string(),
                    path: d.path.clone(),
                    level: d.level,
                }),
        );
    }
    let total = result.deviations.len() as u64;
    TemplateSection {
        name: name.to_string(),
        source: source.to_string(),
        checked: result.checked,
        total,
        share: if result.checked == 0 {
            0.0
        } else {
            total as f64 / result.checked as f64
        },
        counts,
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::check::Deviation;

    fn result(n_unexpected: usize, n_missing: usize, checked: u64) -> CheckResult {
        let mut deviations = Vec::new();
        for i in 0..n_missing {
            deviations.push(Deviation {
                kind: DeviationKind::MissingRequired,
                path: format!(r"D:\fehlt{i}"),
                level: 1,
            });
        }
        for i in 0..n_unexpected {
            deviations.push(Deviation {
                kind: DeviationKind::UnexpectedDir,
                path: format!(r"D:\extra{i}"),
                level: 1,
            });
        }
        CheckResult {
            deviations,
            checked,
            ..CheckResult::default()
        }
    }

    #[test]
    fn zaehlt_je_art_und_nennt_den_anteil() {
        let s = section("PARA", "para", &result(2, 1, 10));
        assert_eq!(s.total, 3);
        assert!((s.share - 0.3).abs() < 1e-9);
        assert_eq!(s.counts.len(), 5);
        let count = |k: &str| s.counts.iter().find(|c| c.key == k).unwrap().count;
        assert_eq!(
            (
                count("missing_required"),
                count("unexpected_dir"),
                count("bad_name")
            ),
            (1, 2, 0)
        );
        assert_eq!(s.items.len(), 3);
    }

    #[test]
    fn begrenzt_die_liste_je_art_aber_nicht_die_anzahl() {
        let s = section("X", "x", &result(MAX_PER_KIND + 50, 3, 1000));
        assert_eq!(s.total as usize, MAX_PER_KIND + 53);
        let unexpected = s
            .items
            .iter()
            .filter(|i| i.kind == "unexpected_dir")
            .count();
        assert_eq!(unexpected, MAX_PER_KIND);
        assert_eq!(
            s.items
                .iter()
                .filter(|i| i.kind == "missing_required")
                .count(),
            3
        );
        assert_eq!(
            s.counts
                .iter()
                .find(|c| c.key == "unexpected_dir")
                .unwrap()
                .count as usize,
            MAX_PER_KIND + 50
        );
    }

    #[test]
    fn nichts_geprueft_ergibt_anteil_null() {
        assert_eq!(section("X", "x", &result(0, 0, 0)).share, 0.0);
    }
}
