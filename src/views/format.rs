//! Dates, times and numbers the way the chosen language writes them (ICU4X).
//!
//! Formatters are built once per language and kept for the life of the process,
//! so rendering a list row never loads locale data.

use std::rc::Rc;

use icu::calendar::{Date, Gregorian};
use icu::datetime::fieldsets::{MD, T};
use icu::datetime::FixedCalendarDateTimeFormatter;
use icu::decimal::input::Decimal;
use icu::decimal::DecimalFormatter;
use icu::locale::Locale;
use icu::time::Time;

use crate::domain::audio::CivilTime;
use crate::domain::gain::clamp_slider_gain;
use crate::domain::language::Language;

struct Formatters {
    clock: Option<FixedCalendarDateTimeFormatter<Gregorian, T>>,
    month_day: Option<FixedCalendarDateTimeFormatter<Gregorian, MD>>,
    number: Option<DecimalFormatter>,
}

impl Formatters {
    fn new(language: Language) -> Self {
        // English stays on the 24-hour clock the library has always shown.
        let tag = match language {
            Language::English => "en-u-hc-h23",
            Language::Swedish => "sv",
            Language::German => "de",
        };
        let locale: Locale = tag.parse().unwrap_or(Locale::UNKNOWN);
        Self {
            clock: FixedCalendarDateTimeFormatter::try_new((&locale).into(), T::hm()).ok(),
            month_day: FixedCalendarDateTimeFormatter::try_new((&locale).into(), MD::medium()).ok(),
            number: DecimalFormatter::try_new((&locale).into(), Default::default()).ok(),
        }
    }
}

// ICU4X formatters hold `Rc` data and are not `Send`, so each thread keeps its own
// copy. Rows render on the GPUI thread, which builds each language once.
thread_local! {
    static ENGLISH: Rc<Formatters> = Rc::new(Formatters::new(Language::English));
    static SWEDISH: Rc<Formatters> = Rc::new(Formatters::new(Language::Swedish));
    static GERMAN: Rc<Formatters> = Rc::new(Formatters::new(Language::German));
}

fn with_formatters<R>(language: Language, use_formatters: impl FnOnce(&Formatters) -> R) -> R {
    match language {
        Language::English => ENGLISH.with(|formatters| use_formatters(formatters)),
        Language::Swedish => SWEDISH.with(|formatters| use_formatters(formatters)),
        Language::German => GERMAN.with(|formatters| use_formatters(formatters)),
    }
}

/// Hour and minute, for example `14:32`.
pub(crate) fn clock_text(time: CivilTime, language: Language) -> String {
    let clock = Time::try_new(time.hour, time.minute, 0, 0)
        .ok()
        .and_then(|value| {
            with_formatters(language, |formatters| {
                formatters
                    .clock
                    .as_ref()
                    .map(|formatter| formatter.format(&value).to_string())
            })
        });
    clock.unwrap_or_else(|| format!("{:02}:{:02}", time.hour, time.minute))
}

/// Month and day, for example `Nov 12` or `12 nov.`.
pub(crate) fn month_day_text(time: CivilTime, language: Language) -> String {
    let text = Date::try_new_gregorian(time.year, time.month, time.day)
        .ok()
        .and_then(|value| {
            with_formatters(language, |formatters| {
                formatters
                    .month_day
                    .as_ref()
                    .map(|formatter| formatter.format(&value).to_string())
            })
        });
    text.unwrap_or_else(|| format!("{:02}-{:02}", time.month, time.day))
}

/// A byte count in the largest unit that keeps it above one, for example
/// `38.0 MB` or `38,0 MB`.
pub(crate) fn size_text(bytes: u64, language: Language) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    let (unit_bytes, unit) = if bytes >= GB {
        (GB, "GB")
    } else if bytes >= MB {
        (MB, "MB")
    } else if bytes >= KB {
        (KB, "KB")
    } else {
        return format!("{bytes} B");
    };
    let tenths = (bytes as f64 / unit_bytes as f64 * 10.0).round() as i64;
    format!("{} {unit}", one_decimal(tenths, language))
}

/// A gain factor with one decimal, for example `1.7×` or `1,7×`.
pub(crate) fn gain_text(gain: f32, language: Language) -> String {
    let tenths = (clamp_slider_gain(gain) * 10.0).round() as i64;
    format!("{}×", one_decimal(tenths, language))
}

/// `tenths` is the value times ten, shown with exactly one fraction digit.
fn one_decimal(tenths: i64, language: Language) -> String {
    let mut value = Decimal::from(tenths);
    value.multiply_pow10(-1);
    with_formatters(language, |formatters| match formatters.number.as_ref() {
        Some(formatter) => formatter.format(&value).to_string(),
        None => format!("{}.{}", tenths / 10, tenths % 10),
    })
}

#[cfg(test)]
mod tests {
    use super::{clock_text, gain_text, month_day_text, size_text};
    use crate::domain::audio::CivilTime;
    use crate::domain::language::Language;

    fn nov_12() -> CivilTime {
        CivilTime::new(2026, 11, 12, 14, 32, 0).expect("valid civil time")
    }

    #[test]
    fn english_clock_stays_24_hour() {
        // Arrange
        let time = nov_12();

        // Act
        let text = clock_text(time, Language::English);

        // Assert
        assert_eq!(text, "14:32");
    }

    #[test]
    fn swedish_clock_is_24_hour() {
        // Arrange
        let time = nov_12();

        // Act
        let text = clock_text(time, Language::Swedish);

        // Assert
        assert_eq!(text, "14:32");
    }

    #[test]
    fn german_clock_is_24_hour() {
        // Arrange
        let time = nov_12();

        // Act
        let text = clock_text(time, Language::German);

        // Assert
        assert_eq!(text, "14:32");
    }

    #[test]
    fn english_month_day() {
        // Arrange
        let time = nov_12();

        // Act
        let text = month_day_text(time, Language::English);

        // Assert
        assert_eq!(text, "Nov 12");
    }

    #[test]
    fn swedish_month_day() {
        // Arrange
        let time = nov_12();

        // Act
        let text = month_day_text(time, Language::Swedish);

        // Assert
        assert_eq!(text, "12 nov.");
    }

    #[test]
    fn german_month_day() {
        // Arrange
        let time = nov_12();

        // Act
        let text = month_day_text(time, Language::German);

        // Assert
        assert_eq!(text, "12. Nov.");
    }

    #[test]
    fn english_size_unchanged() {
        // Arrange
        let bytes = 39_845_888;

        // Act
        let text = size_text(bytes, Language::English);

        // Assert
        assert_eq!(text, "38.0 MB");
        assert_eq!(size_text(500, Language::English), "500 B");
        assert_eq!(size_text(2_048, Language::English), "2.0 KB");
    }

    #[test]
    fn swedish_size_uses_decimal_comma() {
        // Arrange
        let bytes = 39_845_888;

        // Act
        let text = size_text(bytes, Language::Swedish);

        // Assert
        assert_eq!(text, "38,0 MB");
        assert_eq!(size_text(500, Language::Swedish), "500 B");
        assert_eq!(size_text(983_696_512, Language::Swedish), "938,1 MB");
    }

    #[test]
    fn german_size_uses_decimal_comma() {
        // Arrange
        let bytes = 39_845_888;

        // Act
        let text = size_text(bytes, Language::German);
        let gain = gain_text(1.7, Language::German);

        // Assert
        assert_eq!(text, "38,0 MB");
        assert_eq!(gain, "1,7×");
    }

    #[test]
    fn gain_follows_the_language_decimal_mark() {
        // Arrange
        let gain = 1.7;

        // Act
        let english = gain_text(gain, Language::English);
        let swedish = gain_text(gain, Language::Swedish);

        // Assert
        assert_eq!(english, "1.7×");
        assert_eq!(swedish, "1,7×");
        assert_eq!(gain_text(9.0, Language::English), "4.0×");
    }
}
