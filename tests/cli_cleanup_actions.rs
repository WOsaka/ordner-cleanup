//! Ende-zu-Ende der Aufräumaktionen über die echte CLI: scan → plan → apply → undo,
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
    fn age_days(&self, path: &Path, days: u64) {
        let file = std::fs::File::options()
            .write(true)
            .open(paths::extended(path))
            .unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(days * DAY))
            .unwrap();
    }

    fn scan(&self) {
        self.bin().arg("scan").arg(self.root()).assert().success();
    }

    /// `scan` und `plan <aktion> <wurzel> --out <datei> [extra]`; liefert den Planpfad.
    fn plan(&self, action: &str, name: &str, extra: &[&str]) -> PathBuf {
        let plan = self.out.path().join(name);
        self.scan();
        self.bin()
            .args(["plan", action])
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
    let mut out: Vec<String> = plan["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            let p = a["path"].as_str().unwrap();
            Path::new(p)
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------------------------
// junk
// ---------------------------------------------------------------------------------------------

fn junk_tree(env: &Env) {
    env.write("a/Thumbs.db", "thumbs");
    env.write("a/dokument.txt", "wichtig");
    env.write("b/cache.tmp", "temporär");
    env.write("b/~$bericht.docx", "lock");
    env.write("c/Übergröße Ordner/film.crdownload", "halb");
    env.write("c/foto.jpg", "bild");
}

#[test]
fn junk_plan_veraendert_nichts_apply_und_undo_stellen_den_zustand_her() {
    let env = Env::new();
    junk_tree(&env);
    let before = env.snapshot();

    let plan = env.plan("junk", "plan.json", &[]);
    assert_eq!(env.snapshot(), before, "plan darf nichts verändern");

    let json = plan_json(&plan);
    assert_eq!(json["version"], 2);
    assert_eq!(json["kind"], "junk");
    assert_eq!(
        action_paths(&json, env.root()),
        [
            r"a\Thumbs.db",
            r"b\cache.tmp",
            r"b\~$bericht.docx",
            r"c\Übergröße Ordner\film.crdownload"
        ]
    );
    for action in json["actions"].as_array().unwrap() {
        assert_eq!(action["type"], "quarantine");
        assert!(action.get("hash").is_none(), "Junk trägt keinen Hash");
        assert!(action["reason"].as_str().unwrap().starts_with("junk:"));
    }

    let out = stdout(env.apply(&plan).success());
    assert!(!env.exists(r"a\Thumbs.db") && !env.exists(r"b\cache.tmp"));
    assert!(env.exists(r"a\dokument.txt") && env.exists(r"c\foto.jpg"));
    let quarantined = env
        .root()
        .join(".ordner-cleanup")
        .join("quarantine")
        .join(run_id(&out));
    assert!(quarantined.join("a").join("Thumbs.db").exists());

    env.undo(&run_id(&out)).success();
    assert_eq!(
        env.snapshot(),
        before,
        "undo stellt alles byteidentisch her"
    );
}

#[test]
fn junk_category_waehlt_nur_die_genannten_kategorien() {
    let env = Env::new();
    junk_tree(&env);
    let plan = env.plan("junk", "plan.json", &["--category", "temp"]);
    assert_eq!(
        action_paths(&plan_json(&plan), env.root()),
        [r"b\cache.tmp", r"b\~$bericht.docx"]
    );
}

#[test]
fn junk_unbekannte_kategorie_wird_klar_abgelehnt() {
    let env = Env::new();
    junk_tree(&env);
    env.scan();
    env.bin()
        .args(["plan", "junk"])
        .arg(env.root())
        .args(["--category", "temp,gibtsnicht"])
        .assert()
        .failure()
        .stderr(contains("gibtsnicht"));
}

#[test]
fn junk_eigene_regel_aus_der_config_wird_erkannt() {
    let env = Env::new();
    junk_tree(&env);
    env.write("logs/dienst.log", "log");
    env.write("logs/alt.LOG", "log");
    env.config(
        r#"
junk_categories = ["logs"]

[[junk_rules]]
name = "logs"
patterns = ["*.log"]
"#,
    );
    let plan = env.plan("junk", "plan.json", &[]);
    assert_eq!(
        action_paths(&plan_json(&plan), env.root()),
        [r"logs\alt.LOG", r"logs\dienst.log"]
    );
    assert!(plan_json(&plan)["actions"][0]["reason"] == "junk:logs");
}

#[test]
fn junk_installer_nur_im_downloads_ordner_und_ab_mindestalter() {
    let env = Env::new();
    let alt = env.write("Eingang/alt-setup.exe", "alt");
    env.age_days(&alt, 200);
    env.write("Eingang/neu-setup.msi", "neu");
    let tief = env.write("Eingang/sub/tief.exe", "tief");
    env.age_days(&tief, 500);
    let anders = env.write("Projekt/tool.exe", "tool");
    env.age_days(&anders, 500);
    env.config(&format!(
        "downloads_dirs = [{:?}]\n",
        env.root().join("Eingang").to_string_lossy()
    ));

    let plan = env.plan("junk", "plan.json", &["--category", "installer"]);

    let json = plan_json(&plan);
    assert_eq!(action_paths(&json, env.root()), [r"Eingang\alt-setup.exe"]);
    let skipped = json["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0]["reason"], "too-recent");
}

#[test]
fn junk_ohne_downloads_ordner_unter_der_wurzel_nennt_den_grund() {
    let env = Env::new();
    let exe = env.write("Projekt/tool.exe", "tool");
    env.age_days(&exe, 500);
    env.config("downloads_dirs = [\"Y:\\\\Gibt\\\\Es\\\\Nicht\"]\n");
    env.scan();

    let text = stdout(
        env.bin()
            .args(["plan", "junk"])
            .arg(env.root())
            .arg("--out")
            .arg(env.out.path().join("plan.json"))
            .arg("--category")
            .arg("installer")
            .assert()
            .success(),
    );
    assert!(text.contains("0 Aktionen"), "{text}");
    assert!(text.contains("installer"), "{text}");
}

#[test]
fn junk_geaenderte_datei_zwischen_plan_und_apply_ist_stale() {
    let env = Env::new();
    junk_tree(&env);
    let plan = env.plan("junk", "plan.json", &["--category", "temp"]);
    env.write("b/cache.tmp", "plötzlich viel länger als vorher");

    env.apply(&plan).code(2);

    assert!(env.exists(r"b\cache.tmp"), "stale Datei bleibt liegen");
    assert!(!env.exists(r"b\~$bericht.docx"), "der Rest läuft");
}

#[test]
fn junk_ist_idempotent_nach_apply_ist_der_naechste_plan_leer() {
    let env = Env::new();
    junk_tree(&env);
    let plan = env.plan("junk", "plan1.json", &[]);
    env.apply(&plan).success();

    let second = env.plan("junk", "plan2.json", &[]);

    assert!(plan_json(&second)["actions"].as_array().unwrap().is_empty());
}

#[test]
fn junk_geschuetzter_pfad_kommt_nicht_in_den_plan() {
    let env = Env::new();
    env.write("wichtig/cache.tmp", "x");
    env.write("frei/cache.tmp", "x");
    env.config(&format!(
        "protected_paths = [{:?}]\n",
        env.root().join("wichtig").to_string_lossy()
    ));
    let plan = env.plan("junk", "plan.json", &[]);
    let json = plan_json(&plan);
    assert_eq!(action_paths(&json, env.root()), [r"frei\cache.tmp"]);
    assert!(json["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["reason"] == "protected"));
}
