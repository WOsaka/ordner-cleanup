//! Report-Modell: wird ausschließlich aus dem Index gebaut und in mehrere Formate geschrieben.

mod build;
pub mod content;
pub mod csv;
mod folder;
pub mod history;
pub mod html;
pub mod json;
pub(crate) mod rows;
pub mod template;
pub mod terminal;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_folder;
#[cfg(test)]
mod tests_html;
#[cfg(test)]
mod tests_more;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::index::{Index, RootInfo, RootStatus};
use crate::paths;

pub use build::{build, ReportParams};

pub const CONFIDENTIALITY_NOTICE: &str = "Dieser Bericht enthält Datei- und Ordnernamen sowie Metadaten und kann vertrauliche Informationen preisgeben. Die Weitergabe liegt in der Verantwortung des Nutzers.";

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub meta: Meta,
    pub overview: Overview,
    pub size_tree: TreeNode,
    pub top_files: Vec<FileItem>,
    pub top_dirs: Vec<DirItem>,
    pub file_types: FileTypes,
    pub age: AgeReport,
    pub duplicates: DuplicatesReport,
    pub folder_duplicates: FolderDuplicatesReport,
    pub probable_duplicates: Vec<ProbableGroup>,
    pub similar: Vec<SimilarGroup>,
    pub structure: Vec<StructureItem>,
    pub problems: Vec<ProblemItem>,
    pub errors: Vec<ErrorItem>,
    /// Verlauf und Health-Score; der Aufrufer setzt ihn nach `build`
    pub history: Option<history::HistorySection>,
    /// Soll/Ist-Abgleich mit einer Vorlage; der Aufrufer setzt ihn nach `build`
    pub template: Option<template::TemplateSection>,
    /// Inhalte (Kategorien, Zum Prüfen, Fotos); der Aufrufer setzt ihn nach `build`
    pub content: Option<content::ContentSection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    pub tool_version: String,
    pub generated_at: String,
    pub root: String,
    pub scanned_at: Option<String>,
    /// `complete`, `aborted` oder `running`
    pub scan_status: String,
    pub top: usize,
    pub old_after_days: i64,
    pub max_depth_warning: usize,
    pub huge_dir_entries: usize,
    pub notice: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub files: u64,
    pub dirs: u64,
    pub total_size: u64,
    pub summary_size: u64,
    pub summary_files: u64,
    /// Anteil „Nur Summe“ an der Gesamtgröße (0.0 bis 1.0)
    pub summary_share: f64,
    pub cloud_only_files: u64,
    pub cloud_only_size: u64,
    pub hidden_files: u64,
    pub system_files: u64,
    pub links: u64,
    pub error_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TreeNode {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub files: u64,
    pub summary: bool,
    pub link: bool,
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileItem {
    pub path: String,
    pub size: u64,
    /// Unix-Sekunden
    pub mtime: i64,
    pub age_days: i64,
    pub hidden: bool,
    pub system: bool,
    pub cloud_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DirItem {
    pub path: String,
    pub size: u64,
    pub files: u64,
    pub summary: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TypeStat {
    pub key: String,
    pub count: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileTypes {
    pub by_extension: Vec<TypeStat>,
    pub by_category: Vec<TypeStat>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgeStat {
    pub label: String,
    pub count: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgeReport {
    pub classes: Vec<AgeStat>,
    pub old_after_days: i64,
    pub old_total_size: u64,
    pub old_files: Vec<FileItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DupFileItem {
    pub path: String,
    pub mtime: i64,
    pub nlinks: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DupGroupItem {
    pub size: u64,
    pub instances: usize,
    pub wasted: u64,
    pub files: Vec<DupFileItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicatesReport {
    pub group_count: usize,
    pub total_wasted: u64,
    pub groups: Vec<DupGroupItem>,
}

/// Gruppe gleicher Ordner (gleicher Inhalt an gleichen Pfaden, Junk ignoriert).
#[derive(Debug, Clone, Serialize)]
pub struct FolderDupGroup {
    pub dirs: Vec<String>,
    /// Verglichene Dateien je Ordner
    pub files: u64,
    /// Bytes je Ordner
    pub bytes: u64,
    /// `bytes` mal (Anzahl − 1), ohne Schutzregeln und Hardlinks
    pub reclaimable: u64,
}

/// Zwei Ordner mit vielen gemeinsamen Dateien, aber nicht gleich (nur Hinweis).
#[derive(Debug, Clone, Serialize)]
pub struct PartialPair {
    pub a: String,
    pub b: String,
    pub shared_bytes: u64,
    pub ratio: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderDuplicatesReport {
    /// Nach Platzgewinn absteigend
    pub groups: Vec<FolderDupGroup>,
    pub total_reclaimable: u64,
    /// Nach gemeinsamen Bytes absteigend, vollständig (das HTML zeigt die obersten 50)
    pub partial: Vec<PartialPair>,
    pub partial_threshold: f64,
    /// Die Paarbildung wurde wegen zu vieler Paare abgebrochen
    pub partial_incomplete: bool,
    /// Ordner, die sich wegen Cloud-Platzhaltern, Links oder fehlender Hashes nicht vergleichen ließen
    pub unverifiable: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbableFile {
    pub path: String,
    pub cloud_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbableGroup {
    pub name: String,
    pub size: u64,
    /// Immer `false`: Cloud-Platzhalter werden nie gelesen, der Inhalt ist nicht geprüft.
    pub verified: bool,
    pub files: Vec<ProbableFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimilarFile {
    pub name: String,
    pub size: u64,
    pub mtime: i64,
    pub exact_duplicate: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimilarGroup {
    pub dir: String,
    pub files: Vec<SimilarFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StructureItem {
    pub path: String,
    pub issue: String,
    pub label: String,
    pub direct_entries: u64,
    pub depth: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProblemItem {
    pub path: String,
    /// `file` oder `dir`
    pub kind: String,
    pub size: u64,
    pub problems: Vec<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorItem {
    pub path: String,
    pub kind: String,
    pub message: String,
}

/// Die Wurzel, für die ein Bericht erzeugt wird.
#[derive(Debug, Clone)]
pub struct ReportRoot {
    pub path: String,
    /// Ordner-Schlüssel (endet auf `\`)
    pub dir_key: String,
    pub scanned_at: Option<String>,
    pub status: RootStatus,
}

/// Wählt die Wurzel aus dem Index. Ein Unterordner einer gescannten Wurzel ist erlaubt.
pub fn select_root(index: &Index, requested: Option<&Path>) -> Result<ReportRoot> {
    let roots: Vec<RootInfo> = index.roots()?;
    match requested {
        Some(path) => {
            let key = paths::dir_key(path);
            let containing = roots
                .iter()
                .filter(|r| key.starts_with(&r.path_key))
                .max_by_key(|r| r.path_key.len());
            match containing {
                Some(r) => Ok(ReportRoot {
                    path: paths::display(path),
                    dir_key: key,
                    scanned_at: r.finished_at.clone().or_else(|| r.started_at.clone()),
                    status: r.status,
                }),
                None => bail!(
                    "Kein Scan für {} vorhanden – zuerst `scan` ausführen",
                    paths::display(path)
                ),
            }
        }
        None => match roots.as_slice() {
            [] => bail!("Kein Scan vorhanden – zuerst `scan` ausführen"),
            [r] => Ok(ReportRoot {
                path: r.path.clone(),
                dir_key: r.path_key.clone(),
                scanned_at: r.finished_at.clone().or_else(|| r.started_at.clone()),
                status: r.status,
            }),
            many => {
                let list: Vec<String> = many.iter().map(|r| format!("  {}", r.path)).collect();
                bail!(
                    "Mehrere gescannte Wurzeln vorhanden, bitte eine auswählen:\n{}",
                    list.join("\n")
                )
            }
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Html,
    Json,
    Csv,
}

impl Format {
    pub fn parse_list(items: &[String]) -> Result<Vec<Format>> {
        if items.is_empty() {
            return Ok(vec![Format::Html, Format::Json, Format::Csv]);
        }
        let mut out = Vec::new();
        for item in items {
            let f = match item.trim().to_lowercase().as_str() {
                "html" => Format::Html,
                "json" => Format::Json,
                "csv" => Format::Csv,
                other => bail!("Unbekanntes Format '{other}' (erlaubt: html, json, csv)"),
            };
            if !out.contains(&f) {
                out.push(f);
            }
        }
        Ok(out)
    }
}

/// Legt den Berichtsordner `ordner-cleanup-report-<timestamp>` unter `out` an.
pub fn prepare_out_dir(out: Option<&Path>, timestamp: &str) -> Result<PathBuf> {
    let base = match out {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().context("Aktuelles Verzeichnis nicht lesbar")?,
    };
    let dir = base.join(format!("ordner-cleanup-report-{timestamp}"));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Zielordner nicht beschreibbar: {}", paths::display(&dir)))?;
    Ok(dir)
}

/// Schreibt die gewünschten Formate in `dir`; liefert die geschriebenen Dateien.
pub fn write_all(report: &Report, formats: &[Format], dir: &Path) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for format in formats {
        match format {
            Format::Json => written.push(json::write(report, dir)?),
            Format::Csv => written.extend(csv::write(report, dir)?),
            Format::Html => written.push(html::write(report, dir)?),
        }
    }
    Ok(written)
}
