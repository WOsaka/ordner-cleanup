//! Befehl `classify` (Phase 6a): liest Inhalt und Metadaten der Dateien eines gescannten
//! Ordners und legt Kategorie, Konfidenz und Felder im Index-Cache ab. Inkrementell,
//! parallel, abbrechbar, mit niedriger Prozesspriorität; ändert nie etwas im Ordner.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use globset::GlobBuilder;
use indicatif::{ProgressBar, ProgressStyle};

use super::profile;
use crate::change::rules::{ContentLookup, TextProvider};
use crate::cli::ClassifyArgs;
use crate::config::Config;
use crate::content::classify::defs::CategoryDefs;
use crate::content::classify::Classifier;
use crate::content::extract::{abandoned_workers, ExtractOpts};
use crate::content::llm::{Llm, LlmClient, LlmTasks};
use crate::content::ocr::{Ocr, OcrText, NO_LANGUAGE_HINT};
use crate::content::pipeline::{
    existing_record, process_file, LlmGate, LlmSettings, NoTextCache, OcrGate, OcrSettings,
    Outcome, PipelineCtx, Skip, TextCache, OCR_PARALLEL,
};
use crate::content::{
    cache_state, CacheState, ContentRecord, Source, Status, TextSource, OCR_VERSION,
};
use crate::index::{FileRow, Index, OcrTextEntry};
use crate::paths;
use crate::platform::dpapi;
use crate::platform::ocr::WinOcr;
use crate::platform::priority::ProcessPriorityGuard;
use crate::scan::lock::ScanLock;

/// So viele Ergebnisse werden in einer Transaktion geschrieben; ein Abbruch verliert
/// höchstens den laufenden Block.
const BLOCK: usize = 200;

/// Lädt die Kategorie-Definitionen: eigene Datei (Config oder `categories.toml` neben der
/// Config) zusätzlich zu den eingebauten. Fehlt die Datei, gelten nur die eingebauten; Fehler
/// in der Datei nennen Datei, Zeile und Kategorie.
pub(super) fn load_defs(config: &Config) -> Result<CategoryDefs> {
    let config_dir = paths::config_dir();
    let file: Option<PathBuf> = match &config.classify.categories_file {
        Some(f) => Some(PathBuf::from(crate::config::resolve_in(
            f,
            config_dir.as_deref(),
        ))),
        None => config_dir.as_ref().map(|d| d.join("categories.toml")),
    };
    let Some(file) = file else {
        return Ok(CategoryDefs::builtin());
    };
    match std::fs::read_to_string(&file) {
        Ok(text) => CategoryDefs::load(&text, &paths::display(&file)).map_err(Into::into),
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && config.classify.categories_file.is_none() =>
        {
            Ok(CategoryDefs::builtin())
        }
        Err(e) => Err(e)
            .with_context(|| format!("Kategorie-Datei {} nicht lesbar", paths::display(&file))),
    }
}

/// OCR-Text-Cache in der Index-Datenbank, DPAPI-verschlüsselt. Ein beschädigter Eintrag gilt
/// als Fehltreffer.
struct IndexTextCache<'a, 'b> {
    index: &'a Mutex<&'b mut Index>,
    languages: String,
}

impl TextCache for IndexTextCache<'_, '_> {
    fn get(&self, key: &str, size: i64, mtime: i64) -> Option<OcrText> {
        let hit = self
            .index
            .lock()
            .ok()?
            .ocr_text_get(key, size, mtime, OCR_VERSION, &self.languages)
            .ok()??;
        let plain = dpapi::unprotect(&hit.data).ok()?;
        Some(OcrText {
            text: String::from_utf8(plain).ok()?,
            pages: u32::try_from(hit.pages).unwrap_or(0),
        })
    }

    fn put(&self, key: &str, size: i64, mtime: i64, text: &OcrText) {
        let Ok(data) = dpapi::protect(text.text.as_bytes()) else {
            return;
        };
        if let Ok(mut index) = self.index.lock() {
            let _ = index.ocr_text_put(&OcrTextEntry {
                path_key: key.to_string(),
                size,
                mtime,
                ocr_version: OCR_VERSION,
                languages: self.languages.clone(),
                pages: i64::from(text.pages),
                data,
            });
        }
    }
}

/// OCR-Einrichtung: nur nutzbar, wenn ein passendes Sprachpaket installiert ist.
pub(super) struct OcrSetup {
    pub engine: Option<WinOcr>,
    /// Installierte Sprachen, die zu `ocr_languages` passen
    pub languages: Vec<String>,
    /// Schlüssel für den OCR-Text-Cache (sortierte Tags)
    pub lang_key: String,
}

impl OcrSetup {
    pub fn new(config: &Config) -> Self {
        let engine = config
            .classify
            .ocr
            .then(|| WinOcr::new(config.classify.ocr_languages.clone()));
        let languages = engine.as_ref().map(|o| o.languages()).unwrap_or_default();
        let mut sorted = languages.clone();
        sorted.sort();
        Self {
            engine,
            languages,
            lang_key: sorted.join(","),
        }
    }

    /// `None`, wenn OCR aus ist oder kein Sprachpaket passt.
    pub fn usable(&self) -> Option<&dyn Ocr> {
        match &self.engine {
            Some(o) if !self.languages.is_empty() => Some(o),
            _ => None,
        }
    }
}

/// Was ein Aufruf von [`classify_files`] braucht.
pub(super) struct ClassifyJob<'a> {
    pub root: &'a Path,
    pub config: &'a Config,
    pub classifier: &'a Classifier,
    pub force: bool,
    pub no_llm: bool,
}

/// Zähler eines Laufs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct RunStats {
    /// Dateien, die zur Auswahl gehörten
    pub considered: usize,
    pub analysed: usize,
    pub from_cache: usize,
    /// Neu zugeordnet (Kategorien oder LLM-Modell geändert), Text neu gelesen
    pub reassigned: usize,
    pub cloud_only: usize,
    pub stale: usize,
    pub transient: BTreeMap<String, usize>,
    pub ocr_runs: usize,
    pub ocr_cache_hits: usize,
    pub llm_calls: usize,
}

pub(super) struct ClassifyRun {
    pub stats: RunStats,
    /// Endgültige Datensätze (Cache oder neu) für alle Dateien, die einen haben
    pub records: HashMap<String, ContentRecord>,
    pub warnings: Vec<String>,
    pub aborted: bool,
    pub duration: Duration,
}

struct Item<'a> {
    file: &'a FileRow,
    existing: Option<ContentRecord>,
    reassign: bool,
}

fn build_llm(config: &Config, warnings: &mut Vec<String>) -> Option<LlmClient> {
    let client = match LlmClient::new(&config.llm) {
        Ok(c) => c,
        Err(e) => {
            warnings.push(format!("LLM: {e}"));
            return None;
        }
    };
    match client.check() {
        Ok(()) => Some(client),
        Err(e) => {
            warnings.push(format!(
                "LLM nicht erreichbar bzw. unbrauchbar ({}): {e}; ohne LLM weitergemacht",
                config.llm.endpoint
            ));
            None
        }
    }
}

fn llm_tasks(config: &Config) -> LlmTasks {
    LlmTasks {
        category: config.llm.has_task("category"),
        fields: config.llm.has_task("fields"),
        title: config.llm.has_task("title"),
    }
}

fn ocr_settings(config: &Config) -> Result<OcrSettings> {
    let mut globs = globset::GlobSetBuilder::new();
    for g in &config.classify.ocr_image_globs {
        globs.add(
            GlobBuilder::new(g)
                .case_insensitive(true)
                .literal_separator(true)
                .build()
                .with_context(|| format!("classify.ocr_image_globs: ungültiger Glob '{g}'"))?,
        );
    }
    Ok(OcrSettings {
        enabled: config.classify.ocr,
        max_pages: config.classify.ocr_max_pages,
        image_globs: globs.build()?,
    })
}

/// Klassifiziert `files` (Dateien einer gescannten Wurzel): gültige Cache-Einträge bleiben,
/// alles andere wird analysiert und in Blöcken in den Index geschrieben.
pub(super) fn classify_files(
    index: &mut Index,
    job: &ClassifyJob<'_>,
    files: &[&FileRow],
    cancel: &AtomicBool,
) -> Result<ClassifyRun> {
    let started = Instant::now();
    let cfg = &job.config.classify;
    let root_key = paths::dir_key(job.root);
    let mut warnings = Vec::new();
    let mut stats = RunStats {
        considered: files.len(),
        ..RunStats::default()
    };

    // LLM: einmal je Lauf prüfen
    let llm_client = (job.config.llm.enabled && !job.no_llm)
        .then(|| build_llm(job.config, &mut warnings))
        .flatten();
    let llm_model = llm_client.as_ref().map(|c| c.model().to_string());

    // OCR: nur, wenn ein passendes Sprachpaket installiert ist
    let ocr_setup = OcrSetup::new(job.config);
    let ocr_ref = ocr_setup.usable();
    let lang_key = ocr_setup.lang_key.clone();
    if !cfg.cache_ocr_text {
        let _ = index.ocr_text_clear(&root_key);
    }

    // Auswahl: was ist im Cache gültig?
    let cached = index.content_lookup(&root_key)?;
    let fingerprint = job.classifier.fingerprint();
    let mut records: HashMap<String, ContentRecord> = HashMap::new();
    let mut items: Vec<Item<'_>> = Vec::new();
    for file in files {
        if file.is_link || file.cloud_only {
            stats.cloud_only += 1;
            continue;
        }
        let entry = cached.get(&file.key);
        let state = match entry {
            Some(c) if !job.force => {
                cache_state(file.size, file.mtime, c, fingerprint, llm_model.as_deref())
            }
            _ => CacheState::Reextract,
        };
        match (state, entry) {
            (CacheState::Valid, Some(c)) => {
                records.insert(file.key.clone(), ContentRecord::from_cached(c));
                stats.from_cache += 1;
            }
            (state, entry) => items.push(Item {
                file,
                existing: entry.and_then(|c| existing_record(c, file)),
                reassign: state == CacheState::Reassign,
            }),
        }
    }

    if !items.is_empty() {
        let extra = (|| -> Result<Vec<String>> {
            let mut local_warnings = Vec::new();
            let _priority = ProcessPriorityGuard::below_normal();
            let extract_opts = ExtractOpts {
                max_file_size: cfg.max_file_size_bytes(),
                max_text_chars: cfg.max_text_chars,
                ..ExtractOpts::default()
            };
            let settings = ocr_settings(job.config)?;
            let ocr_gate = OcrGate::new(OCR_PARALLEL);
            let llm_gate = LlmGate::default();
            let index_mx = Mutex::new(&mut *index);
            let text_cache: Box<dyn TextCache + '_> = if cfg.cache_ocr_text && ocr_ref.is_some() {
                Box::new(IndexTextCache {
                    index: &index_mx,
                    languages: lang_key.clone(),
                })
            } else {
                Box::new(NoTextCache)
            };
            let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let ctx = PipelineCtx {
                root: job.root,
                classifier: job.classifier,
                extract: &extract_opts,
                ocr: ocr_ref,
                ocr_settings: &settings,
                ocr_cache: text_cache.as_ref(),
                ocr_gate: &ocr_gate,
                llm: llm_client.as_ref().map(|client| LlmSettings {
                    client,
                    tasks: llm_tasks(job.config),
                    max_input_chars: job.config.llm.max_input_chars,
                    max_confidence: job.config.llm.max_confidence,
                    gate: &llm_gate,
                }),
                min_confidence: cfg.min_confidence as f32,
                today: chrono::Local::now().date_naive(),
                now: &now,
                force: job.force,
            };
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(cfg.worker_threads())
                .build()
                .context("Thread-Pool konnte nicht erstellt werden")?;
            // indicatif zeichnet nur in einem Terminal (geplante Läufe bleiben still)
            let bar = ProgressBar::new(items.len() as u64);
            bar.set_style(
                ProgressStyle::with_template("{bar:30} {pos}/{len} {msg}")
                    .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );
            let (tx, rx) =
                crossbeam_channel::bounded::<(usize, crate::content::pipeline::Processed)>(64);
            let mut block: Vec<crate::index::CachedContent> = Vec::new();
            let mut write_error: Option<anyhow::Error> = None;

            std::thread::scope(|scope| {
                scope.spawn(|| {
                    use rayon::prelude::*;
                    pool.install(|| {
                        items.par_iter().enumerate().for_each(|(i, item)| {
                            if cancel.load(Ordering::Relaxed) {
                                return;
                            }
                            let processed = process_file(item.file, item.existing.as_ref(), &ctx);
                            let _ = tx.send((i, processed));
                        });
                    });
                    drop(tx);
                });
                for (i, processed) in rx {
                    let item = &items[i];
                    stats.ocr_runs += processed.ocr_runs as usize;
                    stats.ocr_cache_hits += processed.ocr_cache_hits as usize;
                    stats.llm_calls += processed.llm_calls as usize;
                    match processed.outcome {
                        Outcome::Record(record) => {
                            stats.analysed += 1;
                            if item.reassign {
                                stats.reassigned += 1;
                            }
                            bar.set_message(match (record.text_source, record.source) {
                                (_, Source::Llm) => "LLM",
                                (TextSource::Ocr, _) => "OCR",
                                (TextSource::Layer | TextSource::Office, _) => "Text",
                                _ => "Metadaten",
                            });
                            block.push(record.to_cached());
                            records.insert(item.file.key.clone(), *record);
                        }
                        Outcome::Skipped(Skip::CloudPlaceholder) => stats.cloud_only += 1,
                        Outcome::Skipped(Skip::Stale) => stats.stale += 1,
                        Outcome::Skipped(Skip::Transient(why)) => {
                            *stats.transient.entry(why).or_default() += 1
                        }
                    }
                    bar.inc(1);
                    if block.len() >= BLOCK && write_error.is_none() {
                        write_error = flush(&index_mx, &mut block).err();
                    }
                }
            });
            bar.finish_and_clear();
            if write_error.is_none() {
                write_error = flush(&index_mx, &mut block).err();
            }
            if let Some(e) = write_error {
                return Err(e);
            }
            if let Some(w) = llm_gate.warning() {
                local_warnings.push(w);
            }
            if abandoned_workers() > 0 {
                local_warnings.push(format!(
                    "{} PDF-Parser haben nicht geantwortet und wurden aufgegeben",
                    abandoned_workers()
                ));
            }
            if ocr_setup.engine.is_some() && ocr_setup.languages.is_empty() {
                local_warnings.push(NO_LANGUAGE_HINT.to_string());
            }
            Ok(local_warnings)
        })()?;
        warnings.extend(extra);
    }

    let aborted = cancel.load(Ordering::Relaxed);
    // Veraltete Einträge (Dateien, die es nicht mehr gibt) erst nach vollständigem Lauf.
    if !aborted && files.len() > 1 {
        let _ = index.content_prune(&root_key);
    }
    Ok(ClassifyRun {
        stats,
        records,
        warnings,
        aborted,
        duration: started.elapsed(),
    })
}

fn flush(index: &Mutex<&mut Index>, block: &mut Vec<crate::index::CachedContent>) -> Result<()> {
    if block.is_empty() {
        return Ok(());
    }
    index
        .lock()
        .map_err(|_| anyhow::anyhow!("Index-Sperre vergiftet"))?
        .content_store(block)?;
    block.clear();
    Ok(())
}

/// `ContentLookup` für `plan rules`: Cache lesen, fehlende oder veraltete Einträge über
/// `classify` nachholen (außer bei `--no-classify`).
pub(super) struct CachedLookup<'a> {
    pub root: &'a Path,
    pub config: &'a Config,
    pub classifier: &'a Classifier,
    /// `false` bei `--no-classify`
    pub classify: bool,
    pub notes: Mutex<Vec<String>>,
}

impl ContentLookup for CachedLookup<'_> {
    fn content(
        &self,
        index: &mut Index,
        files: &[&FileRow],
    ) -> std::result::Result<HashMap<String, ContentRecord>, crate::index::IndexError> {
        use crate::index::IndexError;
        let root_key = paths::dir_key(self.root);
        let cached = index.content_lookup(&root_key)?;
        let fingerprint = self.classifier.fingerprint();
        let mut records = HashMap::new();
        let mut missing: Vec<&FileRow> = Vec::new();
        for file in files {
            if file.cloud_only || file.is_link {
                continue;
            }
            match cached.get(&file.key) {
                // Das LLM-Modell zählt hier nicht: ein vorhandenes Ergebnis genügt für den Plan.
                Some(c)
                    if cache_state(file.size, file.mtime, c, fingerprint, None)
                        == CacheState::Valid =>
                {
                    records.insert(file.key.clone(), ContentRecord::from_cached(c));
                }
                _ => missing.push(file),
            }
        }
        if missing.is_empty() || !self.classify {
            if !missing.is_empty() {
                self.notes.lock().unwrap_or_else(|e| e.into_inner()).push(format!(
                    "{} Dateien sind nicht (aktuell) klassifiziert und bleiben ohne Inhaltsregeln (--no-classify).",
                    missing.len()
                ));
            }
            return Ok(records);
        }
        let _lock = ScanLock::acquire(
            &paths::scan_lock_path().map_err(|e| IndexError::Other(e.to_string()))?,
        )
        .map_err(|e| IndexError::Other(e.to_string()))?;
        eprintln!(
            "Hinweis: {} Dateien werden zuerst klassifiziert (--no-classify schaltet das ab).",
            missing.len()
        );
        let cancel = super::global_cancel_flag().map_err(|e| IndexError::Other(e.to_string()))?;
        let job = ClassifyJob {
            root: self.root,
            config: self.config,
            classifier: self.classifier,
            force: false,
            no_llm: false,
        };
        let run = classify_files(index, &job, &missing, &cancel)
            .map_err(|e| IndexError::Other(format!("{e:#}")))?;
        for w in &run.warnings {
            eprintln!("Warnung: {w}");
        }
        if run.aborted {
            return Err(IndexError::Other("Abgebrochen".into()));
        }
        records.extend(run.records);
        Ok(records)
    }
}

/// `TextSource` für `plan rules`: Textlayer und Office-Text werden live gelesen, OCR-Text kommt
/// aus dem verschlüsselten Cache (bei Fehltreffer läuft OCR erneut). Nichts wird gespeichert.
pub(super) struct LiveText<'a> {
    pub config: &'a Config,
}

impl TextProvider for LiveText<'_> {
    fn texts(
        &self,
        index: &Index,
        items: &[(&FileRow, Option<&ContentRecord>)],
    ) -> Vec<Option<String>> {
        use rayon::prelude::*;
        let cfg = &self.config.classify;
        let opts = ExtractOpts {
            max_file_size: cfg.max_file_size_bytes(),
            max_text_chars: cfg.max_text_chars,
            ..ExtractOpts::default()
        };
        let ocr_setup = OcrSetup::new(self.config);
        // OCR-Text zuerst, sequentiell (braucht den Index)
        let mut results: Vec<Option<String>> = vec![None; items.len()];
        let mut live: Vec<usize> = Vec::new();
        let mut ocr_misses: Vec<usize> = Vec::new();
        for (i, (file, record)) in items.iter().enumerate() {
            let is_ocr = record.is_some_and(|r| r.text_source == TextSource::Ocr);
            if !is_ocr {
                live.push(i);
                continue;
            }
            let hit = index
                .ocr_text_get(
                    &file.key,
                    file.size,
                    file.mtime,
                    OCR_VERSION,
                    &ocr_setup.lang_key,
                )
                .ok()
                .flatten()
                .and_then(|h| dpapi::unprotect(&h.data).ok())
                .and_then(|b| String::from_utf8(b).ok());
            match hit {
                Some(text) => results[i] = Some(text),
                None => ocr_misses.push(i),
            }
        }
        let extracted: Vec<(usize, Option<String>)> = live
            .par_iter()
            .map(|&i| {
                let (file, _) = items[i];
                let ext = Path::new(&file.name)
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let out = crate::content::extract::extract(
                    Path::new(&file.path),
                    &ext,
                    file.size as u64,
                    &opts,
                );
                (i, out.text)
            })
            .collect();
        for (i, text) in extracted {
            results[i] = text;
        }
        if let Some(ocr) = ocr_setup.usable() {
            for i in ocr_misses {
                let (file, _) = items[i];
                let ext = Path::new(&file.name)
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let path = Path::new(&file.path);
                let read = if ext == "pdf" {
                    ocr.pdf(path, cfg.ocr_max_pages)
                } else {
                    ocr.image(path)
                };
                results[i] = read.ok().map(|t| t.text);
            }
        }
        results
            .into_iter()
            .map(|t| {
                t.map(|t| crate::content::classify::text::clean(&t, cfg.max_text_chars))
                    .filter(|t| !t.is_empty())
            })
            .collect()
    }
}

/// Zusammenfassung über alle Dateien der Auswahl.
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) struct Summary {
    pub text: usize,
    pub ocr: usize,
    pub meta: usize,
    pub llm: usize,
    pub unsupported: usize,
    pub too_large: usize,
    pub review: usize,
    pub unreadable: BTreeMap<String, usize>,
    /// Kategorie → Dateien, absteigend sortiert nach Anzahl
    pub categories: Vec<(String, usize)>,
    pub uncategorized: usize,
}

fn unreadable_label(why: &str) -> &str {
    match why {
        "encrypted" => "verschlüsselt",
        "corrupt" => "beschädigt",
        other => other,
    }
}

impl Summary {
    pub fn from_records<'a>(
        records: impl Iterator<Item = &'a ContentRecord>,
        min_confidence: f32,
    ) -> Self {
        let mut s = Self::default();
        let mut by_cat: BTreeMap<String, usize> = BTreeMap::new();
        for r in records {
            match &r.status {
                Status::Unsupported => s.unsupported += 1,
                Status::TooLarge => s.too_large += 1,
                Status::Unreadable(why) => *s.unreadable.entry(why.clone()).or_default() += 1,
                Status::Ok => {}
            }
            match r.text_source {
                TextSource::Layer | TextSource::Office => s.text += 1,
                TextSource::Ocr => s.ocr += 1,
                TextSource::None => {
                    if r.fields
                        .keys()
                        .any(|k| k.starts_with("exif.") || k.starts_with("video."))
                    {
                        s.meta += 1;
                    }
                }
            }
            if r.source == Source::Llm || r.field_sources.values().any(|v| v == "llm") {
                s.llm += 1;
            }
            match &r.category {
                Some(c) => {
                    *by_cat.entry(c.clone()).or_default() += 1;
                    if r.status.is_ok() && r.confidence < min_confidence {
                        s.review += 1;
                    }
                }
                None => s.uncategorized += 1,
            }
        }
        let mut categories: Vec<_> = by_cat.into_iter().collect();
        categories.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        s.categories = categories;
        s
    }
}

fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    out
}

fn duration_text(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Text der Zusammenfassung (ohne Wurzelzeile).
pub(super) fn render_summary(root: &str, run: &ClassifyRun, summary: &Summary) -> String {
    use std::fmt::Write;
    let stats = &run.stats;
    let mut out = String::new();
    let _ = writeln!(out, "Klassifiziert: {root}");
    let mut skipped = Vec::new();
    if stats.cloud_only > 0 {
        skipped.push(format!("Cloud-only {}", stats.cloud_only));
    }
    if summary.too_large > 0 {
        skipped.push(format!("zu groß {}", summary.too_large));
    }
    if summary.unsupported > 0 {
        skipped.push(format!("nicht unterstützt {}", summary.unsupported));
    }
    let skipped_total = stats.cloud_only + summary.too_large + summary.unsupported;
    let skipped_text = if skipped.is_empty() {
        String::new()
    } else {
        format!(
            ", {} übersprungen ({})",
            grouped(skipped_total),
            skipped.join(", ")
        )
    };
    let _ = writeln!(
        out,
        "  {} Dateien geprüft: {} analysiert, {} aus dem Cache{skipped_text}",
        grouped(stats.considered),
        grouped(stats.analysed),
        grouped(stats.from_cache)
    );
    if stats.reassigned > 0 {
        let ocr = if stats.ocr_cache_hits > 0 {
            " (OCR-Text aus dem Cache)"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  Kategorien geändert: {} Dateien neu zugeordnet{ocr}",
            grouped(stats.reassigned)
        );
    }
    let _ = writeln!(
        out,
        "  Quellen: Text {}, OCR {}, Metadaten {}, LLM {}",
        summary.text, summary.ocr, summary.meta, summary.llm
    );
    if !summary.categories.is_empty() || summary.uncategorized > 0 {
        let mut parts: Vec<String> = summary
            .categories
            .iter()
            .take(8)
            .map(|(c, n)| format!("{c} {}", grouped(*n)))
            .collect();
        if summary.categories.len() > 8 {
            parts.push("…".into());
        }
        parts.push(format!("ohne Kategorie {}", grouped(summary.uncategorized)));
        let _ = writeln!(out, "  {}", parts.join(" · "));
    }
    let unreadable_total: usize = summary.unreadable.values().sum();
    let mut tail = vec![format!("Zum Prüfen: {} (Liste im Bericht)", summary.review)];
    if unreadable_total > 0 {
        let reasons: Vec<String> = summary
            .unreadable
            .iter()
            .map(|(why, n)| format!("{} {n}", unreadable_label(why)))
            .collect();
        tail.push(format!(
            "Nicht lesbar: {unreadable_total} ({})",
            reasons.join(", ")
        ));
    }
    let _ = writeln!(out, "  {}", tail.join("   "));
    if stats.stale > 0 {
        let _ = writeln!(
            out,
            "  Seit dem Scan geändert: {} (bitte neu scannen)",
            stats.stale
        );
    }
    for (why, n) in &stats.transient {
        let text = match why.as_str() {
            "locked" => "gesperrt",
            "vanished" => "verschwunden",
            "no-ocr-language" => "ohne OCR-Sprachpaket",
            "timeout" => "Zeitüberschreitung",
            "ocr-failed" => "OCR fehlgeschlagen",
            other => other,
        };
        let _ = writeln!(out, "  Später erneut versuchen ({text}): {n}");
    }
    for w in &run.warnings {
        let _ = writeln!(out, "  Warnung: {w}");
    }
    let _ = writeln!(out, "  Dauer {}", duration_text(run.duration));
    out
}

/// Dateien der Auswahl: ohne Links, gefiltert nach `--ext` und `--only`.
fn select_files<'a>(
    all: &'a [FileRow],
    root: &Path,
    exts: &[String],
    only: Option<&str>,
) -> Result<Vec<&'a FileRow>> {
    let exts: Vec<String> = exts
        .iter()
        .map(|e| e.trim().trim_start_matches('.').to_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    let matcher = only
        .map(|g| {
            GlobBuilder::new(&g.replace('\\', "/"))
                .case_insensitive(true)
                .literal_separator(true)
                .build()
                .map(|g| g.compile_matcher())
                .with_context(|| format!("--only: ungültiger Glob '{g}'"))
        })
        .transpose()?;
    Ok(all
        .iter()
        .filter(|f| {
            if !exts.is_empty() {
                let ext = Path::new(&f.name)
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if !exts.contains(&ext) {
                    return false;
                }
            }
            if let Some(m) = &matcher {
                let rel = paths::relative_to(root, Path::new(&f.path))
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|| f.name.clone());
                return m.is_match(&rel);
            }
            true
        })
        .collect())
}

/// `classify` innerhalb von `run`: Sperre und Abbruch-Flag liegen beim Aufrufer. Liefert die
/// Zahl der Dateien „zum Prüfen“.
pub(super) fn classify_for_run(index: &mut Index, root: &Path, config: &Config) -> Result<usize> {
    let classifier = Classifier::new(load_defs(config)?);
    let root_key = paths::dir_key(root);
    let all = index.files_under(&root_key)?;
    let files: Vec<&FileRow> = all.iter().collect();
    let cancel = super::global_cancel_flag()?;
    let job = ClassifyJob {
        root,
        config,
        classifier: &classifier,
        force: false,
        no_llm: false,
    };
    let run = classify_files(index, &job, &files, &cancel)?;
    for w in &run.warnings {
        eprintln!("Warnung: {w}");
    }
    anyhow::ensure!(!run.aborted, "classify abgebrochen");
    let summary =
        Summary::from_records(run.records.values(), config.classify.min_confidence as f32);
    Ok(summary.review)
}

pub(super) fn classify_command(args: &ClassifyArgs) -> Result<i32> {
    let target = profile::target(args.path.as_deref(), args.profile.as_deref())?;
    let root = super::resolve_root(&target.root, target.force())?;
    let config = &target.config;
    let root_key = paths::dir_key(&root);

    let index_file = paths::index_path()?;
    if !index_file.exists() {
        bail!(
            "Kein Index für {}. Zuerst scannen: ordner-cleanup scan {}",
            paths::display(&root),
            paths::display(&root)
        );
    }
    let mut index = Index::open(&index_file)?;
    if !index.roots()?.iter().any(|r| r.path_key == root_key) {
        bail!(
            "{} ist noch nicht gescannt. Zuerst: ordner-cleanup scan {}",
            paths::display(&root),
            paths::display(&root)
        );
    }
    let _lock = ScanLock::acquire(&paths::scan_lock_path()?)?;

    if args.clear {
        let n = index.content_clear(&root_key)?;
        println!(
            "Inhalts- und OCR-Text-Cache von {} geleert ({n} Einträge).",
            paths::display(&root)
        );
        return Ok(0);
    }

    let defs = load_defs(config)?;
    let classifier = Classifier::new(defs);
    let all = index.files_under(&root_key)?;
    let files = select_files(&all, &root, &args.ext, args.only.as_deref())?;
    let cancel = super::global_cancel_flag()?;

    let job = ClassifyJob {
        root: &root,
        config,
        classifier: &classifier,
        force: args.force,
        no_llm: args.no_llm,
    };
    let run = classify_files(&mut index, &job, &files, &cancel)?;
    let summary =
        Summary::from_records(run.records.values(), config.classify.min_confidence as f32);
    print!("{}", render_summary(&paths::display(&root), &run, &summary));
    if run.aborted {
        eprintln!("Abgebrochen; bis dahin Analysiertes bleibt im Cache.");
        return Ok(1);
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::classify::fields::FieldSources;
    use crate::content::{Fields, EXTRACTOR_VERSION};

    fn record(category: Option<&str>, conf: f32) -> ContentRecord {
        ContentRecord {
            path_key: "k".into(),
            size: 1,
            mtime: 1,
            status: Status::Ok,
            category: category.map(String::from),
            confidence: conf,
            category2: None,
            source: Source::Rules,
            hits: vec![],
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
    fn gruppierte_zahlen_und_dauer() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1204), "1.204");
        assert_eq!(grouped(1_234_567), "1.234.567");
        assert_eq!(duration_text(Duration::from_secs(42)), "0:42");
        assert_eq!(duration_text(Duration::from_secs(125)), "2:05");
    }

    #[test]
    fn zusammenfassung_zaehlt_kategorien_quellen_und_pruefliste() {
        let mut ocr = record(Some("vertrag"), 0.9);
        ocr.text_source = TextSource::Ocr;
        let mut photo = record(Some("foto"), 0.95);
        photo.text_source = TextSource::None;
        photo.fields.insert("exif.camera".into(), "X".into());
        let mut llm = record(Some("rechnung"), 0.7);
        llm.source = Source::Llm;
        let mut enc = record(None, 0.0);
        enc.status = Status::Unreadable("encrypted".into());
        let mut big = record(None, 0.0);
        big.status = Status::TooLarge;
        let records = [
            record(Some("rechnung"), 0.95),
            record(Some("rechnung"), 0.5),
            ocr,
            photo,
            llm,
            enc,
            big,
            record(None, 0.0),
        ];
        let s = Summary::from_records(records.iter(), 0.8);
        assert_eq!(s.categories[0], ("rechnung".to_string(), 3));
        assert_eq!((s.text, s.ocr, s.meta, s.llm), (6, 1, 1, 1));
        assert_eq!(s.review, 2, "rechnung 0.5 und LLM 0.7");
        assert_eq!(s.uncategorized, 3);
        assert_eq!(s.unreadable["encrypted"], 1);
        assert_eq!(s.too_large, 1);
    }

    #[test]
    fn ausgabe_entspricht_dem_beispiel() {
        let run = ClassifyRun {
            stats: RunStats {
                considered: 1204,
                analysed: 87,
                from_cache: 1093,
                reassigned: 312,
                cloud_only: 20,
                ocr_cache_hits: 3,
                ..RunStats::default()
            },
            records: HashMap::new(),
            warnings: vec!["LLM nicht erreichbar (http://127.0.0.1:11434)".into()],
            aborted: false,
            duration: Duration::from_secs(42),
        };
        let summary = Summary {
            text: 61,
            ocr: 9,
            meta: 17,
            llm: 12,
            too_large: 4,
            review: 19,
            unreadable: [("encrypted".to_string(), 2), ("corrupt".to_string(), 1)].into(),
            categories: vec![("rechnung".into(), 143), ("foto".into(), 610)],
            uncategorized: 211,
            ..Summary::default()
        };
        let text = render_summary(r"C:\Users\Oskar\Downloads", &run, &summary);
        assert!(text.contains("Klassifiziert: C:\\Users\\Oskar\\Downloads"));
        assert!(text.contains("1.204 Dateien geprüft: 87 analysiert, 1.093 aus dem Cache, 24 übersprungen (Cloud-only 20, zu groß 4)"), "{text}");
        assert!(text
            .contains("Kategorien geändert: 312 Dateien neu zugeordnet (OCR-Text aus dem Cache)"));
        assert!(text.contains("Quellen: Text 61, OCR 9, Metadaten 17, LLM 12"));
        assert!(text.contains("rechnung 143 · foto 610 · ohne Kategorie 211"));
        assert!(text.contains("Zum Prüfen: 19 (Liste im Bericht)   Nicht lesbar: 3 (beschädigt 1, verschlüsselt 2)"), "{text}");
        assert!(text.contains("Warnung: LLM nicht erreichbar"));
        assert!(text.contains("Dauer 0:42"));
    }

    #[test]
    fn auswahl_nach_endung_und_glob() {
        let root = Path::new(r"C:\R");
        let file = |p: &str| FileRow {
            path: p.into(),
            key: paths::path_key(Path::new(p)),
            dir_key: String::new(),
            name: Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            size: 1,
            mtime: 1,
            ctime: None,
            first_seen: None,
            attrs: 0x20,
            cloud_only: false,
            is_link: false,
        };
        let all = vec![
            file(r"C:\R\a.pdf"),
            file(r"C:\R\Scans\b.PDF"),
            file(r"C:\R\Scans\c.jpg"),
            file(r"C:\R\d.txt"),
        ];
        let names = |sel: Vec<&FileRow>| sel.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(select_files(&all, root, &[], None).unwrap()).len(), 4);
        assert_eq!(
            names(select_files(&all, root, &["pdf".into()], None).unwrap()),
            vec!["a.pdf", "b.PDF"]
        );
        assert_eq!(
            names(select_files(&all, root, &[".jpg".into(), "txt".into()], None).unwrap()),
            vec!["c.jpg", "d.txt"]
        );
        assert_eq!(
            names(select_files(&all, root, &[], Some("Scans/**")).unwrap()),
            vec!["b.PDF", "c.jpg"]
        );
        assert_eq!(
            names(select_files(&all, root, &["pdf".into()], Some("Scans/*")).unwrap()),
            vec!["b.PDF"]
        );
        assert!(select_files(&all, root, &[], Some("[")).is_err());
    }
}
