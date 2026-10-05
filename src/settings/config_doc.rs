//! Profile im Dokument der `config.toml`: anlegen, umbenennen, löschen.

use toml_edit::{DocumentMut, Item};

use super::fields::{self, Value};
use crate::config::valid_profile_name;

pub fn profile_names(doc: &DocumentMut) -> Vec<String> {
    doc.get("profiles")
        .and_then(Item::as_table_like)
        .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default()
}

fn check_name(doc: &DocumentMut, name: &str) -> Result<(), String> {
    if !valid_profile_name(name) {
        return Err(
            "Ungültiger Name (erlaubt: Kleinbuchstaben, Ziffern und „-“, nicht leer)".into(),
        );
    }
    if profile_names(doc).iter().any(|n| n == name) {
        return Err(format!("Ein Profil „{name}“ gibt es schon"));
    }
    Ok(())
}

/// Legt `[profiles.<name>]` mit der Wurzel an.
pub fn add_profile(doc: &mut DocumentMut, name: &str, root: &str) -> Result<(), String> {
    check_name(doc, name)?;
    if root.trim().is_empty() {
        return Err("Die Wurzel darf nicht leer sein".into());
    }
    fields::set(
        doc,
        &["profiles", name],
        "root",
        &Value::Text(root.trim().to_string()),
    );
    Ok(())
}

/// Benennt ein Profil um; Inhalt und Kommentare bleiben. Ein vorhandener Zeitplan gehört zum
/// alten Namen und muss neu angelegt werden (das meldet der Aufrufer).
pub fn rename_profile(doc: &mut DocumentMut, old: &str, new: &str) -> Result<(), String> {
    check_name(doc, new)?;
    let profiles = doc
        .get_mut("profiles")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| format!("Profil „{old}“ nicht gefunden"))?;
    let item = profiles
        .remove(old)
        .ok_or_else(|| format!("Profil „{old}“ nicht gefunden"))?;
    profiles.insert(new, item);
    Ok(())
}

pub fn delete_profile(doc: &mut DocumentMut, name: &str) -> bool {
    let Some(profiles) = doc.get_mut("profiles").and_then(Item::as_table_mut) else {
        return false;
    };
    let removed = profiles.remove(name).is_some();
    if profiles.is_empty() {
        doc.remove("profiles");
    }
    removed
}

/// Der Tabellenpfad eines Profils für [`fields::get`] und [`fields::set`].
pub fn profile_base(name: &str) -> [&str; 2] {
    ["profiles", name]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn doc() -> DocumentMut {
        "top = 5\n\n# Fotos\n[profiles.fotos]\nroot = 'D:\\Fotos'\nplans = [\"junk\"] # Müll\n"
            .parse()
            .unwrap()
    }

    #[test]
    fn anlegen_prueft_name_und_wurzel() {
        let mut d = doc();
        add_profile(&mut d, "downloads", r"C:\Users\x\Downloads").unwrap();
        assert_eq!(profile_names(&d), ["fotos", "downloads"]);
        let cfg = Config::parse(&d.to_string()).unwrap();
        assert_eq!(cfg.profiles["downloads"].root, r"C:\Users\x\Downloads");
        assert!(add_profile(&mut d, "Foto Alt", "D:\\x").is_err());
        assert!(add_profile(&mut d, "_gui", "D:\\x").is_err());
        assert!(add_profile(&mut d, "fotos", "D:\\x")
            .unwrap_err()
            .contains("schon"));
        assert!(add_profile(&mut d, "leer", "  ").is_err());
        assert_eq!(profile_names(&d).len(), 2, "Fehler ändern nichts");
    }

    #[test]
    fn umbenennen_behaelt_inhalt_und_kommentare() {
        let mut d = doc();
        rename_profile(&mut d, "fotos", "bilder").unwrap();
        let text = d.to_string();
        assert!(
            text.contains("[profiles.bilder]") && text.contains("# Müll"),
            "{text}"
        );
        assert_eq!(
            Config::parse(&text).unwrap().profiles["bilder"].plans,
            ["junk"]
        );
        assert!(rename_profile(&mut d, "bilder", "Ungültig").is_err());
        assert!(rename_profile(&mut d, "gibt-es-nicht", "x").is_err());
    }

    #[test]
    fn loeschen_entfernt_die_tabelle_samt_leerem_profiles() {
        let mut d = doc();
        assert!(delete_profile(&mut d, "fotos"));
        assert!(!d.to_string().contains("profiles"), "{d}");
        assert!(!delete_profile(&mut d, "fotos"));
        assert!(Config::parse(&d.to_string()).is_ok());
    }
}
