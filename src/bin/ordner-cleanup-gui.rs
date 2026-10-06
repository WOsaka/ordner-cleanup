//! Grafische Oberfläche ohne Konsolenfenster. Als `ordner-cleanup-gui.exe ocr-worker …` verhält
//! sich die exe wie die Kommandozeile (der OCR-Hilfsprozess startet die eigene exe).
#![windows_subsystem = "windows"]

use clap::Parser;
use ordner_cleanup::{app, cli::Cli, gui};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("ocr-worker") {
        let code = app::run(Cli::parse()).unwrap_or(1);
        std::process::exit(code);
    }
    if gui::run().is_err() {
        std::process::exit(1);
    }
}
