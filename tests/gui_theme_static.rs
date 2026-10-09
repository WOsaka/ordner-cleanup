//! Statische Prüfung: Farben der Oberfläche kommen aus `gui/theme.rs`, nirgends sonst steht `Color32`.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn feste_farben_stehen_nur_in_theme_rs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("gui");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    assert!(files.len() > 10, "Quelltext nicht gefunden");
    let offenders: Vec<String> = files
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n != "theme.rs"))
        .filter(|p| std::fs::read_to_string(p).unwrap().contains("Color32"))
        .map(|p| p.strip_prefix(&root).unwrap().display().to_string())
        .collect();
    assert!(offenders.is_empty(), "feste Farben in: {offenders:?}");
}
