use std::path::Path;

use assert_cmd::Command;
use predicates::str::contains;

fn bin(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ordner-cleanup").unwrap();
    cmd.env("ORDNER_CLEANUP_HOME", home);
    cmd
}

fn sample_tree(dir: &Path) {
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::write(dir.join("a").join("x.txt"), "inhalt-eins").unwrap();
    std::fs::write(dir.join("a").join("z.txt"), "inhalt-drei").unwrap();
    std::fs::write(dir.join("y.txt"), "inhalt-zwei").unwrap();
}

fn history_file(home: &Path) -> std::path::PathBuf {
    home.join("data").join("history.db")
}

#[test]
fn scan_schreibt_momentaufnahme_und_nennt_score() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());

    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success()
        .stdout(contains("Health-Score 100 (erster Lauf)"));
    assert!(history_file(home.path()).exists());

    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success()
        .stdout(contains("Health-Score 100 (±0 seit "));
}

#[test]
fn reset_index_laesst_den_verlauf_stehen() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    bin(home.path())
        .args(["scan", "--reset-index"])
        .arg(tree.path())
        .assert()
        .success()
        .stdout(contains("±0 seit "));
}

#[test]
fn schlechterer_zustand_senkt_den_score_sichtbar() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    // Müll: viele Temp-Dateien bei wenigen Dateien
    for i in 0..5 {
        std::fs::write(tree.path().join(format!("m{i}.tmp")), "x").unwrap();
    }
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success()
        .stdout(contains("(−"));
}

#[test]
fn beschaedigter_verlauf_warnt_und_bleibt_unveraendert() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    let file = history_file(home.path());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let junk = "das ist keine sqlite-datei, aber lang genug ".repeat(50);
    std::fs::write(&file, &junk).unwrap();

    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success()
        .stdout(contains("3 Dateien"))
        .stderr(contains("Verlauf nicht aktualisiert"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), junk);
}

#[test]
fn belegte_sperre_stoppt_den_scan_mit_klarer_meldung() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    let lock =
        ordner_cleanup::scan::lock::ScanLock::acquire(&home.path().join("data").join("scan.lock"))
            .unwrap();
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .code(1)
        .stderr(contains("anderer Scan läuft"));
    drop(lock);
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
}
