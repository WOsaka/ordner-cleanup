//! Der kleine „?“-Knopf, der die Hilfe zu einem Thema öffnet.

use eframe::egui;

use crate::gui::help::{self, Topic};
use crate::gui::shell::Shell;

/// Zeichnet „?“ mit Tooltip; `true`, wenn geklickt wurde.
pub fn help_button(ui: &mut egui::Ui, topic: Topic) -> bool {
    finish(ui.add(egui::Button::new("?").small()), topic)
}

/// Wie [`help_button`], aber an einer festen Stelle über bereits gezeichnetem Inhalt (zum Beispiel
/// im Kopf einer Karte). Der Knopf belegt keinen Platz im Layout (ein `Ui::put` würde den Zeiger
/// eines umgebenden Grids verschieben) und liegt über allem, was vorher dort angelegt wurde,
/// sodass sein Klick zuerst ankommt.
pub fn help_button_at(ui: &mut egui::Ui, rect: egui::Rect, topic: Topic) -> bool {
    let id = ui.id().with(("help-button", topic_key(topic)));
    let response = ui.interact(rect, id, egui::Sense::click());
    let visuals = *ui.style().interact(&response);
    ui.painter().rect(
        rect,
        visuals.corner_radius,
        visuals.weak_bg_fill,
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "?",
        egui::TextStyle::Button.resolve(ui.style()),
        visuals.fg_stroke.color,
    );
    finish(response, topic)
}

fn topic_key(topic: Topic) -> String {
    format!("{topic:?}")
}

fn finish(response: egui::Response, topic: Topic) -> bool {
    let title = help::entry(topic).title;
    let response = response.on_hover_text(format!("Hilfe zu „{title}“ (F1)"));
    let label = format!("Hilfe zu {title}");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    response.clicked()
}

/// Seitenüberschrift mit „?“ daneben; ein Klick fragt die Hilfe über die [`Shell`] an.
pub fn heading_with_help(ui: &mut egui::Ui, text: &str, topic: Topic, shell: &mut Shell) {
    ui.horizontal(|ui| {
        ui.heading(text);
        if help_button(ui, topic) {
            shell.open_help(topic);
        }
    });
}
