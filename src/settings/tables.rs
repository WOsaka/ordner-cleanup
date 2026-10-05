//! Listen von Tabellen im Dokument (`[[rules]]`, `[[junk_rules]]`): auflisten, hinzufügen,
//! duplizieren, löschen, verschieben und einzelne Felder lesen und schreiben. Kommentare und
//! die übrigen Felder bleiben erhalten.

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

use super::fields::{self, Value};

fn array<'a>(doc: &'a DocumentMut, key: &str) -> Option<&'a ArrayOfTables> {
    doc.get(key)?.as_array_of_tables()
}

fn array_mut<'a>(doc: &'a mut DocumentMut, key: &str) -> &'a mut ArrayOfTables {
    if !doc.contains_key(key) {
        doc.insert(key, Item::ArrayOfTables(ArrayOfTables::new()));
    }
    doc[key]
        .as_array_of_tables_mut()
        .expect("Schlüssel ist eine Liste von Tabellen")
}

pub fn len(doc: &DocumentMut, key: &str) -> usize {
    array(doc, key).map_or(0, ArrayOfTables::len)
}

/// Der Name (Feld `name`) je Eintrag; ohne Namen ein Platzhalter.
pub fn names(doc: &DocumentMut, key: &str) -> Vec<String> {
    array(doc, key)
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, t)| {
                    t.get("name")
                        .and_then(Item::as_str)
                        .filter(|n| !n.trim().is_empty())
                        .map_or_else(|| format!("(Eintrag {})", i + 1), String::from)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn unique_name(existing: &[String], wanted: &str) -> String {
    let taken = |n: &str| existing.iter().any(|e| e.eq_ignore_ascii_case(n));
    if !taken(wanted) {
        return wanted.to_string();
    }
    (2..)
        .map(|n| format!("{wanted} {n}"))
        .find(|c| !taken(c))
        .expect("unendliche Folge")
}

/// Hängt einen Eintrag mit diesen Anfangswerten an; der Name wird eindeutig gemacht. Liefert den
/// Index.
pub fn add(doc: &mut DocumentMut, key: &str, name: &str, initial: &[(&str, Value)]) -> usize {
    let existing = names(doc, key);
    let mut table = Table::new();
    table.insert("name", toml_edit::value(unique_name(&existing, name)));
    for (field, value) in initial {
        fields::set_in_table(&mut table, field, value);
    }
    let array = array_mut(doc, key);
    array.push(table);
    array.len() - 1
}

/// Kopiert einen Eintrag direkt hinter das Original („<Name> Kopie“). Liefert den neuen Index.
pub fn duplicate(doc: &mut DocumentMut, key: &str, index: usize) -> Option<usize> {
    let existing = names(doc, key);
    let mut copy = array(doc, key)?.get(index)?.clone();
    let base = existing.get(index)?.clone();
    copy.insert(
        "name",
        toml_edit::value(unique_name(&existing, &format!("{base} Kopie"))),
    );
    let mut tables: Vec<Table> = array(doc, key)?.iter().cloned().collect();
    tables.insert(index + 1, copy);
    replace_all(doc, key, tables);
    Some(index + 1)
}

fn replace_all(doc: &mut DocumentMut, key: &str, tables: Vec<Table>) {
    let array = array_mut(doc, key);
    array.clear();
    for t in tables {
        array.push(t);
    }
}

pub fn remove(doc: &mut DocumentMut, key: &str, index: usize) -> bool {
    let Some(a) = doc.get_mut(key).and_then(Item::as_array_of_tables_mut) else {
        return false;
    };
    if index >= a.len() {
        return false;
    }
    a.remove(index);
    if a.is_empty() {
        doc.remove(key);
    }
    true
}

/// Verschiebt einen Eintrag um `delta` Stellen (negativ = nach oben). Liefert den neuen Index.
pub fn move_by(doc: &mut DocumentMut, key: &str, index: usize, delta: isize) -> Option<usize> {
    let mut tables: Vec<Table> = array(doc, key)?.iter().cloned().collect();
    let target = index
        .checked_add_signed(delta)
        .filter(|t| *t < tables.len())?;
    let table = tables.remove(index);
    tables.insert(target, table);
    replace_all(doc, key, tables);
    Some(target)
}

pub fn get_field(doc: &DocumentMut, key: &str, index: usize, field: &str) -> Option<Value> {
    fields::get_in_table(array(doc, key)?.get(index)?, field)
}

pub fn set_field(doc: &mut DocumentMut, key: &str, index: usize, field: &str, value: &Value) {
    if let Some(table) = doc
        .get_mut(key)
        .and_then(Item::as_array_of_tables_mut)
        .and_then(|a| a.get_mut(index))
    {
        fields::set_in_table(table, field, value);
    }
}

pub fn unset_field(doc: &mut DocumentMut, key: &str, index: usize, field: &str) {
    if let Some(table) = doc
        .get_mut(key)
        .and_then(Item::as_array_of_tables_mut)
        .and_then(|a| a.get_mut(index))
    {
        table.remove(field);
    }
}

/// Eine Tabelle von Text zu Text im Eintrag (`[rules.fields]`) als `schlüssel = wert`-Zeilen.
pub fn get_map(doc: &DocumentMut, key: &str, index: usize, field: &str) -> Vec<String> {
    array(doc, key)
        .and_then(|a| a.get(index))
        .and_then(|t| t.get(field))
        .and_then(|i| i.as_table_like())
        .map(|t| {
            t.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| format!("{k} = {v}")))
                .collect()
        })
        .unwrap_or_default()
}

/// Ersetzt die Tabelle durch die Zeilen `schlüssel = wert`; leer entfernt sie. Zeilen ohne `=`
/// sind ein Fehler.
pub fn set_map(
    doc: &mut DocumentMut,
    key: &str,
    index: usize,
    field: &str,
    lines: &[String],
) -> Result<(), String> {
    let mut map = Table::new();
    for line in lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| format!("„{line}“: erwartet schlüssel = wert"))?;
        if k.trim().is_empty() {
            return Err(format!("„{line}“: der Schlüssel fehlt"));
        }
        map.insert(k.trim(), toml_edit::value(v.trim()));
    }
    let Some(table) = doc
        .get_mut(key)
        .and_then(Item::as_array_of_tables_mut)
        .and_then(|a| a.get_mut(index))
    else {
        return Ok(());
    };
    if map.is_empty() {
        table.remove(field);
    } else {
        table.insert(field, Item::Table(map));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> DocumentMut {
        "# Regeln\n[[rules]]\nname = \"pdf\" # Dokumente\next = [\"pdf\"]\ntarget = \"Dokumente/\"\n\n[[rules]]\nname = \"fotos\"\next = [\"jpg\"]\ntarget = \"Fotos/\"\n"
            .parse()
            .unwrap()
    }

    #[test]
    fn namen_und_anzahl() {
        let d = doc();
        assert_eq!(len(&d, "rules"), 2);
        assert_eq!(names(&d, "rules"), ["pdf", "fotos"]);
        assert_eq!(len(&d, "nix"), 0);
    }

    #[test]
    fn hinzufuegen_macht_den_namen_eindeutig_und_setzt_anfangswerte() {
        let mut d = doc();
        let i = add(
            &mut d,
            "rules",
            "PDF",
            &[("target", Value::Text("X/".into()))],
        );
        assert_eq!(i, 2);
        assert_eq!(names(&d, "rules")[2], "PDF 2");
        assert_eq!(
            get_field(&d, "rules", 2, "target"),
            Some(Value::Text("X/".into()))
        );
        let mut empty: DocumentMut = "".parse().unwrap();
        assert_eq!(add(&mut empty, "rules", "neu", &[]), 0);
        assert!(empty.to_string().contains("[[rules]]"));
    }

    #[test]
    fn duplizieren_kopiert_hinter_das_original_mit_neuem_namen() {
        let mut d = doc();
        assert_eq!(duplicate(&mut d, "rules", 0), Some(1));
        assert_eq!(names(&d, "rules"), ["pdf", "pdf Kopie", "fotos"]);
        assert_eq!(
            get_field(&d, "rules", 1, "target"),
            Some(Value::Text("Dokumente/".into()))
        );
        assert_eq!(duplicate(&mut d, "rules", 0), Some(1));
        assert_eq!(
            names(&d, "rules"),
            ["pdf", "pdf Kopie 2", "pdf Kopie", "fotos"]
        );
        assert_eq!(duplicate(&mut d, "rules", 9), None);
    }

    #[test]
    fn loeschen_und_verschieben_behalten_kommentare() {
        let mut d = doc();
        assert_eq!(move_by(&mut d, "rules", 1, -1), Some(0));
        assert_eq!(names(&d, "rules"), ["fotos", "pdf"]);
        assert!(d.to_string().contains("name = \"pdf\" # Dokumente"), "{d}");
        assert_eq!(move_by(&mut d, "rules", 0, -1), None, "über den Rand");
        assert!(remove(&mut d, "rules", 0));
        assert_eq!(names(&d, "rules"), ["pdf"]);
        assert!(remove(&mut d, "rules", 0));
        assert!(!d.contains_key("rules"), "leere Liste verschwindet");
        assert!(!remove(&mut d, "rules", 0));
    }

    #[test]
    fn felder_setzen_und_zuruecksetzen() {
        let mut d = doc();
        set_field(&mut d, "rules", 0, "min_age", &Value::Text("30d".into()));
        assert_eq!(
            get_field(&d, "rules", 0, "min_age"),
            Some(Value::Text("30d".into()))
        );
        set_field(&mut d, "rules", 0, "name", &Value::Text("pdfs".into()));
        assert!(d.to_string().contains("name = \"pdfs\" # Dokumente"), "{d}");
        unset_field(&mut d, "rules", 0, "min_age");
        assert_eq!(get_field(&d, "rules", 0, "min_age"), None);
    }

    #[test]
    fn tabellenfelder_als_zeilen() {
        let mut d = doc();
        set_map(
            &mut d,
            "rules",
            0,
            "fields",
            &["absender = acme".to_string(), " nummer = 12 ".to_string()],
        )
        .unwrap();
        assert_eq!(
            get_map(&d, "rules", 0, "fields"),
            ["absender = acme", "nummer = 12"]
        );
        assert!(set_map(&mut d, "rules", 0, "fields", &["kaputt".to_string()]).is_err());
        assert_eq!(
            get_map(&d, "rules", 0, "fields").len(),
            2,
            "Fehler ändert nichts"
        );
        set_map(&mut d, "rules", 0, "fields", &[]).unwrap();
        assert!(get_map(&d, "rules", 0, "fields").is_empty());
    }
}
