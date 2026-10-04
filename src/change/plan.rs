//! Plan-Datei (JSON): Datenmodell, Laden mit Versionsprüfung und Validierung.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::protect::TOOL_DIR;
use super::SkipReason;
use crate::paths;
use crate::scan::source::TICKS_PER_SEC;

/// Aktuelle Plan-Version; Version 1 (nur `dedupe`) wird weiterhin gelesen.
pub const PLAN_VERSION: u32 = 2;
const MIN_PLAN_VERSION: u32 = 1;

/// Name des Archivordners unter der Wurzel (Ziel von `move`).
pub const ARCHIVE_DIR: &str = "_Archiv";

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("Plan-Datei nicht lesbar: {0}")]
    Unreadable(String),
    #[error("Plan-Datei ist beschädigt: {0}")]
    Corrupt(String),
    #[error("Plan hat Version {found}, unterstützt werden die Versionen 1 bis {expected}")]
    UnsupportedVersion { found: u64, expected: u32 },
    #[error("Plan ist ungültig: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanKind {
    Dedupe,
    Junk,
    EmptyDirs,
    Archive,
    Versions,
    Rules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionType {
    Quarantine,
    RemoveDir,
    Move,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub created: String,
    pub kind: PlanKind,
    pub root: String,
    /// Nur `dedupe`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_strategy: Option<String>,
    /// Aufrufparameter zur Nachvollziehbarkeit (z. B. `older_than=2y`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
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
    /// xxh3-128 als 32 Hex-Zeichen (nur `dedupe`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Behaltene Datei (`dedupe`, `versions`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_hash: Option<String>,
    pub reason: String,
    /// Ziel unter `<Wurzel>\_Archiv` (nur `move`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// `move` eines ganzen Ordners; `size`/`mtime_ticks` sind dann Summe bzw. jüngste mtime.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_dir: bool,
    /// Dateianzahl eines verschobenen Ordners.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<u64>,
    /// Name der auslösenden Regel (nur `move` in Plänen der Art `rules`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skipped {
    pub path: String,
    pub reason: SkipReason,
}

/// Ergebnis eines Planers der Aufräumaktionen (`junk`, `empty-dirs`, `archive`, `versions`).
#[derive(Debug)]
pub struct CleanupPlan {
    pub plan: Plan,
    /// Betroffene Bytes (bei `remove-dir` 0).
    pub bytes: u64,
    /// Hinweise für die Zusammenfassung, z. B. „kein Downloads-Ordner unter der Wurzel“.
    pub notes: Vec<String>,
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

/// Schlüssel der Wurzel und der gesperrten Zielbereiche.
struct Keys<'a> {
    root: &'a str,
    archive: &'a str,
    tool: &'a str,
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
        if !(u64::from(MIN_PLAN_VERSION)..=u64::from(PLAN_VERSION)).contains(&version) {
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
        let root = Path::new(&self.root);
        let root_key = paths::path_key(root);
        let archive_key = paths::path_key(&root.join(ARCHIVE_DIR));
        let tool_key = paths::path_key(&root.join(TOOL_DIR));
        let mut ids = HashSet::new();
        let mut sources = HashSet::new();
        let mut targets = HashSet::new();
        for a in &self.actions {
            if !ids.insert(a.id) {
                return invalid(format!("Aktions-ID {} kommt doppelt vor", a.id));
            }
            if has_dot_component(&a.path)
                || a.keep.as_deref().is_some_and(has_dot_component)
                || a.target.as_deref().is_some_and(has_dot_component)
            {
                return invalid(format!(
                    "Aktion {}: Pfade mit „.“ oder „..“ sind nicht erlaubt",
                    a.id
                ));
            }
            let key = paths::path_key(Path::new(&a.path));
            if !paths::is_under(&key, &root_key) || key == root_key {
                return invalid(format!(
                    "Aktion {}: {} liegt nicht in der Wurzel",
                    a.id, a.path
                ));
            }
            self.validate_fields(a, &key, &Keys { root: &root_key, archive: &archive_key, tool: &tool_key })?;
            if let Some(target) = &a.target {
                if !targets.insert(paths::path_key(Path::new(target))) {
                    return invalid(format!("Aktion {}: Ziel {target} kommt doppelt vor", a.id));
                }
            }
            sources.insert(key);
        }
        for a in &self.actions {
            if let Some(keep) = &a.keep {
                if sources.contains(&paths::path_key(Path::new(keep))) {
                    return invalid(format!(
                        "Aktion {}: die behaltene Datei {keep} steht selbst als Aktion im Plan",
                        a.id
                    ));
                }
            }
            if let Some(target) = &a.target {
                let target_key = paths::path_key(Path::new(target));
                let own_key = paths::path_key(Path::new(&a.path));
                if target_key != own_key && sources.contains(&target_key) {
                    return invalid(format!(
                        "Aktion {}: Ziel {target} ist die Quelle einer anderen Aktion",
                        a.id
                    ));
                }
                let nested = self.actions.iter().any(|o| {
                    o.is_dir && o.id != a.id && {
                        let dir = paths::path_key(Path::new(&o.path));
                        paths::is_under(&target_key, &dir)
                    }
                });
                if nested {
                    return invalid(format!(
                        "Aktion {}: Ziel {target} liegt in einem Ordner, der selbst verschoben wird",
                        a.id
                    ));
                }
            }
        }
        Ok(())
    }

    /// Pflicht- und Verbotsfelder je Aktionstyp.
    fn validate_fields(
        &self,
        a: &PlannedAction,
        key: &str,
        keys: &Keys,
    ) -> Result<(), PlanError> {
        let invalid = |msg: String| Err(PlanError::Invalid(format!("Aktion {}: {msg}", a.id)));
        let is_rules = self.kind == PlanKind::Rules;
        if is_rules && a.action != ActionType::Move {
            return invalid("ein Regel-Plan enthält nur move-Aktionen".into());
        }
        if a.rule.is_some() != is_rules {
            return invalid(if is_rules {
                "move braucht den Namen der Regel (rule)".into()
            } else {
                "rule gibt es nur in Plänen der Art rules".into()
            });
        }
        match a.action {
            ActionType::Quarantine => {
                let present = [&a.hash, &a.keep, &a.keep_hash]
                    .iter()
                    .filter(|f| f.is_some())
                    .count();
                if present != 0 && present != 3 {
                    return invalid("hash, keep und keep_hash gelten nur gemeinsam".into());
                }
                if self.kind == PlanKind::Dedupe && present == 0 {
                    return invalid("Hash der Aktion fehlt".into());
                }
                if let (Some(hash), Some(keep_hash), Some(keep)) = (&a.hash, &a.keep_hash, &a.keep)
                {
                    if !is_hash(hash) || !is_hash(keep_hash) {
                        return invalid("Hash ist kein xxh3-128-Hex".into());
                    }
                    if hash != keep_hash {
                        return invalid("Hash und keep_hash weichen ab".into());
                    }
                    if paths::path_key(Path::new(keep)) == key {
                        return invalid("Pfad entspricht der behaltenen Datei".into());
                    }
                }
                if a.target.is_some() {
                    return invalid("quarantine hat kein Ziel".into());
                }
            }
            ActionType::RemoveDir => {
                if a.hash.is_some() || a.keep.is_some() || a.keep_hash.is_some() {
                    return invalid(
                        "remove-dir trägt keinen Hash und keine behaltene Datei".into(),
                    );
                }
                if a.target.is_some() {
                    return invalid("remove-dir hat kein Ziel".into());
                }
            }
            ActionType::Move => {
                if a.hash.is_some() || a.keep_hash.is_some() {
                    return invalid("move trägt keinen Hash".into());
                }
                let Some(target) = &a.target else {
                    return invalid("move braucht ein Ziel".into());
                };
                let target_key = paths::path_key(Path::new(target));
                if is_rules {
                    if !paths::is_under(&target_key, keys.root) || target_key == keys.root {
                        return invalid(format!("Ziel {target} liegt nicht in der Wurzel"));
                    }
                    if paths::is_under(&target_key, keys.archive)
                        || paths::is_under(&target_key, keys.tool)
                    {
                        return invalid(format!(
                            "Ziel {target} liegt unter {ARCHIVE_DIR} oder {TOOL_DIR}"
                        ));
                    }
                    if a.is_dir {
                        return invalid("Regeln verschieben nur Dateien".into());
                    }
                    // Dasselbe Ziel ist nur ein Umbenennen, wenn sich die Schreibweise ändert.
                    if target_key == key && target == &a.path {
                        return invalid("Ziel ist identisch mit der Quelle".into());
                    }
                } else if target_key == keys.archive || !paths::is_under(&target_key, keys.archive)
                {
                    return invalid(format!(
                        "Ziel {target} liegt nicht unter {}\\{ARCHIVE_DIR}",
                        self.root
                    ));
                }
                if a.is_dir && a.files.is_none() {
                    return invalid("verschobener Ordner braucht die Dateianzahl (files)".into());
                }
                if paths::is_under(&target_key, key) && !(is_rules && target_key == key) {
                    return invalid("Ziel liegt in der Quelle selbst".into());
                }
                if a.keep.as_deref().map(|k| paths::path_key(Path::new(k))) == Some(key.into()) {
                    return invalid("Pfad entspricht der behaltenen Datei".into());
                }
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
            hash: Some(HASH.into()),
            keep: Some(keep.into()),
            keep_hash: Some(HASH.into()),
            reason: "exact-duplicate".into(),
            target: None,
            is_dir: false,
            files: None,
            rule: None,
        }
    }

    fn plan(actions: Vec<PlannedAction>) -> Plan {
        Plan {
            version: PLAN_VERSION,
            created: "2026-10-03T12:00:00+02:00".into(),
            kind: PlanKind::Dedupe,
            root: r"D:\Daten".into(),
            keep_strategy: Some("oldest".into()),
            params: Default::default(),
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
        for version in [0, 3] {
            let mut v: serde_json::Value = serde_json::from_str(&ok_plan().to_json()).unwrap();
            v["version"] = version.into();
            v["neues_feld"] = true.into();
            let err = Plan::from_json(&v.to_string()).unwrap_err();
            assert!(
                matches!(err, PlanError::UnsupportedVersion { expected: 2, .. }),
                "{err}"
            );
            assert!(err.to_string().contains(&format!("Version {version}")));
        }
    }

    const V1_JSON: &str = r#"{
        "version": 1, "created": "2026-10-03T12:00:00+02:00", "kind": "dedupe",
        "root": "D:\\Daten", "keep_strategy": "oldest",
        "actions": [{"id": 1, "type": "quarantine", "path": "D:\\Daten\\b.txt", "size": 1,
            "mtime_ticks": 0, "mtime": "", "hash": "0123456789abcdef0123456789abcdef",
            "keep": "D:\\Daten\\a.txt", "keep_hash": "0123456789abcdef0123456789abcdef",
            "reason": "exact-duplicate"}],
        "skipped": []}"#;

    #[test]
    fn plan_version_1_aus_phase_2_wird_gelesen() {
        let p = Plan::from_json(V1_JSON).unwrap();
        assert_eq!(p.version, 1);
        assert_eq!(p.kind, PlanKind::Dedupe);
        assert_eq!(p.keep_strategy.as_deref(), Some("oldest"));
        assert!(p.params.is_empty());
        let a = &p.actions[0];
        assert_eq!(a.keep.as_deref(), Some(r"D:\Daten\a.txt"));
        assert!(!a.is_dir && a.target.is_none() && a.files.is_none());
    }

    #[test]
    fn plan_version_1_ohne_hash_ist_ungueltig() {
        let mut v: serde_json::Value = serde_json::from_str(V1_JSON).unwrap();
        v["actions"][0].as_object_mut().unwrap().remove("hash");
        assert!(matches!(
            Plan::from_json(&v.to_string()),
            Err(PlanError::Invalid(_))
        ));
    }

    fn junk(id: u32, path: &str) -> PlannedAction {
        PlannedAction {
            hash: None,
            keep: None,
            keep_hash: None,
            reason: "junk:temp".into(),
            ..action(id, path, "")
        }
    }

    fn remove_dir(id: u32, path: &str) -> PlannedAction {
        PlannedAction {
            action: ActionType::RemoveDir,
            reason: "empty-dir".into(),
            is_dir: true,
            ..junk(id, path)
        }
    }

    fn mv(id: u32, path: &str, target: &str) -> PlannedAction {
        PlannedAction {
            action: ActionType::Move,
            target: Some(target.into()),
            reason: "archive".into(),
            ..junk(id, path)
        }
    }

    fn plan_of(kind: PlanKind, actions: Vec<PlannedAction>) -> Plan {
        Plan {
            kind,
            keep_strategy: None,
            ..plan(actions)
        }
    }

    #[test]
    fn neue_aktionstypen_sind_verlustfrei_serialisierbar() {
        let mut dir_move = mv(3, r"D:\Daten\alt", r"D:\Daten\_Archiv\2020\alt");
        dir_move.is_dir = true;
        dir_move.files = Some(4);
        let mut p = plan_of(
            PlanKind::Archive,
            vec![
                junk(1, r"D:\Daten\x.tmp"),
                remove_dir(2, r"D:\Daten\leer"),
                dir_move,
            ],
        );
        p.params.insert("older_than".into(), "2y".into());
        assert!(p.validate().is_ok());
        let json = p.to_json();
        assert_eq!(Plan::from_json(&json).unwrap(), p);
        assert!(json.contains("\"type\": \"remove-dir\""));
        assert!(json.contains("\"type\": \"move\""));
        assert!(json.contains("\"older_than\": \"2y\""));
    }

    #[test]
    fn leere_optionale_felder_stehen_nicht_im_json() {
        let json = plan_of(PlanKind::Junk, vec![junk(1, r"D:\Daten\x.tmp")]).to_json();
        for field in [
            "hash",
            "keep",
            "target",
            "is_dir",
            "files",
            "keep_strategy",
            "params",
        ] {
            assert!(!json.contains(&format!("\"{field}\"")), "{field}");
        }
    }

    #[test]
    fn quarantine_ohne_hash_ist_nur_ausserhalb_von_dedupe_gueltig() {
        let p = plan_of(PlanKind::Junk, vec![junk(1, r"D:\Daten\x.tmp")]);
        assert!(p.validate().is_ok());
        let p = plan_of(PlanKind::Dedupe, vec![junk(1, r"D:\Daten\x.tmp")]);
        assert!(matches!(p.validate(), Err(PlanError::Invalid(m)) if m.contains("Hash")));
    }

    #[test]
    fn hash_und_keep_gelten_nur_gemeinsam() {
        let mut a = junk(1, r"D:\Daten\x.tmp");
        a.hash = Some(HASH.into());
        assert!(plan_of(PlanKind::Junk, vec![a]).validate().is_err());
        let mut a = junk(1, r"D:\Daten\x.tmp");
        a.keep = Some(r"D:\Daten\y.tmp".into());
        assert!(plan_of(PlanKind::Junk, vec![a]).validate().is_err());
    }

    #[test]
    fn remove_dir_braucht_einen_unterordner_ohne_hash_keep_target() {
        let p = plan_of(PlanKind::EmptyDirs, vec![remove_dir(1, r"D:\Daten\leer")]);
        assert!(p.validate().is_ok());
        let root = plan_of(PlanKind::EmptyDirs, vec![remove_dir(1, r"D:\Daten")]);
        assert!(root.validate().is_err());
        let mut a = remove_dir(1, r"D:\Daten\leer");
        a.hash = Some(HASH.into());
        assert!(plan_of(PlanKind::EmptyDirs, vec![a]).validate().is_err());
        let mut a = remove_dir(1, r"D:\Daten\leer");
        a.target = Some(r"D:\Daten\_Archiv\leer".into());
        assert!(plan_of(PlanKind::EmptyDirs, vec![a]).validate().is_err());
        let mut a = remove_dir(1, r"D:\Daten\leer");
        a.keep = Some(r"D:\Daten\x".into());
        assert!(plan_of(PlanKind::EmptyDirs, vec![a]).validate().is_err());
    }

    #[test]
    fn move_braucht_ein_ziel_unter_dem_archiv() {
        let ok = mv(1, r"D:\Daten\a.txt", r"D:\Daten\_Archiv\2020\a.txt");
        assert!(plan_of(PlanKind::Archive, vec![ok]).validate().is_ok());
        let ok = mv(1, r"D:\Daten\a.txt", r"d:\daten\_archiv\Versionen\a.txt");
        assert!(plan_of(PlanKind::Versions, vec![ok]).validate().is_ok());

        let mut none = mv(1, r"D:\Daten\a.txt", "");
        none.target = None;
        assert!(plan_of(PlanKind::Archive, vec![none]).validate().is_err());
        for bad in [
            r"D:\Daten\neu\a.txt",
            r"D:\Daten\_Archiv",
            r"D:\Daten\_Archiv2\a.txt",
            r"E:\_Archiv\a.txt",
            r"D:\Daten\_Archiv\..\a.txt",
            r"D:\Daten\_Archiv\.\a.txt",
        ] {
            let p = plan_of(PlanKind::Archive, vec![mv(1, r"D:\Daten\a.txt", bad)]);
            assert!(
                matches!(p.validate(), Err(PlanError::Invalid(_))),
                "Ziel {bad} muss abgelehnt werden"
            );
        }
    }

    #[test]
    fn move_mit_hash_ist_ungueltig() {
        let mut a = mv(1, r"D:\Daten\a.txt", r"D:\Daten\_Archiv\a.txt");
        a.hash = Some(HASH.into());
        assert!(plan_of(PlanKind::Archive, vec![a]).validate().is_err());
    }

    #[test]
    fn doppelte_ziele_sind_ungueltig() {
        let p = plan_of(
            PlanKind::Archive,
            vec![
                mv(1, r"D:\Daten\a.txt", r"D:\Daten\_Archiv\x.txt"),
                mv(2, r"D:\Daten\b.txt", r"D:\Daten\_ARCHIV\X.txt"),
            ],
        );
        assert!(matches!(p.validate(), Err(PlanError::Invalid(m)) if m.contains("Ziel")));
    }

    #[test]
    fn ziel_unter_einem_quellordner_ist_ungueltig() {
        let mut dir = mv(1, r"D:\Daten\alt", r"D:\Daten\_Archiv\alt");
        dir.is_dir = true;
        dir.files = Some(1);
        let inner = mv(2, r"D:\Daten\b.txt", r"D:\Daten\alt\b.txt");
        let p = plan_of(PlanKind::Archive, vec![dir, inner]);
        assert!(p.validate().is_err());
    }

    #[test]
    fn ziel_innerhalb_der_eigenen_quelle_ist_ungueltig() {
        let nested = mv(1, r"D:\Daten\_Archiv", r"D:\Daten\_Archiv\x");
        assert!(plan_of(PlanKind::Archive, vec![nested]).validate().is_err());
        let same = mv(1, r"D:\Daten\_Archiv\x", r"D:\Daten\_Archiv\x");
        assert!(plan_of(PlanKind::Archive, vec![same]).validate().is_err());
        let ok = mv(1, r"D:\Daten\_Archiv\2019\x", r"D:\Daten\_Archiv\2020\x");
        assert!(plan_of(PlanKind::Archive, vec![ok]).validate().is_ok());
    }

    #[test]
    fn verschobener_ordner_braucht_dateianzahl() {
        let mut dir = mv(1, r"D:\Daten\alt", r"D:\Daten\_Archiv\alt");
        dir.is_dir = true;
        assert!(plan_of(PlanKind::Archive, vec![dir.clone()])
            .validate()
            .is_err());
        dir.files = Some(2);
        assert!(plan_of(PlanKind::Archive, vec![dir]).validate().is_ok());
    }

    #[test]
    fn versions_move_darf_keep_tragen_ohne_hash() {
        let mut a = mv(
            1,
            r"D:\Daten\a_v1.doc",
            r"D:\Daten\_Archiv\Versionen\a_v1.doc",
        );
        a.keep = Some(r"D:\Daten\a_v2.doc".into());
        assert!(plan_of(PlanKind::Versions, vec![a.clone()])
            .validate()
            .is_ok());
        a.keep = Some(r"D:\Daten\a_v1.doc".into());
        assert!(plan_of(PlanKind::Versions, vec![a]).validate().is_err());
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
        a.hash = Some("xyz".into());
        assert!(plan(vec![a]).validate().is_err());
        let mut a = action(1, r"D:\Daten\b.txt", r"D:\Daten\a.txt");
        a.keep_hash = Some("ffffffffffffffffffffffffffffffff".into());
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

    fn rules_mv(id: u32, path: &str, target: &str) -> PlannedAction {
        PlannedAction {
            rule: Some("fotos".into()),
            reason: "rule:fotos".into(),
            ..mv(id, path, target)
        }
    }

    fn rules_plan(actions: Vec<PlannedAction>) -> Plan {
        plan_of(PlanKind::Rules, actions)
    }

    #[test]
    fn rules_move_darf_ueberall_unter_der_wurzel_landen() {
        let ok = rules_mv(1, r"D:\Daten\Downloads\a.jpg", r"D:\Daten\Fotos\2026\a.jpg");
        let p = rules_plan(vec![ok]);
        assert!(p.validate().is_ok());
        assert_eq!(Plan::from_json(&p.to_json()).unwrap(), p);
        assert!(p.to_json().contains("\"rule\": \"fotos\""));
    }

    #[test]
    fn archive_move_darf_weiterhin_nur_ins_archiv() {
        let p = plan_of(
            PlanKind::Archive,
            vec![mv(1, r"D:\Daten\a.txt", r"D:\Daten\Fotos\a.txt")],
        );
        assert!(p.validate().is_err());
    }

    #[test]
    fn rules_ziel_ausserhalb_der_wurzel_im_archiv_oder_im_werkzeugordner_ist_ungueltig() {
        for bad in [
            r"E:\Fotos\a.jpg",
            r"D:\Daten2\a.jpg",
            r"D:\Daten\_Archiv\a.jpg",
            r"D:\Daten\_archiv",
            r"D:\Daten\.ordner-cleanup\q\a.jpg",
            r"D:\Daten",
            r"D:\Daten\..\a.jpg",
        ] {
            let p = rules_plan(vec![rules_mv(1, r"D:\Daten\Downloads\a.jpg", bad)]);
            assert!(
                matches!(p.validate(), Err(PlanError::Invalid(_))),
                "Ziel {bad} muss abgelehnt werden"
            );
        }
    }

    #[test]
    fn rules_ziel_nur_in_der_schreibweise_ist_gueltig_identisches_nicht() {
        let rename = rules_mv(1, r"D:\Daten\foto.JPG", r"D:\Daten\foto.jpg");
        assert!(rules_plan(vec![rename]).validate().is_ok());
        let same = rules_mv(1, r"D:\Daten\foto.jpg", r"D:\Daten\foto.jpg");
        assert!(rules_plan(vec![same]).validate().is_err());
    }

    #[test]
    fn rules_ziel_in_der_eigenen_quelle_ist_ungueltig() {
        let nested = rules_mv(1, r"D:\Daten\a.jpg", r"D:\Daten\a.jpg\x");
        assert!(rules_plan(vec![nested]).validate().is_err());
    }

    #[test]
    fn rules_braucht_den_regelnamen_und_keine_ordner() {
        let mut no_rule = rules_mv(1, r"D:\Daten\a.jpg", r"D:\Daten\F\a.jpg");
        no_rule.rule = None;
        assert!(rules_plan(vec![no_rule]).validate().is_err());
        let mut dir = rules_mv(1, r"D:\Daten\a", r"D:\Daten\F\a");
        dir.is_dir = true;
        dir.files = Some(1);
        assert!(rules_plan(vec![dir]).validate().is_err());
        let quarantine = PlannedAction {
            rule: Some("x".into()),
            ..junk(1, r"D:\Daten\x.tmp")
        };
        assert!(rules_plan(vec![quarantine]).validate().is_err());
    }

    #[test]
    fn rule_gibt_es_nur_bei_rules_plaenen() {
        let a = rules_mv(1, r"D:\Daten\a.txt", r"D:\Daten\_Archiv\a.txt");
        assert!(plan_of(PlanKind::Archive, vec![a]).validate().is_err());
    }

    #[test]
    fn ziel_darf_nicht_die_quelle_einer_anderen_aktion_sein() {
        let p = rules_plan(vec![
            rules_mv(1, r"D:\Daten\a.jpg", r"D:\Daten\b.jpg"),
            rules_mv(2, r"D:\Daten\B.jpg", r"D:\Daten\c.jpg"),
        ]);
        assert!(matches!(p.validate(), Err(PlanError::Invalid(m)) if m.contains("Quelle")));
        let p = plan_of(
            PlanKind::Archive,
            vec![
                mv(1, r"D:\Daten\_Archiv\1\a", r"D:\Daten\_Archiv\2\a"),
                mv(2, r"D:\Daten\_Archiv\2\a", r"D:\Daten\_Archiv\3\a"),
            ],
        );
        assert!(p.validate().is_err(), "gilt für alle Arten");
    }

    #[test]
    fn plan_ohne_rule_feld_aus_phase_3_wird_unveraendert_gelesen() {
        let p = plan_of(
            PlanKind::Archive,
            vec![mv(1, r"D:\Daten\a.txt", r"D:\Daten\_Archiv\2020\a.txt")],
        );
        let json = p.to_json();
        assert!(!json.contains("\"rule\""), "leer wird nicht geschrieben");
        let loaded = Plan::from_json(&json).unwrap();
        assert_eq!(loaded.actions[0].rule, None);
        assert_eq!(loaded.version, PLAN_VERSION);
    }
}
