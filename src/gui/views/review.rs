//! Darstellung des Review-Modells: Filterleiste, Auswahl, Tabelle, Detailbereich.

use eframe::egui;
use egui_extras::{Column as TCol, TableBuilder};

use crate::change::plan::ActionType;
use crate::gui::format;
use crate::gui::help::Topic;
use crate::gui::keys::KeyAction;
use crate::gui::review::{step_focus, Column, Filter, ReviewModel, Show, Sort};
use crate::gui::shell::{Dialog, Shell};
use crate::gui::texts;
use crate::gui::widgets::help_button::help_button;
use crate::gui::widgets::table::path_menu;

pub const ID_DEPENDENTS: &str = "cleanup.dependents";
const ROW_H: f32 = 24.0;
const SEARCH_ID: &str = "review-search";

/// Abstand zwischen Trennlinie und Zellinhalt.
fn cell_padding(ui: &mut egui::Ui) {
    ui.add_space(6.0);
}

/// Dünne Trennlinie am rechten Zellrand, in der Theme-Farbe.
fn cell_rule(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .vline(rect.right() - 0.5, rect.y_range(), stroke);
}

pub struct ReviewState {
    pub model: ReviewModel,
    pub plan_path: Option<std::path::PathBuf>,
    pub headline: String,
    pub notes: Vec<String>,
    /// Was mit den Dateien geschieht (Quarantäne, Verschieben), einmal beim Öffnen ermittelt
    safety: String,
    text: String,
    folder: String,
    min_mb: String,
    max_mb: String,
    action: Option<ActionType>,
    rule: Option<String>,
    show_skipped: bool,
    selected_row: Option<usize>,
    /// Zeile, die beim nächsten Zeichnen in den sichtbaren Bereich soll
    scroll_to: Option<usize>,
    /// Strg+Enter wurde gedrückt; `ui` meldet es wie den Knopf „Anwenden“
    apply_requested: bool,
    /// Nur ansehen (Regel-Vorschau): keine Auswahl, kein Anwenden
    pub read_only: bool,
    /// Abhängige Einträge, nach denen gerade gefragt wird
    dependents: Vec<u32>,
}

/// Sicherheitshinweis zum Plan; die Tage der Quarantäne kommen aus der Config.
fn safety_for(plan: &crate::change::plan::Plan) -> String {
    let has = |t: ActionType| plan.actions.iter().any(|a| a.action == t);
    let days = crate::ops::load_config()
        .unwrap_or_default()
        .quarantine_days;
    format::safety_note(
        has(ActionType::Quarantine),
        has(ActionType::Move),
        has(ActionType::RemoveDir),
        days,
    )
}

fn action_label(a: Option<ActionType>) -> &'static str {
    match a {
        None => "Alle Aktionen",
        Some(ActionType::Quarantine) => "In Quarantäne",
        Some(ActionType::Move) => "Verschieben",
        Some(ActionType::RemoveDir) => "Ordner entfernen",
    }
}

fn mb(text: &str) -> Option<u64> {
    text.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| *v >= 0.0)
        .map(|v| (v * 1_048_576.0) as u64)
}

impl ReviewState {
    pub fn new(
        model: ReviewModel,
        plan_path: Option<std::path::PathBuf>,
        headline: String,
        notes: Vec<String>,
    ) -> Self {
        let safety = safety_for(model.plan());
        Self {
            model,
            plan_path,
            headline,
            notes,
            safety,
            text: String::new(),
            folder: String::new(),
            min_mb: String::new(),
            max_mb: String::new(),
            action: None,
            rule: None,
            show_skipped: false,
            selected_row: None,
            scroll_to: None,
            apply_requested: false,
            read_only: false,
            dependents: Vec::new(),
        }
    }

    fn current_filter(&self) -> Filter {
        Filter {
            show: if self.show_skipped {
                Show::Skipped
            } else {
                Show::Actions
            },
            action: self.action,
            rule: self.rule.clone(),
            folder: Some(self.folder.trim().to_string()).filter(|f| !f.is_empty()),
            min_size: mb(&self.min_mb),
            max_size: mb(&self.max_mb),
            text: self.text.clone(),
        }
    }

    /// Antwort auf „abhängige Einträge ebenfalls abwählen?“.
    pub fn on_dependents_answer(&mut self, ok: bool) {
        if ok {
            let ids = std::mem::take(&mut self.dependents);
            self.model.deselect_ids(&ids);
        } else {
            self.dependents.clear();
        }
    }

    /// Zeichnet die Review-Ansicht; `true`, wenn „Anwenden“ gedrückt wurde.
    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) -> bool {
        let mut apply = false;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&self.headline).strong());
            if help_button(ui, Topic::Review) {
                shell.open_help(Topic::Review);
            }
        });
        for n in &self.notes {
            ui.label(egui::RichText::new(format!("Hinweis: {n}")).weak());
        }
        let s = self.model.summary();
        ui.horizontal(|ui| {
            ui.label(format!(
                "Ausgewählt: {} von {} Einträgen, {} von {}",
                texts::grouped(s.selected as u64),
                texts::grouped(s.total as u64),
                texts::bytes(s.selected_bytes),
                texts::bytes(s.total_bytes)
            ));
            if !self.read_only {
                self.select_menu(ui);
                let label =
                    egui::RichText::new(format::apply_label(s.selected, s.selected_bytes)).strong();
                if ui
                    .add_enabled(s.selected > 0, egui::Button::new(label))
                    .on_hover_text("Strg+Enter")
                    .clicked()
                {
                    apply = true;
                }
            }
        });
        if !self.read_only && !self.safety.is_empty() {
            ui.label(egui::RichText::new(&self.safety).weak());
        }
        self.filter_bar(ui);
        self.table(ui, shell);
        self.detail(ui);
        apply | std::mem::take(&mut self.apply_requested)
    }

    fn filter_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Suche:");
            ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .id(egui::Id::new(SEARCH_ID))
                    .desired_width(150.0),
            );
            ui.label("Unterhalb von:");
            ui.add(egui::TextEdit::singleline(&mut self.folder).desired_width(180.0));
            ui.label("Größe (MB) von/bis:");
            ui.add(egui::TextEdit::singleline(&mut self.min_mb).desired_width(40.0));
            ui.add(egui::TextEdit::singleline(&mut self.max_mb).desired_width(40.0));
            egui::ComboBox::from_id_salt("flt-action")
                .selected_text(action_label(self.action))
                .show_ui(ui, |ui| {
                    for a in [
                        None,
                        Some(ActionType::Quarantine),
                        Some(ActionType::Move),
                        Some(ActionType::RemoveDir),
                    ] {
                        ui.selectable_value(&mut self.action, a, action_label(a));
                    }
                });
            let rules = self.model.rules();
            if !rules.is_empty() {
                egui::ComboBox::from_id_salt("flt-rule")
                    .selected_text(self.rule.clone().unwrap_or_else(|| "Alle Regeln".into()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.rule, None, "Alle Regeln");
                        for r in rules {
                            ui.selectable_value(&mut self.rule, Some(r.clone()), r);
                        }
                    });
            }
            ui.checkbox(
                &mut self.show_skipped,
                format!("Übersprungene zeigen ({})", self.model.skipped_count()),
            );
        });
        let wanted = self.current_filter();
        if &wanted != self.model.filter() {
            self.model.set_filter(wanted);
            self.selected_row = None;
        }
    }

    fn select_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Auswahl ▾", |ui| {
            if ui.button("Alle wählen").clicked() {
                self.model.select_all(true);
                ui.close();
            }
            if ui.button("Keine wählen").clicked() {
                self.model.select_all(false);
                ui.close();
            }
            ui.separator();
            if ui.button("Gefilterte wählen").clicked() {
                self.model.select_filtered(true);
                ui.close();
            }
            if ui.button("Gefilterte abwählen").clicked() {
                self.model.select_filtered(false);
                ui.close();
            }
            let folder = self.folder.trim().to_string();
            if !folder.is_empty() {
                ui.separator();
                if ui.button("Ordner wählen").clicked() {
                    self.model.select_folder(&folder, true);
                    ui.close();
                }
                if ui.button("Ordner abwählen").clicked() {
                    self.model.select_folder(&folder, false);
                    ui.close();
                }
            }
        });
    }

    fn table(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let skipped = self.show_skipped || self.read_only;
        let mut toggled: Option<usize> = None;
        let mut clicked: Option<usize> = None;
        let rows = self.model.len();
        let avail = (ui.available_height() - 140.0).max(160.0);
        let has_target = self.model.plan().actions.iter().any(|a| a.target.is_some());
        let mut header_cols: Vec<(&str, Column)> =
            vec![("Aktion", Column::Action), ("Quelle", Column::Source)];
        if has_target {
            header_cols.push(("Ziel", Column::Target));
        }
        header_cols.extend([
            ("Größe", Column::Size),
            ("Grund / Regel", Column::Reason),
            ("Alter", Column::Age),
        ]);
        let mut header_click: Option<Column> = None;
        let model = &self.model;
        let selected_row = self.selected_row;
        let mut builder = TableBuilder::new(ui)
            .id_salt("review-table")
            .striped(true)
            .sense(egui::Sense::click())
            .max_scroll_height(avail)
            .column(TCol::exact(26.0))
            .column(
                TCol::initial(110.0)
                    .at_least(60.0)
                    .resizable(true)
                    .clip(true),
            )
            .column(TCol::remainder().at_least(160.0).resizable(true).clip(true));
        if has_target {
            builder = builder.column(
                TCol::initial(240.0)
                    .at_least(80.0)
                    .resizable(true)
                    .clip(true),
            );
        }
        builder = builder
            .column(
                TCol::initial(80.0)
                    .at_least(60.0)
                    .resizable(true)
                    .clip(true),
            )
            .column(
                TCol::initial(220.0)
                    .at_least(100.0)
                    .resizable(true)
                    .clip(true),
            )
            .column(TCol::initial(64.0).at_least(50.0).clip(true));
        if let Some(row) = self.scroll_to.take() {
            builder = builder.scroll_to_row(row, None);
        }
        builder
            .header(ROW_H + 4.0, |mut h| {
                h.col(|_| {});
                for (title, column) in header_cols.drain(..) {
                    h.col(|ui| {
                        let mark = match model.sort() {
                            Some(s) if s.column == column => {
                                if s.ascending {
                                    " ▲"
                                } else {
                                    " ▼"
                                }
                            }
                            _ => "",
                        };
                        if column != Column::Age {
                            cell_rule(ui);
                        }
                        cell_padding(ui);
                        let text = egui::RichText::new(format!("{title}{mark}")).strong();
                        if ui.add(egui::Button::new(text).frame(false)).clicked() {
                            header_click = Some(column);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_H, rows, |mut row| {
                    let i = row.index();
                    let r = model.row(i);
                    row.set_selected(selected_row == Some(i));
                    row.col(|ui| {
                        if !skipped {
                            let mut on = model.is_selected(i);
                            if ui.checkbox(&mut on, "").changed() {
                                toggled = Some(i);
                            }
                        }
                    });
                    row.col(|ui| {
                        cell_rule(ui);
                        cell_padding(ui);
                        ui.add(egui::Label::new(action_label(r.action)).truncate());
                    });
                    row.col(|ui| {
                        cell_rule(ui);
                        cell_padding(ui);
                        ui.add(
                            egui::Label::new(egui::RichText::new(&r.path).monospace()).truncate(),
                        )
                        .on_hover_text(&r.path);
                    });
                    if has_target {
                        row.col(|ui| {
                            cell_rule(ui);
                            cell_padding(ui);
                            let t = r.target.as_deref().unwrap_or("");
                            let label = ui.add(
                                egui::Label::new(egui::RichText::new(t).monospace()).truncate(),
                            );
                            if !t.is_empty() {
                                label.on_hover_text(t);
                            }
                        });
                    }
                    row.col(|ui| {
                        cell_rule(ui);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(8.0);
                            ui.label(texts::bytes(r.size));
                        });
                    });
                    row.col(|ui| {
                        cell_rule(ui);
                        cell_padding(ui);
                        let text = match &r.rule {
                            Some(rule) => format!("{rule}: {}", r.reason),
                            None => r.reason.clone(),
                        };
                        ui.add(egui::Label::new(&text).truncate())
                            .on_hover_text(&text);
                    });
                    row.col(|ui| {
                        cell_padding(ui);
                        ui.label(age_text(r.mtime_ticks));
                    });
                    let response = row.response();
                    path_menu(&response, &r.path, shell);
                    if response.clicked() {
                        clicked = Some(i);
                    }
                });
            });
        if let Some(column) = header_click {
            let next = match self.model.sort() {
                Some(s) if s.column == column && s.ascending => Some(Sort {
                    column,
                    ascending: false,
                }),
                Some(s) if s.column == column => None,
                _ => Some(Sort {
                    column,
                    ascending: true,
                }),
            };
            self.model.set_sort(next);
        }
        if let Some(i) = clicked {
            self.selected_row = Some(i);
        }
        if let Some(i) = toggled {
            self.model.toggle(i);
            self.after_toggle(i, shell);
        }
    }

    /// Fragt nach abhängigen Einträgen, wenn Zeile `i` gerade abgewählt wurde.
    fn after_toggle(&mut self, i: usize, shell: &mut Shell) {
        if !self.model.is_selected(i) {
            let deps = self.model.dependents_to_deselect(i);
            if !deps.is_empty() {
                self.ask_dependents(shell, deps);
            }
        }
    }

    /// Eine Taste im Review. Schreibende Aktionen laufen weiter über den Bestätigungsdialog.
    pub fn handle_key(&mut self, action: KeyAction, ctx: &egui::Context, shell: &mut Shell) {
        let len = self.model.len();
        match action {
            KeyAction::Next | KeyAction::Prev => {
                let delta = if action == KeyAction::Next { 1 } else { -1 };
                self.selected_row = step_focus(self.selected_row, delta, len);
                self.scroll_to = self.selected_row;
            }
            _ if self.read_only => {}
            KeyAction::Toggle => {
                if let Some(i) = self.selected_row.filter(|i| *i < len) {
                    let next = self.model.toggle_and_advance(i);
                    self.after_toggle(i, shell);
                    self.selected_row = Some(next);
                    self.scroll_to = Some(next);
                }
            }
            KeyAction::SelectFiltered => self.model.select_filtered(true),
            KeyAction::DeselectFiltered => self.model.select_filtered(false),
            KeyAction::FocusSearch => {
                ctx.memory_mut(|m| m.request_focus(egui::Id::new(SEARCH_ID)));
            }
            KeyAction::Apply => {
                self.apply_requested = self.model.summary().selected > 0;
            }
            _ => {}
        }
    }

    fn ask_dependents(&mut self, shell: &mut Shell, deps: Vec<u32>) {
        shell.confirm(Dialog::Confirm {
            id: ID_DEPENDENTS,
            title: "Abhängige Einträge".into(),
            text: format!(
                "{} übergeordnete Einträge hängen von diesem ab. Ebenfalls abwählen?\n\nBei \
                 „Abbrechen“ bleiben sie gewählt: Ein gewähltes Verschieben des ganzen Ordners \
                 nimmt auch diesen Eintrag mit, ein gewähltes Entfernen eines Ordners wird als \
                 „nicht leer“ übersprungen.",
                deps.len()
            ),
            ok_label: "Ebenfalls abwählen".into(),
            must_check: None,
            checked: false,
            danger: false,
        });
        self.dependents = deps;
    }

    fn detail(&self, ui: &mut egui::Ui) {
        let Some(i) = self.selected_row.filter(|i| *i < self.model.len()) else {
            return;
        };
        let r = self.model.row(i);
        ui.separator();
        egui::Grid::new("review-detail")
            .num_columns(2)
            .show(ui, |ui| {
                let mut line = |k: &str, v: &str| {
                    ui.label(egui::RichText::new(k).weak());
                    ui.label(egui::RichText::new(v).monospace());
                    ui.end_row();
                };
                line("Quelle", &r.path);
                if let Some(t) = &r.target {
                    line("Ziel", t);
                }
                line("Größe", &texts::bytes(r.size));
                line("Grund", &r.reason);
                if let Some(rule) = &r.rule {
                    line("Regel", rule);
                }
                if let Some(a) =
                    r.id.and_then(|id| self.model.plan().actions.iter().find(|a| a.id == id))
                {
                    if let Some(keep) = &a.keep {
                        line("Behaltene Kopie", keep);
                    }
                    line("Geändert", &a.mtime);
                    if let Some(files) = a.files {
                        line("Dateien im Ordner", &files.to_string());
                    }
                }
            });
    }
}

fn age_text(mtime_ticks: i64) -> String {
    if mtime_ticks <= 0 {
        return String::new();
    }
    let secs = mtime_ticks / crate::scan::source::TICKS_PER_SEC;
    let days = (chrono::Utc::now().timestamp() - secs) / 86_400;
    match days {
        d if d < 0 => "neu".into(),
        d if d < 60 => format!("{d} T"),
        d if d < 730 => format!("{} Mon", d / 30),
        d => format!("{} J", d / 365),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn megabyte_eingaben_werden_in_bytes_umgerechnet() {
        assert_eq!(mb("1"), Some(1_048_576));
        assert_eq!(mb(" 0,5 "), Some(524_288));
        assert_eq!(mb(""), None);
        assert_eq!(mb("abc"), None);
        assert_eq!(mb("-1"), None);
    }

    #[test]
    fn alterstext_waehlt_passende_einheit() {
        let now = chrono::Utc::now().timestamp();
        let ticks = |days: i64| (now - days * 86_400) * crate::scan::source::TICKS_PER_SEC;
        assert_eq!(age_text(0), "");
        assert_eq!(age_text(ticks(3)), "3 T");
        assert_eq!(age_text(ticks(120)), "4 Mon");
        assert_eq!(age_text(ticks(800)), "2 J");
    }
}
