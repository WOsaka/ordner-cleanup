//! Erkennung doppelter Ordner: reine Analyse auf dem Index, ohne Dateizugriff.
//!
//! Ein Merkle-Fingerabdruck wird von unten nach oben berechnet: je Ordner aus den Kindern
//! (Datei: Name, Größe, voller Hash; Unterordner: Name, Fingerabdruck). Der Name des Ordners
//! selbst geht nicht ein. Junk-Dateien und leere Unterordner zählen nicht mit.

use std::collections::HashMap;

use xxhash_rust::xxh3::xxh3_128;

use crate::change::junk::is_builtin_junk_name;
use crate::change::tree::Tree;
use crate::index::FileHash;

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
    },
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
            } => {
                let name = tree
                    .row(child)
                    .map_or_else(String::new, |r| dir_name(&r.path));
                files += f;
                bytes += b;
                data |= dt;
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

    let mut by_print: HashMap<u128, (Vec<String>, u64, u64)> = HashMap::new();
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
            } => {
                let is_internal = tree
                    .row(key)
                    .is_some_and(|r| dir_name(&r.path) == ".ordner-cleanup");
                if *data && !is_internal {
                    by_print
                        .entry(*fingerprint)
                        .or_insert_with(|| (Vec::new(), *files, *bytes))
                        .0
                        .push((*key).to_string());
                }
            }
        }
    }
    let mut groups: Vec<ExactGroup> = by_print
        .into_iter()
        .filter(|(_, (members, _, _))| members.len() >= 2)
        .map(|(fingerprint, (mut members, files, bytes))| {
            members.sort();
            ExactGroup {
                members,
                fingerprint,
                files,
                bytes,
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
}
