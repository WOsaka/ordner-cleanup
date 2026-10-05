//! Task-Runner: führt Operationen in Worker-Threads aus, mit Abbruch-Flag, gemeinsamem
//! Fortschritt und `catch_unwind`. Kennt egui nicht; die Oberfläche übergibt einen Rückruf, der
//! ein Neuzeichnen anstößt.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::ops::{Error, OpCtx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// Liest nur; beliebig viele gleichzeitig
    Read,
    /// Verändert Index, Dateien oder Journal; höchstens einer gleichzeitig
    Write,
}

pub type TaskId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    /// Fehlertext (Ketten mit `: `)
    Failed(String),
    /// Ein anderer Scan hält die Sperre
    Busy,
    /// Der Worker ist abgestürzt (Panic)
    Panicked(String),
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(text) => f.write_str(text),
            Self::Busy => f.write_str("Ein anderer Scan läuft"),
            Self::Panicked(text) => write!(f, "Interner Fehler: {text}"),
        }
    }
}

pub struct Finished {
    pub id: TaskId,
    pub name: String,
    pub result: Result<Box<dyn Any + Send>, TaskError>,
}

impl Finished {
    /// Das Ergebnis als `T`; ein falscher Typ ist ein Fehler, keine Panic.
    pub fn take<T: 'static>(self) -> Result<T, TaskError> {
        match self.result?.downcast::<T>() {
            Ok(value) => Ok(*value),
            Err(_) => Err(TaskError::Failed(format!(
                "Ergebnis von „{}“ hat einen unerwarteten Typ",
                self.name
            ))),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Rejected;

pub struct RunningInfo {
    pub id: TaskId,
    pub name: String,
    pub kind: TaskKind,
    pub ctx: OpCtx,
}

pub struct TaskRunner {
    wake: Arc<dyn Fn() + Send + Sync>,
    next_id: TaskId,
    running: Vec<RunningInfo>,
    tx: Sender<Finished>,
    rx: Receiver<Finished>,
    /// Bereits abgeholte, aber noch nicht an `poll` übergebene Ergebnisse
    pending: Vec<Finished>,
}

fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unbekannter Fehler".into())
}

fn error_of(e: &anyhow::Error) -> TaskError {
    if matches!(e.downcast_ref::<Error>(), Some(Error::Busy(_))) {
        TaskError::Busy
    } else {
        TaskError::Failed(format!("{e:#}"))
    }
}

impl TaskRunner {
    /// `wake` wird nach jedem beendeten Task aufgerufen (die Oberfläche stößt damit ein
    /// Neuzeichnen an).
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, rx) = channel();
        Self {
            wake,
            next_id: 1,
            running: Vec::new(),
            tx,
            rx,
            pending: Vec::new(),
        }
    }

    /// Startet `f` in einem Worker-Thread. Ein schreibender Task wird abgelehnt, solange ein
    /// anderer schreibender läuft.
    pub fn spawn<T, F>(&mut self, name: &str, kind: TaskKind, f: F) -> Result<TaskId, Rejected>
    where
        T: Send + 'static,
        F: FnOnce(&OpCtx) -> anyhow::Result<T> + Send + 'static,
    {
        self.collect();
        if kind == TaskKind::Write && self.running.iter().any(|r| r.kind == TaskKind::Write) {
            return Err(Rejected);
        }
        let id = self.next_id;
        self.next_id += 1;
        let ctx = OpCtx::default();
        self.running.push(RunningInfo {
            id,
            name: name.to_string(),
            kind,
            ctx: ctx.clone(),
        });
        let (tx, wake, task_name) = (self.tx.clone(), Arc::clone(&self.wake), name.to_string());
        std::thread::spawn(move || {
            let result = match catch_unwind(AssertUnwindSafe(|| f(&ctx))) {
                Ok(Ok(value)) => Ok(Box::new(value) as Box<dyn Any + Send>),
                Ok(Err(e)) => Err(error_of(&e)),
                Err(payload) => Err(TaskError::Panicked(panic_text(&*payload))),
            };
            let _ = tx.send(Finished {
                id,
                name: task_name,
                result,
            });
            wake();
        });
        Ok(id)
    }

    /// Holt fertige Ergebnisse aus dem Kanal in `pending` und räumt `running` auf.
    fn collect(&mut self) {
        let done: Vec<Finished> = self.rx.try_iter().collect();
        self.running.retain(|r| !done.iter().any(|d| d.id == r.id));
        self.pending.extend(done);
    }

    /// Beendete Tasks seit dem letzten Aufruf.
    pub fn poll(&mut self) -> Vec<Finished> {
        self.collect();
        std::mem::take(&mut self.pending)
    }

    pub fn running(&self) -> Vec<&RunningInfo> {
        self.running.iter().collect()
    }

    pub fn cancel_all(&self) {
        for r in &self.running {
            r.ctx.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_idle(&self) -> bool {
        self.running.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    fn runner() -> (TaskRunner, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&wakes);
        let runner = TaskRunner::new(Arc::new(move || {
            w.fetch_add(1, Ordering::Relaxed);
        }));
        (runner, wakes)
    }

    fn wait_for(runner: &mut TaskRunner, n: usize) -> Vec<Finished> {
        let end = Instant::now() + Duration::from_secs(10);
        let mut all = Vec::new();
        while all.len() < n {
            assert!(Instant::now() < end, "Task kam nicht zurück");
            all.extend(runner.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        all
    }

    #[test]
    fn spawn_verwirft_kein_fertiges_ergebnis() {
        let (mut r, wakes) = runner();
        r.spawn("a", TaskKind::Read, |_| Ok(1u32)).unwrap();
        // warten, bis a fertig im Kanal liegt, ohne zu pollen
        let end = Instant::now() + Duration::from_secs(10);
        while wakes.load(Ordering::Relaxed) == 0 {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(5));
        }
        r.spawn("b", TaskKind::Read, |_| Ok(2u32)).unwrap();
        let mut names: Vec<String> = wait_for(&mut r, 2).into_iter().map(|f| f.name).collect();
        names.sort();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn ergebnis_kommt_an_und_der_rueckruf_wird_ausgeloest() {
        let (mut r, wakes) = runner();
        r.spawn("zahl", TaskKind::Read, |_| Ok(42u32)).unwrap();
        let done = wait_for(&mut r, 1).pop().unwrap();
        assert_eq!(done.name, "zahl");
        assert_eq!(done.take::<u32>().unwrap(), 42);
        assert!(wakes.load(Ordering::Relaxed) >= 1);
        assert!(r.is_idle());
    }

    #[test]
    fn fehler_kommen_mit_text_an() {
        let (mut r, _) = runner();
        r.spawn("f", TaskKind::Read, |_| -> anyhow::Result<()> {
            Err(anyhow::anyhow!("kaputt").context("außen"))
        })
        .unwrap();
        let err = wait_for(&mut r, 1).pop().unwrap().take::<()>().unwrap_err();
        assert_eq!(err, TaskError::Failed("außen: kaputt".into()));
    }

    #[test]
    fn belegte_scan_sperre_wird_zu_busy() {
        let (mut r, _) = runner();
        r.spawn("s", TaskKind::Write, |_| -> anyhow::Result<()> {
            Err(crate::ops::Error::Busy(std::path::PathBuf::from("scan.lock")).into())
        })
        .unwrap();
        let err = wait_for(&mut r, 1).pop().unwrap().take::<()>().unwrap_err();
        assert_eq!(err, TaskError::Busy);
    }

    #[test]
    fn abbrechen_setzt_das_flag_des_laufenden_tasks() {
        let (mut r, _) = runner();
        let seen = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&seen);
        r.spawn("lang", TaskKind::Write, move |ctx| {
            while !ctx.is_cancelled() {
                std::thread::sleep(Duration::from_millis(2));
            }
            s.store(true, Ordering::Relaxed);
            Ok(())
        })
        .unwrap();
        assert_eq!(r.running().len(), 1);
        r.cancel_all();
        wait_for(&mut r, 1);
        assert!(seen.load(Ordering::Relaxed));
    }

    #[test]
    fn panic_wird_zu_fehler_und_der_runner_bleibt_nutzbar() {
        let (mut r, _) = runner();
        r.spawn("p", TaskKind::Write, |_| -> anyhow::Result<()> {
            panic!("boom")
        })
        .unwrap();
        let err = wait_for(&mut r, 1).pop().unwrap().take::<()>().unwrap_err();
        match err {
            TaskError::Panicked(text) => assert!(text.contains("boom"), "{text}"),
            other => panic!("{other:?}"),
        }
        assert!(r.is_idle());
        r.spawn("danach", TaskKind::Write, |_| Ok(1u8)).unwrap();
        assert_eq!(wait_for(&mut r, 1).pop().unwrap().take::<u8>().unwrap(), 1);
    }

    #[test]
    fn zweiter_schreibender_task_wird_abgelehnt_lesende_nicht() {
        let (mut r, _) = runner();
        let gate = Arc::new(AtomicBool::new(false));
        let g = Arc::clone(&gate);
        r.spawn("w1", TaskKind::Write, move |_| {
            while !g.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(
            r.spawn("w2", TaskKind::Write, |_| Ok(())).unwrap_err(),
            Rejected
        );
        r.spawn("r1", TaskKind::Read, |_| Ok(())).unwrap();
        gate.store(true, Ordering::Relaxed);
        wait_for(&mut r, 2);
        r.spawn("w3", TaskKind::Write, |_| Ok(())).unwrap();
        wait_for(&mut r, 1);
    }

    #[test]
    fn falscher_typ_beim_entnehmen_ist_ein_fehler_statt_panic() {
        let (mut r, _) = runner();
        r.spawn("t", TaskKind::Read, |_| Ok(1u8)).unwrap();
        let done = wait_for(&mut r, 1).pop().unwrap();
        assert!(matches!(done.take::<String>(), Err(TaskError::Failed(_))));
    }
}
