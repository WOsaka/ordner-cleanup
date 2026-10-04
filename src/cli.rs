use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::change::dedupe::KeepStrategy;
use crate::change::RunId;

#[derive(Debug, Parser)]
#[command(
    name = "ordner-cleanup",
    version,
    about = "Scan, Analyse-Bericht und sicheres Aufräumen für Ordnersysteme (Windows)"
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
    /// Änderungsplan erzeugen (verändert nichts)
    #[command(subcommand)]
    Plan(PlanCommand),
    /// Plan ausführen: Dateien wandern nach Bestätigung in die Quarantäne
    Apply(ApplyArgs),
    /// Einen Lauf zurückdrehen
    Undo(UndoArgs),
    /// Läufe mit Status und Quarantäne-Größe auflisten
    Runs(RunsArgs),
    /// Abgelaufene Quarantäne-Läufe endgültig löschen
    Purge(PurgeArgs),
}

#[derive(Debug, Args)]
pub struct ApplyArgs {
    /// Plan-Datei aus `plan`
    pub plan: PathBuf,
    /// Ohne Rückfrage ausführen (nötig in nicht interaktiven Sitzungen)
    #[arg(long)]
    pub yes: bool,
    /// Die OneDrive-Obergrenze (Dateianzahl/Größe) für diesen Plan aufheben
    #[arg(long)]
    pub allow_large: bool,
}

#[derive(Debug, Args)]
pub struct UndoArgs {
    /// Lauf-ID aus `apply` oder `runs`
    pub run_id: RunId,
    /// Wurzel des Laufs, falls er nicht im Register steht
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Ohne Rückfrage zurückdrehen
    #[arg(long)]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct RunsArgs {
    /// Nur Läufe dieser Wurzel (Default: alle bekannten)
    pub path: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PurgeArgs {
    /// Mindestalter eines Laufs, z. B. 30d, 2m (Default: quarantine_days aus der Config)
    #[arg(long)]
    pub older_than: Option<String>,
    /// Nur diese Wurzel (Default: alle bekannten)
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Ohne Rückfrage löschen
    #[arg(long)]
    pub yes: bool,
}

#[derive(Debug, Subcommand)]
pub enum PlanCommand {
    /// Exakte Duplikate in die Quarantäne planen
    Dedupe(PlanDedupeArgs),
    /// Müll (Temp-, Lock- und Download-Reste, alte Installer) in die Quarantäne planen
    Junk(PlanJunkArgs),
    /// Leere Ordner (rekursiv, von unten nach oben) zum Entfernen planen
    EmptyDirs(PlanEmptyDirsArgs),
    /// Lange unberührte Ordner nach `_Archiv\<Jahr>\…` verschieben
    Archive(PlanArchiveArgs),
    /// Ältere Versionen (`_v1`, `- Kopie`, …) nach `_Archiv\Versionen\…` verschieben
    Versions(PlanVersionsArgs),
    /// Dateien nach Regeln aus einer Regeldatei einsortieren und umbenennen
    Rules(PlanRulesArgs),
}

#[derive(Debug, Args)]
pub struct PlanRulesArgs {
    /// Bereits gescannter Ordner
    pub path: PathBuf,
    /// Regeldatei (Default: `rules_file` aus der Config, sonst `rules.toml` neben der Config)
    #[arg(long)]
    pub rules: Option<PathBuf>,
    /// Nur diese Regel(n) anwenden (mehrfach möglich)
    #[arg(long = "rule")]
    pub rule: Vec<String>,
    /// Zieldatei für den Plan (Default: plan-<Zeitstempel>.json im aktuellen Ordner)
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PlanDedupeArgs {
    /// Bereits gescannter Ordner
    pub path: PathBuf,
    /// Welche Kopie bleibt: oldest, newest oder path:<absoluter Ordner>
    #[arg(long, default_value = "oldest")]
    pub keep: KeepStrategy,
    /// Zieldatei für den Plan (Default: plan-<Zeitstempel>.json im aktuellen Ordner)
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PlanJunkArgs {
    /// Bereits gescannter Ordner
    pub path: PathBuf,
    /// Kategorien, kommagetrennt: system, temp, downloads, installer oder eigene aus der Config
    /// (Default: `junk_categories` aus der Config, sonst alle eingebauten)
    #[arg(long, value_delimiter = ',')]
    pub category: Vec<String>,
    /// Zieldatei für den Plan (Default: plan-<Zeitstempel>.json im aktuellen Ordner)
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PlanEmptyDirsArgs {
    /// Bereits gescannter Ordner
    pub path: PathBuf,
    /// Zieldatei für den Plan (Default: plan-<Zeitstempel>.json im aktuellen Ordner)
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PlanArchiveArgs {
    /// Bereits gescannter Ordner
    pub path: PathBuf,
    /// Ab welchem Alter der jüngsten Datei ein Ordner archiviert wird, z. B. 2y, 18m, 90d
    /// (Default: `archive_older_than` aus der Config, sonst 2y)
    #[arg(long)]
    pub older_than: Option<String>,
    /// Zieldatei für den Plan (Default: plan-<Zeitstempel>.json im aktuellen Ordner)
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PlanVersionsArgs {
    /// Bereits gescannter Ordner
    pub path: PathBuf,
    /// Mindestalter einer älteren Version, z. B. 30d, 2m
    /// (Default: `versions_min_age` aus der Config, sonst 30d)
    #[arg(long)]
    pub min_age: Option<String>,
    /// Zieldatei für den Plan (Default: plan-<Zeitstempel>.json im aktuellen Ordner)
    #[arg(long)]
    pub out: Option<PathBuf>,
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
