//! Meldungs- und Bestätigungsdialoge über dem Hauptfenster.

use eframe::egui::{self, Align2};

use crate::gui::shell::{Answer, Dialog, Shell};
use crate::gui::texts;

/// Antwort auf Esc: eine Meldung wird quittiert, eine Rückfrage abgelehnt (nie bestätigt).
pub fn esc_answer(dialog: &Dialog, checked: bool) -> Answer {
    Answer {
        ok: matches!(dialog, Dialog::Message { .. }),
        checked,
    }
}

/// Zeichnet den obersten Dialog; ein Dialog nach dem anderen.
pub fn show(ctx: &egui::Context, shell: &mut Shell) {
    let Some(dialog) = shell.dialogs.first().cloned() else {
        return;
    };
    let mut result: Option<Answer> = None;
    let mut checked = matches!(&dialog, Dialog::Confirm { checked: true, .. });
    let title = match &dialog {
        Dialog::Message { title, .. } | Dialog::Confirm { title, .. } => title.clone(),
    };
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .max_width(560.0)
        .show(ctx, |ui| match &dialog {
            Dialog::Message { text, .. } => {
                ui.label(text);
                ui.add_space(8.0);
                if ui.button(texts::OK).clicked() {
                    result = Some(Answer {
                        ok: true,
                        checked: false,
                    });
                }
            }
            Dialog::Confirm {
                text,
                ok_label,
                must_check,
                danger,
                ..
            } => {
                ui.label(text);
                if let Some(label) = must_check {
                    ui.add_space(6.0);
                    ui.checkbox(&mut checked, label);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let enabled = must_check.is_none() || checked;
                    let mut button = egui::Button::new(ok_label.as_str());
                    if *danger {
                        button = button
                            .fill(crate::gui::theme::palette(ui.visuals().dark_mode).danger_fill);
                    }
                    if ui.add_enabled(enabled, button).clicked() {
                        result = Some(Answer { ok: true, checked });
                    }
                    if ui.button(texts::CANCEL).clicked() {
                        result = Some(Answer { ok: false, checked });
                    }
                });
            }
        });
    if result.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        result = Some(esc_answer(&dialog, checked));
    }
    if let Dialog::Confirm { checked: state, .. } = &mut shell.dialogs[0] {
        *state = checked;
    }
    if let Some(answer) = result {
        let dialog = shell.dialogs.remove(0);
        if let Dialog::Confirm { id, .. } = dialog {
            shell.set_answer(id, answer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirm() -> Dialog {
        Dialog::Confirm {
            id: "t",
            title: "T".into(),
            text: "x".into(),
            ok_label: "Ja".into(),
            must_check: None,
            checked: false,
            danger: true,
        }
    }

    #[test]
    fn esc_lehnt_rueckfragen_ab_und_quittiert_meldungen() {
        let a = esc_answer(&confirm(), true);
        assert!(!a.ok, "Esc darf nie bestätigen");
        let m = Dialog::Message {
            title: "T".into(),
            text: "x".into(),
        };
        assert!(esc_answer(&m, false).ok);
    }
}
