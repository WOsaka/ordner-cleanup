//! OCR von gescannten PDFs läuft in einem eigenen Prozess (`ocr-worker`), weil
//! `Windows.Data.Pdf` den Prozess nach dem Rendern beim Beenden abstürzen lässt.
//! Die Tests laufen nur, wenn das deutsche OCR-Sprachpaket installiert ist.

use ordner_cleanup::content::extract::pdf::testing::scanned_pdf;
use ordner_cleanup::content::ocr::{Ocr, OcrError};
use ordner_cleanup::platform::ocr::WinOcr;

const FIXTURE: &[u8] = include_bytes!("fixtures/content/rechnung-scan.jpg");
const WORKER: &str = env!("CARGO_BIN_EXE_ordner-cleanup");

fn german() -> Option<WinOcr> {
    let ocr = WinOcr::new(vec!["de".into()]).with_worker(WORKER.into());
    if ocr.languages().is_empty() {
        eprintln!(
            "OCR-Test übersprungen: kein deutsches OCR-Sprachpaket ({})",
            ordner_cleanup::content::ocr::NO_LANGUAGE_HINT
        );
        return None;
    }
    Some(ocr)
}

#[test]
fn liest_ein_gescanntes_pdf_und_begrenzt_die_seiten() {
    let Some(ocr) = german() else { return };
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("scan.pdf");
    std::fs::write(&file, scanned_pdf(FIXTURE, 1600, 700)).unwrap();
    let out = ocr.pdf(&file, 5).unwrap();
    let text = out.text.to_lowercase();
    assert!(text.contains("rechnung"), "{text:?}");
    assert!(text.contains("4711"), "{text:?}");
    assert_eq!(out.pages, 1);
    assert_eq!(
        ocr.pdf(&file, 0).unwrap().pages,
        0,
        "max_pages = 0 liest nichts"
    );
}

#[test]
fn kaputtes_pdf_ist_corrupt_und_der_worker_beendet_sich_sauber() {
    let Some(ocr) = german() else { return };
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.pdf");
    std::fs::write(&bad, b"%PDF-1.4 kaputt").unwrap();
    assert_eq!(ocr.pdf(&bad, 3), Err(OcrError::Corrupt));
    let missing = dir.path().join("fehlt.pdf");
    assert!(matches!(
        ocr.pdf(&missing, 3),
        Err(OcrError::Unavailable(_))
    ));
}

#[test]
fn ohne_sprachpaket_meldet_der_worker_no_language() {
    let ocr = WinOcr::new(vec!["xx-zz".into()]).with_worker(WORKER.into());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("scan.pdf");
    std::fs::write(&file, scanned_pdf(FIXTURE, 1600, 700)).unwrap();
    assert_eq!(ocr.pdf(&file, 3), Err(OcrError::NoLanguage));
}

#[test]
fn der_worker_exit_code_ist_null_trotz_pdf_rendering() {
    let Some(_) = german() else { return };
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("scan.pdf");
    std::fs::write(&file, scanned_pdf(FIXTURE, 1600, 700)).unwrap();
    let out = std::process::Command::new(WORKER)
        .args(["ocr-worker"])
        .arg(&file)
        .args(["--langs", "de"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{:?}", out);
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"result\":\"ok\""));
}
