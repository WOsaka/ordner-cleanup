//! Windows-OCR (`Windows.Media.Ocr`) für Bilder und gescannte PDFs (`Windows.Data.Pdf`).
//! Die Dateien werden mit den eigenen Schutzprüfungen gelesen (nie Cloud-only, nie Links) und
//! als Bytes an die WinRT-Klassen gegeben; es gibt keinen Dateizugriff durch WinRT.
//! Jeder Thread, der OCR aufruft, initialisiert die WinRT-Umgebung als MTA.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows::core::{Interface, HSTRING};
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Foundation::IClosable;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
    ExifOrientationMode, SoftwareBitmap,
};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, IRandomAccessStream, InMemoryRandomAccessStream};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows::UI::Color;

use crate::content::ocr::{Ocr, OcrError, OcrText};
use crate::scan::exif::{open_local, ExifError};

/// Größere Dateien werden nicht per OCR gelesen.
const MAX_BYTES: u64 = 200 * 1024 * 1024;
/// Rendering-Skalierung für PDF-Seiten (relativ zur Seitengröße in Punkten, 72 dpi → 144 dpi).
const PDF_SCALE: f32 = 2.0;
/// Liefert eine Sprache weniger Zeichen als das, wird die nächste versucht.
const MIN_CHARS_FOR_LANGUAGE: usize = 20;

/// Hält die WinRT-Initialisierung eines Threads und gibt sie beim Beenden des Threads frei.
struct WinRtThread;

impl WinRtThread {
    fn init() -> Self {
        // SAFETY: Initialisierung der COM/WinRT-Umgebung des aktuellen Threads. Ein bereits
        // anders initialisierter Thread ist kein Fehler (der Aufruf wird dann nicht gepaart).
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        Self
    }
}

impl Drop for WinRtThread {
    fn drop(&mut self) {
        // SAFETY: gehört zum `RoInitialize` aus `init` auf demselben Thread.
        unsafe { RoUninitialize() };
    }
}

thread_local! {
    static WINRT: WinRtThread = WinRtThread::init();
}

/// Initialisiert WinRT für diesen Thread (MTA), einmal je Thread.
fn ensure_winrt() {
    WINRT.with(|_| {});
}

/// Gibt WinRT-Ressourcen (Seiten, Bitmaps, Ströme) ausdrücklich frei. Ohne das stürzt der
/// Prozess beim Beenden gelegentlich ab, weil `Windows.Data.Pdf` interne Threads hält.
fn close<T: Interface>(object: &T) {
    if let Ok(closable) = object.cast::<IClosable>() {
        let _ = closable.Close();
    }
}

fn failed(e: windows::core::Error) -> OcrError {
    OcrError::Failed(e.message())
}

/// Passt der Sprach-Tag (`de-DE`) zur gewünschten Angabe (`de` oder `de-DE`)?
pub fn tag_matches(tag: &str, wanted: &str) -> bool {
    let (tag, wanted) = (tag.to_lowercase(), wanted.trim().to_lowercase());
    tag == wanted || tag.split('-').next() == Some(wanted.as_str())
}

/// Windows-OCR mit den konfigurierten Sprachen (`de`, `en` oder volle Tags).
pub struct WinOcr {
    wanted: Vec<String>,
    /// Programm, das PDF-Seiten in einem eigenen Prozess liest (siehe `worker_main`).
    worker: Option<PathBuf>,
}

/// Zeitlimit für einen PDF-Worker.
const WORKER_TIMEOUT: Duration = Duration::from_secs(180);

/// Sucht das Programm, das den Unterbefehl `ocr-worker` kennt: `ordner-cleanup.exe` neben dem
/// laufenden Programm (auch für `ordner-cleanup-bg.exe` und Tests), sonst das laufende selbst.
pub fn default_worker() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [
        dir.join("ordner-cleanup.exe"),
        dir.parent()?.join("ordner-cleanup.exe"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .or(Some(exe))
}

impl WinOcr {
    pub fn new(wanted: Vec<String>) -> Self {
        Self {
            wanted,
            worker: default_worker(),
        }
    }

    pub fn with_worker(mut self, worker: PathBuf) -> Self {
        self.worker = Some(worker);
        self
    }

    /// Alle installierten OCR-Sprachen (Tags).
    pub fn available_languages() -> Vec<String> {
        ensure_winrt();
        let Ok(list) = OcrEngine::AvailableRecognizerLanguages() else {
            return Vec::new();
        };
        list.into_iter()
            .filter_map(|l| l.LanguageTag().ok())
            .map(|t| t.to_string())
            .collect()
    }

    fn usable_tags(&self) -> Vec<String> {
        let available = Self::available_languages();
        let mut out: Vec<String> = Vec::new();
        for want in &self.wanted {
            for tag in &available {
                if tag_matches(tag, want) && !out.contains(tag) {
                    out.push(tag.clone());
                }
            }
        }
        out
    }

    fn engines(&self) -> Result<Vec<OcrEngine>, OcrError> {
        let mut engines = Vec::new();
        for tag in self.usable_tags() {
            let language =
                Language::CreateLanguage(&HSTRING::from(tag.as_str())).map_err(failed)?;
            if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&language) {
                engines.push(engine);
            }
        }
        if engines.is_empty() {
            Err(OcrError::NoLanguage)
        } else {
            Ok(engines)
        }
    }
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, OcrError> {
    let reader = open_local(path, MAX_BYTES).map_err(|e| match e {
        ExifError::NotLocal => OcrError::Unavailable("cloud-placeholder".into()),
        ExifError::Io(io) => OcrError::Unavailable(io.to_string()),
    })?;
    let mut bytes = Vec::new();
    std::io::BufReader::new(reader)
        .read_to_end(&mut bytes)
        .map_err(|e| OcrError::Unavailable(e.to_string()))?;
    Ok(bytes)
}

fn stream_of(bytes: &[u8]) -> Result<InMemoryRandomAccessStream, OcrError> {
    let stream = InMemoryRandomAccessStream::new().map_err(failed)?;
    let writer = DataWriter::CreateDataWriter(&stream.GetOutputStreamAt(0).map_err(failed)?)
        .map_err(failed)?;
    writer.WriteBytes(bytes).map_err(failed)?;
    writer
        .StoreAsync()
        .map_err(failed)?
        .join()
        .map_err(failed)?;
    writer
        .FlushAsync()
        .map_err(failed)?
        .join()
        .map_err(failed)?;
    writer.DetachStream().map_err(failed)?;
    stream.Seek(0).map_err(failed)?;
    Ok(stream)
}

/// Dekodiert einen Bildstrom, verkleinert auf die Maximalgröße der Engine.
fn bitmap_of(stream: &IRandomAccessStream) -> Result<SoftwareBitmap, OcrError> {
    let decoder = BitmapDecoder::CreateAsync(stream)
        .and_then(|op| op.join())
        .map_err(|_| OcrError::Corrupt)?;
    let max = OcrEngine::MaxImageDimension().map_err(failed)?;
    let (w, h) = (
        decoder.PixelWidth().map_err(failed)?,
        decoder.PixelHeight().map_err(failed)?,
    );
    if w == 0 || h == 0 {
        return Err(OcrError::Corrupt);
    }
    let transform = BitmapTransform::new().map_err(failed)?;
    if w.max(h) > max {
        let scale = f64::from(max) / f64::from(w.max(h));
        transform
            .SetScaledWidth(((f64::from(w) * scale) as u32).max(1))
            .map_err(failed)?;
        transform
            .SetScaledHeight(((f64::from(h) * scale) as u32).max(1))
            .map_err(failed)?;
    }
    decoder
        .GetSoftwareBitmapTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )
        .and_then(|op| op.join())
        .map_err(|_| OcrError::Corrupt)
}

/// Erkennt mit der ersten Sprache; bei sehr wenig Text wird die nächste versucht und der
/// längere Text genommen.
fn recognize(engines: &[OcrEngine], bitmap: &SoftwareBitmap) -> Result<String, OcrError> {
    let mut best = String::new();
    for engine in engines {
        let result = engine
            .RecognizeAsync(bitmap)
            .and_then(|op| op.join())
            .map_err(failed)?;
        let text = result.Text().map_err(failed)?.to_string();
        if text.trim().chars().count() > best.trim().chars().count() {
            best = text;
        }
        if best.trim().chars().count() >= MIN_CHARS_FOR_LANGUAGE {
            break;
        }
    }
    Ok(best)
}

impl Ocr for WinOcr {
    fn languages(&self) -> Vec<String> {
        self.usable_tags()
    }

    fn image(&self, path: &Path) -> Result<OcrText, OcrError> {
        ensure_winrt();
        let engines = self.engines()?;
        let bytes = read_bytes(path)?;
        let stream = stream_of(&bytes)?;
        let bitmap = bitmap_of(&stream.cast::<IRandomAccessStream>().map_err(failed)?)?;
        Ok(OcrText {
            text: recognize(&engines, &bitmap)?,
            pages: 1,
        })
    }

    fn pdf(&self, path: &Path, max_pages: usize) -> Result<OcrText, OcrError> {
        match &self.worker {
            Some(worker) => run_worker(worker, path, max_pages, &self.wanted),
            None => self.pdf_in_process(path, max_pages),
        }
    }
}

impl WinOcr {
    /// Rendert und liest PDF-Seiten im eigenen Prozess. `Windows.Data.Pdf` lässt den Prozess
    /// nach dem Rendern beim Beenden abstürzen (Zugriffsverletzung beim Entladen), deshalb läuft
    /// das normalerweise im Worker, der sich nach der Ausgabe mit `TerminateProcess` beendet.
    pub fn pdf_in_process(&self, path: &Path, max_pages: usize) -> Result<OcrText, OcrError> {
        ensure_winrt();
        let engines = self.engines()?;
        let bytes = read_bytes(path)?;
        let stream = stream_of(&bytes)?;
        let doc = PdfDocument::LoadFromStreamAsync(
            &stream.cast::<IRandomAccessStream>().map_err(failed)?,
        )
        .and_then(|op| op.join())
        .map_err(|e| {
            // Passwortgeschützte PDFs liefern beim Laden einen Fehler.
            if e.code().0 as u32 == 0x8000_0100 || e.message().to_lowercase().contains("password") {
                OcrError::Encrypted
            } else {
                OcrError::Corrupt
            }
        })?;
        if doc.IsPasswordProtected().unwrap_or(false) {
            return Err(OcrError::Encrypted);
        }
        let page_count = doc.PageCount().map_err(failed)?;
        let max = OcrEngine::MaxImageDimension().map_err(failed)?;
        let mut text = String::new();
        let mut read = 0u32;
        for index in 0..page_count.min(max_pages as u32) {
            let page = doc.GetPage(index).map_err(|_| OcrError::Corrupt)?;
            let page_text = (|| {
                let size = page.Size().map_err(failed)?;
                let options = PdfPageRenderOptions::new().map_err(failed)?;
                let longest = size.Width.max(size.Height).max(1.0);
                let scale = PDF_SCALE.min(max as f32 / longest);
                options
                    .SetDestinationWidth((size.Width * scale).round().max(1.0) as u32)
                    .map_err(failed)?;
                options
                    .SetDestinationHeight((size.Height * scale).round().max(1.0) as u32)
                    .map_err(failed)?;
                options
                    .SetBackgroundColor(Color {
                        A: 255,
                        R: 255,
                        G: 255,
                        B: 255,
                    })
                    .map_err(failed)?;
                let out = InMemoryRandomAccessStream::new().map_err(failed)?;
                let rendered = page
                    .RenderWithOptionsToStreamAsync(&out, &options)
                    .and_then(|op| op.join())
                    .map_err(|_| OcrError::Corrupt)
                    .and_then(|()| out.Seek(0).map_err(failed))
                    .and_then(|()| bitmap_of(&out.cast::<IRandomAccessStream>().map_err(failed)?));
                close(&out);
                let bitmap = rendered?;
                let text = recognize(&engines, &bitmap);
                close(&bitmap);
                text
            })();
            close(&page);
            text.push_str(&page_text?);
            text.push('\n');
            read += 1;
        }
        close(&doc);
        close(&stream);
        Ok(OcrText { text, pages: read })
    }
}

/// Antwort des Workers (eine JSON-Zeile auf stdout).
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
enum WorkerReply {
    Ok { text: String, pages: u32 },
    NoLanguage,
    Encrypted,
    Corrupt,
    Unavailable { message: String },
    Failed { message: String },
}

impl From<Result<OcrText, OcrError>> for WorkerReply {
    fn from(r: Result<OcrText, OcrError>) -> Self {
        match r {
            Ok(t) => Self::Ok {
                text: t.text,
                pages: t.pages,
            },
            Err(OcrError::NoLanguage) => Self::NoLanguage,
            Err(OcrError::Encrypted) => Self::Encrypted,
            Err(OcrError::Corrupt) => Self::Corrupt,
            Err(OcrError::Unavailable(message)) => Self::Unavailable { message },
            Err(OcrError::Failed(message)) => Self::Failed { message },
        }
    }
}

impl From<WorkerReply> for Result<OcrText, OcrError> {
    fn from(r: WorkerReply) -> Self {
        match r {
            WorkerReply::Ok { text, pages } => Ok(OcrText { text, pages }),
            WorkerReply::NoLanguage => Err(OcrError::NoLanguage),
            WorkerReply::Encrypted => Err(OcrError::Encrypted),
            WorkerReply::Corrupt => Err(OcrError::Corrupt),
            WorkerReply::Unavailable { message } => Err(OcrError::Unavailable(message)),
            WorkerReply::Failed { message } => Err(OcrError::Failed(message)),
        }
    }
}

/// Startet den Worker für ein PDF und wartet höchstens [`WORKER_TIMEOUT`].
fn run_worker(
    worker: &Path,
    path: &Path,
    max_pages: usize,
    langs: &[String],
) -> Result<OcrText, OcrError> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut child = Command::new(worker)
        .arg("ocr-worker")
        .arg(path)
        .arg("--max-pages")
        .arg(max_pages.to_string())
        .arg("--langs")
        .arg(langs.join(","))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| OcrError::Failed(format!("Worker nicht startbar: {e}")))?;
    let mut stdout = child.stdout.take().expect("stdout ist umgeleitet");
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() > WORKER_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(OcrError::Failed(e.to_string())),
        }
    };
    let output = reader.join().unwrap_or_default();
    let Some(status) = status else {
        return Err(OcrError::Unavailable("timeout".into()));
    };
    // Der Exit-Code zählt nicht: maßgeblich ist die Antwortzeile.
    output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<WorkerReply>(line).ok())
        .map_or_else(
            || {
                Err(OcrError::Failed(format!(
                    "Worker ohne Antwort (Exit-Code {:?})",
                    status.code()
                )))
            },
            Into::into,
        )
}

/// Einstieg des Unterbefehls `ocr-worker`: liest das PDF, schreibt eine JSON-Zeile und beendet
/// den Prozess sofort mit `TerminateProcess` (umgeht den Absturz beim normalen Beenden).
pub fn worker_main(path: &Path, max_pages: usize, langs: Vec<String>) -> ! {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    // Kein Fehlerdialog, falls doch etwas abstürzt.
    // SAFETY: setzt nur die Fehlermodus-Flags des eigenen Prozesses.
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::SetErrorMode(0x0001 | 0x0002 | 0x8000)
    };
    let reply: WorkerReply = WinOcr {
        wanted: langs,
        worker: None,
    }
    .pdf_in_process(path, max_pages)
    .into();
    let line = serde_json::to_string(&reply)
        .unwrap_or_else(|_| r#"{"result":"failed","message":"Ausgabe"}"#.into());
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
    // SAFETY: beendet den eigenen Prozess; stdout ist bereits geleert.
    unsafe { TerminateProcess(GetCurrentProcess(), 0) };
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprach_tags_passen_auf_kurzform_und_volle_form() {
        assert!(tag_matches("de-DE", "de"));
        assert!(tag_matches("de-DE", "DE-de"));
        assert!(tag_matches("en-US", "en"));
        assert!(!tag_matches("en-US", "de"));
        assert!(!tag_matches("de-DE", "d"));
    }

    #[test]
    fn fehlendes_sprachpaket_ist_ein_klarer_fehler() {
        let ocr = WinOcr::new(vec!["xx-zz".into()]);
        assert!(ocr.languages().is_empty());
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.png");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(ocr.image(&f), Err(OcrError::NoLanguage));
    }

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/content/rechnung-scan.jpg");

    /// `None` (mit Hinweis), wenn das deutsche Sprachpaket fehlt.
    fn german() -> Option<WinOcr> {
        let ocr = WinOcr::new(vec!["de".into()]);
        if ocr.languages().is_empty() {
            eprintln!(
                "OCR-Test übersprungen: kein deutsches OCR-Sprachpaket ({}). Installiert: {:?}",
                crate::content::ocr::NO_LANGUAGE_HINT,
                WinOcr::available_languages()
            );
            return None;
        }
        Some(ocr)
    }

    #[test]
    fn liest_ein_bild_scan() {
        let Some(ocr) = german() else { return };
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("scan.jpg");
        std::fs::write(&file, FIXTURE).unwrap();
        let out = ocr.image(&file).unwrap();
        let text = out.text.to_lowercase();
        assert!(text.contains("rechnung"), "{text:?}");
        assert!(text.contains("4711"), "{text:?}");
        assert!(text.contains("119,00"), "{text:?}");
        assert_eq!(out.pages, 1);
    }

    #[test]
    fn kaputte_dateien_sind_corrupt_kein_absturz() {
        let Some(ocr) = german() else { return };
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.jpg");
        std::fs::write(&bad, b"das ist kein bild").unwrap();
        assert_eq!(ocr.image(&bad), Err(OcrError::Corrupt));
    }

    #[test]
    fn cloud_platzhalter_wird_nicht_gelesen() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;
        let Some(ocr) = german() else { return };
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.jpg");
        std::fs::write(&file, FIXTURE).unwrap();
        let wide: Vec<u16> = file.as_os_str().encode_wide().chain(Some(0)).collect();
        assert_ne!(unsafe { SetFileAttributesW(wide.as_ptr(), 0x1000) }, 0);
        assert!(matches!(ocr.image(&file), Err(OcrError::Unavailable(_))));
    }
}
