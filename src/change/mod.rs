//! Phase 2: Änderungsplan, Apply und Undo. Nichts wird hart gelöscht (außer `purge`).

pub mod dedupe;
pub mod plan;
pub mod protect;

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};

use serde::{Deserialize, Serialize};

/// Kennung eines Laufs: `YYYYMMDD-HHMMSS-<4 hex>`. Wird als Ordner- und Dateiname verwendet,
/// daher akzeptiert [`RunId::parse`] ausschließlich dieses Format (kein Pfad-Trick über die CLI).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RunId(String);

impl RunId {
    pub fn generate(now: chrono::DateTime<chrono::Local>) -> Self {
        let suffix = RandomState::new().build_hasher().finish() & 0xffff;
        Self(format!("{}-{suffix:04x}", now.format("%Y%m%d-%H%M%S")))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let bytes = text.as_bytes();
        let valid = bytes.len() == 20
            && bytes[..8].iter().all(u8::is_ascii_digit)
            && bytes[8] == b'-'
            && bytes[9..15].iter().all(u8::is_ascii_digit)
            && bytes[15] == b'-'
            && bytes[16..]
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b));
        if valid {
            Ok(Self(text.to_string()))
        } else {
            Err(format!(
                "Ungültige Lauf-ID „{text}“ (erwartet: JJJJMMTT-HHMMSS-xxxx)"
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RunId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        Self::parse(&value)
    }
}

impl From<RunId> for String {
    fn from(id: RunId) -> String {
        id.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Warum ein Eintrag nicht in den Plan kam bzw. nicht ausgeführt wurde.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkipReason {
    Protected,
    CloudPlaceholder,
    Hardlink,
    Link,
    GroupIncomplete,
    TooLong,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Protected => "geschützter Pfad",
            Self::CloudPlaceholder => "Cloud-Platzhalter",
            Self::Hardlink => "Hardlink",
            Self::Link => "Symlink/Junction",
            Self::GroupIncomplete => "keine zulässige Datei zum Behalten",
            Self::TooLong => "Quarantäne-Pfad zu lang",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn erzeugte_id_ist_gueltig_und_eindeutig_genug() {
        let a = RunId::generate(chrono::Local::now());
        assert_eq!(RunId::parse(a.as_str()).unwrap(), a);
        let ids: std::collections::HashSet<_> = (0..50)
            .map(|_| RunId::generate(chrono::Local::now()))
            .collect();
        assert!(ids.len() > 25, "Suffix muss variieren");
    }

    #[rstest]
    #[case("20261003-120000-ab12", true)]
    #[case("20261003-120000-AB12", false)]
    #[case("20261003-120000-ab1", false)]
    #[case("..", false)]
    #[case(r"..\..\Windows", false)]
    #[case("20261003-120000-ab12\\x", false)]
    #[case("", false)]
    fn run_id_validierung(#[case] text: &str, #[case] ok: bool) {
        assert_eq!(RunId::parse(text).is_ok(), ok, "{text}");
    }

    #[test]
    fn skip_reason_serialisiert_kebab_case() {
        assert_eq!(
            serde_json::to_string(&SkipReason::CloudPlaceholder).unwrap(),
            "\"cloud-placeholder\""
        );
    }
}
