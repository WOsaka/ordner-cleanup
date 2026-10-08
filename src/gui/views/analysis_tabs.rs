//! Die Tabs der Analyse, gezeichnet aus dem vorhandenen `report::Report`.

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

use crate::gui::shell::{Goto, Page, Shell};
use crate::gui::texts;
use crate::gui::widgets::table::{path_cell, path_menu, share_bar};
use crate::report::history::format_delta;
use crate::report::{FileItem, Report, TreeNode};

const ROW_H: f32 = 20.0;

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.heading(text);
}

fn plan_button(ui: &mut egui::Ui, shell: &mut Shell, label: &str, kind: &'static str) {
    if ui.button(label).clicked() {
        shell.goto = Some(Goto {
            page: Page::Cleanup,
            plan_kind: Some(kind),
            ..Goto::default()
        });
    }
}

/// Größenbaum: nur aufgeklappte Knoten werden gezeichnet, Kinder nach Größe sortiert.
pub fn size_tree(ui: &mut egui::Ui, report: &Report, shell: &mut Shell) {
    let total = report.size_tree.size.max(1);
    node(ui, &report.size_tree, total, shell, true);
}

fn node(ui: &mut egui::Ui, n: &TreeNode, parent: u64, shell: &mut Shell, root: bool) {
    let share = n.size as f32 / parent.max(1) as f32;
    let title = format!(
        "{}  ·  {}  ·  {} Dateien{}",
        if n.name.is_empty() { &n.path } else { &n.name },
        texts::bytes(n.size),
        texts::grouped(n.files),
        if n.summary { "  ·  nur Summe" } else { "" }
    );
    if n.children.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(ui.spacing().indent);
            let r = ui.label(title);
            path_menu(&r, &n.path, shell);
            share_bar(ui, share, 90.0);
        });
        return;
    }
    let id = ui.make_persistent_id(("tree", &n.path));
    let state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, root);
    state
        .show_header(ui, |ui| {
            let r = ui.label(title);
            path_menu(&r, &n.path, shell);
            share_bar(ui, share, 90.0);
        })
        .body(|ui| {
            let mut children: Vec<&TreeNode> = n.children.iter().collect();
            children.sort_by_key(|a| std::cmp::Reverse(a.size));
            for child in children {
                node(ui, child, n.size, shell, false);
            }
        });
}

fn file_table(ui: &mut egui::Ui, id: &str, items: &[FileItem], shell: &mut Shell) {
    TableBuilder::new(ui)
        .id_salt(id)
        .striped(true)
        .max_scroll_height(360.0)
        .column(Column::remainder().clip(true))
        .column(Column::auto())
        .column(Column::auto())
        .header(ROW_H, |mut h| {
            for t in ["Pfad", "Größe", "Alter"] {
                h.col(|ui| {
                    ui.strong(t);
                });
            }
        })
        .body(|body| {
            body.rows(ROW_H, items.len(), |mut row| {
                let it = &items[row.index()];
                row.col(|ui| path_cell(ui, &it.path, shell));
                row.col(|ui| {
                    ui.label(texts::bytes(it.size));
                });
                row.col(|ui| {
                    ui.label(format!(
                        "{} Tage",
                        texts::grouped(it.age_days.max(0) as u64)
                    ));
                });
            });
        });
}

pub fn types_and_age(ui: &mut egui::Ui, report: &Report, shell: &mut Shell) {
    heading(ui, "Nach Endung");
    egui::Grid::new("by-ext").striped(true).show(ui, |ui| {
        for t in report.file_types.by_extension.iter().take(30) {
            ui.label(&t.key);
            ui.label(texts::grouped(t.count));
            ui.label(texts::bytes(t.size));
            ui.end_row();
        }
    });
    heading(ui, "Nach Kategorie");
    egui::Grid::new("by-cat").striped(true).show(ui, |ui| {
        for t in &report.file_types.by_category {
            ui.label(&t.key);
            ui.label(texts::grouped(t.count));
            ui.label(texts::bytes(t.size));
            ui.end_row();
        }
    });
    heading(ui, "Nach Alter");
    egui::Grid::new("by-age").striped(true).show(ui, |ui| {
        for a in &report.age.classes {
            ui.label(&a.label);
            ui.label(texts::grouped(a.count));
            ui.label(texts::bytes(a.size));
            ui.end_row();
        }
    });
    heading(
        ui,
        &format!("Größte Dateien (Top {})", report.top_files.len()),
    );
    file_table(ui, "top-files", &report.top_files, shell);
    heading(
        ui,
        &format!(
            "Alte Dateien (älter als {} Tage, zusammen {})",
            report.age.old_after_days,
            texts::bytes(report.age.old_total_size)
        ),
    );
    file_table(ui, "old-files", &report.age.old_files, shell);
}

pub fn duplicates(ui: &mut egui::Ui, report: &Report, shell: &mut Shell) {
    let d = &report.duplicates;
    ui.horizontal(|ui| {
        ui.label(format!(
            "{} Gruppen, {} verschwendet",
            texts::grouped(d.group_count as u64),
            texts::bytes(d.total_wasted)
        ));
        plan_button(ui, shell, "Plan dafür erzeugen (Duplikate)", "dedupe");
    });
    for (i, g) in d.groups.iter().enumerate() {
        egui::CollapsingHeader::new(format!(
            "{} Kopien × {}  ·  {} verschwendet",
            g.instances,
            texts::bytes(g.size),
            texts::bytes(g.wasted)
        ))
        .id_salt(("dup", i))
        .show(ui, |ui| {
            for f in &g.files {
                path_cell(ui, &f.path, shell);
            }
        });
    }
    if !report.probable_duplicates.is_empty() {
        heading(ui, "Wahrscheinliche Duplikate (Inhalt nicht geprüft)");
        for (i, g) in report.probable_duplicates.iter().enumerate() {
            egui::CollapsingHeader::new(format!("{}  ·  {}", g.name, texts::bytes(g.size)))
                .id_salt(("prob", i))
                .show(ui, |ui| {
                    for f in &g.files {
                        path_cell(ui, &f.path, shell);
                    }
                });
        }
    }
    if !report.similar.is_empty() {
        heading(ui, "Ähnliche Dateien im selben Ordner");
        for (i, g) in report.similar.iter().enumerate() {
            egui::CollapsingHeader::new(&g.dir)
                .id_salt(("sim", i))
                .show(ui, |ui| {
                    for f in &g.files {
                        ui.label(format!("{}  ·  {}", f.name, texts::bytes(f.size)));
                    }
                });
        }
    }
}

pub fn problems(ui: &mut egui::Ui, report: &Report, shell: &mut Shell) {
    ui.horizontal(|ui| {
        plan_button(ui, shell, "Plan für Müll", "junk");
        plan_button(ui, shell, "Plan für leere Ordner", "empty-dirs");
    });
    heading(ui, &format!("Problemdateien ({})", report.problems.len()));
    for p in &report.problems {
        ui.horizontal(|ui| {
            path_cell(ui, &p.path, shell);
            ui.label(RichText::new(p.labels.join(", ")).weak());
        });
    }
    heading(
        ui,
        &format!("Strukturprobleme ({})", report.structure.len()),
    );
    for s in &report.structure {
        ui.horizontal(|ui| {
            path_cell(ui, &s.path, shell);
            ui.label(RichText::new(&s.label).weak());
        });
    }
    if !report.errors.is_empty() {
        heading(ui, &format!("Scan-Fehler ({})", report.errors.len()));
        for e in &report.errors {
            ui.label(format!("{}: {}", e.path, e.message));
        }
    }
}

pub fn content(ui: &mut egui::Ui, report: &Report, shell: &mut Shell) {
    let Some(c) = &report.content else {
        ui.label("Noch nicht klassifiziert. Auf der Übersicht „Klassifizieren“ wählen.");
        return;
    };
    if !c.classified {
        ui.label("Noch nicht klassifiziert. Auf der Übersicht „Klassifizieren“ wählen.");
        return;
    }
    ui.label(format!(
        "{} von {} Dateien klassifiziert; Quellen: Text {}, OCR {}, Metadaten {}, LLM {}",
        texts::grouped(c.files_classified),
        texts::grouped(c.files_total),
        c.sources.text,
        c.sources.ocr,
        c.sources.meta,
        c.sources.llm
    ));
    plan_button(ui, shell, "Plan nach Regeln erzeugen", "rules");
    heading(ui, "Kategorien");
    egui::Grid::new("categories").striped(true).show(ui, |ui| {
        for cat in &c.categories {
            ui.label(&cat.name);
            ui.label(texts::grouped(cat.count));
            ui.label(texts::bytes(cat.size));
            share_bar(ui, cat.share as f32, 120.0);
            ui.end_row();
        }
    });
    heading(ui, &format!("Zum Prüfen ({})", c.review_total));
    for r in &c.review {
        ui.horizontal(|ui| {
            path_cell(ui, &r.path, shell);
            let second = match (&r.category2, r.confidence2) {
                (Some(c2), Some(p2)) => format!(", {c2} {:.0} %", p2 * 100.0),
                _ => String::new(),
            };
            ui.label(
                RichText::new(format!(
                    "{} {:.0} %{second}",
                    r.category,
                    r.confidence * 100.0
                ))
                .weak(),
            );
        });
    }
}

pub fn health(ui: &mut egui::Ui, report: &Report) {
    let Some(h) = &report.history else {
        ui.label("Kein Verlauf vorhanden. Erst scannen.");
        return;
    };
    ui.heading(format!("Health-Score {}", h.score));
    match &h.comparison {
        Some(c) => {
            ui.label(format!(
                "{} seit {}{}",
                format_delta(c.delta),
                crate::report::history::short_date(&c.previous_at),
                if c.is_limited() {
                    " (eingeschränkt vergleichbar)"
                } else {
                    ""
                }
            ));
        }
        None => {
            ui.label("Erster Lauf");
        }
    }
    for n in &h.notes {
        ui.label(RichText::new(n).weak());
    }
    heading(ui, "Teilwerte");
    egui::Grid::new("parts").striped(true).show(ui, |ui| {
        for p in &h.parts {
            ui.label(&p.label);
            match p.value {
                Some(v) => share_bar(ui, (v / 100.0) as f32, 140.0),
                None => {
                    ui.label("–");
                }
            }
            ui.end_row();
        }
    });
    if !h.deductions.is_empty() {
        heading(ui, "Größte Abzüge");
        for d in &h.deductions {
            ui.label(format!("−{:.1}  {}", d.points, d.text));
        }
    }
    heading(ui, "Kennzahlen");
    egui::Grid::new("metrics").striped(true).show(ui, |ui| {
        for m in &h.metrics {
            ui.label(&m.label);
            ui.label(texts::grouped(m.now));
            ui.label(m.previous.map_or("–".into(), texts::grouped));
            ui.end_row();
        }
    });
    heading(ui, "Ordner");
    egui::Grid::new("folders").striped(true).show(ui, |ui| {
        for f in &h.folders {
            ui.label(&f.folder);
            ui.label(f.score.to_string());
            ui.label(f.top_deduction.clone().unwrap_or_default());
            ui.end_row();
        }
    });
    if h.series.len() > 1 {
        heading(ui, "Verlauf");
        let points: egui_plot::PlotPoints = h
            .series
            .iter()
            .enumerate()
            .map(|(i, p)| [i as f64, f64::from(p.score)])
            .collect();
        egui_plot::Plot::new("score-series")
            .height(180.0)
            .include_y(0.0)
            .include_y(100.0)
            .allow_scroll(false)
            .show(ui, |plot| {
                plot.line(egui_plot::Line::new("Score", points));
            });
    }
}

pub fn template(ui: &mut egui::Ui, report: &Report, shell: &mut Shell) {
    let Some(t) = &report.template else {
        ui.label("Keine Vorlage gewählt (unter „Erweitert“ beim Scannen).");
        return;
    };
    ui.label(format!(
        "{} ({}): {:.0} % der {} geprüften Einträge entsprechen der Vorlage",
        t.name,
        t.source,
        t.share * 100.0,
        texts::grouped(t.checked)
    ));
    for k in &t.counts {
        ui.label(format!("{}: {}", k.label, texts::grouped(k.count)));
    }
    heading(ui, "Abweichungen");
    for item in &t.items {
        ui.horizontal(|ui| {
            path_cell(ui, &item.path, shell);
            ui.label(RichText::new(&item.label).weak());
        });
    }
}
