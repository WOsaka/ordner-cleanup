//! Extraktoren: Text und Metadaten je Dateityp. Fremde Dateien sind nicht vertrauenswürdig:
//! Es gibt Obergrenzen, Panics werden je Datei abgefangen, Parser mit Hang-Risiko laufen in
//! einem Worker-Thread mit Zeitwächter, und Cloud-only-Dateien werden nie geöffnet.

pub mod image;
pub mod office;
pub mod pdf;
pub mod video;

use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;
use std::time::Duration;

use super::{Fields, Status, TextSource};
use crate::scan::exif::ExifError;

/// Grenzen für einen Extraktionslauf.
#[derive(Debug, Clone)]
pub struct ExtractOpts {
    /// Größere Dateien: `too-large` (Metadaten aus dem Kopfbereich werden trotzdem gelesen).
    pub max_file_size: u64,
    /// Obergrenze für den gelesenen Text je Datei (in Zeichen).
    pub max_text_chars: usize,
    /// Zeitwächter je Datei für Parser, die hängen können (PDF).
    pub timeout: Duration,
}

impl Default for ExtractOpts {
    fn default() -> Self {
        Self {
            max_file_size: 100 * 1024 * 1024,
            max_text_chars: 200_000,
            timeout: Duration::from_secs(30),
        }
    }
}

/// Ergebnis der Extraktion einer Datei.
#[derive(Debug, Clone, PartialEq)]
pub struct Extracted {
    pub text: Option<String>,
    pub text_source: TextSource,
    /// Metadaten (`doc.title`, `doc.author`, `doc.created`, `doc.pages`, `exif.*`, `video.date`,
    /// `img.width`, `img.height`).
    pub meta: Fields,
    pub pages: Option<u32>,
    pub status: Status,
}

impl Extracted {
    pub fn new(status: Status) -> Self {
        Self {
            text: None,
            text_source: TextSource::None,
            meta: Fields::new(),
            pages: None,
            status,
        }
    }
}

/// Gründe, die nicht gecacht werden: ein späterer Lauf versucht es erneut.
pub const TRANSIENT_REASONS: &[&str] = &[
    "cloud-placeholder",
    "locked",
    "vanished",
    "timeout",
    "no-ocr-language",
    "ocr-failed",
    "io",
];

impl Status {
    /// Vorübergehend (Datei gesperrt, Cloud-Platzhalter, Zeitüberschreitung …): nicht cachen.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Unreadable(why) if TRANSIENT_REASONS.contains(&why.as_str()))
    }
}

/// Abbildung eines Lesefehlers auf einen Status.
pub fn status_for(error: &ExifError) -> Status {
    match error {
        ExifError::NotLocal => Status::Unreadable("cloud-placeholder".into()),
        ExifError::Io(e) => status_for_io(e),
    }
}

pub fn status_for_io(e: &io::Error) -> Status {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    let why = match e.kind() {
        io::ErrorKind::NotFound => "vanished",
        io::ErrorKind::PermissionDenied => "locked",
        _ => match e.raw_os_error() {
            Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) => "locked",
            _ => "io",
        },
    };
    Status::Unreadable(why.into())
}

/// Bildendungen, bei denen EXIF gelesen wird.
pub const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "heic", "heif", "tif", "tiff", "webp"];
pub const VIDEO_EXTS: &[&str] = &["mp4", "mov", "m4v", "3gp"];
pub const OFFICE_EXTS: &[&str] = &["docx", "xlsx", "pptx"];

pub fn is_image(ext: &str) -> bool {
    IMAGE_EXTS.contains(&ext)
}

/// Liest Text und Metadaten. `ext` ist klein geschrieben, ohne Punkt.
pub fn extract(path: &Path, ext: &str, size: u64, opts: &ExtractOpts) -> Extracted {
    let too_large = size > opts.max_file_size;
    let mut out = if is_image(ext) {
        image::extract(path)
    } else if VIDEO_EXTS.contains(&ext) {
        video::extract(path)
    } else if too_large && (ext == "pdf" || OFFICE_EXTS.contains(&ext)) {
        Extracted::new(Status::Ok)
    } else if ext == "pdf" {
        pdf::extract(path, opts)
    } else if OFFICE_EXTS.contains(&ext) {
        office::extract(path, ext, opts)
    } else {
        return Extracted::new(Status::Unsupported);
    };
    if too_large && out.status.is_ok() {
        out.status = Status::TooLarge;
        out.text = None;
        out.text_source = TextSource::None;
    }
    out
}

/// Kürzt auf höchstens `max` Zeichen (an einer Zeichengrenze).
pub fn truncate_chars(text: &mut String, max: usize) {
    if let Some((idx, _)) = text.char_indices().nth(max) {
        text.truncate(idx);
    }
}

static ABANDONED: AtomicUsize = AtomicUsize::new(0);
/// Höchstens so viele aufgegebene (hängende) Worker je Prozess, danach wird der betroffene
/// Dateityp nicht mehr gelesen.
pub const MAX_ABANDONED: usize = 4;

pub fn abandoned_workers() -> usize {
    ABANDONED.load(Ordering::Relaxed)
}

/// Verhindert, dass abgefangene Parser-Panics in Worker-Threads die Konsole füllen.
pub fn install_quiet_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let quiet = std::thread::current()
                .name()
                .is_some_and(|n| n.starts_with("extract-worker"));
            if !quiet {
                previous(info);
            }
        }));
    });
}

#[derive(Debug, PartialEq, Eq)]
pub enum GuardError {
    Panicked,
    TimedOut,
    /// Zu viele Worker hingen schon; dieser Dateityp wird übersprungen.
    Disabled,
}

/// Führt `f` in einem Worker-Thread aus: Panics werden gefangen, bei Überschreitung von
/// `timeout` wird der Thread aufgegeben (Rust-Threads lassen sich nicht abbrechen).
pub fn guarded<T, F>(timeout: Duration, f: F) -> Result<T, GuardError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    if ABANDONED.load(Ordering::Relaxed) >= MAX_ABANDONED {
        return Err(GuardError::Disabled);
    }
    install_quiet_panic_hook();
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("extract-worker".into())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(f));
            let _ = tx.send(result);
        });
    if spawned.is_err() {
        return Err(GuardError::Panicked);
    }
    match rx.recv_timeout(timeout) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(GuardError::Panicked),
        Err(_) => {
            ABANDONED.fetch_add(1, Ordering::Relaxed);
            Err(GuardError::TimedOut)
        }
    }
}

/// Datum `YYYY-MM-DD` aus Bausteinen, falls plausibel (1900 bis 2200).
pub fn iso_date(y: i32, m: u32, d: u32) -> Option<String> {
    let date = chrono::NaiveDate::from_ymd_opt(y, m, d)?;
    (1900..=2200)
        .contains(&y)
        .then(|| date.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_liefert_ergebnis() {
        assert_eq!(guarded(Duration::from_secs(5), || 42), Ok(42));
    }

    #[test]
    fn guarded_faengt_panics() {
        let r = guarded(Duration::from_secs(5), || -> i32 { panic!("kaputt") });
        assert_eq!(r, Err(GuardError::Panicked));
    }

    #[test]
    fn truncate_chars_achtet_auf_zeichengrenzen() {
        let mut s = "äöüß".to_string();
        truncate_chars(&mut s, 2);
        assert_eq!(s, "äö");
        truncate_chars(&mut s, 10);
        assert_eq!(s, "äö");
    }

    #[test]
    fn transient_gruende() {
        assert!(Status::Unreadable("locked".into()).is_transient());
        assert!(!Status::Unreadable("encrypted".into()).is_transient());
        assert!(!Status::Ok.is_transient());
    }

    #[test]
    fn io_fehler_werden_abgebildet() {
        let nf = io::Error::from(io::ErrorKind::NotFound);
        assert_eq!(status_for_io(&nf), Status::Unreadable("vanished".into()));
        let sv = io::Error::from_raw_os_error(32);
        assert_eq!(status_for_io(&sv), Status::Unreadable("locked".into()));
    }
}
