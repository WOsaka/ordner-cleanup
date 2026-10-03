//! Write-ahead-Journal (JSON Lines): ein Eintrag je Zeile, `fsync` nach jedem Eintrag.
//!
//! Ein `intent` ohne `done` heißt: Der Move kann passiert sein oder nicht. Undo schaut
//! deshalb auf den echten Dateizustand statt nur dem Journal zu vertrauen.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{RunId, SkipReason};
use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndStatus {
    Complete,
    Partial,
}

/// Wohin ein Move ging; steuert in `undo`, unter welchem Präfix `to` liegen muss, und ob
/// `purge` die Aktion unwiederbringlich macht.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dest {
    /// Phase-2-Journale kennen nur die Quarantäne.
    #[default]
    Quarantine,
    Archive,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Entry {
    RunStart {
        run: RunId,
        plan: String,
        root: String,
        started: String,
    },
    /// Vor dem Move geschrieben; `to` ist das tatsächliche Ziel (Quarantäne oder `_Archiv`).
    Intent {
        run: RunId,
        action: u32,
        from: String,
        to: String,
        size: u64,
        /// Nur `dedupe`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
        #[serde(default)]
        dest: Dest,
        /// Ein ganzer Ordner wurde verschoben; `size` ist dann die Summe.
        #[serde(default, skip_serializing_if = "is_false")]
        is_dir: bool,
    },
    /// Vor dem Entfernen eines leeren Ordners geschrieben. Attribute und Zeiten stammen vom
    /// Zustand unmittelbar vor dem Entfernen und dienen dem Undo zur Wiederherstellung.
    IntentRemoveDir {
        run: RunId,
        action: u32,
        path: String,
        attrs: u32,
        mtime_ticks: i64,
        /// Erstellungszeit.
        ctime_ticks: i64,
    },
    Done {
        run: RunId,
        action: u32,
    },
    Skip {
        run: RunId,
        action: u32,
        reason: SkipReason,
    },
    Fail {
        run: RunId,
        action: u32,
        error: String,
    },
    RunEnd {
        run: RunId,
        status: EndStatus,
    },
    UndoStart {
        run: RunId,
        started: String,
    },
    UndoDone {
        run: RunId,
        action: u32,
    },
    UndoConflict {
        run: RunId,
        action: u32,
        reason: String,
    },
    UndoEnd {
        run: RunId,
        status: EndStatus,
    },
    /// Quarantäne wurde endgültig gelöscht; das Journal bleibt als Nachweis.
    Purged {
        run: RunId,
        at: String,
    },
}

impl Entry {
    pub fn run(&self) -> &RunId {
        match self {
            Self::RunStart { run, .. }
            | Self::Intent { run, .. }
            | Self::IntentRemoveDir { run, .. }
            | Self::Done { run, .. }
            | Self::Skip { run, .. }
            | Self::Fail { run, .. }
            | Self::RunEnd { run, .. }
            | Self::UndoStart { run, .. }
            | Self::UndoDone { run, .. }
            | Self::UndoConflict { run, .. }
            | Self::UndoEnd { run, .. }
            | Self::Purged { run, .. } => run,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("Journal nicht lesbar: {0}")]
    Io(#[from] io::Error),
    #[error("Journal ist in Zeile {line} beschädigt: {message}")]
    Corrupt { line: usize, message: String },
}

pub struct JournalWriter {
    file: File,
}

impl JournalWriter {
    /// Legt ein neues Journal an; ein vorhandenes wird nie überschrieben.
    pub fn create(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(paths::extended(parent))?;
        }
        let file = OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(paths::extended(path))?;
        Ok(Self { file })
    }

    /// Hängt an ein vorhandenes Journal an. Eine nach einem Absturz abgeschnittene letzte
    /// Zeile wird vorher entfernt, damit sie keine spätere Zeile verfälscht.
    pub fn open_append(path: &Path) -> io::Result<Self> {
        let path = paths::extended(path);
        let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut bytes = Vec::new();
        io::Read::read_to_end(&mut file, &mut bytes)?;
        if bytes.last().is_some_and(|b| *b != b'\n') {
            let keep = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            file.set_len(keep as u64)?;
            file.sync_all()?;
        }
        drop(file);
        let file = OpenOptions::new().append(true).open(&path)?;
        Ok(Self { file })
    }

    pub fn append(&mut self, entry: &Entry) -> io::Result<()> {
        let mut line = serde_json::to_string(entry).map_err(io::Error::other)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.sync_all()
    }
}

/// Liest alle Einträge. Eine nicht lesbare **letzte** Zeile (Absturz beim Schreiben) wird
/// ignoriert, eine kaputte Zeile davor ist ein Fehler.
pub fn read(path: &Path) -> Result<Vec<Entry>, JournalError> {
    let bytes = std::fs::read(paths::extended(path))?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .collect();
    let mut entries = Vec::with_capacity(lines.len());
    for (i, (number, line)) in lines.iter().enumerate() {
        match serde_json::from_str::<Entry>(line) {
            Ok(entry) => entries.push(entry),
            Err(_) if i + 1 == lines.len() => {}
            Err(e) => {
                return Err(JournalError::Corrupt {
                    line: number + 1,
                    message: e.to_string(),
                })
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> RunId {
        RunId::parse("20261003-120000-ab12").unwrap()
    }

    fn intent(action: u32) -> Entry {
        Entry::Intent {
            run: run(),
            action,
            from: r"D:\Daten\a.txt".into(),
            to: r"D:\Daten\.ordner-cleanup\quarantine\r\a.txt".into(),
            size: 5,
            hash: Some("00".repeat(16)),
            dest: Dest::Quarantine,
            is_dir: false,
        }
    }

    fn remove_dir_intent(action: u32) -> Entry {
        Entry::IntentRemoveDir {
            run: run(),
            action,
            path: r"D:\Daten\leer".into(),
            attrs: 0x12,
            mtime_ticks: 17_000_000_000_000_000,
            ctime_ticks: 16_000_000_000_000_000,
        }
    }

    fn sample() -> Vec<Entry> {
        vec![
            Entry::RunStart {
                run: run(),
                plan: "plan.json".into(),
                root: r"D:\Daten".into(),
                started: "2026-10-03T10:00:00Z".into(),
            },
            intent(1),
            Entry::Done {
                run: run(),
                action: 1,
            },
            Entry::Skip {
                run: run(),
                action: 2,
                reason: SkipReason::Stale,
            },
            Entry::Fail {
                run: run(),
                action: 3,
                error: "gesperrt".into(),
            },
            Entry::RunEnd {
                run: run(),
                status: EndStatus::Partial,
            },
        ]
    }

    fn write_all(path: &Path, entries: &[Entry]) {
        let mut w = JournalWriter::create(path).unwrap();
        for e in entries {
            w.append(e).unwrap();
        }
    }

    #[test]
    fn eintraege_bleiben_in_reihenfolge_erhalten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j").join("r.jsonl");
        write_all(&path, &sample());
        assert_eq!(read(&path).unwrap(), sample());
    }

    #[test]
    fn eine_zeile_je_eintrag_mit_tag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        write_all(&path, &sample());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), sample().len());
        assert!(text
            .lines()
            .nth(1)
            .unwrap()
            .starts_with("{\"t\":\"intent\""));
        assert!(text.contains("\"reason\":\"stale\""));
    }

    #[test]
    fn abgeschnittene_letzte_zeile_wird_ignoriert() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        write_all(&path, &sample()[..3]);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"t\":\"intent\",\"run\":\"2026100");
        std::fs::write(&path, text).unwrap();
        assert_eq!(read(&path).unwrap(), sample()[..3]);
    }

    #[test]
    fn kaputte_zeile_mitten_in_der_datei_ist_ein_fehler() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        write_all(&path, &sample()[..2]);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("kaputt\n");
        text.push_str(&serde_json::to_string(&sample()[2]).unwrap());
        text.push('\n');
        std::fs::write(&path, text).unwrap();
        assert!(matches!(
            read(&path),
            Err(JournalError::Corrupt { line: 3, .. })
        ));
    }

    #[test]
    fn open_append_entfernt_abgeschnittene_zeile_vor_dem_anhaengen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        write_all(&path, &sample()[..2]);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"t\":\"done\",\"ru");
        std::fs::write(&path, text).unwrap();

        let mut w = JournalWriter::open_append(&path).unwrap();
        w.append(&Entry::UndoStart {
            run: run(),
            started: "t".into(),
        })
        .unwrap();
        let entries = read(&path).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[2], Entry::UndoStart { .. }));
    }

    #[test]
    fn create_ueberschreibt_nie() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        write_all(&path, &sample()[..1]);
        assert_eq!(
            JournalWriter::create(&path).err().unwrap().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(read(&path).unwrap().len(), 1);
    }

    /// Zeile, wie Phase 2 sie geschrieben hat: ohne `dest` und `is_dir`.
    const PHASE_2_INTENT: &str = r#"{"t":"intent","run":"20261003-120000-ab12","action":1,"from":"D:\\Daten\\a.txt","to":"D:\\Daten\\.ordner-cleanup\\quarantine\\r\\a.txt","size":5,"hash":"00000000000000000000000000000000"}"#;

    #[test]
    fn journal_zeile_aus_phase_2_wird_als_quarantaene_move_gelesen() {
        let entry: Entry = serde_json::from_str(PHASE_2_INTENT).unwrap();
        let Entry::Intent {
            dest,
            is_dir,
            hash,
            size,
            ..
        } = entry
        else {
            panic!("kein Intent");
        };
        assert_eq!(dest, Dest::Quarantine);
        assert!(!is_dir);
        assert_eq!(hash.as_deref(), Some("0".repeat(32).as_str()));
        assert_eq!(size, 5);
    }

    #[test]
    fn phase_2_journal_datei_bleibt_lesbar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        std::fs::write(
            &path,
            format!(
                "{PHASE_2_INTENT}
"
            ),
        )
        .unwrap();
        assert_eq!(read(&path).unwrap().len(), 1);
    }

    #[test]
    fn archiv_intent_ohne_hash_serialisiert_dest_und_laesst_hash_weg() {
        let entry = Entry::Intent {
            run: run(),
            action: 2,
            from: r"D:\Daten\alt".into(),
            to: r"D:\Daten\_Archiv\2020\alt".into(),
            size: 99,
            hash: None,
            dest: Dest::Archive,
            is_dir: true,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"dest\":\"archive\""), "{json}");
        assert!(json.contains("\"is_dir\":true"), "{json}");
        assert!(!json.contains("\"hash\""), "{json}");
        assert_eq!(serde_json::from_str::<Entry>(&json).unwrap(), entry);
    }

    #[test]
    fn intent_remove_dir_hat_eigenen_tag_und_laeuft_durch_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        write_all(&path, &[remove_dir_intent(4)]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("{\"t\":\"intent_remove_dir\""), "{text}");
        let entries = read(&path).unwrap();
        assert_eq!(entries, vec![remove_dir_intent(4)]);
        assert_eq!(entries[0].run(), &run());
    }

    #[test]
    fn leeres_journal_ist_leer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        std::fs::write(&path, "").unwrap();
        assert!(read(&path).unwrap().is_empty());
    }
}
