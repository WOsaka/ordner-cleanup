//! Schriften: Segoe UI (Text), Consolas (Pfade, TOML) und Fallbacks für Symbole und CJK. Fehlen
//! die Dateien, bleiben die egui-Standardschriften.

use eframe::egui::{FontData, FontDefinitions, FontFamily};

const FONT_DIR: &str = r"C:\Windows\Fonts";

fn load(file: &str) -> Option<Vec<u8>> {
    std::fs::read(std::path::Path::new(FONT_DIR).join(file)).ok()
}

pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let mut add = |key: &str, file: &str, families: &[(FontFamily, bool)]| {
        let Some(bytes) = load(file) else { return };
        fonts
            .font_data
            .insert(key.to_string(), FontData::from_owned(bytes).into());
        for (family, first) in families {
            let list = fonts.families.entry(family.clone()).or_default();
            if *first {
                list.insert(0, key.to_string());
            } else {
                list.push(key.to_string());
            }
        }
    };
    add("segoe", "segoeui.ttf", &[(FontFamily::Proportional, true)]);
    add("consolas", "consola.ttf", &[(FontFamily::Monospace, true)]);
    for (key, file) in [("symbols", "seguisym.ttf"), ("cjk", "msyh.ttc")] {
        add(
            key,
            file,
            &[
                (FontFamily::Proportional, false),
                (FontFamily::Monospace, false),
            ],
        );
    }
    fonts
}
