//! Pipeline je Datei: Extraktion → OCR (mit verschlüsseltem Cache) → Normalisierung →
//! Klassifikation → Felder → optional LLM. Das Ergebnis ist ein [`ContentRecord`]. OCR, LLM und
//! OCR-Text-Cache werden hereingereicht, damit Tests ohne Sprachpaket und ohne Netzwerk laufen.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use chrono::NaiveDate;
use globset::GlobSet;

use super::classify::fields::{clean_value, extract_fields, FieldSources};
use super::classify::{text, Classification, Classifier, ClassifyInput};
use super::extract::{extract, ExtractOpts, Extracted};
use super::geo;
use super::llm::{validate_answer, Llm, LlmError, LlmRequest, LlmTasks};
use super::ocr::{Ocr, OcrError, OcrText};
use super::{ContentRecord, Fields, Source, Status, TextSource, EXTRACTOR_VERSION};
use crate::index::{CachedContent, FileRow};
use crate::paths;
use crate::scan::source::filetime_to_ticks;

/// Textlayer eines PDFs gilt als brauchbar ab so vielen Wörtern je gelesener Seite im Schnitt …
pub const PDF_MIN_WORDS_PER_PAGE: usize = 20;
/// … oder ab so vielen Wörtern insgesamt.
pub const PDF_MIN_WORDS_TOTAL: usize = 50;
/// Bilder unter dieser Größe werden nie per OCR gelesen (kein Scan, nur Grafik).
pub const OCR_MIN_IMAGE_BYTES: u64 = 20 * 1024;
/// So viele OCR-Aufgaben laufen gleichzeitig.
pub const OCR_PARALLEL: usize = 2;
/// Nach so vielen LLM-Fehlern in Folge (nicht erreichbar, Zeitüberschreitung) bleibt das LLM aus.
pub const LLM_MAX_FAILURES: usize = 3;
/// Mehr Seiten rechnet der PDF-Textextraktor nicht (siehe `extract::pdf::MAX_PAGES`).
const PDF_TEXT_PAGES: usize = super::extract::pdf::MAX_PAGES as usize;

/// Cache für OCR-Text (im Betrieb DPAPI-verschlüsselt in der Index-Datenbank).
pub trait TextCache: Sync {
    fn get(&self, key: &str, size: i64, mtime: i64) -> Option<OcrText>;
    fn put(&self, key: &str, size: i64, mtime: i64, text: &OcrText);
}

/// Kein Cache (`cache_ocr_text = false`).
pub struct NoTextCache;

impl TextCache for NoTextCache {
    fn get(&self, _: &str, _: i64, _: i64) -> Option<OcrText> {
        None
    }
    fn put(&self, _: &str, _: i64, _: i64, _: &OcrText) {}
}

/// Begrenzt gleichzeitige OCR-Aufgaben.
pub struct OcrGate {
    free: Mutex<usize>,
    cv: Condvar,
}

impl OcrGate {
    pub fn new(parallel: usize) -> Self {
        Self {
            free: Mutex::new(parallel.max(1)),
            cv: Condvar::new(),
        }
    }

    fn run<T>(&self, f: impl FnOnce() -> T) -> T {
        let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
        while *free == 0 {
            free = self.cv.wait(free).unwrap_or_else(|e| e.into_inner());
        }
        *free -= 1;
        drop(free);
        let result = f();
        *self.free.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.cv.notify_one();
        result
    }
}

/// Zustand des LLM für einen Lauf: höchstens eine Anfrage gleichzeitig, Abschaltung nach
/// wiederholten Fehlern, eine Warnung.
#[derive(Default)]
pub struct LlmGate {
    failures: AtomicUsize,
    disabled: AtomicBool,
    warning: Mutex<Option<String>>,
    serial: Mutex<()>,
}

impl LlmGate {
    pub fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Relaxed)
    }

    pub fn warning(&self) -> Option<String> {
        self.warning.lock().ok().and_then(|w| w.clone())
    }

    /// Schaltet das LLM für den Rest des Laufs ab und merkt sich die Warnung.
    pub fn disable(&self, why: String) {
        self.disabled.store(true, Ordering::Relaxed);
        if let Ok(mut w) = self.warning.lock() {
            w.get_or_insert(why);
        }
    }

    fn note_failure(&self, error: &LlmError) {
        let hard = matches!(error, LlmError::Unreachable(_) | LlmError::ModelMissing(_));
        let n = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        if hard || n >= LLM_MAX_FAILURES {
            self.disable(format!("LLM: {error}; ohne LLM weitergemacht"));
        }
    }

    fn note_success(&self) {
        self.failures.store(0, Ordering::Relaxed);
    }
}

pub struct OcrSettings {
    pub enabled: bool,
    pub max_pages: usize,
    /// Bilder, die auch mit Kamera-EXIF per OCR gelesen werden (Globs auf den Pfad relativ zur
    /// Wurzel)
    pub image_globs: GlobSet,
}

pub struct LlmSettings<'a> {
    pub client: &'a dyn Llm,
    /// Aufgaben aus der Config
    pub tasks: LlmTasks,
    pub max_input_chars: usize,
    pub max_confidence: f64,
    pub gate: &'a LlmGate,
}

pub struct PipelineCtx<'a> {
    pub root: &'a Path,
    pub classifier: &'a Classifier,
    pub extract: &'a ExtractOpts,
    pub ocr: Option<&'a dyn Ocr>,
    pub ocr_settings: &'a OcrSettings,
    pub ocr_cache: &'a dyn TextCache,
    pub ocr_gate: &'a OcrGate,
    pub llm: Option<LlmSettings<'a>>,
    pub min_confidence: f32,
    pub today: NaiveDate,
    pub now: &'a str,
    /// `--force`: LLM-Ergebnisse nicht aus dem Cache übernehmen
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Cloud-Platzhalter oder Link: nie geöffnet
    CloudPlaceholder,
    /// Nicht gecacht, der nächste Lauf versucht es erneut (gesperrt, kein OCR-Sprachpaket …)
    Transient(String),
    /// Datei hat sich seit dem Scan oder während der Analyse geändert
    Stale,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Record(Box<ContentRecord>),
    Skipped(Skip),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Processed {
    pub outcome: Outcome,
    /// OCR wurde tatsächlich ausgeführt
    pub ocr_runs: u32,
    /// OCR-Text kam aus dem verschlüsselten Cache
    pub ocr_cache_hits: u32,
    /// Es gab eine LLM-Anfrage
    pub llm_calls: u32,
}

impl Processed {
    fn skipped(skip: Skip) -> Self {
        Self {
            outcome: Outcome::Skipped(skip),
            ocr_runs: 0,
            ocr_cache_hits: 0,
            llm_calls: 0,
        }
    }
}

fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Größe und mtime (Ticks) der Datei jetzt.
fn current_stat(path: &Path) -> Result<(i64, i64), Skip> {
    use std::os::windows::fs::MetadataExt;
    match std::fs::metadata(paths::extended(path)) {
        Ok(m) => Ok((m.len() as i64, filetime_to_ticks(m.last_write_time()))),
        Err(e) => match super::extract::status_for_io(&e) {
            Status::Unreadable(why) => Err(Skip::Transient(why)),
            _ => Err(Skip::Transient("io".into())),
        },
    }
}

fn pdf_layer_usable(text: Option<&str>, pages: Option<u32>) -> bool {
    let words = text.map_or(0, text::word_count);
    let pages_read = pages.map_or(1, |p| (p as usize).clamp(1, PDF_TEXT_PAGES));
    words >= PDF_MIN_WORDS_TOTAL || words / pages_read >= PDF_MIN_WORDS_PER_PAGE
}

enum OcrResult {
    Text { text: OcrText, from_cache: bool },
    Skip(Skip),
    Unreadable(String),
}

fn map_ocr_error(e: OcrError) -> OcrResult {
    match e {
        OcrError::NoLanguage => OcrResult::Skip(Skip::Transient("no-ocr-language".into())),
        OcrError::Encrypted => OcrResult::Unreadable("encrypted".into()),
        OcrError::Corrupt => OcrResult::Unreadable("corrupt".into()),
        OcrError::Unavailable(why) => {
            OcrResult::Skip(Skip::Transient(if why == "cloud-placeholder" {
                "cloud-placeholder".to_string()
            } else if why == "timeout" {
                "timeout".to_string()
            } else {
                "locked".to_string()
            }))
        }
        OcrError::Failed(_) => OcrResult::Skip(Skip::Transient("ocr-failed".into())),
    }
}

fn run_ocr(ctx: &PipelineCtx<'_>, file: &FileRow, pdf: bool) -> OcrResult {
    if let Some(text) = ctx.ocr_cache.get(&file.key, file.size, file.mtime) {
        return OcrResult::Text {
            text,
            from_cache: true,
        };
    }
    let Some(ocr) = ctx.ocr else {
        return OcrResult::Skip(Skip::Transient("no-ocr-language".into()));
    };
    let path = Path::new(&file.path);
    let result = ctx.ocr_gate.run(|| {
        if pdf {
            ocr.pdf(path, ctx.ocr_settings.max_pages)
        } else {
            ocr.image(path)
        }
    });
    match result {
        Ok(mut text) => {
            super::extract::truncate_chars(&mut text.text, ctx.extract.max_text_chars);
            ctx.ocr_cache.put(&file.key, file.size, file.mtime, &text);
            OcrResult::Text {
                text,
                from_cache: false,
            }
        }
        Err(e) => map_ocr_error(e),
    }
}

/// Felder aus Metadaten, die ohne Text auskommen: Kamera, GPS, Ort, Videodatum.
fn meta_fields(meta: &Fields, fields: &mut Fields, sources: &mut FieldSources) {
    let mut put = |name: &str, value: String| {
        if !value.is_empty() {
            fields.insert(name.to_string(), value);
            sources.insert(name.to_string(), "meta".to_string());
        }
    };
    for key in [
        "exif.camera",
        "exif.lat",
        "exif.lon",
        "exif.has_gps",
        "video.date",
    ] {
        if let Some(v) = meta.get(key) {
            put(key, clean_value(v));
        }
    }
    let coords = meta
        .get("exif.lat")
        .zip(meta.get("exif.lon"))
        .and_then(|(lat, lon)| Some((lat.parse::<f64>().ok()?, lon.parse::<f64>().ok()?)));
    if let Some((lat, lon)) = coords {
        if let Some(place) = geo::nearest(lat, lon) {
            if let Some(city) = place.city {
                put("exif.city", clean_value(&city));
            }
            put("exif.country", place.country);
        }
    }
}

/// Zwischenstand einer Datei: Kategorie, Quelle und Felder, die das LLM ergänzen darf.
struct Draft {
    classification: Classification,
    source: Source,
    fields: Fields,
    sources: FieldSources,
}

/// Ergebnis der LLM-Stufe.
struct LlmOutcome {
    /// Modell, das für diese Datei „befragt“ wurde (auch ohne nötige Anfrage); `None` bei
    /// Netzwerkfehlern, damit der nächste Lauf es erneut versucht.
    model: Option<String>,
    calls: u32,
}

fn apply_llm(
    ctx: &PipelineCtx<'_>,
    file: &FileRow,
    cleaned: &str,
    existing: Option<&ContentRecord>,
    draft: &mut Draft,
) -> LlmOutcome {
    let Draft {
        classification,
        source,
        fields,
        sources,
    } = draft;
    let none = LlmOutcome {
        model: None,
        calls: 0,
    };
    let Some(llm) = &ctx.llm else { return none };
    if llm.gate.is_disabled() || cleaned.trim().is_empty() {
        return none;
    }
    let model = llm.client.model().to_string();
    let min = ctx.min_confidence;
    let weak = classification.category.is_none() || classification.confidence < min;

    // Übernahme aus dem Cache: gleiches Modell und unverändert → nicht erneut fragen
    let same_model = !ctx.force
        && existing
            .is_some_and(|e| e.llm_model.as_deref() == Some(model.as_str()) && e.status.is_ok());
    let mut tasks = llm.tasks;
    if same_model {
        let e = existing.expect("same_model setzt existing voraus");
        for (name, value) in &e.fields {
            if e.field_sources.get(name).map(String::as_str) == Some("llm") {
                fields.insert(name.clone(), value.clone());
                sources.insert(name.clone(), "llm".into());
            }
        }
        tasks.fields = false;
        tasks.title = false;
        if e.source == Source::Llm
            && e.defs_fingerprint == ctx.classifier.fingerprint()
            && e.category.is_some()
            && weak
        {
            classification.second = classification
                .category
                .clone()
                .map(|c| (c, classification.confidence));
            classification.category = e.category.clone();
            classification.confidence = e.confidence;
            classification.hits = e.hits.clone();
            *source = Source::Llm;
            tasks.category = false;
        }
    }
    tasks.category = tasks.category && weak && !(same_model && *source == Source::Llm);
    tasks.fields =
        tasks.fields && (!fields.contains_key("doc.date") || !fields.contains_key("doc.sender"));
    if !tasks.any() {
        return LlmOutcome {
            model: Some(model),
            calls: 0,
        };
    }

    let mut text_part = cleaned.to_string();
    super::extract::truncate_chars(&mut text_part, llm.max_input_chars);
    let request = LlmRequest {
        file_name: file.name.clone(),
        text: text_part,
        categories: ctx
            .classifier
            .defs()
            .categories
            .iter()
            .filter(|c| c.kind == super::classify::defs::Kind::Text)
            .map(|c| (c.name.clone(), c.description.clone()))
            .collect(),
        tasks,
    };
    let answer = {
        let _one_at_a_time = llm.gate.serial.lock().unwrap_or_else(|e| e.into_inner());
        if llm.gate.is_disabled() {
            return none;
        }
        llm.client.ask(&request)
    };
    match answer {
        Err(e @ (LlmError::Unreachable(_) | LlmError::Timeout | LlmError::ModelMissing(_))) => {
            llm.gate.note_failure(&e);
            LlmOutcome {
                model: None,
                calls: 1,
            }
        }
        Err(e) => {
            // Unbrauchbare Antwort: diese Datei bleibt beim Regelergebnis und gilt als befragt.
            llm.gate.note_failure(&e);
            LlmOutcome {
                model: Some(model),
                calls: 1,
            }
        }
        Ok(answer) => {
            llm.gate.note_success();
            let known = ctx
                .classifier
                .defs()
                .categories
                .iter()
                .filter(|c| c.kind == super::classify::defs::Kind::Text)
                .map(|c| c.name.clone())
                .collect();
            let v = validate_answer(&answer, tasks, &known, llm.max_confidence, ctx.today);
            if let Some((name, confidence)) = v.category {
                if weak {
                    classification.second = classification
                        .category
                        .clone()
                        .map(|c| (c, classification.confidence));
                    classification.category = Some(name);
                    classification.confidence = confidence;
                    classification.hits = vec!["LLM".to_string()];
                    *source = Source::Llm;
                }
            }
            for (name, value) in v.fields {
                // Muster und Metadaten gehen vor, außer beim Titel (LLM vor Metadaten-Titel)
                if name == "doc.title"
                    || sources.get(&name).is_none()
                    || !fields.contains_key(&name)
                {
                    fields.insert(name.clone(), value);
                    sources.insert(name, "llm".into());
                }
            }
            LlmOutcome {
                model: Some(model),
                calls: 1,
            }
        }
    }
}

/// Verarbeitet eine Datei. `existing` ist der bisherige Cache-Eintrag (für die Übernahme von
/// LLM-Ergebnissen), nur sinnvoll, wenn die Datei unverändert ist.
pub fn process_file(
    file: &FileRow,
    existing: Option<&ContentRecord>,
    ctx: &PipelineCtx<'_>,
) -> Processed {
    if file.cloud_only || file.is_link {
        return Processed::skipped(Skip::CloudPlaceholder);
    }
    let path = Path::new(&file.path);
    match current_stat(path) {
        Ok((size, mtime)) if size == file.size && mtime == file.mtime => {}
        Ok(_) => return Processed::skipped(Skip::Stale),
        Err(skip) => return Processed::skipped(skip),
    }
    let ext = ext_of(&file.name);
    let mut extracted: Extracted = extract(path, &ext, file.size as u64, ctx.extract);
    match &extracted.status {
        Status::Unreadable(_) if extracted.status.is_transient() => {
            let Status::Unreadable(why) = &extracted.status else {
                unreachable!()
            };
            return Processed::skipped(if why == "cloud-placeholder" {
                Skip::CloudPlaceholder
            } else {
                Skip::Transient(why.clone())
            });
        }
        _ => {}
    }

    let mut ocr_runs = 0u32;
    let mut ocr_cache_hits = 0u32;
    let rel = paths::relative_to(ctx.root, path)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| file.name.clone());

    // OCR: gescannte PDFs und Bilder, die wie Dokumente aussehen
    let needs_ocr = ctx.ocr_settings.enabled
        && extracted.status.is_ok()
        && match ext.as_str() {
            "pdf" => !pdf_layer_usable(extracted.text.as_deref(), extracted.pages),
            e if super::extract::is_image(e) => {
                let camera = extracted.meta.contains_key("exif.camera");
                let globbed = ctx.ocr_settings.image_globs.is_match(&rel);
                let screenshot = ctx.classifier.is_screenshot_name(&file.name);
                (file.size as u64) >= OCR_MIN_IMAGE_BYTES && (globbed || (!camera && !screenshot))
            }
            _ => false,
        };
    if needs_ocr {
        match run_ocr(ctx, file, ext == "pdf") {
            OcrResult::Text { text, from_cache } => {
                if from_cache {
                    ocr_cache_hits += 1;
                } else {
                    ocr_runs += 1;
                }
                let layer_words = extracted.text.as_deref().map_or(0, text::word_count);
                if text::word_count(&text.text) > layer_words {
                    extracted.text = Some(text.text).filter(|t| !t.trim().is_empty());
                    extracted.text_source = if extracted.text.is_some() {
                        TextSource::Ocr
                    } else {
                        TextSource::None
                    };
                }
            }
            OcrResult::Skip(skip) => {
                return Processed {
                    outcome: Outcome::Skipped(skip),
                    ocr_runs,
                    ocr_cache_hits,
                    llm_calls: 0,
                }
            }
            OcrResult::Unreadable(why) => extracted.status = Status::Unreadable(why),
        }
    }

    // Hat sich die Datei während der Analyse geändert?
    match current_stat(path) {
        Ok((size, mtime)) if size == file.size && mtime == file.mtime => {}
        Ok(_) => return Processed::skipped(Skip::Stale),
        Err(skip) => return Processed::skipped(skip),
    }

    let cleaned = extracted
        .text
        .as_deref()
        .map(|t| text::clean(t, ctx.extract.max_text_chars));
    let cleaned = cleaned.filter(|t| !t.is_empty());
    let classification = ctx.classifier.classify(&ClassifyInput {
        name: &file.name,
        ext: &ext,
        text: cleaned.as_deref(),
        meta: &extracted.meta,
    });
    let (mut fields, mut sources) = extract_fields(
        cleaned.as_deref(),
        &extracted.meta,
        &ctx.classifier.defs().senders,
        &file.name,
        ctx.today,
    );
    meta_fields(&extracted.meta, &mut fields, &mut sources);

    let mut draft = Draft {
        classification,
        source: Source::Rules,
        fields,
        sources,
    };
    let llm = apply_llm(
        ctx,
        file,
        cleaned.as_deref().unwrap_or(""),
        existing,
        &mut draft,
    );
    let Draft {
        classification,
        source,
        fields,
        sources,
    } = draft;

    let record = ContentRecord {
        path_key: file.key.clone(),
        size: file.size,
        mtime: file.mtime,
        status: extracted.status,
        category: classification.category,
        confidence: classification.confidence,
        category2: classification.second,
        source,
        hits: classification.hits,
        fields,
        field_sources: sources,
        text_source: if cleaned.is_some() {
            extracted.text_source
        } else {
            TextSource::None
        },
        llm_model: llm.model,
        defs_fingerprint: ctx.classifier.fingerprint().to_string(),
        extractor_version: EXTRACTOR_VERSION,
        classified_at: ctx.now.to_string(),
    };
    Processed {
        outcome: Outcome::Record(Box::new(record)),
        ocr_runs,
        ocr_cache_hits,
        llm_calls: llm.calls,
    }
}

/// Der Cache-Eintrag als Datensatz, falls er zur Datei passt (für die LLM-Übernahme).
pub fn existing_record(cached: &CachedContent, file: &FileRow) -> Option<ContentRecord> {
    (cached.size == file.size
        && cached.mtime == file.mtime
        && cached.extractor_version == EXTRACTOR_VERSION)
        .then(|| ContentRecord::from_cached(cached))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::AtomicUsize;

    use globset::{Glob, GlobSetBuilder};

    use super::*;
    use crate::content::classify::defs::CategoryDefs;
    use crate::content::extract::image::testing::{jpeg_with_camera, png_header};
    use crate::content::extract::office::testing::{docx, ole_container};
    use crate::content::extract::pdf::testing::pdf_with_pages;
    use crate::content::extract::video::testing::{mac_seconds, mp4};
    use crate::content::llm::fake::FakeLlm;
    use crate::content::llm::LlmAnswer;
    use crate::content::ocr::fake::FakeOcr;

    #[derive(Default)]
    struct MemCache {
        map: Mutex<HashMap<String, OcrText>>,
        gets: AtomicUsize,
    }

    impl TextCache for MemCache {
        fn get(&self, key: &str, _: i64, _: i64) -> Option<OcrText> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            self.map.lock().unwrap().get(key).cloned()
        }
        fn put(&self, key: &str, _: i64, _: i64, text: &OcrText) {
            self.map.lock().unwrap().insert(key.into(), text.clone());
        }
    }

    /// Brief ohne Kategorie-Treffer, aber mit brauchbarem Textlayer (über 50 Wörter).
    const LETTER: &str = "Brief an Herrn Muster wegen der Sache vom letzten Montag Wir bedanken uns herzlich fuer das nette Gespraech und melden uns in den kommenden Wochen mit weiteren Informationen zum geplanten Vorhaben im Frueh jahr dieses Jahres Mit freundlichen Gruessen aus dem Buero der Geschaeftsleitung und allen Kolleginnen und Kollegen";

    const INVOICE: &str = "Rechnung Rechnungsnummer: RE-2026-0042 Rechnungsdatum: 30.09.2026 Zahlbar bis 30.10.2026 Zahlungsziel 14 Tage Gesamtbetrag: 119,00 EUR Vielen Dank fuer Ihren Auftrag und Ihr Vertrauen in unsere Firma";

    struct Env {
        dir: tempfile::TempDir,
        classifier: Classifier,
        opts: ExtractOpts,
        settings: OcrSettings,
        cache: MemCache,
        gate: OcrGate,
    }

    impl Env {
        fn new() -> Self {
            let mut globs = GlobSetBuilder::new();
            globs.add(Glob::new("**/Scans/**").unwrap());
            Self {
                dir: tempfile::tempdir().unwrap(),
                classifier: Classifier::new(CategoryDefs::builtin()),
                opts: ExtractOpts::default(),
                settings: OcrSettings {
                    enabled: true,
                    max_pages: 5,
                    image_globs: globs.build().unwrap(),
                },
                cache: MemCache::default(),
                gate: OcrGate::new(2),
            }
        }

        fn write(&self, rel: &str, data: &[u8]) -> FileRow {
            let path = self.dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, data).unwrap();
            self.row(&path)
        }

        fn row(&self, path: &Path) -> FileRow {
            use std::os::windows::fs::MetadataExt;
            let meta = std::fs::metadata(path).unwrap();
            FileRow {
                path: path.to_string_lossy().into_owned(),
                key: paths::path_key(path),
                dir_key: paths::dir_key(path.parent().unwrap()),
                name: path.file_name().unwrap().to_string_lossy().into_owned(),
                size: meta.len() as i64,
                mtime: filetime_to_ticks(meta.last_write_time()),
                ctime: None,
                first_seen: None,
                attrs: 0x20,
                cloud_only: false,
                is_link: false,
            }
        }

        fn ctx<'a>(
            &'a self,
            ocr: Option<&'a dyn Ocr>,
            llm: Option<LlmSettings<'a>>,
        ) -> PipelineCtx<'a> {
            PipelineCtx {
                root: self.dir.path(),
                classifier: &self.classifier,
                extract: &self.opts,
                ocr,
                ocr_settings: &self.settings,
                ocr_cache: &self.cache,
                ocr_gate: &self.gate,
                llm,
                min_confidence: 0.8,
                today: NaiveDate::from_ymd_opt(2026, 10, 4).unwrap(),
                now: "2026-10-04T10:00:00Z",
                force: false,
            }
        }
    }

    fn record(p: &Processed) -> &ContentRecord {
        match &p.outcome {
            Outcome::Record(r) => r,
            other => panic!("kein Datensatz: {other:?}"),
        }
    }

    fn llm_settings<'a>(client: &'a dyn Llm, gate: &'a LlmGate) -> LlmSettings<'a> {
        LlmSettings {
            client,
            tasks: LlmTasks {
                category: true,
                fields: true,
                title: true,
            },
            max_input_chars: 6000,
            max_confidence: 0.85,
            gate,
        }
    }

    #[test]
    fn pdf_mit_textlayer_braucht_kein_ocr() {
        let env = Env::new();
        let pdf = pdf_with_pages(&[INVOICE], Some("Rechnung Mai"), Some("D:20260930"));
        let file = env.write("a.pdf", &pdf);
        let ocr = FakeOcr::with("a.pdf", "unbenutzt");
        let p = process_file(&file, None, &env.ctx(Some(&ocr), None));
        let r = record(&p);
        assert_eq!(r.category.as_deref(), Some("rechnung"));
        assert!(r.confidence >= 0.8, "{}", r.confidence);
        assert_eq!(r.text_source, TextSource::Layer);
        assert_eq!(r.fields["doc.number"], "RE-2026-0042");
        assert_eq!(r.fields["doc.date"], "2026-09-30");
        assert_eq!(r.fields["doc.amount"], "119,00");
        assert_eq!(r.fields["doc.title"], "Rechnung Mai");
        assert_eq!(r.source, Source::Rules);
        assert_eq!((p.ocr_runs, ocr.calls()), (0, 0));
        assert_eq!(r.defs_fingerprint, env.classifier.fingerprint());
        assert_eq!(r.size, file.size);
    }

    #[test]
    fn gescanntes_pdf_wird_per_ocr_gelesen_und_der_text_gecacht() {
        let env = Env::new();
        let file = env.write("scan.pdf", &pdf_with_pages(&[""], None, None));
        let ocr = FakeOcr::with("scan.pdf", INVOICE);
        let p = process_file(&file, None, &env.ctx(Some(&ocr), None));
        let r = record(&p);
        assert_eq!(r.category.as_deref(), Some("rechnung"));
        assert_eq!(r.text_source, TextSource::Ocr);
        assert_eq!((p.ocr_runs, p.ocr_cache_hits, ocr.calls()), (1, 0, 1));

        // zweiter Durchlauf (z. B. nach geänderten Kategorien): Text kommt aus dem Cache
        let p2 = process_file(&file, None, &env.ctx(Some(&ocr), None));
        assert_eq!(record(&p2).category.as_deref(), Some("rechnung"));
        assert_eq!((p2.ocr_runs, p2.ocr_cache_hits, ocr.calls()), (0, 1, 1));
    }

    #[test]
    fn duenner_textlayer_loest_ocr_aus_und_laengerer_text_gewinnt() {
        let env = Env::new();
        let file = env.write("d.pdf", &pdf_with_pages(&["Seite 1 von 1"], None, None));
        let ocr = FakeOcr::with("d.pdf", INVOICE);
        let p = process_file(&file, None, &env.ctx(Some(&ocr), None));
        assert_eq!(record(&p).text_source, TextSource::Ocr);
        // OCR liefert weniger als der Layer: der Layer bleibt
        let file2 = env.write(
            "e.pdf",
            &pdf_with_pages(&["Seite 1 von 1 Hinweis"], None, None),
        );
        let short = FakeOcr::with("e.pdf", "x");
        let p2 = process_file(&file2, None, &env.ctx(Some(&short), None));
        assert_eq!(record(&p2).text_source, TextSource::Layer);
    }

    #[test]
    fn ocr_aus_bleibt_beim_duennen_layer() {
        let mut env = Env::new();
        env.settings.enabled = false;
        let file = env.write("scan.pdf", &pdf_with_pages(&[""], None, None));
        let ocr = FakeOcr::with("scan.pdf", INVOICE);
        let p = process_file(&file, None, &env.ctx(Some(&ocr), None));
        let r = record(&p);
        assert_eq!(
            (r.category.clone(), r.text_source),
            (None, TextSource::None)
        );
        assert_eq!(ocr.calls(), 0);
    }

    #[test]
    fn fehlendes_sprachpaket_ist_transient_und_wird_nicht_gecacht() {
        let env = Env::new();
        let file = env.write("scan.pdf", &pdf_with_pages(&[""], None, None));
        let ocr = FakeOcr {
            no_language: true,
            ..Default::default()
        };
        let p = process_file(&file, None, &env.ctx(Some(&ocr), None));
        assert_eq!(
            p.outcome,
            Outcome::Skipped(Skip::Transient("no-ocr-language".into()))
        );
        let none = process_file(&file, None, &env.ctx(None, None));
        assert_eq!(
            none.outcome,
            Outcome::Skipped(Skip::Transient("no-ocr-language".into()))
        );
    }

    #[test]
    fn foto_mit_kamera_wird_nicht_per_ocr_gelesen_und_bekommt_ort() {
        let env = Env::new();
        let mut data = jpeg_with_camera("Apple", "iPhone 15", Some((38.7223, -9.1393)));
        data.resize(60_000, 0);
        let file = env.write("Urlaub/IMG_4711.jpg", &data);
        let ocr = FakeOcr::with("IMG_4711.jpg", INVOICE);
        let p = process_file(&file, None, &env.ctx(Some(&ocr), None));
        let r = record(&p);
        assert_eq!(r.category.as_deref(), Some("foto"));
        assert_eq!(r.fields["exif.camera"], "Apple iPhone 15");
        assert_eq!(r.fields["exif.country"], "PT");
        assert_eq!(r.fields["exif.city"], "Lisbon");
        assert_eq!(r.fields["exif.has_gps"], "true");
        assert_eq!(r.field_sources["exif.city"], "meta");
        assert_eq!(ocr.calls(), 0);
    }

    #[test]
    fn foto_im_scans_ordner_wird_trotz_kamera_per_ocr_gelesen() {
        let env = Env::new();
        let mut data = jpeg_with_camera("Apple", "iPhone 15", None);
        data.resize(60_000, 0);
        let file = env.write("Scans/IMG_1.jpg", &data);
        let ocr = FakeOcr::with("IMG_1.jpg", INVOICE);
        let p = process_file(&file, None, &env.ctx(Some(&ocr), None));
        assert_eq!(
            record(&p).category.as_deref(),
            Some("rechnung"),
            "genug Wörter: Dokument gewinnt"
        );
        assert_eq!(ocr.calls(), 1);
        let file2 = env.write("Scans/IMG_2.jpg", &data);
        let short = FakeOcr::with("IMG_2.jpg", "Rechnung");
        let p2 = process_file(&file2, None, &env.ctx(Some(&short), None));
        assert_eq!(
            record(&p2).category.as_deref(),
            Some("foto"),
            "zu wenig Text: es bleibt ein Foto"
        );
    }

    #[test]
    fn bild_ohne_exif_wird_gelesen_aber_kleine_und_screenshots_nicht() {
        let env = Env::new();
        let mut png = png_header(1234, 777);
        png.resize(50_000, 0);
        let file = env.write("brief.png", &png);
        let ocr = FakeOcr::with("brief.png", INVOICE);
        let p = process_file(&file, None, &env.ctx(Some(&ocr), None));
        assert_eq!(record(&p).category.as_deref(), Some("rechnung"));
        assert_eq!(ocr.calls(), 1);

        let tiny = env.write("icon.png", &png_header(32, 32));
        let ocr2 = FakeOcr::with("icon.png", INVOICE);
        process_file(&tiny, None, &env.ctx(Some(&ocr2), None));
        assert_eq!(ocr2.calls(), 0, "unter 20 KB kein OCR");

        let mut shot = png_header(1920, 1080);
        shot.resize(80_000, 0);
        let shot_file = env.write("Screenshot 2026-09-01.png", &shot);
        let ocr3 = FakeOcr::with("Screenshot 2026-09-01.png", INVOICE);
        let p3 = process_file(&shot_file, None, &env.ctx(Some(&ocr3), None));
        assert_eq!(record(&p3).category.as_deref(), Some("screenshot"));
        assert_eq!(ocr3.calls(), 0);
    }

    #[test]
    fn office_text_und_verschluesselte_datei() {
        let env = Env::new();
        let data = docx(&[INVOICE], "Rechnung", "Max", "2026-09-30T10:00:00Z");
        let file = env.write("a.docx", &data);
        let p = process_file(&file, None, &env.ctx(None, None));
        let r = record(&p);
        assert_eq!(
            (r.category.as_deref(), r.text_source),
            (Some("rechnung"), TextSource::Office)
        );
        assert_eq!(r.fields["doc.author"], "Max");

        let enc = env.write("b.docx", &ole_container());
        let p = process_file(&enc, None, &env.ctx(None, None));
        let r = record(&p);
        assert_eq!(r.status, Status::Unreadable("encrypted".into()));
        assert_eq!(r.category, None);
    }

    #[test]
    fn video_datum_und_unterstuetzte_typen() {
        let env = Env::new();
        let file = env.write("v.mp4", &mp4(mac_seconds(2025, 7, 14, 12), false, true));
        let p = process_file(&file, None, &env.ctx(None, None));
        assert!(record(&p).fields.contains_key("video.date"));
        let txt = env.write("n.txt", b"Rechnung");
        let p = process_file(&txt, None, &env.ctx(None, None));
        assert_eq!(record(&p).status, Status::Unsupported);
    }

    #[test]
    fn cloud_platzhalter_und_links_werden_nie_gelesen() {
        let env = Env::new();
        let mut file = env.write("a.pdf", b"x");
        std::fs::remove_file(&file.path).unwrap();
        file.cloud_only = true;
        let p = process_file(&file, None, &env.ctx(None, None));
        assert_eq!(p.outcome, Outcome::Skipped(Skip::CloudPlaceholder));
        file.cloud_only = false;
        file.is_link = true;
        assert_eq!(
            process_file(&file, None, &env.ctx(None, None)).outcome,
            Outcome::Skipped(Skip::CloudPlaceholder)
        );
    }

    #[test]
    fn geaenderte_oder_verschwundene_datei() {
        let env = Env::new();
        let mut file = env.write("a.pdf", &pdf_with_pages(&[INVOICE], None, None));
        file.mtime += 1;
        assert_eq!(
            process_file(&file, None, &env.ctx(None, None)).outcome,
            Outcome::Skipped(Skip::Stale)
        );
        let file = env.write("b.pdf", &pdf_with_pages(&[INVOICE], None, None));
        std::fs::remove_file(&file.path).unwrap();
        assert_eq!(
            process_file(&file, None, &env.ctx(None, None)).outcome,
            Outcome::Skipped(Skip::Transient("vanished".into()))
        );
    }

    #[test]
    fn zu_grosse_datei_wird_nicht_gelesen() {
        let mut env = Env::new();
        env.opts.max_file_size = 10;
        let file = env.write("a.pdf", &pdf_with_pages(&[INVOICE], None, None));
        let p = process_file(&file, None, &env.ctx(None, None));
        let r = record(&p);
        assert_eq!(r.status, Status::TooLarge);
        assert_eq!(r.text_source, TextSource::None);
    }

    #[test]
    fn llm_ergaenzt_kategorie_felder_und_titel_nur_wenn_noetig() {
        let env = Env::new();
        let file = env.write("x.pdf", &pdf_with_pages(&[LETTER], None, None));
        let gate = LlmGate::default();
        let llm = FakeLlm::new(Ok(LlmAnswer {
            category: Some("vertrag".into()),
            confidence: Some(0.99),
            date: Some("2026-08-01".into()),
            sender: Some("Muster GmbH".into()),
            amount: None,
            number: Some("V-1".into()),
            title: Some("Vertragsbrief".into()),
        }));
        let p = process_file(&file, None, &env.ctx(None, Some(llm_settings(&llm, &gate))));
        let r = record(&p);
        assert_eq!(r.category.as_deref(), Some("vertrag"));
        assert_eq!(r.confidence, 0.85, "gedeckelt");
        assert_eq!(r.source, Source::Llm);
        assert_eq!(r.fields["doc.sender"], "Muster GmbH");
        assert_eq!(r.field_sources["doc.sender"], "llm");
        assert_eq!(r.fields["doc.title"], "Vertragsbrief");
        assert_eq!(r.llm_model.as_deref(), Some("fake:1b"));
        assert_eq!(p.llm_calls, 1);
        let calls = llm.calls.lock().unwrap();
        assert!(calls[0].tasks.category && calls[0].tasks.fields && calls[0].tasks.title);
        assert!(calls[0].categories.iter().all(|(n, _)| n != "foto"));
    }

    #[test]
    fn llm_ueberstimmt_keine_sichere_regelkategorie_aber_liefert_titel() {
        let env = Env::new();
        let file = env.write("a.pdf", &pdf_with_pages(&[INVOICE], None, None));
        let gate = LlmGate::default();
        let llm = FakeLlm::new(Ok(LlmAnswer {
            category: Some("vertrag".into()),
            confidence: Some(0.9),
            title: Some("Rechnung Musterfirma".into()),
            date: Some("2020-01-01".into()),
            ..Default::default()
        }));
        let p = process_file(&file, None, &env.ctx(None, Some(llm_settings(&llm, &gate))));
        let r = record(&p);
        assert_eq!(
            r.category.as_deref(),
            Some("rechnung"),
            "Regeln sicher: kein LLM-Kategorie"
        );
        assert_eq!(r.source, Source::Rules);
        assert_eq!(r.fields["doc.title"], "Rechnung Musterfirma");
        assert_eq!(r.fields["doc.date"], "2026-09-30", "Muster gehen vor");
        let calls = llm.calls.lock().unwrap();
        assert!(
            !calls[0].tasks.category && calls[0].tasks.fields && calls[0].tasks.title,
            "Absender fehlt: Felder werden erfragt"
        );
    }

    #[test]
    fn llm_ergebnis_wird_uebernommen_statt_neu_angefragt() {
        let env = Env::new();
        let file = env.write("x.pdf", &pdf_with_pages(&[LETTER], None, None));
        let gate = LlmGate::default();
        let llm = FakeLlm::new(Ok(LlmAnswer {
            category: Some("vertrag".into()),
            confidence: Some(0.7),
            sender: Some("Muster GmbH".into()),
            title: Some("Vertragsbrief".into()),
            ..Default::default()
        }));
        let first = process_file(&file, None, &env.ctx(None, Some(llm_settings(&llm, &gate))));
        let existing = record(&first).clone();
        let second = process_file(
            &file,
            Some(&existing),
            &env.ctx(None, Some(llm_settings(&llm, &gate))),
        );
        assert_eq!(llm.call_count(), 1, "zweiter Durchlauf fragt nicht erneut");
        let r = record(&second);
        assert_eq!(r.category.as_deref(), Some("vertrag"));
        assert_eq!(r.source, Source::Llm);
        assert_eq!(r.fields["doc.title"], "Vertragsbrief");
        assert_eq!(r.llm_model.as_deref(), Some("fake:1b"));

        // --force fragt neu
        let mut ctx = env.ctx(None, Some(llm_settings(&llm, &gate)));
        ctx.force = true;
        process_file(&file, Some(&existing), &ctx);
        assert_eq!(llm.call_count(), 2);
    }

    #[test]
    fn geaenderte_kategorien_behalten_llm_felder_aber_fragen_die_kategorie_neu() {
        let env = Env::new();
        let file = env.write("x.pdf", &pdf_with_pages(&[LETTER], None, None));
        let gate = LlmGate::default();
        let llm = FakeLlm::new(Ok(LlmAnswer {
            category: Some("vertrag".into()),
            confidence: Some(0.7),
            title: Some("Titel A".into()),
            ..Default::default()
        }));
        let first = process_file(&file, None, &env.ctx(None, Some(llm_settings(&llm, &gate))));
        let mut existing = record(&first).clone();
        existing.defs_fingerprint = "anderer-fingerabdruck".into();
        process_file(
            &file,
            Some(&existing),
            &env.ctx(None, Some(llm_settings(&llm, &gate))),
        );
        assert_eq!(llm.call_count(), 2);
        let calls = llm.calls.lock().unwrap();
        assert!(calls[1].tasks.category && !calls[1].tasks.title && !calls[1].tasks.fields);
    }

    #[test]
    fn unbrauchbare_llm_antworten_lassen_das_regelergebnis_stehen() {
        let env = Env::new();
        let file = env.write("x.pdf", &pdf_with_pages(&[LETTER], None, None));
        let gate = LlmGate::default();
        let llm = FakeLlm::new(Ok(LlmAnswer {
            category: Some("geheimdienst".into()),
            date: Some("gestern".into()),
            ..Default::default()
        }));
        let p = process_file(&file, None, &env.ctx(None, Some(llm_settings(&llm, &gate))));
        let r = record(&p);
        assert_eq!(r.category, None);
        assert!(!r.fields.contains_key("doc.date"));

        let bad = FakeLlm::new(Err(LlmError::BadResponse("x".into())));
        let p = process_file(&file, None, &env.ctx(None, Some(llm_settings(&bad, &gate))));
        assert_eq!(
            record(&p).llm_model.as_deref(),
            Some("fake:1b"),
            "befragt, kein Dauerversuch"
        );
        assert!(!gate.is_disabled());
    }

    #[test]
    fn nicht_erreichbares_llm_schaltet_ab_und_wird_nicht_vermerkt() {
        let env = Env::new();
        let file = env.write("x.pdf", &pdf_with_pages(&[LETTER], None, None));
        let gate = LlmGate::default();
        let llm = FakeLlm::new(Err(LlmError::Unreachable("refused".into())));
        let p = process_file(&file, None, &env.ctx(None, Some(llm_settings(&llm, &gate))));
        assert_eq!(record(&p).llm_model, None);
        assert!(gate.is_disabled());
        assert!(gate.warning().unwrap().contains("refused"));
        process_file(&file, None, &env.ctx(None, Some(llm_settings(&llm, &gate))));
        assert_eq!(
            llm.call_count(),
            1,
            "nach der Abschaltung keine weiteren Anfragen"
        );
    }

    #[test]
    fn wiederholte_zeitueberschreitungen_schalten_ab() {
        let gate = LlmGate::default();
        for _ in 0..LLM_MAX_FAILURES - 1 {
            gate.note_failure(&LlmError::Timeout);
            assert!(!gate.is_disabled());
        }
        gate.note_failure(&LlmError::Timeout);
        assert!(gate.is_disabled());
        let gate = LlmGate::default();
        gate.note_failure(&LlmError::Timeout);
        gate.note_success();
        gate.note_failure(&LlmError::Timeout);
        gate.note_failure(&LlmError::Timeout);
        assert!(!gate.is_disabled(), "Erfolg setzt den Zähler zurück");
    }

    #[test]
    fn dateien_ohne_text_gehen_nie_zum_llm() {
        let env = Env::new();
        let mut data = jpeg_with_camera("Apple", "iPhone", None);
        data.resize(60_000, 0);
        let file = env.write("IMG_1.jpg", &data);
        let gate = LlmGate::default();
        let llm = FakeLlm::new(Ok(LlmAnswer::default()));
        let p = process_file(&file, None, &env.ctx(None, Some(llm_settings(&llm, &gate))));
        assert_eq!(llm.call_count(), 0);
        assert_eq!(record(&p).llm_model, None);
    }

    #[test]
    fn text_wird_auf_max_text_chars_begrenzt() {
        let mut env = Env::new();
        env.opts.max_text_chars = 1000;
        let text = format!("{} {}", "fuell ".repeat(300), INVOICE);
        let file = env.write("a.pdf", &pdf_with_pages(&[&text], None, None));
        let p = process_file(&file, None, &env.ctx(None, None));
        assert_eq!(
            record(&p).category,
            None,
            "der Rechnungstext liegt hinter der Grenze"
        );
    }

    #[test]
    fn gleiche_eingabe_gleiches_ergebnis() {
        let env = Env::new();
        let file = env.write("a.pdf", &pdf_with_pages(&[INVOICE], None, None));
        let a = process_file(&file, None, &env.ctx(None, None));
        let b = process_file(&file, None, &env.ctx(None, None));
        assert_eq!(a, b);
    }
}
