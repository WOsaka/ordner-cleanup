use clap::Parser;
use ordner_cleanup::change::plan::PlanError;
use ordner_cleanup::{app, cli::Cli};

fn main() {
    let cli = Cli::parse();
    match app::run(cli) {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("Fehler: {err:#}");
            // 3: Plan wurde nach dem Erstellen verändert (Integritätsprüfung fehlgeschlagen).
            let tampered = matches!(
                err.downcast_ref::<PlanError>(),
                Some(PlanError::Tampered { .. })
            );
            std::process::exit(if tampered { 3 } else { 1 });
        }
    }
}
