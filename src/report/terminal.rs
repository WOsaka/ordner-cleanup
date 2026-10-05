use std::collections::BTreeMap;
use std::fmt::Write;

use bytesize::ByteSize;

use super::Report;

fn size(bytes: u64) -> String {
    ByteSize::b(bytes).to_string()
}

/// Kompakte Zusammenfassung mit den wichtigsten Kennzahlen.
pub fn render(report: &Report) -> String {
    let o = &report.overview;
    let mut s = String::new();
    let _ = writeln!(s, "Bericht für {}", report.meta.root);
    if let Some(at) = &report.meta.scanned_at {
        let _ = writeln!(s, "Letzter Scan: {at} ({})", report.meta.scan_status);
    }
    if report.meta.scan_status != "complete" {
        let _ = writeln!(s, "ACHTUNG: Der Scan wurde nicht vollständig abgeschlossen, die Zahlen sind unvollständig.");
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "Übersicht");
    let _ = writeln!(s, "  Dateien:          {}", o.files);
    let _ = writeln!(s, "  Ordner:           {}", o.dirs);
    let _ = writeln!(s, "  Gesamtgröße:      {}", size(o.total_size));
    let _ = writeln!(
        s,
        "  davon nur Summe:  {} ({:.0} %)",
        size(o.summary_size),
        o.summary_share * 100.0
    );
    let _ = writeln!(
        s,
        "  Cloud-Platzhalter: {} ({})",
        o.cloud_only_files,
        size(o.cloud_only_size)
    );
    let _ = writeln!(s, "  Fehler:           {}", o.error_count);
    if let Some(t) = &report.template {
        let _ = writeln!(
            s,
            "  Vorlage {}: {} Abweichungen bei {} geprüften Einträgen",
            t.name, t.total, t.checked
        );
    }
    if let Some(h) = &report.history {
        let _ = writeln!(
            s,
            "  {}",
            super::history::score_line(h.score, h.comparison.as_ref())
        );
    }

    if let Some(c) = &report.content {
        let _ = writeln!(s, "\nInhalte");
        if !c.classified {
            let _ = writeln!(
                s,
                "  noch nicht klassifiziert (ordner-cleanup classify <pfad>)"
            );
        } else {
            let cats: Vec<String> = c
                .categories
                .iter()
                .take(5)
                .map(|k| format!("{} {}", k.name, k.count))
                .collect();
            let _ = writeln!(
                s,
                "  {} von {} Dateien klassifiziert: {} · ohne Kategorie {}",
                c.files_classified,
                c.files_total,
                cats.join(" · "),
                c.uncategorized
            );
            let _ = writeln!(s, "  Zum Prüfen: {}", c.review_total);
        }
    }

    if !report.top_dirs.is_empty() {
        let _ = writeln!(s, "\nGrößte Ordner");
        for d in report.top_dirs.iter().take(5) {
            let _ = writeln!(s, "  {:>10}  {}", size(d.size), d.path);
        }
    }
    if !report.top_files.is_empty() {
        let _ = writeln!(s, "\nGrößte Dateien");
        for f in report.top_files.iter().take(5) {
            let _ = writeln!(s, "  {:>10}  {}", size(f.size), f.path);
        }
    }

    let _ = writeln!(s, "\nDuplikate");
    let _ = writeln!(
        s,
        "  Exakte Gruppen:   {} (verschwendet: {})",
        report.duplicates.group_count,
        size(report.duplicates.total_wasted)
    );
    let _ = writeln!(
        s,
        "  Wahrscheinlich:   {} Gruppen (nicht verifiziert, Cloud)",
        report.probable_duplicates.len()
    );
    let _ = writeln!(s, "  Ähnliche Dateien: {} Gruppen", report.similar.len());

    let _ = writeln!(
        s,
        "\nAlt (> {} Tage): {} Dateien, {}",
        report.age.old_after_days,
        report.age.old_files.len(),
        size(report.age.old_total_size)
    );

    let mut structure: BTreeMap<&str, usize> = BTreeMap::new();
    for item in &report.structure {
        *structure.entry(item.label.as_str()).or_default() += 1;
    }
    if !structure.is_empty() {
        let _ = writeln!(s, "\nStruktur");
        for (label, count) in structure {
            let _ = writeln!(s, "  {label}: {count}");
        }
    }

    let mut problems: BTreeMap<&str, usize> = BTreeMap::new();
    for item in &report.problems {
        for label in &item.labels {
            *problems.entry(label.as_str()).or_default() += 1;
        }
    }
    if !problems.is_empty() {
        let _ = writeln!(s, "\nProbleme");
        for (label, count) in problems {
            let _ = writeln!(s, "  {label}: {count}");
        }
    }
    s
}
