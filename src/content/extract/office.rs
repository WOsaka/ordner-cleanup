//! Office (docx/xlsx/pptx): Text und Kern-Metadaten aus dem ZIP-Container. Fremde Archive
//! sind nicht vertrauenswürdig: Eintragszahl, entpackte Größen und das Verhältnis
//! entpackt/gepackt sind begrenzt (Zip-Bomben). Makros und eingebettete Inhalte werden nie
//! ausgeführt oder gelesen. Verschlüsselte Dateien sind OLE-Container statt ZIP.

use std::io::{BufReader, Cursor, Read};
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;
use zip::ZipArchive;

use super::{iso_date, status_for, truncate_chars, ExtractOpts, Extracted};
use crate::content::{Status, TextSource};
use crate::scan::exif::open_local;

const OLE_SIGNATURE: [u8; 4] = [0xD0, 0xCF, 0x11, 0xE0];
const MAX_ENTRIES: usize = 2_000;
const MAX_ENTRY_BYTES: u64 = 50 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 200 * 1024 * 1024;
const MAX_RATIO: u64 = 100;
/// Mehr als so viele Blätter bzw. Folien werden nicht gelesen.
const MAX_PARTS: usize = 60;

#[derive(Debug, PartialEq, Eq)]
enum OfficeError {
    Encrypted,
    Corrupt,
    TooLarge,
}

impl OfficeError {
    fn status(&self) -> Status {
        match self {
            Self::Encrypted => Status::Unreadable("encrypted".into()),
            Self::Corrupt => Status::Unreadable("corrupt".into()),
            Self::TooLarge => Status::TooLarge,
        }
    }
}

/// Prüft die Grenzen des Archivs anhand der Verzeichniseinträge.
fn check_limits<R: Read + std::io::Seek>(zip: &mut ZipArchive<R>) -> Result<(), OfficeError> {
    if zip.len() > MAX_ENTRIES {
        return Err(OfficeError::TooLarge);
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i).map_err(|_| OfficeError::Corrupt)?;
        let (size, packed) = (entry.size(), entry.compressed_size());
        if size > MAX_ENTRY_BYTES {
            return Err(OfficeError::TooLarge);
        }
        if size > 1024 * 1024 && size / packed.max(1) > MAX_RATIO {
            return Err(OfficeError::TooLarge);
        }
        total = total.saturating_add(size);
        if total > MAX_TOTAL_BYTES {
            return Err(OfficeError::TooLarge);
        }
    }
    Ok(())
}

/// Liest einen Eintrag vollständig (mit harter Obergrenze, auch wenn der Kopf lügt).
fn read_entry<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    name: &str,
) -> Result<Option<Vec<u8>>, OfficeError> {
    let entry = match zip.by_name(name) {
        Ok(e) => e,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(_) => return Err(OfficeError::Corrupt),
    };
    let mut buf = Vec::new();
    entry
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|_| OfficeError::Corrupt)?;
    if buf.len() as u64 > MAX_ENTRY_BYTES {
        return Err(OfficeError::TooLarge);
    }
    Ok(Some(buf))
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

/// Sammelt Text aus `t`-Elementen (`w:t`, `a:t`, `si/t`); Absatzende (`p`, `si`) wird zur Zeile.
fn xml_text(xml: &[u8], out: &mut String, max_chars: usize) -> Result<(), OfficeError> {
    let mut reader = Reader::from_reader(BufReader::new(xml));
    let mut buf = Vec::new();
    let mut in_t = false;
    loop {
        if out.chars().count() >= max_chars {
            return Ok(());
        }
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                "t" => in_t = true,
                "tab" => out.push(' '),
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "tab" => out.push(' '),
                "br" => out.push('\n'),
                _ => {}
            },
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                "t" => in_t = false,
                "p" | "si" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) if in_t => {
                out.push_str(&t);
            }
            Ok(Event::GeneralRef(r)) if in_t => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    out.push(c);
                } else if let Some(c) = entity(&r) {
                    out.push(c);
                }
            }
            Ok(Event::Eof) => return Ok(()),
            Err(_) => return Err(OfficeError::Corrupt),
            _ => {}
        }
        buf.clear();
    }
}

/// Einzeltexte aus `docProps/core.xml` (`title`, `creator`, `created`) und `app.xml`.
fn xml_props(xml: &[u8], wanted: &[&str]) -> Vec<(String, String)> {
    let mut reader = Reader::from_reader(BufReader::new(xml));
    let mut buf = Vec::new();
    let mut found = Vec::new();
    let mut current: Option<String> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.local_name().as_ref().to_string();
                current = wanted.contains(&name.as_str()).then_some(name);
            }
            Ok(Event::Text(t)) => {
                if let Some(name) = &current {
                    found.push((name.clone(), t.trim().to_string()));
                }
            }
            Ok(Event::End(_)) => current = None,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    found
}

/// `2026-09-30T10:00:00Z` → `2026-09-30`.
fn w3cdtf_date(s: &str) -> Option<String> {
    let (y, rest) = s.split_once('-')?;
    let (m, rest) = rest.split_once('-')?;
    let d: String = rest.chars().take(2).collect();
    iso_date(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

fn parts(zip: &ZipArchive<impl Read + std::io::Seek>, prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = zip
        .file_names()
        .filter(|n| n.starts_with(prefix) && n.ends_with(".xml"))
        .map(str::to_string)
        .collect();
    // slide2 vor slide10
    names.sort_by_key(|n| {
        let digits: String = n.chars().filter(char::is_ascii_digit).collect();
        (digits.len(), digits)
    });
    names.truncate(MAX_PARTS);
    names
}

fn extract_bytes(data: &[u8], ext: &str, max_chars: usize) -> Result<Extracted, OfficeError> {
    if data.starts_with(&OLE_SIGNATURE) {
        return Err(OfficeError::Encrypted);
    }
    let mut zip = ZipArchive::new(Cursor::new(data)).map_err(|_| OfficeError::Corrupt)?;
    check_limits(&mut zip)?;
    let mut out = Extracted::new(Status::Ok);

    let text_parts: Vec<String> = match ext {
        "docx" => vec!["word/document.xml".into()],
        "pptx" => parts(&zip, "ppt/slides/slide"),
        "xlsx" => {
            let mut v = vec!["xl/sharedStrings.xml".to_string()];
            v.extend(parts(&zip, "xl/worksheets/sheet"));
            v
        }
        _ => Vec::new(),
    };
    let mut text = String::new();
    for name in &text_parts {
        if let Some(xml) = read_entry(&mut zip, name)? {
            xml_text(&xml, &mut text, max_chars)?;
        }
        if text.chars().count() >= max_chars {
            break;
        }
    }
    truncate_chars(&mut text, max_chars);
    if !text.trim().is_empty() {
        out.text = Some(text);
        out.text_source = TextSource::Office;
    }

    if let Some(core) = read_entry(&mut zip, "docProps/core.xml")? {
        for (key, value) in xml_props(&core, &["title", "creator", "created"]) {
            if value.is_empty() {
                continue;
            }
            match key.as_str() {
                "title" => out.meta.insert("doc.title".into(), value),
                "creator" => out.meta.insert("doc.author".into(), value),
                "created" => {
                    w3cdtf_date(&value).and_then(|d| out.meta.insert("doc.created".into(), d))
                }
                _ => None,
            };
        }
    }
    if let Some(app) = read_entry(&mut zip, "docProps/app.xml")? {
        let key = if ext == "pptx" { "Slides" } else { "Pages" };
        if let Some((_, v)) = xml_props(&app, &[key]).into_iter().next() {
            if let Ok(n) = v.parse::<u32>() {
                if n > 0 {
                    out.pages = Some(n);
                    out.meta.insert("doc.pages".into(), n.to_string());
                }
            }
        }
    }
    Ok(out)
}

pub fn extract(path: &Path, ext: &str, opts: &ExtractOpts) -> Extracted {
    let reader = match open_local(path, opts.max_file_size) {
        Ok(r) => r,
        Err(e) => return Extracted::new(status_for(&e)),
    };
    let mut data = Vec::new();
    if let Err(e) = BufReader::new(reader).read_to_end(&mut data) {
        return Extracted::new(super::status_for_io(&e));
    }
    match extract_bytes(&data, ext, opts.max_text_chars) {
        Ok(out) => out,
        Err(e) => Extracted::new(e.status()),
    }
}

/// Minimale Office-Dateien für Tests (auch Integrationstests).
#[doc(hidden)]
pub mod testing {
    use std::io::{Cursor, Write};

    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn esc(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    pub fn zip_of(entries: &[(&str, String)]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn core(title: &str, author: &str, created: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><cp:coreProperties xmlns:cp="c" xmlns:dc="d" xmlns:dcterms="t"><dc:title>{}</dc:title><dc:creator>{}</dc:creator><dcterms:created>{}</dcterms:created></cp:coreProperties>"#,
            esc(title),
            esc(author),
            created
        )
    }

    /// docx mit je einem Absatz pro Zeile und Kern-Metadaten.
    pub fn docx(lines: &[&str], title: &str, author: &str, created: &str) -> Vec<u8> {
        let body: String = lines
            .iter()
            .map(|l| format!("<w:p><w:r><w:t>{}</w:t></w:r></w:p>", esc(l)))
            .collect();
        zip_of(&[
            (
                "word/document.xml",
                format!(
                    r#"<?xml version="1.0"?><w:document xmlns:w="w"><w:body>{body}</w:body></w:document>"#
                ),
            ),
            ("docProps/core.xml", core(title, author, created)),
            (
                "docProps/app.xml",
                "<Properties><Pages>3</Pages></Properties>".into(),
            ),
        ])
    }

    pub fn xlsx(cells: &[&str]) -> Vec<u8> {
        let si: String = cells
            .iter()
            .map(|c| format!("<si><t>{}</t></si>", esc(c)))
            .collect();
        zip_of(&[(
            "xl/sharedStrings.xml",
            format!(r#"<?xml version="1.0"?><sst xmlns="x">{si}</sst>"#),
        )])
    }

    pub fn pptx(slides: &[&str]) -> Vec<u8> {
        let entries: Vec<(String, String)> = slides
            .iter()
            .enumerate()
            .map(|(i, s)| {
                (
                    format!("ppt/slides/slide{}.xml", i + 1),
                    format!(
                        r#"<p:sld xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>{}</a:t></a:r></a:p></p:sld>"#,
                        esc(s)
                    ),
                )
            })
            .collect();
        let refs: Vec<(&str, String)> = entries
            .iter()
            .map(|(n, c)| (n.as_str(), c.clone()))
            .collect();
        zip_of(&refs)
    }

    /// OLE-Signatur wie bei einer verschlüsselten Office-Datei.
    pub fn ole_container() -> Vec<u8> {
        let mut v = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
        v.extend([0u8; 512]);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn run(data: &[u8], ext: &str) -> Result<Extracted, OfficeError> {
        extract_bytes(data, ext, 200_000)
    }

    #[test]
    fn docx_text_und_metadaten() {
        let data = docx(
            &["Rechnung Nr. 4711", "Gesamtbetrag 123,45 € & Co <x>"],
            "Rechnung",
            "Max Muster",
            "2026-09-30T10:00:00Z",
        );
        let out = run(&data, "docx").unwrap();
        let text = out.text.unwrap();
        assert_eq!(text, "Rechnung Nr. 4711\nGesamtbetrag 123,45 € & Co <x>\n");
        assert_eq!(out.text_source, TextSource::Office);
        assert_eq!(out.meta["doc.title"], "Rechnung");
        assert_eq!(out.meta["doc.author"], "Max Muster");
        assert_eq!(out.meta["doc.created"], "2026-09-30");
        assert_eq!(out.meta["doc.pages"], "3");
        assert_eq!(out.pages, Some(3));
    }

    #[test]
    fn xlsx_und_pptx_text_in_richtiger_reihenfolge() {
        let x = run(&xlsx(&["Kontoauszug", "IBAN DE89"]), "xlsx").unwrap();
        assert_eq!(x.text.unwrap(), "Kontoauszug\nIBAN DE89\n");
        let slides: Vec<String> = (1..=11).map(|i| format!("Folie {i}")).collect();
        let refs: Vec<&str> = slides.iter().map(String::as_str).collect();
        let p = run(&pptx(&refs), "pptx").unwrap().text.unwrap();
        let lines: Vec<&str> = p.lines().collect();
        assert_eq!(lines[1], "Folie 2");
        assert_eq!(lines[9], "Folie 10", "slide10 kommt nach slide9");
    }

    #[test]
    fn ole_container_ist_verschluesselt() {
        assert_eq!(
            run(&ole_container(), "docx").unwrap_err(),
            OfficeError::Encrypted
        );
    }

    #[test]
    fn kaputtes_zip_und_kaputtes_xml() {
        assert_eq!(
            run(b"PK\x03\x04kaputt", "docx").unwrap_err(),
            OfficeError::Corrupt
        );
        assert_eq!(run(b"", "docx").unwrap_err(), OfficeError::Corrupt);
        let bad = zip_of(&[("word/document.xml", "<w:p><w:t>offen".into())]);
        // Abgeschnittenes XML: Text bis dahin ist egal, es darf nur nicht abstürzen.
        let _ = run(&bad, "docx");
        let bad2 = zip_of(&[("word/document.xml", "<<<>>>&&&".into())]);
        assert!(matches!(
            run(&bad2, "docx"),
            Ok(_) | Err(OfficeError::Corrupt)
        ));
    }

    #[test]
    fn zip_bombe_wird_abgelehnt() {
        // 60 MB Nullen komprimieren auf wenige KB: Verhältnis weit über 100.
        let bomb = zip_of(&[("word/document.xml", "0".repeat(60 * 1024 * 1024))]);
        assert_eq!(run(&bomb, "docx").unwrap_err(), OfficeError::TooLarge);
        let ratio = zip_of(&[("word/document.xml", "0".repeat(20 * 1024 * 1024))]);
        assert_eq!(run(&ratio, "docx").unwrap_err(), OfficeError::TooLarge);
    }

    #[test]
    fn zu_viele_eintraege() {
        let entries: Vec<(String, String)> = (0..2_100)
            .map(|i| (format!("f{i}.txt"), "x".into()))
            .collect();
        let refs: Vec<(&str, String)> = entries
            .iter()
            .map(|(n, c)| (n.as_str(), c.clone()))
            .collect();
        assert_eq!(
            run(&zip_of(&refs), "docx").unwrap_err(),
            OfficeError::TooLarge
        );
    }

    #[test]
    fn text_wird_auf_max_zeichen_begrenzt() {
        let lines: Vec<String> = (0..500).map(|i| format!("Zeile Nummer {i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let data = docx(&refs, "", "", "");
        let out = extract_bytes(&data, "docx", 100).unwrap();
        assert!(out.text.unwrap().chars().count() <= 100);
    }

    #[test]
    fn datei_extraktion_und_verschluesselte_datei() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("a.docx");
        std::fs::write(&ok, docx(&["Hallo"], "", "", "")).unwrap();
        let out = extract(&ok, "docx", &ExtractOpts::default());
        assert_eq!(out.status, Status::Ok);
        let enc = dir.path().join("b.docx");
        std::fs::write(&enc, ole_container()).unwrap();
        assert_eq!(
            extract(&enc, "docx", &ExtractOpts::default()).status,
            Status::Unreadable("encrypted".into())
        );
    }

    #[test]
    fn unplausible_daten_werden_verworfen() {
        assert_eq!(
            w3cdtf_date("2026-09-30T10:00:00Z").as_deref(),
            Some("2026-09-30")
        );
        assert_eq!(w3cdtf_date("0001-01-01T00:00:00Z"), None);
        assert_eq!(w3cdtf_date("müll"), None);
    }
}
