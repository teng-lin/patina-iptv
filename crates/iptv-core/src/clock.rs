//! Wall-clock labels for guide times, in the interface language and on a
//! 12-hour or 24-hour clock ([`Style`]). The UTC offset comes from the
//! platform per instant (`offset_minutes`), so labels across a daylight
//! saving change stay right; the core never reads a clock or a time zone.

use crate::i18n::calendar::{Calendar, calendar_or_english};
use crate::i18n::{self, Language, message};

/// Whether a day's hours run 0–23, or 12 and 1–11 twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HourCycle {
    H12,
    H24,
}

impl HourCycle {
    /// What a locale's own convention is, by the Unicode CLDR's defaults
    /// for the catalogue's languages: 12-hour for English outside the
    /// United Kingdom, Ireland and South Africa, for Korean and for
    /// Traditional Chinese; 24-hour for the rest, and for a locale whose
    /// language the catalogue lacks (its interface is English, its habits
    /// are not). `locale` is a tag as the platform or the language setting
    /// gives it (`en_GB.UTF-8`, `ko`).
    pub fn of_locale(locale: &str) -> Self {
        let normalized = locale.trim().replace('_', "-").to_ascii_lowercase();
        let tag = normalized.split(['.', '@']).next().unwrap_or("");
        let mut parts = tag.split('-');
        let base = parts.next().unwrap_or("");
        let region = parts.find(|part| part.len() == 2);
        match base {
            "en" if !matches!(region, Some("gb" | "ie" | "za")) => Self::H12,
            "ko" => Self::H12,
            "zh" if i18n::resolve(locale, None).tag == "zh-Hant" => Self::H12,
            _ => Self::H24,
        }
    }

    /// The clock for the `general.clock` setting: `h12` or `h24`, and
    /// anything else (`auto`) follows `locale`.
    pub fn from_preference(preference: &str, locale: &str) -> Self {
        match preference {
            "h12" => Self::H12,
            "h24" => Self::H24,
            _ => Self::of_locale(locale),
        }
    }
}

/// How dates and times are written: the language's names and order, and
/// the clock.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub language: &'static Language,
    pub hours: HourCycle,
}

impl Style {
    pub fn new(language: &'static Language, hours: HourCycle) -> Self {
        Self { language, hours }
    }

    /// English names on a 24-hour clock: what the app showed before it
    /// had languages, and what logs and tests can rely on.
    pub fn english_24() -> Self {
        Self::new(&i18n::LANGUAGES[0], HourCycle::H24)
    }

    fn calendar(&self) -> &'static Calendar {
        calendar_or_english(self.language.tag)
    }

    fn fill(&self, pattern: &str, args: &i18n::Args<'_>) -> String {
        message::render(pattern, self.language.tag, args)
    }
}

/// Local day number (days since 1970-01-01 in local time) and minute of day.
fn local(unix: i64, offset_minutes: i32) -> (i64, i64) {
    let t = unix + i64::from(offset_minutes) * 60;
    (t.div_euclid(86_400), t.rem_euclid(86_400) / 60)
}

/// Day of the month (1-based) and month (0-based) of a day number
/// (proleptic Gregorian; Howard Hinnant's `civil_from_days`).
fn month_day(days: i64) -> (i64, usize) {
    let z = days + 719_468;
    let doe = z - z.div_euclid(146_097) * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 2 } else { mp - 10 };
    (doy - (153 * mp + 2) / 5 + 1, month as usize)
}

fn weekday(days: i64, calendar: &Calendar) -> &'static str {
    // 1970-01-01 was a Thursday (index 3 from Monday).
    calendar.weekdays[(days + 3).rem_euclid(7) as usize]
}

/// A time of day from its minute (0 is midnight): `18:05`, `6:05 PM`,
/// `오후 6:05`. The 24-hour clock keeps two digits for the hour, so times
/// line up in the guide's ruler; the 12-hour clock has none to pad.
pub fn time(minute_of_day: i64, style: Style) -> String {
    let minute_of_day = minute_of_day.rem_euclid(1440);
    let (hour, minute) = (minute_of_day / 60, minute_of_day % 60);
    if style.hours == HourCycle::H24 {
        return format!("{hour:02}:{minute:02}");
    }
    let calendar = style.calendar();
    let shown = match hour % 12 {
        0 if !calendar.hours_from_zero => 12,
        h => h,
    };
    let period = if hour < 12 { calendar.am } else { calendar.pm };
    let time = format!("{shown}:{minute:02}");
    style.fill(
        calendar.twelve_hour,
        &[("time", time.as_str().into()), ("period", period.into())],
    )
}

/// The local date of `unix`: weekday, day and month (`Tue 10 Nov`,
/// `11月10日(火)`).
pub fn date(unix: i64, offset_minutes: i32, style: Style) -> String {
    let (day, _) = local(unix, offset_minutes);
    let (dom, month) = month_day(day);
    let calendar = style.calendar();
    style.fill(
        calendar.date,
        &[
            ("weekday", weekday(day, calendar).into()),
            ("day", (dom as u64).into()),
            ("month", calendar.months[month].into()),
        ],
    )
}

/// The local time of `unix`; on another local day than `now_unix`,
/// prefixed with the weekday and day of the month (`Tue 10 · 06:30`), as
/// the guide shows times while browsing the past or the future.
pub fn label(
    unix: i64,
    now_unix: i64,
    offset_minutes: impl Fn(i64) -> i32,
    style: Style,
) -> String {
    let (day, minute) = local(unix, offset_minutes(unix));
    let (today, _) = local(now_unix, offset_minutes(now_unix));
    let time = time(minute, style);
    if day == today {
        return time;
    }
    let calendar = style.calendar();
    let date = style.fill(
        calendar.day,
        &[
            ("weekday", weekday(day, calendar).into()),
            ("day", (month_day(day).0 as u64).into()),
        ],
    );
    format!("{date} · {time}")
}

/// The local date and time of `unix` (`Tue 10 Nov  18:00`), at a UTC
/// offset of `offset_minutes`.
pub fn date_time(unix: i64, offset_minutes: i32, style: Style) -> String {
    let (_, minute) = local(unix, offset_minutes);
    format!(
        "{}  {}",
        date(unix, offset_minutes, style),
        time(minute, style)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::SYNTHETIC_EPOCH_UNIX as EPOCH;

    fn english() -> Style {
        Style::english_24()
    }

    fn style(tag: &str, hours: HourCycle) -> Style {
        Style::new(i18n::resolve(tag, None), hours)
    }

    #[test]
    fn today_shows_the_time_in_the_given_offset() {
        assert_eq!(label(EPOCH, EPOCH, |_| 0, english()), "18:00");
        assert_eq!(label(EPOCH + 90 * 60, EPOCH, |_| 0, english()), "19:30");
        assert_eq!(label(EPOCH, EPOCH, |_| -5 * 60, english()), "13:00");
        assert_eq!(label(EPOCH, EPOCH, |_| 5 * 60 + 45, english()), "23:45");
    }

    #[test]
    fn other_days_carry_their_weekday_and_date() {
        // The epoch is Tuesday 10 November 18:00 UTC; six hours later is Wednesday.
        let of = |unix, offset: i32| label(unix, EPOCH, |_| offset, english());
        assert_eq!(of(EPOCH + 6 * 3600, 0), "Wed 11 · 00:00");
        assert_eq!(of(EPOCH - 24 * 3600, 0), "Mon 9 · 18:00");
        // At UTC+7 the epoch is already Wednesday 01:00: today is local.
        assert_eq!(of(EPOCH, 7 * 60), "01:00");
        assert_eq!(of(EPOCH - 2 * 3600, 7 * 60), "Tue 10 · 23:00");
    }

    #[test]
    fn dates_name_the_weekday_day_and_month() {
        assert_eq!(date(EPOCH, 0, english()), "Tue 10 Nov");
        assert_eq!(date(EPOCH, 7 * 60, english()), "Wed 11 Nov");
        assert_eq!(date(0, 0, english()), "Thu 1 Jan");
        assert_eq!(date(951_782_400, 0, english()), "Tue 29 Feb", "2000-02-29");
        assert_eq!(date(EPOCH + 51 * 86_400, 0, english()), "Thu 31 Dec");
    }

    #[test]
    fn the_offset_is_taken_per_instant() {
        // A daylight saving change between the two instants.
        let offset = |t: i64| if t < EPOCH + 3600 { 60 } else { 0 };
        assert_eq!(label(EPOCH, EPOCH, offset, english()), "19:00");
        assert_eq!(label(EPOCH + 2 * 3600, EPOCH, offset, english()), "20:00");
    }

    #[test]
    fn dates_use_each_languages_names_and_order() {
        let of = |tag: &str| date(EPOCH, 0, style(tag, HourCycle::H24));
        assert_eq!(of("en"), "Tue 10 Nov");
        assert_eq!(of("es"), "mar, 10 nov");
        assert_eq!(of("fr"), "mar. 10 nov.");
        assert_eq!(of("de"), "Di., 10. Nov.");
        assert_eq!(of("it"), "mar 10 nov");
        assert_eq!(of("pt"), "ter., 10 de nov.");
        assert_eq!(of("ru"), "вт, 10 нояб.");
        assert_eq!(of("zh"), "11月10日 周二");
        assert_eq!(of("zh-TW"), "11月10日 週二");
        assert_eq!(of("ja"), "11月10日(火)");
        assert_eq!(of("ko"), "11월 10일 (화)");
    }

    #[test]
    fn another_days_label_follows_the_language_too() {
        let of = |tag: &str, hours| label(EPOCH + 6 * 3600, EPOCH, |_| 0, style(tag, hours));
        assert_eq!(of("de", HourCycle::H24), "Mi., 11. · 00:00");
        assert_eq!(of("fr", HourCycle::H24), "mer. 11 · 00:00");
        assert_eq!(of("ja", HourCycle::H24), "11日(水) · 00:00");
        assert_eq!(of("en", HourCycle::H12), "Wed 11 · 12:00 AM");
        assert_eq!(of("ko", HourCycle::H12), "11일 (수) · 오전 12:00");
    }

    #[test]
    fn the_twelve_hour_clock_names_each_half_of_the_day() {
        let en = |minute| time(minute, style("en", HourCycle::H12));
        assert_eq!(en(0), "12:00 AM");
        assert_eq!(en(5), "12:05 AM");
        assert_eq!(en(6 * 60 + 30), "6:30 AM");
        assert_eq!(en(11 * 60 + 59), "11:59 AM");
        assert_eq!(en(12 * 60), "12:00 PM");
        assert_eq!(en(18 * 60 + 5), "6:05 PM");
        assert_eq!(en(23 * 60 + 59), "11:59 PM");
        let at = |tag: &str, minute| time(minute, style(tag, HourCycle::H12));
        assert_eq!(at("es", 18 * 60), "6:00 p. m.");
        assert_eq!(at("de", 18 * 60), "6:00 PM");
        assert_eq!(at("zh", 18 * 60), "下午6:00");
        assert_eq!(at("zh-TW", 9 * 60), "上午9:00");
        assert_eq!(at("ko", 18 * 60), "오후 6:00");
        // Japanese counts the hours of each half from zero.
        assert_eq!(at("ja", 18 * 60), "午後6:00");
        assert_eq!(at("ja", 5), "午前0:05");
        assert_eq!(at("ja", 12 * 60 + 5), "午後0:05");
    }

    #[test]
    fn the_twenty_four_hour_clock_is_the_same_in_every_language() {
        for language in i18n::LANGUAGES {
            let style = Style::new(language, HourCycle::H24);
            assert_eq!(time(18 * 60 + 5, style), "18:05", "{}", language.tag);
            assert_eq!(time(6 * 60, style), "06:00", "{}", language.tag);
            assert_eq!(time(0, style), "00:00", "{}", language.tag);
        }
        // A minute outside the day wraps into it.
        assert_eq!(time(-30, english()), "23:30");
        assert_eq!(time(1440 + 61, english()), "01:01");
    }

    #[test]
    fn date_and_time_together() {
        assert_eq!(date_time(EPOCH, 0, english()), "Tue 10 Nov  18:00");
        assert_eq!(date_time(EPOCH, 7 * 60, english()), "Wed 11 Nov  01:00");
        assert_eq!(
            date_time(EPOCH, -5 * 60 - 30, english()),
            "Tue 10 Nov  12:30"
        );
        // Across the turn of the year, and a leap day.
        assert_eq!(
            date_time(EPOCH + 51 * 86_400, 0, english()),
            "Thu 31 Dec  18:00"
        );
        assert_eq!(
            date_time(EPOCH + 52 * 86_400, 0, english()),
            "Fri 1 Jan  18:00"
        );
        assert_eq!(date_time(951_782_400, 0, english()), "Tue 29 Feb  00:00");
        assert_eq!(
            date_time(EPOCH, 0, style("en", HourCycle::H12)),
            "Tue 10 Nov  6:00 PM"
        );
        assert_eq!(
            date_time(EPOCH, 0, style("ko", HourCycle::H12)),
            "11월 10일 (화)  오후 6:00"
        );
    }

    #[test]
    fn the_clock_follows_the_locale_unless_a_preference_is_set() {
        use HourCycle::{H12, H24};
        for (locale, expected) in [
            ("en", H12),
            ("en-US", H12),
            ("en_US.UTF-8", H12),
            ("en-AU", H12),
            ("en-GB", H24),
            ("en_IE.UTF-8", H24),
            ("en-ZA", H24),
            ("ko", H12),
            ("ko-KR", H12),
            ("zh-TW", H12),
            ("zh-Hant", H12),
            ("zh-CN", H24),
            ("zh", H24),
            ("ja-JP", H24),
            ("es-MX", H24),
            ("fr", H24),
            ("de-DE", H24),
            ("it", H24),
            ("pt-BR", H24),
            ("ru", H24),
        ] {
            assert_eq!(HourCycle::of_locale(locale), expected, "{locale}");
            assert_eq!(HourCycle::from_preference("auto", locale), expected);
        }
        // A language the catalogue lacks is shown in English, but its
        // readers did not start using a 12-hour clock.
        for locale in ["nl-NL", "xx", "C", ""] {
            assert_eq!(HourCycle::of_locale(locale), H24, "{locale}");
        }
        assert_eq!(HourCycle::from_preference("h24", "en-US"), H24);
        assert_eq!(HourCycle::from_preference("h12", "de"), H12);
    }
}
