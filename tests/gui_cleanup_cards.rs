//! Rauchtests der Karten-Ansicht „Aufräumen“ ohne Fenster und GPU.

use std::time::{Duration, Instant};

use eframe::egui::accesskit::{Role, Toggled};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use ordner_cleanup::gui::header::ScanOptions;
use ordner_cleanup::gui::shell::{Choice, Route, Shell};
use ordner_cleanup::gui::views::cleanup::CleanupView;
use ordner_cleanup::ops::scan::scan;
use ordner_cleanup::ops::OpCtx;
use ordner_cleanup::paths::{config_dir, HOME_OVERRIDE_ENV};

/// Tests, die `ORDNER_CLEANUP_HOME` setzen, laufen nacheinander.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct State {
    shell: Shell,
    cleanup: CleanupView,
}

impl State {
    fn new(target: Option<Choice>) -> Self {
        let mut shell = Shell::new(eframe::egui::Context::default());
        shell.target = target;
        Self {
            shell,
            cleanup: CleanupView::default(),
        }
    }

    fn pump(&mut self) {
        for (route, finished) in self.shell.poll() {
            let name = finished.name.clone();
            if route == Route::Cleanup {
                self.cleanup
                    .on_finished(&name, finished.result, &mut self.shell);
            }
        }
    }
}

fn harness(state: State) -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, s: &mut State| {
            s.pump();
            s.cleanup.ui(ui, &mut s.shell);
        },
        state,
    )
}

fn wait_for(harness: &mut Harness<'static, State>, label: &str) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        harness.step();
        if harness.query_all_by_label_contains(label).next().is_some() {
            return;
        }
        assert!(Instant::now() < end, "„{label}“ erschien nicht");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn click_card(harness: &mut Harness<'static, State>, title: &str) {
    harness.get_by_role_and_label(Role::Button, title).click();
    harness.step();
}

fn with_home() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::env::set_var(HOME_OVERRIDE_ENV, home.path());
    (guard, home)
}

#[test]
fn sieben_karten_ohne_ziel_und_ohne_zahlen() {
    let (_guard, _home) = with_home();
    let mut h = harness(State::new(None));
    wait_for(&mut h, "Wähle oben einen Ordner");
    for title in [
        "Doppelte Ordner",
        "Duplikate",
        "Müll",
        "Leere Ordner",
        "Archivieren",
        "Versionen",
        "Nach Regeln",
    ] {
        h.get_by_role_and_label(Role::Button, title);
    }
    assert!(h
        .get_by_label("Plan erzeugen")
        .accesskit_node()
        .is_disabled());
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn duplikat_karte_zeigt_die_zahl_aus_dem_scan_ohne_neue_analyse() {
    let (_guard, home) = with_home();
    let root = home.path().join("daten");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "gleicher Inhalt gleicher Inhalt").unwrap();
    std::fs::write(root.join("b.txt"), "gleicher Inhalt gleicher Inhalt").unwrap();
    let choice = Choice::Folder(root);
    let request = ScanOptions::default().scan_request(&choice);
    scan(&request, &OpCtx::default()).unwrap();

    let mut h = harness(State::new(Some(choice)));
    wait_for(&mut h, "1 Gruppe");
    assert!(h
        .query_all_by_label_contains("könnten frei werden")
        .next()
        .is_some());
    // Archivieren, Versionen und Regeln bekommen keine Zahl.
    click_card(&mut h, "Archivieren");
    assert!(h.query_by_label_contains("Älter als").is_some());
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn karte_doppelte_ordner_zeigt_die_zahl_und_steht_vor_duplikate() {
    let (_guard, home) = with_home();
    let root = home.path().join("daten");
    for dir in ["Projekt", "Kopie von Projekt"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        std::fs::write(
            root.join(dir).join("a.txt"),
            "gleicher Inhalt gleicher Inhalt",
        )
        .unwrap();
    }
    let choice = Choice::Folder(root);
    let request = ScanOptions::default().scan_request(&choice);
    scan(&request, &OpCtx::default()).unwrap();

    let mut h = harness(State::new(Some(choice)));
    wait_for(&mut h, "könnten frei werden");
    // Beide Karten nennen eine Gruppe: Ordner-Kopie und (darin) die einzelne Datei.
    let dirs = h.get_by_role_and_label(Role::Button, "Doppelte Ordner");
    let files = h.get_by_role_and_label(Role::Button, "Duplikate");
    let (a, b) = (
        dirs.accesskit_node().raw_bounds().unwrap(),
        files.accesskit_node().raw_bounds().unwrap(),
    );
    assert!(
        (a.y0, a.x0) < (b.y0, b.x0),
        "„Doppelte Ordner“ steht vor „Duplikate“"
    );
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn regeldatei_mit_syntaxfehler_zeigt_datei_statt_abzustuerzen() {
    let (_guard, _home) = with_home();
    let dir = config_dir().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rules.toml"), "[[rules]]\nname = \n").unwrap();
    let mut h = harness(State::new(None));
    h.step();
    click_card(&mut h, "Nach Regeln");
    wait_for(&mut h, "rules.toml");
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn regelnamen_erscheinen_als_haekchen() {
    let (_guard, _home) = with_home();
    let dir = config_dir().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("rules.toml"),
        "[[rules]]\nname = \"pdf\"\next = [\"pdf\"]\ntarget = \"Dokumente/\"\n",
    )
    .unwrap();
    let mut h = harness(State::new(None));
    h.step();
    click_card(&mut h, "Nach Regeln");
    wait_for(&mut h, "Diese Regeln anwenden");
    let boxed = h.get_by_label("pdf");
    assert_eq!(boxed.accesskit_node().toggled(), Some(Toggled::True));
    std::env::remove_var(HOME_OVERRIDE_ENV);
}
