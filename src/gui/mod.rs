//! Grafische Oberfläche (egui). Die Logik (Tasks, Review-Modell, Formulare) ist fensterfrei und
//! einzeln testbar; `app` und `views` zeichnen sie nur.

pub mod app;
pub mod fonts;
pub mod format;
pub mod header;
pub mod result;
pub mod review;
pub mod shell;
pub mod tasks;
pub mod texts;
pub mod theme;
pub mod views;
pub mod widgets;

use eframe::egui;

/// Fehlerprotokoll: die GUI-exe hat keine Konsole.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(path) = crate::paths::gui_error_log() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                use std::io::Write;
                let _ = writeln!(
                    file,
                    "{} {info}",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
                );
            }
        }
        previous(info);
    }));
}

fn options(renderer: eframe::Renderer) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(texts::TITLE)
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([820.0, 520.0]),
        renderer,
        persist_window: true,
        ..Default::default()
    }
}

fn run_with(renderer: eframe::Renderer) -> eframe::Result {
    eframe::run_native(
        texts::TITLE,
        options(renderer),
        Box::new(|cc| Ok(Box::new(app::GuiApp::new(cc)))),
    )
}

/// Zeigt eine Meldung, wenn sich kein Fenster öffnen lässt (die exe hat keine Konsole).
fn fatal_message(text: &str) {
    use std::os::windows::ffi::OsStrExt;
    let wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let (text, title) = (wide(text), wide(texts::TITLE));
    // SAFETY: nullterminierte Puffer, die den Aufruf überdauern.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            0x10, // MB_ICONERROR
        );
    }
}

/// Startet die Oberfläche: `wgpu` (DX12), bei Fehlschlag einmal `glow`.
pub fn run() -> anyhow::Result<()> {
    install_panic_hook();
    if let Err(first) = run_with(eframe::Renderer::Wgpu) {
        if let Err(second) = run_with(eframe::Renderer::Glow) {
            let text = format!(
                "Das Fenster konnte nicht geöffnet werden.\n\n{first}\n{second}\n\n\
                 Die Kommandozeile (ordner-cleanup.exe) funktioniert weiterhin."
            );
            fatal_message(&text);
            anyhow::bail!(text);
        }
    }
    Ok(())
}
