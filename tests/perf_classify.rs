//! Performance von `classify`: Wiederholungslauf mit 10.000 Dateien aus dem Cache.
//!
//! Laufzeit- und hardwareabhängig, daher standardmäßig ignoriert:
//! `cargo test --release --test perf_classify -- --ignored --nocapture`

use std::time::{Duration, Instant};

use assert_cmd::Command;

const FILES: usize = 10_000;
const LIMIT: Duration = Duration::from_secs(10);

#[test]
#[ignore = "Laufzeit- und hardwareabhängig"]
fn wiederholungslauf_mit_10000_dateien_aus_dem_cache_dauert_wenige_sekunden() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let cfg = home.path().join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), "[classify]\nocr = false\n").unwrap();
    for d in 0..100 {
        let dir = tree.path().join(format!("d{d:03}"));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..(FILES / 100) {
            std::fs::write(
                dir.join(format!("datei{f:03}.txt")),
                format!("inhalt {d} {f}"),
            )
            .unwrap();
        }
    }
    let run = |args: &[&str]| {
        let start = Instant::now();
        Command::cargo_bin("ordner-cleanup")
            .unwrap()
            .env("ORDNER_CLEANUP_HOME", home.path())
            .args(args)
            .arg(tree.path())
            .assert()
            .success();
        start.elapsed()
    };
    run(&["scan"]);
    let first = run(&["classify"]);
    let second = run(&["classify"]);
    println!("erster Lauf {first:?}, Wiederholung {second:?}");
    assert!(second < LIMIT, "Wiederholung dauerte {second:?}");
}
