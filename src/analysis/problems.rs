//! Problemprüfungen für Datei- und Ordnernamen (OneDrive/SharePoint-Regeln u. a.).

pub const MAX_PATH_CHARS: usize = 260;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Problem {
    PathTooLong,
    InvalidChars,
    ReservedName,
    LeadingTrailingSpace,
    TrailingDot,
    ZeroByte,
    OfficeLock,
    TempFile,
    PartialDownload,
    OneDriveConflict,
    SystemJunk,
}

impl Problem {
    /// Stabiler Schlüssel für JSON/CSV.
    pub fn key(self) -> &'static str {
        match self {
            Self::PathTooLong => "path_too_long",
            Self::InvalidChars => "invalid_chars",
            Self::ReservedName => "reserved_name",
            Self::LeadingTrailingSpace => "leading_trailing_space",
            Self::TrailingDot => "trailing_dot",
            Self::ZeroByte => "zero_byte",
            Self::OfficeLock => "office_lock",
            Self::TempFile => "temp_file",
            Self::PartialDownload => "partial_download",
            Self::OneDriveConflict => "onedrive_conflict",
            Self::SystemJunk => "system_junk",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PathTooLong => "Pfad länger als 260 Zeichen",
            Self::InvalidChars => "Ungültige Zeichen im Namen",
            Self::ReservedName => "Reservierter Name",
            Self::LeadingTrailingSpace => "Führendes/abschließendes Leerzeichen",
            Self::TrailingDot => "Abschließender Punkt",
            Self::ZeroByte => "0-Byte-Datei",
            Self::OfficeLock => "Office-Lockdatei",
            Self::TempFile => "Temp-Datei",
            Self::PartialDownload => "Abgebrochener Download",
            Self::OneDriveConflict => "OneDrive-Konfliktdatei",
            Self::SystemJunk => "Systemmüll",
        }
    }
}

/// Rechnernamen, die in OneDrive-Konfliktdateien vorkommen können (kleingeschrieben).
#[derive(Debug, Clone, Default)]
pub struct ProblemCtx {
    hostnames: Vec<String>,
}

impl ProblemCtx {
    pub fn with_hosts<S: AsRef<str>>(hosts: &[S]) -> Self {
        Self {
            hostnames: hosts
                .iter()
                .map(|h| h.as_ref().trim().to_lowercase())
                .filter(|h| !h.is_empty())
                .collect(),
        }
    }

    /// `%COMPUTERNAME%` plus zusätzliche Namen aus der Config.
    pub fn from_env(extra: &[String]) -> Self {
        let mut hosts: Vec<String> = extra.to_vec();
        if let Ok(name) = std::env::var("COMPUTERNAME") {
            hosts.push(name);
        }
        Self::with_hosts(&hosts)
    }
}

const INVALID_CHARS: [char; 9] = ['"', '*', ':', '<', '>', '?', '/', '\\', '|'];
const RESERVED: [&str; 4] = ["con", "prn", "aux", "nul"];

fn is_reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        return true;
    }
    ["com", "lpt"].iter().any(|p| {
        stem.strip_prefix(p)
            .is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
    })
}

/// Erstes Namensproblem, das einen Namen unter Windows/OneDrive unbrauchbar macht (ungültige
/// Zeichen, reservierter Name, Leerzeichen oder Punkt am Rand); `None`, wenn der Name taugt.
pub fn name_issue(name: &str) -> Option<Problem> {
    let mut found = Vec::new();
    name_problems(name, "", &mut found);
    found.into_iter().next()
}

fn name_problems(name: &str, path: &str, out: &mut Vec<Problem>) {
    if path.chars().count() > MAX_PATH_CHARS {
        out.push(Problem::PathTooLong);
    }
    if name
        .chars()
        .any(|c| INVALID_CHARS.contains(&c) || c.is_control())
    {
        out.push(Problem::InvalidChars);
    }
    if is_reserved(name) {
        out.push(Problem::ReservedName);
    }
    if name.starts_with(' ') || name.ends_with(' ') {
        out.push(Problem::LeadingTrailingSpace);
    }
    if name.ends_with('.') {
        out.push(Problem::TrailingDot);
    }
}

fn split_stem(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}

fn is_conflict(stem: &str, hosts: &[String]) -> bool {
    let stem = stem.to_lowercase();
    // `<name>-<PC>` oder `<name>-<PC>-<n>`
    let base = match stem.rsplit_once('-') {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => {
            [stem.as_str(), head]
        }
        _ => [stem.as_str(), stem.as_str()],
    };
    hosts.iter().any(|h| {
        let suffix = format!("-{h}");
        base.iter()
            .any(|s| s.len() > suffix.len() && s.ends_with(&suffix))
    })
}

pub fn check_file(name: &str, path: &str, size: u64, ctx: &ProblemCtx) -> Vec<Problem> {
    let mut out = Vec::new();
    name_problems(name, path, &mut out);
    if size == 0 {
        out.push(Problem::ZeroByte);
    }
    let lower = name.to_lowercase();
    if name.starts_with("~$") {
        out.push(Problem::OfficeLock);
    }
    if lower.ends_with(".tmp") {
        out.push(Problem::TempFile);
    }
    if [".crdownload", ".part", ".partial"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        out.push(Problem::PartialDownload);
    }
    if ["thumbs.db", "desktop.ini", ".ds_store"].contains(&lower.as_str()) {
        out.push(Problem::SystemJunk);
    }
    let (stem, ext) = split_stem(name);
    let conflict_stem = if ext.is_empty() { name } else { stem };
    if is_conflict(conflict_stem, &ctx.hostnames) {
        out.push(Problem::OneDriveConflict);
    }
    out
}

pub fn check_dir(name: &str, path: &str) -> Vec<Problem> {
    let mut out = Vec::new();
    name_problems(name, path, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_issue_meldet_das_erste_problem_oder_nichts() {
        assert_eq!(name_issue("Rechnung 2026.pdf"), None);
        assert_eq!(name_issue("a<b.txt"), Some(Problem::InvalidChars));
        assert_eq!(name_issue("tab	name"), Some(Problem::InvalidChars));
        assert_eq!(name_issue("CON.txt"), Some(Problem::ReservedName));
        assert_eq!(name_issue("name."), Some(Problem::TrailingDot));
        assert_eq!(name_issue(" name"), Some(Problem::LeadingTrailingSpace));
        assert_eq!(name_issue(""), None, "leere Namen prüft der Aufrufer");
        let long = "x".repeat(300);
        assert_eq!(name_issue(&long), None, "Pfadlänge ist kein Namensproblem");
    }
    use rstest::rstest;

    fn ctx() -> ProblemCtx {
        ProblemCtx::with_hosts(&["DESKTOP-ABC123", "Laptop"])
    }

    fn file(name: &str) -> Vec<Problem> {
        check_file(name, &format!(r"C:\x\{name}"), 10, &ctx())
    }

    #[rstest]
    #[case("bericht.docx")]
    #[case("Notizen 2024.txt")]
    #[case("a.b.c")]
    #[case("console.log")]
    #[case("com10.txt")]
    #[case("com.txt")]
    #[case("lpt0.txt")]
    #[case("Bericht-Final.docx")]
    #[case("Bericht-DESKTOP.docx")]
    fn unauffaellige_namen(#[case] name: &str) {
        assert_eq!(file(name), vec![], "{name}");
    }

    #[rstest]
    #[case("a\"b.txt")]
    #[case("a*b.txt")]
    #[case("a:b.txt")]
    #[case("a<b.txt")]
    #[case("a>b.txt")]
    #[case("a?b.txt")]
    #[case("a/b.txt")]
    #[case("a\\b.txt")]
    #[case("a|b.txt")]
    #[case("a\u{1}b.txt")]
    fn ungueltige_zeichen(#[case] name: &str) {
        assert!(file(name).contains(&Problem::InvalidChars), "{name:?}");
    }

    #[rstest]
    #[case("CON")]
    #[case("con.txt")]
    #[case("NUL.tar.gz")]
    #[case("Prn")]
    #[case("AUX.doc")]
    #[case("COM1")]
    #[case("com9.txt")]
    #[case("LPT1.log")]
    fn reservierte_namen(#[case] name: &str) {
        assert!(file(name).contains(&Problem::ReservedName), "{name}");
    }

    #[test]
    fn leerzeichen_und_punkt() {
        assert!(file(" a.txt").contains(&Problem::LeadingTrailingSpace));
        assert!(file("a.txt ").contains(&Problem::LeadingTrailingSpace));
        assert!(file("a.").contains(&Problem::TrailingDot));
        assert!(!file("a b.txt").contains(&Problem::LeadingTrailingSpace));
    }

    #[rstest]
    #[case(259, false)]
    #[case(260, false)]
    #[case(261, true)]
    fn pfadlaenge(#[case] len: usize, #[case] too_long: bool) {
        let path = format!(r"C:\{}", "a".repeat(len - 3));
        assert_eq!(path.chars().count(), len);
        let problems = check_file("a", &path, 1, &ctx());
        assert_eq!(problems.contains(&Problem::PathTooLong), too_long);
    }

    #[test]
    fn pfadlaenge_zaehlt_zeichen_nicht_bytes() {
        let path = format!(r"C:\{}", "ä".repeat(257));
        assert!(!check_file("a", &path, 1, &ctx()).contains(&Problem::PathTooLong));
    }

    #[test]
    fn null_byte_dateien() {
        assert!(check_file("a.txt", r"C:\a.txt", 0, &ctx()).contains(&Problem::ZeroByte));
        assert!(!check_file("a.txt", r"C:\a.txt", 1, &ctx()).contains(&Problem::ZeroByte));
    }

    #[rstest]
    #[case("~$bericht.docx", Problem::OfficeLock)]
    #[case("x.tmp", Problem::TempFile)]
    #[case("~wrd0001.TMP", Problem::TempFile)]
    #[case("film.mp4.crdownload", Problem::PartialDownload)]
    #[case("film.part", Problem::PartialDownload)]
    #[case("film.PARTIAL", Problem::PartialDownload)]
    #[case("Thumbs.db", Problem::SystemJunk)]
    #[case("desktop.ini", Problem::SystemJunk)]
    #[case(".DS_Store", Problem::SystemJunk)]
    fn muster(#[case] name: &str, #[case] expected: Problem) {
        assert!(file(name).contains(&expected), "{name}");
    }

    #[rstest]
    #[case("Bericht-DESKTOP-ABC123.docx", true)]
    #[case("Bericht-DESKTOP-ABC123-2.docx", true)]
    #[case("Bericht-desktop-abc123-17.docx", true)]
    #[case("Bericht-Laptop.xlsx", true)]
    #[case("Bericht-Laptop", true)]
    #[case("Bericht-Anderer-PC.docx", false)]
    #[case("-Laptop.docx", false)]
    #[case("Laptop.docx", false)]
    #[case("Bericht-Laptop-x.docx", false)]
    #[case("Bericht-2.docx", false)]
    fn onedrive_konflikte(#[case] name: &str, #[case] conflict: bool) {
        assert_eq!(
            file(name).contains(&Problem::OneDriveConflict),
            conflict,
            "{name}"
        );
    }

    #[test]
    fn konflikt_ohne_bekannte_hosts_wird_nicht_erkannt() {
        let none = ProblemCtx::with_hosts::<&str>(&[]);
        assert!(!check_file("a-PC.docx", r"C:\a-PC.docx", 1, &none)
            .contains(&Problem::OneDriveConflict));
    }

    #[test]
    fn mehrere_probleme_gleichzeitig() {
        let p = file("~$a:b.tmp");
        assert!(p.contains(&Problem::OfficeLock));
        assert!(p.contains(&Problem::TempFile));
        assert!(p.contains(&Problem::InvalidChars));
    }

    #[test]
    fn ordner_pruefen_nur_namen() {
        assert_eq!(check_dir("Rechnungen", r"C:\Rechnungen"), vec![]);
        assert!(check_dir("a?", r"C:\a?").contains(&Problem::InvalidChars));
        assert!(check_dir("nul", r"C:\nul").contains(&Problem::ReservedName));
        assert!(!check_dir("x.tmp", r"C:\x.tmp").contains(&Problem::TempFile));
    }

    #[test]
    fn schluessel_und_labels_eindeutig() {
        let all = [
            Problem::PathTooLong,
            Problem::InvalidChars,
            Problem::ReservedName,
            Problem::LeadingTrailingSpace,
            Problem::TrailingDot,
            Problem::ZeroByte,
            Problem::OfficeLock,
            Problem::TempFile,
            Problem::PartialDownload,
            Problem::OneDriveConflict,
            Problem::SystemJunk,
        ];
        let keys: std::collections::HashSet<_> = all.iter().map(|p| p.key()).collect();
        let labels: std::collections::HashSet<_> = all.iter().map(|p| p.label()).collect();
        assert_eq!((keys.len(), labels.len()), (11, 11));
    }
}
