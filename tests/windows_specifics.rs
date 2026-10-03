//! Windows-Besonderheiten mit echten Temp-Verzeichnissen: Junctions, lange Pfade,
//! versteckte Dateien, gesperrte Dateien und verweigerter Zugriff.

use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use assert_cmd::assert::Assert;
use assert_cmd::Command;
use rusqlite::Connection;

fn scan(home: &Path, tree: &Path) -> Assert {
    Command::cargo_bin("ordner-cleanup")
        .unwrap()
        .env("ORDNER_CLEANUP_HOME", home)
        .arg("scan")
        .arg(tree)
        .assert()
}

fn db(home: &Path) -> Connection {
    Connection::open(home.join("data").join("index.db")).unwrap()
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn junction_wird_erfasst_aber_nicht_betreten() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("fremd.txt"), "x").unwrap();
    std::fs::write(tree.path().join("eigen.txt"), "y").unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(tree.path().join("link"))
        .arg(outside.path())
        .output()
        .unwrap();
    assert!(status.status.success(), "mklink /J fehlgeschlagen");

    scan(home.path(), tree.path()).success();

    let conn = db(home.path());
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM files"), 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM dirs WHERE is_link=1"), 1);
}

#[test]
fn lange_pfade_ueber_260_zeichen_werden_gescannt() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let mut deep = tree.path().to_path_buf();
    while deep.to_string_lossy().len() < 300 {
        deep.push("sehr-langer-ordnername-zum-testen");
    }
    let extended = format!(r"\\?\{}", deep.display());
    std::fs::create_dir_all(&extended).unwrap();
    std::fs::write(Path::new(&extended).join("tief.txt"), "t").unwrap();

    scan(home.path(), tree.path()).success();

    let conn = db(home.path());
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM files WHERE name='tief.txt'"),
        1
    );
    assert!(count(&conn, "SELECT MAX(LENGTH(path)) FROM files") > 260);
}

#[test]
fn versteckte_dateien_behalten_ihr_attribut() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let hidden = tree.path().join("versteckt.txt");
    std::fs::write(&hidden, "h").unwrap();
    std::fs::write(tree.path().join("sichtbar.txt"), "s").unwrap();
    let status = std::process::Command::new("attrib")
        .arg("+h")
        .arg(&hidden)
        .status()
        .unwrap();
    assert!(status.success());

    scan(home.path(), tree.path()).success();

    let conn = db(home.path());
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM files WHERE (attrs & 2) != 0"),
        1
    );
}

#[test]
fn gesperrte_datei_wird_als_locked_gemeldet() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let locked = tree.path().join("gesperrt.dat");
    std::fs::write(&locked, "gleicher inhalt").unwrap();
    std::fs::write(tree.path().join("frei.dat"), "gleicher inhalt").unwrap();
    // share_mode(0): niemand sonst darf die Datei öffnen.
    let _guard = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&locked)
        .unwrap();

    scan(home.path(), tree.path()).code(2);

    let conn = db(home.path());
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM files WHERE name='gesperrt.dat' AND hash_status='locked'"
        ),
        1
    );
}

/// Setzt die ACLs beim Verlassen zurück, damit das Temp-Verzeichnis gelöscht werden kann.
struct AclReset<'a>(&'a Path);

impl Drop for AclReset<'_> {
    fn drop(&mut self) {
        let _ = std::process::Command::new("icacls")
            .arg(self.0)
            .args(["/reset", "/T", "/C"])
            .output();
    }
}

#[test]
fn verweigerter_zugriff_ist_teilfehler_mit_exit_code_2() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let denied = tree.path().join("gesperrt");
    std::fs::create_dir(&denied).unwrap();
    std::fs::write(tree.path().join("ok.txt"), "o").unwrap();
    let _reset = AclReset(&denied);
    let status = std::process::Command::new("icacls")
        .arg(&denied)
        .args(["/deny", "*S-1-1-0:(RX)"])
        .output()
        .unwrap();
    assert!(status.status.success(), "icacls /deny fehlgeschlagen");

    scan(home.path(), tree.path()).code(2);

    let conn = db(home.path());
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM files"), 1);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM errors WHERE kind='access_denied'"
        ),
        1
    );
}
