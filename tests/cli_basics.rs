use assert_cmd::Command;
use predicates::str::contains;

fn bin() -> Command {
    Command::cargo_bin("ordner-cleanup").unwrap()
}

#[test]
fn version_laeuft() {
    bin()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("ordner-cleanup"));
}

#[test]
fn help_nennt_alle_befehle() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("scan"))
        .stdout(contains("report"))
        .stdout(contains("index"));
}

#[test]
fn unterbefehle_haben_hilfe() {
    for args in [
        ["scan", "--help"],
        ["report", "--help"],
        ["index", "--help"],
    ] {
        bin().args(args).assert().success();
    }
}
