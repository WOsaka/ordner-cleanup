//! Allgemeine Einstellungen: Felder der obersten Ebene, Benachrichtigung, Ablage der GUI-Pläne
//! und eigene Müll-Kategorien.

use eframe::egui;

use super::doc_editor::DocEditor;
use super::list::{self, ListState};
use crate::config::Config;
use crate::gui::shell::Shell;
use crate::gui::widgets::form;
use crate::settings::fields::{
    FieldKind, FieldSpec, Loc, Value, CONFIG_FIELDS, GUI_FIELDS, NOTIFY_FIELDS,
};
use crate::settings::validate::config_text;

const JUNK_KEY: &str = "junk_rules";

static JUNK_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        key: "name",
        label: "Name der Kategorie",
        help: "Darf keinen eingebauten Namen tragen",
        kind: FieldKind::Text,
        default: |_| "–".into(),
    },
    FieldSpec {
        key: "patterns",
        label: "Muster",
        help: "Globs auf den Dateinamen, ein Muster je Zeile",
        kind: FieldKind::TextList,
        default: |_| "–".into(),
    },
    FieldSpec {
        key: "min_age",
        label: "Mindestalter",
        help: "z. B. 30d; leer = keines",
        kind: FieldKind::Duration,
        default: |_| "keines".into(),
    },
    FieldSpec {
        key: "dirs",
        label: "Nur in diesen Ordnern",
        help: "Ein Ordner je Zeile; leer = überall",
        kind: FieldKind::PathList,
        default: |_| "überall".into(),
    },
];

#[derive(Default)]
pub struct GeneralTab {
    junk: ListState,
}

impl GeneralTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, ed: &mut DocEditor, shell: &mut Shell) {
        ed.toolbar(ui, shell, &config_text);
        ui.separator();
        if ed.raw_mode {
            raw(ui, ed);
            return;
        }
        let defaults = Config::default();
        let mut form_state = std::mem::take(&mut ed.form);
        if let Some(doc) = ed.doc() {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Allgemein");
                form::show(
                    ui,
                    doc,
                    Loc::Path(&[]),
                    CONFIG_FIELDS,
                    &defaults,
                    &mut form_state,
                );
                ui.add_space(crate::gui::theme::SPACE_L);
                ui.heading("Benachrichtigung");
                form::show(
                    ui,
                    doc,
                    Loc::Path(&["notify"]),
                    NOTIFY_FIELDS,
                    &defaults,
                    &mut form_state,
                );
                ui.add_space(crate::gui::theme::SPACE_L);
                ui.heading("GUI");
                form::show(
                    ui,
                    doc,
                    Loc::Path(&["gui"]),
                    GUI_FIELDS,
                    &defaults,
                    &mut form_state,
                );
                ui.add_space(crate::gui::theme::SPACE_L);
                ui.heading("Eigene Müll-Kategorien");
                list::ui(
                    ui,
                    doc,
                    JUNK_KEY,
                    &mut self.junk,
                    "meine-kategorie",
                    &[("patterns", Value::List(vec!["*.bak".into()]))],
                );
                if crate::settings::tables::len(doc, JUNK_KEY) > 0 {
                    form::show(
                        ui,
                        doc,
                        Loc::Entry {
                            list: JUNK_KEY,
                            index: self.junk.selected,
                        },
                        JUNK_FIELDS,
                        &defaults,
                        &mut form_state,
                    );
                }
            });
        }
        ed.form = form_state;
    }
}

pub fn raw(ui: &mut egui::Ui, ed: &mut DocEditor) {
    ed.raw.ui(ui, &config_text, 28);
}
