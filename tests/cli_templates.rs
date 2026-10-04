use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

fn bin(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ordner-cleanup").unwrap();
    cmd.env("ORDNER_CLEANUP_HOME", home);
    cmd
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// PARA-Wurzel mit fehlendem Pflicht-Ordner, unerwartetem Ordner und einer Datei oben.
fn para_tree(root: &Path) {
    write(&root.join("1 Projekte").join("alpha").join("a.txt"), "a");
    write(&root.join("1 Projekte").join("alpha").join("b.txt"), "b");
    write(&root.join("2 Bereiche").join("x").join("c.txt"), "c");
    write(&root.join("2 Bereiche").join("x").join("d.txt"), "d");
    write(&root.join("3 Ressourcen").join("y").join("e.txt"), "e");
    write(&root.join("3 Ressourcen").join("y").join("f.txt"), "f");
    write(&root.join("Sonstiges").join("g.txt"), "g");
    write(&root.join("Sonstiges").join("h.txt"), "h");
    write(&root.join("notiz.txt"), "n");
}

fn report_json(home: &Path, args: &[&str]) -> serde_json::Value {
    let out = tempfile::tempdir().unwrap();
    bin(home)
        .arg("report")
        .args(args)
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
    assert!(html.contains(r#"<section id="template""#));
    serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap()
}

fn items(json: &serde_json::Value) -> Vec<(String, String)> {
    json["template"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            let path = i["path"].as_str().unwrap();
            let name = path.rsplit('\\').next().unwrap().to_string();
            (i["kind"].as_str().unwrap().to_string(), name)
        })
        .collect()
}

fn part(json: &serde_json::Value, key: &str) -> Option<f64> {
    json["history"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["key"] == key)
        .and_then(|p| p["value"].as_f64())
}

#[test]
fn para_zeigt_genau_die_abweichungen_und_senkt_die_strukturtreue() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    para_tree(tree.path());
    bin(home.path())
        .args(["scan", "--template", "para"])
        .arg(tree.path())
        .assert()
        .success();
    let json = report_json(
        home.path(),
        &[tree.path().to_str().unwrap(), "--template", "para"],
    );
    assert_eq!(
        items(&json),
        [
            ("missing_required".to_string(), "4 Archiv".to_string()),
            ("unexpected_dir".to_string(), "Sonstiges".to_string()),
            ("file_not_allowed".to_string(), "notiz.txt".to_string()),
        ]
    );
    let t = &json["template"];
    assert_eq!(t["name"], "PARA");
    assert_eq!(t["total"], 3);
    assert_eq!(
        t["checked"], 6,
        "4 Ordner + 1 Datei oben + 1 fehlender Pflicht-Ordner"
    );
    // Strukturtreue: 3 von 6 weichen ab, also 50 % > 30 %-Schwelle
    assert_eq!(part(&json, "template"), Some(0.0));
    // ohne Vorlage gibt es den Teilwert nicht
    let plain = report_json(home.path(), &[tree.path().to_str().unwrap()]);
    assert!(plain["template"].is_null());
    assert_eq!(part(&plain, "template"), None);
    assert!(
        json["history"]["score"].as_i64().unwrap() < 100,
        "Vorlage senkt den Score"
    );
}

#[test]
fn passende_struktur_hat_keine_abweichungen() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    for d in ["1 Projekte", "2 Bereiche", "3 Ressourcen", "4 Archiv"] {
        write(
            &tree.path().join(d).join("sub").join("a.txt"),
            &format!("a {d}"),
        );
        write(
            &tree.path().join(d).join("sub").join("b.txt"),
            &format!("b {d}"),
        );
    }
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    let json = report_json(
        home.path(),
        &[tree.path().to_str().unwrap(), "--template", "para"],
    );
    assert_eq!(json["template"]["total"], 0);
    assert_eq!(part(&json, "template"), Some(100.0));
    assert_eq!(json["history"]["score"], 100);
}

#[test]
fn johnny_decimal_und_eigene_vorlage_funktionieren() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    write(
        &tree
            .path()
            .join("10-19 Verwaltung")
            .join("11 Finanzen")
            .join("11.01 Rechnungen")
            .join("a.pdf"),
        "a",
    );
    write(&tree.path().join("Durcheinander").join("b.txt"), "b");
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    let json = report_json(
        home.path(),
        &[
            tree.path().to_str().unwrap(),
            "--template",
            "johnny-decimal",
        ],
    );
    assert_eq!(
        items(&json),
        [
            ("bad_name".to_string(), "Durcheinander".to_string()),
            // die Datei liegt auf Ebene 2, und dort sind keine Dateien vorgesehen
            ("file_not_allowed".to_string(), "b.txt".to_string()),
        ]
    );

    // eigene Vorlage: nur „Verwaltung“ und „Archiv“ oben erlaubt, höchstens zwei Ordner-Ebenen
    let template = home.path().join("meine.toml");
    write(
        &template,
        "name = 'Eigene'\nmax_depth = 2\n[[levels]]\nallowed = ['Verwaltung', 'Archiv']\nallow_other = false\n",
    );
    let json = report_json(
        home.path(),
        &[
            tree.path().to_str().unwrap(),
            "--template",
            template.to_str().unwrap(),
        ],
    );
    assert_eq!(json["template"]["name"], "Eigene");
    let found = items(&json);
    assert!(found.contains(&("unexpected_dir".to_string(), "10-19 Verwaltung".to_string())));
    assert!(found.contains(&("unexpected_dir".to_string(), "Durcheinander".to_string())));
    assert!(found.contains(&("too_deep".to_string(), "11.01 Rechnungen".to_string())));
}

#[test]
fn ungueltige_vorlage_nennt_datei_und_zeile() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    write(&tree.path().join("a.txt"), "a");
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    let bad = home.path().join("kaputt.toml");
    write(&bad, "name = 'X'\n[[levels]]\npatterns = ['(offen']\n");
    bin(home.path())
        .arg("report")
        .arg(tree.path())
        .args(["--template", bad.to_str().unwrap()])
        .assert()
        .code(1)
        .stderr(contains("kaputt.toml").and(contains("Zeile 3")));
    let unknown = home.path().join("unbekannt.toml");
    write(&unknown, "name = 'X'\nfoo = 1\n[[levels]]\n");
    bin(home.path())
        .args(["scan", "--template", unknown.to_str().unwrap()])
        .arg(tree.path())
        .assert()
        .code(1)
        .stderr(
            contains("unbekannt.toml")
                .and(contains("Zeile 2"))
                .and(contains("foo")),
        );
    bin(home.path())
        .args(["scan", "--template", "gibtsnicht"])
        .arg(tree.path())
        .assert()
        .code(1)
        .stderr(contains("para").and(contains("johnny-decimal")));
}

#[test]
fn profil_vorlage_gilt_fuer_scan_und_report_und_aendert_den_fingerabdruck() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    para_tree(tree.path());
    // erster Lauf ohne Vorlage
    bin(home.path())
        .arg("scan")
        .arg(tree.path())
        .assert()
        .success();
    let config = home.path().join("config").join("config.toml");
    write(
        &config,
        &format!(
            "[profiles.p]\nroot = '{}'\ntemplate = \"para\"\n",
            tree.path().display()
        ),
    );
    // zweiter Lauf mit Vorlage: nicht direkt vergleichbar
    bin(home.path())
        .args(["scan", "--profile", "p"])
        .assert()
        .success()
        .stdout(contains("eingeschränkt vergleichbar"));
    let json = report_json(home.path(), &["--profile", "p"]);
    assert_eq!(json["template"]["total"], 3);
    // relativer Vorlagenpfad im Profil gilt ab dem Config-Ordner
    write(
        &home.path().join("config").join("eigen.toml"),
        "name = 'Nur oben'\n[[levels]]\nfiles = false\n",
    );
    write(
        &config,
        &format!(
            "[profiles.p]\nroot = '{}'\ntemplate = \"eigen.toml\"\n",
            tree.path().display()
        ),
    );
    let json = report_json(home.path(), &["--profile", "p"]);
    assert_eq!(json["template"]["name"], "Nur oben");
    assert_eq!(
        items(&json),
        [("file_not_allowed".to_string(), "notiz.txt".to_string())]
    );
}

#[test]
fn terminal_nennt_die_abweichungen() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    para_tree(tree.path());
    let out = tempfile::tempdir().unwrap();
    bin(home.path())
        .args(["scan"])
        .arg(tree.path())
        .assert()
        .success();
    bin(home.path())
        .arg("report")
        .arg(tree.path())
        .args(["--template", "para", "--format", "json", "--out"])
        .arg(out.path())
        .assert()
        .success()
        .stdout(contains(
            "Vorlage PARA: 3 Abweichungen bei 6 geprüften Einträgen",
        ));
}
