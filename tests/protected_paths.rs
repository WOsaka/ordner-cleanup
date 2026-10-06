//! Schutzregeln gegen manipulierte Pläne: `apply` prüft unabhängig vom Plan.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use ordner_cleanup::platform::windows::short_path;
use predicates::str::contains;
use serde_json::{json, Value};

const ZERO_HASH: &str = "00000000000000000000000000000000";

struct Env {
    home: tempfile::TempDir,
    tree: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            tree: tempfile::tempdir().unwrap(),
        }
    }

    fn bin(&self) -> Command {
        let mut cmd = Command::cargo_bin("ordner-cleanup").unwrap();
        cmd.env("ORDNER_CLEANUP_HOME", self.home.path());
        cmd
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.tree.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    fn config(&self, text: &str) {
        let dir = self.home.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), text).unwrap();
    }

    fn plan_file(&self, root: &Path, actions: Vec<Value>) -> PathBuf {
        let plan = json!({
            "version": 1,
            "created": "2026-10-03T12:00:00+02:00",
            "kind": "dedupe",
            "root": root.to_string_lossy(),
            "keep_strategy": "oldest",
            "actions": actions,
            "skipped": [],
        });
        let path = self.home.path().join("plan.json");
        std::fs::write(&path, plan.to_string()).unwrap();
        path
    }
}

fn action(id: u32, path: &Path, keep: &Path) -> Value {
    json!({
        "id": id,
        "type": "quarantine",
        "path": path.to_string_lossy(),
        "size": 1,
        "mtime_ticks": 0,
        "mtime": "",
        "hash": ZERO_HASH,
        "keep": keep.to_string_lossy(),
        "keep_hash": ZERO_HASH,
        "reason": "exact-duplicate",
    })
}

#[test]
fn config_geschuetzter_pfad_kommt_nicht_in_den_plan() {
    let env = Env::new();
    env.write("wichtig/x.txt", "gleich");
    env.write("a/x.txt", "gleich");
    env.write("b/x.txt", "gleich");
    let protected = env.tree.path().join("wichtig");
    env.config(&format!("protected_paths = ['{}']", protected.display()));
    env.bin()
        .arg("scan")
        .arg(env.tree.path())
        .assert()
        .success();
    let out = env.home.path().join("out.json");

    env.bin()
        .args(["plan", "dedupe"])
        .arg(env.tree.path())
        .arg("--out")
        .arg(&out)
        .assert()
        .success()
        .stdout(contains("1 Aktionen"))
        .stdout(contains("geschützter Pfad"));

    let plan: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let paths: Vec<_> = plan["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(paths.len(), 1);
    assert!(!paths[0].contains("wichtig"));
    assert_eq!(plan["skipped"][0]["reason"], "protected");
    assert!(plan["skipped"][0]["path"]
        .as_str()
        .unwrap()
        .contains("wichtig"));
}

#[test]
fn profil_geschuetzter_pfad_gilt_auch_beim_anwenden_eines_veraenderten_plans() {
    let env = Env::new();
    let victim = env.write("wichtig/x.txt", "gleich");
    env.write("a/x.txt", "gleich");
    env.write("b/x.txt", "gleich");
    let protected = env.tree.path().join("wichtig");
    env.config(&format!(
        "[profiles.t]
root = '{}'
protected_paths = ['{}']
",
        env.tree.path().display(),
        protected.display()
    ));
    env.bin()
        .arg("scan")
        .arg(env.tree.path())
        .assert()
        .success();
    let out = env.home.path().join("out.json");
    env.bin()
        .args(["plan", "dedupe", "--profile", "t", "--out"])
        .arg(&out)
        .assert()
        .success();

    // Der Plan merkt sich den Schutz, auch wenn die globale Config ihn nicht kennt.
    let mut plan: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(
        plan["protected_paths"],
        json!([protected.to_string_lossy()]),
        "{plan}"
    );

    // Von Hand um eine Aktion im geschützten Ordner ergänzt: `apply` verweigert sie. Die Pfade
    // kommen aus der Wurzel des Plans, denn das Programm löst sie auf (Kurznamen auf dem CI-Runner).
    let plan_root = PathBuf::from(plan["root"].as_str().unwrap());
    let in_plan = |rel: &str| plan_root.join(rel.replace('/', "\\"));
    plan["actions"] = json!([action(1, &in_plan("wichtig/x.txt"), &in_plan("a/x.txt"))]);
    let edited = env.home.path().join("edited.json");
    std::fs::write(&edited, plan.to_string()).unwrap();
    env.bin()
        .arg("apply")
        .arg(&edited)
        .arg("--yes")
        .assert()
        .code(2)
        .stdout(contains("1 übersprungen"))
        .stdout(contains("geschützter Pfad"));
    assert!(victim.exists());
}

#[test]
fn manipulierter_plan_mit_geschuetztem_ordnernamen_wird_verweigert() {
    let env = Env::new();
    let keep = env.write("a.txt", "x");
    let victim = env.write("proj/.git/objekt.txt", "x");
    let plan = env.plan_file(env.tree.path(), vec![action(1, &victim, &keep)]);

    env.bin()
        .arg("apply")
        .arg(&plan)
        .arg("--yes")
        .assert()
        .code(2)
        .stdout(contains("1 übersprungen"))
        .stdout(contains("geschützter Pfad"));

    assert!(victim.exists());
    assert!(
        !env.tree
            .path()
            .join(".ordner-cleanup")
            .join("quarantine")
            .exists()
            || std::fs::read_dir(env.tree.path().join(".ordner-cleanup").join("quarantine"))
                .map(|d| d.count() == 0)
                .unwrap_or(true)
    );
}

#[test]
fn manipulierter_plan_mit_system_wurzel_legt_nichts_an() {
    let env = Env::new();
    let plan = env.plan_file(
        Path::new(r"C:\Windows"),
        vec![action(
            1,
            Path::new(r"C:\Windows\ordner-cleanup-test-gibt-es-nicht.txt"),
            Path::new(r"C:\Windows\notepad.exe"),
        )],
    );

    env.bin()
        .arg("apply")
        .arg(&plan)
        .arg("--yes")
        .assert()
        .code(1)
        .stderr(contains("geschützt"));

    assert!(!Path::new(r"C:\Windows\.ordner-cleanup").exists());
}

#[test]
fn plan_mit_system_datei_ausserhalb_der_wurzel_wird_abgelehnt() {
    let env = Env::new();
    let keep = env.write("a.txt", "x");
    let plan = env.plan_file(
        env.tree.path(),
        vec![action(1, Path::new(r"C:\Windows\notepad.exe"), &keep)],
    );

    env.bin()
        .arg("apply")
        .arg(&plan)
        .arg("--yes")
        .assert()
        .code(1)
        .stderr(contains("liegt nicht in der Wurzel"));
}

#[test]
fn plan_mit_punkt_pfad_aus_der_wurzel_heraus_wird_abgelehnt() {
    let env = Env::new();
    let keep = env.write("a.txt", "x");
    let outside = env.tree.path().join("..").join("fremd.txt");
    let plan = env.plan_file(env.tree.path(), vec![action(1, &outside, &keep)]);

    env.bin()
        .arg("apply")
        .arg(&plan)
        .arg("--yes")
        .assert()
        .code(1)
        .stderr(contains(".."));
}

#[test]
fn aktion_im_quarantaene_ordner_selbst_wird_verweigert() {
    let env = Env::new();
    let keep = env.write("a.txt", "x");
    let inside = env.write(".ordner-cleanup/quarantine/alt/b.txt", "x");
    let plan = env.plan_file(env.tree.path(), vec![action(1, &inside, &keep)]);

    env.bin()
        .arg("apply")
        .arg(&plan)
        .arg("--yes")
        .assert()
        .code(2)
        .stdout(contains("geschützter Pfad"));

    assert!(inside.exists());
}

#[test]
fn kurzname_umgeht_den_schutz_nicht() {
    let env = Env::new();
    let long = tempfile::Builder::new()
        .prefix("langer-ordnername-")
        .tempdir()
        .unwrap();
    let keep = long.path().join("a.txt");
    let victim = long.path().join("langer-wichtiger-ordner").join("b.txt");
    std::fs::create_dir_all(victim.parent().unwrap()).unwrap();
    std::fs::write(&keep, "x").unwrap();
    std::fs::write(&victim, "x").unwrap();
    let Some(short_root) = short_path(long.path()) else {
        eprintln!("kein 8.3-Kurzname verfügbar, Test übersprungen");
        return;
    };
    let Some(short_dir) = short_path(victim.parent().unwrap()) else {
        eprintln!("kein 8.3-Kurzname für den Unterordner, Test übersprungen");
        return;
    };
    env.config(&format!(
        "protected_paths = ['{}']",
        victim.parent().unwrap().display()
    ));
    // Wurzel und Aktion nur in Kurzform: Der Plan verschleiert den geschützten Ordner.
    let plan = env.plan_file(
        &short_root,
        vec![action(
            1,
            &short_dir.join("b.txt"),
            &short_root.join("a.txt"),
        )],
    );

    env.bin()
        .arg("apply")
        .arg(&plan)
        .arg("--yes")
        .assert()
        .code(2)
        .stdout(contains("geschützter Pfad"));

    assert!(victim.exists());
}
