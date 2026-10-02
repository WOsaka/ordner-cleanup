//! Strukturprobleme: leere Ordner, Ein-Datei-Ordner, tiefe Verschachtelung, Riesenordner.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StructureIssue {
    Empty,
    SingleFile,
    TooDeep,
    Huge,
}

impl StructureIssue {
    pub fn key(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::SingleFile => "single_file",
            Self::TooDeep => "too_deep",
            Self::Huge => "huge",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Empty => "Leerer Ordner",
            Self::SingleFile => "Ordner mit nur einer Datei",
            Self::TooDeep => "Tief verschachtelt",
            Self::Huge => "Riesenordner",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// Tiefe relativ zur Wurzel; größer als dieser Wert ist zu tief.
    pub max_depth: usize,
    /// Mehr direkte Einträge als dieser Wert ist ein Riesenordner.
    pub huge_entries: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct DirFacts {
    /// Tiefe relativ zur angefragten Wurzel (Wurzel = 0).
    pub rel_depth: usize,
    /// Direkte Kinder (Dateien und Ordner).
    pub direct_entries: usize,
    /// Davon Dateien.
    pub direct_files: usize,
    /// Link oder „Nur Summe“: wird nicht bewertet.
    pub skipped: bool,
}

pub fn check_dir(facts: &DirFacts, t: &Thresholds) -> Vec<StructureIssue> {
    if facts.skipped {
        return vec![];
    }
    let mut out = Vec::new();
    if facts.direct_entries == 0 {
        out.push(StructureIssue::Empty);
    }
    if facts.direct_entries == 1 && facts.direct_files == 1 {
        out.push(StructureIssue::SingleFile);
    }
    if facts.rel_depth > t.max_depth {
        out.push(StructureIssue::TooDeep);
    }
    if facts.direct_entries > t.huge_entries {
        out.push(StructureIssue::Huge);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const T: Thresholds = Thresholds {
        max_depth: 8,
        huge_entries: 1000,
    };

    fn facts(rel_depth: usize, entries: usize, files: usize) -> DirFacts {
        DirFacts {
            rel_depth,
            direct_entries: entries,
            direct_files: files,
            skipped: false,
        }
    }

    #[rstest]
    #[case(facts(1, 0, 0), vec![StructureIssue::Empty])]
    #[case(facts(1, 1, 1), vec![StructureIssue::SingleFile])]
    #[case(facts(1, 1, 0), vec![])]
    #[case(facts(1, 2, 2), vec![])]
    #[case(facts(8, 5, 3), vec![])]
    #[case(facts(9, 5, 3), vec![StructureIssue::TooDeep])]
    #[case(facts(1, 1000, 900), vec![])]
    #[case(facts(1, 1001, 900), vec![StructureIssue::Huge])]
    #[case(facts(10, 0, 0), vec![StructureIssue::Empty, StructureIssue::TooDeep])]
    #[case(facts(0, 0, 0), vec![StructureIssue::Empty])]
    fn bewertung(#[case] f: DirFacts, #[case] expected: Vec<StructureIssue>) {
        assert_eq!(check_dir(&f, &T), expected);
    }

    #[test]
    fn links_und_summenordner_werden_uebersprungen() {
        let mut f = facts(20, 0, 0);
        f.skipped = true;
        assert!(check_dir(&f, &T).is_empty());
    }

    #[test]
    fn schwellwerte_sind_konfigurierbar() {
        let t = Thresholds {
            max_depth: 2,
            huge_entries: 5,
        };
        assert_eq!(
            check_dir(&facts(3, 6, 6), &t),
            vec![StructureIssue::TooDeep, StructureIssue::Huge]
        );
    }
}
