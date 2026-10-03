//! Erzeugt einen Testbaum für Performance-Messungen.
//!
//! `cargo run --release --example gen-tree -- <zielordner> [anzahl-dateien]`
//!
//! Layout: 100 Dateien pro Ordner, Ordner auf drei Ebenen verteilt. Jede zehnte Datei
//! hat einen gemeinsamen Inhalt (Duplikate), der Rest ist pro Datei eindeutig.

use std::path::PathBuf;

use anyhow::{bail, Result};

const FILES_PER_DIR: usize = 100;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(target) = args.next().map(PathBuf::from) else {
        bail!("Aufruf: gen-tree <zielordner> [anzahl-dateien]");
    };
    let total: usize = match args.next() {
        Some(n) => n.parse()?,
        None => 100_000,
    };
    if target.exists() && std::fs::read_dir(&target)?.next().is_some() {
        bail!("{} ist nicht leer", target.display());
    }

    let dirs = total.div_ceil(FILES_PER_DIR);
    let mut written = 0usize;
    for d in 0..dirs {
        let dir = target
            .join(format!("a{}", d % 10))
            .join(format!("b{}", (d / 10) % 10))
            .join(format!("c{d}"));
        std::fs::create_dir_all(&dir)?;
        for f in 0..FILES_PER_DIR.min(total - written) {
            let n = written;
            let ext = ["txt", "docx", "jpg", "pdf", "xlsx"][n % 5];
            let content = if n.is_multiple_of(10) {
                format!("gemeinsamer inhalt {}", n % 7).repeat(50)
            } else {
                format!("datei {n}").repeat(20 + n % 40)
            };
            std::fs::write(dir.join(format!("datei-{f}.{ext}")), content)?;
            written += 1;
        }
    }
    println!(
        "{written} Dateien in {dirs} Ordnern unter {}",
        target.display()
    );
    Ok(())
}
