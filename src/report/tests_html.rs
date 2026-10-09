use super::html::{self, SECTIONS};
use super::tests::{fixture, report_of};
use super::*;

fn page() -> String {
    html::render(&report_of(&fixture())).unwrap()
}

/// Inhalt des eingebetteten JSON-Blocks.
fn data_block(html: &str) -> &str {
    let start_tag = r#"<script type="application/json" id="report-data">"#;
    let start = html.find(start_tag).unwrap() + start_tag.len();
    let end = start + html[start..].find("</script>").unwrap();
    &html[start..end]
}

#[test]
fn keine_externen_referenzen() {
    // Der SVG-Namensraum der Trendgrafiken ist ein Bezeichner, keine Ressource.
    let html = page().replace("http://www.w3.org/2000/svg", "");
    for forbidden in [
        "http://",
        "https://",
        "ftp:",
        "//cdn",
        "url(",
        "@import",
        "<link",
        "<img",
        "<iframe",
        "<object",
        "<embed",
        " src=",
        "<form",
        "fetch(",
        "XMLHttpRequest",
        "WebSocket",
        "sendBeacon",
        "import(",
    ] {
        assert!(!html.contains(forbidden), "enthält '{forbidden}'");
    }
    // Links dürfen nur auf Anker im Dokument zeigen.
    for (i, _) in html.match_indices("href=") {
        assert!(
            html[i..].starts_with("href=\"#"),
            "externer Link: {}",
            &html[i..i + 30]
        );
    }
}

#[test]
fn alle_abschnitte_sind_vorhanden() {
    let html = page();
    assert_eq!(SECTIONS.len(), 15);
    for s in SECTIONS {
        assert!(
            html.contains(&format!(r#"<section id="{}""#, s.id)),
            "Abschnitt {}",
            s.id
        );
        assert!(html.contains(&format!(r##"<a href="#{}">{}</a>"##, s.id, s.title)));
        assert!(html.contains(&format!(r#"data-slot="{}""#, s.id)));
        assert!(
            html.contains(&format!("slot('{}')", s.id)) || s.id == "tree" || s.id == "overview",
            "Skript füllt {} nicht",
            s.id
        );
    }
    assert!(html.contains(CONFIDENTIALITY_NOTICE));
    assert!(html.contains(r#"<html lang="de">"#));
}

#[test]
fn eingebettetes_json_ist_der_report() {
    let report = report_of(&fixture());
    let html = html::render(&report).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(data_block(&html)).unwrap();
    assert_eq!(parsed, serde_json::to_value(&report).unwrap());
}

#[test]
fn gefaehrliche_zeichen_in_pfaden_werden_neutralisiert() {
    let mut report = report_of(&fixture());
    let evil = r#"C:\x\</script><img src=x onerror=alert(1)>&"'<!-- \u2028"#;
    report.top_files[0].path = evil.to_string();
    report.errors.push(ErrorItem {
        path: evil.to_string(),
        kind: "io".into(),
        message: "</SCRIPT>".into(),
    });
    report.meta.root = r#"C:\a&b<"x">"#.to_string();
    let html = html::render(&report).unwrap();

    // Genau zwei Skript-Blöcke (Daten und Code) und das Daten-JSON enthält kein '<'.
    assert_eq!(html.matches("</script>").count(), 2);
    assert!(!html.contains("<img src=x"));
    assert!(!data_block(&html).contains('<'));
    let parsed: serde_json::Value = serde_json::from_str(data_block(&html)).unwrap();
    assert_eq!(parsed["top_files"][0]["path"], evil);
    assert_eq!(parsed["errors"][0]["message"], "</SCRIPT>");

    // Titel und Kopfzeile sind HTML-escaped.
    assert!(
        html.contains("<title>Analysebericht – C:\\a&#38;b&#60;&#34;x&#34;&#62;</title>")
            || html.contains("<title>Analysebericht – C:\\a&amp;b&lt;&quot;x&quot;&gt;</title>")
    );
    assert!(!html.contains(r#"a&b<"x">"#));
}

#[test]
fn skript_rendert_nur_ueber_textknoten() {
    // Das Skript darf keine HTML-Sinks verwenden, sonst wären Dateinamen ein Angriffsweg.
    for sink in [
        "innerHTML",
        "outerHTML",
        "insertAdjacentHTML",
        "document.write",
        "eval(",
        "new Function",
    ] {
        assert!(
            !include_str!("../../assets/report.js").contains(sink),
            "{sink}"
        );
    }
}

#[test]
fn write_all_schreibt_alle_drei_formate() {
    let report = report_of(&fixture());
    let out = tempfile::tempdir().unwrap();
    let written = write_all(&report, &Format::parse_list(&[]).unwrap(), out.path()).unwrap();
    assert_eq!(written.len(), 1 + 1 + 10);
    assert!(out.path().join("report.html").is_file());
    assert!(out.path().join("report.json").is_file());
    assert!(out.path().join("errors.csv").is_file());
    let size = std::fs::metadata(out.path().join("report.html"))
        .unwrap()
        .len();
    assert!(size > 10_000 && size < 2_000_000, "{size}");
}

/// Manuell: `ORDNER_CLEANUP_SAMPLE_ROOT=<ordner> ORDNER_CLEANUP_SAMPLE_OUT=<ordner> cargo test sample_report -- --ignored`
/// scannt einen echten Ordner und schreibt den Bericht zur Sichtprüfung im Browser.
#[test]
#[ignore]
fn sample_report() {
    use super::tests::{params, run_scan};
    let (Some(root), Some(out)) = (
        std::env::var_os("ORDNER_CLEANUP_SAMPLE_ROOT"),
        std::env::var_os("ORDNER_CLEANUP_SAMPLE_OUT"),
    ) else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let mut index = Index::open_in_memory().unwrap();
    run_scan(&mut index, &crate::scan::source::StdDirSource, &root);
    let selected = select_root(&index, Some(&root)).unwrap();
    let report = build(&index, &selected, &params(50)).unwrap();
    let dir = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&dir).unwrap();
    write_all(&report, &Format::parse_list(&[]).unwrap(), &dir).unwrap();
    println!("{}", terminal::render(&report));
}
