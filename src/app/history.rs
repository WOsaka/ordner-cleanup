//! Befehl `history`: eine Zeile je Momentaufnahme.

use std::fmt::Write;

use anyhow::Result;
use bytesize::ByteSize;

use crate::cli::HistoryArgs;
use crate::history::Point;
use crate::ops::local_time;

/// Die Tabelle für `history`; `points` sind chronologisch (älteste zuerst).
fn render(root_path: &str, folder: &str, points: &[Point]) -> String {
    let scope = if folder.is_empty() {
        "Wurzel".to_string()
    } else if folder == crate::health::OTHER_FOLDER {
        "Sonstige Ordner".to_string()
    } else {
        format!("Ordner {folder}")
    };
    let mut s = String::new();
    let _ = writeln!(s, "Verlauf für {root_path} ({scope})");
    let _ = writeln!(
        s,
        "{:<16}  {:>5}  {:>9}  {:>10}  {:>6}  {:>10}  {:>8}",
        "Datum", "Score", "Dateien", "Größe", "Müll", "Duplikate", "Probleme"
    );
    for p in points {
        let m = &p.metrics;
        let _ = writeln!(
            s,
            "{:<16}  {:>5}  {:>9}  {:>10}  {:>6}  {:>10}  {:>8}",
            local_time(&p.taken_at),
            p.score,
            m.files,
            ByteSize::b(m.size).to_string(),
            m.junk_files,
            ByteSize::b(m.dup_wasted).to_string(),
            m.problem_files
        );
    }
    s
}

pub(super) fn history_command(args: &HistoryArgs) -> Result<i32> {
    let view = crate::ops::admin::history(
        args.path.as_deref(),
        args.profile.as_deref(),
        args.folder.as_deref(),
        args.limit,
    )?;
    print!("{}", render(&view.root_path, &view.folder, &view.points));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{Metrics, METRICS_VERSION};

    fn point(day: u32, score: u8, junk: u64) -> Point {
        Point {
            id: i64::from(day),
            taken_at: format!("2026-10-{day:02}T12:00:00Z"),
            config_fp: "fp".into(),
            metrics_version: METRICS_VERSION,
            score,
            metrics: Metrics {
                files: 1234,
                size: 2048,
                junk_files: junk,
                dup_wasted: 1024,
                problem_files: 2,
                ..Metrics::default()
            },
        }
    }

    #[test]
    fn tabelle_hat_kopf_und_eine_zeile_je_punkt() {
        let text = render(r"D:\Daten", "", &[point(1, 80, 5), point(2, 75, 9)]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].contains(r"D:\Daten") && lines[0].contains("Wurzel"));
        assert!(lines[1].contains("Score") && lines[1].contains("Duplikate"));
        assert!(lines[2].contains("80") && lines[2].contains("1234"));
        assert!(lines[3].contains("75") && lines[3].contains("2.0 KiB"));
    }

    #[test]
    fn ordner_und_sonstige_stehen_in_der_ueberschrift() {
        assert!(render("D:\\", "Fotos", &[point(1, 1, 1)]).contains("Ordner Fotos"));
        assert!(render("D:\\", "*", &[point(1, 1, 1)]).contains("Sonstige Ordner"));
    }
}
