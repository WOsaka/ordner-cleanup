//! Ergebnis eines Scans oder Klassifizierens als Kennzahl-Kacheln mit Hinweisliste. Der Aufbau
//! (`scan_view`, `classify_view`) ist rein und ohne egui testbar; `show` zeichnet ihn nur.

use eframe::egui;

use super::format::{score_style, Tone};
use super::texts;
use super::theme;
use crate::ops::classify::{render_summary, ClassifyOutcome};
use crate::ops::scan::ScanReport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    pub label: &'static str,
    pub value: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResultView {
    pub title: String,
    pub tiles: Vec<Tile>,
    pub notes: Vec<String>,
    /// Ausführlicher Text, einklappbar (leer = keiner)
    pub details: String,
    /// Fehler des Scans; bei mehr als 0 führt ein Klick auf die Kachel zu den Details
    pub errors: u64,
}

fn tile(label: &'static str, value: impl Into<String>, tone: Tone) -> Tile {
    Tile {
        label,
        value: value.into(),
        tone,
    }
}

pub fn scan_view(report: &ScanReport) -> ResultView {
    let o = &report.outcome;
    let mut tiles = vec![
        tile("Dateien", texts::grouped(o.files), Tone::Muted),
        tile("Ordner", texts::grouped(o.dirs), Tone::Muted),
        tile("Größe", texts::bytes(o.bytes), Tone::Muted),
        tile(
            "Fehler",
            texts::grouped(o.errors),
            if o.errors > 0 { Tone::Warn } else { Tone::Ok },
        ),
    ];
    let mut notes = report.notes.hints.clone();
    if o.aborted {
        notes.push("Der Index bleibt konsistent, aber unvollständig.".into());
    } else {
        if let Some(score) = report.score {
            let style = score_style(score);
            tiles.push(tile(
                "Score",
                format!("{score} – {}", style.text),
                style.tone,
            ));
        }
        notes.extend(report.score_line.clone());
        if let Some(w) = &report.history_warning {
            notes.push(format!("Verlauf nicht aktualisiert: {w}"));
        }
    }
    ResultView {
        title: if o.aborted {
            "Scan abgebrochen".into()
        } else {
            "Scan fertig".into()
        },
        tiles,
        notes,
        details: String::new(),
        errors: o.errors,
    }
}

pub fn classify_view(outcome: &ClassifyOutcome) -> ResultView {
    match outcome {
        ClassifyOutcome::Cleared { entries, .. } => ResultView {
            title: "Text-Cache geleert".into(),
            tiles: vec![tile(
                "Einträge",
                texts::grouped(*entries as u64),
                Tone::Muted,
            )],
            ..ResultView::default()
        },
        ClassifyOutcome::Classified { root, run, summary } => {
            let stats = &run.stats;
            let skipped = stats.cloud_only + summary.too_large + summary.unsupported;
            let count = |n: usize| texts::grouped(n as u64);
            let mut notes = run.warnings.clone();
            if run.aborted {
                notes.push("Bis dahin Analysiertes bleibt im Cache.".into());
            }
            ResultView {
                title: if run.aborted {
                    "Klassifizieren abgebrochen".into()
                } else {
                    "Klassifiziert".into()
                },
                tiles: vec![
                    tile("Geprüft", count(stats.considered), Tone::Muted),
                    tile("Analysiert", count(stats.analysed), Tone::Muted),
                    tile("Aus dem Cache", count(stats.from_cache), Tone::Muted),
                    tile(
                        "Zum Prüfen",
                        count(summary.review),
                        if summary.review > 0 {
                            Tone::Warn
                        } else {
                            Tone::Ok
                        },
                    ),
                    tile("Übersprungen", count(skipped), Tone::Muted),
                ],
                notes,
                details: render_summary(&crate::paths::display(root), run, summary),
                errors: 0,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultAction {
    None,
    Close,
    ShowErrors,
}

fn tile_ui(ui: &mut egui::Ui, tile: &Tile, clickable: bool) -> egui::Response {
    let color = theme::tone_color(ui, tile.tone);
    let inner = egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(tile.label).small().weak());
            ui.label(egui::RichText::new(&tile.value).heading().color(color));
        });
    });
    if clickable {
        inner
            .response
            .interact(egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Fehler anzeigen")
    } else {
        inner.response
    }
}

/// Zeichnet die Kachelreihe, die Hinweise und die Details.
pub fn show(ui: &mut egui::Ui, view: &ResultView) -> ResultAction {
    let mut action = ResultAction::None;
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.strong(&view.title);
            if ui.button(texts::CLOSE).clicked() {
                action = ResultAction::Close;
            }
        });
        ui.horizontal_wrapped(|ui| {
            for tile in &view.tiles {
                let clickable = tile.label == "Fehler" && view.errors > 0;
                if tile_ui(ui, tile, clickable).clicked() && clickable {
                    action = ResultAction::ShowErrors;
                }
            }
        });
        for note in &view.notes {
            ui.label(note);
        }
        if !view.details.is_empty() {
            ui.collapsing("Details", |ui| {
                ui.label(egui::RichText::new(&view.details).monospace());
            });
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::classify::{ClassifyRun, RunStats, Summary};
    use crate::ops::Notes;
    use crate::scan::ScanOutcome;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    fn report(errors: u64, aborted: bool) -> ScanReport {
        let mut notes = Notes::default();
        notes.hint("Hinweis: Index ist alt");
        ScanReport {
            root: PathBuf::from(r"D:\Daten"),
            outcome: ScanOutcome {
                files: 1234,
                dirs: 7,
                bytes: 2048,
                errors,
                aborted,
                hash: Default::default(),
            },
            index_file: PathBuf::from("index.db"),
            score: Some(80),
            score_line: Some("Health-Score 80 (erster Lauf)".into()),
            history_warning: None,
            notes,
        }
    }

    fn tile<'a>(view: &'a ResultView, label: &str) -> &'a Tile {
        view.tiles
            .iter()
            .find(|t| t.label == label)
            .unwrap_or_else(|| panic!("Kachel {label} fehlt in {:?}", view.tiles))
    }

    #[test]
    fn scan_zeigt_zaehler_und_score_als_kacheln() {
        let view = scan_view(&report(1, false));
        assert_eq!(view.title, "Scan fertig");
        let labels: Vec<&str> = view.tiles.iter().map(|t| t.label).collect();
        assert_eq!(labels, ["Dateien", "Ordner", "Größe", "Fehler", "Score"]);
        assert_eq!(tile(&view, "Dateien").value, "1.234");
        assert_eq!(tile(&view, "Ordner").value, "7");
        assert_eq!(tile(&view, "Größe").value, "2.0 KiB");
        assert_eq!(tile(&view, "Fehler").value, "1");
        assert_eq!(tile(&view, "Fehler").tone, Tone::Warn);
        assert_eq!(tile(&view, "Score").value, "80 – gut");
        assert_eq!(tile(&view, "Score").tone, Tone::Ok);
        assert_eq!(view.errors, 1);
    }

    #[test]
    fn scan_ohne_fehler_hat_eine_ruhige_fehlerkachel() {
        let view = scan_view(&report(0, false));
        assert_eq!(tile(&view, "Fehler").value, "0");
        assert_eq!(tile(&view, "Fehler").tone, Tone::Ok);
        assert_eq!(view.errors, 0);
    }

    #[test]
    fn scan_hinweise_und_scoreverlauf_stehen_in_der_liste() {
        let mut r = report(0, false);
        r.history_warning = Some("Datenbank gesperrt".into());
        let view = scan_view(&r);
        assert!(view.notes.contains(&"Hinweis: Index ist alt".to_string()));
        assert!(view
            .notes
            .contains(&"Health-Score 80 (erster Lauf)".to_string()));
        assert!(view
            .notes
            .contains(&"Verlauf nicht aktualisiert: Datenbank gesperrt".to_string()));
    }

    #[test]
    fn abgebrochener_scan_hat_keinen_score_und_sagt_unvollstaendig() {
        let view = scan_view(&report(0, true));
        assert_eq!(view.title, "Scan abgebrochen");
        assert!(view.tiles.iter().all(|t| t.label != "Score"));
        assert!(
            view.notes.iter().any(|n| n.contains("unvollständig")),
            "{:?}",
            view.notes
        );
        assert!(view.notes.iter().all(|n| !n.contains("Health-Score")));
    }

    #[test]
    fn geleerter_cache_zeigt_die_zahl_der_eintraege() {
        let view = classify_view(&ClassifyOutcome::Cleared {
            root: PathBuf::from(r"D:\Daten"),
            entries: 1500,
        });
        assert_eq!(view.title, "Text-Cache geleert");
        assert_eq!(tile(&view, "Einträge").value, "1.500");
        assert!(view.details.is_empty());
    }

    fn classified(aborted: bool) -> ClassifyOutcome {
        ClassifyOutcome::Classified {
            root: PathBuf::from(r"D:\Daten"),
            run: Box::new(ClassifyRun {
                stats: RunStats {
                    considered: 10,
                    analysed: 6,
                    from_cache: 4,
                    cloud_only: 1,
                    ..RunStats::default()
                },
                records: HashMap::new(),
                warnings: vec!["LLM: nicht erreichbar".into()],
                aborted,
                duration: Duration::from_secs(3),
            }),
            summary: Summary {
                review: 3,
                too_large: 1,
                unsupported: 2,
                ..Summary::default()
            },
        }
    }

    #[test]
    fn klassifizieren_zeigt_zaehler_hinweise_und_details() {
        let view = classify_view(&classified(false));
        assert_eq!(view.title, "Klassifiziert");
        assert_eq!(tile(&view, "Geprüft").value, "10");
        assert_eq!(tile(&view, "Analysiert").value, "6");
        assert_eq!(tile(&view, "Aus dem Cache").value, "4");
        assert_eq!(tile(&view, "Zum Prüfen").value, "3");
        assert_eq!(tile(&view, "Zum Prüfen").tone, Tone::Warn);
        assert_eq!(tile(&view, "Übersprungen").value, "4");
        assert_eq!(view.notes, ["LLM: nicht erreichbar"]);
        assert!(
            view.details.starts_with("Klassifiziert:"),
            "{}",
            view.details
        );
    }

    #[test]
    fn abgebrochenes_klassifizieren_sagt_wo_das_ergebnis_bleibt() {
        let view = classify_view(&classified(true));
        assert_eq!(view.title, "Klassifizieren abgebrochen");
        assert!(
            view.notes.iter().any(|n| n.contains("Cache")),
            "{:?}",
            view.notes
        );
    }

    #[test]
    fn nichts_zum_pruefen_ist_ruhig() {
        let ClassifyOutcome::Classified { root, run, .. } = classified(false) else {
            unreachable!()
        };
        let view = classify_view(&ClassifyOutcome::Classified {
            root,
            run,
            summary: Summary::default(),
        });
        assert_eq!(tile(&view, "Zum Prüfen").tone, Tone::Ok);
    }
}
