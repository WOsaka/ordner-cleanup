//! Performance der Quarantäne-Ansicht: 100 Läufe mit je 1.000 Quarantäne-Moves (100.000
//! Einträge, echte kleine Dateien). Das Laden der Liste soll unter 10 Sekunden bleiben (wie
//! die Planer in `perf_plans.rs`), Suchen und Sortieren der geladenen Liste unter 100 ms.
//!
//! Laufzeit- und hardwareabhängig, daher standardmäßig ignoriert:
//! `cargo test --release --test perf_quarantine -- --ignored --nocapture`

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ordner_cleanup::change::fsops::RealFs;
use ordner_cleanup::change::journal::{Dest, EndStatus, Entry};
use ordner_cleanup::change::quarantine::{journal_dir, journal_path, run_dir};
use ordner_cleanup::change::restore::list_quarantine;
use ordner_cleanup::change::RunId;
use ordner_cleanup::gui::views::quarantine::{visible_rows, Row, Sort, SortColumn};
use ordner_cleanup::paths;

const RUNS: usize = 100;
const PER_RUN: usize = 1_000;
const DIRS: usize = 20;
const LOAD_LIMIT: Duration = Duration::from_secs(10);
const UI_LIMIT: Duration = Duration::from_millis(100);

fn run_id(n: usize) -> RunId {
    RunId::parse(&format!(
        "20260101-00{:02}{:02}-{n:04x}",
        n / 60 % 60,
        n % 60
    ))
    .unwrap()
}

/// Schreibt Journale und Dateien direkt (ein `fsync` je Eintrag wie beim echten Apply wäre
/// bei 200.000 Zeilen selbst die Messung).
fn build(root: &Path) {
    std::fs::create_dir_all(journal_dir(root)).unwrap();
    for r in 0..RUNS {
        let run = run_id(r);
        let quarantine = run_dir(root, &run);
        for d in 0..DIRS {
            std::fs::create_dir_all(quarantine.join(format!("ordner{d:02}"))).unwrap();
        }
        let mut lines: Vec<Entry> = vec![Entry::RunStart {
            run: run.clone(),
            plan: "plan.json".into(),
            root: paths::display(root),
            started: "2026-01-01T10:00:00Z".into(),
        }];
        for k in 0..PER_RUN {
            let rel = format!("ordner{:02}/datei{k:04}.dat", k % DIRS);
            let stored = quarantine.join(&rel);
            std::fs::write(&stored, b"0123456789").unwrap();
            let action = k as u32 + 1;
            lines.push(Entry::Intent {
                run: run.clone(),
                action,
                from: paths::display(&root.join(format!("lauf{r:03}")).join(&rel)),
                to: paths::display(&stored),
                size: 10,
                hash: None,
                dest: Dest::Quarantine,
                is_dir: false,
            });
            lines.push(Entry::Done {
                run: run.clone(),
                action,
            });
        }
        lines.push(Entry::RunEnd {
            run: run.clone(),
            status: EndStatus::Complete,
        });
        let mut file = std::fs::File::create(journal_path(root, &run)).unwrap();
        for entry in &lines {
            writeln!(file, "{}", serde_json::to_string(entry).unwrap()).unwrap();
        }
    }
}

fn timed<T>(label: &str, limit: Duration, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let value = f();
    let elapsed = start.elapsed();
    println!("{label}: {elapsed:.2?}");
    assert!(
        elapsed < limit,
        "{label} braucht {elapsed:?} (Grenze {limit:?})"
    );
    value
}

#[test]
#[ignore = "Performance-Messung, siehe Modul-Dokumentation"]
fn hunderttausend_eintraege_laden_filtern_und_sortieren() {
    let dir = tempfile::tempdir().unwrap();
    let root: PathBuf = dir.path().join("wurzel");
    let built = Instant::now();
    build(&root);
    println!("Aufbau (nicht gemessen): {:.2?}", built.elapsed());

    let listing = timed("Liste laden (100.000 Einträge)", LOAD_LIMIT, || {
        list_quarantine(&root, 30, &RealFs).unwrap()
    });
    assert_eq!(listing.items.len(), RUNS * PER_RUN);
    assert_eq!(listing.unreadable, 0);
    assert!(listing.items.iter().all(|i| i.present));

    let rows: Vec<Row> = timed("Zeilen aufbauen", LOAD_LIMIT, || {
        listing
            .items
            .into_iter()
            .map(|item| Row::new(root.clone(), item))
            .collect()
    });

    let by = |column, ascending| Sort { column, ascending };
    for (label, filter, sort) in [
        ("Standard (Datum absteigend)", "", Sort::default()),
        ("Suche „datei05“", "datei05", Sort::default()),
        ("Suche „ORDNER07“", "ORDNER07", by(SortColumn::Name, true)),
        ("Sortiert nach Name", "", by(SortColumn::Name, true)),
        ("Sortiert nach Größe", "", by(SortColumn::Size, false)),
        ("Sortiert nach Ablauf", "", by(SortColumn::Expires, true)),
    ] {
        let visible = timed(label, UI_LIMIT, || {
            visible_rows(&rows, filter, None, None, sort)
        });
        assert!(!visible.is_empty(), "{label}");
    }
    let one_run = run_id(7);
    let visible = timed("Filter auf einen Lauf", UI_LIMIT, || {
        visible_rows(&rows, "", Some(&root), Some(&one_run), Sort::default())
    });
    assert_eq!(visible.len(), PER_RUN);
}
