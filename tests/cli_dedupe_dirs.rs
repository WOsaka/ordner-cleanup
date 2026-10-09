//! Ende-zu-Ende von `plan dedupe-dirs` über die echte CLI: scan → plan → apply → undo,
//! jeweils mit Temp-Ordnern und isoliertem `ORDNER_CLEANUP_HOME`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use assert_cmd::assert::Assert;
use assert_cmd::Command;
use ordner_cleanup::paths;
use predicates::str::contains;

struct Env {
    home: tempfile::TempDir,
    tree: tempfile::TempDir,
    out: tempfile::TempDir,
}

/// Pfad → (Größe, mtime, Inhalt); Ordner nur mit Existenz (ihre mtime ändert Windows selbst).
type Snapshot = BTreeMap<String, (u64, SystemTime, Vec<u8>)>;

const DAY: u64 = 86_400;

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
            .env_remove("OneDrive")
            .env_remove("OneDriveConsumer")
            .env_remove("OneDriveCommercial")
            .current_dir(self.out.path());
        cmd
    }

    fn config(&self, text: &str) {
        let dir = self.home.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), text).unwrap();
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.root().join(rel);
        std::fs::create_dir_all(paths::extended(path.parent().unwrap())).unwrap();
        std::fs::write(paths::extended(&path), content).unwrap();
        path
    }

    /// Setzt die Änderungszeit auf „vor `days` Tagen“.
    fn age_days(&self, rel: &str, days: u64) {
        let file = std::fs::File::options()
            .write(true)
            .open(paths::extended(&self.root().join(rel)))
            .unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(days * DAY))
            .unwrap();
    }

    fn scan(&self) {
        self.bin().arg("scan").arg(self.root()).assert().success();
    }

    /// `scan` und `plan dedupe-dirs <wurzel> --out <datei> [extra]`; liefert den Planpfad.
    fn plan(&self, name: &str, extra: &[&str]) -> PathBuf {
        let plan = self.out.path().join(name);
        self.scan();
        self.bin()
            .args(["plan", "dedupe-dirs"])
            .arg(self.root())
            .arg("--out")
            .arg(&plan)
            .args(extra)
            .assert()
            .success();
        plan
    }

    fn apply(&self, plan: &Path) -> Assert {
        self.bin().arg("apply").arg(plan).arg("--yes").assert()
    }

    fn undo(&self, run: &str) -> Assert {
        self.bin().args(["undo", run, "--yes"]).assert()
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

    fn exists(&self, rel: &str) -> bool {
        self.root().join(rel).exists()
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

fn action_paths(plan: &serde_json::Value, root: &Path) -> Vec<String> {
    // Plan-Pfade sind kanonisiert; der Temp-Pfad kann ein 8.3-Kurzname sein (CI: RUNNER~1).
    let root = PathBuf::from(paths::display(&std::fs::canonicalize(root).unwrap()));
    let mut out: Vec<String> = plan["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            Path::new(a["path"].as_str().unwrap())
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    out.sort();
    out
}

/// `Projekt` und `Kopie von Projekt` mit denselben zwei Dateien.
fn project_tree(env: &Env) {
    for dir in ["Projekt", "Kopie von Projekt"] {
        env.write(&format!("{dir}/a.txt"), "alpha");
        env.write(&format!("{dir}/sub/b.txt"), "bravo bravo");
    }
    env.write("Anderes/x.txt", "ganz etwas anderes");
}

#[test]
fn plan_veraendert_nichts_apply_und_undo_stellen_den_zustand_her() {
    let env = Env::new();
    project_tree(&env);
    let before = env.snapshot();

    let plan = env.plan("plan.json", &[]);
    assert_eq!(env.snapshot(), before, "plan darf nichts verändern");

    let json = plan_json(&plan);
    assert_eq!(json["version"], 2);
    assert_eq!(json["kind"], "dedupe-dirs");
    assert_eq!(action_paths(&json, env.root()), ["Kopie von Projekt"]);
    let action = &json["actions"][0];
    assert_eq!(action["type"], "quarantine");
    assert_eq!(action["is_dir"], true);
    assert_eq!(action["files"], 2);
    assert!(action["reason"]
        .as_str()
        .unwrap()
        .starts_with("exact-duplicate-dir"));
    assert!(action["keep"].as_str().unwrap().ends_with("Projekt"));
    assert_eq!(action["keep_fingerprint"].as_str().unwrap().len(), 32);

    let out = stdout(env.apply(&plan).success());
    assert!(!env.exists("Kopie von Projekt"));
    assert!(env.exists(r"Projekt\sub\b.txt") && env.exists(r"Anderes\x.txt"));
    let quarantined = env
        .root()
        .join(".ordner-cleanup")
        .join("quarantine")
        .join(run_id(&out));
    assert!(quarantined
        .join("Kopie von Projekt")
        .join("sub")
        .join("b.txt")
        .exists());

    env.undo(&run_id(&out)).success();
    assert_eq!(
        env.snapshot(),
        before,
        "undo stellt alles byteidentisch her"
    );
    assert!(
        !quarantined.exists(),
        "unter quarantine\\<run-id> bleibt nichts zurück"
    );
}

#[test]
fn plan_nennt_platzgewinn_und_apply_ist_idempotent() {
    let env = Env::new();
    project_tree(&env);
    env.scan();
    let out = stdout(
        env.bin()
            .args(["plan", "dedupe-dirs"])
            .arg(env.root())
            .arg("--out")
            .arg(env.out.path().join("p.json"))
            .assert()
            .success(),
    );
    assert!(out.contains("doppelte Ordner"), "{out}");
    assert!(out.contains("freiwerdend"), "{out}");

    let plan = env.out.path().join("p.json");
    env.apply(&plan).success();
    let second = env.plan("plan2.json", &[]);
    assert!(plan_json(&second)["actions"].as_array().unwrap().is_empty());
}

#[test]
fn junk_dateien_zaehlen_beim_vergleich_nicht() {
    let env = Env::new();
    project_tree(&env);
    env.write("Kopie von Projekt/Thumbs.db", "thumbs");
    env.write("Kopie von Projekt/desktop.ini", "ini");

    let plan = env.plan("plan.json", &[]);

    assert_eq!(
        action_paths(&plan_json(&plan), env.root()),
        ["Kopie von Projekt"]
    );
    env.apply(&plan).success();
    assert!(!env.exists("Kopie von Projekt"), "Junk wandert mit");
}

#[test]
fn eine_abweichende_datei_ergibt_keinen_treffer() {
    let env = Env::new();
    project_tree(&env);
    env.write("Kopie von Projekt/sub/b.txt", "bravo BRAVO");

    let plan = env.plan("plan.json", &[]);

    assert!(plan_json(&plan)["actions"].as_array().unwrap().is_empty());
}

#[test]
fn drei_gleiche_ordner_behalten_genau_einen() {
    let env = Env::new();
    project_tree(&env);
    env.write("Projekt (2)/a.txt", "alpha");
    env.write("Projekt (2)/sub/b.txt", "bravo bravo");

    let plan = env.plan("plan.json", &[]);

    let json = plan_json(&plan);
    assert_eq!(
        action_paths(&json, env.root()),
        ["Kopie von Projekt", "Projekt (2)"]
    );
}

#[test]
fn nur_die_oberste_ebene_kommt_in_den_plan() {
    let env = Env::new();
    for dir in ["A", "B"] {
        env.write(&format!("{dir}/sub/b.txt"), "tief");
        env.write(&format!("{dir}/x.txt"), "oben");
    }

    let plan = env.plan("plan.json", &[]);

    let paths = action_paths(&plan_json(&plan), env.root());
    assert_eq!(paths.len(), 1, "{paths:?}");
    assert!(!paths[0].contains("sub"));
}

#[test]
fn namens_heuristik_geht_vor_dem_alter() {
    let env = Env::new();
    for dir in ["Projekt", "Projekt (2)", "Backup Projekt"] {
        env.write(&format!("{dir}/a.txt"), "alpha");
    }
    env.age_days("Projekt (2)/a.txt", 900);
    env.age_days("Backup Projekt/a.txt", 400);
    env.age_days("Projekt/a.txt", 10);

    let plan = env.plan("plan.json", &[]);

    let json = plan_json(&plan);
    assert_eq!(
        action_paths(&json, env.root()),
        ["Backup Projekt", "Projekt (2)"]
    );
    assert!(json["actions"][0]["keep"]
        .as_str()
        .unwrap()
        .ends_with("Projekt"));
}

#[test]
fn geschuetzter_ordner_wird_weder_entfernt_noch_behalten() {
    let env = Env::new();
    project_tree(&env);
    env.config(&format!(
        "protected_paths = [{:?}]\n",
        env.root().join("Kopie von Projekt").to_string_lossy()
    ));

    let plan = env.plan("plan.json", &[]);

    let json = plan_json(&plan);
    assert!(json["actions"].as_array().unwrap().is_empty());
    assert!(json["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["reason"] == "protected"));
}

#[test]
fn ordner_der_zwischen_plan_und_apply_veraendert_wurde_ist_stale() {
    let env = Env::new();
    project_tree(&env);
    let plan = env.plan("plan.json", &[]);
    env.write("Kopie von Projekt/neu.txt", "inzwischen angelegt");

    env.apply(&plan).code(2);

    assert!(env.exists(r"Kopie von Projekt\neu.txt"), "stale bleibt");
}

#[test]
fn geaenderter_behaltener_ordner_verhindert_das_entfernen() {
    let env = Env::new();
    project_tree(&env);
    let plan = env.plan("plan.json", &[]);
    env.write("Projekt/neu.txt", "inzwischen angelegt");

    env.apply(&plan).code(2);

    assert!(env.exists(r"Kopie von Projekt\a.txt"), "nichts entfernt");
    assert!(env.exists(r"Projekt\neu.txt"));
}

#[test]
fn onedrive_obergrenze_zaehlt_die_dateien_der_ordner() {
    let env = Env::new();
    project_tree(&env);
    env.config("onedrive_max_move_files = 1\n");
    let plan = env.plan("plan.json", &[]);
    let before = env.snapshot();

    env.bin()
        .env("OneDrive", env.root())
        .arg("apply")
        .arg(&plan)
        .arg("--yes")
        .assert()
        .code(1)
        .stderr(contains("--allow-large"));
    assert_eq!(env.snapshot(), before);

    env.bin()
        .env("OneDrive", env.root())
        .arg("apply")
        .arg(&plan)
        .arg("--yes")
        .arg("--allow-large")
        .assert()
        .success();
    assert!(!env.exists("Kopie von Projekt"));
}

#[test]
fn min_size_laesst_kleine_ordner_weg() {
    let env = Env::new();
    project_tree(&env);

    let plan = env.plan("plan.json", &["--min-size", "1MB"]);

    assert!(plan_json(&plan)["actions"].as_array().unwrap().is_empty());
}

#[test]
fn keep_path_waehlt_den_ordner_unter_dem_praefix() {
    let env = Env::new();
    env.write("A/a.txt", "alpha");
    env.write("Archiv/Kopie von A/a.txt", "alpha");
    let prefix =
        PathBuf::from(paths::display(&std::fs::canonicalize(env.root()).unwrap())).join("Archiv");
    env.scan();
    let plan = env.out.path().join("plan.json");
    env.bin()
        .args(["plan", "dedupe-dirs"])
        .arg(env.root())
        .arg("--keep")
        .arg(format!("path:{}", prefix.display()))
        .arg("--out")
        .arg(&plan)
        .assert()
        .success();

    assert_eq!(action_paths(&plan_json(&plan), env.root()), ["A"]);
}

#[test]
fn profil_laeuft_mit_dedupe_dirs() {
    let env = Env::new();
    project_tree(&env);
    env.config(&format!(
        "[profiles.test]\nroot = {:?}\nplans = [\"dedupe-dirs\"]\n",
        env.root().to_string_lossy()
    ));
    env.scan();
    let plan = env.out.path().join("profil.json");

    env.bin()
        .args(["plan", "dedupe-dirs", "--profile", "test", "--out"])
        .arg(&plan)
        .assert()
        .success();

    assert_eq!(
        action_paths(&plan_json(&plan), env.root()),
        ["Kopie von Projekt"]
    );
}
