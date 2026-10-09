//! Plan-Integrität über die echte CLI: Prüfsumme in der Plan-Datei, Abbruch mit Exit-Code 3 bei
//! Veränderung, Warnung bei alten Plänen und `plan seal`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use assert_cmd::assert::Assert;
use assert_cmd::Command;
use ordner_cleanup::paths;
use predicates::str::contains;

const CONTENT: &str = "gleicher inhalt";

struct Env {
    home: tempfile::TempDir,
    tree: tempfile::TempDir,
    out: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let env = Self {
            home: tempfile::tempdir().unwrap(),
            tree: tempfile::tempdir().unwrap(),
            out: tempfile::tempdir().unwrap(),
        };
        let orig = env.write("a/orig.txt", CONTENT);
        env.write("b/kopie.txt", CONTENT);
        env.write("c/kopie 2.txt", CONTENT);
        let file = std::fs::File::options()
            .write(true)
            .open(paths::extended(&orig))
            .unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(1000))
            .unwrap();
        env
    }

    fn root(&self) -> &Path {
        self.tree.path()
    }

    fn bin(&self) -> Command {
        let mut cmd = Command::cargo_bin("ordner-cleanup").unwrap();
        cmd.env("ORDNER_CLEANUP_HOME", self.home.path())
            .current_dir(self.out.path());
        cmd
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.root().join(rel);
        std::fs::create_dir_all(paths::extended(path.parent().unwrap())).unwrap();
        std::fs::write(paths::extended(&path), content).unwrap();
        path
    }

    /// `scan` und `plan dedupe`; liefert den Pfad der Plan-Datei und die Ausgabe von `plan`.
    fn plan(&self) -> (PathBuf, String) {
        self.bin().arg("scan").arg(self.root()).assert().success();
        let plan = self.out.path().join("plan.json");
        let text = stdout(
            self.bin()
                .args(["plan", "dedupe"])
                .arg(self.root())
                .arg("--out")
                .arg(&plan)
                .assert()
                .success(),
        );
        (plan, text)
    }

    fn apply(&self, plan: &Path) -> Assert {
        self.bin().arg("apply").arg(plan).arg("--yes").assert()
    }

    fn assert_no_runs(&self) {
        self.bin()
            .arg("runs")
            .assert()
            .success()
            .stdout(contains("Keine Läufe gefunden."));
    }

    /// Dateien unter der Wurzel mit Inhalt (ohne den Werkzeugordner).
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.file_name().is_some_and(|n| n == ".ordner-cleanup") {
                    continue;
                }
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    let content = std::fs::read(&path).unwrap();
                    out.insert(path, content);
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&paths::extended(self.root()), &mut out);
        out
    }
}

fn stdout(assert: Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

fn plan_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn write_json(path: &Path, json: &serde_json::Value) {
    std::fs::write(path, serde_json::to_string_pretty(json).unwrap()).unwrap();
}

#[test]
fn geaenderter_plan_bricht_mit_exit_code_3_ab_und_aendert_nichts() {
    let env = Env::new();
    let (plan, _) = env.plan();
    let mut json = plan_json(&plan);
    json["actions"][0]["reason"] = "von Hand".into();
    write_json(&plan, &json);
    let before = env.snapshot();

    env.apply(&plan)
        .code(3)
        .stderr(contains("Plan wurde nach dem Erstellen verändert"))
        .stderr(contains("plan seal"));

    assert_eq!(env.snapshot(), before);
    env.assert_no_runs();
}

#[test]
fn entfernte_aktion_bricht_mit_exit_code_3_ab() {
    let env = Env::new();
    let (plan, _) = env.plan();
    let mut json = plan_json(&plan);
    json["actions"].as_array_mut().unwrap().pop();
    write_json(&plan, &json);
    let before = env.snapshot();

    env.apply(&plan).code(3);

    assert_eq!(env.snapshot(), before);
    env.assert_no_runs();
}

#[test]
fn nur_formatierung_zeilenenden_und_schluesselreihenfolge_geaendert_laeuft_durch() {
    let env = Env::new();
    let (plan, _) = env.plan();
    // `Value` sortiert die Schlüssel alphabetisch; dazu kompakt und mit CRLF.
    let text = plan_json(&plan).to_string().replace(',', ",\r\n");
    std::fs::write(&plan, text).unwrap();

    env.apply(&plan).success().stdout(contains("Lauf "));
}

#[test]
fn plan_nennt_die_pruefsumme_gekuerzt() {
    let env = Env::new();
    let (plan, text) = env.plan();
    let integrity = plan_json(&plan)["integrity"].as_str().unwrap().to_owned();
    assert!(
        text.contains(&format!("Prüfsumme: {}…", &integrity[..19])),
        "{text}"
    );
}

/// Ein Plan im Format 1 (Phase 2): ohne Prüfsumme und ohne v2/v3-Felder.
fn downgrade_to_v1(plan: &Path) {
    let mut json = plan_json(plan);
    json.as_object_mut().unwrap().remove("integrity");
    json["version"] = 1.into();
    write_json(plan, &json);
}

#[test]
fn alter_plan_laeuft_mit_warnung_durch() {
    let env = Env::new();
    let (plan, _) = env.plan();
    downgrade_to_v1(&plan);

    env.apply(&plan)
        .success()
        .stderr(contains("Ungeschützter Plan (Format 1)"))
        .stdout(contains("Lauf "));
}

fn seal(env: &Env, plan: &Path, yes: bool) -> Assert {
    let mut cmd = env.bin();
    cmd.args(["plan", "seal"]).arg(plan);
    if yes {
        cmd.arg("--yes");
    }
    cmd.assert()
}

#[test]
fn geaenderter_plan_laeuft_nach_seal_durch() {
    let env = Env::new();
    let (plan, _) = env.plan();
    let mut json = plan_json(&plan);
    json["actions"].as_array_mut().unwrap().pop();
    write_json(&plan, &json);

    let text = stdout(seal(&env, &plan, true).success());
    assert!(text.contains("1 Aktionen"), "{text}");
    assert!(text.contains("passt nicht zum Inhalt"), "{text}");
    assert!(text.contains("Plan versiegelt"), "{text}");
    let sealed = plan_json(&plan);
    assert_ne!(sealed["integrity"], json["integrity"]);
    assert_eq!(sealed["actions"], json["actions"]);

    env.apply(&plan).success().stdout(contains("Lauf "));
}

#[test]
fn seal_ohne_bestaetigung_laesst_die_datei_unveraendert() {
    let env = Env::new();
    let (plan, _) = env.plan();
    let mut json = plan_json(&plan);
    json["actions"].as_array_mut().unwrap().pop();
    write_json(&plan, &json);
    let before = std::fs::read(&plan).unwrap();

    seal(&env, &plan, false).code(1).stderr(contains("--yes"));

    assert_eq!(std::fs::read(&plan).unwrap(), before);
    env.apply(&plan).code(3);
}

#[test]
fn seal_auf_versiegeltem_plan_aendert_nichts() {
    let env = Env::new();
    let (plan, _) = env.plan();
    let before = std::fs::read(&plan).unwrap();

    seal(&env, &plan, false)
        .success()
        .stdout(contains("Plan ist bereits versiegelt"));

    assert_eq!(std::fs::read(&plan).unwrap(), before);
}

#[test]
fn seal_hebt_alten_plan_auf_format_3() {
    let env = Env::new();
    let (plan, _) = env.plan();
    downgrade_to_v1(&plan);

    seal(&env, &plan, true)
        .success()
        .stdout(contains("keine Prüfsumme (Format 1)"))
        .stdout(contains("wird auf Format 3 gehoben"));

    let json = plan_json(&plan);
    assert_eq!(json["version"], 3);
    assert!(json["integrity"].as_str().unwrap().starts_with("sha256:"));
    let out = env.apply(&plan).success();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(!stderr.contains("Ungeschützter Plan"), "{stderr}");
}

#[test]
fn strukturell_ungueltiger_plan_wird_nicht_versiegelt() {
    let env = Env::new();
    let (plan, _) = env.plan();
    let mut json = plan_json(&plan);
    json["actions"][1]["id"] = json["actions"][0]["id"].clone();
    write_json(&plan, &json);
    let before = std::fs::read(&plan).unwrap();

    seal(&env, &plan, true)
        .code(1)
        .stderr(contains("Plan ist ungültig"));

    assert_eq!(std::fs::read(&plan).unwrap(), before);
}
