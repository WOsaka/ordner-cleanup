//! Lauf-Protokoll eines Profils (`runs\<profil>.jsonl`): eine JSON-Zeile je `run`.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::report::history::short_date;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatus {
    /// Alles lief durch
    Ok,
    /// Scan mit Warnungen oder ein Plan ist gescheitert
    Partial,
    /// Ein anderer Scan hielt die Sperre
    SkippedLocked,
    /// Die Wurzel war nicht erreichbar
    RootMissing,
    /// Der Lauf ist gescheitert
    Failed,
}

impl RunStatus {
    /// Der Lauf ist ausgefallen: es gibt keinen neuen Stand.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::SkippedLocked | Self::RootMissing | Self::Failed)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Partial => "mit Warnungen",
            Self::SkippedLocked => "übersprungen (anderer Scan läuft)",
            Self::RootMissing => "Wurzel nicht erreichbar",
            Self::Failed => "gescheitert",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRecord {
    pub kind: String,
    pub actions: u64,
    pub path: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub started: String,
    pub ended: String,
    pub status: RunStatus,
    pub score: Option<u8>,
    pub score_delta: Option<i32>,
    pub report: Option<String>,
    pub plans: Vec<PlanRecord>,
    pub errors: Vec<String>,
    pub notified: bool,
}

impl RunRecord {
    /// Plan-Dateien mit mindestens einer Aktion.
    pub fn non_empty_plans(&self) -> usize {
        self.plans
            .iter()
            .filter(|p| p.error.is_none() && p.actions > 0)
            .count()
    }
}

pub fn append(path: &Path, record: &RunRecord) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(record).map_err(std::io::Error::other)?;
    writeln!(file, "{line}")
}

/// Alle lesbaren Einträge, älteste zuerst; kaputte Zeilen und eine fehlende Datei werden übergangen.
pub fn read_all(path: &Path) -> Vec<RunRecord> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Die ausgefallenen Läufe am Ende des Protokolls (seit dem letzten Lauf, der durchlief).
pub fn trailing_failures(records: &[RunRecord]) -> &[RunRecord] {
    let ok_until = records
        .iter()
        .rposition(|r| !r.status.is_failure())
        .map_or(0, |i| i + 1);
    &records[ok_until..]
}

fn reason(record: &RunRecord) -> String {
    match record.status {
        RunStatus::Failed => record
            .errors
            .first()
            .cloned()
            .unwrap_or_else(|| record.status.label().to_string()),
        other => other.label().to_string(),
    }
}

/// Hinweis für den nächsten Bericht, falls geplante Läufe ausgefallen sind.
pub fn missed_runs_note(records: &[RunRecord]) -> Option<String> {
    let failures = trailing_failures(records);
    let last = failures.last()?;
    let when = short_date(&last.started);
    Some(match failures.len() {
        1 => format!("Der letzte geplante Lauf am {when} ist ausgefallen: {}.", reason(last)),
        n => format!(
            "Seit dem letzten erfolgreichen Lauf sind {n} geplante Läufe ausgefallen (zuletzt am {when}: {}).",
            reason(last)
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(status: RunStatus) -> RunRecord {
        RunRecord {
            started: "2026-10-03T08:00:00Z".into(),
            ended: "2026-10-03T08:01:00Z".into(),
            status,
            score: Some(70),
            score_delta: Some(-2),
            report: Some(r"C:\r\latest.html".into()),
            plans: vec![PlanRecord {
                kind: "junk".into(),
                actions: 3,
                path: Some(r"C:\p.json".into()),
                error: None,
            }],
            errors: vec![],
            notified: false,
        }
    }

    #[test]
    fn anhaengen_und_lesen_ueberspringt_kaputte_zeilen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runs").join("p.jsonl");
        assert!(read_all(&path).is_empty());
        append(&path, &rec(RunStatus::Ok)).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{kaputt\n")
            .unwrap();
        append(&path, &rec(RunStatus::RootMissing)).unwrap();
        let all = read_all(&path);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], rec(RunStatus::Ok));
        assert_eq!(all[1].status, RunStatus::RootMissing);
    }

    #[test]
    fn status_wird_kebab_serialisiert() {
        let json = serde_json::to_string(&rec(RunStatus::SkippedLocked)).unwrap();
        assert!(json.contains(r#""status":"skipped-locked""#), "{json}");
    }

    #[test]
    fn ausgefallene_laeufe_am_ende() {
        let mut v = vec![
            rec(RunStatus::Ok),
            rec(RunStatus::Failed),
            rec(RunStatus::SkippedLocked),
        ];
        assert_eq!(trailing_failures(&v).len(), 2);
        v.push(rec(RunStatus::Partial));
        assert!(trailing_failures(&v).is_empty());
        assert!(missed_runs_note(&v).is_none());
        assert!(missed_runs_note(&[]).is_none());
    }

    #[test]
    fn hinweis_nennt_anzahl_und_grund() {
        let mut failed = rec(RunStatus::Failed);
        failed.errors = vec!["Scan abgebrochen".into()];
        let one = missed_runs_note(&[rec(RunStatus::Ok), failed.clone()]).unwrap();
        assert!(one.contains("ist ausgefallen: Scan abgebrochen."), "{one}");
        let two = missed_runs_note(&[rec(RunStatus::RootMissing), failed]).unwrap();
        assert!(two.contains("2 geplante Läufe"), "{two}");
        let root = missed_runs_note(&[rec(RunStatus::RootMissing)]).unwrap();
        assert!(root.contains("Wurzel nicht erreichbar"), "{root}");
    }

    #[test]
    fn nicht_leere_plaene_werden_gezaehlt() {
        let mut r = rec(RunStatus::Ok);
        assert_eq!(r.non_empty_plans(), 1);
        r.plans.push(PlanRecord {
            kind: "rules".into(),
            actions: 0,
            path: None,
            error: None,
        });
        r.plans.push(PlanRecord {
            kind: "versions".into(),
            actions: 9,
            path: None,
            error: Some("kaputt".into()),
        });
        assert_eq!(r.non_empty_plans(), 1);
    }
}
