//! Rauchtests der Oberfläche ohne Fenster und GPU (egui_kittest, AccessKit-Baum).

use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use ordner_cleanup::change::plan::{ActionType, Plan, PlanKind, PlannedAction, PLAN_VERSION};
use ordner_cleanup::gui::review::ReviewModel;
use ordner_cleanup::gui::shell::{Dialog, Shell};
use ordner_cleanup::gui::views::review::ReviewState;
use ordner_cleanup::gui::widgets::dialogs;

fn plan(n: u32) -> Plan {
    Plan {
        version: PLAN_VERSION,
        created: "2026-10-05T10:00:00+02:00".into(),
        kind: PlanKind::Junk,
        root: r"D:\Daten".into(),
        keep_strategy: None,
        params: Default::default(),
        protected_paths: Vec::new(),
        actions: (1..=n)
            .map(|id| PlannedAction {
                id,
                action: ActionType::Quarantine,
                path: format!(r"D:\Daten\ordner{}\datei{id}.tmp", id % 10),
                size: 100,
                mtime_ticks: 0,
                mtime: String::new(),
                hash: None,
                keep: None,
                keep_hash: None,
                reason: "junk:temp".into(),
                target: None,
                is_dir: false,
                keep_fingerprint: None,
                source_fingerprint: None,
                files: None,
                rule: None,
            })
            .collect(),
        skipped: vec![],
    }
}

/// Tests, die `ORDNER_CLEANUP_HOME` setzen, laufen nacheinander.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct State {
    review: ReviewState,
    shell: Shell,
    apply_clicked: bool,
}

#[test]
fn review_mit_tausend_eintraegen_zeigt_zusammenfassung_und_loest_anwenden_aus() {
    // Der Hinweis liest die Quarantäne-Tage aus der Config: eine leere, eigene verwenden.
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::env::set_var(ordner_cleanup::paths::HOME_OVERRIDE_ENV, home.path());
    let model = ReviewModel::new(plan(1000));
    let review = ReviewState::new(model, None, "1000 Aktionen".into(), vec![]);
    std::env::remove_var(ordner_cleanup::paths::HOME_OVERRIDE_ENV);
    let shell = Shell::new(eframe::egui::Context::default());
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut State| {
            if state.review.ui(ui, &mut state.shell) {
                state.apply_clicked = true;
            }
        },
        State {
            review,
            shell,
            apply_clicked: false,
        },
    );
    harness.run();
    harness.get_by_label_contains("Ausgewählt: 1.000 von 1.000 Einträgen");
    harness.get_by_label_contains("Nichts wird gelöscht");
    harness.get_by_label_contains("Übersprungene zeigen (0)");
    harness
        .get_by_label_contains("Anwenden (1.000 Einträge")
        .click();
    harness.run();
    assert!(harness.state().apply_clicked);
}

#[test]
fn obergrenze_dialog_braucht_den_haken() {
    let mut shell = Shell::new(eframe::egui::Context::default());
    shell.confirm(Dialog::Confirm {
        id: "test.apply",
        title: "Anwenden".into(),
        text: "3 Dateien".into(),
        ok_label: "Jetzt anwenden".into(),
        must_check: Some("Obergrenze für diesen Plan aufheben".into()),
        checked: false,
        danger: true,
    });
    let mut harness = Harness::new_ui_state(
        |ui, shell: &mut Shell| dialogs::show(&ui.ctx().clone(), shell),
        shell,
    );
    harness.run();
    assert!(harness
        .get_by_label("Jetzt anwenden")
        .accesskit_node()
        .is_disabled());
    harness
        .get_by_label("Obergrenze für diesen Plan aufheben")
        .click();
    harness.run();
    assert!(!harness
        .get_by_label("Jetzt anwenden")
        .accesskit_node()
        .is_disabled());
    harness.get_by_label("Jetzt anwenden").click();
    harness.run();
    let answers = harness.state_mut().drain_answers();
    assert_eq!(answers.len(), 1);
    assert!(answers[0].1.ok && answers[0].1.checked);
}

#[test]
fn alle_seiten_lassen_sich_ohne_daten_zeichnen() {
    use ordner_cleanup::gui::views::{
        analysis::AnalysisView, cleanup::CleanupView, history::HistoryView, overview::OverviewView,
        quarantine::QuarantineView, settings::SettingsView,
    };
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::env::set_var(ordner_cleanup::paths::HOME_OVERRIDE_ENV, home.path());
    struct Pages {
        shell: Shell,
        overview: OverviewView,
        analysis: AnalysisView,
        cleanup: CleanupView,
        history: HistoryView,
        quarantine: QuarantineView,
        settings: SettingsView,
    }
    let pages = Pages {
        shell: Shell::new(eframe::egui::Context::default()),
        overview: OverviewView::default(),
        analysis: AnalysisView::default(),
        cleanup: CleanupView::default(),
        history: HistoryView::default(),
        quarantine: QuarantineView::default(),
        settings: SettingsView::default(),
    };
    let mut harness = Harness::new_ui_state(
        |ui, p: &mut Pages| {
            p.overview.ui(ui, &mut p.shell);
            p.analysis.ui(ui, &mut p.shell);
            p.cleanup.ui(ui, &mut p.shell);
            p.history.ui(ui, &mut p.shell);
            p.quarantine.ui(ui, &mut p.shell);
            p.settings.ui(ui, &mut p.shell);
        },
        pages,
    );
    let _ = harness.run_ok();
    harness.get_by_label("Plan erzeugen");
}

#[test]
fn einstellungen_zeigen_alle_tabs_mit_echten_dateien() {
    use ordner_cleanup::gui::views::settings::SettingsView;
    use ordner_cleanup::paths::{config_dir, HOME_OVERRIDE_ENV};

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::env::set_var(HOME_OVERRIDE_ENV, home.path());
    let dir = config_dir().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        "# Meine Config\ntop = 40\n\n[profiles.downloads]\nroot = 'D:\\Downloads'\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("rules.toml"),
        "[[rules]]\nname = \"pdf\"\next = [\"pdf\"]\ntarget = \"Dokumente/\"\n",
    )
    .unwrap();

    struct State {
        shell: Shell,
        settings: SettingsView,
    }
    let mut harness = Harness::new_ui_state(
        |ui, s: &mut State| s.settings.ui(ui, &mut s.shell),
        State {
            shell: Shell::new(eframe::egui::Context::default()),
            settings: SettingsView::default(),
        },
    );
    let _ = harness.run_ok();
    harness.get_by_label("Alt ab");
    for (tab, expected) in [
        ("Profile & Zeitpläne", "Profil downloads"),
        ("Regeln", "Vorschau: Treffer"),
        ("Klassifikation & LLM", "Verbindung testen"),
        ("Kategorien", "Eingebaute Kategorien"),
        ("Vorlagen", "Eingebaute Vorlagen"),
        ("Dateien", "Verknüpfung im Startmenü anlegen"),
    ] {
        harness.get_by_label(tab).click();
        let _ = harness.run_ok();
        harness.get_by_label_contains(expected);
    }
    std::env::remove_var(HOME_OVERRIDE_ENV);
}

#[test]
fn review_laesst_sich_mit_der_tastatur_bedienen_und_anwenden_oeffnet_nur_den_dialog() {
    use eframe::egui::Context;
    use ordner_cleanup::gui::keys::KeyAction;

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::env::set_var(ordner_cleanup::paths::HOME_OVERRIDE_ENV, home.path());
    let mut review = ReviewState::new(ReviewModel::new(plan(5)), None, "5".into(), vec![]);
    std::env::remove_var(ordner_cleanup::paths::HOME_OVERRIDE_ENV);
    let ctx = Context::default();
    let mut shell = Shell::new(ctx.clone());
    // Erste Zeile fokussieren, abwählen (Leertaste springt weiter), zweite ebenso.
    review.handle_key(KeyAction::Next, &ctx, &mut shell);
    review.handle_key(KeyAction::Toggle, &ctx, &mut shell);
    review.handle_key(KeyAction::Toggle, &ctx, &mut shell);
    assert_eq!(review.model.summary().selected, 3);
    // Strg+Umschalt+A wählt alle gefilterten ab; Anwenden bleibt dann wirkungslos.
    review.handle_key(KeyAction::DeselectFiltered, &ctx, &mut shell);
    review.handle_key(KeyAction::Apply, &ctx, &mut shell);
    review.handle_key(KeyAction::SelectFiltered, &ctx, &mut shell);
    review.handle_key(KeyAction::Apply, &ctx, &mut shell);

    let mut harness = Harness::new_ui_state(
        |ui, state: &mut State| {
            if state.review.ui(ui, &mut state.shell) {
                state.apply_clicked = true;
            }
        },
        State {
            review,
            shell,
            apply_clicked: false,
        },
    );
    harness.run();
    harness.get_by_label_contains("Ausgewählt: 5 von 5 Einträgen");
    // Nur das gemeldete Anwenden aus dem zweiten Strg+Enter kommt an; es startet nichts selbst.
    assert!(harness.state().apply_clicked);
    assert!(harness.state().shell.dialogs.is_empty());
}

fn quarantine_list(count: usize) -> ordner_cleanup::ops::quarantine::QuarantineList {
    use ordner_cleanup::change::restore::QuarantineItem;
    use ordner_cleanup::change::RunId;
    use ordner_cleanup::ops::quarantine::{QuarantineList, RootItems};
    let run = RunId::parse("20261003-120000-ab12").unwrap();
    let items = (0..count)
        .map(|i| QuarantineItem {
            run: run.clone(),
            action: i as u32 + 1,
            origin: format!(r"D:\Daten\ordner{}\datei{i}.tmp", i % 50),
            stored: format!(r"D:\Daten\.ordner-cleanup\quarantine\{run}\datei{i}.tmp"),
            size: 1000,
            hash: None,
            is_dir: false,
            file_count: None,
            started: Some("2026-10-03T12:00:00Z".into()),
            expires: None,
            present: true,
            cloud_only: false,
        })
        .collect();
    QuarantineList {
        roots: vec![RootItems {
            root: r"D:\Daten".into(),
            items,
        }],
        unreadable: 0,
        unreachable: vec![],
    }
}

#[test]
fn quarantaene_zeigt_summe_und_zeilen_auch_bei_100000_eintraegen() {
    use ordner_cleanup::gui::views::quarantine::QuarantineView;
    struct State {
        shell: Shell,
        view: QuarantineView,
    }
    let mut view = QuarantineView::default();
    let shell = Shell::new(eframe::egui::Context::default());
    view.set_list(quarantine_list(100_000), shell.generation);
    let mut harness = Harness::new_ui_state(
        |ui, s: &mut State| s.view.ui(ui, &mut s.shell),
        State { shell, view },
    );
    let started = std::time::Instant::now();
    harness.run();
    harness.get_by_label_contains("100.000 Einträge");
    assert!(harness.query_all_by_label_contains("datei0.tmp").count() >= 1);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "nur sichtbare Zeilen werden gezeichnet: {:?}",
        started.elapsed()
    );
}

#[test]
fn leere_quarantaene_sagt_es() {
    use ordner_cleanup::gui::views::quarantine::QuarantineView;
    use ordner_cleanup::ops::quarantine::QuarantineList;
    struct State {
        shell: Shell,
        view: QuarantineView,
    }
    let mut view = QuarantineView::default();
    let shell = Shell::new(eframe::egui::Context::default());
    view.set_list(QuarantineList::default(), shell.generation);
    let mut harness = Harness::new_ui_state(
        |ui, s: &mut State| s.view.ui(ui, &mut s.shell),
        State { shell, view },
    );
    harness.run();
    harness.get_by_label("Die Quarantäne ist leer.");
}
