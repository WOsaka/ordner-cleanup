//! Ende-zu-Ende von `classify` über die echte CLI mit Temp-Ordnern und isoliertem
//! `ORDNER_CLEANUP_HOME`. OCR und LLM sind aus (kein Sprachpaket und kein Ollama nötig).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use assert_cmd::assert::Assert;
use assert_cmd::Command;
use ordner_cleanup::content::extract::image::testing::jpeg_with_camera;
use ordner_cleanup::content::extract::office::testing::docx;
use ordner_cleanup::content::extract::pdf::testing::pdf_with_pages;
use ordner_cleanup::paths;
use predicates::str::contains;

const INVOICE: &str = "Rechnung Rechnungsnummer: RE-2026-0042 Rechnungsdatum: 30.09.2026 Zahlbar bis 30.10.2026 Zahlungsziel 14 Tage Gesamtbetrag: 119,00 EUR Vielen Dank fuer Ihren Auftrag und Ihr Vertrauen in unsere Firma";

struct Env {
    home: tempfile::TempDir,
    tree: tempfile::TempDir,
    out: tempfile::TempDir,
}

/// Pfad → (Größe, mtime, Inhalt)
type Snapshot = BTreeMap<String, (u64, SystemTime, Vec<u8>)>;

impl Env {
    fn new() -> Self {
        let env = Self {
            home: tempfile::tempdir().unwrap(),
            tree: tempfile::tempdir().unwrap(),
            out: tempfile::tempdir().unwrap(),
        };
        env.config("[classify]\nocr = false\n");
        env
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

    fn write_bytes(&self, rel: &str, content: &[u8]) -> PathBuf {
        let path = self.root().join(rel);
        std::fs::create_dir_all(paths::extended(path.parent().unwrap())).unwrap();
        std::fs::write(paths::extended(&path), content).unwrap();
        path
    }

    fn scan(&self) {
        self.bin().arg("scan").arg(self.root()).assert().success();
    }

    fn classify(&self, extra: &[&str]) -> Assert {
        self.bin()
            .arg("classify")
            .arg(self.root())
            .args(extra)
            .assert()
    }

    fn snapshot(&self) -> Snapshot {
        fn walk(dir: &Path, out: &mut Snapshot) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    let meta = std::fs::metadata(&path).unwrap();
                    out.insert(
                        path.to_string_lossy().into_owned(),
                        (
                            meta.len(),
                            meta.modified().unwrap(),
                            std::fs::read(&path).unwrap(),
                        ),
                    );
                }
            }
        }
        let mut out = Snapshot::new();
        walk(self.root(), &mut out);
        out
    }

    fn corpus(&self) {
        self.write_bytes("scan0012.pdf", &pdf_with_pages(&[INVOICE], None, None));
        let mut photo = jpeg_with_camera("Apple", "iPhone 15", Some((38.7223, -9.1393)));
        photo.resize(40_000, 0);
        self.write_bytes("Urlaub/IMG_4711.jpg", &photo);
        self.write_bytes(
            "Brief.docx",
            &docx(&["Hallo Welt"], "", "", "2026-01-01T00:00:00Z"),
        );
        self.write_bytes("notiz.txt", b"nur Text");
    }
}

#[test]
fn ohne_index_bricht_classify_mit_hinweis_auf_scan_ab() {
    let env = Env::new();
    env.corpus();
    env.classify(&[]).failure().code(1).stderr(contains("scan"));
}

#[test]
fn nicht_gescannter_ordner_bricht_ab() {
    let env = Env::new();
    env.corpus();
    let other = tempfile::tempdir().unwrap();
    env.scan();
    env.bin()
        .arg("classify")
        .arg(other.path())
        .assert()
        .failure()
        .code(1)
        .stderr(contains("noch nicht gescannt"));
}

#[test]
fn scan_classify_zweiter_lauf_aus_dem_cache_und_clear() {
    let env = Env::new();
    env.corpus();
    env.scan();
    let before = env.snapshot();

    env.classify(&["--no-llm"])
        .success()
        .stdout(contains("4 Dateien geprüft: 4 analysiert, 0 aus dem Cache"))
        .stdout(contains("rechnung 1"))
        .stdout(contains("foto 1"))
        .stdout(contains("Zum Prüfen: 0"))
        .stdout(contains("Dauer"));
    assert_eq!(
        env.snapshot(),
        before,
        "classify verändert den Ordner nicht"
    );

    env.classify(&["--no-llm"])
        .success()
        .stdout(contains("0 analysiert, 4 aus dem Cache"));

    env.classify(&["--ext", "pdf", "--force"])
        .success()
        .stdout(contains("1 Dateien geprüft: 1 analysiert, 0 aus dem Cache"));

    env.classify(&["--clear"])
        .success()
        .stdout(contains("geleert (4 Einträge)"));
    env.classify(&[])
        .success()
        .stdout(contains("4 analysiert, 0 aus dem Cache"));
    assert_eq!(env.snapshot(), before);
}

#[test]
fn geaenderte_datei_wird_neu_analysiert_und_neue_kategorien_ordnen_neu_zu() {
    let env = Env::new();
    env.corpus();
    env.scan();
    env.classify(&[]).success();

    // Datei ändert sich, Neu-Scan, nur sie wird analysiert
    std::thread::sleep(std::time::Duration::from_millis(30));
    env.write_bytes(
        "scan0012.pdf",
        &pdf_with_pages(&[INVOICE, "Seite 2"], None, None),
    );
    env.scan();
    env.classify(&[])
        .success()
        .stdout(contains("1 analysiert, 3 aus dem Cache"));

    // eigene Kategorie: betroffene Dateien werden neu zugeordnet
    let cfg = env.home.path().join("config");
    std::fs::write(
        cfg.join("categories.toml"),
        "[[categories]]\nname = \"hallo\"\nkeywords = { hallo = 9 }\next = [\"docx\"]\n",
    )
    .unwrap();
    env.classify(&[])
        .success()
        .stdout(contains("Kategorien geändert: 4 Dateien neu zugeordnet"))
        .stdout(contains("hallo 1"));
    env.classify(&[])
        .success()
        .stdout(contains("0 analysiert, 4 aus dem Cache"));
}

#[test]
fn fehlerhafte_kategorie_datei_bricht_vor_der_analyse_ab() {
    let env = Env::new();
    env.corpus();
    env.scan();
    let cfg = env.home.path().join("config");
    std::fs::write(
        cfg.join("categories.toml"),
        "[[categories]]\nname = \"kaputt\"\nkeywords = { x = 99 }\n",
    )
    .unwrap();
    env.classify(&[])
        .failure()
        .code(1)
        .stderr(contains("categories.toml:1"))
        .stderr(contains("kaputt"))
        .stderr(contains("1 bis 10"));
}

#[test]
fn only_waehlt_dateien_nach_glob_aus() {
    let env = Env::new();
    env.corpus();
    env.scan();
    env.classify(&["--only", "Urlaub/**"])
        .success()
        .stdout(contains("1 Dateien geprüft: 1 analysiert"));
}

#[test]
fn clear_schliesst_andere_schalter_aus() {
    let env = Env::new();
    env.corpus();
    env.scan();
    env.classify(&["--clear", "--force"]).failure().code(2);
}

#[test]
fn endpoint_ausserhalb_von_loopback_ist_ein_config_fehler() {
    let env = Env::new();
    env.corpus();
    env.scan();
    env.config("[llm]\nenabled = true\nendpoint = \"http://192.168.1.5:11434\"\n");
    env.classify(&[])
        .failure()
        .code(1)
        .stderr(contains("Loopback"));
}

#[test]
fn llm_ohne_server_warnt_und_laeuft_durch() {
    let env = Env::new();
    env.corpus();
    env.scan();
    // freier Port ohne Server
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    env.config(&format!(
        "[classify]\nocr = false\n\n[llm]\nenabled = true\nendpoint = \"http://127.0.0.1:{port}\"\ntimeout = \"2s\"\n"
    ));
    env.classify(&[])
        .success()
        .stdout(contains("Warnung: LLM nicht erreichbar"))
        .stdout(contains("rechnung 1"));
}

// ---- OCR (nur mit deutschem Sprachpaket) ----

const SCAN_JPG: &[u8] = include_bytes!("fixtures/content/rechnung-scan.jpg");

/// Prüft in einem Hilfsprozess, ob das deutsche OCR-Sprachpaket da ist: WinRT bleibt so aus dem
/// Testprozess draußen (dort stürzt es auf manchen Systemen beim Beenden ab).
fn german_ocr_probe(exe: &str) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("probe.pdf");
    std::fs::write(&file, b"%PDF-1.4").unwrap();
    let out = std::process::Command::new(exe)
        .arg("ocr-worker")
        .arg(&file)
        .args(["--langs", "de"])
        .output()
        .unwrap();
    !String::from_utf8_lossy(&out.stdout).contains("no-language")
}

fn german_ocr_available() -> bool {
    let available = german_ocr_probe(env!("CARGO_BIN_EXE_ordner-cleanup"));
    if !available {
        eprintln!("OCR-Test übersprungen: kein deutsches OCR-Sprachpaket");
    }
    available
}

impl Env {
    fn ocr_config(&self, cache: bool) {
        self.config(&format!(
            "[classify]\nocr = true\nocr_languages = [\"de\"]\ncache_ocr_text = {cache}\n"
        ));
    }

    fn index_bytes(&self) -> Vec<u8> {
        let dir = self.home.path().join("data");
        let mut all = Vec::new();
        for name in ["index.db", "index.db-wal"] {
            if let Ok(b) = std::fs::read(dir.join(name)) {
                all.extend(b);
            }
        }
        all
    }

    fn ocr_rows(&self) -> i64 {
        let conn =
            rusqlite::Connection::open(self.home.path().join("data").join("index.db")).unwrap();
        conn.query_row("SELECT COUNT(*) FROM ocr_text", [], |r| r.get(0))
            .unwrap()
    }

    fn scans(&self) {
        use ordner_cleanup::content::extract::pdf::testing::scanned_pdf;
        self.write_bytes("Scans/scan.pdf", &scanned_pdf(SCAN_JPG, 1600, 700));
        self.write_bytes("Scans/foto-scan.jpg", SCAN_JPG);
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn ocr_liest_scans_cacht_verschluesselt_und_ordnet_ohne_neues_ocr_neu_zu() {
    if !german_ocr_available() {
        return;
    }
    let env = Env::new();
    env.ocr_config(true);
    env.scans();
    env.scan();
    let before = env.snapshot();

    env.classify(&[])
        .success()
        .stdout(contains("2 Dateien geprüft: 2 analysiert"))
        .stdout(contains("rechnung 2"))
        .stdout(contains("OCR 2"));
    assert_eq!(env.ocr_rows(), 2);
    let raw = env.index_bytes();
    assert!(!raw.is_empty());
    for word in ["Musterfirma", "Gesamtbetrag", "Vielen Dank"] {
        assert!(
            !contains_bytes(&raw, word.as_bytes()),
            "OCR-Klartext {word:?} steht im Index"
        );
    }

    // geänderte Kategorien: neu zuordnen, OCR-Text kommt aus dem Cache
    let cfg = env.home.path().join("config");
    std::fs::write(
        cfg.join("categories.toml"),
        "[[categories]]\nname = \"musterfirma\"\nkeywords = { musterfirma = 9 }\n",
    )
    .unwrap();
    env.classify(&[]).success().stdout(contains(
        "Kategorien geändert: 2 Dateien neu zugeordnet (OCR-Text aus dem Cache)",
    ));
    assert_eq!(env.snapshot(), before);
}

#[test]
fn ohne_ocr_cache_entsteht_kein_eintrag_und_vorhandene_werden_geloescht() {
    if !german_ocr_available() {
        return;
    }
    let env = Env::new();
    env.ocr_config(true);
    env.scans();
    env.scan();
    env.classify(&[]).success();
    assert_eq!(env.ocr_rows(), 2);

    env.ocr_config(false);
    env.classify(&["--force"])
        .success()
        .stdout(contains("rechnung 2"));
    assert_eq!(
        env.ocr_rows(),
        0,
        "mit cache_ocr_text = false wird nichts gespeichert"
    );
}

#[test]
fn ocr_abgeschaltet_laesst_scans_ohne_kategorie() {
    let env = Env::new();
    env.scans();
    env.scan();
    env.classify(&[])
        .success()
        .stdout(contains("ohne Kategorie 2"))
        .stdout(contains("OCR 0"));
}

#[test]
fn bericht_enthaelt_den_abschnitt_inhalte_und_content_csv() {
    let env = Env::new();
    env.corpus();
    env.scan();
    let out = env.out.path().join("bericht");
    env.bin()
        .arg("report")
        .arg(env.root())
        .arg("--out")
        .arg(&out)
        .assert()
        .success()
        .stdout(contains("noch nicht klassifiziert"));
    env.classify(&[]).success();
    let out2 = env.out.path().join("bericht2");
    env.bin()
        .arg("report")
        .arg(env.root())
        .arg("--out")
        .arg(&out2)
        .assert()
        .success()
        .stdout(contains("Inhalte"))
        .stdout(contains("rechnung 1"));
    let report_dir = std::fs::read_dir(&out2)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let out2 = report_dir;
    let csv = std::fs::read_to_string(out2.join("content.csv")).unwrap();
    assert!(
        csv.contains("Kategorie") && csv.contains("rechnung"),
        "{csv}"
    );
    assert!(
        csv.contains("RE-2026-0042") && csv.contains("2026-09-30"),
        "{csv}"
    );
    let html = std::fs::read_to_string(out2.join("report.html")).unwrap();
    assert!(html.contains("id=\"content\""));
    let json = std::fs::read_to_string(out2.join("report.json")).unwrap();
    assert!(json.contains("doc_number") && json.contains("RE-2026-0042"));
}

// ---- Synthetischer Korpus: erwartete Kategorie und Felder je Datei ----

#[test]
fn korpus_stimmt_mit_der_erwarteten_liste_ueberein() {
    use ordner_cleanup::content::extract::image::testing::png_header;
    use ordner_cleanup::content::extract::office::testing::{ole_container, pptx, xlsx};
    use ordner_cleanup::content::extract::video::testing::{mac_seconds, mp4};

    let env = Env::new();
    env.write_bytes("rechnung.pdf", &pdf_with_pages(&[INVOICE], None, None));
    env.write_bytes(
        "mahnung.docx",
        &docx(
            &[
                "Letzte Mahnung",
                "Zahlungserinnerung: Ihre Rechnung ist überfällig",
                "Mahngebühren 5,00 EUR",
                "Zahlungsaufforderung",
            ],
            "",
            "",
            "2026-01-01T00:00:00Z",
        ),
    );
    env.write_bytes(
        "konto.xlsx",
        &xlsx(&[
            "Kontoauszug",
            "Alter Kontostand 100,00",
            "Neuer Kontostand 90,00",
            "Buchungstag",
        ]),
    );
    env.write_bytes(
        "vertrag.pptx",
        &pptx(&[
            "Mietvertrag Vertragsnummer: V-7788",
            "Kündigungsfrist 3 Monate",
            "Vertragslaufzeit 24 Monate",
        ]),
    );
    let mut photo = jpeg_with_camera("Apple", "iPhone 15", Some((38.7223, -9.1393)));
    photo.resize(40_000, 0);
    env.write_bytes("IMG_4711.jpg", &photo);
    let mut shot = png_header(1920, 1080);
    shot.resize(30_000, 0);
    env.write_bytes("Screenshot 2026-09-01.png", &shot);
    env.write_bytes(
        "urlaub.mp4",
        &mp4(mac_seconds(2025, 7, 14, 12), false, true),
    );
    env.write_bytes("verschluesselt.docx", &ole_container());
    env.write_bytes("kaputt.pdf", b"%PDF-1.4 kaputt");
    env.write_bytes("notiz.txt", b"Rechnung");
    env.scan();
    env.classify(&["--no-llm"])
        .success()
        .stdout(contains("Nicht lesbar: 2"));

    let out = env.out.path().join("b");
    env.bin()
        .arg("report")
        .arg(env.root())
        .arg("--out")
        .arg(&out)
        .assert()
        .success();
    let dir = std::fs::read_dir(&out)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let csv = std::fs::read_to_string(dir.join("content.csv")).unwrap();
    let rows: BTreeMap<String, Vec<String>> = csv
        .trim_start_matches('\u{feff}')
        .lines()
        .skip(1)
        .map(|l| {
            let c: Vec<String> = l.split(';').map(String::from).collect();
            (c[0].rsplit('\\').next().unwrap().to_string(), c)
        })
        .collect();
    // Spalten: Pfad;Status;Kategorie;Konfidenz;Quelle;Datum;Absender;Nummer;Betrag;Titel;Zum Prüfen
    let expect = [
        ("rechnung.pdf", "ok", "rechnung"),
        ("mahnung.docx", "ok", "mahnung"),
        ("konto.xlsx", "ok", "kontoauszug"),
        ("vertrag.pptx", "ok", "vertrag"),
        ("IMG_4711.jpg", "ok", "foto"),
        ("Screenshot 2026-09-01.png", "ok", "screenshot"),
        ("urlaub.mp4", "ok", ""),
        ("verschluesselt.docx", "unreadable:encrypted", ""),
        ("kaputt.pdf", "unreadable:corrupt", ""),
        ("notiz.txt", "unsupported", ""),
    ];
    assert_eq!(rows.len(), expect.len(), "{rows:?}");
    for (name, status, category) in expect {
        let row = &rows[name];
        assert_eq!(
            (row[1].as_str(), row[2].as_str()),
            (status, category),
            "{name}: {row:?}"
        );
        if !category.is_empty() {
            let conf: f32 = row[3].parse().unwrap();
            assert!(conf >= 0.8, "{name} Konfidenz {conf}");
            assert_eq!(row[10], "false", "{name} nicht zum Prüfen");
        }
    }
    let r = &rows["rechnung.pdf"];
    assert_eq!(
        (r[5].as_str(), r[7].as_str(), r[8].as_str()),
        ("2026-09-30", "RE-2026-0042", "119,00")
    );
    assert_eq!(rows["vertrag.pptx"][7], "V-7788");
}
