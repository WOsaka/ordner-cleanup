//! Karten der Aufräumen-Seite: die sechs Plan-Arten mit einem Satz Erklärung und, wo der letzte
//! Scan eine Zahl kennt, mit dieser Zahl. Die Zahlen kommen aus dem Report; es gibt keine neue
//! Analyse.

use std::path::Path;

use chrono::{DateTime, Local};

use crate::config::Config;
use crate::gui::format::DurationUnit;
use crate::gui::texts;
use crate::report::history::{MetricRow, Unit};
use crate::report::Report;
use crate::rules::{KnownCategories, RuleSet};

/// Eine Plan-Art als Karte: Schlüssel für `PlanKindRequest`, Titel und ein Satz Erklärung.
pub struct Card {
    pub key: &'static str,
    pub title: &'static str,
    pub text: &'static str,
}

pub const CARDS: [Card; 6] = [
    Card {
        key: "dedupe",
        title: "Duplikate",
        text: "Identische Dateien finden; eine Kopie bleibt, die übrigen gehen in die Quarantäne.",
    },
    Card {
        key: "junk",
        title: "Müll",
        text: "Temporäre Dateien, Installer und Systemreste in die Quarantäne verschieben.",
    },
    Card {
        key: "empty-dirs",
        title: "Leere Ordner",
        text: "Ordner ohne Inhalt entfernen.",
    },
    Card {
        key: "archive",
        title: "Archivieren",
        text: "Alte, lange unberührte Ordner als ZIP ablegen.",
    },
    Card {
        key: "versions",
        title: "Versionen",
        text: "Überzählige alte Dateiversionen („Datei (1)“, „Kopie von …“) aufräumen.",
    },
    Card {
        key: "rules",
        title: "Nach Regeln",
        text: "Dateien nach deiner Regeldatei in eine neue Ordnerstruktur sortieren.",
    },
];

pub const CARD_MIN_WIDTH: f32 = 200.0;
pub const CARD_MAX_WIDTH: f32 = 300.0;

/// Spaltenzahl und Innenbreite der Karten für den verfügbaren Platz. `chrome` ist Innenrand plus
/// Strich einer Karte, `spacing` der Abstand zwischen den Karten. Die Zeile füllt den Platz
/// gleichmäßig und läuft nie über, außer eine einzelne Karte hat schon die Mindestbreite nicht.
pub fn card_layout(avail_width: f32, chrome: f32, spacing: f32) -> (usize, f32) {
    let columns = ((avail_width + spacing) / (CARD_MIN_WIDTH + chrome + spacing))
        .floor()
        .max(1.0);
    let width = ((avail_width + spacing) / columns - spacing - chrome)
        .clamp(CARD_MIN_WIDTH, CARD_MAX_WIDTH);
    (columns as usize, width)
}

/// Zahlen des letzten Scans für die Karten. `None` = dafür liegt keine Zahl vor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardFacts {
    /// Duplikat-Gruppen und die Bytes, die frei würden
    pub dedupe: Option<(u64, u64)>,
    /// Müll-Dateien und -Bytes (aus dem Verlauf)
    pub junk: Option<(u64, u64)>,
    pub empty_dirs: Option<u64>,
    pub scanned_at: Option<String>,
}

fn metric(rows: &[MetricRow], key: &str, unit: Unit) -> Option<u64> {
    rows.iter()
        .find(|r| r.key == key && r.unit == unit)
        .map(|r| r.now)
}

impl CardFacts {
    /// Müll und leere Ordner stehen nur im Verlauf (`metrics`). Fehlt er, bleibt der Müll ohne
    /// Zahl; leere Ordner fallen auf die Strukturbefunde zurück (`empty_structure`).
    pub fn new(
        duplicates: (usize, u64),
        metrics: Option<&[MetricRow]>,
        empty_structure: u64,
        scanned_at: Option<&str>,
    ) -> Self {
        let junk = metrics.and_then(|m| {
            Some((
                metric(m, "junk_files", Unit::Count)?,
                metric(m, "junk_bytes", Unit::Bytes)?,
            ))
        });
        let empty_dirs = metrics
            .and_then(|m| metric(m, "empty_dirs", Unit::Count))
            .or(Some(empty_structure));
        Self {
            dedupe: Some((duplicates.0 as u64, duplicates.1)),
            junk,
            empty_dirs,
            scanned_at: scanned_at.map(String::from),
        }
    }

    pub fn from_report(report: &Report) -> Self {
        let empty = report
            .structure
            .iter()
            .filter(|s| s.issue == "empty")
            .count() as u64;
        Self::new(
            (
                report.duplicates.group_count,
                report.duplicates.total_wasted,
            ),
            report.history.as_ref().map(|h| h.metrics.as_slice()),
            empty,
            report.meta.scanned_at.as_deref(),
        )
    }

    /// Die Zahl einer Karte als Satz; `None` bei Karten ohne Zahl (Archivieren, Versionen, Regeln)
    /// und wenn der Report keine liefert.
    pub fn number_text(&self, key: &str) -> Option<String> {
        let plural = |n: u64, one: &str, many: &str| {
            format!("{} {}", texts::grouped(n), if n == 1 { one } else { many })
        };
        match key {
            "dedupe" => self.dedupe.map(|(groups, bytes)| {
                format!(
                    "{}, {} könnten frei werden",
                    plural(groups, "Gruppe", "Gruppen"),
                    texts::bytes(bytes)
                )
            }),
            "junk" => self.junk.map(|(files, bytes)| {
                format!(
                    "{}, {}",
                    plural(files, "Datei", "Dateien"),
                    texts::bytes(bytes)
                )
            }),
            "empty-dirs" => self.empty_dirs.map(|n| plural(n, "Ordner", "Ordner")),
            _ => None,
        }
    }

    pub fn scanned(&self) -> Option<DateTime<Local>> {
        let text = self.scanned_at.as_deref()?;
        DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|t| t.with_timezone(&Local))
    }
}

/// Eingabe „Zahl plus Einheit“ für Dauer-Felder. Leer = Wert aus der Config.
#[derive(Debug, Clone)]
pub struct DurationField {
    pub amount: String,
    pub unit: DurationUnit,
}

impl DurationField {
    pub fn new(unit: DurationUnit) -> Self {
        Self {
            amount: String::new(),
            unit,
        }
    }

    /// Argument für die Plan-Anfrage (`2y`), `None` = aus der Config.
    pub fn arg(&self) -> Result<Option<String>, String> {
        crate::gui::format::duration_to_arg(&self.amount, self.unit)
    }
}

/// Platzhalter aus dem Config-Wert: `2y` → „Standard: 2 Jahre“.
pub fn duration_placeholder(config_value: &str) -> String {
    let value = config_value.trim();
    let split = value.len().saturating_sub(1);
    let (number, unit) = value.split_at(split);
    let word = match (number.parse::<u64>(), unit.to_ascii_lowercase().as_str()) {
        (Ok(1), "d") => Some("Tag"),
        (Ok(_), "d") => Some("Tage"),
        (Ok(1), "m") => Some("Monat"),
        (Ok(_), "m") => Some("Monate"),
        (Ok(1), "y") => Some("Jahr"),
        (Ok(_), "y") => Some("Jahre"),
        _ => None,
    };
    match word {
        Some(word) => format!("Standard: {number} {word}"),
        None => format!("Standard: {value}"),
    }
}

/// Namen der angehakten Regeln für `only`. Sind alle angehakt, bleibt die Liste leer (= alle).
pub fn only_from_checks(rules: &[(String, bool)]) -> Result<Vec<String>, String> {
    let checked: Vec<String> = rules
        .iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| name.clone())
        .collect();
    if rules.is_empty() || checked.len() == rules.len() {
        return Ok(Vec::new());
    }
    if checked.is_empty() {
        return Err("Mindestens eine Regel muss angehakt sein.".into());
    }
    Ok(checked)
}

/// Regelnamen einer Regeldatei; Fehler mit Datei und Zeile als Text.
pub fn rule_names(file: &Path, config: &Config) -> Result<Vec<String>, String> {
    let known = match crate::ops::classify::load_defs(config) {
        Ok(defs) => KnownCategories::Names(defs.names()),
        Err(e) => KnownCategories::Unavailable(format!("{e:#}")),
    };
    let set = RuleSet::load(file, &known).map_err(|e| e.to_string())?;
    Ok(set.rules.into_iter().map(|r| r.name).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPACING: f32 = 8.0;
    const CHROME: f32 = 14.0;

    #[test]
    fn schmal_ergibt_eine_spalte_mit_mindestbreite() {
        let (columns, width) = card_layout(150.0, CHROME, SPACING);
        assert_eq!(columns, 1);
        assert_eq!(width, CARD_MIN_WIDTH);
    }

    #[test]
    fn mittel_ergibt_zwei_spalten_die_in_die_zeile_passen() {
        let (columns, width) = card_layout(560.0, CHROME, SPACING);
        assert_eq!(columns, 2);
        assert!(columns as f32 * (width + CHROME) + (columns - 1) as f32 * SPACING <= 560.0);
    }

    #[test]
    fn breit_ergibt_mehrere_spalten_und_hoechstens_maximalbreite() {
        let (columns, width) = card_layout(1400.0, CHROME, SPACING);
        assert!(columns >= 3, "{columns}");
        assert!(width <= CARD_MAX_WIDTH);
    }

    #[test]
    fn zeile_passt_fuer_jede_breite_in_den_platz() {
        let mut avail = 100.0;
        while avail <= 2000.0 {
            let (columns, width) = card_layout(avail, CHROME, SPACING);
            let row = columns as f32 * (width + CHROME) + (columns - 1) as f32 * SPACING;
            let one_column_too_narrow = columns == 1 && avail < CARD_MIN_WIDTH + CHROME;
            assert!(
                row <= avail || one_column_too_narrow,
                "avail {avail}: {row}"
            );
            assert!((CARD_MIN_WIDTH..=CARD_MAX_WIDTH).contains(&width));
            avail += 7.0;
        }
    }

    fn row(key: &str, unit: Unit, now: u64) -> MetricRow {
        MetricRow {
            key: key.into(),
            label: key.into(),
            unit,
            now,
            previous: None,
        }
    }

    fn metrics() -> Vec<MetricRow> {
        vec![
            row("junk_files", Unit::Count, 1204),
            row("junk_bytes", Unit::Bytes, 860_000_000),
            row("empty_dirs", Unit::Count, 58),
        ]
    }

    #[test]
    fn zahlen_kommen_aus_report_und_verlauf() {
        let m = metrics();
        let f = CardFacts::new((37, 4_200_000_000), Some(&m), 3, None);
        assert_eq!(f.dedupe, Some((37, 4_200_000_000)));
        assert_eq!(f.junk, Some((1204, 860_000_000)));
        // Der Verlauf gewinnt gegen die Strukturbefunde.
        assert_eq!(f.empty_dirs, Some(58));
    }

    #[test]
    fn ohne_verlauf_gibt_es_keine_muellzahl_aber_leere_ordner_aus_der_struktur() {
        let f = CardFacts::new((0, 0), None, 5, None);
        assert_eq!(f.junk, None);
        assert_eq!(f.empty_dirs, Some(5));
        assert_eq!(f.dedupe, Some((0, 0)));
    }

    #[test]
    fn verlauf_ohne_die_kennzahl_laesst_die_zahl_weg() {
        let m = vec![row("junk_files", Unit::Count, 4)];
        let f = CardFacts::new((1, 2), Some(&m), 0, None);
        // Bytes fehlen: lieber keine Zahl als eine halbe.
        assert_eq!(f.junk, None);
        assert_eq!(f.empty_dirs, Some(0));
    }

    #[test]
    fn scan_zeitpunkt_wird_gelesen_und_kaputtes_ignoriert() {
        let f = CardFacts::new((0, 0), None, 0, Some("2026-10-01T10:00:00+00:00"));
        assert!(f.scanned().is_some());
        let bad = CardFacts::new((0, 0), None, 0, Some("gestern"));
        assert!(bad.scanned().is_none());
        assert!(CardFacts::default().scanned().is_none());
    }

    #[test]
    fn kartensaetze_nennen_die_zahlen() {
        let m = metrics();
        let f = CardFacts::new((37, 4_200_000_000), Some(&m), 0, None);
        let dup = f.number_text("dedupe").unwrap();
        assert!(
            dup.contains("37 Gruppen") && dup.contains("könnten frei werden"),
            "{dup}"
        );
        assert!(f.number_text("junk").unwrap().contains("1.204 Dateien"));
        assert_eq!(f.number_text("empty-dirs").unwrap(), "58 Ordner");
    }

    #[test]
    fn einzahl_und_karten_ohne_zahl() {
        let m = vec![
            row("junk_files", Unit::Count, 1),
            row("junk_bytes", Unit::Bytes, 10),
            row("empty_dirs", Unit::Count, 1),
        ];
        let f = CardFacts::new((1, 5), Some(&m), 0, None);
        assert!(f.number_text("dedupe").unwrap().starts_with("1 Gruppe,"));
        assert!(f.number_text("junk").unwrap().starts_with("1 Datei,"));
        assert_eq!(f.number_text("empty-dirs").unwrap(), "1 Ordner");
        for key in ["archive", "versions", "rules", "unbekannt"] {
            assert_eq!(f.number_text(key), None, "{key}");
        }
        assert_eq!(CardFacts::default().number_text("junk"), None);
    }

    #[test]
    fn dauer_feld_wird_zum_cli_argument() {
        let mut f = DurationField::new(DurationUnit::Years);
        assert_eq!(f.arg(), Ok(None));
        f.amount = "2".into();
        assert_eq!(f.arg(), Ok(Some("2y".into())));
        f.unit = DurationUnit::Days;
        f.amount = "30".into();
        assert_eq!(f.arg(), Ok(Some("30d".into())));
        f.amount = "abc".into();
        assert!(f.arg().is_err());
    }

    #[test]
    fn platzhalter_zeigt_den_config_wert_lesbar() {
        assert_eq!(duration_placeholder("2y"), "Standard: 2 Jahre");
        assert_eq!(duration_placeholder("1y"), "Standard: 1 Jahr");
        assert_eq!(duration_placeholder("30d"), "Standard: 30 Tage");
        assert_eq!(duration_placeholder("18m"), "Standard: 18 Monate");
        assert_eq!(duration_placeholder("1m"), "Standard: 1 Monat");
        assert_eq!(duration_placeholder("wirr"), "Standard: wirr");
    }

    #[test]
    fn haekchen_ergeben_only() {
        let r = |a: bool, b: bool| vec![("fotos".to_string(), a), ("pdf".to_string(), b)];
        assert_eq!(only_from_checks(&r(true, true)), Ok(vec![]));
        assert_eq!(only_from_checks(&r(true, false)), Ok(vec!["fotos".into()]));
        assert!(only_from_checks(&r(false, false)).is_err());
        // Ohne geladene Regeln gibt es nichts einzuschränken.
        assert_eq!(only_from_checks(&[]), Ok(vec![]));
    }

    #[test]
    fn regelnamen_aus_datei_und_fehler_mit_zeile() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("rules.toml");
        std::fs::write(
            &ok,
            "[[rules]]
name = \"pdf\"
ext = [\"pdf\"]
target = \"Dokumente/\"

             [[rules]]
name = \"bilder\"
ext = [\"png\"]
target = \"Bilder/\"
",
        )
        .unwrap();
        let cfg = Config::default();
        assert_eq!(rule_names(&ok, &cfg).unwrap(), ["pdf", "bilder"]);

        let bad = dir.path().join("kaputt.toml");
        std::fs::write(
            &bad,
            "[[rules]]
name = 
",
        )
        .unwrap();
        let err = rule_names(&bad, &cfg).unwrap_err();
        assert!(err.contains("kaputt.toml"), "{err}");

        let missing = dir.path().join("fehlt.toml");
        assert!(rule_names(&missing, &cfg)
            .unwrap_err()
            .contains("fehlt.toml"));
    }
}
