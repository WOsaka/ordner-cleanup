//! Tastenkürzel: übersetzt eine Taste in eine Aktion der Oberfläche. Reine Logik ohne Fenster.
//! Kein Kürzel wendet an oder löscht; `Apply` öffnet nur den Bestätigungsdialog.

use eframe::egui::Key;

use super::shell::Page;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Next,
    Prev,
    Toggle,
    SelectFiltered,
    DeselectFiltered,
    FocusSearch,
    /// Öffnet den Bestätigungsdialog „Anwenden“, nichts weiter
    Apply,
    OpenPlan,
    Reload,
    Page(Page),
}

/// Wo die Taste gedrückt wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyContext {
    /// Ein Dialog ist offen; er gehört allein der Tastatur
    pub dialog_open: bool,
    /// Ein Textfeld hat den Fokus; getippte Zeichen gehören ihm
    pub text_focus: bool,
    /// Die Review-Tabelle ist sichtbar (Seite Aufräumen mit geöffnetem Plan)
    pub review_open: bool,
}

/// Die Aktion zu einer Taste (`ctrl` = Strg, `shift` = Umschalt) oder `None`.
pub fn action(key: Key, ctrl: bool, shift: bool, ctx: KeyContext) -> Option<KeyAction> {
    if ctx.dialog_open || ctx.text_focus {
        return None;
    }
    if ctrl {
        let page = |index: usize| Some(KeyAction::Page(Page::ALL[index]));
        return match key {
            Key::Num1 => page(0),
            Key::Num2 => page(1),
            Key::Num3 => page(2),
            Key::Num4 => page(3),
            Key::Num5 => page(4),
            Key::O => Some(KeyAction::OpenPlan),
            Key::R => Some(KeyAction::Reload),
            Key::A if ctx.review_open => Some(if shift {
                KeyAction::DeselectFiltered
            } else {
                KeyAction::SelectFiltered
            }),
            Key::F if ctx.review_open => Some(KeyAction::FocusSearch),
            Key::Enter if ctx.review_open => Some(KeyAction::Apply),
            _ => None,
        };
    }
    if !ctx.review_open || shift {
        return None;
    }
    match key {
        Key::ArrowDown => Some(KeyAction::Next),
        Key::ArrowUp => Some(KeyAction::Prev),
        Key::Space => Some(KeyAction::Toggle),
        _ => None,
    }
}

/// Kürzel für die Hilfeliste: Taste, Wirkung.
pub const HELP: [(&str, &str); 12] = [
    ("↑ / ↓", "Zeile wechseln (Review)"),
    (
        "Leertaste",
        "Eintrag an-/abwählen, eine Zeile weiter (Review)",
    ),
    ("Strg+A", "Alle gefilterten wählen (Review)"),
    ("Strg+Umschalt+A", "Gefilterte abwählen (Review)"),
    ("Strg+F", "Suchfeld fokussieren (Review)"),
    (
        "Strg+Enter",
        "„Anwenden …“ öffnen (Review, mit Bestätigung)",
    ),
    ("Strg+O", "Plan öffnen"),
    ("Strg+R", "Neu laden"),
    ("Strg+1 … Strg+5", "Seite wechseln"),
    ("Esc", "Dialog schließen"),
    ("F1", "Hilfe zur aktuellen Seite"),
    ("?", "Diese Liste"),
];

#[cfg(test)]
mod tests {
    use super::*;

    const REVIEW: KeyContext = KeyContext {
        dialog_open: false,
        text_focus: false,
        review_open: true,
    };
    const PLAIN: KeyContext = KeyContext {
        review_open: false,
        ..REVIEW
    };

    #[test]
    fn pfeile_und_leertaste_wirken_nur_im_review() {
        assert_eq!(
            action(Key::ArrowDown, false, false, REVIEW),
            Some(KeyAction::Next)
        );
        assert_eq!(
            action(Key::ArrowUp, false, false, REVIEW),
            Some(KeyAction::Prev)
        );
        assert_eq!(
            action(Key::Space, false, false, REVIEW),
            Some(KeyAction::Toggle)
        );
        for key in [Key::ArrowDown, Key::ArrowUp, Key::Space] {
            assert_eq!(action(key, false, false, PLAIN), None, "{key:?}");
        }
        // Mit Strg sind es andere Kürzel, keine Zeilenwechsel.
        assert_eq!(action(Key::ArrowDown, true, false, REVIEW), None);
    }

    #[test]
    fn auswahl_und_suche_im_review() {
        assert_eq!(
            action(Key::A, true, false, REVIEW),
            Some(KeyAction::SelectFiltered)
        );
        assert_eq!(
            action(Key::A, true, true, REVIEW),
            Some(KeyAction::DeselectFiltered)
        );
        assert_eq!(
            action(Key::F, true, false, REVIEW),
            Some(KeyAction::FocusSearch)
        );
        assert_eq!(action(Key::A, false, false, REVIEW), None);
        assert_eq!(action(Key::A, true, false, PLAIN), None);
    }

    #[test]
    fn anwenden_nur_mit_strg_enter_und_nur_im_review() {
        assert_eq!(
            action(Key::Enter, true, false, REVIEW),
            Some(KeyAction::Apply)
        );
        assert_eq!(action(Key::Enter, false, false, REVIEW), None);
        assert_eq!(action(Key::Enter, true, false, PLAIN), None);
    }

    #[test]
    fn globale_kuerzel() {
        assert_eq!(
            action(Key::O, true, false, PLAIN),
            Some(KeyAction::OpenPlan)
        );
        assert_eq!(action(Key::R, true, false, PLAIN), Some(KeyAction::Reload));
        assert_eq!(
            action(Key::Num1, true, false, PLAIN),
            Some(KeyAction::Page(Page::Overview))
        );
        assert_eq!(
            action(Key::Num3, true, false, PLAIN),
            Some(KeyAction::Page(Page::Cleanup))
        );
        assert_eq!(
            action(Key::Num5, true, false, PLAIN),
            Some(KeyAction::Page(Page::Settings))
        );
        assert_eq!(action(Key::Num6, true, false, PLAIN), None);
        assert_eq!(action(Key::Num1, false, false, PLAIN), None);
    }

    #[test]
    fn textfelder_und_dialoge_haben_vorrang() {
        let typing = KeyContext {
            text_focus: true,
            ..REVIEW
        };
        let dialog = KeyContext {
            dialog_open: true,
            ..REVIEW
        };
        for key in [
            Key::ArrowDown,
            Key::Space,
            Key::A,
            Key::Enter,
            Key::O,
            Key::Num1,
        ] {
            for ctrl in [false, true] {
                assert_eq!(action(key, ctrl, false, typing), None, "{key:?} {ctrl}");
                assert_eq!(action(key, ctrl, false, dialog), None, "{key:?} {ctrl}");
            }
        }
    }

    #[test]
    fn kein_kuerzel_loescht_oder_leert_die_quarantaene() {
        // Alle Tasten mit allen Modifikatoren durchprobieren: nur die dokumentierten Aktionen.
        for key in Key::ALL {
            for (ctrl, shift) in [(false, false), (true, false), (true, true), (false, true)] {
                if let Some(a) = action(*key, ctrl, shift, REVIEW) {
                    assert!(
                        matches!(
                            a,
                            KeyAction::Next
                                | KeyAction::Prev
                                | KeyAction::Toggle
                                | KeyAction::SelectFiltered
                                | KeyAction::DeselectFiltered
                                | KeyAction::FocusSearch
                                | KeyAction::Apply
                                | KeyAction::OpenPlan
                                | KeyAction::Reload
                                | KeyAction::Page(_)
                        ),
                        "{key:?}"
                    );
                }
            }
        }
        assert_eq!(HELP.len(), 12);
        assert!(
            HELP.iter().any(|(key, _)| *key == "F1"),
            "F1 fehlt in der Hilfeliste"
        );
    }
}
