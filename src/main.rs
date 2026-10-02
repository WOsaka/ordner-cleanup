use clap::Parser;
use ordner_cleanup::{app, cli::Cli};

fn main() {
    let cli = Cli::parse();
    match app::run(cli) {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("Fehler: {err:#}");
            std::process::exit(1);
        }
    }
}
