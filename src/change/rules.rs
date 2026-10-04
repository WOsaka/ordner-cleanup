//! Planer für `plan rules`: wendet die Regeln der Regeldatei auf alle Dateien im Index an und
//! erzeugt `move`-Aktionen (Einsortieren oder Umbenennen). Verändert nichts; liest nur den
//! Index. Das EXIF-Datum (nur wenn ein Ziel es braucht) und der Inhaltsvergleich bei belegten
//! Zielen kommen über Traits herein, damit der Planer ohne Dateisystem testbar bleibt.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, NaiveDateTime};
use rayon::prelude::*;

use super::plan::{
    ticks_to_rfc3339, ActionType, Plan, PlanKind, PlannedAction, Skipped, ARCHIVE_DIR, PLAN_VERSION,
};
use super::protect::{Protector, TOOL_DIR};
use super::SkipReason;
use crate::analysis::problems::{name_issue, MAX_PATH_CHARS};
use crate::index::{ExifEntry, FileRow, Index, IndexError};
use crate::paths;
use crate::rules::template::TargetError;
use crate::rules::{Candidate, RuleSet};
use crate::scan::exif::{self, ExifError};
use crate::scan::hasher;

/// Mehr Nummern (` (2)`, ` (3)` …) werden nicht probiert.
const MAX_NUMBERING: usize = 1000;
/// So viele Beispiele „alt → neu“ je Regel für die Vorschau.
const EXAMPLES_PER_RULE: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExifResult {
    Date(NaiveDateTime),
    /// Kein (lesbares) EXIF.
    NoDate,
    /// Cloud-only oder Link: nicht geöffnet.
    Cloud,
}

pub trait ExifSource: Sync {
    /// Aufnahmedaten zu `files` (gleiche Reihenfolge). Die echte Implementierung nutzt den
    /// Cache im Index und liest nur fehlende oder geänderte Dateien.
    fn taken(&self, index: &mut Index, files: &[&FileRow]) -> Vec<ExifResult>;
}

pub trait ContentSource: Sync {
    /// Haben beide Dateien (gleicher Größe) denselben Inhalt?
    fn same_content(&self, a: &FileRow, b: &FileRow) -> bool;
}

pub struct RulesEnv<'a> {
    pub protector: &'a Protector,
    pub exif: &'a dyn ExifSource,
    pub content: &'a dyn ContentSource,
    pub created: &'a str,
    pub now_ticks: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleStats {
    pub name: String,
    pub actions: usize,
    pub bytes: u64,
    /// Bis zu drei Beispiele „alt → neu“, relativ zur Wurzel.
    pub examples: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct RulesPlan {
    pub plan: Plan,
    pub bytes: u64,
    pub per_rule: Vec<RuleStats>,
    /// Dateien, auf die keine Regel passt.
    pub unmatched: usize,
    /// Hinweise, z. B. Regeln, die einander die Dateien weiterreichen.
    pub notes: Vec<String>,
}

/// Was an einem Pfad (Schlüssel) schon liegt oder im Plan dorthin soll.
enum Occupant {
    File(usize),
    Dir { is_link: bool },
    Assigned,
}

struct Pending {
    file: usize,
    rule: usize,
    caps: Vec<String>,
    rel: String,
    parent: String,
}

fn candidate<'a>(p: &'a Pending, row: &'a FileRow) -> Candidate<'a> {
    Candidate {
        rel: &p.rel,
        name: &row.name,
        parent: &p.parent,
        size: row.size.max(0) as u64,
        mtime_ticks: row.mtime,
    }
}

fn rel_string(root: &Path, path: &Path) -> String {
    paths::relative_to(root, path)
        .unwrap_or_default()
        .to_string_lossy()
        .replace('\\', "/")
}

fn numbered(file: &str, n: usize) -> String {
    match file.rfind('.') {
        Some(i) if i > 0 => format!("{} ({n}){}", &file[..i], &file[i..]),
        _ => format!("{file} ({n})"),
    }
}

fn skip(skipped: &mut Vec<(String, Skipped)>, row: &FileRow, reason: SkipReason) {
    skipped.push((
        row.key.clone(),
        Skipped {
            path: row.path.clone(),
            reason,
        },
    ));
}

pub fn plan_rules(
    index: &mut Index,
    root: &Path,
    rules: &RuleSet,
    env: &RulesEnv,
) -> Result<RulesPlan, IndexError> {
    let root_key = paths::dir_key(root);
    let archive_key = paths::dir_key(&root.join(ARCHIVE_DIR));
    let tool_key = paths::dir_key(&root.join(TOOL_DIR));
    let dirs = index.dirs_under(&root_key)?;
    let files = index.files_under(&root_key)?;

    // Schreibweise der Ordner, wie sie im Index stehen: Ziele nutzen sie, damit eine Datei in
    // einem vorhandenen Ordner nicht wegen Groß-/Kleinschreibung bei jedem Lauf „umbenannt“ wird.
    let mut spelling: HashMap<String, String> = HashMap::new();
    let mut occupied: HashMap<String, Occupant> = HashMap::new();
    for d in &dirs {
        let key = d.key.trim_end_matches('\\').to_string();
        spelling.insert(key.clone(), d.path.clone());
        occupied.insert(key, Occupant::Dir { is_link: d.is_link });
    }
    for (i, f) in files.iter().enumerate() {
        occupied.insert(f.key.clone(), Occupant::File(i));
    }
    let root_spelling = spelling
        .get(root_key.trim_end_matches('\\'))
        .map_or_else(|| root.to_path_buf(), PathBuf::from);
    let root_name = root_spelling
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut skipped: Vec<(String, Skipped)> = Vec::new();
    let mut unmatched = 0usize;
    let mut pending: Vec<Pending> = Vec::new();
    for (i, row) in files.iter().enumerate() {
        if row.dir_key.starts_with(&archive_key) || row.dir_key.starts_with(&tool_key) {
            continue;
        }
        let path = Path::new(&row.path);
        let rel = rel_string(&root_spelling, path);
        let parent = path
            .parent()
            .filter(|p| paths::dir_key(p) != root_key)
            .and_then(Path::file_name)
            .map_or_else(|| root_name.clone(), |n| n.to_string_lossy().into_owned());
        let cand = Candidate {
            rel: &rel,
            name: &row.name,
            parent: &parent,
            size: row.size.max(0) as u64,
            mtime_ticks: row.mtime,
        };
        let Some((rule, caps)) = rules
            .rules
            .iter()
            .enumerate()
            .find_map(|(r, rule)| rule.matches(&cand, env.now_ticks).map(|c| (r, c)))
        else {
            unmatched += 1;
            continue;
        };
        if env.protector.check_cached(path).is_some() {
            skip(&mut skipped, row, SkipReason::Protected);
        } else if row.is_link {
            skip(&mut skipped, row, SkipReason::Link);
        } else if row.cloud_only {
            skip(&mut skipped, row, SkipReason::CloudPlaceholder);
        } else {
            pending.push(Pending {
                file: i,
                rule,
                caps,
                rel,
                parent,
            });
        }
    }

    // EXIF nur für Dateien, deren Regel das Aufnahmedatum braucht.
    let needing: Vec<usize> = (0..pending.len())
        .filter(|&p| rules.rules[pending[p].rule].needs_exif())
        .collect();
    let mut taken: HashMap<usize, ExifResult> = HashMap::new();
    if !needing.is_empty() {
        let rows: Vec<&FileRow> = needing.iter().map(|&p| &files[pending[p].file]).collect();
        for (&p, result) in needing.iter().zip(env.exif.taken(index, &rows)) {
            taken.insert(p, result);
        }
    }

    let mut actions: Vec<(String, PlannedAction)> = Vec::new();
    let mut stats: Vec<RuleStats> = rules
        .rules
        .iter()
        .map(|r| RuleStats {
            name: r.name.clone(),
            actions: 0,
            bytes: 0,
            examples: Vec::new(),
        })
        .collect();
    // Aktionen, die beim nächsten Lauf ein anderes Ziel hätten (ohne Durchnummerierte).
    let mut probes: Vec<(usize, PathBuf)> = Vec::new();
    let mut hints: HashMap<String, usize> = HashMap::new();

    for (p_idx, p) in pending.iter().enumerate() {
        let row = &files[p.file];
        let rule = &rules.rules[p.rule];
        let exif_date = match taken.get(&p_idx) {
            Some(ExifResult::Cloud) => {
                skip(&mut skipped, row, SkipReason::CloudPlaceholder);
                continue;
            }
            Some(ExifResult::Date(d)) => Some(*d),
            _ => None,
        };
        let rel_target = match rule.target(&candidate(p, row), &p.caps, exif_date) {
            Ok(t) => t,
            Err(TargetError::NoExifDate) => {
                skip(&mut skipped, row, SkipReason::NoExifDate);
                continue;
            }
            Err(TargetError::Invalid(_)) => {
                skip(&mut skipped, row, SkipReason::InvalidTarget);
                continue;
            }
        };
        if rel_target
            .dirs
            .iter()
            .chain(std::iter::once(&rel_target.file))
            .any(|segment| name_issue(segment).is_some())
        {
            skip(&mut skipped, row, SkipReason::InvalidTarget);
            continue;
        }

        // Zielordner Ebene für Ebene: Eine Datei auf dem Weg blockiert, ein Link lenkt um.
        let source = Path::new(&row.path);
        let mut dir = if rel_target.same_dir {
            source.parent().unwrap_or(&root_spelling).to_path_buf()
        } else {
            root_spelling.clone()
        };
        let mut blocked = None;
        for segment in &rel_target.dirs {
            dir = dir.join(segment);
            let key = paths::path_key(&dir);
            match occupied.get(&key) {
                Some(Occupant::File(_) | Occupant::Assigned) => {
                    blocked = Some(SkipReason::InvalidTarget);
                    break;
                }
                Some(Occupant::Dir { is_link: true }) => {
                    blocked = Some(SkipReason::Link);
                    break;
                }
                Some(Occupant::Dir { is_link: false }) => {
                    if let Some(existing) = spelling.get(&key) {
                        dir = PathBuf::from(existing);
                    }
                }
                None => {}
            }
        }
        if let Some(reason) = blocked {
            skip(&mut skipped, row, reason);
            continue;
        }

        // Freies Ziel suchen: erst der Name selbst, dann ` (2)`, ` (3)` …
        enum Outcome {
            AlreadyThere,
            Move(PathBuf, bool),
            Skip(SkipReason),
        }
        let mut outcome = Outcome::Skip(SkipReason::InvalidTarget);
        // Viele Dateien mit demselben Zielnamen würden sonst jedes Mal alle vergebenen
        // Nummern erneut durchprobieren. Der Merker gilt nur über rein vergebene Nummern.
        let base_key = paths::path_key(&dir.join(&rel_target.file));
        let start = hints.get(&base_key).copied().unwrap_or(1);
        let mut prefix_assigned = true;
        let mut chosen = start;
        for n in start..=MAX_NUMBERING {
            chosen = n;
            let name = if n == 1 {
                rel_target.file.clone()
            } else {
                numbered(&rel_target.file, n)
            };
            let target = dir.join(&name);
            if paths::display(&target).chars().count() > MAX_PATH_CHARS {
                outcome = Outcome::Skip(SkipReason::PathTooLong);
                break;
            }
            let key = paths::path_key(&target);
            if key == row.key {
                outcome = if paths::display(&target) == row.path {
                    Outcome::AlreadyThere
                } else {
                    Outcome::Move(target, n > 1)
                };
                break;
            }
            match occupied.get(&key) {
                None => {
                    outcome = Outcome::Move(target, n > 1);
                    break;
                }
                Some(Occupant::Dir { .. }) => {
                    outcome = Outcome::Skip(SkipReason::InvalidTarget);
                    break;
                }
                Some(Occupant::File(j)) => {
                    prefix_assigned = false;
                    let other = &files[*j];
                    if other.size == row.size
                        && !other.cloud_only
                        && !other.is_link
                        && env.content.same_content(row, other)
                    {
                        outcome = Outcome::Skip(SkipReason::DuplicateAtTarget);
                        break;
                    }
                }
                Some(Occupant::Assigned) => {}
            }
        }
        if prefix_assigned && matches!(outcome, Outcome::Move(..)) {
            hints.insert(base_key, chosen + 1);
        }
        let (target, was_numbered) = match outcome {
            Outcome::AlreadyThere => continue,
            Outcome::Skip(reason) => {
                skip(&mut skipped, row, reason);
                continue;
            }
            Outcome::Move(target, was_numbered) => (target, was_numbered),
        };
        let target_key = paths::path_key(&target);
        if paths::is_under(&target_key, archive_key.trim_end_matches('\\'))
            || paths::is_under(&target_key, tool_key.trim_end_matches('\\'))
            || env.protector.check_cached(&target).is_some()
        {
            skip(&mut skipped, row, SkipReason::Protected);
            continue;
        }
        occupied.insert(target_key, Occupant::Assigned);

        let old_rel = p.rel.clone();
        let new_rel = rel_string(&root_spelling, &target);
        let st = &mut stats[p.rule];
        st.actions += 1;
        st.bytes += row.size.max(0) as u64;
        if st.examples.len() < EXAMPLES_PER_RULE {
            st.examples.push((old_rel, new_rel));
        }
        if !was_numbered {
            probes.push((p_idx, target.clone()));
        }
        actions.push((
            row.key.clone(),
            PlannedAction {
                id: 0,
                action: ActionType::Move,
                path: row.path.clone(),
                size: row.size.max(0) as u64,
                mtime_ticks: row.mtime,
                mtime: ticks_to_rfc3339(row.mtime),
                hash: None,
                keep: None,
                keep_hash: None,
                reason: format!("rule:{}", rule.name),
                target: Some(paths::display(&target)),
                is_dir: false,
                files: None,
                rule: Some(rule.name.clone()),
            },
        ));
    }

    let notes = stability_notes(
        &probes,
        &pending,
        &files,
        &taken,
        rules,
        &root_spelling,
        env,
    );

    actions.sort_by(|a, b| a.0.cmp(&b.0));
    skipped.sort_by(|a, b| a.0.cmp(&b.0));
    let actions: Vec<PlannedAction> = actions
        .into_iter()
        .zip(1u32..)
        .map(|((_, mut a), id)| {
            a.id = id;
            a
        })
        .collect();
    let bytes = actions.iter().map(|a| a.size).sum();
    let mut params = BTreeMap::new();
    params.insert("rules_file".to_string(), paths::display(&rules.source));
    Ok(RulesPlan {
        plan: Plan {
            version: PLAN_VERSION,
            created: env.created.to_string(),
            kind: PlanKind::Rules,
            root: paths::display(root),
            keep_strategy: None,
            params,
            actions,
            skipped: skipped.into_iter().map(|(_, s)| s).collect(),
        },
        bytes,
        per_rule: stats,
        unmatched,
        notes,
    })
}

/// Wendet die Regeln probehalber auf jedes Ziel an: Würde eine Datei beim nächsten Lauf erneut
/// bewegt, bekommt das Regelpaar einen Hinweis (einmal je Paar).
fn stability_notes(
    probes: &[(usize, PathBuf)],
    pending: &[Pending],
    files: &[FileRow],
    taken: &HashMap<usize, ExifResult>,
    rules: &RuleSet,
    root: &Path,
    env: &RulesEnv,
) -> Vec<String> {
    let root_name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    let mut notes = Vec::new();
    for (p_idx, target) in probes {
        let p = &pending[*p_idx];
        let row = &files[p.file];
        let rel = rel_string(root, target);
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parent = target
            .parent()
            .filter(|d| paths::dir_key(d) != paths::dir_key(root))
            .and_then(Path::file_name)
            .map_or_else(|| root_name.clone(), |n| n.to_string_lossy().into_owned());
        let cand = Candidate {
            rel: &rel,
            name: &name,
            parent: &parent,
            size: row.size.max(0) as u64,
            mtime_ticks: row.mtime,
        };
        let Some((next, caps)) = rules
            .rules
            .iter()
            .enumerate()
            .find_map(|(r, rule)| rule.matches(&cand, env.now_ticks).map(|c| (r, c)))
        else {
            continue;
        };
        let exif_date = match taken.get(p_idx) {
            Some(ExifResult::Date(d)) => Some(*d),
            _ => None,
        };
        let Ok(again) = rules.rules[next].target(&cand, &caps, exif_date) else {
            continue;
        };
        let mut next_dir = if again.same_dir {
            target.parent().unwrap_or(root).to_path_buf()
        } else {
            root.to_path_buf()
        };
        for d in &again.dirs {
            next_dir = next_dir.join(d);
        }
        let next_target = next_dir.join(&again.file);
        if paths::path_key(&next_target) == paths::path_key(target) {
            continue;
        }
        if seen.insert((p.rule, next)) {
            notes.push(format!(
                "Regel „{}“ legt Dateien so ab, dass Regel „{}“ sie beim nächsten Lauf erneut \
                 bewegen würde (z. B. {rel} → {}). Der Zustand wird erst nach mehreren Läufen \
                 stabil oder nie.",
                rules.rules[p.rule].name,
                rules.rules[next].name,
                rel_string(root, &next_target),
            ));
        }
    }
    notes
}

/// Echte EXIF-Quelle: Cache im Index (gültig bei gleicher Größe und mtime), fehlende Einträge
/// werden parallel gelesen und in **einer** Transaktion gespeichert.
pub struct CachedExif;

impl ExifSource for CachedExif {
    fn taken(&self, index: &mut Index, files: &[&FileRow]) -> Vec<ExifResult> {
        let keys: Vec<(&str, i64, i64)> = files
            .iter()
            .map(|f| (f.key.as_str(), f.size, f.mtime))
            .collect();
        let cached = index.exif_lookup(&keys).unwrap_or_default();
        let from_secs = |secs: i64| {
            DateTime::from_timestamp(secs, 0)
                .map(|t| t.naive_utc())
                .map_or(ExifResult::NoDate, ExifResult::Date)
        };
        let mut results: Vec<Option<ExifResult>> = files
            .iter()
            .map(|f| {
                cached.get(&f.key).map(|taken| match taken {
                    Some(secs) => from_secs(*secs),
                    None => ExifResult::NoDate,
                })
            })
            .collect();
        let missing: Vec<usize> = (0..files.len()).filter(|&i| results[i].is_none()).collect();
        let read: Vec<(usize, Result<Option<NaiveDateTime>, ExifError>)> = missing
            .par_iter()
            .map(|&i| (i, exif::read_taken(Path::new(&files[i].path))))
            .collect();
        let mut entries = Vec::new();
        for (i, result) in read {
            let f = files[i];
            let (value, store) = match result {
                Ok(Some(date)) => (
                    ExifResult::Date(date),
                    Some(Some(date.and_utc().timestamp())),
                ),
                Ok(None) => (ExifResult::NoDate, Some(None)),
                Err(ExifError::NotLocal) => (ExifResult::Cloud, None),
                // Gesperrt oder nicht lesbar: ohne Datum, aber nicht merken.
                Err(ExifError::Io(_)) => (ExifResult::NoDate, None),
            };
            if let Some(taken) = store {
                entries.push(ExifEntry {
                    path_key: f.key.clone(),
                    size: f.size,
                    mtime: f.mtime,
                    taken,
                });
            }
            results[i] = Some(value);
        }
        // Ein Cache-Fehler kostet nur Zeit beim nächsten Lauf, nie das Ergebnis.
        let _ = index.exif_store(&entries);
        results
            .into_iter()
            .map(|r| r.unwrap_or(ExifResult::NoDate))
            .collect()
    }
}

/// Echter Inhaltsvergleich: Voll-Hash beider Dateien (einmal je Datei gemerkt).
#[derive(Default)]
pub struct LiveContent {
    hashes: Mutex<HashMap<String, Option<Vec<u8>>>>,
}

impl LiveContent {
    fn hash_of(&self, row: &FileRow) -> Option<Vec<u8>> {
        if let Some(known) = self.hashes.lock().ok()?.get(&row.key) {
            return known.clone();
        }
        let hash = hasher::open_read(Path::new(&row.path))
            .and_then(|mut f| hasher::full_hash(&mut f, row.size.max(0) as u64))
            .ok();
        self.hashes
            .lock()
            .ok()?
            .insert(row.key.clone(), hash.clone());
        hash
    }
}

impl ContentSource for LiveContent {
    fn same_content(&self, a: &FileRow, b: &FileRow) -> bool {
        if a.size != b.size || a.cloud_only || b.cloud_only {
            return false;
        }
        match (self.hash_of(a), self.hash_of(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
    }
}
