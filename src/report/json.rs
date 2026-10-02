use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::Report;

pub const FILE_NAME: &str = "report.json";

pub fn to_string(report: &Report) -> Result<String> {
    Ok(serde_json::to_string_pretty(report)?)
}

pub fn write(report: &Report, dir: &Path) -> Result<PathBuf> {
    let path = dir.join(FILE_NAME);
    std::fs::write(&path, to_string(report)?)
        .with_context(|| format!("{} konnte nicht geschrieben werden", path.display()))?;
    Ok(path)
}
