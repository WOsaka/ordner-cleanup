//! Hilfetexte der Oberfläche: je Seite, Aufräum-Karte und Review-Ansicht ein Kurztext in einfachem
//! Deutsch plus ein Verweis auf die ausführliche Doku. Reine Daten ohne Fenster; die Texte sind in
//! die exe eingebaut und brauchen keine Datei. Nur die ausführliche Doku liegt als Datei neben der
//! exe.

use std::path::{Path, PathBuf};

use super::shell::Page;

/// Wozu die Hilfe gerade gezeigt wird.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    Page(Page),
    /// Eine Karte der Aufräumen-Seite, Schlüssel wie in `cleanup_cards::CARDS`
    Card(&'static str),
    /// Die Review-Ansicht eines geöffneten Plans
    Review,
    /// Die Liste der Tastenkürzel
    Keys,
}

/// Die ausführlichen Dokumente, die neben der exe liegen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocFile {
    Dokumentation,
    Readme,
}

impl DocFile {
    /// Pfad relativ zum Ordner der exe (bzw. zur Repo-Wurzel bei Entwicklerläufen).
    pub fn relative(self) -> &'static str {
        match self {
            DocFile::Dokumentation => "docs/dokumentation.md",
            DocFile::Readme => "README.md",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DocFile::Dokumentation => "Gesamtdokumentation",
            DocFile::Readme => "README",
        }
    }
}

/// Verweis auf einen Abschnitt: Datei und der Überschriftentext ohne die führenden `#`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocRef {
    pub file: DocFile,
    pub heading: &'static str,
}

#[derive(Debug)]
pub struct HelpEntry {
    pub title: &'static str,
    /// Was die Funktion tut
    pub what: &'static str,
    /// Was sich dabei ändert
    pub changes: &'static str,
    /// Wie man es rückgängig macht
    pub undo: &'static str,
    /// Wichtige Optionen: Name, Erklärung
    pub options: &'static [(&'static str, &'static str)],
    pub doc: Option<DocRef>,
}

const DOC_ANALYSE: DocRef = DocRef {
    file: DocFile::Dokumentation,
    heading: "5. Analyse: scan und report",
};
const DOC_AUFRAEUMEN: DocRef = DocRef {
    file: DocFile::Dokumentation,
    heading: "6. Aufräumen: Plan, Apply, Undo",
};
const DOC_AKTIONEN: DocRef = DocRef {
    file: DocFile::Dokumentation,
    heading: "7. Die Aufräumaktionen im Einzelnen",
};
const DOC_KONFIG: DocRef = DocRef {
    file: DocFile::Dokumentation,
    heading: "9. Konfiguration",
};
const DOC_REGELN: DocRef = DocRef {
    file: DocFile::Readme,
    heading: "Regeln: Einsortieren und Umbenennen (Phase 4)",
};
const DOC_GUI: DocRef = DocRef {
    file: DocFile::Readme,
    heading: "Grafische Oberfläche (Phase 6b)",
};

static OVERVIEW: HelpEntry = HelpEntry {
    title: "Übersicht",
    what: "Zeigt, welche Ordner schon gescannt wurden, wie gut sie geordnet sind (Score), die \
           letzten Läufe und ob noch Dateien in der Quarantäne auf das endgültige Löschen warten. \
           Von hier wählst du einen Ordner und scannst ihn.",
    changes: "Nichts. Ein Scan liest nur Namen, Größen und Zeiten; deine Dateien werden nicht \
              verändert oder gelöscht.",
    undo: "Nicht nötig.",
    options: &[
        (
            "Ordner wählen und scannen",
            "Ohne Scan gibt es keine Zahlen. Nach einem Scan sind Analyse und Aufräumen bereit.",
        ),
        (
            "Score",
            "Eine Zahl für den Zustand des Ordners; höher ist besser. Der Trend zeigt, ob er besser oder schlechter wurde.",
        ),
    ],
    doc: Some(DOC_GUI),
};

static ANALYSIS: HelpEntry = HelpEntry {
    title: "Analyse",
    what: "Zeigt, wo der Platz hingeht: die größten Ordner, Dateitypen, Alter, doppelte und \
           ähnliche Dateien, Problemdateien und den Score mit seinem Verlauf.",
    changes: "Nichts. Die Analyse liest nur das Ergebnis des letzten Scans.",
    undo: "Nicht nötig.",
    options: &[
        (
            "Exportieren",
            "Speichert den Bericht als HTML, JSON oder CSV, zum Beispiel zum Weitergeben.",
        ),
        (
            "Rechtsklick auf einen Pfad",
            "Im Explorer zeigen, Pfad kopieren oder die Datei öffnen (nicht bei Dateien, die nur in der Cloud liegen).",
        ),
    ],
    doc: Some(DOC_ANALYSE),
};

static CLEANUP: HelpEntry = HelpEntry {
    title: "Aufräumen",
    what: "Hier erzeugst du einen Plan fürs Aufräumen. Ein Plan ist nur eine Liste von \
           Vorschlägen. Erst wenn du ihn prüfst und mit „Anwenden“ bestätigst, passiert etwas.",
    changes: "Beim Erzeugen eines Plans nichts. Beim Anwenden gehen Dateien in die Quarantäne \
              oder an einen neuen Ort. Nichts wird sofort endgültig gelöscht.",
    undo: "Jeder angewendete Lauf lässt sich unter „Verlauf“ mit „Rückgängig“ zurückholen, \
           solange die Quarantäne nicht geleert wurde.",
    options: &[
        (
            "Karten",
            "Wähle eine Art von Aufräumen. Die Zahl auf der Karte stammt vom letzten Scan.",
        ),
        (
            "Plan öffnen",
            "Öffnet einen schon vorhandenen Plan, zum Beispiel von einem geplanten Lauf.",
        ),
    ],
    doc: Some(DOC_AUFRAEUMEN),
};

static HISTORY: HelpEntry = HelpEntry {
    title: "Verlauf",
    what: "Zeigt alle früheren Läufe mit Details. Hier machst du einen Lauf rückgängig und \
           leerst die Quarantäne.",
    changes: "„Rückgängig“ stellt die Dateien eines Laufs wieder her. „Quarantäne leeren“ löscht \
              Dateien endgültig.",
    undo: "„Rückgängig“ lässt sich nicht noch einmal umkehren, ist aber jederzeit wiederholbar \
           für andere Läufe. Das Leeren der Quarantäne ist endgültig; vorher gibt es eine \
           Vorschau und eine Rückfrage.",
    options: &[
        (
            "Läuft ab",
            "Bis zu diesem Tag bleiben Dateien in der Quarantäne und lassen sich zurückholen.",
        ),
        (
            "Inhalt ansehen",
            "Springt zur Seite „Quarantäne“ und zeigt die Dateien dieses Laufs, die noch dort liegen. Dort holst du einzelne zurück.",
        ),
        (
            "Gescannte Wurzeln im Index",
            "Entfernt gemerkte Ordner aus dem Index. Deine Dateien bleiben unberührt.",
        ),
    ],
    doc: Some(DOC_AUFRAEUMEN),
};

static QUARANTINE: HelpEntry = HelpEntry {
    title: "Quarantäne",
    what: "Zeigt, was nach dem Aufräumen in der Quarantäne liegt: wie viel, wie groß, woher und            bis wann. Einzelne Dateien oder Ordner holst du hier gezielt zurück, ohne den ganzen            Lauf rückgängig zu machen.",
    changes: "„Zurückholen“ verschiebt die gewählten Einträge an ihren alten Ort. Ist der Name               dort inzwischen belegt, bekommt die zurückgeholte Datei einen neuen Namen wie               „Bericht (2).docx“; vorhandene Dateien werden nie überschrieben.",
    undo: "Nicht nötig: Ein Lauf lässt sich danach weiter unter „Verlauf“ rückgängig machen,            die schon zurückgeholten Einträge werden dabei übersprungen. Wurde die Quarantäne            geleert, sind die Dateien nicht mehr da.",
    options: &[
        (
            "Läuft ab",
            "Nach diesem Tag darf die Quarantäne geleert werden. Bis dahin bleibt alles zurückholbar.",
        ),
        (
            "fehlt",
            "Die Datei liegt nicht mehr in der Quarantäne (zum Beispiel von Hand gelöscht).",
        ),
        (
            "nur online",
            "Die Datei liegt nur in der Cloud. Beim Zurückholen wird sie nicht heruntergeladen.",
        ),
    ],
    doc: Some(DOC_AUFRAEUMEN),
};

static SETTINGS: HelpEntry = HelpEntry {
    title: "Einstellungen",
    what: "Hier stellst du Regeln, Profile mit Zeitplänen, die Inhaltserkennung und das Aussehen \
           ein, ohne Dateien von Hand zu bearbeiten.",
    changes: "Nur die Einstellungsdateien des Programms. Deine eigenen Dateien bleiben \
              unberührt. Vor dem Speichern entsteht eine Sicherungskopie.",
    undo: "Die Sicherungskopie (.bak) neben der Einstellungsdatei stellt den vorherigen Stand \
           wieder her.",
    options: &[
        (
            "Regeln",
            "Legen fest, welche Dateien wohin sortiert oder wie umbenannt werden.",
        ),
        (
            "Profile & Zeitpläne",
            "Ein Profil merkt sich Ordner und Aufräumarten. Ein Zeitplan lässt Windows es regelmäßig prüfen; angewendet wird dabei nie etwas.",
        ),
    ],
    doc: Some(DOC_KONFIG),
};

static REVIEW: HelpEntry = HelpEntry {
    title: "Plan prüfen",
    what: "Hier siehst du den Plan Zeile für Zeile, bevor irgendetwas passiert. Ein Häkchen \
           heißt: Diese Zeile wird ausgeführt. Du kannst suchen, filtern, sortieren und \
           einzelne Zeilen abwählen.",
    changes: "Erst „Anwenden“ (mit Rückfrage) verändert etwas. Dateien gehen in die Quarantäne \
              oder werden verschoben, nie sofort gelöscht. Wählst du Zeilen ab, entsteht eine \
              neue Plan-Datei; der ursprüngliche Plan bleibt unverändert. Die Plan-Datei trägt \
              eine Prüfsumme: Wurde sie nach dem Erstellen verändert, wird sie nicht angewendet.",
    undo: "Unter „Verlauf“ mit „Rückgängig“.",
    options: &[
        (
            "Auswahl",
            "Alle, keine oder nur die gefilterten Zeilen an- oder abwählen.",
        ),
        (
            "Übersprungene zeigen",
            "Zeilen, die der Plan bewusst auslässt, mit dem Grund (zum Beispiel geschützt oder zu jung).",
        ),
        (
            "Anwenden",
            "Zeigt vorher Anzahl, Größe und Warnungen und führt erst nach deiner Bestätigung aus.",
        ),
    ],
    doc: Some(DOC_AUFRAEUMEN),
};

static KEYS: HelpEntry = HelpEntry {
    title: "Tastenkürzel",
    what: "Alle Tastenkürzel der Oberfläche. Kein Kürzel wendet etwas an oder löscht; „Anwenden“ \
           öffnet immer erst die Rückfrage.",
    changes: "Nichts.",
    undo: "Nicht nötig.",
    options: &[],
    doc: None,
};

static DEDUPE_DIRS: HelpEntry = HelpEntry {
    title: "Doppelte Ordner",
    what: "Findet ganze Ordner, die Kopien voneinander sind: dieselben Dateien mit demselben \
           Inhalt an denselben Pfaden. Der Name des Ordners spielt keine Rolle; Müll wie \
           Thumbs.db zählt nicht mit. Von jeder Gruppe bleibt ein Ordner liegen.",
    changes: "Die überzähligen Ordner werden als Ganzes in die Quarantäne verschoben, samt \
              Müll-Dateien darin. Welcher Ordner bleibt, entscheidet zuerst der Name \
              („Kopie“, „Backup“, „(2)“ … gehen zuerst), dann deine Wahl. Erst doppelte \
              Ordner bereinigen, dann einzelne Duplikate.",
    undo: "Unter „Verlauf“ mit „Rückgängig“, solange die Quarantäne nicht geleert wurde.",
    options: &[(
        "Welcher Ordner bleibt",
        "Bei gleichem Namens-Hinweis der älteste, der neueste oder der Ordner in einem Ordner, den du wählst.",
    )],
    doc: Some(DOC_AKTIONEN),
};

static DEDUPE: HelpEntry = HelpEntry {
    title: "Duplikate",
    what: "Findet Dateien mit genau gleichem Inhalt. Von jeder Gruppe bleibt eine Datei liegen, \
           die übrigen kommen in die Quarantäne.",
    changes: "Die überzähligen Kopien werden aus ihrem Ordner in die Quarantäne verschoben. \
              Mindestens eine Datei jeder Gruppe bleibt immer liegen.",
    undo: "Unter „Verlauf“ mit „Rückgängig“, solange die Quarantäne nicht geleert wurde.",
    options: &[(
        "Welche Kopie bleibt",
        "Die älteste, die neueste oder die Kopie in einem Ordner, den du wählst.",
    )],
    doc: Some(DOC_AKTIONEN),
};

static JUNK: HelpEntry = HelpEntry {
    title: "Müll",
    what: "Sucht Müll: temporäre Dateien (.tmp, ~$ …), Thumbs.db, abgebrochene Downloads und \
           alte Installer im Downloads-Ordner.",
    changes: "Die gefundenen Dateien werden in die Quarantäne verschoben.",
    undo: "Unter „Verlauf“ mit „Rückgängig“, solange die Quarantäne nicht geleert wurde.",
    options: &[(
        "Kategorien",
        "Welche Arten Müll gesucht werden: system, temp, downloads, installer oder eigene. Leer heißt: wie in den Einstellungen.",
    )],
    doc: Some(DOC_AKTIONEN),
};

static EMPTY_DIRS: HelpEntry = HelpEntry {
    title: "Leere Ordner",
    what: "Findet Ordner, in denen keine einzige Datei liegt, auch nicht in Unterordnern.",
    changes: "Die leeren Ordner werden entfernt. Ordner mit Dateien bleiben. Liegt nur Müll darin \
              (zum Beispiel Thumbs.db), zählt der Ordner nicht als leer: erst „Müll“ aufräumen, \
              dann neu scannen.",
    undo: "Unter „Verlauf“ mit „Rückgängig“; die Ordner werden wieder angelegt.",
    options: &[],
    doc: Some(DOC_AKTIONEN),
};

static ARCHIVE: HelpEntry = HelpEntry {
    title: "Archivieren",
    what: "Verschiebt Ordner, an denen sehr lange nichts geändert wurde, in einen Archivordner \
           innerhalb des gescannten Ordners.",
    changes: "Der ganze Ordner wird nach „_Archiv“ (nach Jahr sortiert) verschoben. Nichts wird \
              gelöscht oder gepackt.",
    undo: "Unter „Verlauf“ mit „Rückgängig“; der Ordner wandert zurück.",
    options: &[(
        "Älter als",
        "So lange darf die jüngste Datei im Ordner unverändert sein. Leer heißt: wie in den Einstellungen.",
    )],
    doc: Some(DOC_AKTIONEN),
};

static VERSIONS: HelpEntry = HelpEntry {
    title: "Versionen",
    what: "Findet Dateien, die Versionen voneinander sind (zum Beispiel „Bericht_final_v2“ oder \
           „Bericht - Kopie (3)“) und behält die jüngste.",
    changes: "Ältere Versionen werden nach „_Archiv\\Versionen“ verschoben. Die jüngste Datei \
              bleibt, wo sie ist.",
    undo: "Unter „Verlauf“ mit „Rückgängig“; die Dateien wandern zurück.",
    options: &[(
        "Mindestalter",
        "Ältere Versionen, die jünger als dieser Wert sind, bleiben liegen. Leer heißt: wie in den Einstellungen.",
    )],
    doc: Some(DOC_AKTIONEN),
};

static RULES: HelpEntry = HelpEntry {
    title: "Nach Regeln",
    what: "Sortiert und benennt Dateien nach deiner Regeldatei in eine neue Ordnerstruktur um, \
           zum Beispiel Fotos nach Jahr.",
    changes: "Dateien werden verschoben oder umbenannt, nie gelöscht. Pro Datei gilt die erste \
              Regel, die passt.",
    undo: "Unter „Verlauf“ mit „Rückgängig“; Namen und Orte werden zurückgesetzt.",
    options: &[
        (
            "Regeldatei",
            "Welche Datei die Regeln enthält. „Standard“ nimmt die Regeln aus den Einstellungen.",
        ),
        (
            "Diese Regeln anwenden",
            "Nur Regeln mit Häkchen gehen in den Plan.",
        ),
        (
            "Fehlende Inhalte nicht nachklassifizieren",
            "Spart Zeit, aber Dateien ohne erkannte Inhalte werden dann übersprungen.",
        ),
    ],
    doc: Some(DOC_REGELN),
};

/// Der Hilfetext zu einem Thema. Eine unbekannte Karte fällt auf die Seite „Aufräumen“ zurück.
pub fn entry(topic: Topic) -> &'static HelpEntry {
    match topic {
        Topic::Page(Page::Overview) => &OVERVIEW,
        Topic::Page(Page::Analysis) => &ANALYSIS,
        Topic::Page(Page::Cleanup) => &CLEANUP,
        Topic::Page(Page::History) => &HISTORY,
        Topic::Page(Page::Quarantine) => &QUARANTINE,
        Topic::Page(Page::Settings) => &SETTINGS,
        Topic::Review => &REVIEW,
        Topic::Keys => &KEYS,
        Topic::Card(key) => match key {
            "dedupe-dirs" => &DEDUPE_DIRS,
            "dedupe" => &DEDUPE,
            "junk" => &JUNK,
            "empty-dirs" => &EMPTY_DIRS,
            "archive" => &ARCHIVE,
            "versions" => &VERSIONS,
            "rules" => &RULES,
            _ => &CLEANUP,
        },
    }
}

/// Orte, an denen die Datei liegen kann: zuerst neben der exe, danach (Entwicklerläufe aus
/// `target\debug` oder `target\release`) in der Repo-Wurzel. Die Namen kommen nur aus dem Enum.
pub fn candidates(exe_dir: &Path, file: DocFile) -> Vec<PathBuf> {
    let mut bases = vec![exe_dir.to_path_buf()];
    if let Some(root) = exe_dir.parent().and_then(Path::parent) {
        bases.push(root.to_path_buf());
    }
    bases.into_iter().map(|b| b.join(file.relative())).collect()
}

/// Die erste vorhandene Datei aus [`candidates`].
pub fn find_in(exe_dir: &Path, file: DocFile) -> Option<PathBuf> {
    candidates(exe_dir, file).into_iter().find(|p| p.is_file())
}

/// Die Doku-Datei neben der laufenden exe, wenn es sie gibt.
pub fn doc_path(file: DocFile) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    find_in(exe.parent()?, file)
}

/// Wo die Datei liegen müsste; für die Meldung, wenn sie fehlt.
pub fn expected_path(file: DocFile) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(file.relative())))
        .unwrap_or_else(|| PathBuf::from(file.relative()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::views::cleanup_cards::CARDS;

    fn all_topics() -> Vec<Topic> {
        let mut topics: Vec<Topic> = Page::ALL.iter().map(|p| Topic::Page(*p)).collect();
        topics.extend(CARDS.iter().map(|c| Topic::Card(c.key)));
        topics.push(Topic::Review);
        topics.push(Topic::Keys);
        topics
    }

    fn repo_file(relative: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn jede_seite_und_karte_hat_einen_eigenen_text() {
        let mut titles = std::collections::HashSet::new();
        for topic in all_topics() {
            let e = entry(topic);
            // Der Rückfall für unbekannte Karten wäre ein doppelter Titel.
            assert!(
                titles.insert(e.title),
                "{topic:?} teilt den Titel {}",
                e.title
            );
        }
    }

    #[test]
    fn plan_pruefen_nennt_die_pruefsumme() {
        let changes = entry(Topic::Review).changes;
        assert!(changes.contains("Prüfsumme"), "{changes}");
        assert!(changes.contains("nicht angewendet"), "{changes}");
    }

    #[test]
    fn unbekannte_karte_faellt_auf_aufraeumen_zurueck() {
        assert_eq!(
            entry(Topic::Card("gibt-es-nicht")).title,
            entry(Topic::Page(Page::Cleanup)).title
        );
    }

    #[test]
    fn texte_sind_gefuellt_und_nicht_zu_lang() {
        for topic in all_topics() {
            let e = entry(topic);
            for (name, text) in [("what", e.what), ("changes", e.changes), ("undo", e.undo)] {
                assert!(!text.trim().is_empty(), "{topic:?}: {name} leer");
                assert!(text.chars().count() <= 700, "{topic:?}: {name} zu lang");
            }
            for (option, text) in e.options {
                assert!(
                    !option.is_empty() && !text.is_empty(),
                    "{topic:?}: Option leer"
                );
                assert!(text.chars().count() <= 700, "{topic:?}: {option} zu lang");
            }
        }
    }

    #[test]
    fn texte_enthalten_keine_befehlszeilen_begriffe() {
        let verboten = ["--", "cargo", ".exe", "Befehl", "Kommandozeile", "CLI"];
        for topic in all_topics() {
            let e = entry(topic);
            let mut texts = vec![e.title, e.what, e.changes, e.undo];
            for (option, text) in e.options {
                texts.push(option);
                texts.push(text);
            }
            for text in texts {
                for word in verboten {
                    assert!(!text.contains(word), "{topic:?}: „{word}“ in „{text}“");
                }
                // „plan“ als Befehl; das Hauptwort „Plan“ und „Zeitplan“ sind erlaubt
                assert!(
                    !text.split_whitespace().any(|w| w == "plan"),
                    "{topic:?}: „plan“ in „{text}“"
                );
            }
        }
    }

    #[test]
    fn jeder_doku_verweis_zeigt_auf_eine_vorhandene_ueberschrift() {
        for topic in all_topics() {
            let Some(doc) = entry(topic).doc else {
                continue;
            };
            let content = repo_file(doc.file.relative());
            let found = content.lines().any(|l| {
                l.trim_end()
                    .trim_start_matches('#')
                    .trim_start()
                    .eq(doc.heading)
                    && l.starts_with('#')
            });
            assert!(
                found,
                "{topic:?}: Überschrift „{}“ fehlt in {}",
                doc.heading,
                doc.file.relative()
            );
        }
    }

    #[test]
    fn doku_wird_neben_der_exe_und_in_der_repo_wurzel_gefunden() {
        let tmp = std::env::temp_dir().join(format!("oc-help-{}", std::process::id()));
        let exe_dir = tmp.join("target").join("debug");
        std::fs::create_dir_all(&exe_dir).unwrap();
        assert_eq!(find_in(&exe_dir, DocFile::Readme), None);

        // Entwicklerlauf: Datei in der Repo-Wurzel zwei Ebenen über der exe
        std::fs::write(tmp.join("README.md"), "x").unwrap();
        assert_eq!(
            find_in(&exe_dir, DocFile::Readme),
            Some(tmp.join("README.md"))
        );

        // Ausgeliefert: Datei direkt neben der exe gewinnt
        std::fs::write(exe_dir.join("README.md"), "y").unwrap();
        assert_eq!(
            find_in(&exe_dir, DocFile::Readme),
            Some(exe_dir.join("README.md"))
        );

        std::fs::create_dir_all(exe_dir.join("docs")).unwrap();
        std::fs::write(exe_dir.join("docs").join("dokumentation.md"), "z").unwrap();
        assert!(find_in(&exe_dir, DocFile::Dokumentation).is_some());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn kandidaten_kommen_nur_aus_dem_enum() {
        let c = candidates(Path::new("C:/x/y"), DocFile::Dokumentation);
        assert!(c[0].ends_with("docs/dokumentation.md"), "{c:?}");
    }
}
