//! Momentaufnahme nach einem vollständigen Scan: Kennzahlen berechnen, in den Verlauf schreiben
//! und mit dem letzten vergleichbaren Lauf vergleichen.

use std::path::Path;

use anyhow::{Context, Result};

use super::plan::downloads_dirs;
use super::{now_rfc3339, now_ticks};
use crate::config::Config;
use crate::health::{self, CollectEnv, Snapshot, METRICS_VERSION};
use crate::history::{Comparable, History, SnapshotMeta, Stored};
use crate::index::Index;
use crate::paths;
use crate::report::history::{self as report_history, Comparison};
use crate::report::{self, Report, ReportRoot};

/// Länge der Reihen für die Trendgrafiken im Bericht (ein Jahr bei wöchentlichen Läufen).
const SERIES_LIMIT: usize = 52;

/// Was eine Momentaufnahme dem Nutzer zu sagen hat.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Recorded {
    pub snapshot: Snapshot,
    pub previous: Option<(Stored, Comparable)>,
}

impl Recorded {
    pub fn score(&self) -> u8 {
        self.snapshot.root.score.total
    }

    pub fn comparison(&self) -> Option<Comparison> {
        self.previous
            .as_ref()
            .map(|(p, kind)| Comparison::new(self.score(), p, *kind))
    }
}

/// Berechnet die Kennzahlen aus dem Index, schreibt sie in den Verlauf und liefert sie samt
/// Vergleichslauf. Fehler im Verlauf brechen den Scan nicht ab; der Aufrufer meldet sie.
pub(super) fn record(
    index: &Index,
    root: &Path,
    config: &Config,
    profile: Option<&str>,
    scan_errors: u64,
) -> Result<Recorded> {
    let scanned = report::select_root(index, Some(root))?;
    let snapshot = health::collect(
        index,
        &scanned,
        &CollectEnv {
            config,
            downloads_dirs: &downloads_dirs(config),
            now_ticks: now_ticks(),
        },
    )?;
    let fp = health::fingerprint(config, None);
    let mut history = History::open(&paths::history_path()?)?;
    let previous = history.latest_comparable(&scanned.dir_key, &fp, None)?;
    history
        .write(
            &snapshot,
            &SnapshotMeta {
                root_key: scanned.dir_key.clone(),
                root_path: scanned.path.clone(),
                taken_at: now_rfc3339(),
                scan_finished_at: scanned.scanned_at.clone(),
                tool_version: env!("CARGO_PKG_VERSION").to_string(),
                metrics_version: METRICS_VERSION,
                config_fp: fp,
                template: None,
                profile: profile.map(String::from),
                scan_errors: i64::try_from(scan_errors).unwrap_or(i64::MAX),
            },
        )
        .context("Momentaufnahme nicht geschrieben")?;
    Ok(Recorded { snapshot, previous })
}

/// Ergänzt den Bericht um den Abschnitt „Verlauf“. Ohne Verlauf (Fehler, Unterordner) bleibt er
/// leer bzw. nennt den Grund; der Bericht entsteht in jedem Fall.
pub(super) fn attach_history(
    model: &mut Report,
    index: &Index,
    root: &ReportRoot,
    config: &Config,
) -> Result<()> {
    // Nur ganze gescannte Wurzeln haben einen Verlauf, keine Unterordner davon.
    if !index.roots()?.iter().any(|r| r.path_key == root.dir_key) {
        return Ok(());
    }
    let now = health::collect(
        index,
        root,
        &CollectEnv {
            config,
            downloads_dirs: &downloads_dirs(config),
            now_ticks: now_ticks(),
        },
    )?;
    let fp = health::fingerprint(config, None);
    let mut notes = Vec::new();
    let mut previous = None;
    let mut series = Vec::new();
    match History::open(&paths::history_path()?) {
        Ok(history) => {
            let lookup = || -> Result<_, crate::history::HistoryError> {
                let own = match root.scanned_at.as_deref() {
                    Some(at) => history.snapshot_for_scan(&root.dir_key, at)?,
                    None => None,
                };
                Ok((
                    history.latest_comparable(&root.dir_key, &fp, own)?,
                    history.series(&root.dir_key, "", SERIES_LIMIT)?,
                ))
            };
            match lookup() {
                Ok((p, s)) => {
                    previous = p;
                    series = s;
                }
                Err(e) => notes.push(format!("Verlauf nicht lesbar: {e}")),
            }
        }
        Err(e) => notes.push(format!("Verlauf nicht verfügbar: {e}")),
    }
    model.history = Some(report_history::section(
        &now,
        previous.as_ref().map(|(p, kind)| (p, *kind)),
        &series,
        notes,
    ));
    Ok(())
}
