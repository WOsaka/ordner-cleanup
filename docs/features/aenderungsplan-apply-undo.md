---
title: "Änderungsplan, Apply & Undo (Phase 2)"
status: draft          # draft | approved | implemented
created: 2026-10-03
updated: 2026-10-03
---

# Feature: Änderungsplan, Apply & Undo (Phase 2)

## Problem Statement
Phase 1 liefert nur Erkenntnisse (z. B. exakte Duplikate), ändert aber nichts. Damit spätere Aufräum- und Neuorganisationsfunktionen sicher schreiben dürfen, braucht es ein gemeinsames Sicherheitsfundament: Jede Änderung wird zuerst als prüfbarer Plan beschrieben, erst nach Bestätigung ausgeführt, lückenlos protokolliert und lässt sich vollständig zurückdrehen. Dateien werden dabei nie hart gelöscht.

Betroffen ist zunächst der Entwickler selbst, später Kollegen. Phase 2 blockiert alle weiteren schreibenden Features (Phase 3 bis 5, siehe [`../roadmap.md`](../roadmap.md)).

Neben der Infrastruktur enthält Phase 2 genau **eine Referenzaktion**, um alles Ende-zu-Ende zu testen: exakte Duplikate in die Quarantäne verschieben.

## Goals
- [ ] `plan` erzeugt aus dem Index einen Änderungsplan als JSON-Datei, ohne etwas zu verändern (Dry-Run ist der Standard)
- [ ] `apply <plan>` führt den Plan nach einer Bestätigung aus
- [ ] Nichts wird hart gelöscht: Entfernte Dateien wandern in eine Quarantäne mit Aufbewahrungsfrist
- [ ] Jeder Lauf wird in einem Undo-Journal protokolliert, `undo <run-id>` stellt einen kompletten Lauf wieder her
- [ ] Geschützte Pfade werden nie angefasst
- [ ] OneDrive-Cloud-Platzhalter werden nie gelesen, gehasht oder verschoben
- [ ] Idempotenz: Ein zweiter `apply` desselben Plans ändert nichts
- [ ] Die Referenzaktion „Duplikate in Quarantäne“ funktioniert Ende-zu-Ende

## Out of Scope
- Regel-Engine (YAML), kommt in Phase 4
- Rename-Aktionen
- Weitere Aufräumaktionen aus Phase 3 (leere Ordner, Müll, Archivieren)
- Ersetzen von Duplikaten durch Hardlinks, Archivieren/ZIP
- Interaktives Review (TUI) oder GUI; Review erfolgt über die Plan-Datei
- Windows-Papierkorb als Ziel
- Automatisches Leeren der Quarantäne
- Netzlaufwerke (SMB) und SharePoint-spezifische Behandlung
- Linux/macOS

## User Journey
1. Der Nutzer hat einen Ordner mit `scan` indiziert.
2. Er startet `ordner-cleanup plan dedupe <pfad> [--keep oldest|newest|path:<prefix>] [--out <plan.json>]`.
3. Das System wertet die exakten Duplikatgruppen (Voll-Hash) aus dem Index aus, wählt pro Gruppe die zu behaltende Datei nach der Strategie (Default: älteste) und schreibt einen Plan. Pro Aktion enthält er: Aktionstyp, Quellpfad, Größe, Änderungszeitpunkt, Hash, Grund, Ziel in der Quarantäne. Zusätzlich werden übersprungene Einträge mit Grund aufgeführt (geschützter Pfad, Cloud-Platzhalter, Hardlink auf behaltene Datei usw.).
4. Das System gibt eine Zusammenfassung aus (Anzahl Aktionen, freiwerdende Bytes, Anzahl übersprungener Einträge) und nennt den Pfad der Plan-Datei. Es wurde nichts verändert.
5. Der Nutzer prüft die Plan-Datei (menschenlesbar, JSON).
6. Er startet `ordner-cleanup apply <plan.json> [--yes]`.
7. Das System zeigt eine Zusammenfassung (Anzahl Aktionen, Bytes, betroffene Wurzeln) und fragt einmal `j/N`. Mit `--yes` entfällt die Abfrage.
8. Vor jeder Aktion prüft das System, ob die Datei noch zum Plan passt (Größe, Änderungszeitpunkt, Hash), und schreibt den Journal-Eintrag (Write-ahead). Dann verschiebt es die Datei nach `<wurzel>\.ordner-cleanup\quarantine\<run-id>\…` auf demselben Volume.
9. Am Ende gibt das System eine Zusammenfassung aus (ausgeführt, übersprungen, `stale`, Fehler, freigegebene Bytes) und die `run-id`.
10. Der Nutzer kann mit `ordner-cleanup undo <run-id>` den Lauf zurückdrehen. Das System verschiebt die Dateien aus der Quarantäne an ihren ursprünglichen Pfad zurück.
11. `ordner-cleanup runs` listet alle Läufe (Zeitpunkt, Aktionen, Status, Quarantäne-Größe, Ablaufdatum).
12. `ordner-cleanup purge [--older-than 30d] [--yes]` löscht abgelaufene Quarantäne-Läufe endgültig. Das passiert nie automatisch.

### Plan-Format (Skizze)
```json
{
  "version": 1,
  "created": "2026-10-03T12:00:00+02:00",
  "kind": "dedupe",
  "root": "C:\\Users\\<user>\\Dokumente",
  "actions": [
    {
      "id": 1,
      "type": "quarantine",
      "path": "C:\\...\\kopie.docx",
      "size": 12345,
      "mtime": "2026-01-05T10:00:00+01:00",
      "hash": "blake3:…",
      "keep": "C:\\...\\original.docx",
      "reason": "exact-duplicate"
    }
  ],
  "skipped": [
    { "path": "C:\\...\\x.docx", "reason": "cloud-placeholder" }
  ]
}
```

### Journal und Quarantäne
- Journal: JSON Lines pro Lauf, append-only, Eintrag **vor** der Aktion (Write-ahead) und Ergebniseintrag danach. Ablage neben der Quarantäne unter `<wurzel>\.ordner-cleanup\journal\<run-id>.jsonl`.
- Quarantäne: pro Laufwerk/Wurzel `<wurzel>\.ordner-cleanup\quarantine\<run-id>\`, der ursprüngliche relative Pfad bleibt erhalten. Aufbewahrung 30 Tage, per Config änderbar.
- Der Ordner `.ordner-cleanup` ist selbst von Scan, Plan und Apply ausgeschlossen.

### Geschützte Pfade
- Immer tabu und **nicht abschaltbar**: `C:\Windows`, `Program Files`, `Program Files (x86)`, `ProgramData`.
- Per Default tabu und **konfigurierbar**: `AppData`, `.git`, `node_modules` sowie Projektordner mit Marker-Dateien (z. B. `Cargo.toml`, `package.json`, `.sln`).
- Der Nutzer kann über die Config weitere Pfade hinzufügen. Eine Aktion auf einem geschützten Pfad wird nicht in den Plan aufgenommen und beim Apply nochmals geprüft und verweigert.

## Acceptance Criteria
- [x] Given ein indizierter Ordner mit Duplikaten, when `plan dedupe` läuft, then entsteht eine Plan-Datei und im Ordner ändert sich kein Byte
- [x] Given eine Duplikatgruppe, when `plan dedupe --keep oldest` läuft, then bleibt pro Gruppe genau eine Datei (die älteste) unberührt und alle anderen stehen als `quarantine`-Aktion im Plan
- [x] Given ein Plan, when `apply` ohne `--yes` läuft, then erscheint eine Zusammenfassung und eine einmalige j/N-Abfrage, und bei „N“ oder leerer Eingabe wird nichts verändert
- [x] Given ein bestätigter Apply, when er durchläuft, then liegen die Dateien unter der Quarantäne, die behaltenen Dateien sind unverändert, und ein Journal mit passender `run-id` existiert
- [x] Given ein abgeschlossener Lauf, when `undo <run-id>` läuft, then ist der Ausgangszustand byteidentisch wiederhergestellt (Pfad, Inhalt, Änderungszeitpunkt)
- [x] Given ein bereits ausgeführter Plan, when `apply` erneut läuft, then ändert sich nichts und die Aktionen werden als „bereits erledigt“ gemeldet (Idempotenz)
- [x] Given eine Datei, die sich seit `plan` geändert hat, when `apply` läuft, then wird nur diese Aktion als `stale` übersprungen, die übrigen werden ausgeführt, und der Exit-Code zeigt Teilerfolg
- [x] Given ein Pfad unter einem geschützten Ordner, when er in einem (manipulierten) Plan steht, then verweigert `apply` die Aktion und meldet den Grund
- [ ] Given eine OneDrive-Cloud-only-Datei, when `plan` oder `apply` läuft, then wird sie weder gelesen noch gehasht noch verschoben und erscheint als übersprungen (Teilweise belegt: Unit-Test mit simuliertem Platzhalter und OFFLINE-Attribut am echten Dateisystem; echter OneDrive-Platzhalter nicht geprüft, siehe „Manueller Test“)
- [x] Given ein Abbruch mitten im Lauf (Prozess beendet), when `undo <run-id>` läuft, then stellt es anhand des Write-ahead-Journals alle bereits verschobenen Dateien wieder her und ignoriert noch nicht begonnene Aktionen
- [x] Given eine Quarantäne-Datei, deren Ursprungspfad inzwischen belegt ist, when `undo` läuft, then wird nichts überschrieben, die Kollision wird gemeldet und die Datei bleibt in der Quarantäne
- [x] Given abgelaufene Quarantäne-Läufe, when `purge` läuft, then werden nur diese nach Bestätigung endgültig gelöscht; jüngere Läufe bleiben erhalten
- [x] `runs` listet alle Läufe mit Status; `cargo clippy -- -D warnings` und `cargo test` sind grün

## Edge Cases & Error States
| Scenario | Expected Behavior |
|----------|-------------------|
| Datei zwischen `plan` und `apply` geändert (Größe/mtime/Hash) | Aktion als `stale` überspringen, melden, Rest ausführen, Exit-Code Teilerfolg |
| Behaltene Datei fehlt beim Apply | Alle Aktionen dieser Gruppe überspringen (sonst droht Datenverlust), melden |
| Alle Dateien einer Gruppe stehen im Plan (z. B. weil „keep“ geschützt/übersprungen ist) | Gruppe nicht in den Plan aufnehmen, mindestens eine Kopie bleibt immer erhalten |
| Duplikate sind Hardlinks derselben Datei | Nicht als Platzgewinn zählen und nicht aufnehmen |
| Gesperrte Datei (von anderem Prozess geöffnet) | Aktion fehlgeschlagen, Journal vermerkt Fehler, Lauf geht weiter |
| Keine Schreibrechte | Aktion fehlgeschlagen, melden; Rechteprüfung vor dem Lauf, wo möglich |
| Pfad länger als 260 Zeichen | `\\?\`-Pfade verwenden; Quarantäne-Ziel zu lang → Aktion überspringen und melden |
| Quarantäne-Ziel existiert schon | Nie überschreiben, eindeutigen Namen bilden bzw. Aktion mit Fehler abbrechen |
| Symlink/Junction | Nicht betreten, nicht verschieben, als übersprungen führen |
| Cloud-only-Platzhalter (OneDrive) | Nie anfassen, im Plan als `skipped: cloud-placeholder` |
| Datei in Gruppe teils Platzhalter | Platzhalter nie als „keep“ oder Löschkandidat, Gruppe nur anhand lokal vorhandener Dateien bewerten |
| Plan-Datei beschädigt oder unbekannte `version` | Apply verweigert mit klarer Fehlermeldung |
| Undo eines bereits zurückgedrehten Laufs | Meldet „bereits zurückgedreht“, ändert nichts |
| Undo: Ursprungspfad belegt | Nicht überschreiben, Kollision melden, Datei bleibt in Quarantäne |
| Quarantäne bereits per `purge` gelöscht | Undo meldet „nicht mehr wiederherstellbar“ |
| Prozessabbruch mitten im Lauf | Write-ahead-Journal erlaubt konsistentes Undo; Lauf-Status `unvollständig` in `runs` |
| Zu wenig Platz auf dem Volume | Move auf demselben Volume braucht keinen Zusatzplatz; sonst Fehler vor dem Lauf |
| Ziel liegt auf anderem Volume als die Quelle | Quarantäne wird pro Volume angelegt, kein Kopieren+Löschen über Laufwerksgrenzen |

## Technical Constraints
- Performance: Planerstellung für 100.000 Dateien in unter 10 Sekunden (nur Index-Zugriff). Apply ist durch Dateisystem-Moves begrenzt; Fortschrittsanzeige wie bei `scan`. Moves auf demselben Volume sind schnell (Rename, kein Kopieren).
- Security: Nur Windows 10/11. Keine harten Löschungen außer durch `purge` nach Bestätigung. Pfade werden normalisiert (8.3-Kurznamen, Groß-/Kleinschreibung, `\\?\`), bevor sie gegen geschützte Pfade geprüft werden. Apply prüft Schutzregeln unabhängig vom Plan erneut (manipulierte Pläne). Die Quarantäne liegt auf dem gleichen Volume, sodass keine Daten über Laufwerksgrenzen kopiert werden.
- Compliance: Journal und Quarantäne enthalten Dateipfade und Hashes, aber keine Dateiinhalte außerhalb der Quarantäne-Dateien selbst; alles bleibt lokal. Keine Telemetrie.
- Zuverlässigkeit: Write-ahead-Journal pro Aktion, `fsync` nach jedem Eintrag; Undo muss auch nach Absturz funktionieren.
- Exit-Codes: 0 Erfolg, eigener Code für Teilerfolg (`stale`/Fehler einzelner Aktionen), eigener Code für Abbruch/Fehler. Konkrete Werte im Implementierungsplan; Konsistenz mit den Phase-1-Codes beachten.
- Tests: Integrationstests mit Temp-Ordnern für Plan → Apply → Undo, Absturz-Simulation (Journal abgeschnitten), geschützte Pfade, Stale-Erkennung.

## Dependencies
- Phase 1: Scan, SQLite-Index und exakte Duplikat-Erkennung ([`scan-analyse-bericht.md`](scan-analyse-bericht.md))
- Wiederverwendung der Windows-Besonderheiten aus Phase 1 (Cloud-Attribute, Pfadnormalisierung, Junctions)
- Config-Datei für geschützte Pfade und Aufbewahrungsfrist (neu oder Erweiterung der vorhandenen)

## Open Questions
- [ ] Wo liegt die Config-Datei (neben dem Index in `%LOCALAPPDATA%\ordner-cleanup\` oder pro Wurzel)? Vorschlag: neben dem Index.
- [ ] Soll `apply` zusätzlich prüfen, ob der Index aktuell ist (Hinweis „bitte neu scannen“), oder reicht die Stale-Prüfung pro Datei?
- [ ] Welcher Hash-Algorithmus wird für Plan und Stale-Prüfung verwendet (der aus Phase 1 wiederverwenden)?
- [ ] Soll die Quarantäne unter OneDrive-Wurzeln wegen Sync-Traffic auf einen lokalen, nicht synchronisierten Ort ausweichen? Vorschlag: Warnung, Entscheidung im Implementierungsplan.

## Manueller Test (2026-10-03)

Ausgeführt mit dem Release-Build in einem eigenen Testordner `C:\Users\Oskar\OneDrive\ordner-cleanup-test` (Index und Config isoliert über `ORDNER_CLEANUP_HOME`). Der Ordner wurde danach vollständig entfernt.

| Prüfung | Ergebnis |
|---|---|
| `scan` und `plan dedupe` verändern den Baum nicht (Pfad, Größe, mtime, Attribute, SHA256 vorher/nachher) | bestanden |
| OneDrive-Warnung bei `plan` und `apply` | erscheint |
| `apply --yes`: Duplikate liegen in `.ordner-cleanup\quarantine\<run-id>\…`, Journal vorhanden, `runs` zeigt „vollständig“ | bestanden |
| `undo`: Ausgangszustand byteidentisch (Pfad, Größe, mtime, Attribute, SHA256) | bestanden |
| Zweites `undo` | „bereits zurückgedreht“, Exit 0 |
| `purge`: junger Lauf bleibt, künstlich gealterter Lauf wird gelöscht, Journal bleibt, `undo` danach „nicht mehr wiederherstellbar“ (Exit 2) | bestanden |
| Simulierter Platzhalter (Attribut `OFFLINE`): fehlt im Plan; in einem manipulierten Plan wird er als „Cloud-Platzhalter“ übersprungen (Exit 2), Datei, Attribut, Zeit und Inhalt unverändert | bestanden |

Nicht geprüft:
- **Echter Cloud-only-Platzhalter:** Der OneDrive-Client lief nicht (kein Prozess), daher konnte kein Platzhalter über den Sync-Filtertreiber entstehen. Ob ein echter Platzhalter nie einen Download auslöst, ist damit nur über die Attribut-Erkennung belegt (dieselbe wie in Phase 1, dort manuell geprüft).
- **Interaktive j/N-Abfrage mit echtem Terminal** (nur Unit-Tests) sowie ein echtes Beenden des Prozesses oder Strg+C während `apply`.

Beobachtung: Nach `undo` bleiben leere Ordner unter `quarantine\<run-id>` stehen, bis `purge` läuft.

Performance (Release, 100.000 Dateien, synthetischer Baum): `plan dedupe` 1,3 s (Vorgabe unter 10 s); `apply` von 9.993 Aktionen 24,9 s; `undo` 13,7 s.
