//! Ende-zu-Ende über die echte CLI: scan → plan → apply → undo, mit Temp-Ordnern.

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

type Snapshot = BTreeMap<String, (u64, SystemTime, Vec<u8>)>;

impl Env {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            tree: tempfile::tempdir().unwrap(),
            out: tempfile::tempdir().unwrap(),
        }
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

    /// Setzt die Änderungszeit auf „vor `secs` Sekunden“ (steuert `--keep oldest`).
    fn age(&self, path: &Path, secs: u64) {
        let file = std::fs::File::options()
            .write(true)
            .open(paths::extended(path))
            .unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(secs))
            .unwrap();
    }

    fn scan(&self) {
        self.bin().arg("scan").arg(self.root()).assert().success();
    }

    /// `plan dedupe` und Pfad der Plan-Datei (außerhalb des Baums).
    fn plan(&self, name: &str) -> PathBuf {
        self.scan();
        let plan = self.out.path().join(name);
        self.bin()
            .args(["plan", "dedupe"])
            .arg(self.root())
            .arg("--out")
            .arg(&plan)
            .assert()
            .success();
        plan
    }

    fn apply(&self, plan: &Path) -> Assert {
        self.bin().arg("apply").arg(plan).arg("--yes").assert()
    }

    fn quarantine_root(&self) -> PathBuf {
        self.root().join(".ordner-cleanup").join("quarantine")
    }

    fn snapshot(&self) -> Snapshot {
        fn walk(dir: &Path, root: &Path, out: &mut Snapshot) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.file_name().is_some_and(|n| n == ".ordner-cleanup") {
                    continue;
                }
                let meta = std::fs::metadata(&path).unwrap();
                let key = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if meta.is_dir() {
                    // Die Änderungszeit eines Ordners ändert Windows selbst, sobald Einträge
                    // verschwinden oder zurückkommen; verglichen wird nur seine Existenz.
                    out.insert(key, (0, SystemTime::UNIX_EPOCH, Vec::new()));
                    walk(&path, root, out);
                } else {
                    out.insert(
                        key,
                        (
                            meta.len(),
                            meta.modified().unwrap(),
                            std::fs::read(&path).unwrap(),
                        ),
                    );
                }
            }
        }
        let root = paths::extended(self.root());
        let mut out = Snapshot::new();
        walk(&root, &root, &mut out);
        out
    }
}

fn stdout(assert: Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

/// Lauf-ID aus „Lauf <id>: …“.
fn run_id(text: &str) -> String {
    let rest = text.split("Lauf ").nth(1).expect("Zeile „Lauf …“ fehlt");
    rest.split(':').next().unwrap().trim().to_string()
}

fn plan_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Ordner mit einem älteren Original und zwei Kopien (Umlaute, Leerzeichen, Unterordner).
fn dup_tree(env: &Env) {
    let orig = env.write("a/orig.txt", CONTENT);
    env.write("b/kopie.txt", CONTENT);
    env.write("c/Übergröße Ordner/kopie 2.txt", CONTENT);
    env.write("solo.txt", "ganz anderer inhalt");
    env.age(&orig, 1000);
}

#[test]
fn plan_veraendert_den_baum_nicht_und_schreibt_nur_die_plan_datei() {
    let env = Env::new();
    dup_tree(&env);
    env.scan();
    let before = env.snapshot();

    let plan = env.out.path().join("plan.json");
    let text = stdout(
        env.bin()
            .args(["plan", "dedupe"])
            .arg(env.root())
            .arg("--out")
            .arg(&plan)
            .assert()
            .success()
            .stdout(contains("2 Aktionen"))
            .stdout(contains("nichts verändert")),
    );

    assert!(text.contains("Plan:"));
    assert_eq!(env.snapshot(), before);
    assert!(!env.root().join(".ordner-cleanup").exists());
    assert_eq!(plan_json(&plan)["actions"].as_array().unwrap().len(), 2);
}

#[test]
fn plan_ohne_out_legt_die_datei_im_aktuellen_ordner_ab() {
    let env = Env::new();
    dup_tree(&env);
    env.scan();
    env.bin()
        .args(["plan", "dedupe"])
        .arg(env.root())
        .assert()
        .success();
    let found = std::fs::read_dir(env.out.path())
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("plan-"));
    assert!(found);
}

#[test]
fn plan_ohne_scan_nennt_den_naechsten_schritt() {
    let env = Env::new();
    dup_tree(&env);
    env.bin()
        .args(["plan", "dedupe"])
        .arg(env.root())
        .assert()
        .failure()
        .stderr(contains("zuerst `scan`"));
}

#[test]
fn plan_apply_undo_stellt_den_ausgangszustand_byteidentisch_her() {
    let env = Env::new();
    dup_tree(&env);
    let before = env.snapshot();
    let plan = env.plan("plan.json");

    let applied = stdout(env.apply(&plan).success().stdout(contains("2 verschoben")));
    let run = run_id(&applied);

    let during = env.snapshot();
    assert!(during.contains_key("a\\orig.txt") && during.contains_key("solo.txt"));
    assert!(!during.contains_key("b\\kopie.txt"));
    assert!(!during.contains_key("c\\Übergröße Ordner\\kopie 2.txt"));
    assert!(env
        .quarantine_root()
        .join(&run)
        .join("b")
        .join("kopie.txt")
        .exists());
    assert!(env
        .root()
        .join(".ordner-cleanup")
        .join("journal")
        .join(format!("{run}.jsonl"))
        .exists());
    env.bin()
        .arg("runs")
        .assert()
        .success()
        .stdout(contains(run.as_str()))
        .stdout(contains("vollständig"));

    env.bin()
        .args(["undo", &run, "--yes"])
        .assert()
        .success()
        .stdout(contains("2 wiederhergestellt"));

    assert_eq!(env.snapshot(), before, "Pfad, Inhalt und Zeit identisch");
    env.bin()
        .arg("runs")
        .assert()
        .success()
        .stdout(contains("zurückgedreht"));
    env.bin()
        .args(["undo", &run, "--yes"])
        .assert()
        .success()
        .stdout(contains("bereits zurückgedreht"));
}

#[test]
fn zweiter_apply_aendert_nichts() {
    let env = Env::new();
    dup_tree(&env);
    let plan = env.plan("plan.json");
    env.apply(&plan).success();
    let after_first = env.snapshot();

    env.apply(&plan)
        .success()
        .stdout(contains("2 bereits erledigt"));

    assert_eq!(env.snapshot(), after_first);
    let runs = std::fs::read_dir(env.quarantine_root()).unwrap().count();
    assert_eq!(runs, 1, "der zweite Lauf legt keine Quarantäne an");
}

#[test]
fn geaenderte_datei_ist_stale_der_rest_laeuft_exit_2() {
    let env = Env::new();
    dup_tree(&env);
    let plan = env.plan("plan.json");
    env.write("b/kopie.txt", "inzwischen etwas völlig anderes");

    env.apply(&plan)
        .code(2)
        .stdout(contains("1 verschoben"))
        .stdout(contains("1 stale"));

    assert!(env.root().join("b/kopie.txt").exists());
    assert!(!env.root().join("c/Übergröße Ordner/kopie 2.txt").exists());
}

#[test]
fn apply_ohne_yes_bricht_in_nicht_interaktiver_sitzung_ab() {
    let env = Env::new();
    dup_tree(&env);
    let plan = env.plan("plan.json");
    let before = env.snapshot();

    for input in ["j\n", "n\n", ""] {
        env.bin()
            .arg("apply")
            .arg(&plan)
            .write_stdin(input)
            .assert()
            .code(1)
            .stderr(contains("--yes"));
    }

    assert_eq!(env.snapshot(), before);
    assert!(!env.root().join(".ordner-cleanup").exists());
}

#[test]
fn undo_meldet_kollision_und_ueberschreibt_nichts() {
    let env = Env::new();
    dup_tree(&env);
    let plan = env.plan("plan.json");
    let run = run_id(&stdout(env.apply(&plan).success()));
    env.write("b/kopie.txt", "neue datei am alten ort");

    env.bin()
        .args(["undo", &run, "--yes"])
        .assert()
        .code(2)
        .stdout(contains("1 Kollisionen"))
        .stdout(contains("Kollision"));

    assert_eq!(
        std::fs::read_to_string(env.root().join("b/kopie.txt")).unwrap(),
        "neue datei am alten ort"
    );
    assert!(env
        .quarantine_root()
        .join(&run)
        .join("b")
        .join("kopie.txt")
        .exists());
    assert!(env.root().join("c/Übergröße Ordner/kopie 2.txt").exists());
}

#[test]
fn unbekannter_oder_ungueltiger_lauf() {
    let env = Env::new();
    env.bin()
        .args(["undo", "20261003-120000-ab12", "--yes"])
        .assert()
        .failure()
        .stderr(contains("unbekannt"));
    env.bin()
        .args(["undo", "..\\..\\Windows", "--yes"])
        .assert()
        .code(2)
        .stderr(contains("Ungültige Lauf-ID"));
}

#[test]
fn plan_mit_unbekannter_version_wird_klar_abgelehnt() {
    let env = Env::new();
    dup_tree(&env);
    let plan = env.plan("plan.json");
    let mut json = plan_json(&plan);
    json["version"] = 2.into();
    std::fs::write(&plan, json.to_string()).unwrap();
    let before = env.snapshot();

    env.apply(&plan).code(1).stderr(contains("Version 2"));

    assert_eq!(env.snapshot(), before);
}

#[test]
fn purge_loescht_nur_abgelaufene_laeufe_und_nur_nach_bestaetigung() {
    let env = Env::new();
    dup_tree(&env);
    let plan = env.plan("plan1.json");
    let old_run = run_id(&stdout(env.apply(&plan).success()));

    // Zweiter, jüngerer Lauf mit neuen Duplikaten.
    let orig = env.write("x/orig2.txt", "zweite gruppe");
    env.write("y/kopie2.txt", "zweite gruppe");
    env.age(&orig, 500);
    let plan2 = env.plan("plan2.json");
    let new_run = run_id(&stdout(env.apply(&plan2).success()));
    assert_ne!(old_run, new_run);

    // Den ersten Lauf künstlich altern lassen (Startzeit im Journal).
    let journal = env
        .root()
        .join(".ordner-cleanup")
        .join("journal")
        .join(format!("{old_run}.jsonl"));
    let text = std::fs::read_to_string(&journal).unwrap();
    let start = text.find("\"started\":\"").unwrap() + "\"started\":\"".len();
    let end = start + text[start..].find('"').unwrap();
    let aged = format!("{}2020-01-01T00:00:00Z{}", &text[..start], &text[end..]);
    std::fs::write(&journal, aged).unwrap();

    // Ohne --yes in nicht interaktiver Sitzung: nichts wird gelöscht.
    env.bin()
        .arg("purge")
        .assert()
        .code(1)
        .stderr(contains("--yes"));
    assert!(env.quarantine_root().join(&old_run).exists());

    env.bin()
        .args(["purge", "--yes"])
        .assert()
        .success()
        .stdout(contains(old_run.as_str()));

    assert!(!env.quarantine_root().join(&old_run).exists());
    assert!(
        env.quarantine_root().join(&new_run).exists(),
        "jüngerer Lauf bleibt"
    );
    assert!(journal.exists(), "Journal bleibt als Nachweis");
    env.bin()
        .arg("runs")
        .assert()
        .success()
        .stdout(contains("Quarantäne gelöscht"));
    env.bin()
        .args(["undo", &old_run, "--yes"])
        .assert()
        .code(2)
        .stdout(contains("nicht mehr wiederherstellbar"));
    env.bin()
        .args(["undo", &new_run, "--yes"])
        .assert()
        .success();
}

#[test]
fn lange_pfade_umlaute_und_leerzeichen_laufen_durch() {
    let env = Env::new();
    let orig = env.write("orig datei.txt", CONTENT);
    env.age(&orig, 1000);
    let deep: PathBuf = (0..6)
        .map(|i| format!("Ünterordner mit Leerzeichen {i} {}", "x".repeat(40)))
        .collect();
    let kopie = env.write(&format!("{}/kopie ä.txt", deep.display()), CONTENT);
    assert!(
        paths::display(&kopie).len() > 300,
        "Test braucht einen langen Pfad"
    );
    let before = env.snapshot();
    let plan = env.plan("plan.json");

    let run = run_id(&stdout(
        env.apply(&plan).success().stdout(contains("1 verschoben")),
    ));
    assert!(!std::fs::exists(paths::extended(&kopie)).unwrap());

    env.bin().args(["undo", &run, "--yes"]).assert().success();
    assert_eq!(env.snapshot(), before);
}

#[test]
fn hardlinks_der_behaltenen_datei_werden_nicht_angefasst() {
    let env = Env::new();
    let orig = env.write("orig.txt", CONTENT);
    env.age(&orig, 1000);
    std::fs::hard_link(&orig, env.root().join("link.txt")).unwrap();
    env.write("kopie.txt", CONTENT);
    let plan = env.plan("plan.json");

    let json = plan_json(&plan);
    assert_eq!(json["actions"].as_array().unwrap().len(), 1);
    assert!(json["actions"][0]["path"]
        .as_str()
        .unwrap()
        .ends_with("kopie.txt"));

    env.apply(&plan).success();

    assert_eq!(
        std::fs::read_to_string(env.root().join("link.txt")).unwrap(),
        CONTENT
    );
    assert!(!env.root().join("kopie.txt").exists());
}
