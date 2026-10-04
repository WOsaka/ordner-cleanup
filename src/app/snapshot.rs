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
use crate::report;

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
}

pub(super) fn format_delta(delta: i32) -> String {
    match delta {
        0 => "±0".to_string(),
        d if d > 0 => format!("+{d}"),
        d => format!("−{}", -d),
    }
}

/// „27.09.“ in Ortszeit aus einem RFC-3339-Zeitpunkt.
pub(super) fn short_date(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| t.with_timezone(&chrono::Local).format("%d.%m.").to_string())
        .unwrap_or_else(|_| rfc3339.to_string())
}

/// „Health-Score 72 (−5 seit 27.09.)“
pub(super) fn score_line(score: u8, previous: Option<&(Stored, Comparable)>) -> String {
    match previous {
        None => format!("Health-Score {score} (erster Lauf)"),
        Some((prev, kind)) => {
            let delta = i32::from(score) - i32::from(prev.snapshot.root.score.total);
            let limited = match kind {
                Comparable::Full => "",
                Comparable::Limited => ", eingeschränkt vergleichbar",
            };
            format!(
                "Health-Score {score} ({} seit {}{limited})",
                format_delta(delta),
                short_date(&prev.meta.taken_at)
            )
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{score::score, Area, Metrics, Weights};

    fn stored(junk_files: u64) -> Stored {
        let metrics = Metrics {
            files: 100,
            size: 1000,
            local_bytes: 1000,
            junk_files,
            ..Metrics::default()
        };
        let area = Area {
            folder: String::new(),
            score: score(&metrics, &Weights::default()),
            metrics,
        };
        Stored {
            id: 1,
            meta: SnapshotMeta {
                root_key: "d:\\".into(),
                root_path: "D:\\".into(),
                taken_at: "2026-09-27T12:00:00Z".into(),
                scan_finished_at: None,
                tool_version: "0".into(),
                metrics_version: METRICS_VERSION,
                config_fp: "fp".into(),
                template: None,
                profile: None,
                scan_errors: 0,
            },
            snapshot: Snapshot {
                root: area,
                folders: vec![],
            },
        }
    }

    #[test]
    fn deltas_werden_mit_vorzeichen_formatiert() {
        assert_eq!(format_delta(0), "±0");
        assert_eq!(format_delta(3), "+3");
        assert_eq!(format_delta(-5), "−5");
    }

    #[test]
    fn erste_zeile_ohne_vergleich() {
        assert_eq!(score_line(72, None), "Health-Score 72 (erster Lauf)");
    }

    #[test]
    fn zeile_nennt_veraenderung_und_datum() {
        let prev = stored(0);
        assert_eq!(prev.snapshot.root.score.total, 100);
        let line = score_line(95, Some(&(prev, Comparable::Full)));
        assert!(line.starts_with("Health-Score 95 (−5 seit "), "{line}");
        assert!(line.ends_with(".)"), "{line}");
    }

    #[test]
    fn eingeschraenkter_vergleich_wird_benannt() {
        let line = score_line(100, Some(&(stored(0), Comparable::Limited)));
        assert!(line.contains("±0"), "{line}");
        assert!(line.ends_with(", eingeschränkt vergleichbar)"), "{line}");
    }
}
