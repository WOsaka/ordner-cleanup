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

fn report_json(home: &Path, tree: &Path) -> serde_json::Value {
    let out = tempfile::tempdir().unwrap();
    bin(home)
        .arg("report")
        .arg(tree)
        .args(["--format", "json,html", "--out"])
        .arg(out.path())
        .assert()
        .success();
    let dir = std::fs::read_dir(out.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let html = std::fs::read_to_string(dir.join("report.html")).unwrap();
    assert!(!html.contains("<script src"), "externes Skript im Bericht");
    assert!(html.contains(r#"<section id="history""#));
    serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap()
}

#[test]
fn erster_bericht_zeigt_erster_lauf_und_score() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    let json = report_json(home.path(), tree.path());
    let h = &json["history"];
    assert_eq!(h["score"], 100);
    assert!(h["comparison"].is_null(), "{h}");
    assert_eq!(h["series"].as_array().unwrap().len(), 1);
    assert!(h["metrics"].as_array().unwrap().len() >= 10);
}

#[test]
fn zweiter_bericht_zeigt_veraenderung_und_abzuege() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    for i in 0..6 {
        std::fs::write(tree.path().join(format!("m{i}.tmp")), "x").unwrap();
    }
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    let json = report_json(home.path(), tree.path());
    let h = &json["history"];
    let c = &h["comparison"];
    assert_eq!(c["kind"], "full", "{h}");
    assert_eq!(c["previous_score"], 100);
    assert!(c["delta"].as_i64().unwrap() < 0);
    assert_eq!(
        h["score"].as_i64().unwrap(),
        100 + c["delta"].as_i64().unwrap()
    );
    assert_eq!(h["series"].as_array().unwrap().len(), 2);
    assert_eq!(h["deductions"][0]["part"], "junk");
    let junk = h["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["key"] == "junk_files")
        .unwrap();
    assert_eq!(
        (junk["now"].as_u64(), junk["previous"].as_u64()),
        (Some(6), Some(0))
    );
}

#[test]
fn geaenderte_config_macht_den_vergleich_eingeschraenkt() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    let config = home.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.toml"), "exclude = [\"*.iso\"]\n").unwrap();
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success()
        .stdout(contains("eingeschränkt vergleichbar"));
    let json = report_json(home.path(), tree.path());
    assert_eq!(json["history"]["comparison"]["kind"], "limited");
    // Mit der ursprünglichen Config ist der erste Lauf wieder voll vergleichbar.
    std::fs::remove_file(config.join("config.toml")).unwrap();
    let json = report_json(home.path(), tree.path());
    assert_eq!(json["history"]["comparison"]["kind"], "full");
}

#[test]
fn report_ohne_verlauf_bleibt_vollstaendig() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    let file = history_file(home.path());
    let junk = "kaputt, aber lang genug ".repeat(80);
    std::fs::write(&file, &junk).unwrap();
    let json = report_json(home.path(), tree.path());
    assert!(json["overview"]["files"].as_u64().unwrap() >= 3);
    let notes = json["history"]["notes"].as_array().unwrap();
    assert!(notes[0]
        .as_str()
        .unwrap()
        .contains("Verlauf nicht verfügbar"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), junk);
}

#[test]
fn history_zeigt_eine_zeile_je_momentaufnahme() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    for _ in 0..3 {
        bin(home.path())
            .arg("scan")
            .arg(tree.path())
            .assert()
            .success();
    }
    let out = bin(home.path())
        .arg("history")
        .assert()
        .success()
        .stdout(contains("Score"))
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(text.lines().count(), 2 + 3, "{text}");
    let out = bin(home.path())
        .args(["history", "--limit", "2", "--folder", "a"])
        .arg(tree.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(String::from_utf8(out).unwrap().lines().count(), 2 + 2);
    bin(home.path())
        .args(["history", "--folder", "gibtsnicht"])
        .assert()
        .code(1)
        .stderr(contains("bekannt: a"));
}

#[test]
fn history_ohne_verlauf_sagt_scan() {
    let home = tempfile::tempdir().unwrap();
    bin(home.path())
        .arg("history")
        .assert()
        .code(1)
        .stderr(contains("zuerst `scan`"));
}
