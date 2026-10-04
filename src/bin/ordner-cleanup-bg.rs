//! Dasselbe Programm ohne Konsolenfenster. Die Aufgabenplanung startet `ordner-cleanup-bg.exe run
//! --profile <name> --notify`; eine Konsolen-exe würde dabei jedes Mal ein Fenster öffnen.
//! Ausgaben gehen ins Leere, das Ergebnis steht im Lauf-Protokoll des Profils.
#![windows_subsystem = "windows"]

use clap::Parser;
use ordner_cleanup::{app, cli::Cli};

fn main() {
    let cli = Cli::parse();
    let code = app::run(cli).unwrap_or(1);
    std::process::exit(code);
}
