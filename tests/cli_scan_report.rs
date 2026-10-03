use assert_cmd::Command;
use predicates::str::contains;

fn bin(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("ordner-cleanup").unwrap();
    cmd.env("ORDNER_CLEANUP_HOME", home);
    cmd
}

fn sample_tree(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::write(dir.join("a").join("x.txt"), "gleich").unwrap();
    std::fs::write(dir.join("y.txt"), "gleich").unwrap();
}

#[test]
fn scan_report_und_index_ende_zu_ende() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    sample_tree(tree.path());

    bin(home.path())
        .args(["scan"])
        .arg(tree.path())
        .assert()
        .success()
        .stdout(contains("2 Dateien"));

    bin(home.path())
        .args(["index", "list"])
        .assert()
        .success()
        .stdout(contains("Complete"));

    bin(home.path())
        .args(["report", "--format", "json,html"])
        .arg("--out")
        .arg(out.path())
        .assert()
        .success()
        .stdout(contains("Geschrieben"));
    let report_dir = std::fs::read_dir(out.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(
        report_dir.join("report.html").exists()
            || std::fs::read_dir(&report_dir).unwrap().count() >= 2
    );

    bin(home.path())
        .args(["index", "remove"])
        .arg(tree.path())
        .assert()
        .success();
}

#[test]
fn scan_von_nicht_existentem_pfad_scheitert() {
    let home = tempfile::tempdir().unwrap();
    bin(home.path())
        .args(["scan", r"C:\gibt\es\garantiert\nicht"])
        .assert()
        .code(1)
        .stderr(contains("Fehler"));
}

#[test]
fn report_ohne_scan_scheitert_mit_hinweis() {
    let home = tempfile::tempdir().unwrap();
    bin(home.path())
        .arg("report")
        .assert()
        .code(1)
        .stderr(contains("zuerst `scan` ausführen"));
}
