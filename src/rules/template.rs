//! Ziel-Vorlage einer Regel: wird beim Laden in Tokens zerlegt und geprüft, beim Planen
//! mit den Werten einer Datei eingesetzt.

use std::fmt::Write as _;

use chrono::format::{Item, StrftimeItems};
use chrono::{Datelike, NaiveDateTime};

/// Format, wenn `{exif.date}` ohne Angabe steht.
const DEFAULT_EXIF_FORMAT: &str = "%Y-%m-%d";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Literal(String),
    Name,
    Ext,
    Parent,
    Year,
    Month,
    Day,
    Exif(String),
    Group(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// Endet auf `/`: nur Ordner, der Dateiname bleibt.
    Folder,
    /// Das letzte Segment ist der neue Dateiname.
    File,
    /// `./name`: Umbenennen im selben Ordner.
    SameDir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    kind: TargetKind,
    segments: Vec<Vec<Token>>,
}

/// Werte einer Datei für die Platzhalter.
pub struct Values<'a> {
    /// Dateiname ohne Endung.
    pub name: &'a str,
    pub ext: &'a str,
    pub parent: &'a str,
    /// mtime in lokaler Zeit.
    pub mtime: NaiveDateTime,
    /// Aufnahmedatum (bereits mit Fallback aufgelöst); `None` = nicht verfügbar.
    pub exif: Option<NaiveDateTime>,
    pub groups: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub dirs: Vec<String>,
    /// `None` bei [`TargetKind::Folder`]: der Dateiname bleibt.
    pub file: Option<String>,
    pub same_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    NoExifDate,
    Invalid(String),
}

impl Template {
    /// `groups`: Anzahl der Regex-Gruppen (`None` = die Regel hat keine `name_regex`).
    pub fn parse(text: &str, groups: Option<usize>) -> Result<Self, String> {
        if text.is_empty() {
            return Err("target darf nicht leer sein".into());
        }
        let (same_dir, rest) = match text.strip_prefix("./").or_else(|| text.strip_prefix(".\\")) {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let b = rest.as_bytes();
        let has_drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
        if rest.starts_with(['/', '\\']) || has_drive {
            return Err(format!(
                "target „{text}“ muss relativ zur Wurzel sein (kein absoluter Pfad, kein Laufwerk, kein UNC)"
            ));
        }
        let mut segments = tokenize(rest, groups)?;
        let trailing_sep = segments.last().is_some_and(Vec::is_empty);
        if trailing_sep {
            segments.pop();
        }
        if segments.is_empty() {
            return Err(format!("target „{text}“ enthält keinen Namen"));
        }
        for segment in &segments {
            match segment.as_slice() {
                [] => return Err(format!("target „{text}“ enthält ein leeres Segment")),
                [Token::Literal(l)] if l == "." || l == ".." => {
                    return Err(format!("target „{text}“ darf „.“ und „..“ nicht enthalten"))
                }
                _ => {}
            }
        }
        let kind = match (same_dir, trailing_sep) {
            (true, true) => {
                return Err(format!(
                    "target „{text}“: „./“ braucht einen neuen Dateinamen"
                ))
            }
            (true, false) if segments.len() > 1 => {
                return Err(format!(
                    "target „{text}“: Umbenennen mit „./“ darf keinen Ordner enthalten"
                ))
            }
            (true, false) => TargetKind::SameDir,
            (false, true) => TargetKind::Folder,
            (false, false) => TargetKind::File,
        };
        Ok(Self { kind, segments })
    }

    pub fn kind(&self) -> TargetKind {
        self.kind
    }

    pub fn uses_exif(&self) -> bool {
        self.segments
            .iter()
            .flatten()
            .any(|t| matches!(t, Token::Exif(_)))
    }

    pub fn expand(&self, values: &Values) -> Result<Expanded, TargetError> {
        let mut out = Vec::with_capacity(self.segments.len());
        for segment in &self.segments {
            let mut text = String::new();
            for token in segment {
                match token {
                    Token::Literal(l) => text.push_str(l),
                    Token::Name => text.push_str(values.name),
                    Token::Ext => text.push_str(values.ext),
                    Token::Parent => text.push_str(values.parent),
                    Token::Year => {
                        let _ = write!(text, "{:04}", values.mtime.year());
                    }
                    Token::Month => {
                        let _ = write!(text, "{:02}", values.mtime.month());
                    }
                    Token::Day => {
                        let _ = write!(text, "{:02}", values.mtime.day());
                    }
                    Token::Exif(format) => {
                        let date = values.exif.ok_or(TargetError::NoExifDate)?;
                        write!(text, "{}", date.format(format))
                            .map_err(|_| TargetError::Invalid("Datumsformat".into()))?;
                    }
                    Token::Group(n) => {
                        text.push_str(values.groups.get(n - 1).map_or("", String::as_str));
                    }
                }
            }
            if text.is_empty() || text == "." || text == ".." {
                return Err(TargetError::Invalid(format!(
                    "ungültiger Name „{text}“ nach dem Einsetzen der Platzhalter"
                )));
            }
            out.push(text);
        }
        let (dirs, file) = match self.kind {
            TargetKind::Folder => (out, None),
            TargetKind::File => {
                let file = out.pop();
                (out, file)
            }
            TargetKind::SameDir => (Vec::new(), out.pop()),
        };
        Ok(Expanded {
            dirs,
            file,
            same_dir: self.kind == TargetKind::SameDir,
        })
    }
}

/// Zerlegt in Segmente (an `/` und `\` im Literal-Text) aus Tokens.
fn tokenize(text: &str, groups: Option<usize>) -> Result<Vec<Vec<Token>>, String> {
    let mut segments: Vec<Vec<Token>> = Vec::new();
    let mut current: Vec<Token> = Vec::new();
    let mut literal = String::new();
    let flush = |literal: &mut String, current: &mut Vec<Token>| {
        if !literal.is_empty() {
            current.push(Token::Literal(std::mem::take(literal)));
        }
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                literal.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                literal.push('}');
            }
            '}' => return Err(format!("einzelnes „}}“ in „{text}“ (Escape: „}}}}“)")),
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => name.push(c),
                        None => return Err(format!("„{{“ ohne „}}“ in „{text}“")),
                    }
                }
                flush(&mut literal, &mut current);
                current.push(placeholder(&name, groups)?);
            }
            '/' | '\\' => {
                flush(&mut literal, &mut current);
                segments.push(std::mem::take(&mut current));
            }
            c => literal.push(c),
        }
    }
    flush(&mut literal, &mut current);
    segments.push(current);
    Ok(segments)
}

fn placeholder(name: &str, groups: Option<usize>) -> Result<Token, String> {
    Ok(match name {
        "name" => Token::Name,
        "ext" => Token::Ext,
        "parent" => Token::Parent,
        "year" => Token::Year,
        "month" => Token::Month,
        "day" => Token::Day,
        "exif.date" => Token::Exif(DEFAULT_EXIF_FORMAT.to_string()),
        _ if name.starts_with("exif.date:") => {
            let format = &name["exif.date:".len()..];
            validate_format(format)?;
            Token::Exif(format.to_string())
        }
        _ if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) => {
            let n: usize = name
                .parse()
                .map_err(|_| format!("Platzhalter „{{{name}}}“ ist zu groß"))?;
            let available = groups.ok_or_else(|| {
                format!("Platzhalter „{{{name}}}“ braucht eine name_regex mit Gruppen")
            })?;
            if n == 0 || n > available {
                return Err(format!(
                    "Platzhalter „{{{name}}}“: die name_regex hat {available} Gruppe(n)"
                ));
            }
            Token::Group(n)
        }
        _ => return Err(format!("unbekannter Platzhalter „{{{name}}}“")),
    })
}

fn validate_format(format: &str) -> Result<(), String> {
    if format.is_empty() {
        return Err("{exif.date:} braucht ein Format".into());
    }
    if format.contains(['/', '\\']) {
        return Err(format!(
            "Datumsformat „{format}“ darf keinen Pfadtrenner enthalten (Ordner bitte als eigene Segmente)"
        ));
    }
    if StrftimeItems::new(format).any(|item| matches!(item, Item::Error)) {
        return Err(format!("ungültiges Datumsformat „{format}“"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use rstest::rstest;

    fn dt(y: i32, m: u32, d: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(8, 5, 3)
            .unwrap()
    }

    fn values(groups: &[String], exif: Option<NaiveDateTime>) -> Values<'_> {
        Values {
            name: "Rechnung 4711",
            ext: "PDF",
            parent: "Downloads",
            mtime: dt(2026, 3, 7),
            exif,
            groups,
        }
    }

    fn expand(text: &str, groups: &[String], exif: Option<NaiveDateTime>) -> Expanded {
        Template::parse(text, Some(groups.len()))
            .unwrap()
            .expand(&values(groups, exif))
            .unwrap()
    }

    #[test]
    fn ordner_ziel_behaelt_den_dateinamen() {
        let e = expand("Finanzen/Rechnungen/{year}/", &[], None);
        assert_eq!(e.dirs, ["Finanzen", "Rechnungen", "2026"]);
        assert_eq!(e.file, None);
        assert!(!e.same_dir);
    }

    #[test]
    fn datei_ziel_hat_neuen_namen() {
        let e = expand("Doku\\{year}-{month}-{day}_{name}.{ext}", &[], None);
        assert_eq!(e.dirs, ["Doku"]);
        assert_eq!(e.file.as_deref(), Some("2026-03-07_Rechnung 4711.PDF"));
    }

    #[test]
    fn punkt_slash_benennt_im_selben_ordner_um() {
        let t = Template::parse("./{year}_{name}.{ext}", None).unwrap();
        assert_eq!(t.kind(), TargetKind::SameDir);
        let e = t.expand(&values(&[], None)).unwrap();
        assert!(e.same_dir && e.dirs.is_empty());
        assert_eq!(e.file.as_deref(), Some("2026_Rechnung 4711.PDF"));
    }

    #[test]
    fn parent_gruppen_und_exif() {
        let groups = vec!["4711".to_string(), "x".to_string()];
        let e = expand(
            "{parent}/{1}-{2}/{exif.date:%Y}/{exif.date}_{exif.date:%H}.{ext}",
            &groups,
            Some(dt(2024, 12, 31)),
        );
        assert_eq!(e.dirs, ["Downloads", "4711-x", "2024"]);
        assert_eq!(e.file.as_deref(), Some("2024-12-31_08.PDF"));
    }

    #[test]
    fn escape_fuer_geschweifte_klammern() {
        let e = expand("a{{b}}/", &[], None);
        assert_eq!(e.dirs, ["a{b}"]);
    }

    #[test]
    fn fehlendes_exif_datum_ist_ein_eigener_fehler() {
        let t = Template::parse("{exif.date}/", None).unwrap();
        assert!(t.uses_exif());
        assert_eq!(t.expand(&values(&[], None)), Err(TargetError::NoExifDate));
        assert!(!Template::parse("{year}/", None).unwrap().uses_exif());
    }

    #[test]
    fn leere_gruppe_ergibt_ungueltiges_ziel() {
        let groups = vec![String::new()];
        let t = Template::parse("{1}/{name}.{ext}", Some(1)).unwrap();
        assert!(matches!(
            t.expand(&values(&groups, None)),
            Err(TargetError::Invalid(_))
        ));
    }

    #[test]
    fn punkte_aus_gruppen_werden_abgelehnt() {
        let groups = vec!["..".to_string()];
        let t = Template::parse("{1}/x/", Some(1)).unwrap();
        assert!(matches!(
            t.expand(&values(&groups, None)),
            Err(TargetError::Invalid(_))
        ));
    }

    #[rstest]
    #[case("", "leer")]
    #[case("C:\\Ablage\\x\\", "relativ")]
    #[case("c:x/", "relativ")]
    #[case("/abs/", "relativ")]
    #[case("\\\\server\\share\\x\\", "relativ")]
    #[case("a/../b/", "„..“")]
    #[case("a/./b/", "„.“")]
    #[case("a//b/", "leeres Segment")]
    #[case("{foo}/", "unbekannter Platzhalter")]
    #[case("{name/", "ohne")]
    #[case("a}/", "einzelnes")]
    #[case("{1}/", "name_regex")]
    #[case("{exif.date:%Q}/", "Datumsformat")]
    #[case("{exif.date:}/", "Format")]
    #[case("{exif.date:%Y/%m}/", "Pfadtrenner")]
    #[case("./", "keinen Namen")]
    #[case("./a/b", "Ordner")]
    #[case("{0}/", "name_regex")]
    fn ungueltige_vorlagen(#[case] text: &str, #[case] hint: &str) {
        let err = Template::parse(text, None).unwrap_err();
        assert!(err.contains(hint), "{text}: {err}");
    }

    #[test]
    fn gruppe_ueber_der_anzahl_ist_ein_fehler() {
        let err = Template::parse("{3}/", Some(2)).unwrap_err();
        assert!(err.contains("2 Gruppe"), "{err}");
        assert!(Template::parse("{2}/", Some(2)).is_ok());
    }
}
