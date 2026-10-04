//! Abschnitt „Inhalte“ des Berichts (Phase 6a): Kategorien, Quellen, nicht lesbare Dateien,
//! „Zum Prüfen“ und Fotoübersicht. Gebaut aus Index und Inhalts-Cache; es wird nie Text
//! gelesen oder gespeichert.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use serde::Serialize;

use super::TypeStat;
use crate::content::{ContentRecord, Source, Status, TextSource};
use crate::index::Index;

/// So viele Einträge der Liste „Zum Prüfen“ stehen im HTML; CSV und JSON haben alle.
pub const HTML_REVIEW_LIMIT: usize = 200;
/// Fotos nach Ort und Kamera: so viele Einträge.
pub const PHOTO_TOP: usize = 10;

#[derive(Debug, Clone, Serialize)]
pub struct CategoryStat {
    pub name: String,
    pub count: u64,
    pub size: u64,
    /// Anteil an allen klassifizierten Dateien (0.0 bis 1.0)
    pub share: f64,
    /// davon mit LLM-Kategorie
    pub llm: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SourceStats {
    pub text: u64,
    pub ocr: u64,
    pub meta: u64,
    pub llm: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewItem {
    pub path: String,
    pub category: String,
    pub confidence: f32,
    pub category2: Option<String>,
    pub confidence2: Option<f32>,
    pub hits: Vec<String>,
}

/// Eine Zeile je Datei für CSV und JSON.
#[derive(Debug, Clone, Serialize)]
pub struct ContentFile {
    pub path: String,
    pub status: String,
    pub category: Option<String>,
    pub confidence: Option<f32>,
    pub source: Option<String>,
    pub doc_date: Option<String>,
    pub doc_sender: Option<String>,
    pub doc_number: Option<String>,
    pub doc_amount: Option<String>,
    pub doc_title: Option<String>,
    pub review: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentSection {
    /// Gab es für diese Wurzel überhaupt Klassifikationsergebnisse?
    pub classified: bool,
    pub files_total: u64,
    pub files_classified: u64,
    pub min_confidence: f32,
    pub categories: Vec<CategoryStat>,
    pub uncategorized: u64,
    pub sources: SourceStats,
    pub unreadable: Vec<TypeStat>,
    pub unsupported: u64,
    pub too_large: u64,
    /// Cloud-Platzhalter: nie gelesen
    pub cloud_only: u64,
    /// Dateien ohne (aktuellen) Cache-Eintrag
    pub not_classified: u64,
    pub review_total: u64,
    /// Im HTML gekürzt auf [`HTML_REVIEW_LIMIT`]
    pub review: Vec<ReviewItem>,
    pub places: Vec<TypeStat>,
    pub cameras: Vec<TypeStat>,
    /// Alle Dateien mit Ergebnis (nur CSV/JSON; im HTML leer)
    pub files: Vec<ContentFile>,
}

impl ContentSection {
    /// Variante für die eingebettete HTML-Seite: gekürzte Liste, keine Dateizeilen.
    pub fn for_html(&self) -> Self {
        let mut s = self.clone();
        s.files.clear();
        s.review.truncate(HTML_REVIEW_LIMIT);
        s
    }
}

fn needs_review(r: &ContentRecord, min_confidence: f32) -> bool {
    r.status.is_ok() && r.category.is_some() && r.confidence < min_confidence
}

fn top(map: BTreeMap<String, u64>, n: usize) -> Vec<TypeStat> {
    let mut v: Vec<(String, u64)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.into_iter()
        .take(n)
        .map(|(key, count)| TypeStat {
            key,
            count,
            size: 0,
        })
        .collect()
}

/// Baut den Abschnitt für alle Dateien unterhalb von `root_key`.
pub fn build(index: &Index, root_key: &str, min_confidence: f32) -> Result<ContentSection> {
    let files = index.files_under(root_key)?;
    let cached: HashMap<String, ContentRecord> = index
        .content_lookup(root_key)?
        .into_iter()
        .map(|(k, v)| (k, ContentRecord::from_cached(&v)))
        .collect();

    let mut section = ContentSection {
        classified: !cached.is_empty(),
        files_total: 0,
        files_classified: 0,
        min_confidence,
        categories: Vec::new(),
        uncategorized: 0,
        sources: SourceStats::default(),
        unreadable: Vec::new(),
        unsupported: 0,
        too_large: 0,
        cloud_only: 0,
        not_classified: 0,
        review_total: 0,
        review: Vec::new(),
        places: Vec::new(),
        cameras: Vec::new(),
        files: Vec::new(),
    };
    let mut by_cat: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    let mut unreadable: BTreeMap<String, u64> = BTreeMap::new();
    let mut places: BTreeMap<String, u64> = BTreeMap::new();
    let mut cameras: BTreeMap<String, u64> = BTreeMap::new();
    let mut review: Vec<ReviewItem> = Vec::new();

    for f in files.iter().filter(|f| !f.is_link) {
        section.files_total += 1;
        if f.cloud_only {
            section.cloud_only += 1;
            continue;
        }
        let Some(r) = cached
            .get(&f.key)
            .filter(|r| r.size == f.size && r.mtime == f.mtime)
        else {
            section.not_classified += 1;
            continue;
        };
        section.files_classified += 1;
        match &r.status {
            Status::Unsupported => section.unsupported += 1,
            Status::TooLarge => section.too_large += 1,
            Status::Unreadable(why) => *unreadable.entry(why.clone()).or_default() += 1,
            Status::Ok => {}
        }
        match r.text_source {
            TextSource::Layer | TextSource::Office => section.sources.text += 1,
            TextSource::Ocr => section.sources.ocr += 1,
            TextSource::None => {
                if r.fields
                    .keys()
                    .any(|k| k.starts_with("exif.") || k.starts_with("video."))
                {
                    section.sources.meta += 1;
                }
            }
        }
        if r.source == Source::Llm || r.field_sources.values().any(|v| v == "llm") {
            section.sources.llm += 1;
        }
        match &r.category {
            Some(c) => {
                let e = by_cat.entry(c.clone()).or_default();
                e.0 += 1;
                e.1 += f.size.max(0) as u64;
                if r.source == Source::Llm {
                    e.2 += 1;
                }
            }
            None => section.uncategorized += 1,
        }
        let is_review = needs_review(r, min_confidence);
        if is_review {
            section.review_total += 1;
            review.push(ReviewItem {
                path: f.path.clone(),
                category: r.category.clone().unwrap_or_default(),
                confidence: r.confidence,
                category2: r.category2.as_ref().map(|c| c.0.clone()),
                confidence2: r.category2.as_ref().map(|c| c.1),
                hits: r.hits.clone(),
            });
        }
        if r.category.as_deref() == Some("foto") {
            if let Some(country) = r.fields.get("exif.country") {
                let label = match r.fields.get("exif.city") {
                    Some(city) => format!("{country} {city}"),
                    None => country.clone(),
                };
                *places.entry(label).or_default() += 1;
            }
            if let Some(camera) = r.fields.get("exif.camera") {
                *cameras.entry(camera.clone()).or_default() += 1;
            }
        }
        let field = |k: &str| r.fields.get(k).cloned();
        section.files.push(ContentFile {
            path: f.path.clone(),
            status: r.status.as_str(),
            category: r.category.clone(),
            confidence: r.category.as_ref().map(|_| r.confidence),
            source: r.category.as_ref().map(|_| r.source.as_str().to_string()),
            doc_date: field("doc.date"),
            doc_sender: field("doc.sender"),
            doc_number: field("doc.number"),
            doc_amount: field("doc.amount"),
            doc_title: field("doc.title"),
            review: is_review,
        });
    }

    let classified = section.files_classified.max(1) as f64;
    let mut categories: Vec<CategoryStat> = by_cat
        .into_iter()
        .map(|(name, (count, size, llm))| CategoryStat {
            name,
            count,
            size,
            share: count as f64 / classified,
            llm,
        })
        .collect();
    categories.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    section.categories = categories;
    section.unreadable = top(unreadable, usize::MAX);
    section.places = top(places, PHOTO_TOP);
    section.cameras = top(cameras, PHOTO_TOP);
    // Zum Prüfen: nach Ordner, dann Pfad
    review.sort_by_key(|r| r.path.to_lowercase());
    section.review = review;
    section.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(section)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::content::classify::fields::FieldSources;
    use crate::content::{Fields, EXTRACTOR_VERSION};
    use crate::index::FileRecord;
    use crate::paths;

    fn root_key() -> String {
        paths::dir_key(Path::new(r"C:\R"))
    }

    fn put(index: &mut Index, path: &str, cloud: bool, record: Option<ContentRecord>) {
        let p = Path::new(path);
        let key = paths::path_key(p);
        index
            .upsert_files(
                &[FileRecord {
                    dir_key: paths::dir_key(p.parent().unwrap()),
                    path: path.into(),
                    path_key: key.clone(),
                    name: p.file_name().unwrap().to_string_lossy().into_owned(),
                    size: 10,
                    mtime: 20,
                    cloud_only: cloud,
                    ..FileRecord::default()
                }],
                1,
            )
            .unwrap();
        if let Some(mut r) = record {
            r.path_key = key;
            r.size = 10;
            r.mtime = 20;
            index.content_store(&[r.to_cached()]).unwrap();
        }
    }

    fn rec(category: Option<&str>, confidence: f32) -> ContentRecord {
        ContentRecord {
            path_key: String::new(),
            size: 10,
            mtime: 20,
            status: Status::Ok,
            category: category.map(String::from),
            confidence,
            category2: Some(("mahnung".into(), 0.3)),
            source: Source::Rules,
            hits: vec!["rechnung".into()],
            fields: Fields::new(),
            field_sources: FieldSources::new(),
            text_source: TextSource::Layer,
            llm_model: None,
            defs_fingerprint: "f".into(),
            extractor_version: EXTRACTOR_VERSION,
            classified_at: "t".into(),
        }
    }

    #[test]
    fn ohne_classify_steht_nicht_klassifiziert() {
        let mut index = Index::open_in_memory().unwrap();
        put(&mut index, r"C:\R\a.pdf", false, None);
        let s = build(&index, &root_key(), 0.8).unwrap();
        assert!(!s.classified);
        assert_eq!(
            (s.files_total, s.not_classified, s.files_classified),
            (1, 1, 0)
        );
    }

    #[test]
    fn kategorien_quellen_pruefliste_und_fotos() {
        let mut index = Index::open_in_memory().unwrap();
        put(
            &mut index,
            r"C:\R\b\sicher.pdf",
            false,
            Some(rec(Some("rechnung"), 0.95)),
        );
        put(
            &mut index,
            r"C:\R\a\unsicher.pdf",
            false,
            Some(rec(Some("rechnung"), 0.5)),
        );
        let mut llm = rec(Some("vertrag"), 0.85);
        llm.source = Source::Llm;
        put(&mut index, r"C:\R\llm.pdf", false, Some(llm));
        let mut photo = rec(Some("foto"), 0.95);
        photo.text_source = TextSource::None;
        photo.fields.insert("exif.country".into(), "PT".into());
        photo.fields.insert("exif.city".into(), "Lisbon".into());
        photo
            .fields
            .insert("exif.camera".into(), "Apple iPhone 15".into());
        put(&mut index, r"C:\R\foto.jpg", false, Some(photo));
        let mut enc = rec(None, 0.0);
        enc.status = Status::Unreadable("encrypted".into());
        put(&mut index, r"C:\R\enc.pdf", false, Some(enc));
        put(&mut index, r"C:\R\cloud.pdf", true, None);
        put(&mut index, r"C:\R\neu.pdf", false, None);

        let s = build(&index, &root_key(), 0.8).unwrap();
        assert!(s.classified);
        assert_eq!((s.files_total, s.files_classified), (7, 5));
        assert_eq!((s.cloud_only, s.not_classified), (1, 1));
        assert_eq!(s.categories[0].name, "rechnung");
        assert_eq!(s.categories[0].count, 2);
        assert_eq!(
            s.categories
                .iter()
                .find(|c| c.name == "vertrag")
                .unwrap()
                .llm,
            1
        );
        assert_eq!(s.uncategorized, 1);
        assert_eq!(s.unreadable[0].key, "encrypted");
        assert_eq!((s.sources.text, s.sources.meta, s.sources.llm), (4, 1, 1));
        assert_eq!(s.review_total, 1);
        assert_eq!(s.review[0].category, "rechnung");
        assert_eq!(s.review[0].category2.as_deref(), Some("mahnung"));
        assert!(s.review[0].path.ends_with("unsicher.pdf"));
        assert_eq!(s.places[0].key, "PT Lisbon");
        assert_eq!(s.cameras[0].key, "Apple iPhone 15");
        assert_eq!(s.files.len(), 5);
        assert!(s
            .files
            .iter()
            .any(|f| f.review && f.path.ends_with("unsicher.pdf")));
    }

    #[test]
    fn veralteter_cache_eintrag_zaehlt_als_nicht_klassifiziert() {
        let mut index = Index::open_in_memory().unwrap();
        put(
            &mut index,
            r"C:\R\a.pdf",
            false,
            Some(rec(Some("rechnung"), 0.9)),
        );
        let mut stale = rec(Some("rechnung"), 0.9);
        stale.mtime = 99;
        stale.path_key = paths::path_key(Path::new(r"C:\R\a.pdf"));
        index.content_store(&[stale.to_cached()]).unwrap();
        let s = build(&index, &root_key(), 0.8).unwrap();
        assert_eq!((s.files_classified, s.not_classified), (0, 1));
    }

    #[test]
    fn html_variante_kuerzt_pruefliste_und_laesst_dateien_weg() {
        let mut index = Index::open_in_memory().unwrap();
        for i in 0..(HTML_REVIEW_LIMIT + 5) {
            put(
                &mut index,
                &format!(r"C:\R\u{i:04}.pdf"),
                false,
                Some(rec(Some("rechnung"), 0.5)),
            );
        }
        let s = build(&index, &root_key(), 0.8).unwrap();
        assert_eq!(s.review.len(), HTML_REVIEW_LIMIT + 5);
        let h = s.for_html();
        assert_eq!((h.review.len(), h.files.len()), (HTML_REVIEW_LIMIT, 0));
        assert_eq!(h.review_total, (HTML_REVIEW_LIMIT + 5) as u64);
    }
}
