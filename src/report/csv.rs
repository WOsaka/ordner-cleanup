//! CSV für deutsches Excel: `;`-getrennt, UTF-8 mit BOM.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{FileItem, Report};

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Excel wertet Zellen, die mit `=`, `+`, `-` oder `@` beginnen, als Formel aus.
/// Dateinamen sind nicht vertrauenswürdig, daher wird ein `'` vorangestellt.
fn safe(field: &str) -> String {
    if field.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{field}")
    } else {
        field.to_string()
    }
}

fn render(header: &[&str], rows: &[Vec<String>]) -> Result<Vec<u8>> {
    let mut out = BOM.to_vec();
    {
        let mut w = csv::WriterBuilder::new()
            .delimiter(b';')
            .from_writer(&mut out);
        w.write_record(header)?;
        for row in rows {
            w.write_record(row.iter().map(|f| safe(f)))?;
        }
        w.flush()?;
    }
    Ok(out)
}

fn file_rows(items: &[FileItem]) -> Vec<Vec<String>> {
    items
        .iter()
        .map(|f| {
            vec![
                f.path.clone(),
                f.size.to_string(),
                f.mtime.to_string(),
                f.age_days.to_string(),
                f.hidden.to_string(),
                f.system.to_string(),
                f.cloud_only.to_string(),
            ]
        })
        .collect()
}

const FILE_HEADER: [&str; 7] = [
    "Pfad",
    "Größe (Bytes)",
    "Geändert (Unix-Sekunden)",
    "Alter (Tage)",
    "Versteckt",
    "System",
    "Nur Cloud",
];

/// Alle Listen als `(Dateiname, Inhalt)`.
pub fn render_all(report: &Report) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let mut duplicates = Vec::new();
    for (i, g) in report.duplicates.groups.iter().enumerate() {
        for f in &g.files {
            duplicates.push(vec![
                (i + 1).to_string(),
                g.size.to_string(),
                g.wasted.to_string(),
                g.instances.to_string(),
                f.path.clone(),
                f.mtime.to_string(),
                f.nlinks.to_string(),
            ]);
        }
    }
    let mut similar = Vec::new();
    for (i, g) in report.similar.iter().enumerate() {
        for f in &g.files {
            similar.push(vec![
                (i + 1).to_string(),
                g.dir.clone(),
                f.name.clone(),
                f.size.to_string(),
                f.mtime.to_string(),
                f.exact_duplicate.to_string(),
            ]);
        }
    }
    let problems: Vec<Vec<String>> = report
        .problems
        .iter()
        .map(|p| {
            vec![
                p.path.clone(),
                p.kind.clone(),
                p.size.to_string(),
                p.labels.join(", "),
            ]
        })
        .collect();
    let empty_dirs: Vec<Vec<String>> = report
        .structure
        .iter()
        .filter(|s| s.issue == "empty")
        .map(|s| vec![s.path.clone(), s.depth.to_string()])
        .collect();
    let errors: Vec<Vec<String>> = report
        .errors
        .iter()
        .map(|e| vec![e.path.clone(), e.kind.clone(), e.message.clone()])
        .collect();

    let content: Vec<Vec<String>> = report
        .content
        .iter()
        .flat_map(|c| c.files.iter())
        .map(|f| {
            vec![
                f.path.clone(),
                f.status.clone(),
                f.category.clone().unwrap_or_default(),
                f.confidence.map(|c| format!("{c:.2}")).unwrap_or_default(),
                f.source.clone().unwrap_or_default(),
                f.doc_date.clone().unwrap_or_default(),
                f.doc_sender.clone().unwrap_or_default(),
                f.doc_number.clone().unwrap_or_default(),
                f.doc_amount.clone().unwrap_or_default(),
                f.doc_title.clone().unwrap_or_default(),
                f.review.to_string(),
            ]
        })
        .collect();

    Ok(vec![
        (
            "content.csv",
            render(
                &[
                    "Pfad",
                    "Status",
                    "Kategorie",
                    "Konfidenz",
                    "Quelle",
                    "Datum",
                    "Absender",
                    "Nummer",
                    "Betrag",
                    "Titel",
                    "Zum Prüfen",
                ],
                &content,
            )?,
        ),
        (
            "duplicates.csv",
            render(
                &[
                    "Gruppe",
                    "Größe (Bytes)",
                    "Verschwendet (Bytes)",
                    "Instanzen",
                    "Pfad",
                    "Geändert (Unix-Sekunden)",
                    "Hardlinks",
                ],
                &duplicates,
            )?,
        ),
        (
            "similar.csv",
            render(
                &[
                    "Gruppe",
                    "Ordner",
                    "Name",
                    "Größe (Bytes)",
                    "Geändert (Unix-Sekunden)",
                    "Exaktes Duplikat",
                ],
                &similar,
            )?,
        ),
        (
            "problems.csv",
            render(&["Pfad", "Art", "Größe (Bytes)", "Probleme"], &problems)?,
        ),
        (
            "old-files.csv",
            render(&FILE_HEADER, &file_rows(&report.age.old_files))?,
        ),
        (
            "top-files.csv",
            render(&FILE_HEADER, &file_rows(&report.top_files))?,
        ),
        ("empty-dirs.csv", render(&["Pfad", "Tiefe"], &empty_dirs)?),
        ("errors.csv", render(&["Pfad", "Art", "Meldung"], &errors)?),
    ])
}

pub fn write(report: &Report, dir: &Path) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (name, content) in render_all(report)? {
        let path = dir.join(name);
        std::fs::write(&path, content)
            .with_context(|| format!("{} konnte nicht geschrieben werden", path.display()))?;
        written.push(path);
    }
    Ok(written)
}
