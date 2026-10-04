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
    std::fs::write(dir.join("a").join("m.tmp"), "x").unwrap();
}

fn write_config(home: &Path, text: &str) {
    let dir = home.join("config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), text).unwrap();
}

/// Config mit einem Profil `t` auf `root`; `extra` landet im Profil.
fn profile_config(root: &Path, extra: &str) -> String {
    format!("[profiles.t]\nroot = '{}'\n{extra}\n", root.display())
}

#[test]
fn scan_report_plan_und_history_laufen_ueber_das_profil() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(home.path(), &profile_config(tree.path(), ""));

    bin(home.path())
        .args(["scan", "--profile", "t"])
        .assert()
        .success()
        .stdout(contains("4 Dateien"));
    bin(home.path())
        .args(["report", "--profile", "t", "--format", "json", "--out"])
        .arg(out.path())
        .assert()
        .success();
    let plan = out.path().join("plan.json");
    bin(home.path())
        .args(["plan", "junk", "--profile", "t", "--out"])
        .arg(&plan)
        .assert()
        .success()
        .stdout(contains("1 Aktionen"));
    assert!(plan.exists());
    bin(home.path())
        .args(["history", "--profile", "t"])
        .assert()
        .success()
        .stdout(contains("Score"));
}

#[test]
fn profil_excludes_ergaenzen_die_config() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(
        home.path(),
        &format!(
            "exclude = [\"y.txt\"]\n{}",
            profile_config(tree.path(), "exclude = [\"*.tmp\"]")
        ),
    );
    // y.txt (global) und m.tmp (Profil) fallen weg: 2 von 4 Dateien bleiben
    bin(home.path())
        .args(["scan", "--profile", "t"])
        .assert()
        .success()
        .stdout(contains("2 Dateien"));
}

#[test]
fn cli_angaben_haben_vorrang_vor_dem_profil() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(home.path(), &profile_config(tree.path(), "threads = 1"));
    // --exclude ergänzt zusätzlich zum Profil
    bin(home.path())
        .args(["scan", "--profile", "t", "--exclude", "*.txt"])
        .assert()
        .success()
        .stdout(contains("1 Dateien"));
}

#[test]
fn unbekanntes_profil_nennt_die_vorhandenen() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    write_config(home.path(), &profile_config(tree.path(), ""));
    bin(home.path())
        .args(["scan", "--profile", "nope"])
        .assert()
        .code(1)
        .stderr(contains("Unbekanntes Profil 'nope'").and(contains("Vorhandene Profile: t")));
}

#[test]
fn pfad_und_profil_zusammen_sind_ein_fehler() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    write_config(home.path(), &profile_config(tree.path(), ""));
    for args in [
        vec!["scan", "--profile", "t", "X:\\irgendwas"],
        vec!["plan", "junk", "--profile", "t", "X:\\irgendwas"],
        vec!["report", "--profile", "t", "X:\\irgendwas"],
    ] {
        bin(home.path())
            .args(&args)
            .assert()
            .failure()
            .stderr(contains("cannot be used with"));
    }
    bin(home.path()).arg("scan").assert().failure();
}

#[test]
fn ungueltiges_profil_scheitert_beim_laden_mit_schluessel() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    write_config(
        home.path(),
        &profile_config(tree.path(), "plans = [\"apply\"]"),
    );
    bin(home.path())
        .args(["scan", "--profile", "t"])
        .assert()
        .code(1)
        .stderr(contains("profiles.t").and(contains("apply")));
    write_config(home.path(), &profile_config(tree.path(), "muell = 1"));
    bin(home.path())
        .args(["profiles"])
        .assert()
        .code(1)
        .stderr(contains("muell"));
}

#[test]
fn regeldatei_des_profils_ist_relativ_zum_config_ordner() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(
        home.path(),
        &profile_config(tree.path(), "rules_file = \"meine.rules.toml\""),
    );
    std::fs::write(
        home.path().join("config").join("meine.rules.toml"),
        "[[rules]]\nname = \"txt\"\next = [\"txt\"]\ntarget = \"Texte/\"\n",
    )
    .unwrap();
    bin(home.path())
        .args(["scan", "--profile", "t"])
        .assert()
        .success();
    bin(home.path())
        .args(["plan", "rules", "--profile", "t", "--out"])
        .arg(out.path().join("p.json"))
        .assert()
        .success()
        .stdout(contains("Regel „txt“"));
}

#[test]
fn profiles_listet_wurzel_plaene_und_score() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(
        home.path(),
        &profile_config(tree.path(), "plans = [\"junk\"]"),
    );
    bin(home.path())
        .arg("profiles")
        .assert()
        .success()
        .stdout(contains("noch kein Verlauf"));
    bin(home.path())
        .args(["scan", "--profile", "t"])
        .assert()
        .success();
    bin(home.path()).arg("profiles").assert().success().stdout(
        contains("t\n")
            .and(contains("Pläne:    junk"))
            .and(contains("Score ")),
    );
}

#[test]
fn profiles_ohne_profile_erklaert_die_config() {
    let home = tempfile::tempdir().unwrap();
    bin(home.path())
        .arg("profiles")
        .assert()
        .success()
        .stdout(contains("[profiles.<name>]"));
}

use predicates::prelude::PredicateBooleanExt;
