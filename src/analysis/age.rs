use crate::scan::source::TICKS_PER_SEC;

const SECS_PER_DAY: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AgeClass {
    UnderMonth,
    Under6Months,
    UnderYear,
    Under3Years,
    Over3Years,
}

impl AgeClass {
    pub const ALL: [AgeClass; 5] = [
        Self::UnderMonth,
        Self::Under6Months,
        Self::UnderYear,
        Self::Under3Years,
        Self::Over3Years,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::UnderMonth => "< 1 Monat",
            Self::Under6Months => "< 6 Monate",
            Self::UnderYear => "< 1 Jahr",
            Self::Under3Years => "< 3 Jahre",
            Self::Over3Years => "> 3 Jahre",
        }
    }
}

/// Alter in ganzen Tagen; Zeitstempel in der Zukunft ergeben 0.
pub fn age_days(mtime_ticks: i64, now_ticks: i64) -> i64 {
    ((now_ticks - mtime_ticks) / TICKS_PER_SEC / SECS_PER_DAY).max(0)
}

/// Monat = 30 Tage, Jahr = 365 Tage.
pub fn classify(mtime_ticks: i64, now_ticks: i64) -> AgeClass {
    match age_days(mtime_ticks, now_ticks) {
        0..=29 => AgeClass::UnderMonth,
        30..=181 => AgeClass::Under6Months,
        182..=364 => AgeClass::UnderYear,
        365..=1094 => AgeClass::Under3Years,
        _ => AgeClass::Over3Years,
    }
}

/// Parst `--old-after`: Zahl plus `d` (Tage), `m` (Monate zu 30 Tagen) oder `y` (Jahre zu 365 Tagen).
pub fn parse_old_after(text: &str) -> Result<i64, String> {
    let text = text.trim();
    let err = || format!("Ungültige Dauer '{text}' (erwartet z. B. 90d, 18m, 1y)");
    let unit = text.chars().last().ok_or_else(err)?;
    let factor = match unit.to_ascii_lowercase() {
        'd' => 1,
        'm' => 30,
        'y' => 365,
        _ => return Err(err()),
    };
    let number: i64 = text[..text.len() - 1].parse().map_err(|_| err())?;
    if number <= 0 {
        return Err(err());
    }
    Ok(number * factor)
}

/// Älter als der Schwellwert (in Tagen); genau am Schwellwert gilt noch nicht als alt.
pub fn is_old(mtime_ticks: i64, now_ticks: i64, threshold_days: i64) -> bool {
    age_days(mtime_ticks, now_ticks) > threshold_days
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const NOW: i64 = 1_700_000_000 * TICKS_PER_SEC;

    fn days_ago(days: i64) -> i64 {
        NOW - days * SECS_PER_DAY * TICKS_PER_SEC
    }

    #[rstest]
    #[case(0, AgeClass::UnderMonth)]
    #[case(29, AgeClass::UnderMonth)]
    #[case(30, AgeClass::Under6Months)]
    #[case(181, AgeClass::Under6Months)]
    #[case(182, AgeClass::UnderYear)]
    #[case(364, AgeClass::UnderYear)]
    #[case(365, AgeClass::Under3Years)]
    #[case(1094, AgeClass::Under3Years)]
    #[case(1095, AgeClass::Over3Years)]
    #[case(5000, AgeClass::Over3Years)]
    fn altersklassen_grenzen(#[case] days: i64, #[case] expected: AgeClass) {
        assert_eq!(classify(days_ago(days), NOW), expected);
    }

    #[test]
    fn zukunft_ist_juenger_als_ein_monat() {
        assert_eq!(
            classify(NOW + 1000 * TICKS_PER_SEC, NOW),
            AgeClass::UnderMonth
        );
    }

    #[test]
    fn teiltage_werden_abgerundet() {
        let almost_30 = NOW - (30 * SECS_PER_DAY - 1) * TICKS_PER_SEC;
        assert_eq!(classify(almost_30, NOW), AgeClass::UnderMonth);
    }

    #[rstest]
    #[case("1y", 365)]
    #[case("18m", 540)]
    #[case("90d", 90)]
    #[case(" 2Y ", 730)]
    #[case("1D", 1)]
    fn parser_gueltig(#[case] input: &str, #[case] days: i64) {
        assert_eq!(parse_old_after(input), Ok(days));
    }

    #[rstest]
    #[case("")]
    #[case("y")]
    #[case("1")]
    #[case("1x")]
    #[case("0d")]
    #[case("-3m")]
    #[case("1.5y")]
    #[case("einjahr")]
    fn parser_ungueltig(#[case] input: &str) {
        assert!(parse_old_after(input).is_err(), "{input}");
    }

    #[test]
    fn alt_ist_strikt_groesser_als_schwellwert() {
        assert!(!is_old(days_ago(365), NOW, 365));
        assert!(is_old(days_ago(366), NOW, 365));
    }

    #[test]
    fn labels_sind_eindeutig() {
        let labels: std::collections::HashSet<_> =
            AgeClass::ALL.iter().map(|c| c.label()).collect();
        assert_eq!(labels.len(), 5);
    }
}
