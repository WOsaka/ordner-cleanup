//! Performance der Planer (Spec: 100.000 Dateien, jeder Planer unter 10 Sekunden).
//!
//! Laufzeit- und hardwareabhängig, daher standardmäßig ignoriert:
//! `cargo test --release --test perf_plans -- --ignored --nocapture`

use std::path::Path;
use std::time::{Duration, Instant};

use ordner_cleanup::change::archive::{plan_archive, ArchiveOptions};
use ordner_cleanup::change::dedupe_dirs::{plan_dedupe_dirs, DedupeDirsOptions};
use ordner_cleanup::change::empty_dirs::plan_empty_dirs;
use ordner_cleanup::change::junk::{plan_junk, JunkOptions};
use ordner_cleanup::change::protect::{ProtectPaths, Protector};
use ordner_cleanup::change::rules::{
    plan_rules, CachedExif, LiveContent, NoContent, NoText, RulesEnv,
};
use ordner_cleanup::change::versions::{plan_versions, VersionsOptions};
use ordner_cleanup::config::Config;
use ordner_cleanup::index::{DirRecord, FileRecord, Index};
use ordner_cleanup::paths;
use ordner_cleanup::rules::{KnownCategories, RuleSet};

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

const TEN_RULES: &str = r#"
[[rules]]
name = "berichte"
ext = ["docx"]
name_regex = '^bericht(\d+)_v(\d)'
target = "Berichte/{1}/"
[[rules]]
name = "cache"
ext = ["tmp"]
target = "Temp/"
[[rules]]
name = "alt-txt"
ext = ["txt"]
min_age = "365d"
target = "Alt/{year}/{parent}_{name}.{ext}"
[[rules]]
name = "txt-gross"
ext = ["txt"]
min_size = "1MB"
target = "Gross/"
[[rules]]
name = "txt"
ext = ["txt"]
target = "Texte/{year}/"
[[rules]]
name = "pdf"
ext = ["pdf"]
target = "Pdf/"
[[rules]]
name = "xlsx"
ext = ["xlsx"]
target = "Tabellen/"
[[rules]]
name = "bilder"
ext = ["png", "gif"]
target = "Bilder/"
[[rules]]
name = "downloads"
glob = "Downloads/**"
target = "Eingang/"
[[rules]]
name = "rest"
glob = "**/rest*"
target = "Rest/"
"#;

#[test]
#[ignore = "Performance-Messung, siehe Modul-Dokumentation"]
fn plan_rules_schafft_100000_dateien_mit_10_regeln_ohne_exif_in_unter_10_sekunden() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("wurzel");
    std::fs::create_dir_all(&root).unwrap();
    let mut index = timed("Index aufbauen (100.000 Dateien)", || build(&root));
    let protector = Protector::new(&root, &Config::default(), &ProtectPaths::from_env());
    let rules = RuleSet::parse(
        TEN_RULES,
        Path::new("rules.toml"),
        &KnownCategories::builtin(),
    )
    .unwrap();
    let content = LiveContent::default();

    let result = timed("plan rules", || {
        plan_rules(
            &mut index,
            &root,
            &rules,
            &RulesEnv {
                protector: &protector,
                exif: &CachedExif,
                content: &content,
                created: "t",
                now_ticks: NOW,
                content_lookup: &NoContent,
                text: &NoText,
                min_confidence: 0.8,
            },
        )
        .unwrap()
    });

    println!(
        "Aktionen: {}, übersprungen: {}, ohne Regel: {}",
        result.plan.actions.len(),
        result.plan.skipped.len(),
        result.unmatched
    );
    assert!(result.plan.actions.len() > 10_000);
    result.plan.validate().unwrap();
}

#[test]
#[ignore = "Performance-Messung, siehe Modul-Dokumentation"]
fn plan_rules_mit_10000_jpegs_liest_beim_zweiten_lauf_aus_dem_cache() {
    use chrono::NaiveDate;
    use ordner_cleanup::scan::exif::testing::{jpeg_with_date, DATE_TIME_ORIGINAL};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("wurzel");
    let taken = NaiveDate::from_ymd_opt(2019, 8, 15)
        .unwrap()
        .and_hms_opt(10, 0, 0)
        .unwrap();
    let jpeg = jpeg_with_date(DATE_TIME_ORIGINAL, taken);
    let mut index = Index::open_in_memory().unwrap();
    let root_str = paths::display(&root);
    let root_key = paths::dir_key(&root);
    let run = index.begin_root(&root_str, &root_key, "t").unwrap();
    let mut dirs = vec![DirRecord {
        path: root_str.clone(),
        path_key: root_key.clone(),
        mode: "full".into(),
        attrs: 0x10,
        direct_entries: 100,
        ..DirRecord::default()
    }];
    let mut files = Vec::new();
    for d in 0..100 {
        let sub = root.join(format!("alben{d:02}"));
        std::fs::create_dir_all(&sub).unwrap();
        dirs.push(DirRecord {
            path: paths::display(&sub),
            path_key: paths::dir_key(&sub),
            parent_key: Some(root_key.clone()),
            depth: 1,
            mode: "full".into(),
            attrs: 0x10,
            direct_entries: 100,
            ..DirRecord::default()
        });
        for f in 0..100 {
            let name = format!("IMG_{d:02}{f:02}.jpg");
            let path = sub.join(&name);
            std::fs::write(&path, &jpeg).unwrap();
            files.push(FileRecord {
                dir_key: paths::dir_key(&sub),
                path: paths::display(&path),
                path_key: paths::path_key(&path),
                name,
                size: jpeg.len() as i64,
                mtime: NOW - DAY,
                attrs: 0x20,
                ..FileRecord::default()
            });
        }
    }
    index.upsert_dirs(&dirs, run.generation).unwrap();
    index.upsert_files(&files, run.generation).unwrap();
    let protector = Protector::new(&root, &Config::default(), &ProtectPaths::from_env());
    let rules = RuleSet::parse(
        "[[rules]]\nname = \"fotos\"\next = [\"jpg\"]\ntarget = \"Fotos/{exif.date:%Y}/\"\n",
        Path::new("rules.toml"),
        &KnownCategories::builtin(),
    )
    .unwrap();
    let content = LiveContent::default();
    let run_plan = |index: &mut Index| {
        plan_rules(
            index,
            &root,
            &rules,
            &RulesEnv {
                protector: &protector,
                exif: &CachedExif,
                content: &content,
                created: "t",
                now_ticks: NOW,
                content_lookup: &NoContent,
                text: &NoText,
                min_confidence: 0.8,
            },
        )
        .unwrap()
    };

    let start = Instant::now();
    let first = run_plan(&mut index);
    println!("erster Lauf (10.000 JPEGs lesen): {:.2?}", start.elapsed());
    assert_eq!(first.plan.actions.len(), 10_000);
    assert!(first.plan.actions[0]
        .target
        .as_deref()
        .unwrap()
        .contains("2019"));

    // Dateien weg: Der zweite Lauf darf nichts mehr lesen müssen.
    std::fs::remove_dir_all(&root).unwrap();
    let second = timed("zweiter Lauf (Cache)", || run_plan(&mut index));
    assert_eq!(second.plan.actions.len(), 10_000);
    assert!(
        start.elapsed() > Duration::ZERO
            && second.plan.actions[0].target == first.plan.actions[0].target
    );
}

/// `Plan::validate` bleibt bei vielen Aktionen schnell (Laden, Anwenden und Teilplan rufen es auf):
/// 100.000 Datei-Verschiebungen und 1.000 Ordner-Verschiebungen ins Archiv.
#[test]
#[ignore = "Performance-Messung, siehe Modul-Dokumentation"]
fn plan_validate_schafft_100000_aktionen_mit_1000_ordnern_in_unter_zwei_sekunden() {
    use ordner_cleanup::change::plan::{ActionType, Plan, PlanKind, PlannedAction, PLAN_VERSION};

    let root = r"Z:\Root";
    let action = |id: u32, path: String, target: String, is_dir: bool| PlannedAction {
        id,
        action: ActionType::Move,
        path,
        size: 1,
        mtime_ticks: 0,
        mtime: String::new(),
        hash: None,
        keep: None,
        keep_hash: None,
        reason: "archive".into(),
        target: Some(target),
        is_dir,
        files: is_dir.then_some(1),
        keep_fingerprint: None,
        source_fingerprint: None,
        rule: None,
    };
    let mut actions = Vec::new();
    for d in 0..1_000u32 {
        actions.push(action(
            d + 1,
            format!(r"{root}\ordner{d}"),
            format!(r"{root}\_Archiv\ordner{d}"),
            true,
        ));
    }
    for f in 0..100_000u32 {
        actions.push(action(
            f + 1_001,
            format!(r"{root}\lose\datei{f}.txt"),
            format!(r"{root}\_Archiv\lose\datei{f}.txt"),
            false,
        ));
    }
    let plan = Plan {
        version: PLAN_VERSION,
        created: "t".into(),
        kind: PlanKind::Archive,
        root: root.into(),
        keep_strategy: None,
        params: Default::default(),
        protected_paths: Vec::new(),
        actions,
        skipped: Vec::new(),
    };
    let start = Instant::now();
    plan.validate().unwrap();
    let elapsed = start.elapsed();
    println!("validate: {elapsed:.2?}");
    assert!(
        elapsed < Duration::from_secs(2),
        "validate braucht {elapsed:?} (Grenze 2 s)"
    );
}

/// 20 × 50 Ordner mit je 100 Dateien und vollen Hashes; `projekt10..19` sind Kopien von
/// `projekt00..09` (500 Ordnerpaare auf zwei Ebenen).
fn build_with_hashes(root: &Path) -> Index {
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
            dirs.push(DirRecord {
                path: paths::display(&sub),
                path_key: paths::dir_key(&sub),
                parent_key: Some(paths::dir_key(&top)),
                depth: 2,
                mode: "full".into(),
                attrs: 0x10,
                direct_entries: FILES as i64,
                ..DirRecord::default()
            });
            let sub_key = paths::dir_key(&sub);
            for f in 0..FILES {
                let name = format!("datei{f}.txt");
                let path = sub.join(&name);
                let hash = vec![
                    (t % 10) as u8,
                    s as u8,
                    f as u8,
                    7,
                    7,
                    7,
                    7,
                    7,
                    7,
                    7,
                    7,
                    7,
                    7,
                    7,
                    7,
                    7,
                ];
                files.push(FileRecord {
                    dir_key: sub_key.clone(),
                    path: paths::display(&path),
                    path_key: paths::path_key(&path),
                    name,
                    size: 1000 + (s * FILES + f) as i64,
                    mtime: NOW - (t as i64) * DAY,
                    attrs: 0x20,
                    partial_hash: Some(hash.clone()),
                    full_hash: Some(hash),
                    hash_status: Some("ok".into()),
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

/// Spec: 1 Mio. Dateien unter 30 s; hier 100.000 Dateien, also linear hochgerechnet unter 3 s.
#[test]
#[ignore = "Performance-Messung, siehe Modul-Dokumentation"]
fn plan_dedupe_dirs_schafft_100000_dateien_in_unter_3_sekunden() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Root");
    let index = build_with_hashes(&root);
    let protector = Protector::new(&root, &Config::default(), &ProtectPaths::default());
    let start = Instant::now();
    let result = plan_dedupe_dirs(
        &index,
        &root,
        &"oldest".parse().unwrap(),
        &protector,
        "t",
        &DedupeDirsOptions::default(),
    )
    .unwrap();
    let elapsed = start.elapsed();
    println!("plan dedupe-dirs (100.000 Dateien): {elapsed:.2?}");
    assert_eq!(
        result.plan.actions.len(),
        10,
        "zehn Kopien der Projektordner"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "plan dedupe-dirs braucht {elapsed:?} (Grenze 3 s je 100.000 Dateien)"
    );
}
