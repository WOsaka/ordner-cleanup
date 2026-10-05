//! Operationen: die Befehle von CLI und GUI als Funktionen mit Anfrage-Struktur und
//! strukturiertem Ergebnis. Keine Ausgabe auf der Konsole, keine Rückfragen: Hinweise und
//! Warnungen sind Daten (`Notes`), Abbruch und Fortschritt kommen über den `OpCtx` herein, und
//! Bestätigungen holt der Aufrufer zwischen „prüfen“ und „ausführen“.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{bail, Context, Result};

use crate::change::limits;
use crate::change::registry;
use crate::change::undo::RunStatus;
use crate::config::Config;
use crate::index::{Index, RootStatus};
use crate::paths::{self, index_path, registry_path};
use crate::platform::windows::{drive_kind, DriveKind};
use crate::scan::lock::{LockError, ScanLock};
use crate::scan::source::TICKS_PER_SEC;

pub mod admin;
pub mod apply;
pub mod classify;
pub mod plan;
pub mod report;
pub mod runs;
pub mod scan;
pub mod snapshot;
pub mod target;

/// Ab diesem Alter des letzten Scans weist `plan` auf einen möglicherweise veralteten Index hin.
pub const STALE_SCAN_DAYS: i64 = 7;

/// Fehler, die eine Oberfläche anders behandelt als einen Text: sie stecken in `anyhow::Error`
/// und lassen sich mit `downcast_ref` erkennen.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Ein anderer Scan hält die Sperre.
    #[error("Ein anderer Scan läuft bereits ({})", .0.display())]
    Busy(PathBuf),
    /// Es gibt noch keinen Index; erst scannen.
    #[error("Kein Index vorhanden. Bitte zuerst einen Ordner scannen.")]
    NoIndex,
    /// Der Plan überschreitet eine Obergrenze und `allow_large` fehlt.
    #[error("{0}")]
    LimitExceeded(String),
}

/// Fortschritt einer laufenden Operation. Die Operation schreibt, die Oberfläche liest je Frame.
#[derive(Debug, Default)]
pub struct TaskProgress {
    pub done: AtomicU64,
    /// Nur beim Scan: gefundene Ordner (`done` zählt dort die Dateien)
    pub dirs: AtomicU64,
    pub total: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    phase: Mutex<String>,
    current: Mutex<String>,
}

impl TaskProgress {
    pub fn set_phase(&self, text: impl Into<String>) {
        if let Ok(mut phase) = self.phase.lock() {
            *phase = text.into();
        }
    }

    pub fn set_current(&self, text: impl Into<String>) {
        if let Ok(mut current) = self.current.lock() {
            *current = text.into();
        }
    }

    pub fn phase(&self) -> String {
        self.phase.lock().map(|p| p.clone()).unwrap_or_default()
    }

    pub fn current(&self) -> String {
        self.current.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn reset(&self, phase: &str, total: u64) {
        self.done.store(0, Ordering::Relaxed);
        self.dirs.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.bytes.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
        self.set_phase(phase);
        self.set_current("");
    }

    pub fn inc(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }
}

/// Abbruch und Fortschritt einer Operation. Die CLI übergibt das Strg+C-Flag, die GUI je Task
/// ein frisches.
#[derive(Debug, Clone, Default)]
pub struct OpCtx {
    pub cancel: Arc<AtomicBool>,
    pub progress: Arc<TaskProgress>,
}

impl OpCtx {
    pub fn new(cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            progress: Arc::default(),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Hinweise (Information) und Warnungen eines Ergebnisses; der Aufrufer entscheidet über die
/// Darstellung.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Notes {
    pub hints: Vec<String>,
    pub warnings: Vec<String>,
}

impl Notes {
    pub fn hint(&mut self, text: impl Into<String>) {
        self.hints.push(text.into());
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.warnings.push(text.into());
    }

    pub fn is_empty(&self) -> bool {
        self.hints.is_empty() && self.warnings.is_empty()
    }
}

/// Nimmt die Scan-Sperre; ein belegtes Lock wird zu `Error::Busy`.
pub fn acquire_scan_lock() -> Result<ScanLock> {
    acquire_scan_lock_at(&paths::scan_lock_path()?)
}

fn acquire_scan_lock_at(path: &Path) -> Result<ScanLock> {
    match ScanLock::acquire(path) {
        Ok(lock) => Ok(lock),
        Err(LockError::Busy(path)) => Err(Error::Busy(path).into()),
        Err(e) => Err(e.into()),
    }
}

/// Hinweis, wenn die Wurzel unter einem OneDrive-Ordner liegt: Die Quarantäne läge dann im
/// synchronisierten Bereich und erzeugt Sync-Traffic.
pub fn onedrive_warning(root: &Path, onedrive_roots: &[PathBuf]) -> Option<String> {
    limits::under_onedrive(root, onedrive_roots).then(|| {
        "Warnung: Der Ordner liegt in OneDrive. Die Quarantäne (.ordner-cleanup) wird \
             mitsynchronisiert und erzeugt Sync-Traffic; Dateien lassen sich später mit \
             `purge` endgültig entsorgen."
            .to_string()
    })
}

pub fn onedrive_roots_from_env() -> Vec<PathBuf> {
    ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .filter_map(|v| std::env::var_os(v).map(|p| normalize(Path::new(&p))))
        .collect()
}

/// Hinweis zum Index-Zustand der Wurzel (kein Abbruch).
pub fn index_age_note(
    root: &crate::report::ReportRoot,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    if root.status != RootStatus::Complete {
        return Some(
            "Hinweis: Der letzte Scan dieses Ordners war nicht vollständig; der Plan kann \
             unvollständig sein. Bitte neu scannen."
                .to_string(),
        );
    }
    let scanned = chrono::DateTime::parse_from_rfc3339(root.scanned_at.as_deref()?).ok()?;
    let age = now.signed_duration_since(scanned).num_days();
    (age >= STALE_SCAN_DAYS).then(|| {
        format!("Hinweis: Der letzte Scan ist {age} Tage alt. Bitte neu scannen, falls sich viel geändert hat.")
    })
}

pub fn status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Complete => "vollständig",
        RunStatus::Partial => "teilweise",
        RunStatus::Incomplete => "unvollständig",
        RunStatus::Undone => "zurückgedreht",
        RunStatus::PartiallyUndone => "teilweise zurückgedreht",
        RunStatus::Purged => "Quarantäne gelöscht",
        RunStatus::Unreadable => "Journal unlesbar",
    }
}

pub fn local_time(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_string())
}

/// Wurzel eines Laufs: ausdrücklich angegeben oder aus dem Register.
pub fn find_run_root(run: &crate::change::RunId, explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(normalize(path));
    }
    match registry::find_root(&registry_path()?, run) {
        Some(root) => Ok(PathBuf::from(root)),
        None => bail!("Lauf {run} ist im Register unbekannt; Wurzel mit --root angeben"),
    }
}

/// Alle Wurzeln mit möglichen Läufen: Register plus gescannte Wurzeln, ohne Duplikate.
pub fn known_roots() -> Result<Vec<PathBuf>> {
    let mut roots = registry::known_roots(&registry_path()?);
    let index_file = index_path()?;
    if index_file.exists() {
        roots.extend(
            Index::open(&index_file)?
                .roots()?
                .into_iter()
                .map(|r| r.path),
        );
    }
    let mut seen = std::collections::HashSet::new();
    Ok(roots
        .into_iter()
        .filter(|r| seen.insert(paths::path_key(Path::new(r))))
        .map(PathBuf::from)
        .collect())
}

/// Ordner, in denen `installer` greift: die Config ersetzt den Known Folder.
pub fn downloads_dirs(config: &Config) -> Vec<PathBuf> {
    if config.downloads_dirs.is_empty() {
        crate::platform::windows::downloads_dir()
            .into_iter()
            .collect()
    } else {
        config
            .downloads_dirs
            .iter()
            .map(|d| normalize(Path::new(d)))
            .collect()
    }
}

pub fn load_config() -> Result<Config> {
    match paths::config_dir() {
        Some(dir) => Config::load(&dir.join("config.toml")),
        None => Ok(Config::default()),
    }
}

pub fn now_ticks() -> i64 {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    secs * TICKS_PER_SEC
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Löst Kurznamen (8.3), Symlinks und relative Pfade auf, damit Scan, Report und
/// `index remove` denselben Schlüssel verwenden. Nicht existierende Pfade bleiben unverändert.
pub fn normalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|p| PathBuf::from(paths::display(&p)))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Prüft die Wurzel und liefert den normalisierten Pfad.
pub fn resolve_root(path: &Path, force: bool) -> Result<PathBuf> {
    let meta = std::fs::metadata(paths::extended(path))
        .with_context(|| format!("Pfad nicht lesbar: {}", paths::display(path)))?;
    if !meta.is_dir() {
        bail!("{} ist kein Ordner", paths::display(path));
    }
    let absolute = normalize(path);
    if drive_kind(&absolute) != DriveKind::Local && !force {
        bail!(
            "{} liegt auf einem Netzlaufwerk bzw. UNC-Pfad. Das wird in Phase 1 nicht unterstützt; \
             mit --force trotzdem scannen",
            paths::display(&absolute)
        );
    }
    Ok(absolute)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(status: RootStatus, scanned_at: Option<&str>) -> crate::report::ReportRoot {
        crate::report::ReportRoot {
            path: r"D:\Daten".into(),
            dir_key: r"d:\daten\".into(),
            scanned_at: scanned_at.map(String::from),
            status,
        }
    }

    fn at(text: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn onedrive_warnung_nur_unterhalb_der_onedrive_wurzel() {
        let roots = [PathBuf::from(r"C:\Users\me\OneDrive")];
        assert!(onedrive_warning(Path::new(r"C:\Users\me\onedrive\Doku"), &roots).is_some());
        assert!(onedrive_warning(Path::new(r"C:\Users\me\OneDrive"), &roots).is_some());
        assert!(onedrive_warning(Path::new(r"C:\Users\me\OneDrive2\x"), &roots).is_none());
        assert!(onedrive_warning(Path::new(r"D:\Daten"), &roots).is_none());
        assert!(onedrive_warning(Path::new(r"D:\Daten"), &[]).is_none());
    }

    #[test]
    fn unvollstaendiger_scan_wird_gemeldet() {
        let r = root(RootStatus::Aborted, Some("2026-10-03T10:00:00Z"));
        assert!(index_age_note(&r, at("2026-10-03T12:00:00Z"))
            .unwrap()
            .contains("nicht vollständig"));
    }

    #[test]
    fn alter_scan_wird_ab_sieben_tagen_gemeldet() {
        let r = root(RootStatus::Complete, Some("2026-09-26T12:00:00Z"));
        assert!(index_age_note(&r, at("2026-10-03T12:00:00Z")).is_some());
        assert!(index_age_note(&r, at("2026-10-03T11:00:00Z")).is_none());
    }

    #[test]
    fn frischer_oder_undatierter_scan_ist_still() {
        let fresh = root(RootStatus::Complete, Some("2026-10-03T10:00:00Z"));
        assert!(index_age_note(&fresh, at("2026-10-03T12:00:00Z")).is_none());
        let undated = root(RootStatus::Complete, None);
        assert!(index_age_note(&undated, at("2026-10-03T12:00:00Z")).is_none());
    }

    #[test]
    fn fortschritt_wird_zurueckgesetzt_und_gelesen() {
        let p = TaskProgress::default();
        p.reset("Scan", 10);
        p.inc();
        p.inc();
        p.set_current(r"D:\x");
        assert_eq!(p.done.load(Ordering::Relaxed), 2);
        assert_eq!(p.total.load(Ordering::Relaxed), 10);
        assert_eq!(
            (p.phase().as_str(), p.current().as_str()),
            ("Scan", r"D:\x")
        );
        p.reset("Apply", 0);
        assert_eq!(p.done.load(Ordering::Relaxed), 0);
        assert_eq!(p.current(), "");
    }

    #[test]
    fn kontext_teilt_das_abbruch_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        let ctx = OpCtx::new(Arc::clone(&flag));
        assert!(!ctx.is_cancelled());
        flag.store(true, Ordering::Relaxed);
        assert!(ctx.is_cancelled());
        assert!(ctx.clone().is_cancelled(), "Klone teilen dasselbe Flag");
    }

    #[test]
    fn hinweise_und_warnungen_werden_gesammelt() {
        let mut notes = Notes::default();
        assert!(notes.is_empty());
        notes.hint("a");
        notes.warn("b");
        assert_eq!((notes.hints.len(), notes.warnings.len()), (1, 1));
        assert!(!notes.is_empty());
    }

    #[test]
    fn belegte_scan_sperre_wird_zu_busy() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("scan.lock");
        let first = acquire_scan_lock_at(&file).unwrap();
        let err = acquire_scan_lock_at(&file).unwrap_err();
        assert!(matches!(err.downcast_ref::<Error>(), Some(Error::Busy(_))));
        drop(first);
        assert!(acquire_scan_lock_at(&file).is_ok());
    }
}
