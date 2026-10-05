//! Einstellungen bearbeiten: TOML-Dateien kommentarerhaltend (`toml_edit`), vor dem Schreiben
//! immer mit dem vorhandenen Parser geprüft, mit Sicherung `.bak` und atomar geschrieben.

pub mod config_doc;
pub mod fields;
pub mod rules_doc;
pub mod tables;
pub mod validate;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use toml_edit::DocumentMut;
use xxhash_rust::xxh3::xxh3_128;

use crate::paths;

/// Ein Fehler beim Prüfen, mit Fundstelle soweit bekannt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub message: String,
    /// 1-basierte Zeile im Text
    pub line: Option<usize>,
    /// Name der Regel bzw. Kategorie
    pub rule: Option<String>,
    /// Schlüssel des Feldes
    pub field: Option<String>,
}

impl ValidationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            line: None,
            rule: None,
            field: None,
        }
    }

    pub fn at_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(line) = self.line {
            write!(f, "Zeile {line}: ")?;
        }
        if let Some(rule) = &self.rule {
            write!(f, "„{rule}“: ")?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for ValidationError {}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("{0}")]
    Invalid(ValidationError),
    /// Die Datei wurde seit dem Laden von außen geändert
    #[error("Die Datei wurde extern geändert")]
    Conflict,
    #[error("{0}")]
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveResult {
    /// Sicherung der vorherigen Fassung (nur wenn die Datei schon existierte)
    pub backup: Option<PathBuf>,
}

/// Merkmale einer Datei auf der Platte, um externe Änderungen zu erkennen.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    mtime: Option<SystemTime>,
    hash: u128,
}

fn fingerprint_of(path: &Path) -> Option<Fingerprint> {
    let bytes = std::fs::read(paths::extended(path)).ok()?;
    let mtime = std::fs::metadata(paths::extended(path))
        .and_then(|m| m.modified())
        .ok();
    Some(Fingerprint {
        len: bytes.len() as u64,
        mtime,
        hash: xxh3_128(&bytes),
    })
}

/// Zeile aus einer Fehlermeldung („line 3“, „Zeile 3“) oder aus dem Span eines TOML-Fehlers.
pub fn line_in_message(message: &str) -> Option<usize> {
    for marker in ["line ", "Zeile ", "linie "] {
        if let Some(pos) = message.find(marker) {
            let digits: String = message[pos + marker.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(n) = digits.parse() {
                return Some(n);
            }
        }
    }
    None
}

fn line_of_offset(text: &str, offset: usize) -> usize {
    let mut end = offset.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].matches('\n').count() + 1
}

/// Parst TOML-Text; Syntaxfehler nennen die Zeile.
pub fn parse_document(text: &str) -> Result<DocumentMut, ValidationError> {
    text.parse::<DocumentMut>().map_err(|e| {
        let line = e.span().map(|s| line_of_offset(text, s.start));
        let mut error = ValidationError::new(e.message().to_string());
        error.line = line;
        error
    })
}

/// Eine TOML-Datei im Editor: Dokument mit Kommentaren, Fingerabdruck zum Zeitpunkt des Ladens.
pub struct EditableFile {
    pub path: PathBuf,
    doc: DocumentMut,
    fingerprint: Option<Fingerprint>,
}

impl EditableFile {
    /// Lädt die Datei; fehlt sie, entsteht ein leeres Dokument. Eine Datei mit TOML-Syntaxfehler
    /// ist ein Fehler (der Aufrufer bietet dann „Im Editor öffnen“ an).
    pub fn load_or_empty(path: &Path) -> Result<Self, ValidationError> {
        let text = match std::fs::read_to_string(paths::extended(path)) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return Err(ValidationError::new(format!(
                    "{} nicht lesbar: {e}",
                    paths::display(path)
                )))
            }
        };
        Ok(Self {
            path: path.to_path_buf(),
            doc: parse_document(&text)?,
            fingerprint: fingerprint_of(path),
        })
    }

    pub fn doc(&self) -> &DocumentMut {
        &self.doc
    }

    pub fn doc_mut(&mut self) -> &mut DocumentMut {
        &mut self.doc
    }

    pub fn text(&self) -> String {
        self.doc.to_string()
    }

    /// Ersetzt das Dokument durch den (Roh-)Text; ein Syntaxfehler ändert nichts.
    pub fn set_text(&mut self, text: &str) -> Result<(), ValidationError> {
        self.doc = parse_document(text)?;
        Ok(())
    }

    /// Die Datei hat sich seit dem Laden (oder letzten Speichern) auf der Platte geändert.
    pub fn changed_on_disk(&self) -> bool {
        fingerprint_of(&self.path) != self.fingerprint
    }

    /// Prüft mit `validate`, sichert die vorherige Fassung als `<datei>.bak` und schreibt atomar
    /// (temporäre Datei, dann Umbenennen). Bei `Invalid` und `Conflict` wird nichts geschrieben;
    /// `force` überschreibt eine externe Änderung ausdrücklich.
    pub fn save(
        &mut self,
        validate: &dyn Fn(&str) -> Result<(), ValidationError>,
        force: bool,
    ) -> Result<SaveResult, SaveError> {
        let text = self.doc.to_string();
        validate(&text).map_err(SaveError::Invalid)?;
        if !force && self.changed_on_disk() {
            return Err(SaveError::Conflict);
        }
        let io = |e: std::io::Error| SaveError::Io(format!("{}: {e}", paths::display(&self.path)));
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(paths::extended(dir)).map_err(io)?;
        }
        let backup = if self.path.exists() {
            let backup = with_suffix(&self.path, ".bak");
            std::fs::copy(paths::extended(&self.path), paths::extended(&backup)).map_err(io)?;
            Some(backup)
        } else {
            None
        };
        let temp = with_suffix(&self.path, ".tmp");
        std::fs::write(paths::extended(&temp), text.as_bytes()).map_err(io)?;
        if let Err(e) = std::fs::rename(paths::extended(&temp), paths::extended(&self.path)) {
            let _ = std::fs::remove_file(paths::extended(&temp));
            return Err(io(e));
        }
        self.fingerprint = fingerprint_of(&self.path);
        Ok(SaveResult { backup })
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Meine Config\ntop = 50 # Anzahl\n\n[classify]\nocr = true\n";

    fn ok(_: &str) -> Result<(), ValidationError> {
        Ok(())
    }

    fn file_with(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn rundreise_behaelt_kommentare_und_reihenfolge() {
        let (_dir, path) = file_with(SAMPLE);
        let mut file = EditableFile::load_or_empty(&path).unwrap();
        file.doc_mut()["classify"]["ocr_max_pages"] = toml_edit::value(3);
        let text = file.text();
        assert!(text.contains("# Meine Config"), "{text}");
        assert!(text.contains("top = 50 # Anzahl"), "{text}");
        assert!(text.find("top").unwrap() < text.find("[classify]").unwrap());
        assert!(text.contains("ocr_max_pages = 3"), "{text}");
    }

    #[test]
    fn fehlende_datei_ergibt_leeres_dokument_und_wird_beim_speichern_angelegt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("rules.toml");
        let mut file = EditableFile::load_or_empty(&path).unwrap();
        assert_eq!(file.text(), "");
        file.doc_mut()["top"] = toml_edit::value(1);
        let result = file.save(&ok, false).unwrap();
        assert_eq!(result.backup, None, "keine Sicherung ohne Vorgängerfassung");
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "top = 1");
    }

    #[test]
    fn syntaxfehler_beim_laden_nennt_die_zeile() {
        let (_dir, path) = file_with("top = 1\nkaputt = \n");
        let err = EditableFile::load_or_empty(&path).err().unwrap();
        assert_eq!(err.line, Some(2), "{err}");
    }

    #[test]
    fn ungueltiger_inhalt_wird_nie_geschrieben() {
        let (_dir, path) = file_with(SAMPLE);
        let mut file = EditableFile::load_or_empty(&path).unwrap();
        file.doc_mut()["top"] = toml_edit::value(-5);
        let reject = |_: &str| Err(ValidationError::new("top ungültig").at_line(2));
        match file.save(&reject, false) {
            Err(SaveError::Invalid(e)) => assert_eq!(e.line, Some(2)),
            other => panic!("{other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SAMPLE);
        assert!(!with_suffix(&path, ".bak").exists());
        assert!(!with_suffix(&path, ".tmp").exists());
    }

    #[test]
    fn speichern_legt_eine_sicherung_der_vorherigen_fassung_an() {
        let (_dir, path) = file_with(SAMPLE);
        let mut file = EditableFile::load_or_empty(&path).unwrap();
        file.doc_mut()["top"] = toml_edit::value(99);
        let result = file.save(&ok, false).unwrap();
        let backup = result.backup.unwrap();
        assert_eq!(backup, with_suffix(&path, ".bak"));
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), SAMPLE);
        assert!(std::fs::read_to_string(&path).unwrap().contains("top = 99"));
        assert!(!with_suffix(&path, ".tmp").exists());
        assert!(
            !file.changed_on_disk(),
            "nach dem Speichern wieder abgeglichen"
        );
    }

    #[test]
    fn externe_aenderung_wird_erkannt_und_nur_mit_force_ueberschrieben() {
        let (_dir, path) = file_with(SAMPLE);
        let mut file = EditableFile::load_or_empty(&path).unwrap();
        assert!(!file.changed_on_disk());
        std::fs::write(&path, "top = 7\n").unwrap();
        assert!(file.changed_on_disk());
        file.doc_mut()["top"] = toml_edit::value(99);
        assert!(matches!(file.save(&ok, false), Err(SaveError::Conflict)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "top = 7\n");
        file.save(&ok, true).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("top = 99"));
    }

    #[test]
    fn set_text_ersetzt_das_dokument_nur_bei_gueltiger_syntax() {
        let (_dir, path) = file_with(SAMPLE);
        let mut file = EditableFile::load_or_empty(&path).unwrap();
        assert!(file.set_text("top = \n").is_err());
        assert_eq!(file.text(), SAMPLE);
        file.set_text("top = 3\n").unwrap();
        assert_eq!(file.text(), "top = 3\n");
    }

    #[test]
    fn zeile_aus_fehlermeldungen() {
        assert_eq!(
            line_in_message("TOML parse error at line 4, column 2"),
            Some(4)
        );
        assert_eq!(line_in_message("Zeile 12: kaputt"), Some(12));
        assert_eq!(line_in_message("ohne Zeile"), None);
    }
}
