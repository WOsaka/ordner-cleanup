//! Ergebnis der Inhaltsklassifikation je Datei (`ContentRecord`) und seine Umwandlung von und
//! zu Zeilen des Inhalts-Cache.

use std::collections::BTreeMap;

use super::classify::fields::FieldSources;
use super::{Fields, Source, Status, TextSource, EXTRACTOR_VERSION};
use crate::index::CachedContent;

#[derive(Debug, Clone, PartialEq)]
pub struct ContentRecord {
    pub path_key: String,
    pub size: i64,
    pub mtime: i64,
    pub status: Status,
    pub category: Option<String>,
    pub confidence: f32,
    pub category2: Option<(String, f32)>,
    pub source: Source,
    /// Ausschlaggebende Treffer der Kategorie, stärkste zuerst
    pub hits: Vec<String>,
    pub fields: Fields,
    pub field_sources: FieldSources,
    pub text_source: TextSource,
    pub llm_model: Option<String>,
    pub defs_fingerprint: String,
    pub extractor_version: i64,
    pub classified_at: String,
}

impl ContentRecord {
    /// Eine Zeile für `content_store`.
    pub fn to_cached(&self) -> CachedContent {
        CachedContent {
            path_key: self.path_key.clone(),
            size: self.size,
            mtime: self.mtime,
            extractor_version: self.extractor_version,
            defs_fingerprint: self.defs_fingerprint.clone(),
            status: self.status.as_str(),
            category: self.category.clone(),
            confidence: self.category.as_ref().map(|_| f64::from(self.confidence)),
            category2: self.category2.as_ref().map(|(c, _)| c.clone()),
            confidence2: self.category2.as_ref().map(|(_, c)| f64::from(*c)),
            source: self
                .category
                .as_ref()
                .map(|_| self.source.as_str().to_string()),
            hits: Some(serde_json::to_string(&self.hits).unwrap_or_else(|_| "[]".into())),
            fields: serde_json::to_string(&self.fields).unwrap_or_else(|_| "{}".into()),
            field_sources: serde_json::to_string(&self.field_sources)
                .unwrap_or_else(|_| "{}".into()),
            text_source: Some(self.text_source.as_str().to_string()),
            llm_model: self.llm_model.clone(),
            classified_at: self.classified_at.clone(),
        }
    }

    pub fn from_cached(c: &CachedContent) -> Self {
        let category2 = match (&c.category2, c.confidence2) {
            (Some(name), Some(conf)) => Some((name.clone(), conf as f32)),
            _ => None,
        };
        Self {
            path_key: c.path_key.clone(),
            size: c.size,
            mtime: c.mtime,
            status: Status::parse(&c.status),
            category: c.category.clone(),
            confidence: c.confidence.unwrap_or(0.0) as f32,
            category2,
            source: c.source.as_deref().map_or(Source::Rules, Source::parse),
            hits: c
                .hits
                .as_deref()
                .and_then(|h| serde_json::from_str(h).ok())
                .unwrap_or_default(),
            fields: serde_json::from_str(&c.fields).unwrap_or_default(),
            field_sources: serde_json::from_str::<BTreeMap<String, String>>(&c.field_sources)
                .unwrap_or_default(),
            text_source: c
                .text_source
                .as_deref()
                .map_or(TextSource::None, TextSource::parse),
            llm_model: c.llm_model.clone(),
            defs_fingerprint: c.defs_fingerprint.clone(),
            extractor_version: c.extractor_version,
            classified_at: c.classified_at.clone(),
        }
    }
}

/// In welchem Zustand ist ein Cache-Eintrag für die Datei, wie sie jetzt im Index steht?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    /// Alles gleich: nichts zu tun
    Valid,
    /// Datei unverändert, aber Kategorie-Definitionen oder LLM-Modell sind anders: Text neu
    /// lesen (OCR aus dem OCR-Cache) und neu zuordnen
    Reassign,
    /// Größe, mtime oder Extraktor-Version sind anders: neu extrahieren
    Reextract,
}

pub fn cache_state(
    size: i64,
    mtime: i64,
    cached: &CachedContent,
    fingerprint: &str,
    llm_model: Option<&str>,
) -> CacheState {
    if cached.size != size || cached.mtime != mtime || cached.extractor_version != EXTRACTOR_VERSION
    {
        return CacheState::Reextract;
    }
    if cached.defs_fingerprint != fingerprint {
        return CacheState::Reassign;
    }
    let has_text = matches!(
        cached.text_source.as_deref(),
        Some("layer" | "office" | "ocr")
    );
    if let Some(model) = llm_model {
        if cached.status == "ok" && has_text && cached.llm_model.as_deref() != Some(model) {
            return CacheState::Reassign;
        }
    }
    CacheState::Valid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> ContentRecord {
        let mut fields = Fields::new();
        fields.insert("doc.date".into(), "2026-09-30".into());
        let mut field_sources = FieldSources::new();
        field_sources.insert("doc.date".into(), "pattern".into());
        ContentRecord {
            path_key: r"C:\r\a.pdf".into(),
            size: 10,
            mtime: 20,
            status: Status::Ok,
            category: Some("rechnung".into()),
            confidence: 0.91,
            category2: Some(("mahnung".into(), 0.2)),
            source: Source::Llm,
            hits: vec!["rechnung".into(), "Muster x".into()],
            fields,
            field_sources,
            text_source: TextSource::Ocr,
            llm_model: Some("qwen2.5:7b".into()),
            defs_fingerprint: "fp".into(),
            extractor_version: EXTRACTOR_VERSION,
            classified_at: "t".into(),
        }
    }

    #[test]
    fn rundlauf_ueber_die_cache_zeile() {
        let r = record();
        let back = ContentRecord::from_cached(&r.to_cached());
        assert_eq!(back, r);
    }

    #[test]
    fn ohne_kategorie_und_unlesbar() {
        let mut r = record();
        r.category = None;
        r.category2 = None;
        r.confidence = 0.0;
        r.status = Status::Unreadable("encrypted".into());
        let row = r.to_cached();
        assert_eq!(row.status, "unreadable:encrypted");
        assert_eq!(row.confidence, None);
        assert_eq!(row.source, None);
        let back = ContentRecord::from_cached(&row);
        assert_eq!(back.status, Status::Unreadable("encrypted".into()));
        assert_eq!(back.category, None);
    }

    #[test]
    fn cache_zustaende() {
        let row = record().to_cached();
        assert_eq!(cache_state(10, 20, &row, "fp", None), CacheState::Valid);
        assert_eq!(cache_state(11, 20, &row, "fp", None), CacheState::Reextract);
        assert_eq!(cache_state(10, 21, &row, "fp", None), CacheState::Reextract);
        assert_eq!(
            cache_state(10, 20, &row, "andere", None),
            CacheState::Reassign
        );
        let mut old = row.clone();
        old.extractor_version = EXTRACTOR_VERSION + 1;
        assert_eq!(cache_state(10, 20, &old, "fp", None), CacheState::Reextract);
    }

    #[test]
    fn anderes_llm_modell_ordnet_nur_dateien_mit_text_neu_zu() {
        let row = record().to_cached();
        assert_eq!(
            cache_state(10, 20, &row, "fp", Some("qwen2.5:7b")),
            CacheState::Valid
        );
        assert_eq!(
            cache_state(10, 20, &row, "fp", Some("llama3.2:3b")),
            CacheState::Reassign
        );
        let mut no_text = row.clone();
        no_text.text_source = Some("none".into());
        no_text.llm_model = None;
        assert_eq!(
            cache_state(10, 20, &no_text, "fp", Some("x")),
            CacheState::Valid
        );
        let mut unreadable = row.clone();
        unreadable.status = "unreadable:encrypted".into();
        assert_eq!(
            cache_state(10, 20, &unreadable, "fp", Some("x")),
            CacheState::Valid
        );
    }
}
