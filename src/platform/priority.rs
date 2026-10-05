//! Prozesspriorität „unter normal“ für lange Läufe (`classify`), damit der Rechner
//! bedienbar bleibt. Die alte Klasse wird beim Verwerfen des Wächters wiederhergestellt.

use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetPriorityClass, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS,
};

pub struct ProcessPriorityGuard {
    previous: u32,
}

impl ProcessPriorityGuard {
    /// Senkt die Priorität. Scheitert das Setzen, bleibt alles wie es war (kein Fehler:
    /// die Priorität ist nur eine Annehmlichkeit).
    pub fn below_normal() -> Self {
        // SAFETY: Pseudo-Handle des eigenen Prozesses, kein Freigeben nötig.
        let previous = unsafe { GetPriorityClass(GetCurrentProcess()) };
        if previous != 0 && previous != BELOW_NORMAL_PRIORITY_CLASS {
            // SAFETY: wie oben.
            unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) };
        }
        Self { previous }
    }

    #[cfg(test)]
    fn current() -> u32 {
        // SAFETY: wie oben.
        unsafe { GetPriorityClass(GetCurrentProcess()) }
    }
}

impl Drop for ProcessPriorityGuard {
    fn drop(&mut self) {
        if self.previous != 0 {
            // SAFETY: wie oben.
            unsafe { SetPriorityClass(GetCurrentProcess(), self.previous) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn senkt_und_stellt_wieder_her() {
        let before = ProcessPriorityGuard::current();
        {
            let _guard = ProcessPriorityGuard::below_normal();
            assert_eq!(ProcessPriorityGuard::current(), BELOW_NORMAL_PRIORITY_CLASS);
        }
        assert_eq!(ProcessPriorityGuard::current(), before);
    }
}
