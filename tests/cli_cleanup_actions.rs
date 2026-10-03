//! Ende-zu-Ende der Aufräumaktionen über die echte CLI: scan → plan → apply → undo,
//! jeweils mit Temp-Ordnern und isoliertem `ORDNER_CLEANUP_HOME`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use assert_cmd::assert::Assert;
use assert_cmd::Command;
use ordner_cleanup::change::fsops::{FsOps, RealFs};
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

    fn mkdir(&self, rel: &str) -> PathBuf {
        let path = self.root().join(rel);
        std::fs::create_dir_all(paths::extended(&path)).unwrap();
        path
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
    // Plan-Pfade sind kanonisiert; der Temp-Pfad kann ein 8.3-Kurzname sein (CI: RUNNER~1).
    let root = PathBuf::from(paths::display(&std::fs::canonicalize(root).unwrap()));
    let mut out: Vec<String> = plan["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            let p = a["path"].as_str().unwrap();
            Path::new(p)
                .strip_prefix(&root)
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
    assert!(
        !quarantined.exists(),
        "keine leeren Ordner unter quarantine\\<run-id>"
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

// ---------------------------------------------------------------------------------------------
// empty-dirs
// ---------------------------------------------------------------------------------------------

const KNOWN_MTIME: i64 = 1_600_000_000 * 10_000_000;
const KNOWN_CTIME: i64 = 1_500_000_000 * 10_000_000;

/// Setzt Attribute und Zeiten einer Ordnerliste (Kinder zuerst, sonst ändert das Anlegen der
/// Kinder die Zeit der Eltern wieder).
fn set_dir_times(env: &Env, rels_bottom_up: &[(&str, u32)]) {
    for (rel, attrs) in rels_bottom_up {
        RealFs
            .set_dir_meta(&env.root().join(rel), *attrs, KNOWN_MTIME, KNOWN_CTIME)
            .unwrap();
    }
}

fn dir_meta(env: &Env, rel: &str) -> (i64, i64, bool) {
    let meta = RealFs.metadata(&env.root().join(rel)).unwrap();
    (meta.mtime_ticks, meta.ctime_ticks, meta.attrs.is_hidden())
}

fn empty_tree(env: &Env) {
    env.mkdir("a/b/c");
    env.mkdir("d");
    env.write("keep/datei.txt", "bleibt");
}

#[test]
fn empty_dirs_plan_apply_undo_stellen_ordner_attribute_und_zeiten_her() {
    let env = Env::new();
    empty_tree(&env);
    set_dir_times(
        &env,
        &[
            ("a/b/c", 0x10),
            ("a/b", 0x10 | 0x2),
            ("a", 0x10),
            ("d", 0x10),
        ],
    );
    let before = env.snapshot();
    let metas: Vec<_> = ["a", "a/b", "a/b/c", "d"]
        .iter()
        .map(|rel| dir_meta(&env, rel))
        .collect();

    let plan = env.plan("empty-dirs", "plan.json", &[]);
    assert_eq!(env.snapshot(), before, "plan darf nichts verändern");

    let json = plan_json(&plan);
    assert_eq!(json["kind"], "empty-dirs");
    let actions = json["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 4);
    assert!(actions.iter().all(|a| a["type"] == "remove-dir"));
    let root = PathBuf::from(paths::display(&std::fs::canonicalize(env.root()).unwrap()));
    let order: Vec<String> = actions
        .iter()
        .map(|a| {
            Path::new(a["path"].as_str().unwrap())
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(order, [r"a\b\c", r"a\b", "a", "d"], "von unten nach oben");

    let out = stdout(env.apply(&plan).success());
    assert!(!env.exists("a") && !env.exists("d"));
    assert!(env.exists("keep") && env.root().exists(), "Wurzel bleibt");

    env.undo(&run_id(&out)).success();

    assert_eq!(env.snapshot(), before);
    let after: Vec<_> = ["a", "a/b", "a/b/c", "d"]
        .iter()
        .map(|rel| dir_meta(&env, rel))
        .collect();
    assert_eq!(
        after, metas,
        "Zeiten und versteckt-Attribut wie vor dem Lauf"
    );
}

#[test]
fn empty_dirs_ordner_mit_nur_thumbs_db_ist_nicht_leer_und_der_hinweis_nennt_junk() {
    let env = Env::new();
    env.write("muell/Thumbs.db", "t");
    env.mkdir("leer");
    env.scan();

    let text = stdout(
        env.bin()
            .args(["plan", "empty-dirs"])
            .arg(env.root())
            .arg("--out")
            .arg(env.out.path().join("plan.json"))
            .assert()
            .success(),
    );

    let json = plan_json(&env.out.path().join("plan.json"));
    assert_eq!(action_paths(&json, env.root()), ["leer"]);
    assert!(text.contains("plan junk"), "{text}");
}

#[test]
fn kompletter_putz_junk_dann_empty_dirs_entfernt_auch_ordner_die_nur_muell_enthielten() {
    let env = Env::new();
    env.write("muell/Thumbs.db", "t");
    env.write("muell/sub/cache.tmp", "c");
    env.write("keep/datei.txt", "bleibt");
    let before = env.snapshot();

    let junk = env.plan("junk", "junk.json", &[]);
    let junk_run = run_id(&stdout(env.apply(&junk).success()));
    let dirs = env.plan("empty-dirs", "dirs.json", &[]);
    let dirs_run = run_id(&stdout(env.apply(&dirs).success()));

    assert!(!env.exists("muell"), "Ordner nur mit Müll ist jetzt weg");
    assert!(env.exists(r"keep\datei.txt"));

    env.undo(&dirs_run).success();
    env.undo(&junk_run).success();
    assert_eq!(env.snapshot(), before);
}

#[test]
fn empty_dirs_leere_wurzel_wird_nie_entfernt() {
    let env = Env::new();
    let plan = env.plan("empty-dirs", "plan.json", &[]);
    assert!(plan_json(&plan)["actions"].as_array().unwrap().is_empty());
    env.apply(&plan).success();
    assert!(env.root().exists());
}

#[test]
fn empty_dirs_ordner_der_zwischen_plan_und_apply_befuellt_wurde_ist_stale() {
    let env = Env::new();
    empty_tree(&env);
    let plan = env.plan("empty-dirs", "plan.json", &[]);
    env.write("d/neu.txt", "inzwischen angelegt");

    env.apply(&plan).code(2);

    assert!(env.exists(r"d\neu.txt"), "der befüllte Ordner bleibt");
    assert!(!env.exists("a"), "der Rest läuft");
}

#[test]
fn empty_dirs_ist_idempotent() {
    let env = Env::new();
    empty_tree(&env);
    let plan = env.plan("empty-dirs", "plan1.json", &[]);
    env.apply(&plan).success();

    let second = env.plan("empty-dirs", "plan2.json", &[]);

    assert!(plan_json(&second)["actions"].as_array().unwrap().is_empty());
}

#[test]
fn undo_meldet_kollision_wenn_an_der_stelle_des_ordners_inzwischen_eine_datei_liegt() {
    let env = Env::new();
    empty_tree(&env);
    let plan = env.plan("empty-dirs", "plan.json", &[]);
    let run = run_id(&stdout(env.apply(&plan).success()));
    env.write("d", "ich bin jetzt eine datei");

    env.undo(&run).code(2);

    assert_eq!(
        std::fs::read_to_string(env.root().join("d")).unwrap(),
        "ich bin jetzt eine datei"
    );
    assert!(
        env.root().join("a/b/c").is_dir(),
        "der Rest wird wiederhergestellt"
    );
}

#[test]
fn empty_dirs_geschuetzter_ordner_kommt_nicht_in_den_plan() {
    let env = Env::new();
    env.mkdir("wichtig");
    env.mkdir("frei");
    env.config(&format!(
        "protected_paths = [{:?}]\n",
        env.root().join("wichtig").to_string_lossy()
    ));
    let plan = env.plan("empty-dirs", "plan.json", &[]);
    let json = plan_json(&plan);
    assert_eq!(action_paths(&json, env.root()), ["frei"]);
    assert!(json["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["reason"] == "protected"));
}

// ---------------------------------------------------------------------------------------------
// archive
// ---------------------------------------------------------------------------------------------

impl Env {
    /// Setzt die Änderungszeit aller Dateien unterhalb von `rel` auf „vor `days` Tagen“.
    fn age_tree(&self, rel: &str, days: u64) {
        fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(paths::extended(dir)).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, files);
                } else {
                    files.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&self.root().join(rel), &mut files);
        for file in files {
            self.age_days(&file, days);
        }
    }
}

const THREE_YEARS: u64 = 3 * 365;

fn archive_tree(env: &Env) {
    env.write("Projekt2019/a.txt", "alpha");
    env.write("Projekt2019/sub/b.txt", "bravo bravo");
    env.write("Aktiv/neu.txt", "frisch");
    env.write("Aktiv/alt/x.txt", "alt");
    env.write("lose-alt.txt", "einzeldatei");
    env.age_tree("Projekt2019", THREE_YEARS);
    env.age_tree("Aktiv/alt", THREE_YEARS);
    env.age_days(&env.root().join("lose-alt.txt"), 5 * 365);
}

/// Jahr der jüngsten mtime eines vor `days` Tagen geänderten Ordners (Ziel unter `_Archiv`).
fn year_ago(days: u64) -> i32 {
    use chrono::Datelike;
    (chrono::Utc::now() - chrono::Duration::days(days as i64)).year()
}

#[test]
fn archive_plan_apply_undo_verschiebt_nur_oberste_alte_ordner_und_stellt_alles_her() {
    let env = Env::new();
    archive_tree(&env);
    let before = env.snapshot();

    let plan = env.plan("archive", "plan.json", &[]);
    assert_eq!(env.snapshot(), before, "plan darf nichts verändern");

    let json = plan_json(&plan);
    assert_eq!(json["kind"], "archive");
    assert_eq!(json["params"]["older_than"], "2y");
    assert_eq!(
        action_paths(&json, env.root()),
        [r"Aktiv\alt", "Projekt2019"],
        "Einzeldatei und aktiver Elternordner nicht"
    );
    let year = year_ago(THREE_YEARS);
    for action in json["actions"].as_array().unwrap() {
        assert_eq!(action["type"], "move");
        assert_eq!(action["is_dir"], true);
        assert_eq!(action["reason"], "archive:older-than-2y");
        let target = action["target"].as_str().unwrap();
        assert!(target.contains(&format!(r"\_Archiv\{year}\")), "{target}");
    }
    let projekt = json["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["path"].as_str().unwrap().ends_with("Projekt2019"))
        .unwrap();
    assert_eq!(projekt["files"], 2);
    assert_eq!(projekt["size"], 16);

    let out = stdout(env.apply(&plan).success());
    assert!(!env.exists("Projekt2019") && !env.exists(r"Aktiv\alt"));
    assert!(env.exists(r"Aktiv\neu.txt") && env.exists("lose-alt.txt"));
    let archived = env.root().join("_Archiv").join(year.to_string());
    assert_eq!(
        std::fs::read_to_string(archived.join(r"Projekt2019\sub\b.txt")).unwrap(),
        "bravo bravo"
    );

    env.undo(&run_id(&out)).success();

    assert_eq!(env.snapshot(), before, "byteidentisch inkl. mtime");
    assert!(!env.exists("_Archiv"), "leeres Archiv wird aufgeräumt");
}

#[test]
fn archive_older_than_und_config_bestimmen_die_schwelle() {
    let env = Env::new();
    archive_tree(&env);
    // 3 Jahre alt: bei 5 Jahren nichts, bei 2 Jahren (Default) beides.
    let strict = env.plan("archive", "strict.json", &["--older-than", "5y"]);
    assert!(plan_json(&strict)["actions"].as_array().unwrap().is_empty());

    env.config("archive_older_than = \"4y\"\n");
    let by_config = env.plan("archive", "config.json", &[]);
    assert!(plan_json(&by_config)["actions"]
        .as_array()
        .unwrap()
        .is_empty());

    let flag_wins = env.plan("archive", "flag.json", &["--older-than", "1y"]);
    let json = plan_json(&flag_wins);
    assert_eq!(json["params"]["older_than"], "1y");
    assert_eq!(json["actions"].as_array().unwrap().len(), 2);
}

#[test]
fn archive_ungueltige_dauer_wird_klar_abgelehnt() {
    let env = Env::new();
    archive_tree(&env);
    env.scan();
    env.bin()
        .args(["plan", "archive"])
        .arg(env.root())
        .args(["--older-than", "bald"])
        .assert()
        .failure()
        .stderr(contains("bald"));
}

#[test]
fn archive_ist_idempotent_nach_apply_ist_der_naechste_plan_leer() {
    let env = Env::new();
    archive_tree(&env);
    let plan = env.plan("archive", "plan1.json", &[]);
    env.apply(&plan).success();

    let second = env.plan("archive", "plan2.json", &[]);

    assert!(plan_json(&second)["actions"].as_array().unwrap().is_empty());
}

#[test]
fn archive_ordner_der_zwischen_plan_und_apply_veraendert_wurde_ist_stale() {
    let env = Env::new();
    archive_tree(&env);
    let plan = env.plan("archive", "plan.json", &[]);
    env.write("Projekt2019/neu.txt", "inzwischen angelegt");

    env.apply(&plan).code(2);

    assert!(env.exists(r"Projekt2019\neu.txt"), "stale Ordner bleibt");
    assert!(!env.exists(r"Aktiv\alt"), "der Rest läuft");
}

#[test]
fn archive_geschuetzter_unterordner_verhindert_das_verschieben() {
    let env = Env::new();
    archive_tree(&env);
    env.config(&format!(
        "protected_paths = [{:?}]\n",
        env.root().join("Projekt2019").join("sub").to_string_lossy()
    ));
    let plan = env.plan("archive", "plan.json", &[]);
    let json = plan_json(&plan);
    assert_eq!(action_paths(&json, env.root()), [r"Aktiv\alt"]);
    assert!(json["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["reason"] == "protected"));
}

#[test]
fn archive_zaehlt_dateien_im_ordner_fuer_die_onedrive_obergrenze() {
    let env = Env::new();
    archive_tree(&env);
    env.config("onedrive_max_move_files = 2\n");
    let plan = env.plan("archive", "plan.json", &[]);
    let before = env.snapshot();

    // Projekt2019 (2 Dateien) + Aktiv\alt (1 Datei) = 3 > 2
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
    assert!(!env.exists("Projekt2019"));
}

// ---------------------------------------------------------------------------------------------
// versions
// ---------------------------------------------------------------------------------------------

fn versions_tree(env: &Env) {
    let v1 = env.write("Bericht_v1.docx", "version eins");
    let v2 = env.write("Bericht_v2.docx", "version zwei");
    env.write("Bericht final.docx", "final");
    env.age_days(&v1, 400);
    env.age_days(&v2, 200);
    let p1 = env.write("Sub/Plan_v1.xlsx", "plan eins");
    env.write("Sub/Plan_v2.xlsx", "plan zwei");
    env.age_days(&p1, 100);
    env.write("Einzeln.txt", "ohne gruppe");
}

#[test]
fn versions_plan_apply_undo_behaelt_die_juengste_und_stellt_alles_her() {
    let env = Env::new();
    versions_tree(&env);
    let before = env.snapshot();

    let plan = env.plan("versions", "plan.json", &[]);
    assert_eq!(env.snapshot(), before, "plan darf nichts verändern");

    let json = plan_json(&plan);
    assert_eq!(json["kind"], "versions");
    assert_eq!(json["params"]["min_age"], "30d");
    assert_eq!(
        action_paths(&json, env.root()),
        [r"Bericht_v1.docx", r"Bericht_v2.docx", r"Sub\Plan_v1.xlsx"]
    );
    for action in json["actions"].as_array().unwrap() {
        assert_eq!(action["type"], "move");
        assert_eq!(action["reason"], "older-version");
        assert!(action.get("hash").is_none() && action.get("is_dir").is_none());
        let target = action["target"].as_str().unwrap();
        assert!(target.contains(r"\_Archiv\Versionen\"), "{target}");
        let keep = action["keep"].as_str().unwrap();
        if action["path"].as_str().unwrap().contains("Plan_v1") {
            assert!(keep.ends_with(r"Sub\Plan_v2.xlsx"), "{keep}");
        } else {
            assert!(keep.ends_with("Bericht final.docx"), "{keep}");
        }
    }

    let out = stdout(env.apply(&plan).success());
    assert!(!env.exists("Bericht_v1.docx") && !env.exists(r"Sub\Plan_v1.xlsx"));
    assert!(env.exists("Bericht final.docx") && env.exists(r"Sub\Plan_v2.xlsx"));
    assert_eq!(
        std::fs::read_to_string(env.root().join(r"_Archiv\Versionen\Sub\Plan_v1.xlsx")).unwrap(),
        "plan eins"
    );

    env.undo(&run_id(&out)).success();

    assert_eq!(env.snapshot(), before, "byteidentisch inkl. mtime");
    assert!(!env.exists("_Archiv"), "leeres Archiv wird aufgeräumt");
}

#[test]
fn versions_juengere_aeltere_version_bleibt_und_erscheint_als_too_recent() {
    let env = Env::new();
    let v1 = env.write("a_v1.doc", "eins");
    env.write("a_v2.doc", "zwei");
    env.age_days(&v1, 10);
    // keep ist die jüngere; v1 ist nur 10 Tage alt
    env.age_days(&env.root().join("a_v2.doc"), 1);

    let plan = env.plan("versions", "plan.json", &[]);
    let json = plan_json(&plan);
    assert!(json["actions"].as_array().unwrap().is_empty());
    assert_eq!(json["skipped"][0]["reason"], "too-recent");

    let relaxed = env.plan("versions", "relaxed.json", &["--min-age", "5d"]);
    assert_eq!(action_paths(&plan_json(&relaxed), env.root()), ["a_v1.doc"]);
}

#[test]
fn versions_min_age_aus_der_config_und_ungueltige_dauer() {
    let env = Env::new();
    let v1 = env.write("a_v1.doc", "eins");
    env.write("a_v2.doc", "zwei");
    env.age_days(&v1, 10);
    env.age_days(&env.root().join("a_v2.doc"), 1);
    env.config("versions_min_age = \"5d\"\n");

    let plan = env.plan("versions", "plan.json", &[]);
    assert_eq!(action_paths(&plan_json(&plan), env.root()), ["a_v1.doc"]);
    assert_eq!(plan_json(&plan)["params"]["min_age"], "5d");

    env.bin()
        .args(["plan", "versions"])
        .arg(env.root())
        .args(["--min-age", "bald"])
        .assert()
        .failure()
        .stderr(contains("bald"));
}

#[test]
fn versions_ist_idempotent_nach_apply_ist_der_naechste_plan_leer() {
    let env = Env::new();
    versions_tree(&env);
    let plan = env.plan("versions", "plan1.json", &[]);
    env.apply(&plan).success();

    let second = env.plan("versions", "plan2.json", &[]);

    assert!(plan_json(&second)["actions"].as_array().unwrap().is_empty());
}

#[test]
fn versions_fehlende_behaltene_datei_ueberspringt_die_gruppe() {
    let env = Env::new();
    versions_tree(&env);
    let plan = env.plan("versions", "plan.json", &[]);
    std::fs::remove_file(env.root().join("Bericht final.docx")).unwrap();

    env.apply(&plan).code(2);

    assert!(env.exists("Bericht_v1.docx") && env.exists("Bericht_v2.docx"));
    assert!(!env.exists(r"Sub\Plan_v1.xlsx"), "die andere Gruppe läuft");
}

#[test]
fn versions_gleich_benannte_dateien_in_verschiedenen_ordnern_oder_mit_anderer_endung_bilden_keine_gruppe(
) {
    let env = Env::new();
    let a = env.write("a/Bericht_v1.docx", "x");
    let b = env.write("b/Bericht_v2.docx", "y");
    let c = env.write("c/Plan_v1.docx", "z");
    let d = env.write("c/Plan_v2.pdf", "w");
    for f in [&a, &b, &c, &d] {
        env.age_days(f, 300);
    }
    let plan = env.plan("versions", "plan.json", &[]);
    assert!(plan_json(&plan)["actions"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Ausgabe von apply und runs
// ---------------------------------------------------------------------------------------------

#[test]
fn apply_nennt_je_aktionstyp_was_geschehen_ist() {
    let env = Env::new();
    env.write("a/cache.tmp", "t");
    let junk = env.plan("junk", "junk.json", &[]);
    let text = stdout(env.apply(&junk).success());
    assert!(text.contains("1 in die Quarantäne verschoben"), "{text}");

    let env = Env::new();
    env.mkdir("leer/auch");
    let dirs = env.plan("empty-dirs", "dirs.json", &[]);
    let text = stdout(env.apply(&dirs).success());
    assert!(text.contains("2 leere Ordner entfernt"), "{text}");

    let env = Env::new();
    archive_tree(&env);
    let archive = env.plan("archive", "archive.json", &[]);
    let text = stdout(env.apply(&archive).success());
    assert!(text.contains("2 nach _Archiv verschoben"), "{text}");
}

#[test]
fn runs_zeigt_aktionen_je_typ_und_quarantaene_bytes_nur_fuer_quarantaene() {
    let env = Env::new();
    env.write("a/cache.tmp", "temporär");
    let junk = env.plan("junk", "junk.json", &[]);
    env.apply(&junk).success();
    let runs = stdout(env.bin().args(["runs"]).arg(env.root()).assert().success());
    assert!(runs.contains("1 Quarantäne (9 B)"), "{runs}");

    let env = Env::new();
    env.mkdir("leer/auch");
    let dirs = env.plan("empty-dirs", "dirs.json", &[]);
    env.apply(&dirs).success();
    let runs = stdout(env.bin().args(["runs"]).arg(env.root()).assert().success());
    assert!(runs.contains("2 Ordner"), "{runs}");
    assert!(
        !runs.contains("Quarantäne"),
        "keine Bytes für Ordner: {runs}"
    );

    let env = Env::new();
    archive_tree(&env);
    let archive = env.plan("archive", "archive.json", &[]);
    env.apply(&archive).success();
    let runs = stdout(env.bin().args(["runs"]).arg(env.root()).assert().success());
    assert!(runs.contains("2 Archiv"), "{runs}");
    assert!(
        !runs.contains("Quarantäne"),
        "keine Bytes fürs Archiv: {runs}"
    );
}
