//! Performance der Planer (Spec: 100.000 Dateien, jeder Planer unter 10 Sekunden).
//!
//! Laufzeit- und hardwareabhängig, daher standardmäßig ignoriert:
//! `cargo test --release --test perf_plans -- --ignored --nocapture`

use std::path::Path;
use std::time::{Duration, Instant};

use ordner_cleanup::change::archive::{plan_archive, ArchiveOptions};
use ordner_cleanup::change::empty_dirs::plan_empty_dirs;
use ordner_cleanup::change::junk::{plan_junk, JunkOptions};
use ordner_cleanup::change::protect::{ProtectPaths, Protector};
use ordner_cleanup::change::versions::{plan_versions, VersionsOptions};
use ordner_cleanup::config::Config;
use ordner_cleanup::index::{DirRecord, FileRecord, Index};
use ordner_cleanup::paths;

const LIMIT: Duration = Duration::from_secs(10);
const TICKS_PER_SEC: i64 = 10_000_000;
const DAY: i64 = 86_400 * TICKS_PER_SEC;
const NOW: i64 = 1_790_985_600 * TICKS_PER_SEC;

const TOP: usize = 20;
const SUB: usize = 50;
const FILES: usize = 100;

/// 20 × 50 Ordner mit je 100 Dateien = 100.000 Dateien; dazu leere Ordner, Müll,
/// Versionsgruppen und alte Ordner.
fn build(root: &Path) -> Index {
    let mut index = Index::open_in_memory().unwrap();
    let root_str = paths::display(root);
    let root_key = paths::dir_key(root);
    let run = index.begin_root(&root_str, &root_key, "t").unwrap();

    let mut dirs = vec![DirRecord {
        path: root_str.clone(),
        path_key: root_key.clone(),
        mode: "full".into(),
        attrs: 0x10,
        direct_entries: TOP as i64,
        ..DirRecord::default()
    }];
    let mut files = Vec::with_capacity(TOP * SUB * FILES);
    for t in 0..TOP {
        let top = root.join(format!("projekt{t:02}"));
        dirs.push(DirRecord {
            path: paths::display(&top),
            path_key: paths::dir_key(&top),
            parent_key: Some(root_key.clone()),
            depth: 1,
            mode: "full".into(),
            attrs: 0x10,
            direct_entries: SUB as i64,
            ..DirRecord::default()
        });
        for s in 0..SUB {
            let sub = top.join(format!("ordner{s:02}"));
            // Jeder 10. Unterordner ist leer, jeder 5. alt (3 Jahre), der Rest aktuell.
            let empty = s % 10 == 0;
            let old = s % 5 == 1;
            let count = if empty { 0 } else { FILES };
            dirs.push(DirRecord {
                path: paths::display(&sub),
                path_key: paths::dir_key(&sub),
                parent_key: Some(paths::dir_key(&top)),
                depth: 2,
                mode: "full".into(),
                attrs: 0x10,
                direct_entries: count as i64,
                ..DirRecord::default()
            });
            let sub_key = paths::dir_key(&sub);
            for f in 0..count {
                let age_days = if old { 1100 } else { f as i64 % 400 };
                // Versionsgruppen (5er), Müll (jede 50.), sonst normale Dateien.
                let name = match f {
                    f if f % 50 == 0 => format!("cache{f}.tmp"),
                    f if f < 50 => format!("bericht{}_v{}.docx", f / 5, f % 5),
                    f => format!("datei{f}.txt"),
                };
                let path = sub.join(&name);
                files.push(FileRecord {
                    dir_key: sub_key.clone(),
                    path: paths::display(&path),
                    path_key: paths::path_key(&path),
                    name,
                    size: 1000,
                    mtime: NOW - age_days * DAY,
                    attrs: 0x20,
                    ..FileRecord::default()
                });
            }
        }
    }
    index.upsert_dirs(&dirs, run.generation).unwrap();
    for chunk in files.chunks(10_000) {
        index.upsert_files(chunk, run.generation).unwrap();
    }
    index
}

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let value = f();
    let elapsed = start.elapsed();
    println!("{label}: {elapsed:.2?}");
    assert!(
        elapsed < LIMIT,
        "{label} braucht {elapsed:?} (Grenze {LIMIT:?})"
    );
    value
}

#[test]
#[ignore = "Performance-Messung, siehe Modul-Dokumentation"]
fn jeder_planer_schafft_100000_dateien_in_unter_10_sekunden() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("wurzel");
    std::fs::create_dir_all(&root).unwrap();
    let index = timed("Index aufbauen (100.000 Dateien)", || build(&root));
    let protector = Protector::new(&root, &Config::default(), &ProtectPaths::from_env());

    let junk = timed("plan junk", || {
        let categories = Config::default().junk_categories;
        plan_junk(
            &index,
            &root,
            &protector,
            "t",
            &JunkOptions {
                categories: &categories,
                rules: &[],
                downloads_dirs: &[],
                installer_min_age_days: 90,
                now_ticks: NOW,
            },
        )
        .unwrap()
    });
    let empty = timed("plan empty-dirs", || {
        plan_empty_dirs(&index, &root, &protector, "t").unwrap()
    });
    let archive = timed("plan archive", || {
        plan_archive(
            &index,
            &root,
            &protector,
            "t",
            &ArchiveOptions {
                older_than: "2y",
                older_than_days: 730,
                now_ticks: NOW,
            },
        )
        .unwrap()
    });
    let versions = timed("plan versions", || {
        plan_versions(
            &index,
            &root,
            &protector,
            "t",
            &VersionsOptions {
                min_age: "30d",
                min_age_days: 30,
                now_ticks: NOW,
            },
        )
        .unwrap()
    });

    // Plausibilität: Die Planer haben wirklich etwas gefunden.
    println!(
        "Aktionen: junk {}, empty-dirs {}, archive {}, versions {}",
        junk.plan.actions.len(),
        empty.plan.actions.len(),
        archive.plan.actions.len(),
        versions.plan.actions.len()
    );
    assert!(junk.plan.actions.len() > 1000);
    assert_eq!(empty.plan.actions.len(), TOP * SUB / 10);
    assert!(archive.plan.actions.len() > 100);
    assert!(versions.plan.actions.len() > 1000);
}
