//! Farben der Oberfläche als Tokens für hell und dunkel. Außerhalb dieses Moduls steht kein
//! `Color32::` mehr; die Ansichten fragen nach einem [`Tone`].

use eframe::egui::{self, Color32};

use super::format::Tone;

/// Hell, dunkel oder dem Windows-Modus folgen (egui verfolgt ihn selbst).
pub type ThemeChoice = egui::ThemePreference;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub ok: Color32,
    pub warn: Color32,
    pub error: Color32,
    pub muted: Color32,
    pub accent: Color32,
    /// Füllung gefährlicher Knöpfe (endgültiges Löschen); weißer Text darauf
    pub danger_fill: Color32,
}

impl Palette {
    pub fn tone(&self, tone: Tone) -> Color32 {
        match tone {
            Tone::Ok => self.ok,
            Tone::Warn => self.warn,
            Tone::Error => self.error,
            Tone::Muted => self.muted,
        }
    }
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            ok: Color32::from_rgb(110, 200, 120),
            warn: Color32::from_rgb(235, 190, 70),
            error: Color32::from_rgb(255, 120, 110),
            muted: Color32::from_rgb(150, 150, 150),
            accent: Color32::from_rgb(110, 170, 255),
            danger_fill: Color32::from_rgb(160, 40, 40),
        }
    } else {
        Palette {
            ok: Color32::from_rgb(22, 110, 45),
            warn: Color32::from_rgb(140, 90, 0),
            error: Color32::from_rgb(185, 35, 35),
            muted: Color32::from_rgb(95, 95, 95),
            accent: Color32::from_rgb(20, 85, 175),
            danger_fill: Color32::from_rgb(160, 40, 40),
        }
    }
}

/// Farbe eines Tons passend zum aktuellen Modus der Oberfläche.
pub fn tone_color(ui: &egui::Ui, tone: Tone) -> Color32 {
    palette(ui.visuals().dark_mode).tone(tone)
}

/// Übernimmt die gewählte Darstellung.
pub fn apply(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_theme(choice);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    fn all(p: &Palette) -> [(&'static str, Color32); 5] {
        [
            ("ok", p.ok),
            ("warn", p.warn),
            ("error", p.error),
            ("muted", p.muted),
            ("accent", p.accent),
        ]
    }

    #[test]
    fn farben_sind_in_beiden_modi_gut_lesbar() {
        for (dark, background) in [
            (true, egui::Visuals::dark().panel_fill),
            (false, egui::Visuals::light().panel_fill),
        ] {
            for (name, color) in all(&palette(dark)) {
                let ratio = contrast(color, background);
                assert!(ratio >= 4.5, "{name} (dark={dark}): Kontrast {ratio:.2}");
            }
        }
    }

    #[test]
    fn jeder_ton_hat_eine_eigene_farbe() {
        for dark in [true, false] {
            let p = palette(dark);
            let tones = [Tone::Ok, Tone::Warn, Tone::Error, Tone::Muted].map(|t| p.tone(t));
            for (i, a) in tones.iter().enumerate() {
                for b in &tones[i + 1..] {
                    assert_ne!(a, b, "dark={dark}");
                }
            }
        }
    }

    #[test]
    fn hell_und_dunkel_unterscheiden_sich() {
        assert_ne!(palette(true), palette(false));
    }

    #[test]
    fn weisser_text_auf_gefahr_knopf_ist_lesbar() {
        for dark in [true, false] {
            let ratio = contrast(Color32::WHITE, palette(dark).danger_fill);
            assert!(ratio >= 4.5, "dark={dark}: {ratio:.2}");
        }
    }
}
