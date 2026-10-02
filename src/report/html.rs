//! Einzelne HTML-Datei ohne externe Abhängigkeiten. Die Daten stehen als JSON im Dokument,
//! ein eingebettetes Skript rendert sie (Textknoten, nie HTML).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use askama::Template;

use super::Report;

pub const FILE_NAME: &str = "report.html";

const CSS: &str = include_str!("../../assets/report.css");
const JS: &str = include_str!("../../assets/report.js");

pub struct Section {
    pub id: &'static str,
    pub title: &'static str,
}

pub const SECTIONS: [Section; 11] = [
    Section {
        id: "overview",
        title: "Übersicht",
    },
    Section {
        id: "tree",
        title: "Größenbaum",
    },
    Section {
        id: "top",
        title: "Top-Listen",
    },
    Section {
        id: "types",
        title: "Dateitypen",
    },
    Section {
        id: "age",
        title: "Alter",
    },
    Section {
        id: "duplicates",
        title: "Exakte Duplikate",
    },
    Section {
        id: "probable",
        title: "Wahrscheinliche Duplikate",
    },
    Section {
        id: "similar",
        title: "Ähnliche Dateien",
    },
    Section {
        id: "structure",
        title: "Struktur",
    },
    Section {
        id: "problems",
        title: "Problemdateien",
    },
    Section {
        id: "errors",
        title: "Fehler",
    },
];

#[derive(Template)]
#[template(path = "report.html")]
struct Page<'a> {
    title: String,
    root: &'a str,
    sections: &'a [Section],
    notice: &'a str,
    version: &'a str,
    css: &'a str,
    js: &'a str,
    data: &'a str,
}

/// JSON für den Einbau in `<script type="application/json">`: `<` wird escaped,
/// damit weder `</script>` noch `<!--` im Dokument entstehen kann.
fn embed_json(report: &Report) -> Result<String> {
    Ok(serde_json::to_string(report)?
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029"))
}

pub fn render(report: &Report) -> Result<String> {
    let data = embed_json(report)?;
    let page = Page {
        title: format!("Analysebericht – {}", report.meta.root),
        root: &report.meta.root,
        sections: &SECTIONS,
        notice: &report.meta.notice,
        version: &report.meta.tool_version,
        css: CSS,
        js: JS,
        data: &data,
    };
    Ok(page.render()?)
}

pub fn write(report: &Report, dir: &Path) -> Result<PathBuf> {
    let path = dir.join(FILE_NAME);
    std::fs::write(&path, render(report)?)
        .with_context(|| format!("{} konnte nicht geschrieben werden", path.display()))?;
    Ok(path)
}
