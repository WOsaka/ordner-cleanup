use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ordner-cleanup",
    version,
    about = "Scan und Analyse-Bericht für Ordnersysteme (rein lesend)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Ordnerbaum scannen und im Index speichern
    Scan(ScanArgs),
    /// Bericht aus dem Index erzeugen
    Report(ReportArgs),
    /// Gescannte Wurzeln im Index verwalten
    #[command(subcommand)]
    Index(IndexCommand),
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Zu scannender Ordner
    pub path: PathBuf,
    /// Zusätzliches Ausschluss-Glob (mehrfach möglich)
    #[arg(long = "exclude")]
    pub exclude: Vec<String>,
    /// Glob, dessen Ordner nur aufsummiert werden (mehrfach möglich)
    #[arg(long = "summary-only")]
    pub summary_only: Vec<String>,
    /// Eingebaute Default-Ausschlüsse deaktivieren
    #[arg(long)]
    pub no_default_excludes: bool,
    /// Netzlaufwerke/UNC-Pfade trotz Warnung scannen
    #[arg(long)]
    pub force: bool,
    /// Index verwerfen und neu aufbauen
    #[arg(long)]
    pub reset_index: bool,
    /// Anzahl Threads (0 = automatisch)
    #[arg(long)]
    pub threads: Option<usize>,
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Gescannte Wurzel (optional, wenn nur eine im Index ist)
    pub path: Option<PathBuf>,
    /// Zielordner für die Berichtsdateien
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Ausgabeformate: html, json, csv (kommagetrennt)
    #[arg(long, value_delimiter = ',')]
    pub format: Vec<String>,
    /// Ab welchem Alter Dateien als „alt“ gelten, z. B. 1y, 18m, 90d
    #[arg(long)]
    pub old_after: Option<String>,
    /// Anzahl Einträge in den Top-Listen
    #[arg(long)]
    pub top: Option<usize>,
}

#[derive(Debug, Subcommand)]
pub enum IndexCommand {
    /// Gescannte Wurzeln auflisten
    List,
    /// Wurzel aus dem Index entfernen
    Remove { path: PathBuf },
}
