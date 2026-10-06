//! Liste von Tabellen im Dokument (`[[rules]]`, `[[junk_rules]]`) mit Knöpfen für Neu,
//! Duplizieren, Löschen und Verschieben.

use eframe::egui;
use toml_edit::DocumentMut;

use crate::settings::fields::Value;
use crate::settings::tables;

#[derive(Default)]
pub struct ListState {
    pub selected: usize,
}

/// Zeichnet Liste und Knöpfe; `true`, wenn das Dokument geändert wurde.
pub fn ui(
    ui: &mut egui::Ui,
    doc: &mut DocumentMut,
    key: &str,
    state: &mut ListState,
    new_name: &str,
    initial: &[(&str, Value)],
) -> bool {
    let mut changed = false;
    let names = tables::names(doc, key);
    if state.selected >= names.len() {
        state.selected = names.len().saturating_sub(1);
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button("Neu").clicked() {
            if let Some(i) = tables::add(doc, key, new_name, initial) {
                state.selected = i;
                changed = true;
            }
        }
        let has = !names.is_empty();
        if ui
            .add_enabled(has, egui::Button::new("Duplizieren"))
            .clicked()
        {
            if let Some(i) = tables::duplicate(doc, key, state.selected) {
                state.selected = i;
                changed = true;
            }
        }
        if ui.add_enabled(has, egui::Button::new("Löschen")).clicked()
            && tables::remove(doc, key, state.selected)
        {
            changed = true;
        }
        if ui
            .add_enabled(has && state.selected > 0, egui::Button::new("▲"))
            .clicked()
        {
            if let Some(i) = tables::move_by(doc, key, state.selected, -1) {
                state.selected = i;
                changed = true;
            }
        }
        if ui
            .add_enabled(
                has && state.selected + 1 < names.len(),
                egui::Button::new("▼"),
            )
            .clicked()
        {
            if let Some(i) = tables::move_by(doc, key, state.selected, 1) {
                state.selected = i;
                changed = true;
            }
        }
    });
    let names = tables::names(doc, key);
    egui::ScrollArea::vertical()
        .id_salt(("list", key))
        .max_height(220.0)
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                if ui
                    .selectable_label(state.selected == i, format!("{}. {name}", i + 1))
                    .clicked()
                {
                    state.selected = i;
                }
            }
        });
    changed
}
