//! Phase 2: Änderungsplan, Apply und Undo. Nichts wird hart gelöscht (außer `purge`).

pub mod apply;
pub mod archive;
pub mod dedupe;
pub mod empty_dirs;
pub mod fsops;
pub mod journal;
pub mod junk;
pub mod limits;
pub mod plan;
pub mod protect;
pub mod quarantine;
pub mod registry;
pub mod rules;
#[cfg(test)]
mod rules_tests;
#[cfg(test)]
mod test_support;
pub mod tree;
pub mod undo;
pub mod versions;

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

impl std::str::FromStr for RunId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Self::parse(s)
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

/// Anzahl Aktionen nach Typ; formuliert Rückfrage, Zusammenfassung und `runs`-Zeile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActionCounts {
    /// Dateien in der Quarantäne (`quarantine`).
    pub quarantined: usize,
    /// Ordner oder Dateien unter `_Archiv` (`move`).
    pub archived: usize,
    /// Entfernte leere Ordner (`remove-dir`).
    pub dirs_removed: usize,
    /// Einsortierte bzw. umbenannte Dateien (`move` in Plänen der Art `rules`).
    pub sorted: usize,
    /// Ganze Ordner in der Quarantäne (`quarantine` mit `is_dir`, Plan `dedupe-dirs`).
    pub dirs_quarantined: usize,
}

impl ActionCounts {
    pub fn count(&mut self, action: plan::ActionType, is_dir: bool) {
        match action {
            plan::ActionType::Quarantine if is_dir => self.dirs_quarantined += 1,
            plan::ActionType::Quarantine => self.quarantined += 1,
            plan::ActionType::Move => self.archived += 1,
            plan::ActionType::RemoveDir => self.dirs_removed += 1,
        }
    }

    /// Ein Move einer Regel (Einsortieren oder Umbenennen).
    pub fn count_sorted(&mut self) {
        self.sorted += 1;
    }

    pub fn from_plan(plan: &plan::Plan) -> Self {
        let mut counts = Self::default();
        for a in &plan.actions {
            if plan.kind == plan::PlanKind::Rules && a.action == plan::ActionType::Move {
                counts.count_sorted();
            } else {
                counts.count(a.action, a.is_dir);
            }
        }
        counts
    }

    pub fn total(&self) -> usize {
        self.quarantined + self.dirs_quarantined + self.archived + self.dirs_removed + self.sorted
    }

    fn join(parts: Vec<String>, empty: &str) -> String {
        if parts.is_empty() {
            empty.to_string()
        } else {
            parts.join(", ")
        }
    }

    /// Rückfrage vor `apply`: was geschehen wird.
    pub fn plan_text(&self) -> String {
        let mut parts = Vec::new();
        if self.quarantined > 0 {
            parts.push(format!(
                "{} Dateien in die Quarantäne verschieben",
                self.quarantined
            ));
        }
        if self.dirs_quarantined > 0 {
            parts.push(format!(
                "{} Ordner in die Quarantäne verschieben",
                self.dirs_quarantined
            ));
        }
        if self.archived > 0 {
            parts.push(format!(
                "{} Elemente nach _Archiv verschieben",
                self.archived
            ));
        }
        if self.dirs_removed > 0 {
            parts.push(format!("{} leere Ordner entfernen", self.dirs_removed));
        }
        if self.sorted > 0 {
            parts.push(format!("{} Dateien einsortieren/umbenennen", self.sorted));
        }
        Self::join(parts, "nichts tun")
    }

    /// Zusammenfassung nach `apply` und vor `undo`: was geschehen ist.
    pub fn done_text(&self) -> String {
        let mut parts = Vec::new();
        if self.quarantined > 0 {
            parts.push(format!("{} in die Quarantäne verschoben", self.quarantined));
        }
        if self.dirs_quarantined > 0 {
            parts.push(format!(
                "{} Ordner in die Quarantäne verschoben",
                self.dirs_quarantined
            ));
        }
        if self.archived > 0 {
            parts.push(format!("{} nach _Archiv verschoben", self.archived));
        }
        if self.dirs_removed > 0 {
            parts.push(format!("{} leere Ordner entfernt", self.dirs_removed));
        }
        if self.sorted > 0 {
            parts.push(format!("{} Dateien einsortiert/umbenannt", self.sorted));
        }
        Self::join(parts, "nichts ausgeführt")
    }

    /// Kurzform für `runs`; Bytes gibt es nur für Quarantäne-Aktionen.
    pub fn short_text(&self, quarantine_bytes: u64) -> String {
        let mut parts = Vec::new();
        if self.quarantined > 0 {
            parts.push(format!(
                "{} Quarantäne ({})",
                self.quarantined,
                bytesize::ByteSize::b(quarantine_bytes)
            ));
        }
        if self.dirs_quarantined > 0 {
            parts.push(format!("{} Ordner (Quarantäne)", self.dirs_quarantined));
        }
        if self.archived > 0 {
            parts.push(format!("{} Archiv", self.archived));
        }
        if self.dirs_removed > 0 {
            parts.push(format!("{} Ordner", self.dirs_removed));
        }
        if self.sorted > 0 {
            parts.push(format!("{} einsortiert", self.sorted));
        }
        Self::join(parts, "keine Aktionen")
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
    /// Datei hat sich seit dem Plan geändert (Größe, Zeit oder Hash).
    Stale,
    /// Behaltene Datei fehlt, ist ein Platzhalter/Link oder nicht lesbar.
    KeepMissing,
    /// Quelle existiert nicht mehr: nichts zu tun.
    AlreadyDone,
    /// Quarantäne läge auf einem anderen Volume als die Quelle.
    DifferentVolume,
    OutsideRoot,
    /// Ordner enthält Einträge, die der Index nicht (vollständig) kennt.
    IncompleteIndex,
    /// Jünger als das Mindestalter (`junk`-Installer, `versions`).
    TooRecent,
    /// Archiv-Ziel existiert bereits (nichts wird überschrieben).
    TargetExists,
    /// `remove-dir`: Ordner ist (inzwischen) nicht leer.
    NotEmpty,
    /// Liegt bereits unter `_Archiv`.
    InArchive,
    /// `rules`: kein (lesbares) EXIF-Datum und `exif_fallback = "skip"`.
    NoExifDate,
    /// `rules`: Am Ziel liegt schon eine Datei mit identischem Inhalt.
    DuplicateAtTarget,
    /// `rules`: Das Ziel enthält ungültige Zeichen oder leere Namen.
    InvalidTarget,
    /// `rules`: Das Ziel wäre länger als 260 Zeichen.
    PathTooLong,
    /// `rules`: Die Kategorie ist zu unsicher (Konfidenz unter der Schwelle der Regel).
    LowConfidence,
    /// `rules`: Die Datei ist (noch) nicht klassifiziert, z. B. mit `--no-classify`.
    NotClassified,
    /// `rules`: Ein Feld im Ziel fehlt und hat keinen Ersatzwert (`detail` nennt es).
    MissingField,
    /// `rules`: `min_dwell` ist noch nicht erreicht (Zeit seit Ankunft im Ordner).
    TooRecentArrival,
    /// `dedupe-dirs`: Der behaltene Ordner hat sich seit dem Plan geändert.
    KeepChanged,
    /// `dedupe-dirs`: Inhalt nicht prüfbar (Cloud-only oder ohne Hash).
    Unverifiable,
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
            Self::Stale => "seit dem Plan geändert (stale)",
            Self::KeepMissing => "behaltene Datei fehlt oder ist nicht nutzbar",
            Self::AlreadyDone => "bereits erledigt",
            Self::DifferentVolume => "Quarantäne läge auf einem anderen Volume",
            Self::OutsideRoot => "liegt nicht in der Wurzel",
            Self::IncompleteIndex => "Index unvollständig für diesen Ordner",
            Self::TooRecent => "zu neu (Mindestalter nicht erreicht)",
            Self::TargetExists => "Ziel existiert bereits",
            Self::NotEmpty => "Ordner nicht leer",
            Self::InArchive => "bereits im Archiv",
            Self::NoExifDate => "kein EXIF-Aufnahmedatum",
            Self::DuplicateAtTarget => "am Ziel liegt bereits eine Datei mit identischem Inhalt",
            Self::InvalidTarget => "Ziel ungültig (Zeichen, reservierter Name oder leer)",
            Self::PathTooLong => "Ziel länger als 260 Zeichen",
            Self::LowConfidence => "Kategorie zu unsicher (zum Prüfen)",
            Self::NotClassified => "Inhalt noch nicht klassifiziert",
            Self::MissingField => "Feld für das Ziel fehlt",
            Self::TooRecentArrival => "zu kurz im Ordner (Wartezeit nicht erreicht)",
            Self::KeepChanged => "behaltener Ordner seit dem Plan geändert",
            Self::Unverifiable => "Inhalt nicht prüfbar (Cloud-only oder ohne Hash)",
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

    #[rstest]
    #[case(SkipReason::IncompleteIndex, "incomplete-index", "Index unvollständig")]
    #[case(SkipReason::TooRecent, "too-recent", "zu neu")]
    #[case(SkipReason::TargetExists, "target-exists", "Ziel existiert bereits")]
    #[case(SkipReason::NotEmpty, "not-empty", "Ordner nicht leer")]
    #[case(SkipReason::InArchive, "in-archive", "bereits im Archiv")]
    fn neue_skip_reasons_phase_3(
        #[case] reason: SkipReason,
        #[case] json: &str,
        #[case] text: &str,
    ) {
        assert_eq!(
            serde_json::to_string(&reason).unwrap(),
            format!("\"{json}\"")
        );
        assert!(reason.to_string().contains(text), "{reason}");
    }

    fn counts(quarantined: usize, archived: usize, dirs_removed: usize) -> ActionCounts {
        ActionCounts {
            quarantined,
            archived,
            dirs_removed,
            sorted: 0,
            dirs_quarantined: 0,
        }
    }

    #[test]
    fn ordner_quarantaene_hat_eigene_zaehlung_und_texte() {
        let mut c = ActionCounts::default();
        c.count(plan::ActionType::Quarantine, true);
        c.count(plan::ActionType::Quarantine, true);
        c.count(plan::ActionType::Quarantine, false);
        c.count(plan::ActionType::RemoveDir, true);
        assert_eq!(
            (c.dirs_quarantined, c.quarantined, c.dirs_removed),
            (2, 1, 1)
        );
        assert_eq!(c.total(), 4);
        let only_dirs = ActionCounts {
            dirs_quarantined: 2,
            ..ActionCounts::default()
        };
        assert_eq!(
            only_dirs.plan_text(),
            "2 Ordner in die Quarantäne verschieben"
        );
        assert_eq!(
            only_dirs.done_text(),
            "2 Ordner in die Quarantäne verschoben"
        );
        assert_eq!(only_dirs.short_text(0), "2 Ordner (Quarantäne)");
    }

    #[rstest]
    #[case(SkipReason::KeepChanged, "keep-changed", "behaltener Ordner")]
    #[case(SkipReason::Unverifiable, "unverifiable", "nicht prüfbar")]
    fn skip_reasons_dedupe_dirs(
        #[case] reason: SkipReason,
        #[case] json: &str,
        #[case] text: &str,
    ) {
        assert_eq!(
            serde_json::to_string(&reason).unwrap(),
            format!("\"{json}\"")
        );
        assert!(reason.to_string().contains(text), "{reason}");
    }

    #[rstest]
    #[case(SkipReason::NoExifDate, "no-exif-date", "EXIF")]
    #[case(
        SkipReason::DuplicateAtTarget,
        "duplicate-at-target",
        "identischem Inhalt"
    )]
    #[case(SkipReason::InvalidTarget, "invalid-target", "ungültig")]
    #[case(SkipReason::PathTooLong, "path-too-long", "260")]
    fn neue_skip_reasons_phase_4(
        #[case] reason: SkipReason,
        #[case] json: &str,
        #[case] text: &str,
    ) {
        assert_eq!(
            serde_json::to_string(&reason).unwrap(),
            format!("\"{json}\"")
        );
        assert!(reason.to_string().contains(text), "{reason}");
    }

    #[test]
    fn einsortierte_dateien_haben_eigene_texte() {
        let c = ActionCounts {
            sorted: 3,
            ..ActionCounts::default()
        };
        assert_eq!(c.total(), 3);
        assert_eq!(c.plan_text(), "3 Dateien einsortieren/umbenennen");
        assert_eq!(c.done_text(), "3 Dateien einsortiert/umbenannt");
        assert_eq!(c.short_text(0), "3 einsortiert");
        let mut counted = ActionCounts::default();
        counted.count_sorted();
        assert_eq!(counted.sorted, 1);
    }

    #[test]
    fn moves_eines_regel_plans_zaehlen_als_einsortiert() {
        use crate::change::plan::{ActionType, Plan, PlanKind, PlannedAction, PLAN_VERSION};
        let plan = Plan {
            version: PLAN_VERSION,
            created: String::new(),
            kind: PlanKind::Rules,
            root: r"D:\Daten".into(),
            keep_strategy: None,
            params: Default::default(),
            protected_paths: Vec::new(),
            actions: vec![PlannedAction {
                id: 1,
                action: ActionType::Move,
                path: r"D:\Daten\a".into(),
                size: 0,
                mtime_ticks: 0,
                mtime: String::new(),
                hash: None,
                keep: None,
                keep_hash: None,
                reason: String::new(),
                target: Some(r"D:\Daten\F\a".into()),
                is_dir: false,
                keep_fingerprint: None,
                files: None,
                rule: Some("r".into()),
            }],
            skipped: vec![],
        };
        let c = ActionCounts::from_plan(&plan);
        assert_eq!((c.sorted, c.archived), (1, 0));
    }

    #[test]
    fn aktionen_werden_nach_typ_gezaehlt() {
        let mut c = ActionCounts::default();
        c.count(plan::ActionType::Quarantine, false);
        c.count(plan::ActionType::Quarantine, false);
        c.count(plan::ActionType::Move, false);
        c.count(plan::ActionType::RemoveDir, true);
        assert_eq!(c, counts(2, 1, 1));
        assert_eq!(c.total(), 4);
        assert_eq!(ActionCounts::default().total(), 0);
    }

    #[rstest]
    #[case(counts(3, 0, 0), "3 Dateien in die Quarantäne verschieben")]
    #[case(counts(0, 2, 0), "2 Elemente nach _Archiv verschieben")]
    #[case(counts(0, 0, 4), "4 leere Ordner entfernen")]
    #[case(
        counts(3, 2, 4),
        "3 Dateien in die Quarantäne verschieben, 2 Elemente nach _Archiv verschieben, 4 leere Ordner entfernen"
    )]
    #[case(counts(0, 0, 0), "nichts tun")]
    fn plan_text_beschreibt_was_apply_tun_wird(#[case] c: ActionCounts, #[case] text: &str) {
        assert_eq!(c.plan_text(), text);
    }

    #[rstest]
    #[case(counts(2, 0, 0), "2 in die Quarantäne verschoben")]
    #[case(counts(0, 1, 0), "1 nach _Archiv verschoben")]
    #[case(counts(0, 0, 3), "3 leere Ordner entfernt")]
    #[case(
        counts(2, 1, 3),
        "2 in die Quarantäne verschoben, 1 nach _Archiv verschoben, 3 leere Ordner entfernt"
    )]
    #[case(counts(0, 0, 0), "nichts ausgeführt")]
    fn done_text_beschreibt_das_ergebnis(#[case] c: ActionCounts, #[case] text: &str) {
        assert_eq!(c.done_text(), text);
    }

    #[test]
    fn short_text_nennt_quarantaene_bytes_nur_fuer_quarantaene_aktionen() {
        assert_eq!(
            counts(2, 0, 0).short_text(30),
            "2 Quarantäne (30 B)",
            "Quarantäne mit Bytes"
        );
        assert_eq!(counts(0, 1, 0).short_text(0), "1 Archiv");
        assert_eq!(counts(0, 0, 4).short_text(0), "4 Ordner");
        assert_eq!(
            counts(1, 2, 3).short_text(2048),
            "1 Quarantäne (2.0 KiB), 2 Archiv, 3 Ordner"
        );
        assert_eq!(counts(0, 0, 0).short_text(0), "keine Aktionen");
    }

    #[test]
    fn zaehler_aus_dem_plan() {
        use crate::change::plan::{ActionType, Plan, PlanKind, PlannedAction, PLAN_VERSION};
        let action = |id, action| PlannedAction {
            id,
            action,
            path: format!(r"D:\Daten\x{id}"),
            size: 0,
            mtime_ticks: 0,
            mtime: String::new(),
            hash: None,
            keep: None,
            keep_hash: None,
            reason: String::new(),
            target: None,
            is_dir: false,
            keep_fingerprint: None,
            files: None,
            rule: None,
        };
        let plan = Plan {
            version: PLAN_VERSION,
            created: String::new(),
            kind: PlanKind::Junk,
            root: r"D:\Daten".into(),
            keep_strategy: None,
            params: Default::default(),
            protected_paths: Vec::new(),
            actions: vec![
                action(1, ActionType::Quarantine),
                action(2, ActionType::Move),
                action(3, ActionType::Move),
                action(4, ActionType::RemoveDir),
            ],
            skipped: vec![],
        };
        assert_eq!(ActionCounts::from_plan(&plan), counts(1, 2, 1));
    }

    #[test]
    fn skip_reason_serialisiert_kebab_case() {
        assert_eq!(
            serde_json::to_string(&SkipReason::CloudPlaceholder).unwrap(),
            "\"cloud-placeholder\""
        );
    }
}
