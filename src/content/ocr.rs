//! OCR-Schnittstelle der Pipeline. Die echte Implementierung (Windows-OCR) liegt in
//! `platform::ocr`; Tests nutzen Fakes, damit sie ohne Sprachpaket laufen.

use std::path::Path;

/// Wie weit OCR gekommen ist: Text und gelesene Seiten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrText {
    pub text: String,
    pub pages: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrError {
    /// Keines der gewünschten Sprachpakete ist installiert.
    NoLanguage,
    /// Passwortgeschütztes PDF
    Encrypted,
    /// Datei nicht lesbar oder kein gültiges Bild/PDF
    Corrupt,
    /// Datei gesperrt, Cloud-Platzhalter oder ähnlich: später erneut versuchen
    Unavailable(String),
    /// Sonstiger Fehler der OCR-Engine
    Failed(String),
}

impl std::fmt::Display for OcrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoLanguage => f.write_str("kein OCR-Sprachpaket installiert"),
            Self::Encrypted => f.write_str("verschlüsselt"),
            Self::Corrupt => f.write_str("nicht lesbar"),
            Self::Unavailable(why) => write!(f, "nicht verfügbar ({why})"),
            Self::Failed(why) => write!(f, "OCR fehlgeschlagen ({why})"),
        }
    }
}

impl std::error::Error for OcrError {}

/// Texterkennung für Bilder und gescannte PDFs.
pub trait Ocr: Sync {
    /// Sprachen, die tatsächlich benutzt werden (z. B. `["de-DE", "en-US"]`), oder leer.
    fn languages(&self) -> Vec<String>;
    fn image(&self, path: &Path) -> Result<OcrText, OcrError>;
    /// Liest höchstens `max_pages` Seiten.
    fn pdf(&self, path: &Path, max_pages: usize) -> Result<OcrText, OcrError>;
}

/// Hinweis für Nutzer, wenn das Sprachpaket fehlt.
pub const NO_LANGUAGE_HINT: &str = "OCR-Sprachpaket fehlt: Windows-Einstellungen → Zeit und Sprache → Sprache und Region → Sprache hinzufügen (mit „Texterkennung“)";

#[cfg(test)]
pub mod fake {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;

    /// Liefert je Dateiname einen festen Text und zählt Aufrufe.
    #[derive(Default)]
    pub struct FakeOcr {
        pub texts: Mutex<HashMap<String, String>>,
        pub calls: AtomicUsize,
        pub no_language: bool,
    }

    impl FakeOcr {
        pub fn with(name: &str, text: &str) -> Self {
            let me = Self::default();
            me.texts.lock().unwrap().insert(name.into(), text.into());
            me
        }
        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
        fn run(&self, path: &Path) -> Result<OcrText, OcrError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.no_language {
                return Err(OcrError::NoLanguage);
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            self.texts
                .lock()
                .unwrap()
                .get(&name)
                .map(|t| OcrText {
                    text: t.clone(),
                    pages: 1,
                })
                .ok_or(OcrError::Corrupt)
        }
    }

    impl Ocr for FakeOcr {
        fn languages(&self) -> Vec<String> {
            if self.no_language {
                Vec::new()
            } else {
                vec!["de-DE".into()]
            }
        }
        fn image(&self, path: &Path) -> Result<OcrText, OcrError> {
            self.run(path)
        }
        fn pdf(&self, path: &Path, _max_pages: usize) -> Result<OcrText, OcrError> {
            self.run(path)
        }
    }
}
