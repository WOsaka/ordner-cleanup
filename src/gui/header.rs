//! Kopfleiste über dem Seiteninhalt: Ziel wählen, scannen, klassifizieren. Dieser Teil hält die
//! Optionen und baut daraus die Anfragen; die Zeichnung kommt in den nächsten Schritten dazu.

use eframe::egui;

use super::shell::Choice;
use crate::ops::classify::ClassifyRequest;
use crate::ops::scan::ScanRequest;
use crate::ops::target::TargetSpec;

/// Eingaben des Dialogs „Scan-Optionen“, wie sie als Text im Formular stehen.
#[derive(Debug, Default, Clone)]
pub struct ScanOptions {
    // Scannen
    pub net_force: bool,
    pub no_default_excludes: bool,
    pub reset_index: bool,
    pub threads: String,
    pub template: String,
    pub exclude: String,
    pub summary_only: String,
    // Klassifizieren
    pub no_llm: bool,
    pub force: bool,
    pub ext: String,
    pub only: String,
}

/// Nichtleere, getrimmte Zeilen.
pub fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Getrimmter Text, leer = `None`.
pub fn opt(text: &str) -> Option<String> {
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

impl ScanOptions {
    /// Ordner oder Profil als Ziel; „Netzlaufwerk trotzdem“ gilt nur für Ordner.
    pub fn target_spec(&self, choice: &Choice) -> TargetSpec {
        match choice {
            Choice::Folder(path) => TargetSpec::Path {
                path: path.clone(),
                force: self.net_force,
            },
            Choice::Profile(name) => TargetSpec::Profile(name.clone()),
        }
    }

    pub fn scan_request(&self, choice: &Choice) -> ScanRequest {
        ScanRequest {
            target: self.target_spec(choice),
            exclude: lines(&self.exclude),
            summary_only: lines(&self.summary_only),
            no_default_excludes: self.no_default_excludes,
            reset_index: self.reset_index,
            threads: self.threads.trim().parse().ok(),
            template: opt(&self.template),
        }
    }

    pub fn classify_request(&self, choice: &Choice, clear: bool) -> ClassifyRequest {
        ClassifyRequest {
            target: self.target_spec(choice),
            no_llm: self.no_llm,
            force: self.force,
            ext: self
                .ext
                .split(',')
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
                .collect(),
            only: opt(&self.only),
            clear,
        }
    }
}

/// Der Dialog „Scan-Optionen“ mit den getrennten Abschnitten Scannen und Klassifizieren.
/// `true`, wenn „Text-Cache löschen“ gedrückt wurde.
pub fn options_ui(ui: &mut egui::Ui, options: &mut ScanOptions, can_clear: bool) -> bool {
    ui.strong("Scannen");
    ui.checkbox(&mut options.net_force, "Netzlaufwerk trotzdem scannen");
    ui.checkbox(&mut options.no_default_excludes, "Standard-Ausschlüsse aus");
    ui.checkbox(&mut options.reset_index, "Index neu aufbauen");
    ui.horizontal(|ui| {
        ui.label("Threads (leer = automatisch):");
        ui.add(egui::TextEdit::singleline(&mut options.threads).desired_width(50.0));
    });
    ui.horizontal(|ui| {
        ui.label("Vorlage für Soll/Ist (para, johnny-decimal oder Pfad):");
        ui.add(egui::TextEdit::singleline(&mut options.template).desired_width(220.0));
    });
    ui.label("Zusätzliche Ausschlüsse (ein Muster je Zeile):");
    ui.add(egui::TextEdit::multiline(&mut options.exclude).desired_rows(2));
    ui.label("Nur Summe (ein Muster je Zeile):");
    ui.add(egui::TextEdit::multiline(&mut options.summary_only).desired_rows(2));
    ui.separator();
    ui.strong("Klassifizieren");
    ui.checkbox(&mut options.no_llm, "Ohne LLM");
    ui.checkbox(&mut options.force, "Alles neu analysieren");
    ui.horizontal(|ui| {
        ui.label("Nur Endungen (z. B. pdf,jpg):");
        ui.add(egui::TextEdit::singleline(&mut options.ext).desired_width(140.0));
    });
    ui.horizontal(|ui| {
        ui.label("Nur Glob (relativ zur Wurzel):");
        ui.add(egui::TextEdit::singleline(&mut options.only).desired_width(180.0));
    });
    ui.add_enabled(can_clear, egui::Button::new("Text-Cache löschen"))
        .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn folder() -> Choice {
        Choice::Folder(PathBuf::from(r"D:\Daten"))
    }

    #[test]
    fn zeilen_und_optionen_werden_bereinigt() {
        assert_eq!(
            lines(" *.tmp \n\n  node_modules\n"),
            ["*.tmp", "node_modules"]
        );
        assert_eq!(opt("  "), None);
        assert_eq!(opt(" para "), Some("para".into()));
    }

    #[test]
    fn scan_anfrage_uebernimmt_alle_scan_optionen() {
        let options = ScanOptions {
            net_force: true,
            no_default_excludes: true,
            reset_index: true,
            threads: " 4 ".into(),
            template: " para ".into(),
            exclude: "*.tmp\n  node_modules \n".into(),
            summary_only: "Backup\n".into(),
            ..ScanOptions::default()
        };
        let req = options.scan_request(&folder());
        assert!(req.no_default_excludes && req.reset_index);
        assert_eq!(req.threads, Some(4));
        assert_eq!(req.template.as_deref(), Some("para"));
        assert_eq!(req.exclude, ["*.tmp", "node_modules"]);
        assert_eq!(req.summary_only, ["Backup"]);
        match req.target {
            TargetSpec::Path { path, force } => {
                assert_eq!(path, PathBuf::from(r"D:\Daten"));
                assert!(force, "Netzlaufwerk trotzdem");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn leere_felder_bleiben_leer_und_ungueltige_threads_werden_automatisch() {
        let options = ScanOptions {
            threads: "abc".into(),
            ..ScanOptions::default()
        };
        let req = options.scan_request(&folder());
        assert_eq!(req.threads, None);
        assert_eq!(req.template, None);
        assert!(req.exclude.is_empty() && req.summary_only.is_empty());
        assert!(!req.no_default_excludes && !req.reset_index);
    }

    #[test]
    fn profil_als_ziel_ignoriert_netzlaufwerk_haken() {
        let options = ScanOptions {
            net_force: true,
            ..ScanOptions::default()
        };
        let req = options.scan_request(&Choice::Profile("downloads".into()));
        assert!(matches!(req.target, TargetSpec::Profile(n) if n == "downloads"));
    }

    #[test]
    fn klassifizier_anfrage_uebernimmt_ihre_optionen() {
        let options = ScanOptions {
            no_llm: true,
            force: true,
            ext: " pdf, jpg ,,docx".into(),
            only: " Rechnungen/** ".into(),
            net_force: true,
            ..ScanOptions::default()
        };
        let req = options.classify_request(&folder(), false);
        assert!(req.no_llm && req.force && !req.clear);
        assert_eq!(req.ext, ["pdf", "jpg", "docx"]);
        assert_eq!(req.only.as_deref(), Some("Rechnungen/**"));
        assert!(matches!(req.target, TargetSpec::Path { force: true, .. }));
    }

    #[test]
    fn text_cache_loeschen_setzt_clear() {
        let req = ScanOptions::default().classify_request(&folder(), true);
        assert!(req.clear);
    }
}
