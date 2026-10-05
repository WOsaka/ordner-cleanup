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
                files: None,
                rule: None,
            })
            .collect(),
        skipped: vec![],
    }
}

struct State {
    review: ReviewState,
    shell: Shell,
    apply_clicked: bool,
}

#[test]
fn review_mit_tausend_eintraegen_zeigt_zusammenfassung_und_loest_anwenden_aus() {
    let model = ReviewModel::new(plan(1000));
    let review = ReviewState::new(model, None, "1000 Aktionen".into(), vec![]);
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
    harness.get_by_label("Anwenden …").click();
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
        settings::SettingsView,
    };
    struct Pages {
        shell: Shell,
        overview: OverviewView,
        analysis: AnalysisView,
        cleanup: CleanupView,
        history: HistoryView,
        settings: SettingsView,
    }
    let pages = Pages {
        shell: Shell::new(eframe::egui::Context::default()),
        overview: OverviewView::default(),
        analysis: AnalysisView::default(),
        cleanup: CleanupView::default(),
        history: HistoryView::default(),
        settings: SettingsView::default(),
    };
    let mut harness = Harness::new_ui_state(
        |ui, p: &mut Pages| {
            p.overview.ui(ui, &mut p.shell);
            p.analysis.ui(ui, &mut p.shell);
            p.cleanup.ui(ui, &mut p.shell);
            p.history.ui(ui, &mut p.shell);
            p.settings.ui(ui, &mut p.shell);
        },
        pages,
    );
    harness.run();
    harness.get_by_label("Verknüpfung im Startmenü anlegen");
    harness.get_by_label("Plan erzeugen");
}
