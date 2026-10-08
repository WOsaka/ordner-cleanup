//! Rauchtests der Kopfleiste und des Leerzustands der Übersicht ohne Fenster und GPU.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use ordner_cleanup::gui::header::Header;
use ordner_cleanup::gui::shell::{Choice, HeaderRequest, Route, Shell};
use ordner_cleanup::gui::views::overview::OverviewView;
use ordner_cleanup::paths::{config_dir, HOME_OVERRIDE_ENV};

/// Tests, die `ORDNER_CLEANUP_HOME` setzen, laufen nacheinander.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct State {
    shell: Shell,
    header: Header,
    overview: OverviewView,
}

impl State {
    fn new() -> Self {
        let mut state = Self {
            shell: Shell::new(eframe::egui::Context::default()),
            header: Header::default(),
            overview: OverviewView::default(),
        };
        state.overview.refresh(&mut state.shell);
        state
    }

    /// Reicht beendete Hintergrund-Tasks an die Ansichten weiter, wie `GuiApp` es tut.
    fn pump(&mut self) {
        for (route, finished) in self.shell.poll() {
            let name = finished.name.clone();
            match route {
                Route::Header => self
                    .header
                    .on_finished(&name, finished.result, &mut self.shell),
                Route::Overview => {
                    self.overview
                        .on_finished(&name, finished.result, &mut self.shell)
                }
                _ => {}
            }
        }
    }
}

fn harness(state: State) -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, s: &mut State| {
            s.pump();
            s.header.ui(ui, &mut s.shell);
            s.overview.ui(ui, &mut s.shell);
        },
        state,
    )
}

/// Zeichnet, bis `label` (Teiltext eines Labels oder genauer Wert) erscheint; die Tasks laufen in Threads.
fn wait_for(harness: &mut Harness<'static, State>, label: &str) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        harness.step();
        if harness.query_by_label_contains(label).is_some()
            || harness.query_by_value(label).is_some()
        {
            return;
        }
        assert!(Instant::now() < end, "„{label}“ erschien nicht");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn with_home() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::env::set_var(HOME_OVERRIDE_ENV, home.path());
    (guard, home)
}

#[test]
fn ohne_ziel_fordert_die_kopfleiste_zur_wahl_auf_und_sperrt_scannen() {
    let (_guard, _home) = with_home();
    let mut h = harness(State::new());
    wait_for(&mut h, "Ordner wählen …");
    assert!(h.get_by_label("Scannen").accesskit_node().is_disabled());
    assert!(h
        .get_by_label("Klassifizieren")
        .accesskit_node()
        .is_disabled());
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn gewaehltes_profil_steht_im_kopf_und_nicht_profil_punkte() {
    let (_guard, _home) = with_home();
    let dir = config_dir().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        "[profiles.downloads]\nroot = 'D:\\Downloads'\n",
    )
    .unwrap();
    let mut state = State::new();
    state.shell.target = Some(Choice::Profile("downloads".into()));
    let mut h = harness(state);
    wait_for(&mut h, "Profil downloads");
    assert!(h.query_by_value("Profil …").is_none());
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn fehlender_ordner_wird_markiert_und_scannen_gesperrt() {
    let (_guard, home) = with_home();
    let mut state = State::new();
    state.shell.target = Some(Choice::Folder(home.path().join("gibt-es-nicht")));
    let mut h = harness(state);
    wait_for(&mut h, "Ordner nicht gefunden");
    assert!(h.get_by_label("Scannen").accesskit_node().is_disabled());
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn vorhandener_ordner_erlaubt_scannen() {
    let (_guard, home) = with_home();
    let mut state = State::new();
    let folder: PathBuf = home.path().to_path_buf();
    state.shell.target = Some(Choice::Folder(folder));
    let mut h = harness(state);
    wait_for(&mut h, "Noch nicht gescannt");
    assert!(!h.get_by_label("Scannen").accesskit_node().is_disabled());
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn leere_uebersicht_stellt_den_wunsch_ordner_waehlen_und_scannen() {
    let (_guard, _home) = with_home();
    // Ohne Kopfleiste gezeichnet, damit kein echter Ordnerdialog aufgeht.
    let mut h = Harness::new_ui_state(
        |ui, s: &mut State| {
            s.pump();
            s.overview.ui(ui, &mut s.shell);
        },
        State::new(),
    );
    wait_for(&mut h, "Noch nichts gescannt");
    assert_eq!(h.state().shell.header_request, None);
    h.get_by_label("Ordner wählen und scannen").click();
    h.step();
    assert_eq!(
        h.state().shell.header_request,
        Some(HeaderRequest::PickAndScan)
    );
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn profilzeile_oeffnen_setzt_das_ziel_in_der_kopfleiste() {
    let (_guard, _home) = with_home();
    let dir = config_dir().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        "[profiles.downloads]\nroot = 'D:\\Downloads'\n",
    )
    .unwrap();
    let mut h = harness(State::new());
    wait_for(&mut h, "noch kein Lauf");
    assert!(h.query_by_label("Wählen").is_none(), "alter Knopfname");
    assert_eq!(h.state().shell.target, None);
    h.get_by_label("Öffnen").click();
    h.step();
    assert_eq!(
        h.state().shell.target,
        Some(Choice::Profile("downloads".into()))
    );
    std::env::remove_var(HOME_OVERRIDE_ENV);
}
