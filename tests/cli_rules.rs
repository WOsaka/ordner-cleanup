//! Ende-zu-Ende von `plan rules` über die echte CLI: scan → plan rules → apply → scan →
//! plan rules (leer) → undo, mit Temp-Ordnern und isoliertem `ORDNER_CLEANUP_HOME`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use assert_cmd::assert::Assert;
use assert_cmd::Command;
use chrono::NaiveDate;
use ordner_cleanup::paths;
use ordner_cleanup::scan::exif::testing::{jpeg_with_date, jpeg_without_exif, DATE_TIME_ORIGINAL};
use predicates::str::contains;

struct Env {
    home: tempfile::TempDir,
    tree: tempfile::TempDir,
    out: tempfile::TempDir,
}

/// Pfad → (Größe, mtime, Inhalt); Ordner nur mit Existenz (ihre mtime ändert Windows selbst).
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

    /// Schreibt die Default-Regeldatei (`<HOME>\config\rules.toml`).
    fn rules(&self, text: &str) -> PathBuf {
        let dir = self.home.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rules.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn write_bytes(&self, rel: &str, content: &[u8]) -> PathBuf {
        let path = self.root().join(rel);
        std::fs::create_dir_all(paths::extended(path.parent().unwrap())).unwrap();
        std::fs::write(paths::extended(&path), content).unwrap();
        path
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        self.write_bytes(rel, content.as_bytes())
    }

    fn scan(&self) {
        self.bin().arg("scan").arg(self.root()).assert().success();
    }

    /// `scan` und `plan rules <wurzel> --out <datei> [extra]`; liefert den Planpfad.
    fn plan(&self, name: &str, extra: &[&str]) -> PathBuf {
        let plan = self.out.path().join(name);
        self.scan();
        self.bin()
            .args(["plan", "rules"])
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

    /// Alle Pfade (Dateien und Ordner) relativ zur Wurzel, sortiert, ohne Werkzeugordner.
    fn tree(&self) -> Vec<String> {
        self.snapshot().into_keys().collect()
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

fn photo(year: i32) -> Vec<u8> {
    jpeg_with_date(
        DATE_TIME_ORIGINAL,
        NaiveDate::from_ymd_opt(year, 8, 15)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap(),
    )
}

const RULES: &str = r#"
[[rules]]
name = "fotos"
ext = ["jpg"]
target = "Fotos/{exif.date:%Y}/{name}.{ext}"
exif_fallback = "skip"
lowercase_ext = true

[[rules]]
name = "pdf"
glob = "Downloads/**"
ext = ["pdf"]
target = "Dokumente/"
strip_copy_suffix = true
"#;

fn downloads_tree(env: &Env) {
    env.write("Downloads/Rechnung (2).pdf", "rechnung");
    env.write("Downloads/Bericht.pdf", "bericht");
    env.write_bytes("Downloads/IMG_1.JPG", &photo(2019));
    env.write_bytes("Downloads/IMG_2.JPG", &jpeg_without_exif());
    env.write("Notizen.txt", "keine regel");
}

#[test]
fn plan_aendert_nichts_apply_sortiert_ein_und_undo_stellt_alles_her() {
    let env = Env::new();
    env.rules(RULES);
    downloads_tree(&env);
    let before = env.snapshot();

    let plan = env.plan("plan.json", &[]);
    assert_eq!(env.snapshot(), before, "plan darf nichts verändern");

    let json = plan_json(&plan);
    assert_eq!(json["version"], 3);
    assert!(json["integrity"].as_str().unwrap().starts_with("sha256:"));
    assert_eq!(json["kind"], "rules");
    assert!(json["params"]["rules_file"]
        .as_str()
        .unwrap()
        .ends_with("rules.toml"));
    let mut rules_used: Vec<&str> = json["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["rule"].as_str().unwrap())
        .collect();
    rules_used.sort();
    assert_eq!(rules_used, ["fotos", "pdf", "pdf"]);
    for action in json["actions"].as_array().unwrap() {
        assert_eq!(action["type"], "move");
        assert!(action["reason"].as_str().unwrap().starts_with("rule:"));
    }
    assert!(json["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["reason"] == "no-exif-date"));

    let out = stdout(env.apply(&plan).success());
    assert!(out.contains("einsortiert/umbenannt"), "{out}");
    assert_eq!(
        env.tree(),
        [
            r"Dokumente",
            r"Dokumente\Bericht.pdf",
            r"Dokumente\Rechnung.pdf",
            r"Downloads",
            r"Downloads\IMG_2.JPG",
            r"Fotos",
            r"Fotos\2019",
            r"Fotos\2019\IMG_1.jpg",
            r"Notizen.txt",
        ]
    );

    // Idempotenz: scan → plan rules ergibt keine Aktion mehr.
    let again = env.plan("plan2.json", &[]);
    assert!(plan_json(&again)["actions"].as_array().unwrap().is_empty());

    let runs = stdout(env.bin().arg("runs").arg(env.root()).assert().success());
    assert!(runs.contains("3 einsortiert"), "{runs}");

    env.undo(&run_id(&out)).success();
    assert_eq!(
        env.snapshot(),
        before,
        "undo stellt Pfade, Größen, Zeiten und Inhalt her; neue Ordner sind weg"
    );
}

#[test]
fn fehler_in_der_regeldatei_brechen_mit_zeile_und_regelname_ab_ohne_plan() {
    let env = Env::new();
    downloads_tree(&env);
    env.rules(
        "[[rules]]\nname = \"gut\"\next = [\"pdf\"]\ntarget = \"A/\"\n\n\
         [[rules]]\nname = \"kaputt\"\next = [\"jpg\"]\ntarget = \"{gibtsnicht}/\"\n",
    );
    env.scan();
    let plan = env.out.path().join("plan.json");

    env.bin()
        .args(["plan", "rules"])
        .arg(env.root())
        .arg("--out")
        .arg(&plan)
        .assert()
        .code(1)
        .stderr(contains("rules.toml:6"))
        .stderr(contains("kaputt"))
        .stderr(contains("gibtsnicht"));
    assert!(!plan.exists());
}

#[test]
fn syntaxfehler_und_unbekannter_schluessel_nennen_die_zeile() {
    let env = Env::new();
    downloads_tree(&env);
    env.scan();
    let plan = env.out.path().join("plan.json");
    env.rules("[[rules]]\nname = \"a\"\next = [\"pdf\"]\ntarget = \"A/\"\nfoo = 1\n");
    env.bin()
        .args(["plan", "rules"])
        .arg(env.root())
        .arg("--out")
        .arg(&plan)
        .assert()
        .code(1)
        .stderr(contains("rules.toml:5"))
        .stderr(contains("foo"));
    env.rules("[[rules]\nname = ");
    env.bin()
        .args(["plan", "rules"])
        .arg(env.root())
        .arg("--out")
        .arg(&plan)
        .assert()
        .code(1)
        .stderr(contains("rules.toml:1"));
    assert!(!plan.exists());
}

#[test]
fn fehlende_regeldatei_nennt_pfad_und_beispiel() {
    let env = Env::new();
    downloads_tree(&env);
    env.scan();
    env.bin()
        .args(["plan", "rules"])
        .arg(env.root())
        .assert()
        .code(1)
        .stderr(contains("rules.toml"))
        .stderr(contains("[[rules]]"));
}

#[test]
fn rule_filtert_und_unbekannte_regel_nennt_die_vorhandenen() {
    let env = Env::new();
    env.rules(RULES);
    downloads_tree(&env);

    let plan = env.plan("plan.json", &["--rule", "pdf"]);
    let json = plan_json(&plan);
    assert_eq!(json["params"]["rules"], "pdf");
    assert!(json["actions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["rule"] == "pdf"));
    assert_eq!(json["actions"].as_array().unwrap().len(), 2);

    let other = env.out.path().join("plan3.json");
    env.bin()
        .args(["plan", "rules"])
        .arg(env.root())
        .args(["--rule", "gibtsnicht", "--out"])
        .arg(&other)
        .assert()
        .code(1)
        .stderr(contains("gibtsnicht"))
        .stderr(contains("fotos, pdf"));
    assert!(!other.exists());
}

#[test]
fn rules_option_hat_vorrang_vor_der_default_datei() {
    let env = Env::new();
    env.rules("[[rules]]\nname = \"kaputt\"\n");
    downloads_tree(&env);
    let custom = env.out.path().join("meine.toml");
    std::fs::write(&custom, RULES).unwrap();

    let plan = env.plan("plan.json", &["--rules", custom.to_str().unwrap()]);

    assert_eq!(plan_json(&plan)["actions"].as_array().unwrap().len(), 3);
}

#[test]
fn umbenennen_nur_in_der_schreibweise_ende_zu_ende() {
    let env = Env::new();
    env.rules(
        "[[rules]]\nname = \"klein\"\next = [\"jpg\"]\ntarget = \"./{name}.{ext}\"\nlowercase_ext = true\n",
    );
    env.write("Fotos/foto.JPG", "bild");
    let before = env.snapshot();

    let plan = env.plan("plan.json", &[]);
    let json = plan_json(&plan);
    assert_eq!(json["actions"].as_array().unwrap().len(), 1);

    let out = stdout(env.apply(&plan).success());
    assert_eq!(env.tree(), ["Fotos", r"Fotos\foto.jpg"]);
    assert_eq!(
        std::fs::read_to_string(env.root().join(r"Fotos\foto.jpg")).unwrap(),
        "bild"
    );

    let again = env.plan("plan2.json", &[]);
    assert!(plan_json(&again)["actions"].as_array().unwrap().is_empty());

    env.undo(&run_id(&out)).success();
    assert_eq!(env.snapshot(), before, "alte Schreibweise ist zurück");
}

#[test]
fn onedrive_obergrenze_gilt_auch_fuer_regel_plaene() {
    let env = Env::new();
    env.rules(RULES);
    env.config("onedrive_max_move_files = 1\n");
    downloads_tree(&env);
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
    assert!(env.exists(r"Dokumente\Bericht.pdf"));
}

#[test]
fn manipulierter_plan_mit_ziel_ausserhalb_der_wurzel_wird_abgelehnt() {
    let env = Env::new();
    env.rules(RULES);
    downloads_tree(&env);
    let plan = env.plan("plan.json", &[]);
    let outside = env.out.path().join("ausserhalb");
    let mut json = plan_json(&plan);
    json["actions"][0]["target"] = outside.join("x.pdf").to_string_lossy().into_owned().into();
    let bad = env.out.path().join("bad.json");
    std::fs::write(&bad, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    let before = env.snapshot();

    env.apply(&bad).failure();

    assert_eq!(env.snapshot(), before);
    assert!(!outside.exists());
}

#[test]
fn ziel_das_zwischen_plan_und_apply_entsteht_wird_nicht_ueberschrieben() {
    let env = Env::new();
    env.rules(RULES);
    downloads_tree(&env);
    let plan = env.plan("plan.json", &[]);
    env.write("Dokumente/Bericht.pdf", "schon da");

    env.apply(&plan).code(2);

    assert_eq!(
        std::fs::read_to_string(env.root().join(r"Dokumente\Bericht.pdf")).unwrap(),
        "schon da"
    );
    assert!(env.exists(r"Downloads\Bericht.pdf"), "Quelle bleibt");
}
