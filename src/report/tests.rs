use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use super::*;
use crate::analysis::problems::ProblemCtx;
use crate::analysis::structure::Thresholds;
use crate::config::Config;
use crate::scan::classify::DefaultPaths;
use crate::scan::source::{DirSource, StdDirSource, TICKS_PER_SEC};
use crate::scan::walker::Progress;
use crate::scan::{scan, ScanEnv};

pub(super) fn now_ticks() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        * TICKS_PER_SEC
}

pub(super) fn params(top: usize) -> ReportParams {
    ReportParams {
        generated_at: "2026-10-02T12:00:00Z".into(),
        now_ticks: now_ticks(),
        top,
        old_after_days: 365,
        thresholds: Thresholds {
            max_depth: 8,
            huge_entries: 1000,
        },
        problem_ctx: ProblemCtx::with_hosts(&["zzz-host"]),
        partial_threshold: 0.8,
    }
}

pub(super) fn run_scan(index: &mut Index, source: &dyn DirSource, root: &Path) {
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
        now: "2026-10-02T11:00:00Z",
    };
    scan(index, root, &env).unwrap();
}

pub(super) fn write(root: &Path, rel: &str, content: &[u8]) -> PathBuf {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
    p
}

pub(super) struct Fixture {
    _tmp: tempfile::TempDir,
    pub(super) root: PathBuf,
    pub(super) index: Index,
}

pub(super) fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path().to_path_buf();
    write(&r, "docs/Angebot.docx", &[b'a'; 100]);
    write(&r, "docs/Angebot_v2.docx", &[b'a'; 100]);
    write(&r, "docs/Angebot - Kopie (3).docx", &[b'b'; 50]);
    write(&r, "docs/~$Angebot.docx", &[b'c'; 10]);
    let video = write(&r, "big/video.mp4", &[b'v'; 5000]);
    std::fs::create_dir_all(r.join("empty")).unwrap();
    write(&r, "single/only.txt", b"1234567");
    write(&r, "node_modules/x.js", &[b'x'; 300]);
    write(&r, "node_modules/sub/y.js", &[b'y'; 200]);
    write(&r, "readme.txt", b"12345");
    write(&r, "zero.txt", b"");
    write(&r, "hard1.dat", &[b'h'; 20]);
    std::fs::hard_link(r.join("hard1.dat"), r.join("hard2.dat")).unwrap();
    // video.mp4 ist über 3 Jahre alt
    let old = SystemTime::now() - Duration::from_secs(1200 * 86_400);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&video)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let mut index = Index::open_in_memory().unwrap();
    run_scan(&mut index, &StdDirSource, &r);
    Fixture {
        _tmp: tmp,
        root: r,
        index,
    }
}

pub(super) fn report_of(f: &Fixture) -> Report {
    let root = select_root(&f.index, Some(&f.root)).unwrap();
    build(&f.index, &root, &params(50)).unwrap()
}

#[test]
fn uebersicht_stimmt_mit_unabhaengiger_zaehlung() {
    let r = report_of(&fixture());
    let o = &r.overview;
    // 10 Dateien + 2 in node_modules; Hardlink-Geschwister zählt die Größe nur einmal.
    assert_eq!(o.files, 12);
    assert_eq!(o.summary_files, 2);
    assert_eq!(o.summary_size, 500);
    assert_eq!(o.total_size, 100 + 100 + 50 + 10 + 5000 + 7 + 5 + 20 + 500);
    assert_eq!(o.dirs, 6);
    assert_eq!(o.error_count, 0);
    assert!((o.summary_share - 500.0 / 5792.0).abs() < 1e-9);
    assert_eq!(r.meta.scan_status, "complete");
    assert_eq!(r.meta.notice, CONFIDENTIALITY_NOTICE);
}

#[test]
fn groessenbaum_summiert_und_ist_nach_groesse_sortiert() {
    let r = report_of(&fixture());
    let t = &r.size_tree;
    assert_eq!(t.size, r.overview.total_size);
    assert_eq!(t.files, 12);
    let sizes: Vec<u64> = t.children.iter().map(|c| c.size).collect();
    let mut sorted = sizes.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(sizes, sorted);
    let names: Vec<&str> = t.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names[0], "big");
    let nm = t
        .children
        .iter()
        .find(|c| c.name == "node_modules")
        .unwrap();
    assert!(nm.summary);
    assert_eq!((nm.size, nm.files), (500, 2));
    assert!(nm.children.is_empty());
    let docs = t.children.iter().find(|c| c.name == "docs").unwrap();
    assert_eq!((docs.size, docs.files), (260, 4));
}

#[test]
fn top_listen() {
    let r = report_of(&fixture());
    assert!(r.top_files[0].path.ends_with("video.mp4"));
    assert_eq!(r.top_files[0].size, 5000);
    assert!(r.top_dirs[0].path.ends_with("big"));
    assert!(r.top_dirs.iter().any(|d| d.summary && d.size == 500));
}

#[test]
fn top_n_wird_beachtet() {
    let f = fixture();
    let root = select_root(&f.index, Some(&f.root)).unwrap();
    let r = build(&f.index, &root, &params(2)).unwrap();
    assert_eq!(r.top_files.len(), 2);
    assert_eq!(r.top_dirs.len(), 2);
}

#[test]
fn dateitypen_nach_endung_und_kategorie() {
    let r = report_of(&fixture());
    let docx = r
        .file_types
        .by_extension
        .iter()
        .find(|t| t.key == "docx")
        .unwrap();
    assert_eq!((docx.count, docx.size), (4, 260));
    assert_eq!(r.file_types.by_extension[0].key, "mp4");
    let cat = |name: &str| {
        r.file_types
            .by_category
            .iter()
            .find(|t| t.key == name)
            .unwrap()
    };
    assert_eq!(cat("Video").size, 5000);
    assert_eq!(cat("Dokumente").count, 4 + 3); // docx + txt (only, readme, zero)
    assert_eq!(cat("Sonstige").count, 1); // hard1/hard2 zählen als eine Datei
}
