//! Generischer Formular-Renderer: zeigt `FieldSpec`s für ein `toml_edit`-Dokument. Der Wert
//! kommt aus dem Dokument, sonst wird der Standard angezeigt; „Zurücksetzen“ entfernt den
//! Schlüssel. Eingaben werden sofort geprüft und nur bei Erfolg ins Dokument geschrieben.

use std::collections::HashMap;

use eframe::egui;
use toml_edit::DocumentMut;

use crate::config::Config;
use crate::settings::fields::{self, FieldKind, FieldSpec, Loc, Value};

/// Eingabepuffer und Fehler je Feld; wird geleert, wenn sich das Dokument von außen ändert.
#[derive(Default)]
pub struct FormState {
    buffers: HashMap<String, String>,
    errors: HashMap<String, String>,
}

impl FormState {
    pub fn reset(&mut self) {
        self.buffers.clear();
        self.errors.clear();
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn first_error(&self) -> Option<&str> {
        self.errors.values().next().map(String::as_str)
    }
}

/// Mindestbreite der Bezeichnungsspalte, damit Label und Schlüssel nicht umbrechen.
const LABEL_COL_WIDTH: f32 = 220.0;

fn id_of(loc: Loc, key: &str) -> String {
    format!("{}.{key}", loc.id())
}

/// Zeichnet die Felder; `true`, wenn das Dokument geändert wurde.
pub fn show(
    ui: &mut egui::Ui,
    doc: &mut DocumentMut,
    loc: Loc,
    specs: &[FieldSpec],
    defaults: &Config,
    state: &mut FormState,
) -> bool {
    let mut changed = false;
    egui::Grid::new(("form", loc.id(), specs.len()))
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            for spec in specs {
                changed |= field(ui, doc, loc, spec, defaults, state);
                ui.end_row();
            }
        });
    changed
}

fn field(
    ui: &mut egui::Ui,
    doc: &mut DocumentMut,
    loc: Loc,
    spec: &FieldSpec,
    defaults: &Config,
    state: &mut FormState,
) -> bool {
    let id = id_of(loc, spec.key);
    let current = fields::get_at(doc, loc, spec.key);
    ui.vertical(|ui| {
        ui.set_min_width(LABEL_COL_WIDTH);
        ui.label(egui::RichText::new(spec.label).strong())
            .on_hover_text(spec.help);
        ui.label(egui::RichText::new(spec.key).monospace().weak().small());
    });
    let mut changed = false;
    ui.vertical(|ui| {
        match spec.kind {
            FieldKind::Bool => changed |= bool_field(ui, doc, loc, spec, defaults, &current),
            FieldKind::Multi(options) => {
                changed |= multi_field(ui, doc, loc, spec, options, &current);
            }
            _ => changed |= text_field(ui, doc, loc, spec, state, &id, &current),
        }
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} · Standard: {}",
                    spec.help,
                    (spec.default)(defaults)
                ))
                .weak()
                .small(),
            );
            if current.is_some() && ui.small_button("Zurücksetzen").clicked() {
                fields::unset_at(doc, loc, spec.key);
                state.buffers.remove(&id);
                state.errors.remove(&id);
                changed = true;
            }
        });
        if let Some(error) = state.errors.get(&id) {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    });
    changed
}

fn bool_field(
    ui: &mut egui::Ui,
    doc: &mut DocumentMut,
    loc: Loc,
    spec: &FieldSpec,
    defaults: &Config,
    current: &Option<Value>,
) -> bool {
    let effective = match current {
        Some(Value::Bool(b)) => *b,
        _ => (spec.default)(defaults) == "true",
    };
    let mut on = effective;
    if ui.checkbox(&mut on, "").changed() {
        fields::set_at(doc, loc, spec.key, &Value::Bool(on));
        return true;
    }
    false
}

fn multi_field(
    ui: &mut egui::Ui,
    doc: &mut DocumentMut,
    loc: Loc,
    spec: &FieldSpec,
    options: &'static [&'static str],
    current: &Option<Value>,
) -> bool {
    let chosen: Vec<String> = match current {
        Some(Value::List(items)) => items.clone(),
        _ => Vec::new(),
    };
    let mut next = chosen.clone();
    ui.horizontal_wrapped(|ui| {
        for option in options {
            let mut on = chosen.iter().any(|c| c == option);
            if ui.checkbox(&mut on, *option).changed() {
                next.retain(|c| c != option);
                if on {
                    next.push((*option).to_string());
                }
            }
        }
    });
    if next != chosen {
        // Reihenfolge der Optionen beibehalten
        next.sort_by_key(|n| options.iter().position(|o| o == n));
        if next.is_empty() && !matches!(spec.key, "junk_categories" | "tasks") {
            fields::unset_at(doc, loc, spec.key);
        } else {
            fields::set_at(doc, loc, spec.key, &Value::List(next));
        }
        return true;
    }
    false
}

fn text_field(
    ui: &mut egui::Ui,
    doc: &mut DocumentMut,
    loc: Loc,
    spec: &FieldSpec,
    state: &mut FormState,
    id: &str,
    current: &Option<Value>,
) -> bool {
    let buffer = state
        .buffers
        .entry(id.to_string())
        .or_insert_with(|| current.as_ref().map(fields::to_input).unwrap_or_default());
    let multiline = matches!(spec.kind, FieldKind::TextList | FieldKind::PathList);
    let response = if multiline {
        ui.add(
            egui::TextEdit::multiline(buffer)
                .desired_rows(2)
                .desired_width(420.0)
                .font(egui::TextStyle::Monospace),
        )
    } else if let FieldKind::Choice(options) = spec.kind {
        let mut chosen = buffer.clone();
        egui::ComboBox::from_id_salt(id)
            .selected_text(if chosen.is_empty() {
                "Standard"
            } else {
                &chosen
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut chosen, String::new(), "Standard");
                for option in options {
                    ui.selectable_value(&mut chosen, (*option).to_string(), *option);
                }
            });
        let changed = chosen != *buffer;
        *buffer = chosen;
        let text = buffer.clone();
        if changed {
            return apply(doc, loc, spec, state, id, &text);
        }
        return false;
    } else {
        ui.add(egui::TextEdit::singleline(buffer).desired_width(420.0))
    };
    if response.changed() {
        let text = buffer.clone();
        return apply(doc, loc, spec, state, id, &text);
    }
    false
}

fn apply(
    doc: &mut DocumentMut,
    loc: Loc,
    spec: &FieldSpec,
    state: &mut FormState,
    id: &str,
    text: &str,
) -> bool {
    match fields::apply_text_at(doc, loc, spec, text) {
        Ok(()) => {
            state.errors.remove(id);
            true
        }
        Err(e) => {
            state.errors.insert(id.to_string(), e);
            false
        }
    }
}
