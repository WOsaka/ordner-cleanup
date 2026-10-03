//! Duplikat-Pipeline gegen echte Temp-Verzeichnisse.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::classify::DefaultPaths;
use super::duplicates::{self, HashStats};
use super::source::{cloud_file, FakeDirSource, StdDirSource};
use super::walker::Progress;
use super::{scan, ScanEnv, ScanOutcome};
use crate::config::Config;
use crate::index::{DupGroup, Index, RootRun};
use crate::paths;

fn scan_with(index: &mut Index, source: &dyn super::source::DirSource, root: &Path) -> ScanOutcome {
    let config = Config {
        threads: 2,
        ..Config::default()
    };
    let env = ScanEnv {
        source,
        config: &config,
        default_paths: &DefaultPaths::default(),
        cancel: &AtomicBool::new(false),
        progress: &Progress::default(),
        find_duplicates: true,
        now: "t",
    };
    scan(index, root, &env).unwrap()
}

fn scan_dir(index: &mut Index, root: &Path) -> ScanOutcome {
    scan_with(index, &StdDirSource, root)
}

fn groups(index: &Index, root: &Path) -> Vec<DupGroup> {
    index.exact_duplicate_groups(&paths::dir_key(root)).unwrap()
}

fn write(root: &Path, rel: &str, content: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn status_of(index: &Index, name: &str) -> Option<String> {
    index
        .conn()
        .query_row(
            "SELECT hash_status FROM files WHERE name = ?1",
            [name],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn identische_dateien_an_verschiedenen_orten_sind_eine_gruppe() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "a/x.txt", b"gleicher inhalt");
    write(tmp.path(), "b/y.txt", b"gleicher inhalt");
    write(tmp.path(), "c/z.txt", b"gleicher inhalt");
    let mut index = Index::open_in_memory().unwrap();
    scan_dir(&mut index, tmp.path());
    let g = groups(&index, tmp.path());
    assert_eq!(g.len(), 1);
    assert_eq!((g[0].size, g[0].instances, g[0].wasted), (15, 3, 30));
    assert_eq!(g[0].files.len(), 3);
}

#[test]
fn gleiche_groesse_anderer_inhalt_ist_kein_duplikat() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "a.txt", b"inhalt A");
    write(tmp.path(), "b.txt", b"inhalt B");
    let mut index = Index::open_in_memory().unwrap();
    let out = scan_dir(&mut index, tmp.path());
    assert!(groups(&index, tmp.path()).is_empty());
    assert_eq!(out.hash.candidates, 2);
}

#[test]
fn unterschied_nur_in_der_mitte_wird_vom_voll_hash_erkannt() {
    let tmp = tempfile::tempdir().unwrap();
    let mut content = vec![5u8; 400 * 1024];
    write(tmp.path(), "a.bin", &content);
    content[200 * 1024] = 6;
    write(tmp.path(), "b.bin", &content);
    let mut index = Index::open_in_memory().unwrap();
    let out = scan_dir(&mut index, tmp.path());
    assert!(groups(&index, tmp.path()).is_empty());
    assert_eq!((out.hash.partial_hashed, out.hash.full_hashed), (2, 2));
}

#[test]
fn eindeutige_groessen_werden_nicht_gehasht() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "eins.txt", b"1");
    write(tmp.path(), "zwei.txt", b"22");
    write(tmp.path(), "drei.txt", b"333");
    write(tmp.path(), "leer.txt", b"");
    let mut index = Index::open_in_memory().unwrap();
    let out = scan_dir(&mut index, tmp.path());
    assert_eq!(
        out.hash,
        HashStats {
            candidates: 0,
            skipped_unique: 3,
            ..HashStats::default()
        }
    );
    let hashed: i64 = index
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM files WHERE partial_hash IS NOT NULL OR full_hash IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hashed, 0);
}

#[test]
fn gesperrte_datei_wird_markiert_und_ist_kein_verifiziertes_duplikat() {
    use std::os::windows::fs::OpenOptionsExt;
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "frei.txt", b"gleich");
    write(tmp.path(), "gesperrt.txt", b"gleich");
    let _holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(tmp.path().join("gesperrt.txt"))
        .unwrap();
    let mut index = Index::open_in_memory().unwrap();
    let out = scan_dir(&mut index, tmp.path());
    assert_eq!(out.hash.locked, 1);
    assert_eq!(out.errors, 1);
    assert_eq!(status_of(&index, "gesperrt.txt").as_deref(), Some("locked"));
    assert!(groups(&index, tmp.path()).is_empty());
    let kind: String = index
        .conn()
        .query_row("SELECT kind FROM errors", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kind, "locked");
    assert_eq!(index.roots().unwrap()[0].error_count, 1);
}

#[test]
fn hardlinks_werden_einmal_gezaehlt_und_nicht_als_duplikat_gemeldet() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "a.txt", b"hardlink-inhalt");
    std::fs::hard_link(tmp.path().join("a.txt"), tmp.path().join("b.txt")).unwrap();
    let mut index = Index::open_in_memory().unwrap();
    let out = scan_dir(&mut index, tmp.path());
    assert!(groups(&index, tmp.path()).is_empty());
    assert_eq!(
        out.hash.partial_hashed, 0,
        "ein Hardlink-Paar braucht keinen Hash"
    );

    // Mit einer echten Kopie: zwei Instanzen, drei Pfade.
    write(tmp.path(), "kopie.txt", b"hardlink-inhalt");
    let out = scan_dir(&mut index, tmp.path());
    let g = groups(&index, tmp.path());
    assert_eq!(g.len(), 1);
    assert_eq!((g[0].instances, g[0].wasted, g[0].files.len()), (2, 15, 3));
    assert_eq!(
        out.hash.partial_hashed, 2,
        "nur zwei Vertreter werden gehasht"
    );
    assert_eq!(g[0].files.iter().filter(|f| f.nlinks == 2).count(), 2);
}

#[test]
fn gruppenmitglieder_tragen_die_datei_identitaet() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "a.txt", b"hardlink-inhalt");
    std::fs::hard_link(tmp.path().join("a.txt"), tmp.path().join("b.txt")).unwrap();
    write(tmp.path(), "kopie.txt", b"hardlink-inhalt");
    let mut index = Index::open_in_memory().unwrap();
    scan_dir(&mut index, tmp.path());
    let g = groups(&index, tmp.path());
    let identity = |name: &str| {
        g[0].files
            .iter()
            .find(|f| f.path.ends_with(name))
            .and_then(|f| f.identity)
    };
    assert!(identity("a.txt").is_some());
    assert_eq!(identity("a.txt"), identity("b.txt"));
    assert_ne!(identity("a.txt"), identity("kopie.txt"));
}

#[test]
fn rescan_ohne_aenderung_hasht_nichts_neu() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "a.bin", &vec![1u8; 300 * 1024]);
    write(tmp.path(), "b.bin", &vec![1u8; 300 * 1024]);
    let mut index = Index::open_in_memory().unwrap();
    let first = scan_dir(&mut index, tmp.path());
    assert_eq!((first.hash.partial_hashed, first.hash.full_hashed), (2, 2));
    let second = scan_dir(&mut index, tmp.path());
    assert_eq!(
        (second.hash.partial_hashed, second.hash.full_hashed),
        (0, 0)
    );
    assert_eq!(groups(&index, tmp.path()).len(), 1);
}

#[test]
fn geaenderte_datei_wird_neu_gehasht_und_verlaesst_die_gruppe() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "a.txt", b"gleich");
    write(tmp.path(), "b.txt", b"gleich");
    let mut index = Index::open_in_memory().unwrap();
    scan_dir(&mut index, tmp.path());
    assert_eq!(groups(&index, tmp.path()).len(), 1);
    std::thread::sleep(std::time::Duration::from_millis(20));
    write(tmp.path(), "b.txt", b"anders");
    scan_dir(&mut index, tmp.path());
    assert!(groups(&index, tmp.path()).is_empty());
}

#[test]
fn datei_die_sich_nach_dem_walk_aendert_wird_verworfen() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "a.txt", b"gleich");
    write(tmp.path(), "b.txt", b"gleich");
    let mut index = Index::open_in_memory().unwrap();
    let config = Config {
        threads: 2,
        ..Config::default()
    };
    let env = ScanEnv {
        source: &StdDirSource,
        config: &config,
        default_paths: &DefaultPaths::default(),
        cancel: &AtomicBool::new(false),
        progress: &Progress::default(),
        find_duplicates: false,
        now: "t",
    };
    scan(&mut index, tmp.path(), &env).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    write(tmp.path(), "b.txt", b"gleich"); // gleicher Inhalt, aber neues Änderungsdatum

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let run = RootRun {
        path_key: paths::dir_key(tmp.path()),
        generation: 1,
    };
    let stats = duplicates::run(&mut index, &pool, &run, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.changed, 1);
    assert_eq!(status_of(&index, "b.txt").as_deref(), Some("changed"));
    assert!(groups(&index, tmp.path()).is_empty());
}

#[test]
fn cloud_platzhalter_sind_nie_kandidaten() {
    let source = FakeDirSource::new().dir(
        r"C:\R",
        vec![cloud_file("a.docx", 500), cloud_file("b.docx", 500)],
    );
    let mut index = Index::open_in_memory().unwrap();
    let out = scan_with(&mut index, &source, Path::new(r"C:\R"));
    assert_eq!(out.hash, HashStats::default());
    assert_eq!(out.errors, 0);
    assert_eq!(status_of(&index, "a.docx"), None);
}

#[test]
fn dateien_ueber_4_gib_werden_korrekt_behandelt() {
    // Sparse-Dateien, damit der Test keine 9 GB schreibt; ohne fsutil wird übersprungen.
    let tmp = tempfile::tempdir().unwrap();
    let size: u64 = 4 * 1024 * 1024 * 1024 + 4096;
    for (name, marker) in [("gross1.bin", 1u8), ("gross2.bin", 2u8)] {
        let path = tmp.path().join(name);
        std::fs::File::create(&path).unwrap();
        let ok = std::process::Command::new("fsutil")
            .args(["sparse", "setflag"])
            .arg(&path)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("fsutil sparse nicht verfügbar, Test übersprungen");
            return;
        }
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_len(size).unwrap();
        use std::os::windows::fs::FileExt;
        f.seek_write(&[marker; 16], size - 16).unwrap();
    }
    let mut index = Index::open_in_memory().unwrap();
    let out = scan_dir(&mut index, tmp.path());
    assert_eq!(out.bytes, size * 2);
    let stored: i64 = index
        .conn()
        .query_row("SELECT MAX(size) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored as u64, size);
    // Der Teil-Hash sieht das Ende und unterscheidet die Dateien: kein Voll-Hash nötig.
    assert_eq!((out.hash.partial_hashed, out.hash.full_hashed), (2, 0));
    assert!(groups(&index, tmp.path()).is_empty());
}
