---
title: "Fix-Plan: dedupe-dirs — Review Runde 2 (PR #8)"
status: approved
created: 2026-10-09
updated: 2026-10-09
analysis: dedupe-dirs-review-round2-analysis.md
---

# Fix-Plan: dedupe-dirs — Review Runde 2

Entscheidungen des Users: Befund 2 mit neuem optionalem Plan-Feld `source_fingerprint`; Befund 8 als eigener Schritt; Befund 9 nur `hex()` und `locked`-Set zusammen mit Befund 1; Befund 3, 5, 6, 7 kein Fix; Befund 10 nur Test und Kommentar.

Sechs Schritte, je ein Commit auf `dev`, Test zuerst rot (`superpowers:test-driven-development`). Reihenfolge: A (Befund 1 + 9), B (Absicherung in `validate`), C (Befund 2), D (Befund 4), E (Befund 8), F (Befund 10).

## Schritt A: Gruppen von oben nach unten (Befund 1) und kleine Optimierungen (Befund 9)

### Entwurf
Ein Vorfahre hat immer eine strikt größere **Höhe** als jeder Nachfahre. Höhe eines Ordners = 0, wenn er nur zählende Dateien enthält, sonst `1 + größte Höhe seiner zählenden Unterordner` (dieselben Ordner, die in den Fingerabdruck eingehen; Junk und leere Unterordner zählen nicht). Mitglieder einer Gruppe haben gleiche Höhe, weil ihre Fingerabdrücke gleich sind; ein Vorfahre kann nie in derselben Gruppe wie sein Nachfahre stehen. Sortiert man Gruppen nach Höhe absteigend, laufen Gruppen mit Vorfahren-Mitgliedern **immer vor** Gruppen mit deren Nachfahren. Das ersetzt die Tiefe des flachsten Mitglieds als Näherung (`dedupe_dirs.rs:130`).

Folge für `locked`: Wird ein Ordner zuerst entschieden, kann ein später behandeltes Mitglied keinen behaltenen Ordner mehr enthalten. Die in Runde 1 ergänzte Klausel „enthält einen behaltenen Ordner“ ist dann unerreichbar und entfällt (sie ist sonst linear in der Zahl behaltener Ordner, Befund 9). `Plan::validate` (Schritt B) fängt jede Verletzung der Invariante trotzdem ab.

### Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/analysis/folder_dups.rs` | `enum Print::Value` (Z. 47–55) | neues Feld `height: u32`. |
| `src/analysis/folder_dups.rs` | `print_dir` (Z. 71–145) | Höhe berechnen: 0 ohne zählende Unterordner, sonst `1 + max(child.height)`; in `Print::Value` zurückgeben. |
| `src/analysis/folder_dups.rs` | `struct ExactGroup` (Z. 25) | neues Feld `pub height: u32` mit Doku „größte Verschachtelungstiefe unterhalb des Ordners“. |
| `src/analysis/folder_dups.rs` | `analyze` (Z. 161–201) | `by_print` merkt die Höhe mit; `ExactGroup { height, .. }`. |
| `src/analysis/folder_dups.rs` | neu `pub fn cmp_groups(a: &ExactGroup, b: &ExactGroup) -> Ordering` | Höhe absteigend, dann Tiefe des flachsten Mitglieds aufsteigend, dann Bytes absteigend, dann Mitglieder. |
| `src/analysis/folder_dups.rs` | `top_level_groups` (Z. 280) | Sortierung durch `cmp_groups` ersetzen; Doku-Kommentar: „höhere Gruppe = Gruppe mit größerer Höhe“. Auch der Bericht arbeitet damit von oben nach unten. |
| `src/analysis/folder_dups.rs` | `hex` (Z. 62) | Ohne `format!` je Byte: `String::with_capacity(bytes.len() * 2)` und zwei Nachschlag-Zeichen aus `b"0123456789abcdef"`. |
| `src/change/dedupe_dirs.rs` | Sortierung (Z. 129–136) | `groups.sort_by(|a, b| folder_dups::cmp_groups(a, b))`; `depth`-Import entfällt, falls ungenutzt. |
| `src/change/dedupe_dirs.rs` | `locked` (Z. 185) | Zurück auf `within(&kept, key)`; Kommentar: „Reihenfolge nach Höhe stellt sicher, dass kein späteres Mitglied einen behaltenen Ordner enthält.“ |
| `src/change/dedupe_dirs.rs` | Modulkopf | Halbsatz „Gruppen mit größerer Höhe zuerst“ statt „(oder über einem)“. |
| `docs/dokumentation.md` | Abschnitt 7.1a | Satz „von oben nach unten“ präzisieren: nach Höhe des Teilbaums, nicht nach Pfadtiefe. |

### Tests
1. `dedupe_dirs.rs` **neu, rot zuerst:** `behaltener_ordner_enthaelt_keinen_entfernten_ordner` — Index `W\f.txt` (mtime 1), `P\Y\x\f.txt` (5), `P\Z\x\f.txt` (10), gleicher Hash, Strategie `oldest`. Erwartung: Kein `keep` enthält die Quelle einer Aktion (Hilfsfunktion als Gegenstück zu `keeps_are_outside_removed`), keine Quelle liegt unter einer anderen Quelle, `plan.validate()` ok. Soll-Plan: `P\Z` (keep `P\Y`) und `W`/`P\Y\x`-Gruppe nach der Entscheidung (`P\Y\x` liegt im behaltenen `P\Y`, damit gesperrt).
2. `dedupe_dirs.rs`: `freed_bytes_zaehlt_verschachtelte_ordner_nicht_doppelt` — gleicher Aufbau, Summe = Bytes von `P\Z` allein.
3. `dedupe_dirs.rs`: Der Test `mitglied_mit_behaltenem_unterordner_wird_nie_entfernt` aus Runde 1 bleibt, aber seine zweite Assertion („`A\B` nicht in den Aktionen“) wird ersetzt: Mit der neuen Reihenfolge entscheidet zuerst `{A\B, M\N}` (behält `M\N`, entfernt `A\B`), das ist korrekt. Neu: `keeps_are_outside_removed` ok, keine verschachtelten Aktionen, `A\B\k` und `A\B\k2` nicht einzeln in den Aktionen.
4. `folder_dups.rs`: `gruppen_mit_vorfahren_kommen_vor_ihren_nachfahren` — `cmp_groups` sortiert `{Y, Z}` vor `{x, x'}`, unabhängig von Pfadtiefe; `ExactGroup.height` für eine Gruppe mit zwei Ebenen ist 1, mit nur Dateien 0.
5. `folder_dups.rs`: `top_level_groups` liefert für denselben Aufbau die Gruppe `{Y, Z}` und lässt `x`-Mitglieder in `Y`/`Z` weg (vorher falsch herum).
6. `folder_dups.rs`: `hex` — alle 256 Byte-Werte stimmen mit der `format!("{b:02x}")`-Ausgabe überein.
7. Bestehende Tests (`mitglied_im_behaltenen_ordner_wird_nie_entfernt`, `nur_die_oberste_ebene_wird_geplant`, Rundreise `tests/roundtrip_random.rs`) bleiben unverändert grün.

## Schritt B: Pläne gegen Überlappung absichern

### Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/change/plan.rs` | `Plan::validate`, zweite Schleife (nach dem Block aus Runde 1) | Nur für `PlanKind::DedupeDirs`: (a) Der Pfad einer Aktion darf nicht auf oder unter dem `keep` einer **anderen** Aktion liegen; Menge der `keep`-Schlüssel einmal vorab bilden, Vorfahren des Aktionspfads mit `ancestors_inclusive` nachschlagen. Fehler: „Aktion {id}: {path} liegt im behaltenen Ordner {keep} einer anderen Aktion“. (b) Keine Quelle liegt unter der Quelle einer anderen Aktion (`moved_with_dir`). Fehler: „Aktion {id}: {path} liegt in einem Ordner, der selbst entfernt wird“. |

Beides schränkt Dateien-Pläne nicht ein; Pläne anderer Art bleiben unberührt.

### Tests (`plan.rs`)
1. Aktion `X\k` (keep `W`) und Aktion `Y` (keep `X`) → ungültig (Pfad `X\k` liegt im `keep` `X` einer anderen Aktion).
2. Aktion `B` und Aktion `B\sub` → ungültig (Quelle in Quelle).
3. Gültig: Geschwister, `A`/`AB`-Präfix, mehrere Aktionen mit demselben `keep`.
4. Normaler Plan aus `plan_dedupe_dirs` besteht `validate` (läuft schon in `plan_full`).

## Schritt C: Quelle per Fingerabdruck prüfen (Befund 2)

### Entwurf
Der Planer berechnet für jede entfernte Quelle denselben Metadaten-Fingerabdruck wie für den behaltenen Ordner (`meta_print`: relativer Pfad, Größe, mtime der nicht ignorierten Dateien). Apply bildet ihn aus dem Dateisystem (`dir_fingerprint`) und vergleicht. Abweichung bedeutet `Stale`. Die bisherige Prüfung (Anzahl, Summe, jüngste mtime, Schutz, Links, Platzhalter) bleibt als erste Stufe.

### Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/change/plan.rs` | `PlannedAction` (Z. 74) | neues Feld `#[serde(default, skip_serializing_if = "Option::is_none")] pub source_fingerprint: Option<String>` mit Doku „Metadaten-Fingerabdruck des entfernten Ordners (nur `dedupe-dirs`, 32 Hex)“. `PLAN_VERSION` bleibt 2. |
| `src/change/plan.rs` | `validate_fields` (Z. 328ff.) | `source_fingerprint` nur bei `dedupe-dirs`-Quarantäne erlaubt (wie `keep_fingerprint`) und, falls gesetzt, `is_hash`. **Nicht** Pflicht: Pläne ohne Feld bleiben gültig und werden wie bisher geprüft. |
| alle `PlannedAction { … }`-Literale | rund 24 Stellen (`app/mod.rs`, `change/{apply,archive,dedupe,dedupe_dirs,empty_dirs,junk,limits,mod,rules,test_support,versions}.rs`, `gui/review.rs`, `ops/{apply,plan}.rs`, `tests/{cli_dedupe_dirs,gui_smoke,perf_plans}.rs`) | `source_fingerprint: None`; der Compiler findet alle. Mechanischer Teil zuerst, eigener Commit-Abschnitt. |
| `src/change/dedupe_dirs.rs` | Aktionsbau (Z. 251ff.) | `source_fingerprint: Some(hex128(meta_print(&tree, key)))` je entferntem Ordner. |
| `src/change/apply.rs` | `verify_quarantine_dir` (Z. 612) | Nach `verify_dir_source`: falls `a.source_fingerprint` gesetzt, `dir_fingerprint(Path::new(&a.path), env)` bilden. `Ok(print)` gleich dem Plan → weiter; sonst `Skipped(Stale)`. `Err(Missing)` → `Stale`, `Err(Unusable)` → `Unverifiable` (wie beim behaltenen Ordner). Das Ergebnis wird nicht gecacht (jede Quelle einmal). |
| `src/change/test_support.rs` | `dir_dedupe_plan` (Z. 301) | `source_fingerprint` aus `walk_entries(&dup)` füllen, damit Apply-Tests die neue Prüfung durchlaufen. |
| `docs/dokumentation.md` | 7.1a, Plan- und Apply-Absatz | Feld `source_fingerprint` und die zusätzliche Prüfung beschreiben; Hinweis, dass Pläne ohne Feld nur grob geprüft werden. |
| `docs/features/ordner-duplikate.md` | Abschnitt „Prüfung beim Apply“ (Z. 111) | Quelle zusätzlich per Fingerabdruck. |

Kosten: ein zweiter Metadaten-Walk über die Quelle (keine Dateiinhalte). Falls das spürbar langsam ist, kann `summarize_dir` später denselben Durchlauf liefern; nicht Teil dieses Plans.

### Tests
1. `apply.rs`, **rot zuerst:** `umbenannte_datei_im_entfernten_ordner_ist_stale` — Plan, danach `a.txt` → `umbenannt.txt` (Größe, Anzahl, mtime gleich) → `Skipped(Stale)`, Ordner bleibt, kein `intent`. Zusätzlich: Plan ohne `source_fingerprint` (Feld auf `None` gesetzt) verhält sich wie bisher (`Done`).
2. `apply.rs`: Bestehende Tests `neue_oder_geaenderte_datei_im_entfernten_ordner_ist_stale` und `zwei_kopien_desselben_ordners_gehen_beide` bleiben grün.
3. `apply.rs`: Cloud-Platzhalter in der Quelle → weiterhin `CloudPlaceholder` (erste Stufe schlägt zuerst an).
4. `plan.rs`: Feld bei anderer Plan-Art → ungültig; ungültiger Hex → ungültig; Rundlauf `to_json`/`from_json`; leeres Feld wird nicht geschrieben.
5. `dedupe_dirs.rs`: Plan enthält `source_fingerprint` je Aktion, gleich `meta_print` des Teilbaums.
6. `tests/roundtrip_random.rs` bleibt grün.

## Schritt D: Werkzeugordner im Bericht (Befund 4)

### Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/analysis/folder_dups.rs` | neu `fn in_tool_dir(key: &str) -> bool` | Schlüssel hat eine Komponente `.ordner-cleanup` (`protect::TOOL_DIR`, Schlüssel sind kleingeschrieben). |
| `src/analysis/folder_dups.rs` | `analyze` (Z. 176) | `is_internal` ersetzen durch `in_tool_dir(key)`: Der Werkzeugordner **und alles darunter** bildet keine Gruppe. |
| `src/analysis/folder_dups.rs` | `print_dir` (Kinderschleife) | Kind `.ordner-cleanup` überspringen wie `Print::Empty`, damit der Werkzeugordner den Fingerabdruck seines Elternordners nicht verändert. |
| `src/analysis/folder_dups.rs` | `partial_pairs` (Z. 375) | `skip_dir` verwendet `in_tool_dir`. |

### Tests (`folder_dups.rs`)
1. **Rot zuerst:** `quarantaene_kopie_unter_dem_werkzeugordner_bildet_keine_gruppe` — `Projekt\a.txt` und `.ordner-cleanup\quarantine\r1\Projekt\a.txt` (gleicher Hash) → keine Gruppe.
2. `werkzeugordner_aendert_den_fingerabdruck_des_elternordners_nicht` — zwei sonst gleiche Ordner, einer enthält `.ordner-cleanup\x.txt` → weiter eine Gruppe.
3. Bestehender Test zu `.ordner-cleanup` (falls vorhanden) bleibt grün; Bericht-Test in `tests_folder.rs` erweitern: Quarantäne-Kopie erscheint nicht.

## Schritt E: `short_path` (Befund 8)

### Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/gui/format.rs` | `short_path` (Z. 141) | Nach Berechnung von `head`: Passt der Kopf nicht mit mindestens `…` in `max_chars` (`head_len + 1 > max_chars`), „…“ plus die letzten `max_chars - 1` Zeichen des **ganzen** Pfads zurückgeben (`max_chars == 0` → leerer String). Alle anderen Fälle unverändert. |

### Tests (`format.rs`, Parameter-Test)
1. **Rot zuerst:** UNC-Pfad `\\server\share` bei 8 und `\\sehr-langer-servername\freigabe\ordner\datei.txt` bei 12: `chars().count() <= max_chars`, Ergebnis beginnt mit `…` und endet mit dem Ende des Pfads. (Backslashes im Test über `char::from(92)` bzw. Rohstrings bilden.)
2. Invariante für eine Reihe Pfade und `max_chars` von 0 bis 40: nie länger als `max_chars`.
3. Bestehende `short_path`-Tests bleiben grün.

## Schritt F: `contains_protected` (Befund 10)

### Änderungen
| Datei | Stelle | Änderung |
|-------|--------|----------|
| `src/change/tree.rs` | Doku von `contains_protected` (Z. 166) | Begründung: `check_cached` löst den letzten Pfadteil nicht auf; Datei-Links erkennt `has_link` im Teilbaum (`archive` und `dedupe-dirs` lassen solche Ordner liegen), Verzeichnis-Links `check_inside`, Apply prüft streng mit `check`. |

### Tests
1. `dedupe_dirs.rs`: `datei_link_im_ordner_macht_ihn_zum_nicht_kandidaten` (Gegenstück zu `cloud_datei_macht_den_ordner_zum_nicht_kandidaten`, `tweak` setzt den Link-Marker der Datei).
2. `archive.rs`: Ordner mit Datei-Link wird nicht archiviert (`SkipReason::Link`), Gegenstück zu `link_ordner_darin_verhindert_das_verschieben`.

## Gesamtablauf und Prüfung
1. Pro Schritt: roter Test → Umsetzung → `cargo test --lib` des Moduls → Commit auf `dev` (kurze Message ohne Footer laut CLAUDE.md).
2. Zum Schluss: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, volles `cargo test`.
3. Performance: `cargo test --release --test perf_plans -- --ignored --nocapture plan_dedupe_dirs` im Hintergrund starten (der Release-Build mit LTO dauert länger als das Standard-Zeitlimit) und das Ergebnis (Grenze 3 s je 100.000 Dateien) im Bericht festhalten.
4. `superpowers:verification-before-completion` vor `status: fixed`; danach Push und `/code-review` erneut auf PR #8.

## Risiken
- **Reihenfolge ändert Pläne:** Für verschachtelte Gruppen entsteht ein anderer, konsistenter Plan als in Runde 1 (siehe Test A3). Für unabhängige Gruppen bleibt das Ergebnis gleich; die Aktionen werden ohnehin nach Pfad sortiert und neu nummeriert.
- **Bericht ändert sich:** `top_level_groups` nutzt dieselbe Reihenfolge; Berichtszahlen können sich bei verschachtelten Gruppen verschieben (richtiger als zuvor). Berichts-Tests prüfen.
- **Plan-Format:** Neues optionales Feld; ältere Programmversionen lehnen Pläne mit dem Feld ab (`deny_unknown_fields`). `dedupe-dirs` ist in PR #8 neu und noch nicht veröffentlicht. Pläne ohne Feld bleiben lesbar.
- **Mehr Dateisystemzugriffe im Apply:** ein zusätzlicher Metadaten-Walk je Quelle (Schritt C). Messen, falls Apply auf großen Ordnern langsamer wirkt.
- **`validate` strenger** (Schritt B): Handgebaute Pläne mit überlappenden Aktionen werden abgelehnt. Planerzeugte Pläne erfüllen es nach Schritt A.
- **`height` im `ExactGroup`:** `PartialEq` und Berichts-Code sind davon abhängig; die zwei Konstruktorstellen sind bekannt.

## Rollback
Sechs getrennte Commits, einzeln mit `git revert` zurücknehmbar. Schritt C ändert das Plan-Format um ein optionales Feld: Nach einem Revert müssen bereits erzeugte Pläne mit `source_fingerprint` neu erzeugt werden (oder das Feld im Struct bleibt und nur die Prüfung wird zurückgenommen). Schritt A und B ändern keine Dateiformate.
