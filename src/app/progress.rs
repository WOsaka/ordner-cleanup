//! Fortschrittsbalken im Terminal: liest den `TaskProgress` einer laufenden Operation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

use crate::ops::TaskProgress;

/// Führt `f` aus und zeichnet währenddessen einen Balken aus `progress` (indicatif zeichnet nur
/// in einem Terminal; geplante Läufe bleiben still).
pub(super) fn with_bar<T>(progress: &TaskProgress, f: impl FnOnce() -> T) -> T {
    let finished = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| draw(progress, &finished));
        let result = f();
        finished.store(true, Ordering::Relaxed);
        result
    })
}

/// Wie [`with_bar`], aber als Spinner mit den Zählern eines Scans.
pub(super) fn with_spinner<T>(progress: &TaskProgress, f: impl FnOnce() -> T) -> T {
    let finished = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| draw_spinner(progress, &finished));
        let result = f();
        finished.store(true, Ordering::Relaxed);
        result
    })
}

fn draw_spinner(progress: &TaskProgress, finished: &AtomicBool) {
    let bar = ProgressBar::new_spinner();
    bar.set_style(
        ProgressStyle::with_template("{spinner} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    while !finished.load(Ordering::Relaxed) {
        bar.set_message(format!(
            "{} Dateien, {} Ordner, {}, {} Fehler – {}",
            progress.done.load(Ordering::Relaxed),
            progress.dirs.load(Ordering::Relaxed),
            bytesize::ByteSize::b(progress.bytes.load(Ordering::Relaxed)),
            progress.errors.load(Ordering::Relaxed),
            progress.current()
        ));
        bar.tick();
        std::thread::sleep(Duration::from_millis(200));
    }
    bar.finish_and_clear();
}

fn draw(progress: &TaskProgress, finished: &AtomicBool) {
    let bar = ProgressBar::new(0);
    bar.set_style(
        ProgressStyle::with_template("{bar:30} {pos}/{len} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_bar()),
    );
    while !finished.load(Ordering::Relaxed) {
        bar.set_length(progress.total.load(Ordering::Relaxed));
        bar.set_position(progress.done.load(Ordering::Relaxed));
        bar.set_message(progress.current());
        std::thread::sleep(Duration::from_millis(100));
    }
    bar.finish_and_clear();
}
