//! Felder der Config als Beschreibungen (`FieldSpec`) und das Lesen/Schreiben einzelner Werte in
//! einem `toml_edit`-Dokument. Der Formular-Renderer der GUI arbeitet nur mit diesen Typen.

use toml_edit::{Array, DocumentMut, Item, Table, Value as TomlValue};

use crate::analysis::age::parse_old_after;
use crate::config::{parse_bytes, Config, BUILTIN_JUNK_CATEGORIES, LLM_TASKS, PROFILE_PLAN_KINDS};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldKind {
    Bool,
    /// Ganzzahl ab `min`
    Int {
        min: i64,
    },
    Float {
        min: f64,
        max: f64,
    },
    Text,
    /// `1y`, `18m`, `90d`
    Duration,
    /// `100MB`, `5GB`
    Size,
    TextList,
    PathList,
    Choice(&'static [&'static str]),
    /// Mehrfachauswahl aus festen Werten (gespeichert als Liste)
    Multi(&'static [&'static str]),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    List(Vec<String>),
}

pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: FieldKind,
    /// Standardwert als Text (aus `Config::default()`), für die Anzeige
    pub default: fn(&Config) -> String,
}

const fn spec(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    kind: FieldKind,
    default: fn(&Config) -> String,
) -> FieldSpec {
    FieldSpec {
        key,
        label,
        help,
        kind,
        default,
    }
}

fn list_text(items: &[String]) -> String {
    if items.is_empty() {
        "leer".into()
    } else {
        items.join(", ")
    }
}

/// Allgemeine Felder der obersten Ebene der `config.toml`.
pub static CONFIG_FIELDS: &[FieldSpec] = &[
    spec(
        "old_after",
        "Alt ab",
        "Ab welchem Alter Dateien als „alt“ gelten (z. B. 1y, 18m, 90d)",
        FieldKind::Duration,
        |c| c.old_after.clone(),
    ),
    spec(
        "top",
        "Einträge in Top-Listen",
        "Anzahl der Einträge in den Berichtslisten",
        FieldKind::Int { min: 1 },
        |c| c.top.to_string(),
    ),
    spec(
        "max_depth_warning",
        "Warnung ab Verschachtelungstiefe",
        "Ordner tiefer als dies gelten als Strukturproblem",
        FieldKind::Int { min: 1 },
        |c| c.max_depth_warning.to_string(),
    ),
    spec(
        "huge_dir_entries",
        "Riesiger Ordner ab Einträgen",
        "Ordner mit mehr direkten Einträgen gelten als Strukturproblem",
        FieldKind::Int { min: 1 },
        |c| c.huge_dir_entries.to_string(),
    ),
    spec(
        "threads",
        "Scan-Threads",
        "0 = automatisch",
        FieldKind::Int { min: 0 },
        |c| c.threads.to_string(),
    ),
    spec(
        "quarantine_days",
        "Quarantänefrist (Tage)",
        "So lange bleibt Verschobenes in der Quarantäne, bevor „leeren“ es anbietet",
        FieldKind::Int { min: 1 },
        |c| c.quarantine_days.to_string(),
    ),
    spec(
        "reports_keep",
        "Berichte und Pläne aufbewahren",
        "Je Profil bei geplanten Läufen",
        FieldKind::Int { min: 1 },
        |c| c.reports_keep.to_string(),
    ),
    spec(
        "no_default_excludes",
        "Standard-Ausschlüsse abschalten",
        "Windows-, Programm- und Cache-Ordner werden sonst übersprungen",
        FieldKind::Bool,
        |c| c.no_default_excludes.to_string(),
    ),
    spec(
        "exclude",
        "Zusätzliche Ausschlüsse",
        "Ein Muster je Zeile",
        FieldKind::TextList,
        |c| list_text(&c.exclude),
    ),
    spec(
        "summary_only",
        "Nur Summe",
        "Ordner, die nur als Summe zählen (ein Muster je Zeile)",
        FieldKind::TextList,
        |c| list_text(&c.summary_only),
    ),
    spec(
        "protected_paths",
        "Geschützte Pfade",
        "Hier ändern Pläne nie etwas (ein Pfad je Zeile)",
        FieldKind::PathList,
        |c| list_text(&c.protected_paths),
    ),
    spec(
        "junk_categories",
        "Müll-Kategorien",
        "Welche Kategorien `plan junk` standardmäßig prüft",
        FieldKind::Multi(&BUILTIN_JUNK_CATEGORIES),
        |c| list_text(&c.junk_categories),
    ),
    spec(
        "installer_min_age",
        "Installer-Mindestalter",
        "Installer in Downloads ab diesem Alter gelten als Müll",
        FieldKind::Duration,
        |c| c.installer_min_age.clone(),
    ),
    spec(
        "downloads_dirs",
        "Download-Ordner",
        "Ersetzt den Windows-Ordner „Downloads“ (ein Pfad je Zeile)",
        FieldKind::PathList,
        |c| list_text(&c.downloads_dirs),
    ),
    spec(
        "archive_older_than",
        "Archivieren ab Alter",
        "Standard für `plan archive`",
        FieldKind::Duration,
        |c| c.archive_older_than.clone(),
    ),
    spec(
        "versions_min_age",
        "Versionen ab Alter",
        "Standard für `plan versions`",
        FieldKind::Duration,
        |c| c.versions_min_age.clone(),
    ),
    spec(
        "onedrive_max_move_files",
        "OneDrive: höchstens Dateien",
        "Obergrenze je Plan unter OneDrive",
        FieldKind::Int { min: 0 },
        |c| c.onedrive_max_move_files.to_string(),
    ),
    spec(
        "onedrive_max_move_bytes",
        "OneDrive: höchstens Größe",
        "Obergrenze je Plan unter OneDrive (z. B. 5GB)",
        FieldKind::Size,
        |c| c.onedrive_max_move_bytes.clone(),
    ),
    spec(
        "onedrive_conflict_hostnames",
        "OneDrive-Konflikt-Rechnernamen",
        "Zusätzliche Rechnernamen für Konfliktdateien",
        FieldKind::TextList,
        |c| list_text(&c.onedrive_conflict_hostnames),
    ),
    spec(
        "rules_file",
        "Regeldatei",
        "Leer = rules.toml neben der Config",
        FieldKind::Text,
        |_| "rules.toml".into(),
    ),
];

pub static CLASSIFY_FIELDS: &[FieldSpec] = &[
    spec(
        "min_confidence",
        "Mindest-Konfidenz",
        "Darunter landet eine Datei in „Zum Prüfen“ (0 bis 1)",
        FieldKind::Float { min: 0.0, max: 1.0 },
        |c| c.classify.min_confidence.to_string(),
    ),
    spec(
        "max_file_size",
        "Größte gelesene Datei",
        "Größere Dateien werden nicht gelesen (z. B. 100MB)",
        FieldKind::Size,
        |c| c.classify.max_file_size.clone(),
    ),
    spec(
        "ocr",
        "OCR verwenden",
        "Windows-Texterkennung für Scans und Bilder",
        FieldKind::Bool,
        |c| c.classify.ocr.to_string(),
    ),
    spec(
        "ocr_languages",
        "OCR-Sprachen",
        "Sprach-Tags, z. B. de, en",
        FieldKind::TextList,
        |c| list_text(&c.classify.ocr_languages),
    ),
    spec(
        "ocr_max_pages",
        "OCR-Seiten je PDF",
        "Höchstens so viele Seiten werden gelesen",
        FieldKind::Int { min: 1 },
        |c| c.classify.ocr_max_pages.to_string(),
    ),
    spec(
        "ocr_image_globs",
        "Bilder, die per OCR gelesen werden",
        "Globs relativ zur Wurzel",
        FieldKind::TextList,
        |c| list_text(&c.classify.ocr_image_globs),
    ),
    spec(
        "cache_ocr_text",
        "OCR-Text zwischenspeichern",
        "Nur verschlüsselt (Windows-DPAPI) im Index",
        FieldKind::Bool,
        |c| c.classify.cache_ocr_text.to_string(),
    ),
    spec(
        "max_text_chars",
        "Höchstens Zeichen je Datei",
        "Mindestens 1000",
        FieldKind::Int { min: 1000 },
        |c| c.classify.max_text_chars.to_string(),
    ),
    spec(
        "threads",
        "Threads",
        "0 = halbe Kernanzahl",
        FieldKind::Int { min: 0 },
        |c| c.classify.threads.to_string(),
    ),
    spec(
        "categories_file",
        "Kategorie-Datei",
        "Leer = categories.toml neben der Config",
        FieldKind::Text,
        |_| "categories.toml".into(),
    ),
];

pub static LLM_FIELDS: &[FieldSpec] = &[
    spec(
        "enabled",
        "Lokales LLM verwenden",
        "Nur Loopback (Ollama auf diesem PC)",
        FieldKind::Bool,
        |c| c.llm.enabled.to_string(),
    ),
    spec(
        "endpoint",
        "Endpunkt",
        "Muss auf 127.0.0.1, ::1 oder localhost zeigen",
        FieldKind::Text,
        |c| c.llm.endpoint.clone(),
    ),
    spec(
        "model",
        "Modell",
        "z. B. qwen2.5:7b",
        FieldKind::Text,
        |c| c.llm.model.clone(),
    ),
    spec(
        "timeout",
        "Zeitlimit je Anfrage",
        "z. B. 60s, 2m",
        FieldKind::Text,
        |c| c.llm.timeout.clone(),
    ),
    spec(
        "max_input_chars",
        "Höchstens Eingabezeichen",
        "Wie viel Text das Modell sieht",
        FieldKind::Int { min: 100 },
        |c| c.llm.max_input_chars.to_string(),
    ),
    spec(
        "tasks",
        "Aufgaben des LLM",
        "Was das Modell übernehmen darf",
        FieldKind::Multi(&LLM_TASKS),
        |c| list_text(&c.llm.tasks),
    ),
    spec(
        "max_confidence",
        "Höchste Konfidenz bei LLM-Kategorie",
        "Unter Mindest-Konfidenz halten (0 bis 1)",
        FieldKind::Float { min: 0.0, max: 1.0 },
        |c| c.llm.max_confidence.to_string(),
    ),
];

pub static NOTIFY_FIELDS: &[FieldSpec] = &[
    spec(
        "score_drop",
        "Score fällt um mindestens",
        "Benachrichtigung bei geplanten Läufen",
        FieldKind::Int { min: 0 },
        |c| c.notify.score_drop.to_string(),
    ),
    spec(
        "score_below",
        "Score liegt unter",
        "Benachrichtigung bei geplanten Läufen",
        FieldKind::Int { min: 0 },
        |c| c.notify.score_below.to_string(),
    ),
];

pub static GUI_FIELDS: &[FieldSpec] = &[spec(
    "plans_dir",
    "Ablage der GUI-Pläne",
    "Leer = %LOCALAPPDATA%\\ordner-cleanup\\plans\\_gui",
    FieldKind::Text,
    |_| "plans\\_gui".into(),
)];

/// Felder eines Profils (Abweichungen von der globalen Config; leer = wie global).
pub static PROFILE_FIELDS: &[FieldSpec] = &[
    spec(
        "root",
        "Wurzel",
        "Der Ordner, den das Profil verwaltet",
        FieldKind::Text,
        |_| "(erforderlich)".into(),
    ),
    spec(
        "template",
        "Vorlage",
        "para, johnny-decimal oder Pfad einer Vorlagendatei",
        FieldKind::Text,
        |_| "keine".into(),
    ),
    spec(
        "rules_file",
        "Regeldatei",
        "Relativ zum Config-Ordner",
        FieldKind::Text,
        |_| "wie global".into(),
    ),
    spec(
        "categories_file",
        "Kategorie-Datei",
        "Relativ zum Config-Ordner",
        FieldKind::Text,
        |_| "wie global".into(),
    ),
    spec(
        "plans",
        "Pläne bei geplanten Läufen",
        "Was `run` zusätzlich plant (nie anwenden)",
        FieldKind::Multi(&PROFILE_PLAN_KINDS),
        |_| "keine".into(),
    ),
    spec(
        "force",
        "Netzlaufwerk erlauben",
        "Scan auch auf Netzlaufwerken",
        FieldKind::Bool,
        |_| "false".into(),
    ),
    spec(
        "classify",
        "Nach dem Scan klassifizieren",
        "Nur bei geplanten Läufen",
        FieldKind::Bool,
        |_| "wie global".into(),
    ),
    spec(
        "llm",
        "LLM für dieses Profil",
        "Überschreibt [llm] enabled",
        FieldKind::Bool,
        |_| "wie global".into(),
    ),
    spec(
        "exclude",
        "Zusätzliche Ausschlüsse",
        "Werden zur globalen Liste ergänzt",
        FieldKind::TextList,
        |_| "keine".into(),
    ),
    spec(
        "summary_only",
        "Nur Summe",
        "Werden zur globalen Liste ergänzt",
        FieldKind::TextList,
        |_| "keine".into(),
    ),
    spec(
        "protected_paths",
        "Geschützte Pfade",
        "Werden zur globalen Liste ergänzt",
        FieldKind::PathList,
        |_| "keine".into(),
    ),
    spec(
        "junk_categories",
        "Müll-Kategorien",
        "Werden zur globalen Liste ergänzt",
        FieldKind::Multi(&BUILTIN_JUNK_CATEGORIES),
        |_| "keine".into(),
    ),
    spec(
        "no_default_excludes",
        "Standard-Ausschlüsse abschalten",
        "Leer = wie global",
        FieldKind::Bool,
        |_| "wie global".into(),
    ),
    spec(
        "old_after",
        "Alt ab",
        "Leer = wie global",
        FieldKind::Duration,
        |_| "wie global".into(),
    ),
    spec(
        "installer_min_age",
        "Installer-Mindestalter",
        "Leer = wie global",
        FieldKind::Duration,
        |_| "wie global".into(),
    ),
    spec(
        "archive_older_than",
        "Archivieren ab Alter",
        "Leer = wie global",
        FieldKind::Duration,
        |_| "wie global".into(),
    ),
    spec(
        "versions_min_age",
        "Versionen ab Alter",
        "Leer = wie global",
        FieldKind::Duration,
        |_| "wie global".into(),
    ),
    spec(
        "top",
        "Einträge in Top-Listen",
        "Leer = wie global",
        FieldKind::Int { min: 1 },
        |_| "wie global".into(),
    ),
    spec(
        "max_depth_warning",
        "Warnung ab Tiefe",
        "Leer = wie global",
        FieldKind::Int { min: 1 },
        |_| "wie global".into(),
    ),
    spec(
        "huge_dir_entries",
        "Riesiger Ordner ab Einträgen",
        "Leer = wie global",
        FieldKind::Int { min: 1 },
        |_| "wie global".into(),
    ),
    spec(
        "threads",
        "Scan-Threads",
        "Leer = wie global",
        FieldKind::Int { min: 0 },
        |_| "wie global".into(),
    ),
];

/// Text aus dem Eingabefeld in einen Wert dieser Art umwandeln; der Fehler ist für den Nutzer.
pub fn parse_input(kind: &FieldKind, text: &str) -> Result<Value, String> {
    let text = text.trim();
    match kind {
        FieldKind::Bool => match text.to_lowercase().as_str() {
            "true" | "ja" | "an" => Ok(Value::Bool(true)),
            "false" | "nein" | "aus" => Ok(Value::Bool(false)),
            _ => Err("erwartet an oder aus".into()),
        },
        FieldKind::Int { min } => {
            let n: i64 = text
                .parse()
                .map_err(|_| "erwartet eine ganze Zahl".to_string())?;
            if n < *min {
                return Err(format!("muss mindestens {min} sein"));
            }
            Ok(Value::Int(n))
        }
        FieldKind::Float { min, max } => {
            let n: f64 = text
                .replace(',', ".")
                .parse()
                .map_err(|_| "erwartet eine Zahl".to_string())?;
            if !(*min..=*max).contains(&n) {
                return Err(format!("muss zwischen {min} und {max} liegen"));
            }
            Ok(Value::Float(n))
        }
        FieldKind::Duration => {
            parse_old_after(text).map_err(|e| e.to_string())?;
            Ok(Value::Text(text.to_string()))
        }
        FieldKind::Size => {
            parse_bytes(text)
                .ok_or_else(|| "erwartet eine Größe wie 100MB oder 5GB".to_string())?;
            Ok(Value::Text(text.to_string()))
        }
        FieldKind::Text => Ok(Value::Text(text.to_string())),
        FieldKind::Choice(options) => {
            if options.contains(&text) {
                Ok(Value::Text(text.to_string()))
            } else {
                Err(format!("erlaubt: {}", options.join(", ")))
            }
        }
        FieldKind::TextList | FieldKind::PathList | FieldKind::Multi(_) => {
            let items: Vec<String> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            if let FieldKind::Multi(options) = kind {
                if let Some(bad) = items.iter().find(|i| !options.contains(&i.as_str())) {
                    return Err(format!(
                        "unbekannter Wert „{bad}“ (erlaubt: {})",
                        options.join(", ")
                    ));
                }
            }
            Ok(Value::List(items))
        }
    }
}

/// Wert als Text für das Eingabefeld.
pub fn to_input(value: &Value) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Text(t) => t.clone(),
        Value::List(items) => items.join("\n"),
    }
}

fn to_toml(value: &Value) -> TomlValue {
    match value {
        Value::Bool(b) => TomlValue::from(*b),
        Value::Int(n) => TomlValue::from(*n),
        Value::Float(f) => TomlValue::from(*f),
        Value::Text(t) => TomlValue::from(t.as_str()),
        Value::List(items) => TomlValue::Array(items.iter().map(String::as_str).collect::<Array>()),
    }
}

fn from_toml(value: &TomlValue) -> Option<Value> {
    match value {
        TomlValue::Boolean(b) => Some(Value::Bool(*b.value())),
        TomlValue::Integer(n) => Some(Value::Int(*n.value())),
        TomlValue::Float(f) => Some(Value::Float(*f.value())),
        TomlValue::String(s) => Some(Value::Text(s.value().clone())),
        TomlValue::Array(a) => Some(Value::List(
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
        )),
        _ => None,
    }
}

fn table_at<'a>(doc: &'a DocumentMut, base: &[&str]) -> Option<&'a Table> {
    let mut table = doc.as_table();
    for key in base {
        table = table.get(key)?.as_table()?;
    }
    Some(table)
}

/// Die Tabelle unter `base`; fehlende Ebenen werden angelegt (implizit, ohne eigene Kopfzeile,
/// solange sie leer bleiben).
fn table_at_mut<'a>(doc: &'a mut DocumentMut, base: &[&str]) -> &'a mut Table {
    let mut table = doc.as_table_mut();
    for key in base {
        if !table.contains_key(key) {
            let mut t = Table::new();
            t.set_implicit(true);
            table.insert(key, Item::Table(t));
        }
        table = table
            .get_mut(key)
            .and_then(Item::as_table_mut)
            .expect("Ebene ist eine Tabelle");
    }
    table
}

/// Wert eines Schlüssels in einer Tabelle, falls gesetzt.
pub fn get_in_table(table: &Table, key: &str) -> Option<Value> {
    from_toml(table.get(key)?.as_value()?)
}

/// Setzt einen Wert in einer Tabelle; Kommentare am Wert und Reihenfolge bleiben erhalten.
pub fn set_in_table(table: &mut Table, key: &str, value: &Value) {
    let mut new = to_toml(value);
    if let Some(Item::Value(old)) = table.get_mut(key) {
        *new.decor_mut() = old.decor().clone();
        *old = new;
    } else {
        table.insert(key, Item::Value(new));
    }
}

/// Wert eines Schlüssels unter `base`, falls gesetzt.
pub fn get(doc: &DocumentMut, base: &[&str], key: &str) -> Option<Value> {
    get_in_table(table_at(doc, base)?, key)
}

/// Setzt einen Wert; Kommentare am Wert und Reihenfolge bleiben erhalten.
pub fn set(doc: &mut DocumentMut, base: &[&str], key: &str, value: &Value) {
    set_in_table(table_at_mut(doc, base), key, value);
}

/// Wo ein Feld liegt: unter einem Tabellenpfad oder in einem Eintrag einer Tabellenliste.
#[derive(Debug, Clone, Copy)]
pub enum Loc<'a> {
    Path(&'a [&'a str]),
    Entry { list: &'a str, index: usize },
}

impl Loc<'_> {
    /// Kennung für Eingabepuffer und Widget-IDs
    pub fn id(&self) -> String {
        match self {
            Loc::Path(base) => base.join("."),
            Loc::Entry { list, index } => format!("{list}[{index}]"),
        }
    }
}

pub fn get_at(doc: &DocumentMut, loc: Loc, key: &str) -> Option<Value> {
    match loc {
        Loc::Path(base) => get(doc, base, key),
        Loc::Entry { list, index } => super::tables::get_field(doc, list, index, key),
    }
}

pub fn set_at(doc: &mut DocumentMut, loc: Loc, key: &str, value: &Value) {
    match loc {
        Loc::Path(base) => set(doc, base, key, value),
        Loc::Entry { list, index } => super::tables::set_field(doc, list, index, key, value),
    }
}

pub fn unset_at(doc: &mut DocumentMut, loc: Loc, key: &str) {
    match loc {
        Loc::Path(base) => unset(doc, base, key),
        Loc::Entry { list, index } => super::tables::unset_field(doc, list, index, key),
    }
}

/// Schreibt die Eingabe eines Feldes: leer entfernt den Schlüssel (Standard), sonst wird sie
/// geprüft und geschrieben. Bei einem Fehler bleibt das Dokument unverändert.
pub fn apply_text(
    doc: &mut DocumentMut,
    base: &[&str],
    spec: &FieldSpec,
    text: &str,
) -> Result<(), String> {
    apply_text_at(doc, Loc::Path(base), spec, text)
}

/// Wie [`apply_text`], für einen beliebigen Ort.
pub fn apply_text_at(
    doc: &mut DocumentMut,
    loc: Loc,
    spec: &FieldSpec,
    text: &str,
) -> Result<(), String> {
    let is_list = matches!(
        spec.kind,
        FieldKind::TextList | FieldKind::PathList | FieldKind::Multi(_)
    );
    if text.trim().is_empty() && !matches!(spec.kind, FieldKind::Multi(_)) {
        unset_at(doc, loc, spec.key);
        return Ok(());
    }
    let value = parse_input(&spec.kind, text)?;
    if is_list && matches!(&value, Value::List(items) if items.is_empty()) {
        unset_at(doc, loc, spec.key);
    } else {
        set_at(doc, loc, spec.key, &value);
    }
    Ok(())
}

/// Entfernt den Schlüssel („zurücksetzen“ auf den Standard).
pub fn unset(doc: &mut DocumentMut, base: &[&str], key: &str) {
    if table_at(doc, base).is_some() {
        table_at_mut(doc, base).remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> DocumentMut {
        text.parse().unwrap()
    }

    #[test]
    fn setzen_behaelt_kommentare_und_reihenfolge() {
        let mut d = doc("# Kopf\ntop = 50 # Anzahl\nold_after = \"1y\"\n");
        set(&mut d, &[], "top", &Value::Int(80));
        let text = d.to_string();
        assert!(text.contains("# Kopf"), "{text}");
        assert!(text.contains("top = 80 # Anzahl"), "{text}");
        assert!(text.find("top").unwrap() < text.find("old_after").unwrap());
        assert_eq!(get(&d, &[], "top"), Some(Value::Int(80)));
    }

    #[test]
    fn zuruecksetzen_entfernt_nur_den_schluessel() {
        let mut d = doc("[classify]\nocr = false # aus\nocr_max_pages = 3\n");
        unset(&mut d, &["classify"], "ocr");
        let text = d.to_string();
        assert!(
            !text.contains("ocr = false") && text.contains("ocr_max_pages = 3"),
            "{text}"
        );
        assert_eq!(get(&d, &["classify"], "ocr"), None);
        unset(&mut d, &["gibt", "es", "nicht"], "x");
    }

    #[test]
    fn fehlende_ebenen_werden_angelegt() {
        let mut d = doc("top = 1\n");
        set(
            &mut d,
            &["profiles", "foto"],
            "root",
            &Value::Text(r"D:\Fotos".into()),
        );
        set(
            &mut d,
            &["profiles", "foto"],
            "plans",
            &Value::List(vec!["junk".into()]),
        );
        let text = d.to_string();
        assert!(text.contains("[profiles.foto]"), "{text}");
        let cfg = Config::parse(&text).unwrap();
        assert_eq!(cfg.profiles["foto"].root, r"D:\Fotos");
        assert_eq!(cfg.profiles["foto"].plans, ["junk"]);
    }

    #[test]
    fn eingabe_schreibt_prueft_und_leer_setzt_zurueck() {
        let top = CONFIG_FIELDS.iter().find(|f| f.key == "top").unwrap();
        let mut d = doc("top = 50 # Anzahl
");
        apply_text(&mut d, &[], top, "80").unwrap();
        assert!(d.to_string().contains("top = 80 # Anzahl"), "{d}");
        assert!(apply_text(&mut d, &[], top, "viele").is_err());
        assert_eq!(
            get(&d, &[], "top"),
            Some(Value::Int(80)),
            "Fehler ändert nichts"
        );
        apply_text(&mut d, &[], top, "  ").unwrap();
        assert_eq!(get(&d, &[], "top"), None);

        let exclude = CONFIG_FIELDS.iter().find(|f| f.key == "exclude").unwrap();
        apply_text(
            &mut d,
            &[],
            exclude,
            "*.tmp
node_modules",
        )
        .unwrap();
        assert_eq!(
            get(&d, &[], "exclude"),
            Some(Value::List(vec!["*.tmp".into(), "node_modules".into()]))
        );
        apply_text(&mut d, &[], exclude, "").unwrap();
        assert_eq!(get(&d, &[], "exclude"), None);
    }

    #[test]
    fn eingaben_werden_je_art_geprueft() {
        assert_eq!(
            parse_input(&FieldKind::Int { min: 1 }, " 5 "),
            Ok(Value::Int(5))
        );
        assert!(parse_input(&FieldKind::Int { min: 1 }, "0").is_err());
        assert!(parse_input(&FieldKind::Int { min: 1 }, "x").is_err());
        assert_eq!(
            parse_input(&FieldKind::Float { min: 0.0, max: 1.0 }, "0,8"),
            Ok(Value::Float(0.8))
        );
        assert!(parse_input(&FieldKind::Float { min: 0.0, max: 1.0 }, "1.5").is_err());
        assert_eq!(
            parse_input(&FieldKind::Duration, "90d"),
            Ok(Value::Text("90d".into()))
        );
        assert!(parse_input(&FieldKind::Duration, "bald").is_err());
        assert_eq!(
            parse_input(&FieldKind::Size, "5GB"),
            Ok(Value::Text("5GB".into()))
        );
        assert!(parse_input(&FieldKind::Size, "viel").is_err());
        assert_eq!(parse_input(&FieldKind::Bool, "an"), Ok(Value::Bool(true)));
        assert!(parse_input(&FieldKind::Bool, "vielleicht").is_err());
        assert_eq!(
            parse_input(&FieldKind::TextList, "a\n\n b \n"),
            Ok(Value::List(vec!["a".into(), "b".into()]))
        );
        let multi = FieldKind::Multi(&["temp", "system"]);
        assert!(parse_input(&multi, "temp\nsystem").is_ok());
        assert!(parse_input(&multi, "temp\nnix")
            .unwrap_err()
            .contains("nix"));
    }

    #[test]
    fn jede_spec_hat_einen_gueltigen_default_text_und_schluessel() {
        for (name, specs) in [
            ("config", CONFIG_FIELDS),
            ("classify", CLASSIFY_FIELDS),
            ("llm", LLM_FIELDS),
            ("notify", NOTIFY_FIELDS),
            ("gui", GUI_FIELDS),
            ("profile", PROFILE_FIELDS),
        ] {
            let mut seen = std::collections::HashSet::new();
            for f in specs {
                assert!(seen.insert(f.key), "{name}: {} doppelt", f.key);
                assert!(
                    !(f.default)(&Config::default()).is_empty(),
                    "{name}.{}",
                    f.key
                );
            }
        }
    }

    /// Jedes Feld lässt sich mit einem Beispielwert schreiben und die Config nimmt es an. Das
    /// fängt Tippfehler in Schlüsseln (die Config lehnt unbekannte Schlüssel ab).
    #[test]
    fn jeder_schluessel_ist_der_config_bekannt() {
        let sample = |kind: &FieldKind| match kind {
            FieldKind::Bool => Value::Bool(true),
            FieldKind::Int { min } => Value::Int((*min).max(2000)),
            FieldKind::Float { min, .. } => Value::Float(*min),
            FieldKind::Duration => Value::Text("30d".into()),
            FieldKind::Size => Value::Text("1GB".into()),
            FieldKind::Text => Value::Text("x".into()),
            FieldKind::TextList | FieldKind::PathList => Value::List(vec![]),
            FieldKind::Choice(o) => Value::Text(o[0].into()),
            FieldKind::Multi(o) => Value::List(vec![o[0].into()]),
        };
        for (base, specs) in [
            (vec![], CONFIG_FIELDS),
            (vec!["classify"], CLASSIFY_FIELDS),
            (vec!["notify"], NOTIFY_FIELDS),
            (vec!["gui"], GUI_FIELDS),
            (vec!["profiles", "p"], PROFILE_FIELDS),
        ] {
            for f in specs {
                let mut d = doc("[profiles.p]\nroot = \"D:\\\\x\"\n");
                set(&mut d, &base, f.key, &sample(&f.kind));
                if let Err(e) = Config::parse(&d.to_string()) {
                    // llm.endpoint o. Ä. dürfen am Inhalt scheitern, nicht am Schlüssel
                    assert!(
                        !e.to_string().contains("unknown field"),
                        "{base:?}.{}: {e}",
                        f.key
                    );
                }
            }
        }
        for f in LLM_FIELDS {
            let mut d = doc("");
            set(&mut d, &["llm"], f.key, &sample(&f.kind));
            if let Err(e) = Config::parse(&d.to_string()) {
                assert!(
                    !e.to_string().contains("unknown field"),
                    "llm.{}: {e}",
                    f.key
                );
            }
        }
    }
}
