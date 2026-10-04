//! Read-only-Nachweis: `scan` verändert den gescannten Baum nicht.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::os::windows::fs::MetadataExt;
use std::path::Path;

use assert_cmd::Command;

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    size: u64,
    mtime: u64,
    ctime: u64,
    attrs: u32,
    content: u64,
}

fn snapshot(root: &Path) -> BTreeMap<String, Entry> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Entry>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::metadata(&path).unwrap();
            let content = if meta.is_file() {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                std::fs::read(&path).unwrap().hash(&mut h);
                h.finish()
            } else {
                0
            };
            let key = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            out.insert(
                key,
                Entry {
                    size: meta.file_size(),
                    mtime: meta.last_write_time(),
                    ctime: meta.creation_time(),
                    attrs: meta.file_attributes(),
                    content,
                },
            );
            if meta.is_dir() {
                walk(&path, root, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn scan(home: &Path, tree: &Path) {
    Command::cargo_bin("ordner-cleanup")
        .unwrap()
        .env("ORDNER_CLEANUP_HOME", home)
        .arg("scan")
        .arg(tree)
        .assert()
        .success();
}

#[test]
fn scan_veraendert_den_baum_nicht() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tree.path().join("a").join("b")).unwrap();
    std::fs::create_dir_all(tree.path().join("leer")).unwrap();
    // Gleiche Größe und gleicher Inhalt: erzwingt die Hash-Phase (Dateien werden geöffnet).
    std::fs::write(tree.path().join("a").join("eins.txt"), "gleicher inhalt").unwrap();
    std::fs::write(
        tree.path().join("a").join("b").join("zwei.txt"),
        "gleicher inhalt",
    )
    .unwrap();
    std::fs::write(tree.path().join("anders.bin"), vec![7u8; 4096]).unwrap();

    let before = snapshot(tree.path());
    scan(home.path(), tree.path());
    // Zweiter Lauf: inkrementeller Pfad.
    scan(home.path(), tree.path());
    let after = snapshot(tree.path());

    assert_eq!(before, after);
}

#[test]
fn classify_veraendert_den_baum_nicht() {
    use ordner_cleanup::content::extract::office::testing::docx;
    use ordner_cleanup::content::extract::pdf::testing::pdf_with_pages;
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let cfg = home.path().join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), "[classify]\nocr = false\n").unwrap();
    std::fs::create_dir_all(tree.path().join("a")).unwrap();
    std::fs::write(
        tree.path().join("a").join("r.pdf"),
        pdf_with_pages(
            &["Rechnung Rechnungsnummer: R-1 Gesamtbetrag: 5,00 EUR"],
            None,
            None,
        ),
    )
    .unwrap();
    std::fs::write(
        tree.path().join("b.docx"),
        docx(&["Vertrag"], "", "", "2026-01-01T00:00:00Z"),
    )
    .unwrap();
    std::fs::write(tree.path().join("kaputt.pdf"), b"%PDF-1.4 kaputt").unwrap();
    std::fs::write(tree.path().join("n.txt"), "x").unwrap();

    scan(home.path(), tree.path());
    let before = snapshot(tree.path());
    for args in [vec![], vec!["--force"], vec!["--clear"]] {
        Command::cargo_bin("ordner-cleanup")
            .unwrap()
            .env("ORDNER_CLEANUP_HOME", home.path())
            .arg("classify")
            .arg(tree.path())
            .args(args)
            .assert()
            .success();
    }
    assert_eq!(
        before,
        snapshot(tree.path()),
        "inkl. mtime, Erstellzeit und Attribute"
    );
}
