//! Ziel-Vorlage einer Regel: wird beim Laden in Tokens zerlegt und geprüft, beim Planen
//! mit den Werten einer Datei eingesetzt.

use std::fmt::Write as _;

use chrono::format::{Item, StrftimeItems};
use chrono::{Datelike, NaiveDate, NaiveDateTime};

use crate::content::classify::fields::{clean_value, shorten};
use crate::content::{is_known_field, ContentRecord};

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
    /// `{category}`, `{doc.*}`, `{exif.camera}`, `{video.date}` … mit optionalem Datumsformat
    /// und Ersatzwert (`{doc.sender|Unbekannt}`)
    Field(FieldToken),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldToken {
    /// `category` oder ein Feldname aus [`crate::content::FIELD_NAMES`]
    pub name: String,
    /// Datumsformat (nur `doc.date` und `video.date`)
    pub format: Option<String>,
    /// Wird eingesetzt, wenn das Feld fehlt; darf Platzhalter enthalten, aber keinen weiteren
    /// Ersatz
    pub fallback: Option<Vec<Token>>,
}

/// Datumsfelder, die ein Format (`{doc.date:%Y}`) annehmen.
const DATE_FIELDS: [&str; 2] = ["doc.date", "video.date"];
/// Länge eines eingesetzten Feldwerts (`doc.title` darf länger sein).
const FIELD_MAX_CHARS: usize = 40;
const TITLE_MAX_CHARS: usize = 60;

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
    /// Ergebnis der Inhaltsklassifikation für `{category}`, `{doc.*}` …
    pub content: Option<&'a ContentRecord>,
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
    /// Ein Feld fehlt und hat keinen Ersatzwert
    MissingField(String),
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
        self.any_token(&|t| matches!(t, Token::Exif(_)))
    }

    /// Das Ziel braucht Inhalts-Felder (`{category}`, `{doc.*}`, `{exif.camera}` …).
    pub fn uses_content(&self) -> bool {
        self.any_token(&|t| matches!(t, Token::Field(_)))
    }

    fn any_token(&self, pred: &dyn Fn(&Token) -> bool) -> bool {
        fn walk(tokens: &[Token], pred: &dyn Fn(&Token) -> bool) -> bool {
            tokens.iter().any(|t| {
                pred(t)
                    || matches!(t, Token::Field(f) if f.fallback.as_deref().is_some_and(|fb| walk(fb, pred)))
            })
        }
        self.segments.iter().any(|s| walk(s, pred))
    }

    pub fn expand(&self, values: &Values) -> Result<Expanded, TargetError> {
        let mut out = Vec::with_capacity(self.segments.len());
        for segment in &self.segments {
            let text = expand_tokens(segment, values)?;
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

fn field_value(f: &FieldToken, values: &Values) -> Option<String> {
    let content = values.content?;
    let raw = if f.name == "category" {
        content.category.clone()?
    } else {
        content.fields.get(&f.name)?.clone()
    };
    if let Some(format) = &f.format {
        let date = NaiveDateTime::parse_from_str(&raw, "%Y-%m-%dT%H:%M:%S")
            .ok()
            .or_else(|| {
                NaiveDate::parse_from_str(&raw, "%Y-%m-%d")
                    .ok()
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
            })?;
        let mut out = String::new();
        write!(out, "{}", date.format(format)).ok()?;
        return Some(out).filter(|t| !t.is_empty());
    }
    let max = if f.name == "doc.title" {
        TITLE_MAX_CHARS
    } else {
        FIELD_MAX_CHARS
    };
    Some(shorten(&clean_value(&raw), max)).filter(|t| !t.is_empty())
}

fn expand_tokens(tokens: &[Token], values: &Values) -> Result<String, TargetError> {
    let mut text = String::new();
    for token in tokens {
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
            Token::Field(f) => match field_value(f, values) {
                Some(v) => text.push_str(&v),
                None => match &f.fallback {
                    Some(fallback) => text.push_str(&expand_tokens(fallback, values)?),
                    None => return Err(TargetError::MissingField(f.name.clone())),
                },
            },
        }
    }
    Ok(text)
}

/// Inhalt zwischen den äußeren Klammern; verschachtelte Klammern (`{doc.number|{name}}`) werden
/// mitgezählt.
fn read_placeholder(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    text: &str,
) -> Result<String, String> {
    let mut depth = 1usize;
    let mut inner = String::new();
    for c in chars.by_ref() {
        match c {
            '{' => {
                depth += 1;
                inner.push(c);
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(inner);
                }
                inner.push(c);
            }
            c => inner.push(c),
        }
    }
    Err(format!("„{{“ ohne „}}“ in „{text}“"))
}

/// Erstes `|` auf Klammer-Ebene 0.
fn split_fallback(inner: &str) -> (&str, Option<&str>) {
    let mut depth = 0usize;
    for (i, c) in inner.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => return (&inner[..i], Some(&inner[i + 1..])),
            _ => {}
        }
    }
    (inner, None)
}

/// Ersatz-Text als Token-Folge: Literale und Platzhalter, kein Pfadtrenner, kein weiterer Ersatz.
fn tokenize_fallback(text: &str, groups: Option<usize>) -> Result<Vec<Token>, String> {
    if text.is_empty() {
        return Err("Ersatzwert darf nicht leer sein".into());
    }
    if text.contains(['/', '\\']) {
        return Err(format!(
            "Ersatzwert „{text}“ darf keinen Pfadtrenner enthalten"
        ));
    }
    let mut tokens = Vec::new();
    let mut literal = String::new();
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
            '}' => return Err(format!("einzelnes „}}“ im Ersatzwert „{text}“")),
            '{' => {
                let inner = read_placeholder(&mut chars, text)?;
                if split_fallback(&inner).1.is_some() {
                    return Err(format!(
                        "Ersatzwert „{text}“ darf keinen weiteren Ersatz enthalten"
                    ));
                }
                if !literal.is_empty() {
                    tokens.push(Token::Literal(std::mem::take(&mut literal)));
                }
                tokens.push(placeholder(&inner, groups)?);
            }
            '|' => {
                return Err(format!(
                    "Ersatzwert „{text}“ darf keinen weiteren Ersatz enthalten („|“ ist in Windows-Namen verboten)"
                ))
            }
            c => literal.push(c),
        }
    }
    if !literal.is_empty() {
        tokens.push(Token::Literal(literal));
    }
    Ok(tokens)
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
                let inner = read_placeholder(&mut chars, text)?;
                flush(&mut literal, &mut current);
                current.push(placeholder(&inner, groups)?);
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

fn placeholder(inner: &str, groups: Option<usize>) -> Result<Token, String> {
    let (head, fallback) = split_fallback(inner);
    let name = head;
    let no_fallback = |token: Token| {
        match fallback {
        Some(_) => Err(format!(
            "Platzhalter „{{{inner}}}“: Ersatzwert gibt es nur bei Feldern ({{category}}, {{doc.*}} …)"
        )),
        None => Ok(token),
    }
    };
    match name {
        "name" => return no_fallback(Token::Name),
        "ext" => return no_fallback(Token::Ext),
        "parent" => return no_fallback(Token::Parent),
        "year" => return no_fallback(Token::Year),
        "month" => return no_fallback(Token::Month),
        "day" => return no_fallback(Token::Day),
        "exif.date" => {
            if fallback.is_some() {
                return Err(
                    "{exif.date} hat keinen Ersatzwert; bei fehlendem Datum gilt exif_fallback"
                        .into(),
                );
            }
            return Ok(Token::Exif(DEFAULT_EXIF_FORMAT.to_string()));
        }
        _ if name.starts_with("exif.date:") => {
            if fallback.is_some() {
                return Err(
                    "{exif.date} hat keinen Ersatzwert; bei fehlendem Datum gilt exif_fallback"
                        .into(),
                );
            }
            let format = &name["exif.date:".len()..];
            validate_format(format)?;
            return Ok(Token::Exif(format.to_string()));
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
            return no_fallback(Token::Group(n));
        }
        _ => {}
    }
    // Feld mit optionalem Format: `doc.date:%Y`
    let (field, format) = match name.split_once(':') {
        Some((f, fmt)) => (f, Some(fmt)),
        None => (name, None),
    };
    if field != "category" && !is_known_field(field) {
        return Err(format!("unbekannter Platzhalter „{{{name}}}“"));
    }
    let format = match format {
        None => None,
        Some(fmt) => {
            if !DATE_FIELDS.contains(&field) {
                return Err(format!(
                    "Platzhalter „{{{name}}}“: ein Format gibt es nur bei Datumsfeldern ({})",
                    DATE_FIELDS.join(", ")
                ));
            }
            validate_format(fmt)?;
            Some(fmt.to_string())
        }
    };
    let fallback = fallback
        .map(|text| tokenize_fallback(text, groups))
        .transpose()?;
    Ok(Token::Field(FieldToken {
        name: field.to_string(),
        format,
        fallback,
    }))
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
            content: None,
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

    // ---- Inhaltsfelder (Phase 6a) ----

    use crate::content::classify::fields::FieldSources;
    use crate::content::{ContentRecord, Fields, Source, Status, TextSource, EXTRACTOR_VERSION};

    fn record(category: Option<&str>, fields: &[(&str, &str)]) -> ContentRecord {
        ContentRecord {
            path_key: "k".into(),
            size: 1,
            mtime: 1,
            status: Status::Ok,
            category: category.map(String::from),
            confidence: 0.9,
            category2: None,
            source: Source::Rules,
            hits: vec![],
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Fields>(),
            field_sources: FieldSources::new(),
            text_source: TextSource::Layer,
            llm_model: None,
            defs_fingerprint: "f".into(),
            extractor_version: EXTRACTOR_VERSION,
            classified_at: "t".into(),
        }
    }

    fn expand_with(
        text: &str,
        groups: &[String],
        content: Option<&ContentRecord>,
    ) -> Result<Expanded, TargetError> {
        Template::parse(text, Some(groups.len()))
            .unwrap()
            .expand(&Values {
                content,
                ..values(groups, None)
            })
    }

    #[test]
    fn felder_kategorie_und_datumsformate() {
        let r = record(
            Some("rechnung"),
            &[
                ("doc.date", "2026-09-30"),
                ("doc.sender", "Telekom"),
                ("doc.number", "RE-5"),
                ("video.date", "2025-07-14T12:30:15"),
            ],
        );
        let e = expand_with(
            "{category}/{doc.date:%Y}/{doc.date:%Y-%m-%d}_{doc.sender}_{doc.number}.{ext}",
            &[],
            Some(&r),
        )
        .unwrap();
        assert_eq!(e.dirs, ["rechnung", "2026"]);
        assert_eq!(e.file.as_deref(), Some("2026-09-30_Telekom_RE-5.PDF"));
        assert!(
            Template::parse("{video.date:%Y/%m}/", None).is_err(),
            "Format mit Pfadtrenner schon beim Parsen"
        );
        let e = expand_with("{video.date:%Y}/{video.date:%H-%M}/", &[], Some(&r)).unwrap();
        assert_eq!(e.dirs, ["2025", "12-30"]);
    }

    #[test]
    fn ersatzwert_greift_nur_bei_fehlendem_feld_und_darf_platzhalter_enthalten() {
        let with_sender = record(None, &[("doc.sender", "Telekom")]);
        let without = record(None, &[]);
        let t = "{doc.sender|Unbekannt}_{doc.number|{name}}.{ext}";
        let e = expand_with(t, &[], Some(&with_sender)).unwrap();
        assert_eq!(e.file.as_deref(), Some("Telekom_Rechnung 4711.PDF"));
        let e = expand_with(t, &[], Some(&without)).unwrap();
        assert_eq!(e.file.as_deref(), Some("Unbekannt_Rechnung 4711.PDF"));
        let e = expand_with(t, &[], None).unwrap();
        assert_eq!(
            e.file.as_deref(),
            Some("Unbekannt_Rechnung 4711.PDF"),
            "ohne Datensatz fehlen alle Felder"
        );
        let groups = vec!["77".to_string()];
        let e = expand_with("{doc.number|Nr{1}}.{ext}", &groups, Some(&without)).unwrap();
        assert_eq!(e.file.as_deref(), Some("Nr77.PDF"));
    }

    #[test]
    fn fehlendes_feld_ohne_ersatz_nennt_den_namen() {
        let r = record(None, &[]);
        assert_eq!(
            expand_with("{doc.sender}/", &[], Some(&r)),
            Err(TargetError::MissingField("doc.sender".into()))
        );
        assert_eq!(
            expand_with("{doc.sender|{doc.title}}/", &[], Some(&r)),
            Err(TargetError::MissingField("doc.title".into())),
            "auch ein fehlendes Feld im Ersatz"
        );
        assert_eq!(
            expand_with("{category}/", &[], Some(&r)),
            Err(TargetError::MissingField("category".into()))
        );
        assert_eq!(
            expand_with(
                "{doc.date:%Y}/",
                &[],
                Some(&record(None, &[("doc.date", "kaputt")]))
            ),
            Err(TargetError::MissingField("doc.date".into())),
            "unlesbares Datum zählt als fehlend"
        );
    }

    #[test]
    fn feldwerte_werden_bereinigt_und_gekuerzt() {
        let r = record(
            None,
            &[
                (
                    "doc.sender",
                    "Müller: Söhne/Töchter <GmbH> & Co. KG Süddeutschland Vertrieb",
                ),
                (
                    "doc.title",
                    "Ein sehr langer Titel für ein Dokument über viele Dinge die man lesen kann",
                ),
            ],
        );
        let e = expand_with("{doc.sender}/{doc.title}.{ext}", &[], Some(&r)).unwrap();
        assert!(e.dirs[0].chars().count() <= 40, "{:?}", e.dirs);
        assert!(!e.dirs[0].contains([':', '/', '<', '>']));
        let title = e.file.unwrap();
        assert!(title.chars().count() <= 60 + 4, "{title}");
        assert!(title.starts_with("Ein sehr langer Titel für ein Dokument"));
    }

    #[test]
    fn leerer_feldwert_zaehlt_als_fehlend() {
        let r = record(None, &[("doc.sender", "///")]);
        assert_eq!(
            expand_with("{doc.sender}/", &[], Some(&r)),
            Err(TargetError::MissingField("doc.sender".into()))
        );
        let e = expand_with("{doc.sender|Ersatz}/", &[], Some(&r)).unwrap();
        assert_eq!(e.dirs, ["Ersatz"]);
    }

    #[test]
    fn uses_content_und_exif_auch_im_ersatz() {
        assert!(Template::parse("{category}/", None).unwrap().uses_content());
        assert!(!Template::parse("{name}/", None).unwrap().uses_content());
        assert!(Template::parse("{exif.camera}/", None)
            .unwrap()
            .uses_content());
        let t = Template::parse("{doc.number|{exif.date}}/", None).unwrap();
        assert!(t.uses_exif() && t.uses_content());
    }

    #[rstest]
    #[case("{doc.bogus}/", "unbekannter Platzhalter")]
    #[case("{doc.sender:%Y}/", "nur bei Datumsfeldern")]
    #[case("{doc.date:%Q}/", "Datumsformat")]
    #[case("{doc.date:}/", "Format")]
    #[case("{exif.date|x}/", "exif_fallback")]
    #[case("{exif.date:%Y|x}/", "exif_fallback")]
    #[case("{name|x}/", "nur bei Feldern")]
    #[case("{year|x}/", "nur bei Feldern")]
    #[case("{doc.sender|}/", "nicht leer")]
    #[case("{doc.sender|a/b}/", "Pfadtrenner")]
    #[case("{doc.sender|{doc.title|x}}/", "weiteren Ersatz")]
    #[case("{doc.sender|a|b}/", "weiteren Ersatz")]
    #[case("{doc.sender|{foo}}/", "unbekannter Platzhalter")]
    #[case("{doc.sender|{1}}/", "name_regex")]
    #[case("{doc.sender|abc/", "ohne")]
    #[case("{category:%Y}/", "nur bei Datumsfeldern")]
    fn ungueltige_feld_platzhalter(#[case] text: &str, #[case] hint: &str) {
        let err = Template::parse(text, None).unwrap_err();
        assert!(err.contains(hint), "{text}: {err}");
    }

    #[test]
    fn alle_bekannten_felder_sind_platzhalter() {
        for name in crate::content::FIELD_NAMES {
            let text = if DATE_FIELDS.contains(name) {
                format!("{{{name}:%Y}}/")
            } else {
                format!("{{{name}}}/")
            };
            assert!(Template::parse(&text, None).is_ok(), "{text}");
        }
    }
}
