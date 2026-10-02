use anyhow::{bail, Result};

use crate::cli::{Cli, Command};

/// Führt den Befehl aus und liefert den Exit-Code (0 OK, 2 OK mit Teilfehlern).
pub fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Scan(_) | Command::Report(_) | Command::Index(_) => {
            bail!("Noch nicht implementiert")
        }
    }
}
