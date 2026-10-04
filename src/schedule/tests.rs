use std::cell::RefCell;
use std::collections::BTreeMap;

use chrono::NaiveDate;

use super::*;

/// Aufgabenplanung im Speicher: Name → (XML, Programm).
#[derive(Default)]
struct Fake {
    tasks: RefCell<BTreeMap<String, (String, PathBuf)>>,
    fail_create: bool,
}

impl TaskService for Fake {
    fn create(&self, name: &str, xml: &str) -> io::Result<()> {
        if self.fail_create {
            return Err(io::Error::other("Zugriff verweigert"));
        }
        let exe = xml::command_from_xml(xml).unwrap_or_default();
        self.tasks
            .borrow_mut()
            .insert(name.to_string(), (xml.to_string(), exe));
        Ok(())
    }

    fn delete(&self, name: &str) -> io::Result<()> {
        self.tasks.borrow_mut().remove(name);
        Ok(())
    }

    fn query_command(&self, name: &str) -> io::Result<Option<PathBuf>> {
        Ok(self.tasks.borrow().get(name).map(|(_, exe)| exe.clone()))
    }
}

fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(h, min, 0)
        .unwrap()
}

fn setup() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let register = dir.path().join("schedules.json");
    (dir, register)
}

const EXE: &str = r"C:\Programme\oc\ordner-cleanup-bg.exe";

fn add_default(svc: &Fake, register: &Path, profile: &str, trigger: &Trigger) -> Added {
    add(
        svc,
        register,
        profile,
        trigger,
        Path::new(EXE),
        None,
        at(2026, 10, 4, 12, 0),
    )
    .unwrap()
}

#[test]
fn zeiten_und_wochentage_werden_geparst() {
    assert_eq!(
        parse_time("09:00").unwrap(),
        NaiveTime::from_hms_opt(9, 0, 0).unwrap()
    );
    assert_eq!(
        parse_time("9:05").unwrap(),
        NaiveTime::from_hms_opt(9, 5, 0).unwrap()
    );
    for bad in ["", "9", "25:00", "09:60", "09:5", "ab:cd", "09:00:00"] {
        assert!(parse_time(bad).is_err(), "{bad}");
    }
    assert_eq!(parse_day("mo").unwrap(), Weekday::Mon);
    assert_eq!(parse_day("So").unwrap(), Weekday::Sun);
    assert_eq!(parse_day("thu").unwrap(), Weekday::Thu);
    let err = parse_day("xx").unwrap_err();
    assert!(err.contains("MO"), "{err}");
}

#[test]
fn naechster_lauf_taeglich() {
    let t = Trigger::daily("09:00").unwrap();
    assert_eq!(t.next_run(at(2026, 10, 4, 8, 0)), at(2026, 10, 4, 9, 0));
    assert_eq!(t.next_run(at(2026, 10, 4, 9, 0)), at(2026, 10, 4, 9, 0));
    assert_eq!(t.next_run(at(2026, 10, 4, 9, 1)), at(2026, 10, 5, 9, 0));
    assert_eq!(t.next_run(at(2026, 12, 31, 23, 0)), at(2027, 1, 1, 9, 0));
}

#[test]
fn naechster_lauf_woechentlich() {
    // 2026-10-04 ist ein Sonntag
    let monday = Trigger::weekly("MO", "09:00").unwrap();
    assert_eq!(
        monday.next_run(at(2026, 10, 4, 12, 0)),
        at(2026, 10, 5, 9, 0)
    );
    assert_eq!(
        monday.next_run(at(2026, 10, 5, 8, 59)),
        at(2026, 10, 5, 9, 0)
    );
    assert_eq!(
        monday.next_run(at(2026, 10, 5, 9, 1)),
        at(2026, 10, 12, 9, 0)
    );
    let sunday = Trigger::weekly("SO", "13:00").unwrap();
    assert_eq!(
        sunday.next_run(at(2026, 10, 4, 12, 0)),
        at(2026, 10, 4, 13, 0)
    );
    assert_eq!(
        sunday.next_run(at(2026, 10, 4, 14, 0)),
        at(2026, 10, 11, 13, 0)
    );
    let saturday = Trigger::weekly("SA", "09:00").unwrap();
    assert_eq!(
        saturday.next_run(at(2026, 10, 4, 12, 0)),
        at(2026, 10, 10, 9, 0)
    );
}

#[test]
fn beschreibung_ist_lesbar() {
    assert_eq!(
        Trigger::weekly("MO", "09:00").unwrap().describe(),
        "wöchentlich Mo 09:00"
    );
    assert_eq!(Trigger::daily("07:30").unwrap().describe(), "täglich 07:30");
}

#[test]
fn registertrigger_rundreise() {
    for t in [
        Trigger::weekly("FR", "18:15").unwrap(),
        Trigger::daily("06:00").unwrap(),
    ] {
        assert_eq!(StoredTrigger::from(&t).to_trigger().unwrap(), t);
    }
}

#[test]
fn add_legt_aufgabe_und_registereintrag_an() {
    let (_dir, register) = setup();
    let svc = Fake::default();
    let trigger = Trigger::weekly("MO", "09:00").unwrap();
    let added = add_default(&svc, &register, "downloads", &trigger);
    assert!(!added.replaced);
    assert_eq!(added.next_run, at(2026, 10, 5, 9, 0));
    let tasks = svc.tasks.borrow();
    let (xml, exe) = &tasks[r"\ordner-cleanup\downloads"];
    assert!(xml.contains("<StartBoundary>2026-10-05T09:00:00</StartBoundary>"));
    assert_eq!(exe, Path::new(EXE));
    let entries = load_register(&register).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].profile, "downloads");
    assert_eq!(entries[0].exe, EXE);
}

#[test]
fn add_fuer_dasselbe_profil_ersetzt_statt_doppelt_anzulegen() {
    let (_dir, register) = setup();
    let svc = Fake::default();
    add_default(
        &svc,
        &register,
        "downloads",
        &Trigger::weekly("MO", "09:00").unwrap(),
    );
    let again = add_default(
        &svc,
        &register,
        "downloads",
        &Trigger::daily("07:00").unwrap(),
    );
    assert!(again.replaced);
    assert_eq!(svc.tasks.borrow().len(), 1);
    let entries = load_register(&register).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].trigger,
        StoredTrigger::Daily {
            time: "07:00".into()
        }
    );
}

#[test]
fn add_meldet_ersetzen_auch_wenn_nur_die_aufgabe_existiert() {
    let (_dir, register) = setup();
    let svc = Fake::default();
    svc.tasks
        .borrow_mut()
        .insert(task_name("p"), (String::new(), PathBuf::from(EXE)));
    let added = add_default(&svc, &register, "p", &Trigger::daily("07:00").unwrap());
    assert!(added.replaced);
}

#[test]
fn fehler_beim_anlegen_laesst_das_register_unberuehrt() {
    let (_dir, register) = setup();
    let svc = Fake {
        fail_create: true,
        ..Fake::default()
    };
    let err = add(
        &svc,
        &register,
        "p",
        &Trigger::daily("07:00").unwrap(),
        Path::new(EXE),
        None,
        at(2026, 10, 4, 12, 0),
    )
    .unwrap_err();
    assert!(err.to_string().contains("Zugriff verweigert"));
    assert!(!register.exists());
}

#[test]
fn mehrere_profile_sind_sortiert_im_register() {
    let (_dir, register) = setup();
    let svc = Fake::default();
    let t = Trigger::daily("07:00").unwrap();
    add_default(&svc, &register, "zeta", &t);
    add_default(&svc, &register, "alpha", &t);
    let names: Vec<String> = load_register(&register)
        .unwrap()
        .into_iter()
        .map(|e| e.profile)
        .collect();
    assert_eq!(names, ["alpha", "zeta"]);
}

#[test]
fn remove_entfernt_aufgabe_und_eintrag() {
    let (_dir, register) = setup();
    let svc = Fake::default();
    let t = Trigger::daily("07:00").unwrap();
    add_default(&svc, &register, "a", &t);
    add_default(&svc, &register, "b", &t);
    assert!(remove(&svc, &register, "a").unwrap());
    assert!(svc.tasks.borrow().get(&task_name("a")).is_none());
    let left: Vec<String> = load_register(&register)
        .unwrap()
        .into_iter()
        .map(|e| e.profile)
        .collect();
    assert_eq!(left, ["b"]);
    assert!(
        !remove(&svc, &register, "a").unwrap(),
        "zweites Entfernen findet nichts"
    );
}

#[test]
fn remove_raeumt_auch_eine_verwaiste_aufgabe_ab() {
    let (_dir, register) = setup();
    let svc = Fake::default();
    svc.tasks
        .borrow_mut()
        .insert(task_name("p"), (String::new(), PathBuf::from(EXE)));
    assert!(remove(&svc, &register, "p").unwrap());
    assert!(svc.tasks.borrow().is_empty());
}

#[test]
fn list_meldet_fehlende_aufgabe_und_fehlendes_programm() {
    let (_dir, register) = setup();
    let svc = Fake::default();
    let t = Trigger::daily("07:00").unwrap();
    add_default(&svc, &register, "ok", &t);
    add_default(&svc, &register, "gone", &t);
    add_default(&svc, &register, "moved", &t);
    svc.tasks.borrow_mut().remove(&task_name("gone"));
    let exists = |p: &Path| p == Path::new(r"C:\da.exe");
    // nur „ok“ zeigt auf ein vorhandenes Programm
    svc.tasks.borrow_mut().get_mut(&task_name("ok")).unwrap().1 = PathBuf::from(r"C:\da.exe");
    let rows = list(&svc, &register, &exists).unwrap();
    let state = |name: &str| {
        rows.iter()
            .find(|r| r.entry.profile == name)
            .unwrap()
            .state
            .clone()
    };
    assert_eq!(state("ok"), TaskState::Ok);
    assert_eq!(state("gone"), TaskState::TaskMissing);
    assert_eq!(
        state("moved"),
        TaskState::ProgramMissing(PathBuf::from(EXE))
    );
}

#[test]
fn list_traut_dem_registrierten_pfad_wenn_die_abfrage_zeichen_verfaelscht() {
    let (_dir, register) = setup();
    let svc = Fake::default();
    add_default(&svc, &register, "p", &Trigger::daily("07:00").unwrap());
    svc.tasks.borrow_mut().get_mut(&task_name("p")).unwrap().1 =
        PathBuf::from(r"C:\Users\M?ller\bg.exe");
    let rows = list(&svc, &register, &|p: &Path| p == Path::new(EXE)).unwrap();
    assert_eq!(rows[0].state, TaskState::Ok);
}

#[test]
fn leeres_oder_fehlendes_register_ist_leer_beschaedigtes_ein_fehler() {
    let (dir, register) = setup();
    assert!(load_register(&register).unwrap().is_empty());
    std::fs::write(dir.path().join("schedules.json"), "{kaputt").unwrap();
    let err = load_register(&register).unwrap_err();
    assert!(err.to_string().contains("beschädigt"), "{err}");
}
