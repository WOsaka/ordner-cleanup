//! Plan-Datei (JSON): Datenmodell, Laden mit Versionsprüfung und Validierung.

use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::SkipReason;
use crate::paths;
use crate::scan::source::TICKS_PER_SEC;

pub const PLAN_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("Plan-Datei nicht lesbar: {0}")]
    Unreadable(String),
    #[error("Plan-Datei ist beschädigt: {0}")]
    Corrupt(String),
    #[error("Plan hat Version {found}, unterstützt wird nur Version {expected}")]
    UnsupportedVersion { found: u64, expected: u32 },
    #[error("Plan ist ungültig: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanKind {
    Dedupe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionType {
    Quarantine,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub created: String,
    pub kind: PlanKind,
    pub root: String,
    pub keep_strategy: String,
    pub actions: Vec<PlannedAction>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedAction {
    pub id: u32,
    #[serde(rename = "type")]
    pub action: ActionType,
    pub path: String,
    pub size: u64,
    /// Maßgeblich für die Stale-Prüfung (100-ns-Ticks seit Unix-Epoche, wie im Index).
    pub mtime_ticks: i64,
    /// Nur zur Lesbarkeit; `apply` verwendet `mtime_ticks`.
    pub mtime: String,
    /// xxh3-128 als 32 Hex-Zeichen.
    pub hash: String,
    pub keep: String,
    pub keep_hash: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skipped {
    pub path: String,
    pub reason: SkipReason,
}

pub fn ticks_to_rfc3339(ticks: i64) -> String {
    let secs = ticks.div_euclid(TICKS_PER_SEC);
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
        })
        .unwrap_or_default()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn is_hash(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `path_key` löst `.`/`..` nicht auf; ein Pfad wie `Wurzel\..\Anders` würde die
/// Wurzel-Prüfung bestehen, ohne in der Wurzel zu liegen.
pub(super) fn has_dot_component(path: &str) -> bool {
    path.split(['\\', '/']).any(|c| c == "." || c == "..")
}

impl Plan {
    /// Lädt aus JSON-Text; die Version wird vor allem anderen geprüft, damit ein Plan einer
    /// neueren Version nicht an unbekannten Feldern scheitert.
    pub fn from_json(text: &str) -> Result<Self, PlanError> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| PlanError::Corrupt(e.to_string()))?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| PlanError::Corrupt("Feld „version“ fehlt".into()))?;
        if version != u64::from(PLAN_VERSION) {
            return Err(PlanError::UnsupportedVersion {
                found: version,
                expected: PLAN_VERSION,
            });
        }
        let plan: Self =
            serde_json::from_value(value).map_err(|e| PlanError::Corrupt(e.to_string()))?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn load(path: &Path) -> Result<Self, PlanError> {
        let text = std::fs::read_to_string(paths::extended(path))
            .map_err(|e| PlanError::Unreadable(format!("{}: {e}", paths::display(path))))?;
        Self::from_json(&text)
    }

    pub fn to_json(&self) -> String {
        // Reine Strukturen mit String-Schlüsseln: Serialisieren kann nicht fehlschlagen.
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Schreibt die Plan-Datei und überschreibt nie eine vorhandene.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(paths::extended(path))?;
        file.write_all(self.to_json().as_bytes())?;
        file.sync_all()
    }

    pub fn total_bytes(&self) -> u64 {
        self.actions.iter().map(|a| a.size).sum()
    }

    /// Strukturprüfung, unabhängig von den Schutzregeln (die prüft `apply` separat).
    pub fn validate(&self) -> Result<(), PlanError> {
        let invalid = |msg: String| Err(PlanError::Invalid(msg));
        let root_key = paths::path_key(Path::new(&self.root));
        let mut ids = HashSet::new();
        let mut targets = HashSet::new();
        for a in &self.actions {
            if !ids.insert(a.id) {
                return invalid(format!("Aktions-ID {} kommt doppelt vor", a.id));
            }
            if !is_hash(&a.hash) || !is_hash(&a.keep_hash) {
                return invalid(format!("Aktion {}: Hash ist kein xxh3-128-Hex", a.id));
            }
            let (key, keep_key) = (
                paths::path_key(Path::new(&a.path)),
                paths::path_key(Path::new(&a.keep)),
            );
            if has_dot_component(&a.path) || has_dot_component(&a.keep) {
                return invalid(format!(
                    "Aktion {}: Pfade mit „.“ oder „..“ sind nicht erlaubt",
                    a.id
                ));
            }
            if !paths::is_under(&key, &root_key) || key == root_key {
                return invalid(format!(
                    "Aktion {}: {} liegt nicht in der Wurzel",
                    a.id, a.path
                ));
            }
            if key == keep_key {
                return invalid(format!(
                    "Aktion {}: Pfad entspricht der behaltenen Datei",
                    a.id
                ));
            }
            if a.hash != a.keep_hash {
                return invalid(format!("Aktion {}: Hash und keep_hash weichen ab", a.id));
            }
            targets.insert(key);
        }
        for a in &self.actions {
            if targets.contains(&paths::path_key(Path::new(&a.keep))) {
                return invalid(format!(
                    "Aktion {}: die behaltene Datei {} steht selbst als Aktion im Plan",
                    a.id, a.keep
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef";

    fn action(id: u32, path: &str, keep: &str) -> PlannedAction {
        PlannedAction {
            id,
            action: ActionType::Quarantine,
            path: path.into(),
            size: 10,
            mtime_ticks: 1_700_000_000 * TICKS_PER_SEC,
            mtime: ticks_to_rfc3339(1_700_000_000 * TICKS_PER_SEC),
            hash: HASH.into(),
            keep: keep.into(),
            keep_hash: HASH.into(),
            reason: "exact-duplicate".into(),
        }
    }

    fn plan(actions: Vec<PlannedAction>) -> Plan {
        Plan {
            version: PLAN_VERSION,
            created: "2026-10-03T12:00:00+02:00".into(),
            kind: PlanKind::Dedupe,
            root: r"D:\Daten".into(),
            keep_strategy: "oldest".into(),
            actions,
            skipped: vec![Skipped {
                path: r"D:\Daten\x.txt".into(),
                reason: SkipReason::Protected,
            }],
        }
    }

    fn ok_plan() -> Plan {
        plan(vec![
            action(1, r"D:\Daten\b\kopie.txt", r"D:\Daten\a\orig.txt"),
            action(2, r"D:\Daten\c\kopie.txt", r"D:\Daten\a\orig.txt"),
        ])
    }

    #[test]
    fn roundtrip_ist_verlustfrei() {
        let p = ok_plan();
        assert_eq!(Plan::from_json(&p.to_json()).unwrap(), p);
    }

    #[test]
    fn json_ist_menschenlesbar() {
        let json = ok_plan().to_json();
        assert!(json.contains("\"type\": \"quarantine\""));
        assert!(json.contains("\"reason\": \"protected\""));
        assert!(json.contains('\n'));
    }

    #[test]
    fn unbekannte_version_wird_klar_abgelehnt() {
        let mut v: serde_json::Value = serde_json::from_str(&ok_plan().to_json()).unwrap();
        v["version"] = 2.into();
        v["neues_feld"] = true.into();
        let err = Plan::from_json(&v.to_string()).unwrap_err();
        assert!(matches!(
            err,
            PlanError::UnsupportedVersion {
                found: 2,
                expected: 1
            }
        ));
        assert!(err.to_string().contains("Version 2"));
    }

    #[test]
    fn unbekannte_felder_und_kaputtes_json_sind_fehler() {
        let mut v: serde_json::Value = serde_json::from_str(&ok_plan().to_json()).unwrap();
        v["actions"][0]["extra"] = 1.into();
        assert!(matches!(
            Plan::from_json(&v.to_string()),
            Err(PlanError::Corrupt(_))
        ));
        assert!(matches!(
            Plan::from_json("{ nicht json"),
            Err(PlanError::Corrupt(_))
        ));
        assert!(matches!(Plan::from_json("{}"), Err(PlanError::Corrupt(_))));
    }

    #[test]
    fn doppelte_ids_sind_ungueltig() {
        let p = plan(vec![
            action(1, r"D:\Daten\b.txt", r"D:\Daten\a.txt"),
            action(1, r"D:\Daten\c.txt", r"D:\Daten\a.txt"),
        ]);
        assert!(matches!(p.validate(), Err(PlanError::Invalid(m)) if m.contains("doppelt")));
    }

    #[test]
    fn aktion_auf_die_behaltene_datei_ist_ungueltig() {
        let p = plan(vec![action(1, r"D:\Daten\A.txt", r"d:\daten\a.txt")]);
        assert!(p.validate().is_err());
    }

    #[test]
    fn behaltene_datei_darf_nicht_selbst_aktion_sein() {
        let p = plan(vec![
            action(1, r"D:\Daten\b.txt", r"D:\Daten\a.txt"),
            action(2, r"D:\Daten\a.txt", r"D:\Daten\c.txt"),
        ]);
        assert!(matches!(p.validate(), Err(PlanError::Invalid(m)) if m.contains("selbst")));
    }

    #[test]
    fn punkt_komponenten_sind_ungueltig() {
        for bad in [
            r"D:\Daten\..\Anders\x.txt",
            r"D:\Daten\sub\..\..\x.txt",
            r"D:\Daten\.\x.txt",
        ] {
            let p = plan(vec![action(1, bad, r"D:\Daten\a.txt")]);
            assert!(
                matches!(p.validate(), Err(PlanError::Invalid(m)) if m.contains("..")),
                "{bad}"
            );
        }
        let p = plan(vec![action(1, r"D:\Daten\b.txt", r"D:\Daten\..\a.txt")]);
        assert!(p.validate().is_err());
    }

    #[test]
    fn aktion_ausserhalb_der_wurzel_ist_ungueltig() {
        let p = plan(vec![action(1, r"C:\Windows\x.dll", r"D:\Daten\a.txt")]);
        assert!(matches!(p.validate(), Err(PlanError::Invalid(m)) if m.contains("Wurzel")));
        let p = plan(vec![action(1, r"D:\Daten2\x.txt", r"D:\Daten\a.txt")]);
        assert!(p.validate().is_err());
    }

    #[test]
    fn ungueltige_hashes_sind_abgelehnt() {
        let mut a = action(1, r"D:\Daten\b.txt", r"D:\Daten\a.txt");
        a.hash = "xyz".into();
        assert!(plan(vec![a]).validate().is_err());
        let mut a = action(1, r"D:\Daten\b.txt", r"D:\Daten\a.txt");
        a.keep_hash = "ffffffffffffffffffffffffffffffff".into();
        assert!(plan(vec![a]).validate().is_err());
    }

    #[test]
    fn save_ueberschreibt_nie() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.json");
        let p = ok_plan();
        p.save(&path).unwrap();
        assert_eq!(Plan::load(&path).unwrap(), p);
        assert_eq!(
            p.save(&path).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn summen_und_zeitformat() {
        assert_eq!(ok_plan().total_bytes(), 20);
        assert!(
            ticks_to_rfc3339(0).starts_with("1970-01-01")
                || ticks_to_rfc3339(0).starts_with("1969")
        );
        assert_eq!(hex(&[0x01, 0xab]), "01ab");
    }
}
