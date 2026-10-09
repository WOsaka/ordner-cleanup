//! Ansicht „Quarantäne“: Inhalt, Größe und Ablauf; einzelne Einträge zurückholen.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Utc};
use eframe::egui;
use egui_extras::{Column as TCol, TableBuilder};

use crate::change::restore::{HashCheck, ItemOutcome, ItemResult, QuarantineItem, RestorePreview};
use crate::change::RunId;
use crate::gui::format::{short_path, time_of, StatusStyle, Tone};
use crate::gui::help::Topic;
use crate::gui::shell::{Answer, Dialog, Page, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::texts;
use crate::gui::theme;
use crate::gui::widgets::help_button::heading_with_help;
use crate::gui::widgets::table::{path_menu, status_cell, time_cell};
use crate::ops::quarantine::{
    quarantine_list, restore_check, restore_execute, QuarantineList, Selected,
};
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

/// Eine ausgewählte Zeile, wie sie den Rückfragen und der Vorschau dient.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingItem {
    selected: Selected,
    origin: String,
    size: u64,
}

/// Eine Zeile der Rückfrage: woher, wohin, ob umbenannt, Ergebnis der Hash-Prüfung.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewLine {
    pub origin: String,
    pub target: String,
    pub renamed: bool,
    pub hash: HashCheck,
}

/// Die Vorschau zu einem ausgewählten Eintrag (Wurzel wie im Index verglichen).
fn find_preview<'a>(
    item: &PendingItem,
    previews: &'a [(PathBuf, RestorePreview)],
) -> Option<&'a RestorePreview> {
    let key = paths::path_key(&item.selected.root);
    previews
        .iter()
        .find(|(root, p)| {
            p.run == item.selected.run
                && p.action == item.selected.action
                && paths::path_key(root) == key
        })
        .map(|(_, p)| p)
}

/// Ordnet die Vorschau der Auswahl zu (in der Reihenfolge der Auswahl). Einträge ohne Vorschau
/// (inzwischen nicht mehr zurückholbar) fehlen.
fn preview_lines(
    items: &[PendingItem],
    previews: &[(PathBuf, RestorePreview)],
) -> Vec<PreviewLine> {
    items
        .iter()
        .filter_map(|item| {
            find_preview(item, previews).map(|p| PreviewLine {
                origin: item.origin.clone(),
                target: paths::display(&p.target),
                renamed: p.renamed,
                hash: p.hash,
            })
        })
        .collect()
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

fn entries_text(count: usize) -> String {
    match count {
        1 => "1 Eintrag".to_string(),
        n => format!("{} Einträge", texts::grouped(n as u64)),
    }
}

/// Wie viele Ziele die Rückfrage einzeln aufzählt.
const SHOWN_TARGETS: usize = 15;
/// Wie viele Fehler- bzw. Fehlt-Zeilen die Ergebnismeldung aufzählt.
const SHOWN_PROBLEMS: usize = 10;

/// Text der Rückfrage vor dem Zurückholen.
fn restore_confirm_text(count: usize, bytes: u64, lines: &[PreviewLine]) -> String {
    let mut text = format!(
        "{} ({}) zurückholen?\n\nZiele:",
        entries_text(count),
        texts::bytes(bytes)
    );
    for line in lines.iter().take(SHOWN_TARGETS) {
        if line.renamed {
            text.push_str(&format!(
                "\n  {}  →  {}",
                line.origin,
                file_name(&line.target)
            ));
        } else {
            text.push_str(&format!("\n  {}", line.target));
        }
    }
    if lines.len() > SHOWN_TARGETS {
        text.push_str(&format!(
            "\n  … und {} weitere",
            lines.len() - SHOWN_TARGETS
        ));
    }
    let with = |check| lines.iter().filter(|l| l.hash == check).count();
    let (changed, cloud) = (with(HashCheck::Mismatch), with(HashCheck::CloudOnly));
    if changed > 0 {
        text.push_str(&format!(
            "\n\nWarnung: {changed} Datei(en) wurden in der Quarantäne verändert (anderer \
             Inhalt als beim Verschieben). Sie werden trotzdem zurückgeholt."
        ));
    }
    if cloud > 0 {
        text.push_str(&format!(
            "\n\nHinweis: {cloud} Datei(en) liegen nur in der Cloud und werden weder \
             heruntergeladen noch geprüft."
        ));
    }
    text.push_str(
        "\n\nBelegte Namen werden nie überschrieben; die Datei bekommt dann einen neuen Namen.",
    );
    text
}

/// Text der Meldung nach dem Zurückholen. `requested` ist die Zahl der gewählten Einträge;
/// weniger Ergebnisse heißen: abgebrochen.
fn restore_result_text(requested: usize, results: &[(PathBuf, RunId, ItemResult)]) -> String {
    let outcomes = || results.iter().map(|(_, _, r)| r);
    let count = |f: &dyn Fn(&ItemOutcome) -> bool| outcomes().filter(|r| f(&r.outcome)).count();
    let restored = count(&|o| matches!(o, ItemOutcome::Restored { .. }));
    let renamed = count(&|o| matches!(o, ItemOutcome::Restored { renamed: true, .. }));
    let missing = count(&|o| matches!(o, ItemOutcome::Missing));
    let failed = count(&|o| matches!(o, ItemOutcome::Failed(_)));
    let already = count(&|o| matches!(o, ItemOutcome::AlreadyRestored));
    let mut text = format!(
        "{restored} zurückgeholt, davon {renamed} umbenannt, {missing} fehlen, {failed} Fehler"
    );
    if already > 0 {
        text.push_str(&format!(", {already} schon zurück"));
    }
    if results.len() < requested {
        text.push_str(&format!(
            "\nAbgebrochen: {} von {requested} bearbeitet.",
            results.len()
        ));
    }
    for r in outcomes().take(usize::MAX) {
        if let ItemOutcome::Restored { to, renamed: true } = &r.outcome {
            text.push_str(&format!("\nUmbenannt: {}  →  {}", r.origin, file_name(to)));
        }
    }
    let problems = |keep: fn(&ItemOutcome) -> Option<String>| -> Vec<String> {
        outcomes()
            .filter_map(|r| keep(&r.outcome).map(|prefix| format!("\n{prefix}: {}", r.origin)))
            .collect()
    };
    let errors = problems(|o| match o {
        ItemOutcome::Failed(e) => Some(format!("Fehler ({e})")),
        _ => None,
    });
    for line in errors.iter().take(SHOWN_PROBLEMS) {
        text.push_str(line);
    }
    if errors.len() > SHOWN_PROBLEMS {
        text.push_str(&format!(
            "\n… und {} weitere",
            errors.len() - SHOWN_PROBLEMS
        ));
    }
    let gone = problems(|o| matches!(o, ItemOutcome::Missing).then(|| "Fehlt".to_string()));
    for line in gone.iter().take(SHOWN_PROBLEMS) {
        text.push_str(line);
    }
    text
}

const TASK_LOAD: &str = "Quarantäne laden";
const TASK_CHECK: &str = "Zurückholen prüfen";
const TASK_RESTORE: &str = "Zurückholen";
const ID_RESTORE: &str = "quarantine.restore";
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
    selected: HashSet<SelKey>,
    pending: Option<Vec<PendingItem>>,
}

/// Identität eines Eintrags über Neuladen hinweg.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SelKey {
    root_key: String,
    run: RunId,
    action: u32,
}

impl SelKey {
    fn of(row: &Row) -> Self {
        Self {
            root_key: row.root_key.clone(),
            run: row.item.run.clone(),
            action: row.item.action,
        }
    }
}

impl QuarantineView {
    /// Wählt alle sichtbaren Zeilen, die noch in der Quarantäne liegen.
    fn select_filtered(&mut self, data: &Data) {
        for &i in &self.visible {
            let row = &data.rows[i];
            if row.item.present {
                self.selected.insert(SelKey::of(row));
            }
        }
    }

    fn clear_selection(&mut self) {
        self.selected.clear();
    }

    /// Die ausgewählten Zeilen in Anzeigereihenfolge.
    fn pending_items(&self, data: &Data) -> Vec<PendingItem> {
        self.visible
            .iter()
            .map(|&i| &data.rows[i])
            .filter(|r| self.selected.contains(&SelKey::of(r)))
            .map(|r| PendingItem {
                selected: Selected {
                    root: r.root.clone(),
                    run: r.item.run.clone(),
                    action: r.item.action,
                },
                origin: r.item.origin.clone(),
                size: r.item.size,
            })
            .collect()
    }

    /// „Zurückholen …“: erst prüfen (Ziele, Hash), dann fragen.
    fn begin_restore(&mut self, data: &Data, shell: &mut Shell) {
        let items = self.pending_items(data);
        if items.is_empty() {
            return;
        }
        let selection: Vec<Selected> = items.iter().map(|i| i.selected.clone()).collect();
        self.pending = Some(items);
        shell.spawn(Route::Quarantine, TASK_CHECK, TaskKind::Read, move |_| {
            restore_check(&selection)
        });
    }

    fn on_checked(&mut self, result: TaskResult, shell: &mut Shell) {
        let previews = match result.map(|b| b.downcast::<Vec<(PathBuf, RestorePreview)>>()) {
            Ok(Ok(previews)) => *previews,
            Ok(Err(_)) => return,
            Err(e) => {
                self.pending = None;
                return shell.show_error(TASK_CHECK, &e);
            }
        };
        let Some(items) = self.pending.take() else {
            return;
        };
        let items: Vec<PendingItem> = items
            .into_iter()
            .filter(|i| find_preview(i, &previews).is_some())
            .collect();
        if items.is_empty() {
            return shell.message(TASK_RESTORE, texts::QUARANTINE_NOTHING_TO_RESTORE);
        }
        let lines = preview_lines(&items, &previews);
        let bytes = items.iter().map(|i| i.size).sum();
        let text = restore_confirm_text(items.len(), bytes, &lines);
        self.pending = Some(items);
        shell.confirm(Dialog::Confirm {
            id: ID_RESTORE,
            title: texts::QUARANTINE_RESTORE_TITLE.into(),
            text,
            ok_label: texts::QUARANTINE_RESTORE_OK.into(),
            must_check: None,
            checked: false,
            danger: false,
        });
    }

    fn on_restored(&mut self, result: TaskResult, shell: &mut Shell) {
        match result.map(|b| b.downcast::<(usize, Vec<(PathBuf, RunId, ItemResult)>)>()) {
            Ok(Ok(done)) => {
                let (requested, results) = *done;
                shell.message(TASK_RESTORE, &restore_result_text(requested, &results));
                self.selected.clear();
                shell.generation += 1;
            }
            Ok(Err(_)) => {}
            Err(e) => shell.show_error(TASK_RESTORE, &e),
        }
    }

    /// Zeigt nur die Einträge eines Laufs (Sprung aus dem Verlauf).
    pub fn show_run(&mut self, root: PathBuf, run: RunId) {
        self.root = Some(root);
        self.run = Some(run);
        self.filter.clear();
    }

    /// Setzt die Liste (nach dem Laden); `generation` ist der Stand von `Shell::generation`.
    pub fn set_list(&mut self, list: QuarantineList, generation: u64) {
        let roots: Vec<PathBuf> = list.roots.iter().map(|r| r.root.clone()).collect();
        let rows: Vec<Row> = list
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
        if !self.selected.is_empty() {
            let present: HashSet<SelKey> = rows
                .iter()
                .filter(|r: &&Row| r.item.present)
                .map(SelKey::of)
                .collect();
            self.selected.retain(|k| present.contains(k));
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
        if !data.rows.is_empty() {
            self.toolbar(ui, &data, shell);
        }
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

    fn toolbar(&mut self, ui: &mut egui::Ui, data: &Data, shell: &mut Shell) {
        ui.horizontal(|ui| {
            if ui.button(texts::QUARANTINE_SELECT_FILTERED).clicked() {
                self.select_filtered(data);
            }
            let any = !self.selected.is_empty();
            if ui
                .add_enabled(any, egui::Button::new(texts::QUARANTINE_CLEAR_SELECTION))
                .clicked()
            {
                self.clear_selection();
            }
            // Gesperrt, solange irgendein Schreib-Task läuft (Apply, Undo, Purge, Scan …).
            let busy = shell.is_running(Route::Quarantine)
                || shell
                    .runner
                    .running()
                    .iter()
                    .any(|r| r.kind == TaskKind::Write);
            let restore = ui.add_enabled(
                any && !busy,
                egui::Button::new(texts::quarantine_restore_button(self.selected.len())),
            );
            if restore.clicked() {
                self.begin_restore(data, shell);
            }
        });
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
            if !self.selected.is_empty() {
                let shown: HashSet<SelKey> = self
                    .visible
                    .iter()
                    .map(|&i| SelKey::of(&data.rows[i]))
                    .collect();
                self.selected.retain(|k| shown.contains(k));
            }
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
        let flex = (ui.available_width() - 26.0 - 4.0 * 110.0 - 150.0).max(240.0);
        let mut toggled: Option<SelKey> = None;
        let selected = &self.selected;
        let now = Local::now();
        let mut header_click: Option<SortColumn> = None;
        let sort = self.sort;
        let visible = &self.visible;
        let rows = &data.rows;
        TableBuilder::new(ui)
            .id_salt("quarantine-table-v1")
            .striped(true)
            .max_scroll_height(avail)
            .column(TCol::exact(26.0))
            .column(TCol::exact(flex * 0.4))
            .column(TCol::exact(flex * 0.6))
            .column(TCol::exact(90.0))
            .column(TCol::exact(130.0))
            .column(TCol::exact(100.0))
            .column(TCol::remainder().clip(true))
            .header(ROW_H + 4.0, |mut h| {
                h.col(|_| {});
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
                        let mut on = selected.contains(&SelKey::of(r));
                        let changed = ui
                            .add_enabled(item.present, egui::Checkbox::new(&mut on, ""))
                            .changed();
                        if changed {
                            toggled = Some(SelKey::of(r));
                        }
                    });
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
        if let Some(key) = toggled {
            if !self.selected.remove(&key) {
                self.selected.insert(key);
            }
        }
    }

    pub fn on_answer(&mut self, id: &str, answer: Answer, shell: &mut Shell) {
        if id != ID_RESTORE {
            return;
        }
        let Some(items) = self.pending.take() else {
            return;
        };
        if !answer.ok {
            return;
        }
        let selection: Vec<Selected> = items.into_iter().map(|i| i.selected).collect();
        shell.spawn(
            Route::Quarantine,
            TASK_RESTORE,
            TaskKind::Write,
            move |ctx| {
                let requested = selection.len();
                let results = restore_execute(&selection, ctx)?;
                Ok((requested, results))
            },
        );
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        match name {
            TASK_LOAD => match result.map(|b| b.downcast::<QuarantineList>()) {
                Ok(Ok(list)) => self.set_list(*list, shell.generation),
                Ok(Err(_)) => {}
                Err(e) => shell.show_error(name, &e),
            },
            TASK_CHECK => self.on_checked(result, shell),
            TASK_RESTORE => self.on_restored(result, shell),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::quarantine::RootItems;
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

    fn view_with(rows: Vec<Row>) -> (QuarantineView, Data) {
        let mut view = QuarantineView::default();
        let data = Data {
            roots: vec![PathBuf::from(r"D:\Daten"), PathBuf::from(r"E:\Fotos")],
            rows,
            ..Data::default()
        };
        view.refresh_visible(&data);
        (view, data)
    }

    #[test]
    fn alle_gefilterten_waehlen_nimmt_nur_vorhandene_sichtbare_zeilen() {
        let mut rows = rows();
        rows[1].item.present = false;
        let (mut view, data) = view_with(rows);
        view.filter = "alpha".into();
        view.refresh_visible(&data);
        view.select_filtered(&data);
        // „alpha.txt“ fehlt in der Quarantäne, „Alpha2.jpg“ ist da.
        let picked = view.pending_items(&data);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].selected.action, 4);
        assert_eq!(picked[0].selected.root, PathBuf::from(r"E:\Fotos"));
        assert_eq!(picked[0].origin, r"E:\Fotos\Urlaub\Alpha2.jpg");
        assert_eq!(picked[0].size, 200);
        view.clear_selection();
        assert!(view.pending_items(&data).is_empty());
        assert_eq!(view.selected.len(), 0);
    }

    #[test]
    fn auswahl_folgt_der_anzeigereihenfolge() {
        let (mut view, data) = view_with(rows());
        view.select_filtered(&data);
        let actions: Vec<u32> = view
            .pending_items(&data)
            .iter()
            .map(|p| p.selected.action)
            .collect();
        assert_eq!(actions, [3, 4, 1, 2], "Standard: neueste zuerst");
    }

    #[test]
    fn auswahl_ueberlebt_neuladen_und_verliert_verschwundene_eintraege() {
        let (mut view, data) = view_with(rows());
        view.select_filtered(&data);
        assert_eq!(view.selected.len(), 4);
        // Nach dem Neuladen fehlt Aktion 2.
        let remaining: Vec<Row> = rows().into_iter().filter(|r| r.item.action != 2).collect();
        let list = QuarantineList {
            roots: vec![
                RootItems {
                    root: PathBuf::from(r"D:\Daten"),
                    items: remaining
                        .iter()
                        .filter(|r| r.root == Path::new(r"D:\Daten"))
                        .map(|r| r.item.clone())
                        .collect(),
                },
                RootItems {
                    root: PathBuf::from(r"E:\Fotos"),
                    items: remaining
                        .iter()
                        .filter(|r| r.root == Path::new(r"E:\Fotos"))
                        .map(|r| r.item.clone())
                        .collect(),
                },
            ],
            unreadable: 0,
            unreachable: vec![],
        };
        view.set_list(list, 1);
        assert_eq!(view.selected.len(), 3);
    }

    #[test]
    fn sprung_aus_dem_verlauf_filtert_auf_wurzel_und_lauf() {
        let (mut view, data) = view_with(rows());
        view.filter = "alpha".into();
        view.show_run(PathBuf::from(r"D:\Daten"), RunId::parse(RUN_A).unwrap());
        assert_eq!(
            view.filter, "",
            "ein alter Suchtext würde den Lauf verdecken"
        );
        view.refresh_visible(&data);
        let actions: Vec<u32> = view
            .visible
            .iter()
            .map(|&i| data.rows[i].item.action)
            .collect();
        assert_eq!(actions, [1, 2]);
    }

    fn line(origin: &str, target: &str, hash: HashCheck) -> PreviewLine {
        PreviewLine {
            origin: origin.into(),
            target: target.into(),
            renamed: origin != target,
            hash,
        }
    }

    #[test]
    fn vorschau_wird_der_auswahl_zugeordnet() {
        let items = vec![
            PendingItem {
                selected: Selected {
                    root: PathBuf::from(r"D:\Daten"),
                    run: RunId::parse(RUN_A).unwrap(),
                    action: 2,
                },
                origin: r"D:\Daten\alpha.txt".into(),
                size: 100,
            },
            PendingItem {
                selected: Selected {
                    root: PathBuf::from(r"D:\Daten"),
                    run: RunId::parse(RUN_A).unwrap(),
                    action: 1,
                },
                origin: r"D:\Daten\Zeta.TXT".into(),
                size: 300,
            },
        ];
        let previews = vec![
            (
                PathBuf::from(r"d:\daten"),
                RestorePreview {
                    run: RunId::parse(RUN_A).unwrap(),
                    action: 1,
                    target: PathBuf::from(r"D:\Daten\Zeta (2).TXT"),
                    renamed: true,
                    hash: HashCheck::Mismatch,
                },
            ),
            (
                PathBuf::from(r"D:\Daten"),
                RestorePreview {
                    run: RunId::parse(RUN_A).unwrap(),
                    action: 2,
                    target: PathBuf::from(r"D:\Daten\alpha.txt"),
                    renamed: false,
                    hash: HashCheck::Match,
                },
            ),
        ];
        let lines = preview_lines(&items, &previews);
        assert_eq!(
            lines,
            [
                line(
                    r"D:\Daten\alpha.txt",
                    r"D:\Daten\alpha.txt",
                    HashCheck::Match
                ),
                line(
                    r"D:\Daten\Zeta.TXT",
                    r"D:\Daten\Zeta (2).TXT",
                    HashCheck::Mismatch
                ),
            ]
        );
    }

    #[test]
    fn rueckfrage_nennt_anzahl_ziele_umbenennung_und_warnungen() {
        let mut lines: Vec<PreviewLine> = (0..20)
            .map(|i| {
                let path = format!(r"D:\Daten\datei{i}.txt");
                line(&path, &path, HashCheck::NotApplicable)
            })
            .collect();
        lines[0] = line(
            r"D:\Daten\kopie.txt",
            r"D:\Daten\kopie (2).txt",
            HashCheck::Mismatch,
        );
        lines[1].hash = HashCheck::CloudOnly;
        let text = restore_confirm_text(20, 20_480, &lines);
        assert!(
            text.starts_with(&format!("20 Einträge ({})", texts::bytes(20_480))),
            "{text}"
        );
        assert!(
            text.contains(r"D:\Daten\kopie.txt  →  kopie (2).txt"),
            "{text}"
        );
        assert!(text.contains(r"D:\Daten\datei14.txt"), "{text}");
        assert!(!text.contains(r"D:\Daten\datei15.txt"), "{text}");
        assert!(text.contains("und 5 weitere"), "{text}");
        assert!(
            text.contains("Warnung") && text.contains("verändert"),
            "{text}"
        );
        assert!(
            text.contains("Cloud") && text.contains("weder heruntergeladen noch geprüft"),
            "{text}"
        );
        assert!(text.contains("nie überschrieben"), "{text}");
        let one = restore_confirm_text(1, 5, &lines[2..3]);
        assert!(
            one.starts_with(&format!("1 Eintrag ({})", texts::bytes(5))),
            "{one}"
        );
        assert!(!one.contains("Warnung") && !one.contains("Cloud") && !one.contains("weitere"));
    }

    fn result(action: u32, origin: &str, outcome: ItemOutcome) -> (PathBuf, RunId, ItemResult) {
        (
            PathBuf::from(r"D:\Daten"),
            RunId::parse(RUN_A).unwrap(),
            ItemResult {
                action,
                origin: origin.into(),
                outcome,
            },
        )
    }

    #[test]
    fn ergebnistext_zaehlt_und_nennt_probleme() {
        let results = vec![
            result(
                1,
                r"D:\Daten\a.txt",
                ItemOutcome::Restored {
                    to: r"D:\Daten\a.txt".into(),
                    renamed: false,
                },
            ),
            result(
                2,
                r"D:\Daten\b.txt",
                ItemOutcome::Restored {
                    to: r"D:\Daten\b (2).txt".into(),
                    renamed: true,
                },
            ),
            result(3, r"D:\Daten\c.txt", ItemOutcome::Missing),
            result(4, r"D:\Daten\d.txt", ItemOutcome::Failed("gesperrt".into())),
            result(5, r"D:\Daten\e.txt", ItemOutcome::AlreadyRestored),
        ];
        let text = restore_result_text(5, &results);
        assert!(
            text.starts_with("2 zurückgeholt, davon 1 umbenannt, 1 fehlen, 1 Fehler"),
            "{text}"
        );
        assert!(text.contains("1 schon zurück"), "{text}");
        assert!(text.contains(r"D:\Daten\b.txt  →  b (2).txt"), "{text}");
        assert!(
            text.contains(r"Fehler (gesperrt): D:\Daten\d.txt"),
            "{text}"
        );
        assert!(text.contains(r"Fehlt: D:\Daten\c.txt"), "{text}");
        assert!(!text.contains("Abgebrochen"), "{text}");
    }

    #[test]
    fn ergebnistext_meldet_abbruch_und_begrenzt_fehlerzeilen() {
        let many: Vec<_> = (0..30)
            .map(|i| result(i, &format!(r"D:\x{i}.txt"), ItemOutcome::Failed("e".into())))
            .collect();
        let text = restore_result_text(40, &many);
        assert!(text.contains("Abgebrochen: 30 von 40 bearbeitet"), "{text}");
        assert!(
            text.contains(r"D:\x9.txt") && !text.contains(r"D:\x10.txt"),
            "{text}"
        );
        assert!(text.contains("und 20 weitere"), "{text}");
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
