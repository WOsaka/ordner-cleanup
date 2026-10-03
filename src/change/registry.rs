//! Register der Läufe (`runs.jsonl` im Datenordner): ein reiner Finder, damit `undo <run-id>`
//! die Wurzel kennt. Maßgeblich bleibt das Journal in der Wurzel; fehlt das Register oder
//! ist es unvollständig, hilft `--root`.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::RunId;
use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run: RunId,
    pub root: String,
    pub at: String,
}

pub fn append(file: &Path, record: &RunRecord) -> io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(record).map_err(io::Error::other)?;
    line.push('\n');
    let mut out = OpenOptions::new().append(true).create(true).open(file)?;
    out.write_all(line.as_bytes())?;
    out.sync_all()
}

/// Alle lesbaren Einträge; eine fehlende Datei oder kaputte Zeilen ergeben einfach weniger.
pub fn read(file: &Path) -> Vec<RunRecord> {
    let Ok(bytes) = std::fs::read(file) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Wurzel eines Laufs; bei mehreren Einträgen gilt der letzte.
pub fn find_root(file: &Path, run: &RunId) -> Option<String> {
    read(file)
        .into_iter()
        .rev()
        .find(|r| &r.run == run)
        .map(|r| r.root)
}

/// Alle bekannten Wurzeln, ohne Duplikate (Groß-/Kleinschreibung egal), älteste zuerst.
pub fn known_roots(file: &Path) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    read(file)
        .into_iter()
        .filter(|r| seen.insert(paths::path_key(Path::new(&r.root))))
        .map(|r| r.root)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(run: &str, root: &str) -> RunRecord {
        RunRecord {
            run: RunId::parse(run).unwrap(),
            root: root.into(),
            at: "2026-10-03T10:00:00Z".into(),
        }
    }

    #[test]
    fn eintraege_werden_angehaengt_und_gelesen() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("runs.jsonl");
        append(&file, &record("20261003-100000-aaaa", r"D:\A")).unwrap();
        append(&file, &record("20261003-110000-bbbb", r"D:\B")).unwrap();
        let all = read(&file);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1], record("20261003-110000-bbbb", r"D:\B"));
    }

    #[test]
    fn fehlende_datei_und_kaputte_zeilen_sind_unkritisch() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("runs.jsonl");
        assert!(read(&file).is_empty());
        append(&file, &record("20261003-100000-aaaa", r"D:\A")).unwrap();
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str("kaputt\n{\"run\":\"nicht-gueltig\",\"root\":\"x\",\"at\":\"y\"}\n");
        std::fs::write(&file, text).unwrap();
        assert_eq!(read(&file).len(), 1);
    }

    #[test]
    fn find_root_nimmt_den_letzten_eintrag() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("runs.jsonl");
        let run = RunId::parse("20261003-100000-aaaa").unwrap();
        assert_eq!(find_root(&file, &run), None);
        append(&file, &record("20261003-100000-aaaa", r"D:\Alt")).unwrap();
        append(&file, &record("20261003-100000-aaaa", r"D:\Neu")).unwrap();
        append(&file, &record("20261003-110000-bbbb", r"D:\Andere")).unwrap();
        assert_eq!(find_root(&file, &run).as_deref(), Some(r"D:\Neu"));
    }

    #[test]
    fn known_roots_ohne_duplikate() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("runs.jsonl");
        append(&file, &record("20261003-100000-aaaa", r"D:\Daten")).unwrap();
        append(&file, &record("20261003-110000-bbbb", r"d:\daten")).unwrap();
        append(&file, &record("20261003-120000-cccc", r"E:\Foto")).unwrap();
        assert_eq!(known_roots(&file), [r"D:\Daten", r"E:\Foto"]);
    }
}
