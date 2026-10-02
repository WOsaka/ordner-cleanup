//! Inkrementeller Scan: Hashes übernehmen/verwerfen, Prune, Abbruch und Wiederaufnahme.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::classify::DefaultPaths;
use super::source::{fake_dir, fake_file, DirSource, FakeDirSource, RawEntry};
use super::walker::Progress;
use super::{scan, ScanEnv, ScanOutcome};
use crate::config::Config;
use crate::index::{Index, RootStatus};

fn run(index: &mut Index, source: &dyn DirSource, cancel: bool) -> ScanOutcome {
    let config = Config {
        threads: 2,
        ..Config::default()
    };
    let env = ScanEnv {
        source,
        config: &config,
        default_paths: &DefaultPaths::default(),
        cancel: &AtomicBool::new(cancel),
        progress: &Progress::default(),
        find_duplicates: false,
        now: "t",
    };
    scan(index, Path::new(r"C:\R"), &env).unwrap()
}

fn file_at(name: &str, size: u64, mtime: i64) -> RawEntry {
    RawEntry {
        mtime,
        ..fake_file(name, size)
    }
}

fn source(entries: Vec<RawEntry>) -> FakeDirSource {
    FakeDirSource::new().dir(r"C:\R", entries)
}

fn set_hash(index: &Index, name: &str, status: &str) {
    index
        .conn()
        .execute(
            "UPDATE files SET partial_hash = x'0102', full_hash = x'0304', hash_status = ?1
             WHERE name = ?2",
            [status, name],
        )
        .unwrap();
}

fn hashes(index: &Index, name: &str) -> (Option<Vec<u8>>, Option<Vec<u8>>, Option<String>) {
    index
        .conn()
        .query_row(
            "SELECT partial_hash, full_hash, hash_status FROM files WHERE name = ?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
}

fn names(index: &Index) -> Vec<String> {
    let mut stmt = index
        .conn()
        .prepare("SELECT name FROM files ORDER BY name")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn unveraenderte_datei_behaelt_hashes() {
    let mut index = Index::open_in_memory().unwrap();
    run(&mut index, &source(vec![file_at("a.bin", 10, 100)]), false);
    set_hash(&index, "a.bin", "ok");
    run(&mut index, &source(vec![file_at("a.bin", 10, 100)]), false);
    assert_eq!(
        hashes(&index, "a.bin"),
        (Some(vec![1, 2]), Some(vec![3, 4]), Some("ok".into()))
    );
}

#[test]
fn geaenderte_groesse_oder_datum_verwirft_hashes() {
    for (size, mtime) in [(11, 100), (10, 101)] {
        let mut index = Index::open_in_memory().unwrap();
        run(&mut index, &source(vec![file_at("a.bin", 10, 100)]), false);
        set_hash(&index, "a.bin", "ok");
        run(
            &mut index,
            &source(vec![file_at("a.bin", size, mtime)]),
            false,
        );
        assert_eq!(
            hashes(&index, "a.bin"),
            (None, None, None),
            "{size}/{mtime}"
        );
    }
}

#[test]
fn gesperrte_datei_wird_beim_naechsten_scan_erneut_versucht() {
    let mut index = Index::open_in_memory().unwrap();
    run(&mut index, &source(vec![file_at("a.bin", 10, 100)]), false);
    set_hash(&index, "a.bin", "locked");
    run(&mut index, &source(vec![file_at("a.bin", 10, 100)]), false);
    assert_eq!(hashes(&index, "a.bin").2, None);
}

#[test]
fn geloeschte_neue_und_ordner_spiegeln_den_neuen_zustand() {
    let mut index = Index::open_in_memory().unwrap();
    let first = FakeDirSource::new()
        .dir(r"C:\R", vec![fake_file("alt.txt", 1), fake_dir("weg")])
        .dir(r"C:\R\weg", vec![fake_file("x.txt", 1)]);
    run(&mut index, &first, false);
    assert_eq!(names(&index), vec!["alt.txt", "x.txt"]);

    run(&mut index, &source(vec![fake_file("neu.txt", 2)]), false);
    assert_eq!(names(&index), vec!["neu.txt"]);
    let dirs: i64 = index
        .conn()
        .query_row("SELECT COUNT(*) FROM dirs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(dirs, 1);
}

#[test]
fn nach_abbruch_setzt_der_naechste_scan_sauber_auf() {
    let mut index = Index::open_in_memory().unwrap();
    run(
        &mut index,
        &source(vec![file_at("a.bin", 10, 100), fake_file("alt", 1)]),
        false,
    );
    set_hash(&index, "a.bin", "ok");

    let aborted = run(&mut index, &source(vec![file_at("a.bin", 10, 100)]), true);
    assert!(aborted.aborted);
    assert_eq!(index.roots().unwrap()[0].status, RootStatus::Aborted);
    assert_eq!(names(&index), vec!["a.bin", "alt"]);

    let resumed = run(&mut index, &source(vec![file_at("a.bin", 10, 100)]), false);
    assert!(!resumed.aborted);
    assert_eq!(index.roots().unwrap()[0].status, RootStatus::Complete);
    assert_eq!(names(&index), vec!["a.bin"]);
    assert_eq!(hashes(&index, "a.bin").1, Some(vec![3, 4]));
}
