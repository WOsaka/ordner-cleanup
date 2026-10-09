//! Rauchtests der Hilfe ohne Fenster und GPU: „?“ an den Karten, Doku-Link, Esc-Reihenfolge.

use std::sync::atomic::{AtomicUsize, Ordering};

use eframe::egui;
use eframe::egui::accesskit::Role;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use ordner_cleanup::gui::help::{DocFile, Topic};
use ordner_cleanup::gui::shell::{Dialog, Shell};
use ordner_cleanup::gui::views::cleanup::CleanupView;
use ordner_cleanup::gui::views::help_window::HelpState;
use ordner_cleanup::gui::widgets::dialogs;
use ordner_cleanup::paths::HOME_OVERRIDE_ENV;

/// Tests, die `ORDNER_CLEANUP_HOME` setzen, laufen nacheinander.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static OPENED: AtomicUsize = AtomicUsize::new(0);

struct State {
    shell: Shell,
    cleanup: CleanupView,
    help: HelpState,
}

impl State {
    fn new(help: HelpState) -> Self {
        Self {
            shell: Shell::new(egui::Context::default()),
            cleanup: CleanupView::default(),
            help,
        }
    }
}

/// Wie `GuiApp::ui` in klein: Seite, Hilfeanfrage, Hilfefenster, danach die Dialoge.
fn harness(state: State) -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, s: &mut State| {
            s.cleanup.ui(ui, &mut s.shell);
            if let Some(topic) = s.shell.help_request.take() {
                s.help.show_topic(topic);
            }
            s.help.ui(&ui.ctx().clone());
            dialogs::show(&ui.ctx().clone(), &mut s.shell);
        },
        state,
    )
}

fn with_home() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::env::set_var(HOME_OVERRIDE_ENV, home.path());
    (guard, home)
}

fn opened_ok(_: DocFile) -> Result<(), String> {
    OPENED.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

fn opened_missing(file: DocFile) -> Result<(), String> {
    Err(format!(
        "Die Datei „{}“ wurde nicht gefunden.",
        file.label()
    ))
}

#[test]
fn fragezeichen_der_karte_oeffnet_deren_hilfe_und_waehlt_sie_nicht() {
    let (_guard, _home) = with_home();
    let mut h = harness(State::new(HelpState::default()));
    h.step();
    // Standardkarte ist „Duplikate“: ihre Option ist sichtbar, die von „Müll“ nicht.
    assert!(h.query_by_label_contains("Welche Kopie bleibt").is_some());
    assert!(h.query_by_label_contains("Kategorien (leer").is_none());

    h.get_by_role_and_label(Role::Button, "Hilfe zu Müll")
        .click();
    h.step();
    h.step();

    assert!(h.state().help.open);
    assert_eq!(h.state().help.topic, Topic::Card("junk"));
    assert!(h.query_by_label_contains("Sucht Müll").is_some());
    // Die Auswahl der Karten blieb bei „Duplikate“.
    assert!(h.query_by_label_contains("Welche Kopie bleibt").is_some());
    assert!(h.query_by_label_contains("Kategorien (leer").is_none());
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn karte_doppelte_ordner_hat_einen_hilfetext() {
    let (_guard, _home) = with_home();
    let mut h = harness(State::new(HelpState::default()));
    h.state_mut().help.show_topic(Topic::Card("dedupe-dirs"));
    h.step();
    h.step();
    assert!(h
        .query_all_by_label_contains("Doppelte Ordner")
        .next()
        .is_some());
    assert!(h
        .query_all_by_label_contains("ganze Ordner")
        .next()
        .is_some());
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn ueberschrift_der_seite_hat_ein_fragezeichen() {
    let (_guard, _home) = with_home();
    let mut h = harness(State::new(HelpState::default()));
    h.step();
    h.get_by_role_and_label(Role::Button, "Hilfe zu Aufräumen")
        .click();
    h.step();
    h.step();
    assert!(h.state().help.open);
    assert!(h.query_by_label_contains("Was es tut").is_some());
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn doku_link_oeffnet_die_datei_oder_meldet_den_pfad() {
    let (_guard, _home) = with_home();
    OPENED.store(0, Ordering::SeqCst);
    let mut h = harness(State::new(HelpState::with_opener(opened_ok)));
    h.state_mut().help.show_topic(Topic::Card("junk"));
    // Ein neues Fenster misst sich im ersten Bild und reagiert erst im zweiten.
    h.step();
    h.step();
    h.get_by_label("Ausführliche Doku öffnen").click();
    h.step();
    h.step();
    assert_eq!(OPENED.load(Ordering::SeqCst), 1);
    assert!(h.query_by_label_contains("wurde nicht gefunden").is_none());

    let mut h = harness(State::new(HelpState::with_opener(opened_missing)));
    h.state_mut().help.show_topic(Topic::Card("junk"));
    // Ein neues Fenster misst sich im ersten Bild und reagiert erst im zweiten.
    h.step();
    h.step();
    h.get_by_label("Ausführliche Doku öffnen").click();
    h.step();
    h.step();
    // Die Meldung erscheint, der Kurztext bleibt lesbar.
    assert!(h.query_by_label_contains("wurde nicht gefunden").is_some());
    assert!(h.query_by_label_contains("Sucht Müll").is_some());
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn esc_schliesst_zuerst_die_hilfe_dann_den_dialog() {
    let (_guard, _home) = with_home();
    let mut state = State::new(HelpState::default());
    state.shell.confirm(Dialog::Confirm {
        id: "test.confirm",
        title: "Frage".into(),
        text: "Wirklich?".into(),
        ok_label: "Ja".into(),
        must_check: None,
        checked: false,
        danger: false,
    });
    state.help.show_topic(Topic::Review);
    let mut h = harness(state);
    h.step();

    h.key_press(egui::Key::Escape);
    h.step();
    assert!(!h.state().help.open, "Esc schließt zuerst die Hilfe");
    assert_eq!(h.state().shell.dialogs.len(), 1, "der Dialog bleibt offen");

    h.key_press(egui::Key::Escape);
    h.step();
    assert!(
        h.state().shell.dialogs.is_empty(),
        "das zweite Esc gilt dem Dialog"
    );
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn kuerzel_liste_nennt_f1() {
    let (_guard, _home) = with_home();
    let mut h = harness(State::new(HelpState::default()));
    h.state_mut().help.toggle_keys();
    h.step();
    assert!(h
        .query_by_label_contains("Hilfe zur aktuellen Seite")
        .is_some());
    h.state_mut().help.toggle_keys();
    h.step();
    assert!(!h.state().help.open);
    std::env::remove_var(HOME_OVERRIDE_ENV);
}
