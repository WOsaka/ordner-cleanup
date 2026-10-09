---
title: "Plan-Integrität (Hash der Plan-Datei, vor dem Apply geprüft)"
status: draft          # draft | approved | implemented
created: 2026-10-09
updated: 2026-10-09
---

# Feature: Plan-Integrität

Roadmap 7.0 („Konsolidieren vor neuen Features“): Hash der Plan-Datei, vor dem Apply geprüft.

## Problem Statement

Zwischen `plan` und `apply` liegt bewusst ein Review. In dieser Zeit kann sich die Plan-Datei unbemerkt ändern: Ein Editor speichert sie versehentlich, ein OneDrive-Sync-Konflikt ersetzt sie, ein Schreibvorgang bricht halb ab, oder ein Plan von Kollegen wird nach dem Review noch angepasst. `Plan::validate` prüft nur die Struktur; ein strukturell gültiger, aber veränderter Plan wird heute ausgeführt, obwohl niemand genau diesen Inhalt geprüft hat. Betroffen sind alle, die Pläne in der CLI oder der GUI anwenden, besonders bei weitergegebenen Plänen.

## Goals

- [ ] Neue Pläne (Format v3) tragen ein Feld `integrity` mit dem SHA-256 ihres kanonischen Inhalts; `plan …` schreibt es automatisch.
- [ ] `apply` (CLI und GUI) prüft den Hash vor jeder Dateioperation und bricht bei Abweichung mit klarer Meldung und eigenem Exit-Code ab.
- [ ] Ältere Pläne (v1/v2) bleiben anwendbar, mit sichtbarer Warnung „ungeschützter Plan“.
- [ ] Bewusste Handänderungen bleiben möglich über `plan seal <datei>`, ohne dass die Prüfung abschaltbar ist.

## Out of Scope

- Signaturen, Schlüssel, HMAC: Wer Schreibrechte auf die Plan-Datei hat, kann den Hash neu berechnen. Schutz gegen Angreifer ist kein Ziel, nur gegen Versehen und unbemerkte Änderung.
- Verschlüsselung der Plan-Datei.
- Schutz von Journal, Quarantäne oder Index gegen Manipulation.
- Neue Review-Funktionen.
- Eine Option, die Prüfung zu überspringen (`--no-verify` o. ä.).

## User Journey

1. Nutzer erzeugt einen Plan (`plan dedupe`, `plan junk`, … oder in der GUI). Der Plan wird als Format v3 mit Feld `integrity` gespeichert; die Ausgabe von `plan` nennt den Hash (gekürzt).
2. Nutzer prüft den Plan (CLI, Bericht oder GUI-Review). Das Review verändert die Datei nicht.
3. `apply <plan>` lädt die Datei, berechnet den Hash neu und vergleicht:
   - stimmt: weiter wie bisher (Rückfrage, Ausführung, Journal);
   - weicht ab: Abbruch vor jeder Dateioperation mit „Plan wurde nach dem Erstellen verändert“, Exit-Code `3`, kein Journal-Lauf;
   - v1/v2-Plan ohne Feld: Warnung „ungeschützter Plan (Format vN)“, danach weiter wie bisher.
4. Wer einen Plan bewusst von Hand ändert (z. B. Aktionen streicht), ruft `plan seal <datei>` auf. Der Befehl zeigt, was sich gegenüber dem gespeicherten Hash geändert hat (Anzahl Aktionen und Übersprungene, alter und neuer Hash gekürzt), fragt nach und schreibt den neuen Hash. Ein v1/v2-Plan wird dabei auf v3 gehoben.
5. In der GUI-Review abgewählte Einträge ergeben wie bisher einen Teilplan (`save_subset`, neue Datei `<stem>-auswahl-<Zeitstempel>.json`); dieser bekommt beim Speichern seinen eigenen Hash. Das Original bleibt unverändert.
6. Die GUI merkt sich beim Laden den Hash der Datei. Beim Anwenden prüft sie die Datei auf der Platte erneut; hat sie sich seit dem Laden geändert, bricht sie mit Meldung ab und bietet „Plan neu laden“ an.

## Acceptance Criteria

- [ ] Given ein frisch erzeugter Plan, when er gespeichert wird, then hat er `version: 3` und ein gültiges `integrity` (`sha256:<64 Hex-Zeichen>`).
- [ ] Given ein v3-Plan, when nur Einrückung, Zeilenenden (CRLF/LF) oder die Reihenfolge der JSON-Schlüssel geändert werden, then besteht die Prüfung (Hash über kanonisches JSON).
- [ ] Given ein v3-Plan, when ein Wert geändert, eine Aktion entfernt oder hinzugefügt wird, then bricht `apply` vor jeder Dateioperation ab, Exit-Code `3`, kein Eintrag in `runs`.
- [ ] Given ein v3-Plan ohne Feld `integrity` oder mit falsch formatiertem Wert, when er geladen wird, then ist er ungültig (wie heute bei Strukturfehlern).
- [ ] Given ein v1- oder v2-Plan, when `apply` läuft, then erscheint die Warnung „ungeschützter Plan“ und der Lauf geht weiter wie bisher.
- [ ] Given ein von Hand geänderter v3-Plan, when `plan seal <datei>` mit Bestätigung läuft, then trägt die Datei den neuen Hash und `apply` läuft durch; ohne Bestätigung bleibt die Datei unverändert.
- [ ] Given ein strukturell ungültiger Plan, when `plan seal` läuft, then wird nichts versiegelt (erst `validate`, dann Hash).
- [ ] Given die GUI-Review mit Teilauswahl, when angewendet wird, then hat der gespeicherte Teilplan einen gültigen eigenen Hash.
- [ ] Given ein in der GUI geladener Plan, when die Datei danach auf der Platte geändert wird und der Nutzer „Anwenden“ drückt, then bricht die GUI mit Meldung ab und nichts wird ausgeführt.
- [ ] Given `undo`, `purge`, `runs`, then ist ihr Verhalten unverändert.
- [ ] Die Rundreise-Tests (`tests/roundtrip_random.rs`) und alle bestehenden Tests bleiben grün.

## Edge Cases & Error States

| Scenario | Expected Behavior |
|----------|-------------------|
| Zeilenenden oder Einrückung geändert (Editor-Formatierung) | Kein Fehlalarm: Hash über den geparsten, neu serialisierten Plan |
| Feld `integrity` aus einem v3-Plan entfernt | Ungültiger Plan („v3 braucht integrity“), kein stilles Herabstufen auf v2 |
| `version` von 3 auf 2 geändert und `integrity` entfernt | Wird als v2 behandelt (Warnung). Bekannte Lücke, durch Out-of-Scope (kein Angreifer) gedeckt; in der Doku erwähnt |
| `integrity` falsch formatiert (kein `sha256:`, falsche Länge, kein Hex) | Ungültiger Plan |
| Hash weicht ab | Abbruch vor jeder Dateioperation, Meldung mit erwartetem und gefundenem Hash (je 12 Zeichen), keine Pfade, Exit-Code `3`, kein Journal |
| Teilplan aus der GUI-Review | Neue Datei mit eigenem Hash; Original unverändert |
| Datei ändert sich zwischen Laden in der GUI und „Anwenden“ | Abbruch mit Meldung, Angebot „Plan neu laden“ |
| `plan seal` auf einem Plan mit korrektem Hash | Meldung „Plan ist bereits versiegelt“, Datei unverändert, Exit-Code `0` |
| `plan seal` auf v1/v2-Plan | Plan wird auf v3 gehoben und versiegelt (nach Bestätigung) |
| `plan seal` ohne Terminal (keine Rückfrage möglich) | Wie `apply`: Abbruch ohne `--yes`, mit `--yes` ohne Rückfrage |
| Geplante Läufe (Phase 5) | Erzeugen v3-Pläne; kein Auto-Apply, sonst keine Auswirkung |
| Plan einer neueren Version (> 3) | Wie heute: `UnsupportedVersion` |

## Technical Constraints

- **Hash-Verfahren:** SHA-256 über das kanonische JSON des Plans ohne das Feld `integrity`. Kanonisch heißt: Datei parsen, `integrity` leeren, mit `serde_json::to_string` (kompakt, Feldreihenfolge der Struktur, `BTreeMap` sortiert) serialisieren, die UTF-8-Bytes hashen. Gespeichert als `sha256:<hex>`.
- **Neue Abhängigkeit:** Crate `sha2` (rein Rust). Muss die CI-Sperre in `.github/workflows/ci.yml` (keine HTTP-/TLS-Crates) bestehen; vor dem Einbau prüfen.
- **Formatversion:** `PLAN_VERSION` wird `3`, `MIN_PLAN_VERSION` bleibt `1`. Bei v3 ist `integrity` Pflicht, bei v1/v2 nicht erlaubt (falls doch vorhanden: ungültig).
- **Exit-Codes:** neuer Code `3` „Plan wurde verändert (Integritätsprüfung fehlgeschlagen)“ in `dokumentation.md` §11.
- **Performance:** Kein eigenes Ziel; Parsen und Hashen ist gegenüber dem Apply vernachlässigbar. `tests/perf_plans.rs` misst das Laden großer Pläne mit Prüfung mit.
- **Security:** Keine Netzwerkzugriffe, keine neuen Rechte. Meldungen nennen gekürzte Hashes, keine Pfade aus dem Plan.
- **Compliance:** keine Anforderungen.

## Dependencies

- `src/change/plan.rs`: Feld `integrity`, Formatversion 3, Berechnung beim `save`, Prüfung in `from_json`/`load`, Fehler in `PlanError`.
- Alle Planer (`dedupe`, `dedupe_dirs`, `junk`, `empty_dirs`, `archive`, `versions`, `rules`) und `ops::plan::save_subset`: setzen den Hash beim Speichern.
- `ops/apply`, CLI `apply`: Prüfung, Warnung bei v1/v2, Exit-Code `3`.
- CLI: neuer Unterbefehl `plan seal <datei> [--yes]`.
- GUI: gemerkter Hash beim Laden, erneute Prüfung beim Anwenden (`gui/views/apply.rs`), Warnung bei ungeschützten Plänen, ein Satz in der Hilfe zu „Plan prüfen“ (`gui/help.rs`).
- Doku: `docs/dokumentation.md` (Plan-Format, `plan seal`, Exit-Code `3`), README falls dort Befehle gelistet sind, `docs/manual-tests.md` (neuer Abschnitt), `docs/roadmap.md` (Status).

## Open Questions

- [ ] Name des Feldes: `integrity` (Vorschlag) oder `plan_hash`?
- [ ] Soll die Warnung bei v1/v2-Plänen in der GUI ein Hinweis in der Review sein oder ein Dialog vor dem Anwenden? (Vorschlag: Hinweis in der Review-Kopfzeile, kein zusätzlicher Dialog.)
