//! Ansicht „Quarantäne“: Inhalt, Größe und Ablauf; einzelne Einträge zurückholen.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Utc};
use eframe::egui;
use egui_extras::{Column as TCol, TableBuilder};

use crate::change::restore::QuarantineItem;
use crate::change::RunId;
use crate::gui::format::{short_path, time_of, StatusStyle, Tone};
use crate::gui::help::Topic;
use crate::gui::shell::{Answer, Page, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::texts;
use crate::gui::theme;
use crate::gui::widgets::help_button::heading_with_help;
use crate::gui::widgets::table::{path_menu, status_cell, time_cell};
use crate::ops::quarantine::{quarantine_list, QuarantineList};
use crate::paths;

pub const SEARCH_ID: &str = "quarantine-search";

/// Eine Zeile der Tabelle: der Eintrag samt Wurzel und einmal berechneten Suchtexten.
#[derive(Debug, Clone)]
pub struct Row {
    pub root: PathBuf,
    root_key: String,
    pub item: QuarantineItem,
    /// Dateiname des Ursprungs
    pub name: String,
    name_lower: String,
    /// Ursprungspfad in Kleinbuchstaben (enthält den Namen)
    path_lower: String,
    /// Startzeit des Laufs in Sekunden, `i64::MIN` wenn unbekannt
    started: i64,
}

impl Row {
    pub fn new(root: PathBuf, item: QuarantineItem) -> Self {
        let name = Path::new(&item.origin)
            .file_name()
            .map_or_else(|| item.origin.clone(), |n| n.to_string_lossy().into_owned());
        let started = item
            .started
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map_or(i64::MIN, |t| t.timestamp());
        Self {
            root_key: paths::path_key(&root),
            root,
            name_lower: name.to_lowercase(),
            path_lower: item.origin.to_lowercase(),
            name,
            started,
            item,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Name,
    Size,
    Date,
    Expires,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub column: SortColumn,
    pub ascending: bool,
}

impl Default for Sort {
    /// Neueste zuerst.
    fn default() -> Self {
        Self {
            column: SortColumn::Date,
            ascending: false,
        }
    }
}

impl Sort {
    /// Klick auf eine Spaltenüberschrift: dieselbe Spalte kehrt um, eine andere beginnt
    /// aufsteigend (Größe und Datum absteigend, dort ist „groß/neu zuerst“ üblich).
    pub fn toggled(self, column: SortColumn) -> Self {
        if self.column == column {
            Self {
                ascending: !self.ascending,
                ..self
            }
        } else {
            Self {
                column,
                ascending: matches!(column, SortColumn::Name | SortColumn::Expires),
            }
        }
    }
}

/// Indizes der sichtbaren Zeilen in Anzeigereihenfolge. Die Suche ignoriert Groß- und
/// Kleinschreibung und sucht in Name und Pfad; gleiche Schlüssel behalten ihre Reihenfolge.
pub fn visible_rows(
    rows: &[Row],
    filter: &str,
    root: Option<&Path>,
    run: Option<&RunId>,
    sort: Sort,
) -> Vec<usize> {
    let needle = filter.trim().to_lowercase();
    let root_key = root.map(paths::path_key);
    let mut out: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            root_key.as_ref().is_none_or(|k| *k == r.root_key)
                && run.is_none_or(|run| *run == r.item.run)
                && (needle.is_empty() || r.path_lower.contains(&needle))
        })
        .map(|(i, _)| i)
        .collect();
    out.sort_by(|&a, &b| {
        let (a, b) = (&rows[a], &rows[b]);
        let order = match sort.column {
            SortColumn::Name => a.name_lower.cmp(&b.name_lower),
            SortColumn::Size => a.item.size.cmp(&b.item.size),
            SortColumn::Date => a.started.cmp(&b.started),
            SortColumn::Expires => a.item.expires.cmp(&b.item.expires),
        };
        if sort.ascending {
            order
        } else {
            order.reverse()
        }
    });
    out
}

/// Kennzahlen über die sichtbaren Zeilen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Totals {
    pub count: usize,
    pub bytes: u64,
    /// Früheste Ablaufzeit unter den Einträgen, die noch in der Quarantäne liegen
    pub next_expiry: Option<DateTime<Utc>>,
    pub missing: usize,
}

pub fn totals(rows: &[Row], visible: &[usize]) -> Totals {
    let shown = visible.iter().map(|&i| &rows[i]);
    Totals {
        count: visible.len(),
        bytes: shown.clone().map(|r| r.item.size).sum(),
        next_expiry: shown
            .clone()
            .filter(|r| r.item.present)
            .filter_map(|r| r.item.expires)
            .min(),
        missing: shown.filter(|r| !r.item.present).count(),
    }
}

/// „12 Einträge, 3,4 GB, nächster Ablauf in 5 Tagen“.
pub fn totals_text(totals: &Totals, now: DateTime<Utc>) -> String {
    let entries = if totals.count == 1 {
        "1 Eintrag".to_string()
    } else {
        format!("{} Einträge", texts::grouped(totals.count as u64))
    };
    let mut text = format!("{entries}, {}", texts::bytes(totals.bytes));
    if let Some(expiry) = totals.next_expiry {
        let seconds = (expiry - now).num_seconds();
        if seconds <= 0 {
            text.push_str(", nächster Ablauf: bereits abgelaufen");
        } else {
            let days = (seconds + 86_399) / 86_400;
            let unit = if days == 1 { "Tag" } else { "Tagen" };
            text.push_str(&format!(", nächster Ablauf in {days} {unit}"));
        }
    }
    text
}

pub fn item_status(item: &QuarantineItem) -> StatusStyle {
    let (symbol, text, tone) = match (item.present, item.cloud_only) {
        (false, _) => ("⚠", "fehlt", Tone::Warn),
        (true, true) => ("✔", "in Quarantäne · nur online", Tone::Ok),
        (true, false) => ("✔", "in Quarantäne", Tone::Ok),
    };
    StatusStyle {
        symbol,
        text: text.into(),
        tone,
    }
}

const TASK_LOAD: &str = "Quarantäne laden";
const ROW_H: f32 = 24.0;

/// Geladene Einträge samt Hinweisen.
#[derive(Default)]
struct Data {
    rows: Vec<Row>,
    roots: Vec<PathBuf>,
    unreadable: usize,
    unreachable: Vec<PathBuf>,
}

/// Womit `visible` zuletzt berechnet wurde; nur bei Änderung wird neu gefiltert und sortiert.
#[derive(PartialEq)]
struct VisibleKey {
    version: u64,
    filter: String,
    root: Option<PathBuf>,
    run: Option<RunId>,
    sort: Sort,
}

#[derive(Default)]
pub struct QuarantineView {
    data: Option<Data>,
    /// Zählt Ladevorgänge; ändert sich damit auch `VisibleKey`.
    version: u64,
    loaded_generation: Option<u64>,
    filter: String,
    root: Option<PathBuf>,
    run: Option<RunId>,
    sort: Sort,
    visible: Vec<usize>,
    visible_key: Option<VisibleKey>,
}

impl QuarantineView {
    /// Setzt die Liste (nach dem Laden); `generation` ist der Stand von `Shell::generation`.
    pub fn set_list(&mut self, list: QuarantineList, generation: u64) {
        let roots: Vec<PathBuf> = list.roots.iter().map(|r| r.root.clone()).collect();
        let rows = list
            .roots
            .into_iter()
            .flat_map(|r| {
                let root = r.root;
                r.items
                    .into_iter()
                    .map(move |item| Row::new(root.clone(), item))
            })
            .collect();
        if let Some(selected) = &self.root {
            let key = paths::path_key(selected);
            if !roots.iter().any(|r| paths::path_key(r) == key) {
                self.root = None;
            }
        }
        self.data = Some(Data {
            rows,
            roots,
            unreadable: list.unreadable,
            unreachable: list.unreachable,
        });
        self.version += 1;
        self.loaded_generation = Some(generation);
    }

    fn reload(&mut self, shell: &mut Shell) {
        self.loaded_generation = Some(shell.generation);
        shell.spawn(Route::Quarantine, TASK_LOAD, TaskKind::Read, |_| {
            quarantine_list(None)
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        heading_with_help(
            ui,
            texts::NAV_QUARANTINE,
            Topic::Page(Page::Quarantine),
            shell,
        );
        if self.loaded_generation != Some(shell.generation) && !shell.is_running(Route::Quarantine)
        {
            self.reload(shell);
        }
        let Some(data) = self.data.take() else {
            ui.label(texts::LOADING);
            return;
        };
        self.filter_bar(ui, &data);
        self.refresh_visible(&data);
        let now = Utc::now();
        ui.label(totals_text(&totals(&data.rows, &self.visible), now));
        self.notes(ui, &data);
        if data.rows.is_empty() {
            ui.add_space(crate::gui::theme::SPACE_M);
            ui.label(texts::QUARANTINE_EMPTY);
        } else if self.visible.is_empty() {
            ui.add_space(crate::gui::theme::SPACE_M);
            ui.label(texts::QUARANTINE_NO_MATCH);
        } else {
            self.table(ui, &data, shell);
        }
        self.data = Some(data);
    }

    fn filter_bar(&mut self, ui: &mut egui::Ui, data: &Data) {
        ui.horizontal(|ui| {
            ui.label(texts::QUARANTINE_ROOT);
            egui::ComboBox::from_id_salt("quarantine-root")
                .selected_text(
                    self.root
                        .as_ref()
                        .map_or(texts::ALL.to_string(), |p| paths::display(p)),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.root, None, texts::ALL);
                    for root in &data.roots {
                        ui.selectable_value(
                            &mut self.root,
                            Some(root.clone()),
                            paths::display(root),
                        );
                    }
                });
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .id(egui::Id::new(SEARCH_ID))
                    .hint_text(texts::QUARANTINE_SEARCH)
                    .desired_width(240.0),
            );
            if let Some(run) = self.run.clone() {
                if ui
                    .button(texts::quarantine_only_run(run.as_str()))
                    .on_hover_text(texts::QUARANTINE_REMOVE_RUN_FILTER)
                    .clicked()
                {
                    self.run = None;
                }
            }
        });
    }

    fn refresh_visible(&mut self, data: &Data) {
        let key = VisibleKey {
            version: self.version,
            filter: self.filter.clone(),
            root: self.root.clone(),
            run: self.run.clone(),
            sort: self.sort,
        };
        if self.visible_key.as_ref() != Some(&key) {
            self.visible = visible_rows(
                &data.rows,
                &self.filter,
                self.root.as_deref(),
                self.run.as_ref(),
                self.sort,
            );
            self.visible_key = Some(key);
        }
    }

    fn notes(&self, ui: &mut egui::Ui, data: &Data) {
        let missing = totals(&data.rows, &self.visible).missing;
        if missing > 0 {
            ui.colored_label(
                theme::tone_color(ui, Tone::Warn),
                texts::quarantine_missing(missing),
            );
        }
        if data.unreadable > 0 {
            ui.colored_label(
                theme::tone_color(ui, Tone::Warn),
                texts::quarantine_unreadable(data.unreadable),
            );
        }
        for root in &data.unreachable {
            ui.colored_label(
                theme::tone_color(ui, Tone::Muted),
                texts::quarantine_unreachable(&paths::display(root)),
            );
        }
    }

    fn table(&mut self, ui: &mut egui::Ui, data: &Data, shell: &mut Shell) {
        let avail = ui.available_height();
        let flex = (ui.available_width() - 4.0 * 110.0 - 150.0).max(240.0);
        let now = Local::now();
        let mut header_click: Option<SortColumn> = None;
        let sort = self.sort;
        let visible = &self.visible;
        let rows = &data.rows;
        TableBuilder::new(ui)
            .id_salt("quarantine-table-v1")
            .striped(true)
            .max_scroll_height(avail)
            .column(TCol::exact(flex * 0.4))
            .column(TCol::exact(flex * 0.6))
            .column(TCol::exact(90.0))
            .column(TCol::exact(130.0))
            .column(TCol::exact(100.0))
            .column(TCol::remainder().clip(true))
            .header(ROW_H + 4.0, |mut h| {
                for (title, column) in [
                    (texts::COL_NAME, Some(SortColumn::Name)),
                    (texts::COL_ORIGIN, None),
                    (texts::COL_SIZE, Some(SortColumn::Size)),
                    (texts::COL_DATE, Some(SortColumn::Date)),
                    (texts::COL_EXPIRES, Some(SortColumn::Expires)),
                    (texts::COL_STATUS, None),
                ] {
                    h.col(|ui| {
                        let mark = match column {
                            Some(c) if c == sort.column => {
                                if sort.ascending {
                                    " ▲"
                                } else {
                                    " ▼"
                                }
                            }
                            _ => "",
                        };
                        let text = egui::RichText::new(format!("{title}{mark}")).strong();
                        match column {
                            Some(column) => {
                                if ui.add(egui::Button::new(text).frame(false)).clicked() {
                                    header_click = Some(column);
                                }
                            }
                            None => {
                                ui.label(text);
                            }
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_H, visible.len(), |mut row| {
                    let r = &rows[visible[row.index()]];
                    let item = &r.item;
                    row.col(|ui| {
                        let name = if item.is_dir {
                            texts::quarantine_folder_name(&r.name)
                        } else {
                            r.name.clone()
                        };
                        let response = ui.add(egui::Label::new(name).truncate());
                        if let Some(count) = item.file_count {
                            response.on_hover_text(texts::quarantine_file_count(count));
                        }
                    });
                    row.col(|ui| {
                        let response = ui.add(
                            egui::Label::new(short_path(&item.origin, 60))
                                .truncate()
                                .sense(egui::Sense::click()),
                        );
                        response.clone().on_hover_text(&item.origin);
                        path_menu(&response, &item.stored, shell);
                    });
                    row.col(|ui| {
                        ui.label(texts::bytes(item.size));
                    });
                    row.col(|ui| {
                        let (text, tip) = time_of(now, item.started.as_deref());
                        time_cell(ui, &(text, format!("{tip}\nLauf {}", item.run)));
                    });
                    row.col(|ui| {
                        ui.label(
                            item.expires
                                .map(|e| e.with_timezone(&Local).format("%d.%m.%Y").to_string())
                                .unwrap_or_default(),
                        );
                    });
                    row.col(|ui| status_cell(ui, &item_status(item)));
                });
            });
        if let Some(column) = header_click {
            self.sort = self.sort.toggled(column);
        }
    }

    pub fn on_answer(&mut self, _id: &str, _answer: Answer, _shell: &mut Shell) {}

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        if name == TASK_LOAD {
            match result.map(|b| b.downcast::<QuarantineList>()) {
                Ok(Ok(list)) => self.set_list(*list, shell.generation),
                Ok(Err(_)) => {}
                Err(e) => shell.show_error(name, &e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const RUN_A: &str = "20261003-120000-ab12";
    const RUN_B: &str = "20261004-120000-cd34";

    fn item(run: &str, action: u32, origin: &str, size: u64, started: &str) -> QuarantineItem {
        QuarantineItem {
            run: RunId::parse(run).unwrap(),
            action,
            origin: origin.into(),
            stored: format!(r"D:\.ordner-cleanup\quarantine\{run}\{action}"),
            size,
            hash: None,
            is_dir: false,
            file_count: None,
            started: Some(started.into()),
            expires: DateTime::parse_from_rfc3339(started)
                .ok()
                .map(|t| t.with_timezone(&Utc) + chrono::Duration::days(30)),
            present: true,
            cloud_only: false,
        }
    }

    fn rows() -> Vec<Row> {
        let a = r"D:\Daten";
        let b = r"E:\Fotos";
        vec![
            Row::new(
                a.into(),
                item(
                    RUN_A,
                    1,
                    r"D:\Daten\Berichte\Zeta.TXT",
                    300,
                    "2026-10-03T12:00:00Z",
                ),
            ),
            Row::new(
                a.into(),
                item(RUN_A, 2, r"D:\Daten\alpha.txt", 100, "2026-10-03T12:00:00Z"),
            ),
            Row::new(
                b.into(),
                item(
                    RUN_B,
                    3,
                    r"E:\Fotos\Urlaub\bild.jpg",
                    200,
                    "2026-10-04T12:00:00Z",
                ),
            ),
            Row::new(
                b.into(),
                item(
                    RUN_B,
                    4,
                    r"E:\Fotos\Urlaub\Alpha2.jpg",
                    200,
                    "2026-10-04T12:00:00Z",
                ),
            ),
        ]
    }

    fn actions(rows: &[Row], visible: &[usize]) -> Vec<u32> {
        visible.iter().map(|&i| rows[i].item.action).collect()
    }

    fn by(column: SortColumn, ascending: bool) -> Sort {
        Sort { column, ascending }
    }

    #[test]
    fn standard_ist_neueste_zuerst_und_stabil() {
        let rows = rows();
        let v = visible_rows(&rows, "", None, None, Sort::default());
        assert_eq!(actions(&rows, &v), [3, 4, 1, 2]);
    }

    #[test]
    fn suche_ignoriert_gross_und_kleinschreibung_und_findet_pfadteile() {
        let rows = rows();
        let find = |text: &str| {
            actions(
                &rows,
                &visible_rows(&rows, text, None, None, by(SortColumn::Name, true)),
            )
        };
        assert_eq!(find("ALPHA"), [2, 4]);
        assert_eq!(find("zeta.txt"), [1]);
        assert_eq!(find("urlaub"), [4, 3]);
        assert_eq!(find("berichte"), [1], "Treffer im Pfad, nicht nur im Namen");
        assert_eq!(find("gibt es nicht"), Vec::<u32>::new());
    }

    #[test]
    fn filter_nach_wurzel_und_lauf() {
        let rows = rows();
        let only_root = visible_rows(
            &rows,
            "",
            Some(Path::new(r"E:\Fotos")),
            None,
            Sort::default(),
        );
        assert_eq!(actions(&rows, &only_root), [3, 4]);
        let run = RunId::parse(RUN_A).unwrap();
        let only_run = visible_rows(&rows, "", None, Some(&run), Sort::default());
        assert_eq!(actions(&rows, &only_run), [1, 2]);
        let both = visible_rows(
            &rows,
            "",
            Some(Path::new(r"D:\Daten")),
            Some(&run),
            Sort::default(),
        );
        assert_eq!(actions(&rows, &both), [1, 2]);
        let none = visible_rows(
            &rows,
            "",
            Some(Path::new(r"E:\Fotos")),
            Some(&run),
            Sort::default(),
        );
        assert!(none.is_empty());
    }

    #[test]
    fn wurzelvergleich_ignoriert_schreibweise() {
        let rows = rows();
        let v = visible_rows(
            &rows,
            "",
            Some(Path::new(r"e:\fotos")),
            None,
            Sort::default(),
        );
        assert_eq!(actions(&rows, &v), [3, 4]);
    }

    #[test]
    fn sortierung_je_spalte_auf_und_absteigend() {
        let rows = rows();
        let sorted = |s| actions(&rows, &visible_rows(&rows, "", None, None, s));
        assert_eq!(sorted(by(SortColumn::Name, true)), [2, 4, 3, 1]);
        assert_eq!(sorted(by(SortColumn::Name, false)), [1, 3, 4, 2]);
        assert_eq!(sorted(by(SortColumn::Size, true)), [2, 3, 4, 1]);
        assert_eq!(
            sorted(by(SortColumn::Size, false)),
            [1, 3, 4, 2],
            "gleiche Größe bleibt in Eingangsreihenfolge"
        );
        assert_eq!(sorted(by(SortColumn::Date, true)), [1, 2, 3, 4]);
        assert_eq!(sorted(by(SortColumn::Expires, true)), [1, 2, 3, 4]);
        assert_eq!(sorted(by(SortColumn::Expires, false)), [3, 4, 1, 2]);
    }

    #[test]
    fn klick_auf_kopfzeile_kehrt_um_oder_wechselt_die_spalte() {
        let s = Sort::default().toggled(SortColumn::Date);
        assert_eq!(s, by(SortColumn::Date, true));
        assert_eq!(s.toggled(SortColumn::Name), by(SortColumn::Name, true));
        assert_eq!(s.toggled(SortColumn::Size), by(SortColumn::Size, false));
    }

    #[test]
    fn summen_zaehlen_nur_sichtbare_zeilen() {
        let mut rows = rows();
        rows[1].item.present = false;
        let all: Vec<usize> = (0..rows.len()).collect();
        let t = totals(&rows, &all);
        assert_eq!((t.count, t.bytes, t.missing), (4, 800, 1));
        // Der früheste Ablauf stammt von einem Eintrag, der noch da ist (Lauf A, Aktion 1).
        assert_eq!(
            t.next_expiry,
            Some(Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap())
        );
        let part = totals(&rows, &[2, 3]);
        assert_eq!((part.count, part.bytes, part.missing), (2, 400, 0));
        assert_eq!(totals(&rows, &[]).next_expiry, None);
    }

    #[test]
    fn summentext_nennt_anzahl_groesse_und_frist() {
        let now = Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap();
        let t = Totals {
            count: 12,
            bytes: 3_400_000_000,
            next_expiry: Some(now + chrono::Duration::days(5)),
            missing: 0,
        };
        let text = totals_text(&t, now);
        assert!(text.starts_with("12 Einträge, "), "{text}");
        assert!(text.contains("GiB"), "{text}");
        assert!(text.ends_with("nächster Ablauf in 5 Tagen"), "{text}");
        let one = Totals {
            count: 1,
            next_expiry: Some(now + chrono::Duration::days(1)),
            ..t.clone()
        };
        assert!(totals_text(&one, now).starts_with("1 Eintrag, "));
        assert!(totals_text(&one, now).ends_with("in 1 Tag"));
        let late = Totals {
            next_expiry: Some(now - chrono::Duration::days(2)),
            ..t.clone()
        };
        assert!(totals_text(&late, now).ends_with("bereits abgelaufen"));
        let none = Totals {
            next_expiry: None,
            ..t
        };
        assert!(!totals_text(&none, now).contains("Ablauf"));
    }

    #[test]
    fn status_unterscheidet_vorhanden_fehlend_und_nur_online() {
        let mut it = rows()[0].item.clone();
        let ok = item_status(&it);
        assert_eq!((ok.text.as_str(), ok.tone), ("in Quarantäne", Tone::Ok));
        it.cloud_only = true;
        assert_eq!(item_status(&it).text, "in Quarantäne · nur online");
        it.present = false;
        let gone = item_status(&it);
        assert_eq!((gone.text.as_str(), gone.tone), ("fehlt", Tone::Warn));
    }
}
