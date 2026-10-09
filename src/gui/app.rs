//! Hauptfenster: Seitenleiste, aktive Ansicht, Dialoge, Statuszeile und Schließen-Logik.

use eframe::egui;
use serde::{Deserialize, Serialize};

use super::header::{self, Header};
use super::help::Topic;
use super::keys::{self, KeyAction, KeyContext};
use super::shell::{Choice, Dialog, Page, Route, Shell};
use super::texts;
use super::theme::{self, ThemeChoice};
use super::views::{
    analysis::AnalysisView, cleanup::CleanupView, help_window::HelpState, history::HistoryView,
    overview::OverviewView, quarantine::QuarantineView, settings::SettingsView,
};
use super::widgets::dialogs;

const STATE_KEY: &str = "ordner-cleanup-gui";
const ID_CLOSE: &str = "close";

/// Was die GUI sich zwischen den Starts merkt (nur Komfort).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Persisted {
    page: Page,
    target: Option<Choice>,
    #[serde(default)]
    theme: ThemeChoice,
    #[serde(default)]
    recent: Vec<Choice>,
}

pub struct GuiApp {
    shell: Shell,
    page: Page,
    header: Header,
    /// Das Ziel, das zuletzt in `recent_targets` eingetragen wurde
    remembered: Option<Choice>,
    overview: OverviewView,
    analysis: AnalysisView,
    cleanup: CleanupView,
    history: HistoryView,
    quarantine: QuarantineView,
    settings: SettingsView,
    help: HelpState,
    /// Symbolschrift vorhanden (einmal beim Start geprüft)
    icons: bool,
    /// Schließen wurde bestätigt; wir warten auf das Ende der Tasks
    closing: bool,
    theme: ThemeChoice,
}

impl GuiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_fonts(super::fonts::definitions());
        let saved: Persisted = cc
            .storage
            .and_then(|s| eframe::get_value(s, STATE_KEY))
            .unwrap_or_default();
        theme::apply(&cc.egui_ctx, saved.theme);
        let mut shell = Shell::new(cc.egui_ctx.clone());
        shell.target = saved.target;
        shell.recent_targets = saved.recent;
        let mut app = Self {
            shell,
            page: saved.page,
            header: Header::default(),
            remembered: None,
            overview: OverviewView::default(),
            analysis: AnalysisView::default(),
            cleanup: CleanupView::default(),
            history: HistoryView::default(),
            quarantine: QuarantineView::default(),
            settings: SettingsView::default(),
            help: HelpState::default(),
            icons: super::fonts::symbols_available(),
            closing: false,
            theme: saved.theme,
        };
        app.overview.refresh(&mut app.shell);
        app
    }

    fn dispatch_results(&mut self) {
        for (route, finished) in self.shell.poll() {
            let name = finished.name.clone();
            let result = finished.result;
            match route {
                Route::Header => self.header.on_finished(&name, result, &mut self.shell),
                Route::Overview => self.overview.on_finished(&name, result, &mut self.shell),
                Route::Analysis => self.analysis.on_finished(&name, result, &mut self.shell),
                Route::Cleanup => self.cleanup.on_finished(&name, result, &mut self.shell),
                Route::History => self.history.on_finished(&name, result, &mut self.shell),
                Route::Quarantine => self.quarantine.on_finished(&name, result, &mut self.shell),
                Route::Settings => self.settings.on_finished(&name, result, &mut self.shell),
            }
        }
        if let Some(goto) = self.shell.goto.take() {
            if let Some(kind) = goto.plan_kind {
                self.cleanup.preselect(kind);
            }
            if let Some(tab) = goto.analysis_tab {
                self.analysis.preselect_tab(tab);
            }
            self.page = goto.page;
        }
    }

    /// Trägt ein neu gewähltes Ziel in die Liste der zuletzt benutzten ein, egal woher die
    /// Wahl kam (Kopfleiste, Übersicht, Sprung aus einer Ansicht).
    fn remember_target(&mut self) {
        if self.shell.target != self.remembered {
            if let Some(target) = self.shell.target.clone() {
                self.shell.remember(target);
            }
            self.remembered = self.shell.target.clone();
        }
    }

    fn handle_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.shell.runner.is_idle() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            let asked = self
                .shell
                .dialogs
                .iter()
                .any(|d| matches!(d, Dialog::Confirm { id, .. } if *id == ID_CLOSE));
            if !self.closing && !asked {
                self.shell.confirm(Dialog::Confirm {
                    id: ID_CLOSE,
                    title: texts::CLOSE_TITLE.into(),
                    text: texts::CLOSE_TEXT.into(),
                    ok_label: texts::CLOSE_CANCEL_AND_QUIT.into(),
                    must_check: None,
                    checked: false,
                    danger: false,
                });
            }
        }
        for (id, answer) in self.shell.drain_answers() {
            match id {
                ID_CLOSE => {
                    if answer.ok {
                        self.closing = true;
                        self.shell.runner.cancel_all();
                    }
                }
                id if id.starts_with("settings.") => {
                    self.settings.on_answer(id, answer, &mut self.shell)
                }
                id if id.starts_with("overview.") => {
                    self.overview.on_answer(id, answer, &mut self.shell)
                }
                id if id.starts_with("history.") => {
                    self.history.on_answer(id, answer, &mut self.shell)
                }
                id if id.starts_with("quarantine.") => {
                    self.quarantine.on_answer(id, answer, &mut self.shell)
                }
                id if id.starts_with("cleanup.") => {
                    self.cleanup.on_answer(id, answer, &mut self.shell)
                }
                _ => {}
            }
        }
        if self.closing && self.shell.runner.is_idle() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("nav")
            .resizable(false)
            .default_size(150.0)
            .show(ui, |ui| {
                ui.add_space(crate::gui::theme::SPACE_M);
                ui.heading(texts::TITLE);
                ui.add_space(crate::gui::theme::SPACE_M);
                for (index, page) in Page::ALL.into_iter().enumerate() {
                    if ui
                        .selectable_label(self.page == page, page.nav_label(self.icons))
                        .on_hover_text(format!("Strg+{}", index + 1))
                        .clicked()
                    {
                        self.page = page;
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(crate::gui::theme::SPACE_M);
                    if ui.button("? Kürzel").clicked() {
                        self.help.toggle_keys();
                    }
                    if ui.button("? Hilfe").on_hover_text("F1").clicked() {
                        let topic = self.current_topic();
                        self.help.show_topic(topic);
                    }
                });
            });
    }

    /// Hell, dunkel oder wie Windows; gilt sofort und wird mit dem Fensterzustand gemerkt.
    fn appearance_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Darstellung:");
            for (choice, label) in theme::CHOICES {
                if ui
                    .selectable_value(&mut self.theme, choice, label)
                    .clicked()
                {
                    theme::apply(ui.ctx(), choice);
                }
            }
        });
        ui.separator();
    }

    /// Tastenkürzel verteilen. Eine verbrauchte Taste sehen die Widgets nicht mehr.
    fn handle_keys(&mut self, ctx: &egui::Context) {
        let context = KeyContext {
            dialog_open: !self.shell.dialogs.is_empty(),
            text_focus: ctx.egui_wants_keyboard_input(),
            review_open: self.page == Page::Cleanup && self.cleanup.review_open(),
        };
        let pressed: Vec<(egui::Key, egui::Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        for (key, modifiers) in pressed {
            let Some(action) = keys::action(key, modifiers.command, modifiers.shift, context)
            else {
                continue;
            };
            ctx.input_mut(|i| i.consume_key(modifiers, key));
            self.run_key(action, ctx);
        }
        if !context.text_focus
            && !context.dialog_open
            && ctx.input(|i| {
                i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Text(t) if t == "?"))
            })
        {
            self.help.toggle_keys();
        }
        if !context.dialog_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F1))
        {
            let topic = self.current_topic();
            self.help.show_topic(topic);
        }
    }

    /// Das Thema, das zur Seite (bzw. zur offenen Review-Ansicht) passt; Ziel von F1.
    fn current_topic(&self) -> Topic {
        if self.page == Page::Cleanup && self.cleanup.review_open() {
            Topic::Review
        } else {
            Topic::Page(self.page)
        }
    }

    fn run_key(&mut self, action: KeyAction, ctx: &egui::Context) {
        match action {
            KeyAction::Page(page) => self.page = page,
            KeyAction::Reload => self.shell.generation += 1,
            KeyAction::OpenPlan => {
                self.page = Page::Cleanup;
                self.cleanup.request_open_plan(&mut self.shell);
            }
            other => self.cleanup.review_key(other, ctx, &mut self.shell),
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let running: Vec<_> = self
                    .shell
                    .runner
                    .running()
                    .into_iter()
                    .filter(|r| !header::is_silent(&r.name))
                    .collect();
                if self.closing {
                    ui.label(texts::CLOSING);
                } else if let Some(task) = running.first() {
                    ui.spinner();
                    ui.label(texts::running(&task.name));
                    ui.label(task.ctx.progress.current());
                    if ui.button(texts::CANCEL).clicked() {
                        task.ctx
                            .cancel
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                } else {
                    ui.label(&self.shell.status);
                }
            });
        });
    }
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.dispatch_results();
        self.remember_target();
        self.handle_close(&ctx);
        if !self.shell.runner.is_idle() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(topic) = self.shell.help_request.take() {
            self.help.show_topic(topic);
        }
        self.handle_keys(&ctx);
        self.sidebar(ui);
        self.status_bar(ui);
        if self.page != Page::Settings {
            egui::Panel::top("header").show(ui, |ui| self.header.ui(ui, &mut self.shell));
        }
        egui::CentralPanel::default_margins().show(ui, |ui| match self.page {
            Page::Overview => self.overview.ui(ui, &mut self.shell),
            Page::Analysis => self.analysis.ui(ui, &mut self.shell),
            Page::Cleanup => self.cleanup.ui(ui, &mut self.shell),
            Page::History => self.history.ui(ui, &mut self.shell),
            Page::Quarantine => self.quarantine.ui(ui, &mut self.shell),
            Page::Settings => {
                self.appearance_ui(ui);
                self.settings.ui(ui, &mut self.shell)
            }
        });
        self.help.ui(&ctx);
        dialogs::show(&ctx, &mut self.shell);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(
            storage,
            STATE_KEY,
            &Persisted {
                page: self.page,
                target: self.shell.target.clone(),
                theme: self.theme,
                recent: self.shell.recent_targets.clone(),
            },
        );
    }
}
