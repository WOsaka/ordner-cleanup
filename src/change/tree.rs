//! In-Memory-Baum aus den Index-Listen für die Planer von `empty-dirs` und `archive`.
//!
//! Aus `direct_entries` des Scans und den im Index bekannten Kindern ergibt sich, ob ein
//! Ordner **vollständig bekannt** ist. Nur dann darf ein Ordner „leer“ oder „alt“ heißen:
//! ausgeschlossene Einträge, aufsummierte Ordner, Links und Lesefehler würden ihn sonst
//! fälschlich leer oder alt aussehen lassen.

use std::collections::HashMap;
use std::path::Path;

use super::protect::Protector;

use crate::index::{DirRow, FileRow};

/// Kennzahlen eines Teilbaums über alle im Index bekannten Dateien.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubtreeStats {
    pub files: u64,
    pub bytes: u64,
    /// Jüngste Änderungszeit aller Dateien (100-ns-Ticks seit Unix-Epoche).
    pub newest_mtime: Option<i64>,
    /// Mindestens ein Cloud-Platzhalter.
    pub has_cloud: bool,
    /// Mindestens ein Symlink/eine Junction (Datei oder Ordner).
    pub has_link: bool,
}

struct Node {
    row: DirRow,
    children: Vec<String>,
    files: Vec<FileRow>,
    complete: bool,
    empty: bool,
    stats: SubtreeStats,
}

pub struct Tree {
    nodes: HashMap<String, Node>,
}

impl Tree {
    pub fn new(dirs: Vec<DirRow>, files: Vec<FileRow>) -> Self {
        let mut nodes: HashMap<String, Node> = dirs
            .into_iter()
            .map(|row| {
                let node = Node {
                    row,
                    children: Vec::new(),
                    files: Vec::new(),
                    complete: false,
                    empty: false,
                    stats: SubtreeStats::default(),
                };
                (node.row.key.clone(), node)
            })
            .collect();

        for file in files {
            if let Some(node) = nodes.get_mut(&file.dir_key) {
                node.files.push(file);
            }
        }
        let links: Vec<(String, String)> = nodes
            .values()
            .filter_map(|n| Some((n.row.parent_key.clone()?, n.row.key.clone())))
            .collect();
        for (parent, child) in links {
            if let Some(node) = nodes.get_mut(&parent) {
                node.children.push(child);
            }
        }

        // Kinder vor Eltern: Ein Kind-Schlüssel ist immer länger als der des Elternordners.
        let mut order: Vec<String> = nodes.keys().cloned().collect();
        order.sort_by_key(|k| std::cmp::Reverse(k.len()));
        for key in order {
            let mut node = nodes.remove(&key).expect("Schlüssel stammt aus der Map");
            node.children.sort();
            node.files.sort_by(|a, b| a.key.cmp(&b.key));
            Self::summarize(&mut node, &nodes);
            nodes.insert(key, node);
        }
        Self { nodes }
    }

    /// Berechnet `complete`, `empty` und `stats` aus den bereits fertigen Kindern.
    fn summarize(node: &mut Node, done: &HashMap<String, Node>) {
        let known = (node.files.len() + node.children.len()) as i64;
        let mut complete = !node.row.summary
            && !node.row.is_link
            && !node.row.read_error
            && node.row.direct_entries == known;
        let mut empty = complete && node.files.is_empty();
        let mut stats = SubtreeStats {
            has_link: node.row.is_link,
            ..SubtreeStats::default()
        };
        for file in &node.files {
            stats.files += 1;
            stats.bytes += file.size.max(0) as u64;
            stats.newest_mtime = stats.newest_mtime.max(Some(file.mtime));
            stats.has_cloud |= file.cloud_only;
            stats.has_link |= file.is_link;
        }
        for child in node.children.iter().filter_map(|c| done.get(c)) {
            complete &= child.complete;
            empty &= child.empty;
            stats.files += child.stats.files;
            stats.bytes += child.stats.bytes;
            stats.newest_mtime = stats.newest_mtime.max(child.stats.newest_mtime);
            stats.has_cloud |= child.stats.has_cloud;
            stats.has_link |= child.stats.has_link;
        }
        node.complete = complete;
        node.empty = empty && complete;
        node.stats = stats;
    }

    /// Alle Ordner-Schlüssel (beliebige Reihenfolge).
    pub fn dir_keys(&self) -> impl Iterator<Item = &str> {
        self.nodes.keys().map(String::as_str)
    }

    pub fn row(&self, dir: &str) -> Option<&DirRow> {
        self.nodes.get(dir).map(|n| &n.row)
    }

    /// Schlüssel der direkten Unterordner, sortiert.
    pub fn children(&self, dir: &str) -> &[String] {
        self.nodes.get(dir).map_or(&[], |n| &n.children)
    }

    /// Direkt im Ordner liegende Dateien, sortiert nach Schlüssel.
    pub fn files(&self, dir: &str) -> &[FileRow] {
        self.nodes.get(dir).map_or(&[], |n| &n.files)
    }

    /// Der Index kennt den Ordner und alles darunter vollständig: kein Summary-Ordner, kein
    /// Link, kein Lesefehler, und jeder gesehene Eintrag steht im Index.
    pub fn complete(&self, dir: &str) -> bool {
        self.nodes.get(dir).is_some_and(|n| n.complete)
    }

    /// Rekursiv leer: vollständig bekannt, ohne Dateien, nur leere Unterordner.
    pub fn is_empty(&self, dir: &str) -> bool {
        self.nodes.get(dir).is_some_and(|n| n.empty)
    }

    pub fn stats(&self, dir: &str) -> SubtreeStats {
        self.nodes.get(dir).map(|n| n.stats).unwrap_or_default()
    }
}

/// Schlüssel aller Ordner im Teilbaum (einschließlich `key`), ohne Rekursion.
pub fn subtree_dirs<'a>(tree: &'a Tree, key: &'a str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut stack = vec![key];
    while let Some(dir) = stack.pop() {
        found.push(dir);
        stack.extend(tree.children(dir).iter().map(String::as_str));
    }
    found
}

/// Etwas im Teilbaum verbietet es, den Ordner als Ganzes zu verschieben: der Ordner selbst,
/// ein Unterordner (Name, Marker, Config) oder eine Datei darin ist geschützt.
///
/// Dateien prüft `check_cached` (der Elternordner wird je Ordner einmal aufgelöst; für Planer mit
/// vielen Tausend Dateien). Der letzte Pfadteil bleibt dabei unaufgelöst: Eine Datei, die selbst
/// ein Link auf ein geschütztes Ziel ist, erkennt das nicht. Das ist gedeckt, weil Datei-Links im
/// Index `is_link` tragen (`SubtreeStats::has_link`: `archive` überspringt solche Ordner, die
/// Ordneranalyse macht sie unvergleichbar), Ordner-Links `check_inside` streng prüft und `apply`
/// immer streng mit `check` arbeitet.
pub fn contains_protected(tree: &Tree, protector: &Protector, key: &str) -> bool {
    subtree_dirs(tree, key).into_iter().any(|dir| {
        tree.row(dir)
            .is_some_and(|row| protector.check_inside(Path::new(&row.path)).is_some())
            || tree
                .files(dir)
                .iter()
                .any(|f| protector.check_cached(Path::new(&f.path)).is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths;

    fn k(path: &str) -> String {
        paths::dir_key(Path::new(path))
    }

    /// Ordner mit der Anzahl gesehener Einträge.
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

    fn f(path: &str, size: i64, mtime: i64) -> FileRow {
        let p = Path::new(path);
        FileRow {
            path: path.into(),
            key: paths::path_key(p),
            dir_key: paths::dir_key(p.parent().unwrap()),
            name: p.file_name().unwrap().to_string_lossy().into_owned(),
            size,
            mtime,
            ctime: None,
            first_seen: None,
            attrs: 0x20,
            cloud_only: false,
            is_link: false,
        }
    }

    const ROOT: &str = r"D:\Daten";

    #[test]
    fn verschachtelte_leere_ordner_sind_leer() {
        let tree = Tree::new(
            vec![
                d(ROOT, 2),
                d(r"D:\Daten\a", 1),
                d(r"D:\Daten\a\b", 0),
                d(r"D:\Daten\c", 0),
            ],
            vec![],
        );
        for dir in [ROOT, r"D:\Daten\a", r"D:\Daten\a\b", r"D:\Daten\c"] {
            assert!(tree.is_empty(&k(dir)), "{dir}");
            assert!(tree.complete(&k(dir)), "{dir}");
        }
        assert_eq!(tree.stats(&k(ROOT)), SubtreeStats::default());
    }

    #[test]
    fn datei_macht_ordner_und_vorfahren_nicht_leer_aber_nicht_die_nachbarn() {
        let tree = Tree::new(
            vec![
                d(ROOT, 2),
                d(r"D:\Daten\a", 1),
                d(r"D:\Daten\a\b", 1),
                d(r"D:\Daten\c", 0),
            ],
            vec![f(r"D:\Daten\a\b\x.txt", 5, 1)],
        );
        assert!(!tree.is_empty(&k(r"D:\Daten\a\b")));
        assert!(!tree.is_empty(&k(r"D:\Daten\a")));
        assert!(!tree.is_empty(&k(ROOT)));
        assert!(tree.is_empty(&k(r"D:\Daten\c")));
        assert!(
            tree.complete(&k(ROOT)),
            "Dateien beeinträchtigen complete nicht"
        );
    }

    #[test]
    fn ausgeschlossene_oder_fehlerhafte_eintraege_machen_den_ordner_unvollstaendig() {
        // Der Scan sah 2 Einträge, im Index steht nur einer (der andere war ausgeschlossen).
        let tree = Tree::new(
            vec![d(ROOT, 1), d(r"D:\Daten\a", 2), d(r"D:\Daten\a\x", 0)],
            vec![],
        );
        assert!(!tree.complete(&k(r"D:\Daten\a")));
        assert!(!tree.is_empty(&k(r"D:\Daten\a")));
        assert!(tree.is_empty(&k(r"D:\Daten\a\x")));
        assert!(
            !tree.complete(&k(ROOT)),
            "unvollständig vererbt sich nach oben"
        );
        assert!(!tree.is_empty(&k(ROOT)));
    }

    #[test]
    fn aufsummierter_ordner_ist_unbekannter_inhalt() {
        let mut node_modules = d(r"D:\Daten\node_modules", 3);
        node_modules.summary = true;
        let tree = Tree::new(vec![d(ROOT, 1), node_modules], vec![]);
        assert!(!tree.complete(&k(r"D:\Daten\node_modules")));
        assert!(!tree.is_empty(&k(r"D:\Daten\node_modules")));
        assert!(!tree.complete(&k(ROOT)));
        assert!(!tree.is_empty(&k(ROOT)));
    }

    #[test]
    fn link_ordner_ist_unvollstaendig_und_wird_in_den_kennzahlen_gemeldet() {
        let mut link = d(r"D:\Daten\verweis", 0);
        link.is_link = true;
        let tree = Tree::new(vec![d(ROOT, 1), link], vec![]);
        assert!(!tree.complete(&k(ROOT)));
        assert!(!tree.is_empty(&k(ROOT)));
        assert!(tree.stats(&k(ROOT)).has_link);
        assert!(!tree.stats(&k(r"D:\Daten\verweis")).has_cloud);
    }

    #[test]
    fn link_datei_und_cloud_platzhalter_stehen_in_den_kennzahlen() {
        let mut lnk = f(r"D:\Daten\a\verweis.lnk", 0, 5);
        lnk.is_link = true;
        let mut cloud = f(r"D:\Daten\b\wolke.docx", 7, 9);
        cloud.cloud_only = true;
        let tree = Tree::new(
            vec![d(ROOT, 2), d(r"D:\Daten\a", 1), d(r"D:\Daten\b", 1)],
            vec![lnk, cloud],
        );
        let (a, b, root) = (
            tree.stats(&k(r"D:\Daten\a")),
            tree.stats(&k(r"D:\Daten\b")),
            tree.stats(&k(ROOT)),
        );
        assert!(a.has_link && !a.has_cloud);
        assert!(b.has_cloud && !b.has_link);
        assert!(root.has_link && root.has_cloud);
        assert!(
            tree.complete(&k(ROOT)),
            "Wissen über den Inhalt ist vollständig"
        );
    }

    #[test]
    fn lesefehler_macht_den_ordner_unvollstaendig() {
        let mut gesperrt = d(r"D:\Daten\gesperrt", 0);
        gesperrt.read_error = true;
        let tree = Tree::new(vec![d(ROOT, 1), gesperrt], vec![]);
        assert!(!tree.complete(&k(r"D:\Daten\gesperrt")));
        assert!(!tree.is_empty(&k(r"D:\Daten\gesperrt")));
        assert!(!tree.is_empty(&k(ROOT)));
    }

    #[test]
    fn kennzahlen_summieren_den_teilbaum() {
        let tree = Tree::new(
            vec![d(ROOT, 2), d(r"D:\Daten\a", 2), d(r"D:\Daten\a\b", 1)],
            vec![
                f(r"D:\Daten\x.txt", 100, 50),
                f(r"D:\Daten\a\y.txt", 10, 300),
                f(r"D:\Daten\a\b\z.txt", 1, 200),
            ],
        );
        let sub = tree.stats(&k(r"D:\Daten\a"));
        assert_eq!((sub.files, sub.bytes), (2, 11));
        assert_eq!(sub.newest_mtime, Some(300));
        let root = tree.stats(&k(ROOT));
        assert_eq!((root.files, root.bytes), (3, 111));
        assert_eq!(root.newest_mtime, Some(300));
        let leaf = tree.stats(&k(r"D:\Daten\a\b"));
        assert_eq!(
            (leaf.files, leaf.bytes, leaf.newest_mtime),
            (1, 1, Some(200))
        );
    }

    #[test]
    fn kinder_und_dateien_sind_sortiert_und_unbekannte_ordner_neutral() {
        let tree = Tree::new(
            vec![
                d(ROOT, 4),
                d(r"D:\Daten\b", 0),
                d(r"D:\Daten\a", 0),
                d(r"D:\Daten\c", 0),
            ],
            vec![f(r"D:\Daten\z.txt", 1, 1)],
        );
        assert_eq!(
            tree.children(&k(ROOT)),
            [k(r"D:\Daten\a"), k(r"D:\Daten\b"), k(r"D:\Daten\c")]
        );
        assert_eq!(tree.files(&k(ROOT)).len(), 1);
        assert_eq!(tree.row(&k(ROOT)).unwrap().path, ROOT);
        assert_eq!(tree.dir_keys().count(), 4);

        let unknown = k(r"D:\Fremd");
        assert!(tree.children(&unknown).is_empty());
        assert!(tree.files(&unknown).is_empty());
        assert!(!tree.complete(&unknown) && !tree.is_empty(&unknown));
        assert_eq!(tree.stats(&unknown), SubtreeStats::default());
        assert!(tree.row(&unknown).is_none());
    }

    #[test]
    fn baum_aus_dem_index_entspricht_dem_aus_den_zeilen() {
        use crate::index::{DirRecord, FileRecord, Index};
        let mut idx = Index::open_in_memory().unwrap();
        let root_key = k(ROOT);
        let run = idx.begin_root(ROOT, &root_key, "t").unwrap();
        let rec = |row: DirRow| DirRecord {
            path: row.path,
            path_key: row.key,
            parent_key: row.parent_key,
            mode: "full".into(),
            direct_entries: row.direct_entries,
            ..DirRecord::default()
        };
        idx.upsert_dirs(
            &[rec(d(ROOT, 1)), rec(d(r"D:\Daten\leer", 0))],
            run.generation,
        )
        .unwrap();
        idx.upsert_files(
            &[FileRecord {
                dir_key: root_key.clone(),
                path: r"D:\Daten\a.txt".into(),
                path_key: paths::path_key(Path::new(r"D:\Daten\a.txt")),
                name: "a.txt".into(),
                size: 3,
                mtime: 9,
                ..FileRecord::default()
            }],
            run.generation,
        )
        .unwrap();
        let tree = Tree::new(
            idx.dirs_under(&root_key).unwrap(),
            idx.files_under(&root_key).unwrap(),
        );
        // `direct_entries` des Wurzelordners ist 1, bekannt sind aber Datei und Unterordner.
        assert!(!tree.complete(&root_key));
        assert!(tree.is_empty(&k(r"D:\Daten\leer")));
        assert_eq!(tree.stats(&root_key).bytes, 3);
    }
}
