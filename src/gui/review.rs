//! Review-Modell: Zeilen aus einem `Plan`, Filter, Sortierung und Auswahl. Rein und fensterfrei;
//! die Ansicht (`views::review`) zeichnet nur, was hier berechnet wird.

use std::collections::HashSet;
use std::path::Path;

use crate::change::plan::{dependents, ActionType, Plan, Skipped};
use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    /// Aktionen des Plans (auswählbar)
    Actions,
    /// Übersprungene Einträge mit Grund (nicht auswählbar)
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Action,
    Source,
    Target,
    Size,
    Reason,
    Age,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sort {
    pub column: Column,
    pub ascending: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub show: Show,
    pub action: Option<ActionType>,
    pub rule: Option<String>,
    /// Nur Einträge unterhalb dieses Ordners
    pub folder: Option<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Teilstring in Quelle, Ziel, Grund oder Regel (ohne Groß-/Kleinschreibung)
    pub text: String,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            show: Show::Actions,
            action: None,
            rule: None,
            folder: None,
            min_size: None,
            max_size: None,
            text: String::new(),
        }
    }
}

/// Eine Zeile der Tabelle.
#[derive(Debug, Clone)]
pub struct Row {
    /// Aktions-ID (nur Aktionen)
    pub id: Option<u32>,
    pub action: Option<ActionType>,
    pub path: String,
    pub target: Option<String>,
    pub size: u64,
    /// Grund; bei Übersprungenen der Text von `SkipReason` samt Detail
    pub reason: String,
    pub rule: Option<String>,
    pub mtime_ticks: i64,
    /// Kleingeschriebener Pfadschlüssel (Filter nach Ordner)
    key: String,
    /// Kleingeschriebener Suchtext
    haystack: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub selected: usize,
    pub selected_bytes: u64,
    pub total: usize,
    pub total_bytes: u64,
    pub quarantine: usize,
    pub moves: usize,
    pub remove_dirs: usize,
}

pub struct ReviewModel {
    plan: Plan,
    rows: Vec<Row>,
    /// Je Zeile; nur Aktionen können gewählt sein
    selected: Vec<bool>,
    filter: Filter,
    sort: Option<Sort>,
    /// Indizes in `rows`: gefiltert und sortiert
    view: Vec<usize>,
}

fn key_of(path: &str) -> String {
    paths::path_key(Path::new(path))
}

fn skipped_reason(s: &Skipped) -> String {
    match &s.detail {
        Some(detail) => format!("{} ({detail})", s.reason),
        None => s.reason.to_string(),
    }
}

impl ReviewModel {
    pub fn new(plan: Plan) -> Self {
        let mut rows = Vec::with_capacity(plan.actions.len() + plan.skipped.len());
        for a in &plan.actions {
            rows.push(Row::new(
                Some(a.id),
                Some(a.action),
                &a.path,
                a.target.clone(),
                a.size,
                a.reason.clone(),
                a.rule.clone(),
                a.mtime_ticks,
            ));
        }
        for s in &plan.skipped {
            rows.push(Row::new(
                None,
                None,
                &s.path,
                None,
                0,
                skipped_reason(s),
                None,
                0,
            ));
        }
        let selected = rows.iter().map(|r| r.id.is_some()).collect();
        let mut model = Self {
            plan,
            rows,
            selected,
            filter: Filter::default(),
            sort: None,
            view: Vec::new(),
        };
        model.rebuild();
        model
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub fn filter(&self) -> &Filter {
        &self.filter
    }

    pub fn sort(&self) -> Option<&Sort> {
        self.sort.as_ref()
    }

    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        self.rebuild();
    }

    pub fn set_sort(&mut self, sort: Option<Sort>) {
        self.sort = sort;
        self.rebuild();
    }

    fn matches(&self, row: &Row) -> bool {
        let f = &self.filter;
        if (f.show == Show::Actions) != row.id.is_some() {
            return false;
        }
        if f.action.is_some() && row.action != f.action {
            return false;
        }
        if let Some(rule) = &f.rule {
            if row.rule.as_deref() != Some(rule.as_str()) {
                return false;
            }
        }
        if let Some(folder) = &f.folder {
            if !paths::is_under(&row.key, &key_of(folder)) {
                return false;
            }
        }
        if f.min_size.is_some_and(|m| row.size < m) || f.max_size.is_some_and(|m| row.size > m) {
            return false;
        }
        let text = f.text.trim();
        text.is_empty() || row.haystack.contains(&text.to_lowercase())
    }

    fn rebuild(&mut self) {
        let mut view: Vec<usize> = (0..self.rows.len())
            .filter(|&i| self.matches(&self.rows[i]))
            .collect();
        if let Some(sort) = &self.sort {
            let rows = &self.rows;
            view.sort_by(|&a, &b| {
                let (a, b) = (&rows[a], &rows[b]);
                let ord = match sort.column {
                    Column::Action => a.action.map(action_rank).cmp(&b.action.map(action_rank)),
                    Column::Source => a.key.cmp(&b.key),
                    Column::Target => a.target.cmp(&b.target),
                    Column::Size => a.size.cmp(&b.size),
                    Column::Reason => a.reason.cmp(&b.reason),
                    Column::Age => a.mtime_ticks.cmp(&b.mtime_ticks),
                };
                if sort.ascending {
                    ord
                } else {
                    ord.reverse()
                }
            });
        }
        self.view = view;
    }

    pub fn len(&self) -> usize {
        self.view.len()
    }

    pub fn is_empty(&self) -> bool {
        self.view.is_empty()
    }

    pub fn row(&self, view_index: usize) -> &Row {
        &self.rows[self.view[view_index]]
    }

    pub fn is_selected(&self, view_index: usize) -> bool {
        self.selected[self.view[view_index]]
    }

    pub fn toggle(&mut self, view_index: usize) {
        let i = self.view[view_index];
        if self.rows[i].id.is_some() {
            self.selected[i] = !self.selected[i];
        }
    }

    fn set_rows(&mut self, indices: impl Iterator<Item = usize>, on: bool) {
        let mut turned_off = Vec::new();
        for i in indices {
            if self.rows[i].id.is_some() {
                if !on && self.selected[i] {
                    turned_off.push(i);
                }
                self.selected[i] = on;
            }
        }
        if !on {
            self.deselect_ancestors_of(&turned_off);
        }
    }

    /// Beim gesammelten Abwählen werden gewählte Ordner-Aktionen (`remove-dir`, Ordner
    /// verschieben) mit abgewählt, die einen der abgewählten Einträge enthielten: Sonst nähme
    /// das Verschieben den abgewählten Inhalt mit, und das Entfernen scheiterte.
    fn deselect_ancestors_of(&mut self, turned_off: &[usize]) {
        let dir_ids: HashSet<u32> = self
            .plan
            .actions
            .iter()
            .filter(|a| a.is_dir)
            .map(|a| a.id)
            .collect();
        let containers: std::collections::HashMap<String, usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(i, r)| self.selected[*i] && r.id.is_some())
            .filter(|(_, r)| {
                r.action == Some(ActionType::RemoveDir)
                    || r.id.is_some_and(|id| dir_ids.contains(&id))
            })
            .map(|(i, r)| (r.key.clone(), i))
            .collect();
        if containers.is_empty() {
            return;
        }
        for &i in turned_off {
            let mut key = self.rows[i].key.as_str();
            while let Some(cut) = key.rfind('\\') {
                key = &key[..cut];
                if let Some(&parent) = containers.get(key) {
                    self.selected[parent] = false;
                }
            }
        }
    }

    pub fn select_all(&mut self, on: bool) {
        self.set_rows(0..self.rows.len(), on);
    }

    /// Alle aktuell gefilterten Zeilen.
    pub fn select_filtered(&mut self, on: bool) {
        let view = self.view.clone();
        self.set_rows(view.into_iter(), on);
    }

    /// Alle Aktionen unterhalb von `folder`, unabhängig vom Filter.
    pub fn select_folder(&mut self, folder: &str, on: bool) {
        let prefix = key_of(folder);
        let hits: Vec<usize> = (0..self.rows.len())
            .filter(|&i| paths::is_under(&self.rows[i].key, &prefix))
            .collect();
        self.set_rows(hits.into_iter(), on);
    }

    /// Beim Abwählen: noch gewählte Aktionen, die dadurch scheitern würden.
    pub fn dependents_to_deselect(&self, view_index: usize) -> Vec<u32> {
        let Some(id) = self.row(view_index).id else {
            return Vec::new();
        };
        let chosen = self.selected_ids();
        dependents(&self.plan, id)
            .into_iter()
            .filter(|d| chosen.contains(d))
            .collect()
    }

    pub fn deselect_ids(&mut self, ids: &[u32]) {
        let set: HashSet<u32> = ids.iter().copied().collect();
        for (i, row) in self.rows.iter().enumerate() {
            if row.id.is_some_and(|id| set.contains(&id)) {
                self.selected[i] = false;
            }
        }
    }

    pub fn summary(&self) -> Summary {
        let mut s = Summary::default();
        for (i, row) in self.rows.iter().enumerate() {
            let Some(action) = row.action else { continue };
            s.total += 1;
            s.total_bytes += row.size;
            if self.selected[i] {
                s.selected += 1;
                s.selected_bytes += row.size;
                match action {
                    ActionType::Quarantine => s.quarantine += 1,
                    ActionType::Move => s.moves += 1,
                    ActionType::RemoveDir => s.remove_dirs += 1,
                }
            }
        }
        s
    }

    pub fn selected_ids(&self) -> HashSet<u32> {
        self.rows
            .iter()
            .zip(&self.selected)
            .filter_map(|(row, on)| row.id.filter(|_| *on))
            .collect()
    }

    pub fn all_selected(&self) -> bool {
        self.rows
            .iter()
            .zip(&self.selected)
            .all(|(row, on)| row.id.is_none() || *on)
    }

    /// Regeln des Plans (für den Filter), sortiert und ohne Doppelte.
    pub fn rules(&self) -> Vec<String> {
        let mut rules: Vec<String> = self.rows.iter().filter_map(|r| r.rule.clone()).collect();
        rules.sort();
        rules.dedup();
        rules
    }
}

fn action_rank(a: ActionType) -> u8 {
    match a {
        ActionType::Quarantine => 0,
        ActionType::Move => 1,
        ActionType::RemoveDir => 2,
    }
}

impl Row {
    #[allow(clippy::too_many_arguments)]
    fn new(
        id: Option<u32>,
        action: Option<ActionType>,
        path: &str,
        target: Option<String>,
        size: u64,
        reason: String,
        rule: Option<String>,
        mtime_ticks: i64,
    ) -> Self {
        let haystack = format!(
            "{} {} {} {}",
            path,
            target.as_deref().unwrap_or(""),
            reason,
            rule.as_deref().unwrap_or("")
        )
        .to_lowercase();
        Self {
            id,
            action,
            key: key_of(path),
            path: path.to_string(),
            target,
            size,
            reason,
            rule,
            mtime_ticks,
            haystack,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::plan::{PlanKind, PlannedAction, Skipped, PLAN_VERSION};
    use crate::change::SkipReason;

    fn action(id: u32, kind: ActionType, path: &str, size: u64) -> PlannedAction {
        PlannedAction {
            id,
            action: kind,
            path: path.into(),
            size,
            mtime_ticks: i64::from(id) * 10,
            mtime: String::new(),
            hash: None,
            keep: None,
            keep_hash: None,
            reason: "junk:temp".into(),
            target: None,
            is_dir: kind == ActionType::RemoveDir,
            files: None,
            rule: None,
        }
    }

    fn plan(actions: Vec<PlannedAction>, skipped: Vec<Skipped>) -> Plan {
        Plan {
            version: PLAN_VERSION,
            created: "2026-10-05T10:00:00+02:00".into(),
            kind: PlanKind::Junk,
            root: r"D:\Daten".into(),
            keep_strategy: None,
            params: Default::default(),
            protected_paths: Vec::new(),
            actions,
            skipped,
        }
    }

    fn sample() -> ReviewModel {
        ReviewModel::new(plan(
            vec![
                action(1, ActionType::Quarantine, r"D:\Daten\a\x.tmp", 300),
                action(2, ActionType::Quarantine, r"D:\Daten\b\Y.TMP", 100),
                action(3, ActionType::Quarantine, r"D:\Daten\a\sub\z.tmp", 200),
            ],
            vec![Skipped {
                path: r"D:\Daten\geschuetzt.sys".into(),
                reason: SkipReason::Protected,
                detail: Some("System".into()),
            }],
        ))
    }

    fn ids(m: &ReviewModel) -> Vec<Option<u32>> {
        (0..m.len()).map(|i| m.row(i).id).collect()
    }

    #[test]
    fn neues_modell_hat_alle_aktionen_gewaehlt() {
        let m = sample();
        assert_eq!(m.len(), 3);
        assert!(m.all_selected());
        let s = m.summary();
        assert_eq!(
            (s.selected, s.selected_bytes, s.total, s.total_bytes),
            (3, 600, 3, 600)
        );
        assert_eq!(s.quarantine, 3);
        assert_eq!(m.selected_ids(), [1, 2, 3].into());
    }

    #[test]
    fn uebersprungene_sind_eigener_filter_und_nicht_auswaehlbar() {
        let mut m = sample();
        m.set_filter(Filter {
            show: Show::Skipped,
            ..Filter::default()
        });
        assert_eq!(m.len(), 1);
        assert_eq!(m.row(0).id, None);
        assert!(m.row(0).reason.contains("geschützter Pfad"));
        assert!(m.row(0).reason.contains("System"));
        assert!(!m.is_selected(0));
        m.toggle(0);
        m.select_filtered(true);
        assert_eq!(m.selected_ids().len(), 3, "übersprungene zählen nie");
    }

    #[test]
    fn abwaehlen_aendert_zusammenfassung_und_ids() {
        let mut m = sample();
        m.toggle(1);
        assert!(!m.is_selected(1));
        let s = m.summary();
        assert_eq!((s.selected, s.selected_bytes), (2, 500));
        assert_eq!(m.selected_ids(), [1, 3].into());
        assert!(!m.all_selected());
        m.toggle(1);
        assert!(m.all_selected());
    }

    #[test]
    fn filter_nach_text_ordner_groesse_und_aktion() {
        let mut m = sample();
        m.set_filter(Filter {
            text: "TMP".into(),
            ..Filter::default()
        });
        assert_eq!(m.len(), 3, "ohne Groß-/Kleinschreibung");
        m.set_filter(Filter {
            folder: Some(r"D:\Daten\A".into()),
            ..Filter::default()
        });
        assert_eq!(
            ids(&m),
            [Some(1), Some(3)],
            "unterhalb von, ohne Groß-/Kleinschreibung"
        );
        m.set_filter(Filter {
            min_size: Some(150),
            max_size: Some(250),
            ..Filter::default()
        });
        assert_eq!(ids(&m), [Some(3)]);
        m.set_filter(Filter {
            action: Some(ActionType::Move),
            ..Filter::default()
        });
        assert!(m.is_empty());
    }

    #[test]
    fn ordnerfilter_trifft_nicht_aehnlich_benannte_ordner() {
        let mut m = ReviewModel::new(plan(
            vec![
                action(1, ActionType::Quarantine, r"D:\Daten\a\x.tmp", 1),
                action(2, ActionType::Quarantine, r"D:\Daten\ab\y.tmp", 1),
            ],
            vec![],
        ));
        m.set_filter(Filter {
            folder: Some(r"D:\Daten\a".into()),
            ..Filter::default()
        });
        assert_eq!(ids(&m), [Some(1)]);
    }

    #[test]
    fn sortierung_ist_stabil_und_umkehrbar() {
        let mut m = sample();
        m.set_sort(Some(Sort {
            column: Column::Size,
            ascending: true,
        }));
        assert_eq!(ids(&m), [Some(2), Some(3), Some(1)]);
        m.set_sort(Some(Sort {
            column: Column::Size,
            ascending: false,
        }));
        assert_eq!(ids(&m), [Some(1), Some(3), Some(2)]);
        m.set_sort(Some(Sort {
            column: Column::Source,
            ascending: true,
        }));
        assert_eq!(
            ids(&m),
            [Some(3), Some(1), Some(2)],
            "a\\sub\\z, a\\x, b\\y"
        );
        m.set_sort(None);
        assert_eq!(ids(&m), [Some(1), Some(2), Some(3)], "Planreihenfolge");
    }

    #[test]
    fn gleiche_sortierwerte_behalten_die_planreihenfolge() {
        let mut m = ReviewModel::new(plan(
            (1..=5)
                .map(|i| action(i, ActionType::Quarantine, &format!(r"D:\Daten\f{i}.tmp"), 7))
                .collect(),
            vec![],
        ));
        m.set_sort(Some(Sort {
            column: Column::Size,
            ascending: false,
        }));
        assert_eq!(ids(&m), (1..=5).map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn gesammelt_waehlen_gefiltert_ordner_alle() {
        let mut m = sample();
        m.select_all(false);
        assert_eq!(m.summary().selected, 0);
        m.set_filter(Filter {
            folder: Some(r"D:\Daten\a".into()),
            ..Filter::default()
        });
        m.select_filtered(true);
        assert_eq!(m.selected_ids(), [1, 3].into());
        m.set_filter(Filter::default());
        m.select_folder(r"D:\Daten\a\sub", false);
        assert_eq!(m.selected_ids(), [1].into());
        m.select_all(true);
        assert_eq!(m.selected_ids().len(), 3);
    }

    #[test]
    fn gesammeltes_abwaehlen_nimmt_enthaltende_ordner_mit() {
        let mut dir_move = action(2, ActionType::Move, r"D:\Daten\a", 0);
        dir_move.is_dir = true;
        let mut m = ReviewModel::new(plan(
            vec![
                action(1, ActionType::Quarantine, r"D:\Daten\a\x.tmp", 5),
                dir_move,
                action(3, ActionType::Quarantine, r"D:\Daten\b.tmp", 5),
            ],
            vec![],
        ));
        m.select_folder(r"D:\Daten\a\x.tmp", false);
        assert_eq!(
            m.selected_ids(),
            [3].into(),
            "Ordner a würde x.tmp mitnehmen"
        );
        m.select_all(true);
        m.select_filtered(false);
        assert!(m.selected_ids().is_empty());
    }

    #[test]
    fn abhaengige_eintraege_werden_beim_abwaehlen_vorgeschlagen() {
        let mut m = ReviewModel::new(Plan {
            kind: PlanKind::EmptyDirs,
            ..plan(
                vec![
                    action(1, ActionType::RemoveDir, r"D:\Daten\a\b\c", 0),
                    action(2, ActionType::RemoveDir, r"D:\Daten\a\b", 0),
                    action(3, ActionType::RemoveDir, r"D:\Daten\a", 0),
                ],
                vec![],
            )
        });
        assert_eq!(m.dependents_to_deselect(0), vec![2, 3]);
        m.deselect_ids(&[1, 2, 3]);
        assert_eq!(m.summary().selected, 0);
        // schon abgewählte Abhängige werden nicht erneut genannt
        m.select_all(true);
        m.deselect_ids(&[2]);
        assert_eq!(m.dependents_to_deselect(0), vec![3]);
    }

    #[test]
    fn regeln_des_plans_sind_eindeutig_und_sortiert() {
        let mut a = action(1, ActionType::Move, r"D:\Daten\a.jpg", 1);
        a.rule = Some("fotos".into());
        let mut b = action(2, ActionType::Move, r"D:\Daten\b.jpg", 1);
        b.rule = Some("fotos".into());
        let mut c = action(3, ActionType::Move, r"D:\Daten\c.pdf", 1);
        c.rule = Some("dokumente".into());
        let m = ReviewModel::new(plan(vec![a, b, c], vec![]));
        assert_eq!(m.rules(), ["dokumente", "fotos"]);
        let mut m = m;
        m.set_filter(Filter {
            rule: Some("fotos".into()),
            ..Filter::default()
        });
        assert_eq!(ids(&m), [Some(1), Some(2)]);
    }

    #[test]
    fn hunderttausend_aktionen_filtern_und_sortieren_ist_schnell_genug() {
        let actions: Vec<PlannedAction> = (1..=100_000)
            .map(|i| {
                action(
                    i,
                    ActionType::Quarantine,
                    &format!(r"D:\Daten\Ordner{}\Datei{i}.tmp", i % 100),
                    u64::from(i % 977),
                )
            })
            .collect();
        let started = std::time::Instant::now();
        let mut m = ReviewModel::new(plan(actions, vec![]));
        m.set_filter(Filter {
            text: "datei9".into(),
            ..Filter::default()
        });
        m.set_sort(Some(Sort {
            column: Column::Size,
            ascending: false,
        }));
        let elapsed = started.elapsed();
        assert!(m.len() > 10_000);
        // Release-Ziel < 300 ms; im Debug-Build großzügiger, es geht um Größenordnungen.
        let limit = if cfg!(debug_assertions) { 10 } else { 1 };
        assert!(elapsed.as_secs() < limit, "{elapsed:?}");
    }
}
