//! Erkennung doppelter Ordner: reine Analyse auf dem Index, ohne Dateizugriff.
//!
//! Ein Merkle-Fingerabdruck wird von unten nach oben berechnet: je Ordner aus den Kindern
//! (Datei: Name, Größe, voller Hash; Unterordner: Name, Fingerabdruck). Der Name des Ordners
//! selbst geht nicht ein. Junk-Dateien und leere Unterordner zählen nicht mit.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use xxhash_rust::xxh3::xxh3_128;

use crate::change::junk::is_builtin_junk_name;
use crate::change::protect::TOOL_DIR;
use crate::change::tree::Tree;
use crate::index::FileHash;
use crate::paths;

/// Optionen der Erkennung.
#[derive(Debug, Clone)]
pub struct FolderDupOptions {
    /// Ordner-Schlüssel der Wurzel: nie Mitglied einer Gruppe.
    pub root_key: String,
}

/// Ordner mit gleichem Inhalt (mindestens zwei Mitglieder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactGroup {
    /// Ordner-Schlüssel, sortiert.
    pub members: Vec<String>,
    pub fingerprint: u128,
    /// Anzahl der verglichenen (nicht ignorierten) Dateien je Mitglied.
    pub files: u64,
    /// Summe ihrer Größen je Mitglied.
    pub bytes: u64,
    /// Verschachtelungstiefe der zählenden Unterordner (0 = nur Dateien). Ein Vorfahre hat
    /// immer eine größere Höhe als jeder Nachfahre; Mitglieder einer Gruppe haben dieselbe.
    pub height: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderDupAnalysis {
    pub groups: Vec<ExactGroup>,
    /// Ordner, die wegen Cloud-Platzhaltern, Links, Lücken im Index oder fehlender Hashes
    /// nicht vergleichbar sind.
    pub unverifiable: usize,
}

/// Datei, die beim Vergleich nicht zählt (Müll-Kategorien, `desktop.ini`).
pub fn is_ignored_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("desktop.ini") || is_builtin_junk_name(name)
}

/// Ergebnis des Fingerabdrucks für einen Ordner.
#[derive(Clone, Copy)]
enum Print {
    /// Nicht vergleichbar (Cloud, Link, Lücke im Index, Datei ohne Hash).
    Unverifiable,
    /// Nichts Vergleichbares darin (leer oder nur Junk).
    Empty,
    Value {
        fingerprint: u128,
        files: u64,
        bytes: u64,
        /// Mindestens eine Datei mit Größe > 0.
        data: bool,
        /// Verschachtelungstiefe der zählenden Unterordner (0 = nur Dateien).
        height: u32,
    },
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

/// Der Schlüssel ist der Werkzeugordner `.ordner-cleanup` (Quarantäne, Journal) oder liegt darin.
fn in_tool_dir(key: &str) -> bool {
    key.split('\\').any(|part| part == TOOL_DIR)
}

fn dir_name(path: &str) -> String {
    path.rsplit('\\').next().unwrap_or(path).to_lowercase()
}

fn print_dir(
    tree: &Tree,
    key: &str,
    hashes: &HashMap<String, FileHash>,
    done: &HashMap<&str, Print>,
) -> Print {
    if !tree.complete(key) {
        return Print::Unverifiable;
    }
    let (mut files, mut bytes, mut data) = (0u64, 0u64, false);
    let mut height = 0u32;
    let mut entries: Vec<String> = Vec::new();
    for file in tree.files(key) {
        if is_ignored_name(&file.name) {
            continue;
        }
        if file.cloud_only || file.is_link {
            return Print::Unverifiable;
        }
        let size = file.size.max(0);
        let hash = if size == 0 {
            "0".to_string()
        } else {
            match hashes.get(&file.key) {
                Some(h) => hex(&h.hash),
                None => return Print::Unverifiable,
            }
        };
        files += 1;
        bytes += size as u64;
        data |= size > 0;
        entries.push(format!("F|{}|{size}|{hash}", file.name.to_lowercase()));
    }
    for child in tree.children(key) {
        if in_tool_dir(child) {
            continue;
        }
        match done
            .get(child.as_str())
            .copied()
            .unwrap_or(Print::Unverifiable)
        {
            Print::Unverifiable => return Print::Unverifiable,
            Print::Empty => {}
            Print::Value {
                fingerprint,
                files: f,
                bytes: b,
                data: dt,
                height: child_height,
            } => {
                let name = tree
                    .row(child)
                    .map_or_else(String::new, |r| dir_name(&r.path));
                files += f;
                bytes += b;
                data |= dt;
                height = height.max(child_height + 1);
                entries.push(format!("D|{name}|{fingerprint:032x}"));
            }
        }
    }
    if entries.is_empty() {
        return Print::Empty;
    }
    entries.sort();
    Print::Value {
        fingerprint: xxh3_128(
            entries
                .join(
                    "
",
                )
                .as_bytes(),
        ),
        files,
        bytes,
        data,
        height,
    }
}

pub fn analyze(
    tree: &Tree,
    hashes: &HashMap<String, FileHash>,
    opts: &FolderDupOptions,
) -> FolderDupAnalysis {
    // Kinder vor Eltern: Ein Kind-Schlüssel ist immer länger als der des Elternordners.
    let mut order: Vec<&str> = tree.dir_keys().collect();
    order.sort_by_key(|k| std::cmp::Reverse(k.len()));
    let mut done: HashMap<&str, Print> = HashMap::new();
    for key in order {
        let print = print_dir(tree, key, hashes, &done);
        done.insert(key, print);
    }

    let mut by_print: HashMap<u128, (Vec<String>, u64, u64, u32)> = HashMap::new();
    let mut unverifiable = 0;
    for (key, print) in &done {
        if *key == opts.root_key {
            continue;
        }
        match print {
            Print::Unverifiable => unverifiable += 1,
            Print::Empty => {}
            Print::Value {
                fingerprint,
                files,
                bytes,
                data,
                height,
            } => {
                if *data && !in_tool_dir(key) {
                    by_print
                        .entry(*fingerprint)
                        .or_insert_with(|| (Vec::new(), *files, *bytes, *height))
                        .0
                        .push((*key).to_string());
                }
            }
        }
    }
    let mut groups: Vec<ExactGroup> = by_print
        .into_iter()
        .filter(|(_, (members, _, _, _))| members.len() >= 2)
        .map(|(fingerprint, (mut members, files, bytes, height))| {
            members.sort();
            ExactGroup {
                members,
                fingerprint,
                files,
                bytes,
                height,
            }
        })
        .collect();
    groups.sort_by(|a, b| a.members.cmp(&b.members));
    FolderDupAnalysis {
        groups,
        unverifiable,
    }
}

/// Metadaten-Fingerabdruck für die Prüfung des behaltenen Ordners beim Apply:
/// Einträge `(relativer Pfad, Größe, mtime)` nicht ignorierter Dateien.
pub fn meta_fingerprint(entries: impl Iterator<Item = (String, i64, i64)>) -> u128 {
    let mut lines: Vec<String> = entries
        .map(|(rel, size, mtime)| format!("{}|{size}|{mtime}", rel.to_lowercase()))
        .collect();
    lines.sort();
    xxh3_128(
        lines
            .join(
                "
",
            )
            .as_bytes(),
    )
}

const STRONG_WORDS: [&str; 4] = ["kopie", "copy", "backup", "sicherung"];
const WEAK_WORDS: [&str; 3] = ["alt", "old", "bak"];

/// 0 = normaler Name; höher = eher eine Kopie. Wörter zählen nur als ganze Wörter
/// („Altbau“ und „Copyright“ sind normale Namen), eigene Muster als Teilstring.
pub fn copy_name_score(name: &str, extra: &[String]) -> u8 {
    let lower = name.to_lowercase();
    let mut score = 0u8;
    for word in lower.split(|c: char| !c.is_alphanumeric()) {
        if STRONG_WORDS.contains(&word) {
            score = score.saturating_add(2);
        } else if WEAK_WORDS.contains(&word) {
            score = score.saturating_add(1);
        }
    }
    let trimmed = lower.trim_end();
    if let Some(inner) = trimmed
        .strip_suffix(')')
        .and_then(|t| t.rsplit_once('(').map(|(_, n)| n))
    {
        if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()) {
            score = score.saturating_add(1);
        }
    }
    for pattern in extra {
        let pattern = pattern.trim().to_lowercase();
        if !pattern.is_empty() && lower.contains(&pattern) {
            score = score.saturating_add(2);
        }
    }
    score
}

/// Tiefe eines Ordner-Schlüssels (Anzahl der Backslashes).
pub fn depth(key: &str) -> usize {
    key.matches('\\').count()
}

/// `key` liegt auf oder unter einem Ordner aus `set` (Ordner-Schlüssel mit abschließendem `\`).
pub fn within(set: &HashSet<String>, key: &str) -> bool {
    let mut current = key;
    loop {
        if set.contains(current) {
            return true;
        }
        match current.trim_end_matches('\\').rfind('\\') {
            Some(i) => current = &current[..=i],
            None => return false,
        }
    }
}

/// Reihenfolge, in der Gruppen von oben nach unten abgearbeitet werden: größere Höhe zuerst
/// (eine Gruppe mit Vorfahren-Mitgliedern kommt immer vor der mit deren Nachfahren), dann das
/// flachste Mitglied, viele Bytes, Pfad.
pub fn cmp_groups(a: &ExactGroup, b: &ExactGroup) -> Ordering {
    let top = |g: &ExactGroup| g.members.iter().map(|m| depth(m)).min().unwrap_or(0);
    b.height
        .cmp(&a.height)
        .then_with(|| top(a).cmp(&top(b)))
        .then(b.bytes.cmp(&a.bytes))
        .then_with(|| a.members.cmp(&b.members))
}

/// Nur oberste Gruppen: Mitglieder, die in einem Ordner einer höheren Gruppe liegen, entfallen;
/// bleiben weniger als zwei, entfällt die Gruppe. (Der Planer arbeitet ebenso von oben nach unten.)
pub fn top_level_groups(groups: &[ExactGroup]) -> Vec<ExactGroup> {
    let mut sorted: Vec<&ExactGroup> = groups.iter().collect();
    sorted.sort_by(|a, b| cmp_groups(a, b));
    let mut covered: HashSet<String> = HashSet::new();
    let mut result = Vec::new();
    for group in sorted {
        let members: Vec<String> = group
            .members
            .iter()
            .filter(|m| !within(&covered, m))
            .cloned()
            .collect();
        if members.len() < 2 {
            continue;
        }
        covered.extend(members.iter().cloned());
        result.push(ExactGroup {
            members,
            ..group.clone()
        });
    }
    result
}

/// Duplikat-Gruppen mit mehr Mitgliedern werden für die Paarbildung übersprungen (dieselbe
/// Lizenzdatei hundertfach würde die Paaranzahl quadratisch wachsen lassen).
pub const MAX_PARTIAL_GROUP: usize = 32;
/// Obergrenze für Ordnerpaare; danach bricht die Paarbildung ab (Bericht: „unvollständig“).
pub const MAX_PARTIAL_PAIRS: usize = 200_000;

#[derive(Debug, Clone)]
pub struct PartialOptions {
    pub root_key: String,
    /// Mindestanteil gemeinsamer Bytes am kleineren Ordner.
    pub threshold: f64,
    pub max_group: usize,
    pub max_pairs: usize,
}

impl PartialOptions {
    pub fn new(root_key: String, threshold: f64) -> Self {
        Self {
            root_key,
            threshold,
            max_group: MAX_PARTIAL_GROUP,
            max_pairs: MAX_PARTIAL_PAIRS,
        }
    }
}

/// Zwei Ordner mit vielen gemeinsamen Dateien (gleicher Hash, Pfad egal).
#[derive(Debug, Clone, PartialEq)]
pub struct PartialPair {
    /// Ordner-Schlüssel, `a < b`.
    pub a: String,
    pub b: String,
    pub shared_bytes: u64,
    /// Gemeinsame Bytes geteilt durch die Bytes des kleineren Ordners.
    pub ratio: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartialResult {
    /// Sortiert nach gemeinsamen Bytes absteigend.
    pub pairs: Vec<PartialPair>,
    /// Die Paarbildung wurde wegen `max_pairs` abgebrochen.
    pub incomplete: bool,
}

fn parent_of<'a>(tree: &'a Tree, key: &str) -> Option<&'a str> {
    tree.row(key).and_then(|r| r.parent_key.as_deref())
}

fn ordered(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Ordner, die über gemeinsame Dateien verwandt sind, aber keine exakten Kopien: nur ein Hinweis
/// im Bericht, nie Teil eines Plans. Arbeitet nur auf dem Index.
pub fn partial_pairs(
    tree: &Tree,
    hashes: &HashMap<String, FileHash>,
    exact: &[ExactGroup],
    opts: &PartialOptions,
) -> PartialResult {
    let skip_dir = |key: &str| key == opts.root_key || in_tool_dir(key);

    // Hash → (Ordner, Größe) der zählenden Dateien; wirksame Bytes je Ordner (ohne Junk).
    let mut by_hash: HashMap<&[u8], Vec<(&str, u64)>> = HashMap::new();
    let mut own: HashMap<&str, u64> = HashMap::new();
    for key in tree.dir_keys() {
        for file in tree.files(key) {
            let size = file.size.max(0) as u64;
            if size == 0 || is_ignored_name(&file.name) {
                continue;
            }
            *own.entry(key).or_default() += size;
            if let Some(h) = hashes.get(&file.key) {
                by_hash.entry(&h.hash).or_default().push((key, size));
            }
        }
    }
    let mut effective: HashMap<&str, u64> = own.clone();
    let mut order: Vec<&str> = tree.dir_keys().collect();
    order.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for key in order {
        let bytes = effective.get(key).copied().unwrap_or(0);
        if let Some(parent) = parent_of(tree, key) {
            *effective.entry(tree_key(tree, parent)).or_default() += bytes;
        }
    }

    let group_of: HashMap<&str, usize> = exact
        .iter()
        .enumerate()
        .flat_map(|(i, g)| g.members.iter().map(move |m| (m.as_str(), i)))
        .collect();

    let mut shared: HashMap<(String, String), u64> = HashMap::new();
    let mut incomplete = false;
    'groups: for members in by_hash.values() {
        if members.len() < 2 || members.len() > opts.max_group {
            continue;
        }
        let size = members[0].1;
        // Wie viele Mitglieder liegen unter jedem Ordner (Wurzel ausgenommen).
        let mut count: HashMap<&str, u64> = HashMap::new();
        for (dir, _) in members {
            let mut current = Some(*dir);
            while let Some(d) = current.filter(|d| *d != opts.root_key) {
                *count.entry(d).or_default() += 1;
                current = parent_of(tree, d);
            }
        }
        let mut pairs: HashSet<(String, String)> = HashSet::new();
        for (i, (da, _)) in members.iter().enumerate() {
            for (db, _) in &members[i + 1..] {
                let (mut x, mut y) = (*da, *db);
                loop {
                    if skip_dir(x) || skip_dir(y) || paths::is_under(x, y) || paths::is_under(y, x)
                    {
                        break;
                    }
                    pairs.insert(ordered(x, y));
                    match (parent_of(tree, x), parent_of(tree, y)) {
                        (Some(px), Some(py)) => {
                            x = tree_key(tree, px);
                            y = tree_key(tree, py);
                        }
                        _ => break,
                    }
                }
            }
        }
        for (a, b) in pairs {
            let both = count
                .get(a.as_str())
                .copied()
                .unwrap_or(0)
                .min(count.get(b.as_str()).copied().unwrap_or(0));
            *shared.entry((a, b)).or_default() += both * size;
            if shared.len() > opts.max_pairs {
                incomplete = true;
                break 'groups;
            }
        }
    }

    let ratio_of = |a: &str, b: &str, bytes: u64| -> f64 {
        let smaller = effective
            .get(a)
            .copied()
            .unwrap_or(0)
            .min(effective.get(b).copied().unwrap_or(0));
        if smaller == 0 {
            0.0
        } else {
            bytes as f64 / smaller as f64
        }
    };
    let candidates: HashMap<(String, String), f64> = shared
        .iter()
        .filter(|((a, b), _)| {
            !matches!((group_of.get(a.as_str()), group_of.get(b.as_str())), (Some(x), Some(y)) if x == y)
        })
        .map(|((a, b), bytes)| ((a.clone(), b.clone()), ratio_of(a, b, *bytes)))
        .filter(|(_, ratio)| *ratio >= opts.threshold)
        .collect();

    let mut pairs: Vec<PartialPair> = candidates
        .iter()
        .filter(|((a, b), _)| {
            // Nur oberste Paare: Liegt das Paar der Elternordner auch über der Schwelle, entfällt dieses.
            match (parent_of(tree, a), parent_of(tree, b)) {
                (Some(pa), Some(pb)) if pa != pb => !candidates.contains_key(&ordered(pa, pb)),
                _ => true,
            }
        })
        .map(|((a, b), ratio)| PartialPair {
            a: a.clone(),
            b: b.clone(),
            shared_bytes: shared[&(a.clone(), b.clone())],
            ratio: *ratio,
        })
        .collect();
    pairs.sort_by(|x, y| {
        y.shared_bytes
            .cmp(&x.shared_bytes)
            .then_with(|| x.a.cmp(&y.a))
            .then_with(|| x.b.cmp(&y.b))
    });
    PartialResult { pairs, incomplete }
}

/// Der im Baum gespeicherte Schlüssel (mit der Lebensdauer des Baums) zu einem Schlüsseltext.
fn tree_key<'a>(tree: &'a Tree, key: &str) -> &'a str {
    tree.row(key).map_or("", |r| r.key.as_str())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::index::{DirRow, FileRow};
    use crate::paths;

    const ROOT: &str = r"D:\Daten";

    fn k(path: &str) -> String {
        paths::dir_key(Path::new(path))
    }

    fn d(path: &str, entries: i64) -> DirRow {
        let p = Path::new(path);
        DirRow {
            path: path.into(),
            key: k(path),
            parent_key: p.parent().map(paths::dir_key),
            summary: false,
            attrs: 0x10,
            mtime: None,
            is_link: false,
            direct_entries: entries,
            read_error: false,
        }
    }

    fn f(path: &str, size: i64) -> FileRow {
        let p = Path::new(path);
        FileRow {
            path: path.into(),
            key: paths::path_key(p),
            dir_key: paths::dir_key(p.parent().unwrap()),
            name: p.file_name().unwrap().to_string_lossy().into_owned(),
            size,
            mtime: 1,
            ctime: None,
            first_seen: None,
            attrs: 0x20,
            cloud_only: false,
            is_link: false,
        }
    }

    /// Hash aus einem Byte, damit Tests gleiche/verschiedene Inhalte benennen können.
    fn h(b: u8) -> FileHash {
        FileHash {
            hash: vec![b; 16],
            identity: None,
            nlinks: 1,
        }
    }

    /// Baut Baum und Hash-Tabelle; eine Datei ist (Pfad, Größe, Hash-Byte). Ordner werden aus
    /// den Dateipfaden abgeleitet, `direct_entries` stimmt automatisch.
    fn build(extra_dirs: &[&str], files: &[(&str, i64, u8)]) -> (Tree, HashMap<String, FileHash>) {
        let mut dirs: std::collections::BTreeSet<String> = extra_dirs
            .iter()
            .map(|s| s.to_string())
            .chain([ROOT.to_string()])
            .collect();
        for (path, _, _) in files {
            let mut cur = Path::new(path).parent();
            while let Some(p) = cur {
                let s = p.to_string_lossy().into_owned();
                if s.len() < ROOT.len() {
                    break;
                }
                dirs.insert(s);
                cur = p.parent();
            }
        }
        let dir_rows: Vec<DirRow> = dirs
            .iter()
            .map(|dir| {
                let entries = files
                    .iter()
                    .filter(|(p, _, _)| Path::new(p).parent() == Some(Path::new(dir)))
                    .count()
                    + dirs
                        .iter()
                        .filter(|o| Path::new(o).parent() == Some(Path::new(dir)))
                        .count();
                d(dir, entries as i64)
            })
            .collect();
        let rows: Vec<FileRow> = files.iter().map(|(p, s, _)| f(p, *s)).collect();
        let map = files
            .iter()
            .filter(|(_, s, _)| *s > 0)
            .map(|(p, _, b)| (paths::path_key(Path::new(p)), h(*b)))
            .collect();
        (Tree::new(dir_rows, rows), map)
    }

    fn run(tree: &Tree, hashes: &HashMap<String, FileHash>) -> FolderDupAnalysis {
        analyze(tree, hashes, &FolderDupOptions { root_key: k(ROOT) })
    }

    fn members(a: &FolderDupAnalysis) -> Vec<Vec<String>> {
        a.groups.iter().map(|g| g.members.clone()).collect()
    }

    #[test]
    fn gleicher_inhalt_ist_eine_gruppe_trotz_anderem_ordnernamen() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\Projekt\a.txt", 5, 1),
                (r"D:\Daten\Projekt\sub\b.txt", 7, 2),
                (r"D:\Daten\Kopie von Projekt\a.txt", 5, 1),
                (r"D:\Daten\Kopie von Projekt\sub\b.txt", 7, 2),
            ],
        );
        let a = run(&tree, &hashes);
        let top: Vec<_> = a
            .groups
            .iter()
            .filter(|g| g.members.contains(&k(r"D:\Daten\Projekt")))
            .collect();
        assert_eq!(top.len(), 1);
        assert_eq!(
            top[0].members,
            [k(r"D:\Daten\Kopie von Projekt"), k(r"D:\Daten\Projekt")]
        );
        assert_eq!((top[0].files, top[0].bytes), (2, 12));
    }

    #[test]
    fn unterordner_paare_sind_eigene_gruppen_der_planer_waehlt_oben() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\sub\b.txt", 7, 2),
                (r"D:\Daten\B\sub\b.txt", 7, 2),
            ],
        );
        let a = run(&tree, &hashes);
        assert_eq!(a.groups.len(), 2, "A/B und A\\sub/B\\sub");
    }

    #[test]
    fn ein_unterschiedliches_byte_oder_ein_anderer_pfad_trennt() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\a.txt", 5, 1),
                (r"D:\Daten\B\a.txt", 5, 9),
                (r"D:\Daten\C\a.txt", 5, 1),
                (r"D:\Daten\D\anders.txt", 5, 1),
            ],
        );
        let a = run(&tree, &hashes);
        assert_eq!(members(&a), [vec![k(r"D:\Daten\A"), k(r"D:\Daten\C")]]);
    }

    #[test]
    fn gross_und_kleinschreibung_der_namen_ist_egal() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\Bild.JPG", 5, 1),
                (r"D:\Daten\B\bild.jpg", 5, 1),
            ],
        );
        assert_eq!(run(&tree, &hashes).groups.len(), 1);
    }

    #[test]
    fn junk_und_leere_unterordner_zaehlen_nicht_mit() {
        let (tree, hashes) = build(
            &[r"D:\Daten\B\leer"],
            &[
                (r"D:\Daten\A\a.txt", 5, 1),
                (r"D:\Daten\B\a.txt", 5, 1),
                (r"D:\Daten\B\Thumbs.db", 99, 7),
                (r"D:\Daten\B\desktop.ini", 3, 8),
            ],
        );
        let a = run(&tree, &hashes);
        assert_eq!(members(&a), [vec![k(r"D:\Daten\A"), k(r"D:\Daten\B")]]);
        assert_eq!((a.groups[0].files, a.groups[0].bytes), (1, 5));
    }

    #[test]
    fn nur_junk_oder_nur_leere_dateien_sind_kein_kandidat() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\Thumbs.db", 5, 1),
                (r"D:\Daten\B\Thumbs.db", 5, 1),
                (r"D:\Daten\C\leer.txt", 0, 0),
                (r"D:\Daten\D\leer.txt", 0, 0),
            ],
        );
        assert!(run(&tree, &hashes).groups.is_empty());
    }

    #[test]
    fn datei_ohne_hash_macht_den_ordner_nicht_vergleichbar() {
        let (tree, mut hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\a.txt", 5, 1),
                (r"D:\Daten\B\a.txt", 5, 1),
                (r"D:\Daten\C\a.txt", 5, 1),
            ],
        );
        hashes.remove(&paths::path_key(Path::new(r"D:\Daten\C\a.txt")));
        let a = run(&tree, &hashes);
        assert_eq!(members(&a), [vec![k(r"D:\Daten\A"), k(r"D:\Daten\B")]]);
        assert_eq!(a.unverifiable, 1);
    }

    #[test]
    fn cloud_datei_macht_ordner_nicht_vergleichbar_und_vererbt_nach_oben() {
        let (tree, hashes) = build(
            &[],
            &[(r"D:\Daten\A\x\a.txt", 5, 1), (r"D:\Daten\B\x\a.txt", 5, 1)],
        );
        let mut files = vec![f(r"D:\Daten\A\x\a.txt", 5), f(r"D:\Daten\B\x\a.txt", 5)];
        files[1].cloud_only = true;
        let dirs: Vec<DirRow> = tree
            .dir_keys()
            .map(|key| tree.row(key).unwrap().clone())
            .collect();
        let tree = Tree::new(dirs, files);
        let a = run(&tree, &hashes);
        assert!(a.groups.is_empty());
        assert_eq!(a.unverifiable, 2, "B\\x und B");
    }

    #[test]
    fn wurzel_ist_nie_mitglied() {
        let (tree, hashes) = build(&[], &[(r"D:\Daten\a.txt", 5, 1)]);
        assert!(run(&tree, &hashes).groups.is_empty());
    }

    #[test]
    fn ignorier_regel() {
        for name in [
            "Thumbs.db",
            "desktop.ini",
            "DESKTOP.INI",
            ".DS_Store",
            "~$x.docx",
            "a.tmp",
        ] {
            assert!(is_ignored_name(name), "{name}");
        }
        for name in ["a.txt", "setup.exe", "desktop.ini.txt"] {
            assert!(!is_ignored_name(name), "{name}");
        }
    }

    fn entry(path: &str, size: i64, mtime: i64) -> (String, i64, i64) {
        (path.to_string(), size, mtime)
    }

    #[test]
    fn meta_fingerprint_ist_reihenfolge_und_schreibweise_unabhaengig_und_aendert_sich() {
        let a = meta_fingerprint([entry("A.txt", 5, 10), entry("sub\\b.txt", 7, 20)].into_iter());
        let b = meta_fingerprint([entry("SUB\\B.TXT", 7, 20), entry("a.txt", 5, 10)].into_iter());
        assert_eq!(a, b);
        let c = meta_fingerprint([entry("A.txt", 5, 11), entry("sub\\b.txt", 7, 20)].into_iter());
        assert_ne!(a, c);
        let e = meta_fingerprint([entry("A.txt", 6, 10), entry("sub\\b.txt", 7, 20)].into_iter());
        assert_ne!(a, e);
    }

    #[test]
    fn kopie_namen_haben_hoeheren_score() {
        for name in [
            "Kopie von Projekt",
            "Projekt - Kopie",
            "Projekt (2)",
            "Projekt Copy",
            "Backup Projekt",
            "Projekt_backup",
            "Sicherung 2020",
            "Projekt alt",
            "Projekt_old",
            "Projekt.bak",
            "projekt_bak",
        ] {
            assert!(copy_name_score(name, &[]) > 0, "{name}");
        }
    }

    #[test]
    fn normale_namen_haben_score_null() {
        for name in ["Projekt", "Altbau", "Copyright", "Bold", "Rückblick 2020"] {
            assert_eq!(copy_name_score(name, &[]), 0, "{name}");
        }
    }

    #[test]
    fn eigene_muster_zaehlen_case_insensitiv() {
        assert_eq!(copy_name_score("Entwurf Archiv", &[]), 0);
        assert!(copy_name_score("Entwurf Archiv", &["archiv".into()]) > 0);
        assert!(copy_name_score("ENTWURF ARCHIV", &["Archiv".into()]) > 0);
    }

    #[test]
    fn mehr_kopie_hinweise_ergeben_hoeheren_score() {
        assert!(
            copy_name_score("Kopie von Projekt (2)", &[]) > copy_name_score("Projekt (2)", &[])
        );
    }

    // --- teilweise gleiche Ordner (nur Bericht) ---

    fn partial(tree: &Tree, hashes: &HashMap<String, FileHash>, threshold: f64) -> PartialResult {
        let analysis = run(tree, hashes);
        partial_pairs(
            tree,
            hashes,
            &analysis.groups,
            &PartialOptions::new(k(ROOT), threshold),
        )
    }

    fn pair_names(result: &PartialResult) -> Vec<(String, String)> {
        result
            .pairs
            .iter()
            .map(|p| (p.a.clone(), p.b.clone()))
            .collect()
    }

    #[test]
    fn paar_mit_85_prozent_gemeinsamen_bytes_wird_gemeldet_75_nicht() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\gross.bin", 85, 1),
                (r"D:\Daten\A\nur_a.bin", 15, 2),
                (r"D:\Daten\B\gross.bin", 85, 1),
                (r"D:\Daten\B\nur_b.bin", 15, 3),
            ],
        );
        let result = partial(&tree, &hashes, 0.8);
        assert_eq!(pair_names(&result), [(k(r"D:\Daten\A"), k(r"D:\Daten\B"))]);
        assert_eq!(result.pairs[0].shared_bytes, 85);
        assert!((result.pairs[0].ratio - 0.85).abs() < 1e-9);

        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\gross.bin", 75, 1),
                (r"D:\Daten\A\nur_a.bin", 25, 2),
                (r"D:\Daten\B\gross.bin", 75, 1),
                (r"D:\Daten\B\nur_b.bin", 25, 3),
            ],
        );
        assert!(partial(&tree, &hashes, 0.8).pairs.is_empty());
    }

    #[test]
    fn exakte_gruppen_erscheinen_nicht_als_teilweise_gleich() {
        let (tree, hashes) = build(
            &[],
            &[(r"D:\Daten\A\a.bin", 50, 1), (r"D:\Daten\B\a.bin", 50, 1)],
        );
        assert!(partial(&tree, &hashes, 0.8).pairs.is_empty());
    }

    #[test]
    fn nur_oberste_paare_unterordner_paare_im_gelisteten_paar_entfallen() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\sub\gross.bin", 85, 1),
                (r"D:\Daten\A\nur_a.bin", 15, 2),
                (r"D:\Daten\B\sub\gross.bin", 85, 1),
                (r"D:\Daten\B\nur_b.bin", 15, 3),
            ],
        );
        let result = partial(&tree, &hashes, 0.8);
        assert_eq!(
            pair_names(&result),
            [(k(r"D:\Daten\A"), k(r"D:\Daten\B"))],
            "A\\sub/B\\sub ist exakt gleich und im Paar A/B enthalten"
        );
    }

    #[test]
    fn dateien_im_selben_ordner_oder_ordner_in_ordner_bilden_kein_paar() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\x.bin", 50, 1),
                (r"D:\Daten\A\y.bin", 50, 1),
                (r"D:\Daten\A\sub\z.bin", 50, 1),
            ],
        );
        assert!(partial(&tree, &hashes, 0.5).pairs.is_empty());
    }

    #[test]
    fn sehr_grosse_duplikat_gruppen_werden_fuer_die_paarbildung_uebersprungen() {
        let mut files: Vec<(String, i64, u8)> = (0..=MAX_PARTIAL_GROUP)
            .map(|i| (format!(r"D:\Daten\L{i}\lizenz.txt"), 50, 1))
            .collect();
        files.push((r"D:\Daten\A\x.bin".to_string(), 50, 2));
        files.push((r"D:\Daten\B\x.bin".to_string(), 50, 2));
        let refs: Vec<(&str, i64, u8)> =
            files.iter().map(|(p, s, h)| (p.as_str(), *s, *h)).collect();
        let (tree, hashes) = build(&[], &refs);
        let result = partial(&tree, &hashes, 0.5);
        assert!(
            result
                .pairs
                .iter()
                .all(|p| !p.a.contains(r"\l") || !p.b.contains(r"\l")),
            "Lizenz-Ordner bilden keine Paare"
        );
    }

    #[test]
    fn zu_viele_paare_brechen_ab_und_werden_als_unvollstaendig_vermerkt() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\g.bin", 85, 1),
                (r"D:\Daten\A\n.bin", 15, 2),
                (r"D:\Daten\B\g.bin", 85, 1),
                (r"D:\Daten\B\m.bin", 15, 3),
                (r"D:\Daten\C\g.bin", 85, 1),
            ],
        );
        let analysis = run(&tree, &hashes);
        let result = partial_pairs(
            &tree,
            &hashes,
            &analysis.groups,
            &PartialOptions {
                max_pairs: 1,
                ..PartialOptions::new(k(ROOT), 0.5)
            },
        );
        assert!(result.incomplete);
    }

    /// W = P\Y\x = P\Z\x (Höhe 0); P\Y = P\Z (Höhe 1) enthalten je eine Kopie davon.
    fn wrapped() -> (Tree, HashMap<String, FileHash>) {
        build(
            &[],
            &[
                (r"D:\Daten\W\f.txt", 7, 1),
                (r"D:\Daten\P\Y\x\f.txt", 7, 1),
                (r"D:\Daten\P\Z\x\f.txt", 7, 1),
            ],
        )
    }

    #[test]
    fn hoehe_zaehlt_die_verschachtelung_zaehlender_unterordner() {
        let (tree, hashes) = wrapped();
        let a = run(&tree, &hashes);
        let height_of = |member: &str| {
            a.groups
                .iter()
                .find(|g| g.members.contains(&k(member)))
                .map(|g| g.height)
        };
        assert_eq!(height_of(r"D:\Daten\W"), Some(0));
        assert_eq!(height_of(r"D:\Daten\P\Y"), Some(1));
    }

    #[test]
    fn gruppen_mit_vorfahren_kommen_vor_ihren_nachfahren() {
        let (tree, hashes) = wrapped();
        let a = run(&tree, &hashes);
        let mut sorted: Vec<&ExactGroup> = a.groups.iter().collect();
        sorted.sort_by(|a, b| cmp_groups(a, b));
        // Die Gruppe mit dem flachsten Mitglied (W) steht trotzdem hinter {P\Y, P\Z}.
        assert_eq!(sorted[0].members, [k(r"D:\Daten\P\Y"), k(r"D:\Daten\P\Z")]);
    }

    #[test]
    fn oberste_gruppen_nehmen_die_hoehere_gruppe_und_lassen_gedeckte_mitglieder_weg() {
        let (tree, hashes) = wrapped();
        let top = top_level_groups(&run(&tree, &hashes).groups);
        assert_eq!(top.len(), 1, "{top:?}");
        assert_eq!(top[0].members, [k(r"D:\Daten\P\Y"), k(r"D:\Daten\P\Z")]);
    }

    #[test]
    fn quarantaene_kopie_unter_dem_werkzeugordner_bildet_keine_gruppe() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\Projekt\a.txt", 7, 1),
                (
                    r"D:\Daten\.ordner-cleanup\quarantine\r1\Projekt\a.txt",
                    7,
                    1,
                ),
            ],
        );
        let a = run(&tree, &hashes);
        assert!(a.groups.is_empty(), "{:?}", a.groups);
    }

    #[test]
    fn werkzeugordner_aendert_den_fingerabdruck_des_elternordners_nicht() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\x.txt", 7, 1),
                (r"D:\Daten\B\x.txt", 7, 1),
                (r"D:\Daten\A\.ordner-cleanup\q.txt", 7, 2),
            ],
        );
        let a = run(&tree, &hashes);
        assert_eq!(a.groups.len(), 1, "{:?}", a.groups);
        assert_eq!(a.groups[0].members, [k(r"D:\Daten\A"), k(r"D:\Daten\B")]);
    }

    #[test]
    fn hex_stimmt_fuer_alle_bytewerte_mit_format_ueberein() {
        let all: Vec<u8> = (0..=255).collect();
        let expected: String = all.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex(&all), expected);
        assert_eq!(hex(&[]), "");
    }

    #[test]
    fn oberste_gruppen_lassen_unterordner_paare_gedeckter_ordner_weg() {
        let (tree, hashes) = build(
            &[],
            &[
                (r"D:\Daten\A\sub\b.txt", 7, 2),
                (r"D:\Daten\A\x.txt", 7, 9),
                (r"D:\Daten\B\sub\b.txt", 7, 2),
                (r"D:\Daten\B\x.txt", 7, 9),
            ],
        );
        let a = run(&tree, &hashes);
        assert_eq!(a.groups.len(), 2);
        let top = top_level_groups(&a.groups);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].members, [k(r"D:\Daten\A"), k(r"D:\Daten\B")]);
    }
}
