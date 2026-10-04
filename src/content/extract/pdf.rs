//! PDF: Textlayer, Metadaten (`Title`, `Author`, `CreationDate`), Seitenzahl, Verschlüsselung.
//! Pures Rust (`pdf-extract` auf `lopdf`). Der Parser läuft in einem Worker-Thread mit
//! Zeitwächter; Panics und Hänger machen die Datei zu `unreadable`, nie den Lauf kaputt.
//! Begrenzt sind Seitenzahl und Textmenge.

use std::io::{BufReader, Read};
use std::path::Path;

use pdf_extract::{output_doc_page, Document, Object, PlainTextOutput};

use super::{guarded, iso_date, status_for, truncate_chars, ExtractOpts, Extracted, GuardError};
use crate::content::{Status, TextSource};
use crate::scan::exif::open_local;

/// Mehr Seiten werden für den Text nicht gelesen.
pub const MAX_PAGES: u32 = 30;

/// PDF-Textstring: UTF-16BE mit BOM oder PDFDocEncoding (hier wie Latin-1 behandelt).
fn pdf_string(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&bytes[3..]).into_owned()
    } else {
        bytes.iter().map(|&b| char::from(b)).collect()
    }
}

/// `D:20260930101500+02'00'` → `2026-09-30`.
pub fn pdf_date(raw: &str) -> Option<String> {
    let digits = raw.strip_prefix("D:").unwrap_or(raw);
    let n = |a: usize, b: usize| digits.get(a..b)?.parse::<u32>().ok();
    let year = i32::try_from(n(0, 4)?).ok()?;
    iso_date(year, n(4, 6).unwrap_or(1), n(6, 8).unwrap_or(1))
}

fn info_string(doc: &Document, info: &pdf_extract::Dictionary, key: &[u8]) -> Option<String> {
    let (_, obj) = doc.dereference(info.get(key).ok()?).ok()?;
    match obj {
        Object::String(bytes, _) => {
            let s = pdf_string(bytes);
            let s = s.trim().to_string();
            (!s.is_empty()).then_some(s)
        }
        _ => None,
    }
}

fn info_dict(doc: &Document) -> Option<&pdf_extract::Dictionary> {
    let (_, obj) = doc.dereference(doc.trailer.get(b"Info").ok()?).ok()?;
    obj.as_dict().ok()
}

fn parse(data: &[u8], max_chars: usize) -> Extracted {
    let mut doc = match Document::load_mem(data) {
        Ok(d) => d,
        Err(_) => return Extracted::new(Status::Unreadable("corrupt".into())),
    };
    if doc.is_encrypted() && doc.decrypt("").is_err() {
        return Extracted::new(Status::Unreadable("encrypted".into()));
    }
    let mut out = Extracted::new(Status::Ok);
    let page_count = doc.get_pages().len() as u32;
    out.pages = Some(page_count);
    out.meta.insert("doc.pages".into(), page_count.to_string());

    if let Some(info) = info_dict(&doc) {
        for (key, field) in [(&b"Title"[..], "doc.title"), (b"Author", "doc.author")] {
            if let Some(v) = info_string(&doc, info, key) {
                out.meta.insert(field.into(), v);
            }
        }
        if let Some(date) = info_string(&doc, info, b"CreationDate").and_then(|d| pdf_date(&d)) {
            out.meta.insert("doc.created".into(), date);
        }
    }

    let mut text = String::new();
    for page in 1..=page_count.min(MAX_PAGES) {
        let mut page_text = String::new();
        {
            let mut output = PlainTextOutput::new(&mut page_text);
            // Eine kaputte Seite lässt die übrigen lesbar.
            if output_doc_page(&doc, &mut output, page).is_err() {
                continue;
            }
        }
        text.push_str(&page_text);
        text.push('\n');
        if text.chars().count() >= max_chars {
            break;
        }
    }
    truncate_chars(&mut text, max_chars);
    if !text.trim().is_empty() {
        out.text = Some(text);
        out.text_source = TextSource::Layer;
    }
    out
}

pub fn extract(path: &Path, opts: &ExtractOpts) -> Extracted {
    let reader = match open_local(path, opts.max_file_size) {
        Ok(r) => r,
        Err(e) => return Extracted::new(status_for(&e)),
    };
    let mut data = Vec::new();
    if let Err(e) = BufReader::new(reader).read_to_end(&mut data) {
        return Extracted::new(super::status_for_io(&e));
    }
    extract_bytes(data, opts)
}

pub fn extract_bytes(data: Vec<u8>, opts: &ExtractOpts) -> Extracted {
    let max_chars = opts.max_text_chars;
    match guarded(opts.timeout, move || parse(&data, max_chars)) {
        Ok(out) => out,
        Err(GuardError::Panicked) => Extracted::new(Status::Unreadable("corrupt".into())),
        Err(GuardError::TimedOut | GuardError::Disabled) => {
            Extracted::new(Status::Unreadable("timeout".into()))
        }
    }
}

/// Erzeugt kleine PDFs für Tests (auch Integrationstests): Seiten mit Text, optional Metadaten
/// und Verschlüsselung per leerem Benutzerkennwort entfällt (nur Struktur, kein RC4).
#[doc(hidden)]
pub mod testing {
    use pdf_extract::{dictionary, Document, Object, Stream, StringFormat};

    /// Ein PDF mit einer Textzeile je Zeile des Seitentexts. Leere Seiten (`""`) haben keinen
    /// Textlayer (wie ein Scan).
    pub fn pdf_with_pages(pages: &[&str], title: Option<&str>, created: Option<&str>) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let mut kids = Vec::new();
        for text in pages {
            let mut content = String::new();
            if !text.is_empty() {
                content.push_str("BT /F1 12 Tf 14 TL 50 780 Td\n");
                for line in text.lines() {
                    content.push_str(&format!("({}) Tj T*\n", escape(line)));
                }
                content.push_str("ET");
            }
            let content_id = doc.add_object(Stream::new(dictionary! {}, win_ansi(&content)));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
                "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            });
            kids.push(Object::Reference(page_id));
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids.clone(),
                "Count" => kids.len() as i64,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        let mut info = dictionary! {};
        if let Some(t) = title {
            info.set(
                "Title",
                Object::String(t.as_bytes().to_vec(), StringFormat::Literal),
            );
        }
        if let Some(c) = created {
            info.set(
                "CreationDate",
                Object::String(c.as_bytes().to_vec(), StringFormat::Literal),
            );
        }
        if !info.is_empty() {
            let info_id = doc.add_object(info);
            doc.trailer.set("Info", info_id);
        }
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    fn escape(line: &str) -> String {
        line.replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)")
    }

    /// Text → WinAnsi-Bytes (nur Latin-1-Bereich und €).
    fn win_ansi(s: &str) -> Vec<u8> {
        s.chars()
            .map(|c| match c {
                '€' => 0x80,
                c if (c as u32) < 256 => c as u32 as u8,
                _ => b'?',
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn opts() -> ExtractOpts {
        ExtractOpts::default()
    }

    #[test]
    fn textlayer_und_metadaten() {
        let data = pdf_with_pages(
            &[
                "Rechnung Nr. 4711\nGesamtbetrag 123,45 EUR",
                "Zahlbar bis 30.10.2026",
            ],
            Some("Meine Rechnung"),
            Some("D:20260930101500+02'00'"),
        );
        let out = extract_bytes(data, &opts());
        assert_eq!(out.status, Status::Ok);
        let text = out.text.unwrap();
        assert!(text.contains("Rechnung Nr. 4711"), "{text:?}");
        assert!(text.contains("Zahlbar bis"), "{text:?}");
        assert_eq!(out.text_source, TextSource::Layer);
        assert_eq!(out.pages, Some(2));
        assert_eq!(out.meta["doc.title"], "Meine Rechnung");
        assert_eq!(out.meta["doc.created"], "2026-09-30");
    }

    #[test]
    fn umlaute_und_euro_kommen_an() {
        let data = pdf_with_pages(&["Größe Übersicht Prüfung 50 €"], None, None);
        let text = extract_bytes(data, &opts()).text.unwrap();
        assert!(text.contains("Größe Übersicht Prüfung"), "{text:?}");
        assert!(text.contains('€'), "{text:?}");
    }

    #[test]
    fn seite_ohne_text_ergibt_keinen_textlayer() {
        let out = extract_bytes(pdf_with_pages(&["", ""], None, None), &opts());
        assert_eq!(out.status, Status::Ok);
        assert_eq!(out.text, None);
        assert_eq!(out.pages, Some(2));
    }

    #[test]
    fn kaputte_pdfs_sind_unreadable() {
        for data in [
            Vec::new(),
            b"%PDF-1.4\nkaputt".to_vec(),
            b"kein pdf".to_vec(),
        ] {
            let out = extract_bytes(data, &opts());
            assert!(
                matches!(out.status, Status::Unreadable(_)),
                "{:?}",
                out.status
            );
        }
    }

    #[test]
    fn abgeschnittenes_pdf_bringt_den_lauf_nicht_zu_fall() {
        let full = pdf_with_pages(&["Hallo Welt"], Some("x"), None);
        for cut in (0..full.len()).step_by(37) {
            let _ = extract_bytes(full[..cut].to_vec(), &opts());
        }
    }

    #[test]
    fn seiten_und_zeichen_sind_begrenzt() {
        let pages: Vec<String> = (0..40).map(|i| format!("Seite {i} Text")).collect();
        let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
        let out = extract_bytes(pdf_with_pages(&refs, None, None), &opts());
        let text = out.text.unwrap();
        assert!(text.contains("Seite 29"));
        assert!(!text.contains("Seite 30 "), "höchstens {MAX_PAGES} Seiten");
        assert_eq!(out.pages, Some(40));

        let small = ExtractOpts {
            max_text_chars: 20,
            ..opts()
        };
        let out = extract_bytes(pdf_with_pages(&refs, None, None), &small);
        assert!(out.text.unwrap().chars().count() <= 20);
    }

    #[test]
    fn pdf_daten() {
        assert_eq!(
            pdf_date("D:20260930101500+02'00'").as_deref(),
            Some("2026-09-30")
        );
        assert_eq!(pdf_date("D:2026").as_deref(), Some("2026-01-01"));
        assert_eq!(pdf_date("D:00000101"), None);
        assert_eq!(pdf_date("müll"), None);
    }

    #[test]
    fn pdf_texte_utf16() {
        let mut v = vec![0xFE, 0xFF];
        for u in "Größe".encode_utf16() {
            v.extend(u.to_be_bytes());
        }
        assert_eq!(pdf_string(&v), "Größe");
        assert_eq!(pdf_string(b"Hallo"), "Hallo");
    }
}
