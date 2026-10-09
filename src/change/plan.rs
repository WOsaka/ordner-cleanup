//! Plan-Datei (JSON): Datenmodell, Laden mit Versionsprüfung und Validierung.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::protect::TOOL_DIR;
use super::SkipReason;
use crate::paths;
use crate::scan::source::TICKS_PER_SEC;

/// Aktuelle Plan-Version; Version 1 (nur `dedupe`) wird weiterhin gelesen.
pub const PLAN_VERSION: u32 = 3;
const MIN_PLAN_VERSION: u32 = 1;
/// Ab dieser Version trägt die Plan-Datei die Prüfsumme `integrity` (Pflichtfeld).
const SEALED_VERSION: u32 = 3;
const INTEGRITY_PREFIX: &str = "sha256:";

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
    #[error(
        "Plan wurde nach dem Erstellen verändert (Prüfsumme erwartet {}…, gefunden {}…).          Bewusst geändert? Mit `ordner-cleanup plan seal <datei>` neu versiegeln.",
        short_integrity(.stored),
        short_integrity(.actual)
    )]
    Tampered { stored: String, actual: String },
}

/// Zustand der Prüfsumme einer gelesenen Plan-Datei.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seal {
    /// Format 3, Prüfsumme stimmt.
    Valid,
    /// Format 1 oder 2, ohne Prüfsumme.
    Unprotected {
        version: u32,
    },
    /// Format 3 ohne Feld `integrity`.
    Missing,
    Mismatch {
        stored: String,
        actual: String,
    },
}

/// Die ersten 12 Hex-Zeichen einer Prüfsumme `sha256:<hex>`.
pub fn short_integrity(integrity: &str) -> &str {
    let hex = integrity
        .strip_prefix(INTEGRITY_PREFIX)
        .unwrap_or(integrity);
    hex.get(..12).unwrap_or(hex)
}

fn is_integrity(s: &str) -> bool {
    s.strip_prefix(INTEGRITY_PREFIX).is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Plan-Datei beim Schreiben: Prüfsumme als erstes Feld, danach der Plan.
#[derive(Serialize)]
struct Sealed<'a> {
    integrity: String,
    #[serde(flatten)]
    plan: &'a Plan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanKind {
    Dedupe,
    DedupeDirs,
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
    /// Beim Planen wirksame `protected_paths` (globale Config und Profil). `apply` schützt
    /// zusätzlich zur eigenen Config damit; fehlt das Feld (ältere Pläne), gilt nur die Config.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protected_paths: Vec<String>,
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
    /// Metadaten-Fingerabdruck des behaltenen Ordners (nur `dedupe-dirs`, 32 Hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_fingerprint: Option<String>,
    /// Metadaten-Fingerabdruck des entfernten Ordners (nur `dedupe-dirs`, 32 Hex). Fehlt er
    /// (ältere Pläne), prüft `apply` die Quelle nur über Dateianzahl, Summe und jüngste mtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
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
    /// Zusatz zum Grund, z. B. das fehlende Feld oder `rechnung 0.62 / mahnung 0.41`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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
    /// Lädt aus JSON-Text und lehnt einen veränderten Plan (`Tampered`) oder einen v3-Plan ohne
    /// Prüfsumme ab; Pläne der Formate 1 und 2 werden ungeprüft gelesen.
    pub fn from_json(text: &str) -> Result<Self, PlanError> {
        let (plan, seal) = Self::parse_unvalidated(text)?;
        match seal {
            Seal::Mismatch { stored, actual } => {
                return Err(PlanError::Tampered { stored, actual })
            }
            Seal::Missing => {
                return Err(PlanError::Invalid(format!(
                    "Plan im Format {SEALED_VERSION} braucht das Feld „integrity“"
                )))
            }
            Seal::Valid | Seal::Unprotected { .. } => {}
        }
        plan.validate()?;
        Ok(plan)
    }

    /// Liest und prüft die Struktur, lehnt eine abweichende oder fehlende Prüfsumme aber nicht
    /// ab, sondern meldet sie im `Seal` (für `plan seal`).
    pub fn parse(text: &str) -> Result<(Self, Seal), PlanError> {
        let (plan, seal) = Self::parse_unvalidated(text)?;
        plan.validate()?;
        Ok((plan, seal))
    }

    /// Die Version wird vor allem anderen geprüft, damit ein Plan einer neueren Version nicht an
    /// unbekannten Feldern scheitert. Die Prüfsumme wird vor `validate` verglichen, damit ein
    /// veränderter Plan immer als verändert gemeldet wird.
    fn parse_unvalidated(text: &str) -> Result<(Self, Seal), PlanError> {
        let mut value: serde_json::Value =
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
        let stored = match value.as_object_mut().and_then(|o| o.remove("integrity")) {
            None => None,
            Some(serde_json::Value::String(s)) if is_integrity(&s) => Some(s),
            Some(_) => {
                return Err(PlanError::Invalid(
                    "Feld „integrity“ hat nicht das Format sha256:<64 Hex-Zeichen>".into(),
                ))
            }
        };
        let sealed = version >= u64::from(SEALED_VERSION);
        if stored.is_some() && !sealed {
            return Err(PlanError::Invalid(format!(
                "Plan im Format {version} darf kein Feld „integrity“ tragen"
            )));
        }
        let plan: Self =
            serde_json::from_value(value).map_err(|e| PlanError::Corrupt(e.to_string()))?;
        let seal = match stored {
            _ if !sealed => Seal::Unprotected {
                version: plan.version,
            },
            None => Seal::Missing,
            Some(stored) => {
                let actual = plan.integrity();
                if stored == actual {
                    Seal::Valid
                } else {
                    Seal::Mismatch { stored, actual }
                }
            }
        };
        Ok((plan, seal))
    }

    pub fn load(path: &Path) -> Result<Self, PlanError> {
        Self::from_json(&read_text(path)?)
    }

    /// Liest die Datei wie `parse`, ohne eine abweichende Prüfsumme abzulehnen (nur für
    /// `plan seal`).
    pub fn read_unverified(path: &Path) -> Result<(Self, Seal), PlanError> {
        Self::parse(&read_text(path)?)
    }

    /// SHA-256 der kanonischen Form (kompaktes JSON in Feldreihenfolge der Struktur, ohne
    /// `integrity`) als `sha256:<hex>`.
    pub fn integrity(&self) -> String {
        // Reine Strukturen mit String-Schlüsseln: Serialisieren kann nicht fehlschlagen.
        let canonical = serde_json::to_string(self).unwrap_or_default();
        format!(
            "{INTEGRITY_PREFIX}{}",
            hex(&Sha256::digest(canonical.as_bytes()))
        )
    }

    /// Versiegeltes, eingerücktes JSON; jeder Plan wird dabei im aktuellen Format geschrieben.
    pub fn to_json(&self) -> String {
        let plan = if self.version == PLAN_VERSION {
            Cow::Borrowed(self)
        } else {
            Cow::Owned(Plan {
                version: PLAN_VERSION,
                ..self.clone()
            })
        };
        let sealed = Sealed {
            integrity: plan.integrity(),
            plan: &plan,
        };
        serde_json::to_string_pretty(&sealed).unwrap_or_default()
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

    /// Ersetzt eine vorhandene Plan-Datei atomar: erst `<datei>.tmp` schreiben, dann darüber
    /// umbenennen (nur für `plan seal`).
    pub fn save_replacing(&self, path: &Path) -> std::io::Result<()> {
        let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".tmp");
        let tmp = path.with_file_name(tmp_name);
        let written = (|| {
            let mut file = std::fs::File::create(paths::extended(&tmp))?;
            file.write_all(self.to_json().as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(paths::extended(&tmp), paths::extended(path))
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(paths::extended(&tmp));
        }
        written
    }

    pub fn total_bytes(&self) -> u64 {
        self.actions.iter().map(|a| a.size).sum()
    }

    /// Teilplan mit nur den Aktionen aus `keep`; IDs, `skipped`, Art, Wurzel, Zeitstempel und
    /// Parameter bleiben unverändert. Beim Speichern wird er wie jeder Plan Format 3 mit eigener
    /// Prüfsumme.
    pub fn subset(&self, keep: &HashSet<u32>) -> Plan {
        Plan {
            actions: self
                .actions
                .iter()
                .filter(|a| keep.contains(&a.id))
                .cloned()
                .collect(),
            ..self.clone()
        }
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
            self.validate_fields(
                a,
                &key,
                &Keys {
                    root: &root_key,
                    archive: &archive_key,
                    tool: &tool_key,
                },
            )?;
            if let Some(target) = &a.target {
                if !targets.insert(paths::path_key(Path::new(target))) {
                    return invalid(format!("Aktion {}: Ziel {target} kommt doppelt vor", a.id));
                }
            }
            sources.insert(key);
        }
        // Verschobene Ordner einmal vorab erfassen (Schlüssel → Aktions-IDs), damit die Prüfung
        // je Ziel nur seine Vorfahren nachschlägt statt alle Aktionen zu durchlaufen.
        let mut dir_moves: HashMap<String, Vec<u32>> = HashMap::new();
        for o in self.actions.iter().filter(|o| o.is_dir) {
            dir_moves
                .entry(paths::path_key(Path::new(&o.path)))
                .or_default()
                .push(o.id);
        }
        // `dedupe-dirs`: behaltene Ordner (Schlüssel → Aktions-IDs), um Quellen darin zu finden.
        let mut keep_dirs: HashMap<String, Vec<u32>> = HashMap::new();
        if self.kind == PlanKind::DedupeDirs {
            for o in self.actions.iter().filter(|o| o.is_dir) {
                if let Some(keep) = &o.keep {
                    keep_dirs
                        .entry(paths::path_key(Path::new(keep)))
                        .or_default()
                        .push(o.id);
                }
            }
        }
        for a in &self.actions {
            if self.kind == PlanKind::DedupeDirs {
                let own_key = paths::path_key(Path::new(&a.path));
                if moved_with_dir(&keep_dirs, a.id, &own_key) {
                    return invalid(format!(
                        "Aktion {}: {} liegt im behaltenen Ordner einer anderen Aktion",
                        a.id, a.path
                    ));
                }
                if moved_with_dir(&dir_moves, a.id, &own_key) {
                    return invalid(format!(
                        "Aktion {}: {} liegt in einem Ordner, der selbst entfernt wird",
                        a.id, a.path
                    ));
                }
            }
            if let Some(keep) = &a.keep {
                if sources.contains(&paths::path_key(Path::new(keep))) {
                    return invalid(format!(
                        "Aktion {}: die behaltene Datei {keep} steht selbst als Aktion im Plan",
                        a.id
                    ));
                }
            }
            if let (true, Some(keep)) = (a.is_dir, &a.keep) {
                if moved_with_dir(&dir_moves, a.id, &paths::path_key(Path::new(keep))) {
                    return invalid(format!(
                        "Aktion {}: der behaltene Ordner {keep} liegt in einem Ordner, der selbst entfernt wird",
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
                if moved_with_dir(&dir_moves, a.id, &target_key) {
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
    fn validate_fields(&self, a: &PlannedAction, key: &str, keys: &Keys) -> Result<(), PlanError> {
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
        let is_dir_dedupe = self.kind == PlanKind::DedupeDirs;
        if a.keep_fingerprint.is_some() && !(is_dir_dedupe && a.action == ActionType::Quarantine) {
            return invalid("keep_fingerprint gibt es nur bei dedupe-dirs".into());
        }
        if let Some(print) = &a.source_fingerprint {
            if !(is_dir_dedupe && a.action == ActionType::Quarantine) {
                return invalid("source_fingerprint gibt es nur bei dedupe-dirs".into());
            }
            if !is_hash(print) {
                return invalid("source_fingerprint ist kein xxh3-128-Hex".into());
            }
        }
        match a.action {
            ActionType::Quarantine => {
                if a.is_dir && !is_dir_dedupe {
                    return invalid(
                        "Ordner-Quarantäne gibt es nur in Plänen der Art dedupe-dirs".into(),
                    );
                }
                if is_dir_dedupe {
                    if !a.is_dir {
                        return invalid(
                            "ein dedupe-dirs-Plan enthält nur Ordner-Quarantänen".into(),
                        );
                    }
                    if a.files.is_none() {
                        return invalid("Ordner braucht die Dateianzahl (files)".into());
                    }
                    if !a.keep_fingerprint.as_deref().is_some_and(is_hash) {
                        return invalid("Ordner braucht keep_fingerprint (xxh3-128-Hex)".into());
                    }
                }
                let present = [&a.hash, &a.keep, &a.keep_hash]
                    .iter()
                    .filter(|f| f.is_some())
                    .count();
                if present != 0 && present != 3 {
                    return invalid("hash, keep und keep_hash gelten nur gemeinsam".into());
                }
                if matches!(self.kind, PlanKind::Dedupe | PlanKind::DedupeDirs) && present == 0 {
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
                    let keep_key = paths::path_key(Path::new(keep));
                    if keep_key == key {
                        return invalid("Pfad entspricht der behaltenen Datei".into());
                    }
                    if a.is_dir
                        && (paths::is_under(key, &keep_key) || paths::is_under(&keep_key, key))
                    {
                        return invalid("Ordner liegt im oder über dem behaltenen Ordner".into());
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

fn read_text(path: &Path) -> Result<String, PlanError> {
    std::fs::read_to_string(paths::extended(path))
        .map_err(|e| PlanError::Unreadable(format!("{}: {e}", paths::display(path))))
}

/// `key` und seine Vorfahren (je Ebene bis vor das letzte `\`), von innen nach außen.
fn ancestors_inclusive(key: &str) -> impl Iterator<Item = &str> {
    let mut next = Some(key);
    std::iter::from_fn(move || {
        let current = next?;
        next = current.rfind('\\').map(|i| &current[..i]);
        Some(current)
    })
}

/// Liegt `target_key` auf oder in einem Ordner, den eine andere Aktion als `id` verschiebt?
/// Gleichwertig zu `paths::is_under(target_key, ordner)` für jeden Ordner der Aktionen, nur über
/// die Vorfahren des Ziels statt über alle Aktionen.
fn moved_with_dir(dir_moves: &HashMap<String, Vec<u32>>, id: u32, target_key: &str) -> bool {
    ancestors_inclusive(target_key).any(|ancestor| {
        dir_moves
            .get(ancestor)
            .is_some_and(|ids| ids.iter().any(|o| *o != id))
    })
}

/// Aktionen, die scheitern bzw. übersprungen werden, wenn `id` aus dem Plan fehlt: `remove-dir`
/// auf Vorfahren-Ordnern (der Ordner wäre nicht mehr leer) und das Verschieben ganzer
/// Vorfahren-Ordner (sie nähmen die abgewählte Aktion mit).
pub fn dependents(plan: &Plan, id: u32) -> Vec<u32> {
    let Some(own) = plan.actions.iter().find(|a| a.id == id) else {
        return Vec::new();
    };
    let own_key = paths::path_key(Path::new(&own.path));
    plan.actions
        .iter()
        .filter(|a| a.id != id && (a.action == ActionType::RemoveDir || a.is_dir))
        .filter(|a| {
            let key = paths::path_key(Path::new(&a.path));
            key != own_key && paths::is_under(&own_key, &key)
        })
        .map(|a| a.id)
        .collect()
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
            keep_fingerprint: None,
            source_fingerprint: None,
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
            protected_paths: Vec::new(),
            actions,
            skipped: vec![Skipped {
                path: r"D:\Daten\x.txt".into(),
                reason: SkipReason::Protected,
                detail: None,
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
    fn verschobener_ordner_wird_wie_is_under_erkannt() {
        // (id, Pfad) der verschobenen Ordner; `\a\b` kommt doppelt vor (zwei IDs).
        let dirs: [(u32, &str); 5] = [
            (1, r"D:\Daten\a"),
            (2, r"D:\Daten\a\b"),
            (3, r"D:\Daten\a\b"),
            (4, r"D:\Daten\x y"),
            (5, r"D:\Daten\Gross\Klein"),
        ];
        let targets = [
            r"D:\Daten\a",
            r"D:\Daten\a\datei.txt",
            r"D:\Daten\a\b",
            r"D:\Daten\a\b\c\d.txt",
            r"D:\Daten\ab",
            r"D:\Daten\ab\datei.txt",
            r"D:\Daten\x y\z",
            r"D:\Daten\x",
            r"D:\Daten\gross\klein\f.txt",
            r"D:\Daten\GROSS",
            r"D:\Daten",
            r"D:\",
            r"E:\Daten\a\f.txt",
        ];
        let mut dir_moves: HashMap<String, Vec<u32>> = HashMap::new();
        for (id, path) in dirs {
            dir_moves
                .entry(paths::path_key(Path::new(path)))
                .or_default()
                .push(id);
        }
        for target in targets {
            let target_key = paths::path_key(Path::new(target));
            for id in 0..=6 {
                let reference = dirs.iter().any(|(o, path)| {
                    *o != id && paths::is_under(&target_key, &paths::path_key(Path::new(path)))
                });
                assert_eq!(
                    moved_with_dir(&dir_moves, id, &target_key),
                    reference,
                    "Ziel {target}, Aktion {id}"
                );
            }
        }
    }

    #[test]
    fn schutzpfade_sind_optional_und_bleiben_im_teilplan() {
        let p = ok_plan();
        assert!(
            !p.to_json().contains("protected_paths"),
            "leer: nicht schreiben"
        );
        // Pläne ohne das Feld (ältere Versionen) bleiben lesbar.
        assert!(Plan::from_json(&p.to_json())
            .unwrap()
            .protected_paths
            .is_empty());

        let mut p = p;
        p.protected_paths = vec![r"D:\Daten\wichtig".into()];
        let back = Plan::from_json(&p.to_json()).unwrap();
        assert_eq!(back.protected_paths, [r"D:\Daten\wichtig"]);
        let keep: HashSet<u32> = [1].into();
        assert_eq!(p.subset(&keep).protected_paths, [r"D:\Daten\wichtig"]);
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
        for version in [0, 4] {
            let mut v: serde_json::Value = serde_json::from_str(&ok_plan().to_json()).unwrap();
            v["version"] = version.into();
            v["neues_feld"] = true.into();
            let err = Plan::from_json(&v.to_string()).unwrap_err();
            assert!(
                matches!(err, PlanError::UnsupportedVersion { expected: 3, .. }),
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
    fn subset_behaelt_alles_ausser_den_abgewaehlten_aktionen() {
        let mut p = ok_plan();
        p.params.insert("x".into(), "y".into());
        let keep: HashSet<u32> = [2].into();
        let sub = p.subset(&keep);
        assert_eq!(sub.actions.len(), 1);
        assert_eq!(sub.actions[0].id, 2);
        assert_eq!(sub.skipped, p.skipped);
        assert_eq!(sub.params, p.params);
        assert_eq!(
            (sub.kind, &sub.root, &sub.created, &sub.keep_strategy),
            (p.kind, &p.root, &p.created, &p.keep_strategy)
        );
        assert!(sub.validate().is_ok());
        assert_eq!(p.actions.len(), 2, "Original bleibt unverändert");
    }

    #[test]
    fn leere_auswahl_ergibt_gueltigen_leeren_plan() {
        let sub = ok_plan().subset(&HashSet::new());
        assert!(sub.actions.is_empty());
        assert!(sub.validate().is_ok());
    }

    #[test]
    fn dependents_von_leeren_ordnern_sind_die_vorfahren() {
        let p = plan_of(
            PlanKind::EmptyDirs,
            vec![
                remove_dir(1, r"D:\Daten\a\b\c"),
                remove_dir(2, r"D:\Daten\a\b"),
                remove_dir(3, r"D:\Daten\a"),
                remove_dir(4, r"D:\Daten\ab"),
            ],
        );
        let mut deps = dependents(&p, 1);
        deps.sort();
        assert_eq!(deps, vec![2, 3]);
        assert!(dependents(&p, 3).is_empty());
        assert!(dependents(&p, 4).is_empty());
    }

    #[test]
    fn dependents_bei_ordner_verschiebung_sind_die_ordner_vorfahren() {
        let mut outer = mv(1, r"D:\Daten\alt", r"D:\Daten\_Archiv\alt");
        outer.is_dir = true;
        outer.files = Some(2);
        let inner = mv(2, r"D:\Daten\alt\x.txt", r"D:\Daten\_Archiv\x.txt");
        let p = plan_of(PlanKind::Archive, vec![outer, inner]);
        assert_eq!(dependents(&p, 2), vec![1]);
        assert!(dependents(&p, 1).is_empty());
    }

    #[test]
    fn dependents_ohne_abhaengigkeiten_und_unbekannte_id() {
        let p = ok_plan();
        assert!(dependents(&p, 1).is_empty());
        assert!(dependents(&p, 99).is_empty());
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

    const FP: &str = "fedcba9876543210fedcba9876543210";

    fn dir_action(id: u32, path: &str, keep: &str) -> PlannedAction {
        PlannedAction {
            reason: "exact-duplicate-dir".into(),
            is_dir: true,
            files: Some(3),
            keep_fingerprint: Some(FP.into()),
            source_fingerprint: None,
            ..action(id, path, keep)
        }
    }

    fn dirs_plan(actions: Vec<PlannedAction>) -> Plan {
        plan_of(PlanKind::DedupeDirs, actions)
    }

    fn invalid_message(p: &Plan) -> String {
        match p.validate() {
            Err(PlanError::Invalid(msg)) => msg,
            other => panic!("erwartet Invalid, bekam {other:?}"),
        }
    }

    #[test]
    fn dedupe_dirs_plan_ist_verlustfrei_serialisierbar() {
        let p = dirs_plan(vec![dir_action(1, r"D:\Daten\Kopie von P", r"D:\Daten\P")]);
        assert!(p.validate().is_ok());
        let json = p.to_json();
        assert!(json.contains("\"dedupe-dirs\""));
        assert!(json.contains("keep_fingerprint"));
        assert_eq!(Plan::from_json(&json).unwrap(), p);
    }

    #[test]
    fn ordner_quarantaene_braucht_alle_pflichtfelder() {
        let base = || dir_action(1, r"D:\Daten\B", r"D:\Daten\A");
        let mut no_fp = base();
        no_fp.keep_fingerprint = None;
        assert!(invalid_message(&dirs_plan(vec![no_fp])).contains("keep_fingerprint"));
        let mut no_files = base();
        no_files.files = None;
        assert!(invalid_message(&dirs_plan(vec![no_files])).contains("files"));
        let mut no_hash = base();
        (no_hash.hash, no_hash.keep, no_hash.keep_hash) = (None, None, None);
        assert!(!invalid_message(&dirs_plan(vec![no_hash])).is_empty());
        let mut bad_fp = base();
        bad_fp.keep_fingerprint = Some("zz".into());
        assert!(invalid_message(&dirs_plan(vec![bad_fp])).contains("keep_fingerprint"));
    }

    #[test]
    fn dedupe_dirs_plan_enthaelt_nur_ordner_quarantaenen() {
        let file_action = action(1, r"D:\Daten\B\x.txt", r"D:\Daten\A\x.txt");
        assert!(!invalid_message(&dirs_plan(vec![file_action])).is_empty());
    }

    #[test]
    fn behaltener_ordner_in_einem_entfernten_ordner_ist_ungueltig() {
        let p = dirs_plan(vec![
            dir_action(1, r"D:\Daten\B", r"D:\Daten\M"),
            dir_action(2, r"D:\Daten\X", r"D:\Daten\B\k"),
        ]);
        assert!(invalid_message(&p).contains("behaltene Ordner"));
    }

    #[test]
    fn quelle_im_behaltenen_ordner_einer_anderen_aktion_ist_ungueltig() {
        let p = dirs_plan(vec![
            dir_action(1, r"D:\Daten\X\k", r"D:\Daten\W"),
            dir_action(2, r"D:\Daten\Y", r"D:\Daten\X"),
        ]);
        assert!(invalid_message(&p).contains("im behaltenen Ordner"));
    }

    #[test]
    fn verschachtelte_quellen_sind_in_dedupe_dirs_ungueltig() {
        let p = dirs_plan(vec![
            dir_action(1, r"D:\Daten\B", r"D:\Daten\A"),
            dir_action(2, r"D:\Daten\B\sub", r"D:\Daten\A\sub"),
        ]);
        assert!(invalid_message(&p).contains("selbst entfernt wird"));
    }

    #[test]
    fn source_fingerprint_gibt_es_nur_bei_dedupe_dirs_und_muss_ein_hash_sein() {
        let mut ok = dir_action(1, r"D:\Daten\B", r"D:\Daten\A");
        ok.source_fingerprint = Some(FP.into());
        assert!(dirs_plan(vec![ok.clone()]).validate().is_ok());
        assert!(dirs_plan(vec![ok.clone()])
            .to_json()
            .contains("source_fingerprint"));

        let mut bad = ok.clone();
        bad.source_fingerprint = Some("zz".into());
        assert!(invalid_message(&dirs_plan(vec![bad])).contains("source_fingerprint"));

        let mut elsewhere = action(1, r"D:\Daten\B\x.txt", r"D:\Daten\A\x.txt");
        elsewhere.source_fingerprint = Some(FP.into());
        assert!(invalid_message(&plan(vec![elsewhere])).contains("source_fingerprint"));
    }

    #[test]
    fn plan_ohne_source_fingerprint_bleibt_gueltig_und_schreibt_das_feld_nicht() {
        let p = dirs_plan(vec![dir_action(1, r"D:\Daten\B", r"D:\Daten\A")]);
        assert!(p.validate().is_ok());
        assert!(!p.to_json().contains("source_fingerprint"));
    }

    #[test]
    fn behaltene_ordner_neben_entfernten_sind_gueltig() {
        let p = dirs_plan(vec![
            dir_action(1, r"D:\Daten\B", r"D:\Daten\A"),
            dir_action(2, r"D:\Daten\C", r"D:\Daten\A"),
            dir_action(3, r"D:\Daten\Bx", r"D:\Daten\A\k"),
        ]);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn ordner_quarantaene_gibt_es_nur_in_dedupe_dirs() {
        let p = plan(vec![dir_action(1, r"D:\Daten\B", r"D:\Daten\A")]);
        assert!(invalid_message(&p).contains("dedupe-dirs"));
    }

    #[test]
    fn keep_fingerprint_ist_ausserhalb_von_dedupe_dirs_verboten() {
        let mut a = action(1, r"D:\Daten\b\x.txt", r"D:\Daten\a\x.txt");
        a.keep_fingerprint = Some(FP.into());
        assert!(invalid_message(&plan(vec![a])).contains("keep_fingerprint"));
    }

    #[test]
    fn ordner_aktion_darf_nicht_in_oder_ueber_dem_behaltenen_ordner_liegen() {
        let inside = dirs_plan(vec![dir_action(1, r"D:\Daten\A\sub", r"D:\Daten\A")]);
        assert!(invalid_message(&inside).contains("behalten"));
        let above = dirs_plan(vec![dir_action(1, r"D:\Daten", r"D:\Daten\A")]);
        assert!(!invalid_message(&above).is_empty());
        let same = dirs_plan(vec![dir_action(1, r"D:\Daten\A", r"D:\Daten\A")]);
        assert!(!invalid_message(&same).is_empty());
        let sibling_prefix = dirs_plan(vec![dir_action(1, r"D:\Daten\AB", r"D:\Daten\A")]);
        assert!(
            sibling_prefix.validate().is_ok(),
            "A und AB sind Geschwister"
        );
    }

    #[test]
    fn ordner_quarantaene_ist_abhaengigkeit_der_darin_liegenden_aktion() {
        let outer = dir_action(1, r"D:\Daten\B", r"D:\Daten\A");
        let inner = dir_action(2, r"D:\Daten\B\sub", r"D:\Daten\A\sub");
        let p = dirs_plan(vec![outer, inner]);
        assert_eq!(dependents(&p, 2), vec![1]);
        assert!(dependents(&p, 1).is_empty());
    }

    #[test]
    fn plan_ohne_keep_fingerprint_bleibt_gueltig_und_schreibt_das_feld_nicht() {
        let json = ok_plan().to_json();
        assert!(!json.contains("keep_fingerprint"));
        assert!(Plan::from_json(&json).is_ok());
    }

    fn value(p: &Plan) -> serde_json::Value {
        serde_json::from_str(&p.to_json()).unwrap()
    }

    #[test]
    fn gespeicherter_plan_ist_v3_und_beginnt_mit_der_pruefsumme() {
        let json = ok_plan().to_json();
        assert!(json.starts_with("{\n  \"integrity\": \"sha256:"), "{json}");
        let v = value(&ok_plan());
        assert_eq!(v["version"], 3);
        assert!(is_integrity(v["integrity"].as_str().unwrap()));
        assert_eq!(v["integrity"], ok_plan().integrity());
    }

    #[test]
    fn alter_plan_wird_beim_speichern_auf_v3_gehoben() {
        let old = Plan::from_json(V1_JSON).unwrap();
        let (back, seal) = Plan::parse(&old.to_json()).unwrap();
        assert_eq!(back.version, PLAN_VERSION);
        assert_eq!(seal, Seal::Valid);
    }

    #[test]
    fn formatierung_zeilenenden_schluesselreihenfolge_und_null_aendern_den_hash_nicht() {
        let p = ok_plan();
        let pretty = p.to_json();
        let compact_sorted = value(&p).to_string();
        assert!(
            compact_sorted.starts_with("{\"actions\""),
            "Schlüssel umgestellt"
        );
        let crlf = pretty.replace('\n', "\r\n");
        let mut with_null = value(&p);
        with_null["actions"][0]["target"] = serde_json::Value::Null;
        with_null["params"] = serde_json::json!({});
        for text in [pretty, compact_sorted, crlf, with_null.to_string()] {
            assert_eq!(Plan::from_json(&text).unwrap(), p);
            assert_eq!(Plan::parse(&text).unwrap().1, Seal::Valid);
        }
    }

    fn assert_tampered(v: &serde_json::Value) {
        match Plan::from_json(&v.to_string()) {
            Err(e @ PlanError::Tampered { .. }) => {
                let msg = e.to_string();
                assert!(msg.contains("verändert"), "{msg}");
                assert!(msg.contains("plan seal"), "{msg}");
                assert!(!msg.contains("Daten"), "keine Pfade: {msg}");
            }
            other => panic!("erwartet Tampered, bekam {other:?}"),
        }
    }

    #[test]
    fn inhaltliche_aenderungen_ergeben_tampered() {
        let mut changed = value(&ok_plan());
        changed["actions"][0]["size"] = 11.into();
        assert_tampered(&changed);

        let mut removed = value(&ok_plan());
        removed["actions"].as_array_mut().unwrap().pop();
        assert_tampered(&removed);

        let mut added = value(&ok_plan());
        let mut extra = added["actions"][0].clone();
        extra["id"] = 3.into();
        extra["path"] = r"D:\Daten\d\kopie.txt".into();
        added["actions"].as_array_mut().unwrap().push(extra);
        assert_tampered(&added);

        let mut skipped = value(&ok_plan());
        skipped["skipped"] = serde_json::json!([]);
        assert_tampered(&skipped);
    }

    #[test]
    fn veraenderter_und_ungueltiger_plan_wird_als_veraendert_gemeldet() {
        let mut v = value(&ok_plan());
        v["actions"][1]["id"] = 1.into();
        assert_tampered(&v);
    }

    #[test]
    fn tampered_meldung_nennt_gekuerzte_pruefsummen() {
        let mut v = value(&ok_plan());
        let stored = v["integrity"].as_str().unwrap().to_owned();
        v["actions"][0]["size"] = 11.into();
        let err = Plan::from_json(&v.to_string()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(&format!("{}…", &stored[7..19])), "{msg}");
        assert!(!msg.contains(&stored[7..20]), "nur 12 Zeichen: {msg}");
    }

    #[test]
    fn v3_ohne_pruefsumme_ist_ungueltig() {
        let mut v = value(&ok_plan());
        v.as_object_mut().unwrap().remove("integrity");
        assert!(matches!(
            Plan::from_json(&v.to_string()),
            Err(PlanError::Invalid(m)) if m.contains("integrity")
        ));
        assert_eq!(Plan::parse(&v.to_string()).unwrap().1, Seal::Missing);
    }

    #[test]
    fn falsch_formatierte_pruefsumme_ist_ungueltig() {
        let good = ok_plan().integrity();
        let hex = &good[7..];
        for bad in [
            serde_json::Value::from(format!("sha1:{hex}")),
            format!("sha256:{}", &hex[1..]).into(),
            format!("sha256:{hex}0").into(),
            format!("sha256:{}", hex.to_uppercase()).into(),
            format!("sha256:{}g", &hex[1..]).into(),
            hex.into(),
            1.into(),
            serde_json::Value::Null,
        ] {
            let mut v = value(&ok_plan());
            v["integrity"] = bad.clone();
            assert!(
                matches!(Plan::from_json(&v.to_string()), Err(PlanError::Invalid(_))),
                "{bad}"
            );
            assert!(
                matches!(Plan::parse(&v.to_string()), Err(PlanError::Invalid(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn alte_versionen_mit_pruefsumme_sind_ungueltig() {
        for version in [1, 2] {
            let mut v = value(&ok_plan());
            v["version"] = version.into();
            assert!(
                matches!(Plan::from_json(&v.to_string()), Err(PlanError::Invalid(m)) if m.contains("integrity")),
                "Version {version}"
            );
        }
    }

    #[test]
    fn alte_versionen_ohne_pruefsumme_sind_ungeschuetzt() {
        let (p, seal) = Plan::parse(V1_JSON).unwrap();
        assert_eq!(p.version, 1);
        assert_eq!(seal, Seal::Unprotected { version: 1 });

        let mut v2 = value(&ok_plan());
        v2.as_object_mut().unwrap().remove("integrity");
        v2["version"] = 2.into();
        let text = v2.to_string();
        let mut expected = ok_plan();
        expected.version = 2;
        assert_eq!(Plan::from_json(&text).unwrap(), expected);
        assert_eq!(
            Plan::parse(&text).unwrap().1,
            Seal::Unprotected { version: 2 }
        );
    }

    #[test]
    fn parse_meldet_abweichung_ohne_abzulehnen() {
        let mut v = value(&ok_plan());
        let stored = v["integrity"].as_str().unwrap().to_owned();
        v["actions"][0]["size"] = 11.into();
        let (p, seal) = Plan::parse(&v.to_string()).unwrap();
        assert_eq!(p.actions[0].size, 11);
        assert_eq!(
            seal,
            Seal::Mismatch {
                stored,
                actual: p.integrity()
            }
        );
    }

    #[test]
    fn parse_prueft_die_struktur() {
        let mut v = value(&ok_plan());
        v["actions"][1]["id"] = 1.into();
        assert!(matches!(
            Plan::parse(&v.to_string()),
            Err(PlanError::Invalid(m)) if m.contains("doppelt")
        ));
    }

    /// Hängt der Hash an der Rust-Struktur (Feldreihenfolge, Defaults), schlägt dieser Test an.
    /// Jede solche Änderung braucht eine neue Formatversion.
    #[test]
    fn eingecheckter_v3_plan_bleibt_gueltig() {
        const V3_JSON: &str = r#"{
  "integrity": "sha256:2c49d9c9cb030472cb412c6538ed6c2115d5f4cef24ffa528c5b3871e4fa58c1",
  "version": 3,
  "created": "2026-10-03T12:00:00+02:00",
  "kind": "archive",
  "root": "D:\\Daten",
  "params": {
    "older_than": "2y"
  },
  "protected_paths": [
    "D:\\Daten\\wichtig"
  ],
  "actions": [
    {
      "id": 1,
      "type": "move",
      "path": "D:\\Daten\\alt",
      "size": 10,
      "mtime_ticks": 17000000000000000,
      "mtime": "2023-11-14T23:13:20+01:00",
      "reason": "archive",
      "target": "D:\\Daten\\_Archiv\\2023\\alt",
      "is_dir": true,
      "files": 4
    }
  ],
  "skipped": [
    {
      "path": "D:\\Daten\\x.txt",
      "reason": "protected",
      "detail": "Test"
    }
  ]
}"#;
        let (p, seal) = Plan::parse(V3_JSON).unwrap();
        assert_eq!(seal, Seal::Valid, "{}", p.integrity());
        assert_eq!(p.to_json(), V3_JSON);
    }

    #[test]
    fn save_replacing_ersetzt_den_inhalt_ohne_restdatei() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.json");
        std::fs::write(&path, "alter inhalt").unwrap();
        let p = ok_plan();
        p.save_replacing(&path).unwrap();
        assert_eq!(Plan::load(&path).unwrap(), p);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["plan.json"], "keine .tmp-Datei bleibt zurück");
    }

    #[test]
    fn read_unverified_meldet_den_zustand_der_datei() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.json");
        let mut v = value(&ok_plan());
        v["actions"][0]["size"] = 11.into();
        std::fs::write(&path, v.to_string()).unwrap();
        let (p, seal) = Plan::read_unverified(&path).unwrap();
        assert_eq!(p.actions[0].size, 11);
        assert!(matches!(seal, Seal::Mismatch { .. }));
        assert!(matches!(
            Plan::read_unverified(&dir.path().join("fehlt.json")),
            Err(PlanError::Unreadable(_))
        ));
    }

    #[test]
    fn unbekannte_version_4_wird_abgelehnt() {
        let mut v = value(&ok_plan());
        v["version"] = 4.into();
        assert!(matches!(
            Plan::from_json(&v.to_string()),
            Err(PlanError::UnsupportedVersion {
                found: 4,
                expected: 3
            })
        ));
    }
}
