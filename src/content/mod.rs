//! Inhaltsbasierte Klassifikation (Phase 6a): Extraktion von Text und Metadaten, Zuordnung
//! zu Kategorien, Felder für Regeln und Bericht. Es wird nie Volltext gespeichert.

pub mod classify;
pub mod extract;
pub mod geo;
pub mod llm;
pub mod ocr;
pub mod pipeline;
pub mod record;

use std::collections::BTreeMap;

/// Wird erhöht, wenn sich ändert, was die Extraktoren liefern (macht den Cache ungültig).
pub const EXTRACTOR_VERSION: i64 = 1;
/// Wird erhöht, wenn sich Konfidenzformel oder Feldmuster ändern.
pub const CLASSIFIER_VERSION: i64 = 2;
/// Wird erhöht, wenn sich die OCR-Ausgabe ändert (macht den OCR-Text-Cache ungültig).
pub const OCR_VERSION: i64 = 1;

pub use record::{cache_state, CacheState, ContentRecord};

/// Feldname → Wert (`doc.date` → `2026-09-30`).
pub type Fields = BTreeMap<String, String>;

/// Alle Feldnamen, die Regeln (`fields`, Platzhalter) und Bericht kennen.
pub const FIELD_NAMES: &[&str] = &[
    "doc.date",
    "doc.sender",
    "doc.number",
    "doc.amount",
    "doc.title",
    "doc.author",
    "doc.pages",
    "exif.camera",
    "exif.lat",
    "exif.lon",
    "exif.has_gps",
    "exif.city",
    "exif.country",
    "video.date",
];

pub fn is_known_field(name: &str) -> bool {
    FIELD_NAMES.contains(&name)
}

/// Zustand der Extraktion einer Datei.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// Grund: `encrypted`, `corrupt`, `locked`, `timeout`, `no-ocr-language` …
    Unreadable(String),
    Unsupported,
    TooLarge,
}

impl Status {
    pub fn as_str(&self) -> String {
        match self {
            Self::Ok => "ok".into(),
            Self::Unreadable(why) => format!("unreadable:{why}"),
            Self::Unsupported => "unsupported".into(),
            Self::TooLarge => "too-large".into(),
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "ok" => Self::Ok,
            "unsupported" => Self::Unsupported,
            "too-large" => Self::TooLarge,
            other => Self::Unreadable(
                other
                    .strip_prefix("unreadable:")
                    .unwrap_or("unknown")
                    .to_string(),
            ),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }
}

/// Woher Kategorie bzw. Feld kommt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Rules,
    Llm,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rules => "rules",
            Self::Llm => "llm",
        }
    }
    pub fn parse(s: &str) -> Self {
        if s == "llm" {
            Self::Llm
        } else {
            Self::Rules
        }
    }
}

/// Woher der Text stammt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextSource {
    Layer,
    Office,
    Ocr,
    #[default]
    None,
}

impl TextSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Layer => "layer",
            Self::Office => "office",
            Self::Ocr => "ocr",
            Self::None => "none",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "layer" => Self::Layer,
            "office" => Self::Office,
            "ocr" => Self::Ocr,
            _ => Self::None,
        }
    }
}
