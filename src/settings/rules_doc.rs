//! Regeln im Dokument der Regeldatei: Feldbeschreibungen für das Formular und die Platzhalter
//! für das Zielmuster. Auflisten, Hinzufügen, Duplizieren, Löschen und Verschieben laufen über
//! [`super::tables`] mit dem Schlüssel [`KEY`].

use super::fields::{FieldKind, FieldSpec};
use crate::config::Config;

/// Schlüssel der Regelliste in der Datei.
pub const KEY: &str = "rules";

const fn spec(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    kind: FieldKind,
) -> FieldSpec {
    FieldSpec {
        key,
        label,
        help,
        kind,
        default: |_| "–".into(),
    }
}

/// Bedingungen und Optionen einer Regel (ohne `name`, `fields` und `fields_regex`).
pub static RULE_FIELDS: &[FieldSpec] = &[
    spec(
        "glob",
        "Pfad-Glob",
        "z. B. Downloads/** (relativ zur Wurzel)",
        FieldKind::Text,
    ),
    spec(
        "ext",
        "Endungen",
        "Eine Endung je Zeile, ohne Punkt",
        FieldKind::TextList,
    ),
    spec(
        "name_regex",
        "Name passt auf (Regex)",
        "Gruppen lassen sich als {1}, {2} im Ziel verwenden",
        FieldKind::Text,
    ),
    spec("min_age", "Mindestalter", "z. B. 30d", FieldKind::Duration),
    spec("max_age", "Höchstalter", "z. B. 2y", FieldKind::Duration),
    spec("min_size", "Mindestgröße", "z. B. 10MB", FieldKind::Size),
    spec("max_size", "Höchstgröße", "z. B. 1GB", FieldKind::Size),
    spec(
        "category",
        "Kategorie(n)",
        "Inhaltsbedingung: eine Kategorie je Zeile",
        FieldKind::TextList,
    ),
    spec(
        "min_confidence",
        "Mindest-Konfidenz",
        "Nur mit Kategorie (0 bis 1)",
        FieldKind::Float { min: 0.0, max: 1.0 },
    ),
    spec(
        "category_source",
        "Kategorie-Quelle",
        "any = auch LLM, rules = nur Regeln",
        FieldKind::Choice(&["any", "rules"]),
    ),
    spec(
        "text_regex",
        "Text enthält (Regex)",
        "Auf dem Dokumenttext",
        FieldKind::Text,
    ),
    spec(
        "min_dwell",
        "Mindest-Verweildauer",
        "Seit der letzten Änderung, z. B. 7d",
        FieldKind::Duration,
    ),
    spec(
        "exif_fallback",
        "Ohne EXIF-Datum",
        "mtime = Änderungsdatum verwenden, skip = überspringen",
        FieldKind::Choice(&["mtime", "skip"]),
    ),
    spec(
        "iso_date_prefix",
        "Datum als ISO-Präfix",
        "Stellt dem Namen JJJJ-MM-TT voran",
        FieldKind::Bool,
    ),
    spec(
        "spaces",
        "Leerzeichen ersetzen durch",
        "_ oder -",
        FieldKind::Choice(&["_", "-"]),
    ),
    spec(
        "umlauts",
        "Umlaute ersetzen",
        "ä → ae usw.",
        FieldKind::Bool,
    ),
    spec(
        "strip_copy_suffix",
        "„ - Kopie“ entfernen",
        "Entfernt Kopie-Anhänge aus dem Namen",
        FieldKind::Bool,
    ),
    spec(
        "lowercase_ext",
        "Endung kleinschreiben",
        "JPG → jpg",
        FieldKind::Bool,
    ),
];

/// Platzhalter für das Zielmuster (`target`) mit Erklärung.
pub static PLACEHOLDERS: &[(&str, &str)] = &[
    ("{name}", "Dateiname ohne Endung"),
    ("{ext}", "Endung"),
    ("{parent}", "Name des übergeordneten Ordners"),
    ("{year}", "Änderungsjahr"),
    ("{month}", "Änderungsmonat"),
    ("{day}", "Änderungstag"),
    ("{exif.date:%Y}", "Aufnahmedatum (EXIF) mit Format"),
    ("{category}", "Kategorie aus dem Inhalt"),
    ("{doc.date:%Y-%m}", "Belegdatum mit Format"),
    ("{doc.sender}", "Absender"),
    ("{doc.number}", "Belegnummer"),
    ("{doc.amount}", "Betrag"),
    ("{doc.title}", "Titel"),
    ("{exif.city}", "Ort der Aufnahme"),
    ("{exif.country}", "Land der Aufnahme"),
    ("{exif.camera}", "Kamera"),
    ("{video.date:%Y}", "Videodatum mit Format"),
    ("{1}", "Gruppe 1 der name_regex"),
];

/// Ein Beispiel für eine neue Regel.
pub fn new_rule_defaults() -> Vec<(&'static str, super::fields::Value)> {
    vec![("target", super::fields::Value::Text("Sortiert/".into()))]
}

/// Prüft den Regeltext; die Config liefert Kategorien und Kategorie-Datei.
pub fn validate(text: &str, config: &Config) -> Result<(), super::ValidationError> {
    super::validate::rules_text(text, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::fields::{self, Value};
    use crate::settings::tables;

    #[test]
    fn jeder_platzhalter_ist_in_einem_zielmuster_gueltig() {
        for (placeholder, _) in PLACEHOLDERS {
            let text = format!(
                "[[rules]]\nname = \"p\"\next = [\"jpg\"]\nname_regex = \"(a)\"\ntarget = \"X/{placeholder}/\"\n"
            );
            // `category` und doc.* brauchen eine Inhaltsbedingung bzw. einen Ersatzwert
            let text = if placeholder.starts_with("{doc.")
                || placeholder.starts_with("{exif.c")
                || *placeholder == "{category}"
                || placeholder.starts_with("{video")
            {
                text.replace("target = \"X/", "target = \"X/{exif.date:%Y}/")
                    .replace(placeholder, &placeholder.replace('}', "|unbekannt}"))
            } else {
                text
            };
            if let Err(e) = validate(&text, &Config::default()) {
                assert!(
                    !e.message.contains("unbekannter Platzhalter"),
                    "{placeholder}: {e}"
                );
            }
        }
    }

    #[test]
    fn jedes_rule_feld_ist_der_regeldatei_bekannt() {
        for f in RULE_FIELDS {
            let sample = match &f.kind {
                FieldKind::Bool => Value::Bool(true),
                FieldKind::Float { min, .. } => Value::Float(*min),
                FieldKind::Duration => Value::Text("30d".into()),
                FieldKind::Size => Value::Text("1MB".into()),
                FieldKind::Choice(o) => Value::Text(o[0].into()),
                FieldKind::TextList | FieldKind::PathList => Value::List(vec!["x".into()]),
                _ => Value::Text("x".into()),
            };
            let mut doc: toml_edit::DocumentMut = "".parse().unwrap();
            let i = tables::add(&mut doc, KEY, "r", &new_rule_defaults());
            tables::set_field(&mut doc, KEY, i, f.key, &sample);
            if let Err(e) = validate(&doc.to_string(), &Config::default()) {
                assert!(!e.message.contains("unknown field"), "{}: {e}", f.key);
            }
        }
        let _ = fields::CONFIG_FIELDS.len();
    }

    #[test]
    fn neue_regel_mit_den_anfangswerten_ist_gueltig() {
        let mut doc: toml_edit::DocumentMut = "".parse().unwrap();
        let i = tables::add(&mut doc, KEY, "neu", &new_rule_defaults());
        tables::set_field(&mut doc, KEY, i, "ext", &Value::List(vec!["pdf".into()]));
        assert!(validate(&doc.to_string(), &Config::default()).is_ok());
    }
}
