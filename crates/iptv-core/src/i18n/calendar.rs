//! How each catalogue language writes a day, a month and a time of day:
//! the names, their order, and the two halves of a 12-hour day. The
//! patterns follow the Unicode CLDR's abbreviated formats, except English,
//! which keeps the day before the month as the guide always showed it.
//!
//! [`clock`](crate::clock) fills these in; nothing here reads a clock.

/// One language's calendar words. Patterns are
/// [messages](super::message) with the placeholders named beside them.
#[derive(Debug)]
pub struct Calendar {
    /// Abbreviated, Monday first.
    pub weekdays: [&'static str; 7],
    /// Abbreviated, as written beside a day number; January first.
    pub months: [&'static str; 12],
    /// `{weekday}`, `{day}` (of the month) and `{month}`.
    pub date: &'static str,
    /// `{weekday}` and `{day}`: a date within the days around today.
    pub day: &'static str,
    /// Before and after noon on a 12-hour clock.
    pub am: &'static str,
    pub pm: &'static str,
    /// `{time}` (`6:05`) and `{period}` (one of the two above).
    pub twelve_hour: &'static str,
    /// Whether the hour after midnight and after noon is 0 (Japanese
    /// 午前0:05) rather than 12 (12:05 AM).
    pub hours_from_zero: bool,
}

const LATIN_PERIODS: (&str, &str, &str) = ("AM", "PM", "{time} {period}");

const ENGLISH: Calendar = Calendar {
    weekdays: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    months: [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    date: "{weekday} {day} {month}",
    day: "{weekday} {day}",
    am: LATIN_PERIODS.0,
    pm: LATIN_PERIODS.1,
    twelve_hour: LATIN_PERIODS.2,
    hours_from_zero: false,
};

const SPANISH: Calendar = Calendar {
    weekdays: ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"],
    months: [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic",
    ],
    date: "{weekday}, {day} {month}",
    day: "{weekday} {day}",
    am: "a. m.",
    pm: "p. m.",
    twelve_hour: "{time} {period}",
    hours_from_zero: false,
};

const FRENCH: Calendar = Calendar {
    weekdays: ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."],
    months: [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ],
    date: "{weekday} {day} {month}",
    day: "{weekday} {day}",
    am: LATIN_PERIODS.0,
    pm: LATIN_PERIODS.1,
    twelve_hour: LATIN_PERIODS.2,
    hours_from_zero: false,
};

const GERMAN: Calendar = Calendar {
    weekdays: ["Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa.", "So."],
    months: [
        "Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.",
        "Dez.",
    ],
    date: "{weekday}, {day}. {month}",
    day: "{weekday}, {day}.",
    am: LATIN_PERIODS.0,
    pm: LATIN_PERIODS.1,
    twelve_hour: LATIN_PERIODS.2,
    hours_from_zero: false,
};

const ITALIAN: Calendar = Calendar {
    weekdays: ["lun", "mar", "mer", "gio", "ven", "sab", "dom"],
    months: [
        "gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic",
    ],
    date: "{weekday} {day} {month}",
    day: "{weekday} {day}",
    am: LATIN_PERIODS.0,
    pm: LATIN_PERIODS.1,
    twelve_hour: LATIN_PERIODS.2,
    hours_from_zero: false,
};

const PORTUGUESE: Calendar = Calendar {
    weekdays: ["seg.", "ter.", "qua.", "qui.", "sex.", "sáb.", "dom."],
    months: [
        "jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.",
        "dez.",
    ],
    date: "{weekday}, {day} de {month}",
    day: "{weekday}, {day}",
    am: LATIN_PERIODS.0,
    pm: LATIN_PERIODS.1,
    twelve_hour: LATIN_PERIODS.2,
    hours_from_zero: false,
};

const RUSSIAN: Calendar = Calendar {
    weekdays: ["пн", "вт", "ср", "чт", "пт", "сб", "вс"],
    // The genitive, as a month reads after a day number.
    months: [
        "янв.",
        "февр.",
        "мар.",
        "апр.",
        "мая",
        "июн.",
        "июл.",
        "авг.",
        "сент.",
        "окт.",
        "нояб.",
        "дек.",
    ],
    date: "{weekday}, {day} {month}",
    day: "{weekday}, {day}",
    am: LATIN_PERIODS.0,
    pm: LATIN_PERIODS.1,
    twelve_hour: LATIN_PERIODS.2,
    hours_from_zero: false,
};

/// Chinese, Japanese and Korean number their months.
const MONTHS_CJ: [&str; 12] = [
    "1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月",
];
const MONTHS_KOREAN: [&str; 12] = [
    "1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월",
];

const CHINESE_SIMPLIFIED: Calendar = Calendar {
    weekdays: ["周一", "周二", "周三", "周四", "周五", "周六", "周日"],
    months: MONTHS_CJ,
    date: "{month}{day}日 {weekday}",
    day: "{day}日 {weekday}",
    am: "上午",
    pm: "下午",
    twelve_hour: "{period}{time}",
    hours_from_zero: false,
};

const CHINESE_TRADITIONAL: Calendar = Calendar {
    weekdays: ["週一", "週二", "週三", "週四", "週五", "週六", "週日"],
    months: MONTHS_CJ,
    date: "{month}{day}日 {weekday}",
    day: "{day}日 {weekday}",
    am: "上午",
    pm: "下午",
    twelve_hour: "{period}{time}",
    hours_from_zero: false,
};

const JAPANESE: Calendar = Calendar {
    weekdays: ["月", "火", "水", "木", "金", "土", "日"],
    months: MONTHS_CJ,
    date: "{month}{day}日({weekday})",
    day: "{day}日({weekday})",
    am: "午前",
    pm: "午後",
    twelve_hour: "{period}{time}",
    hours_from_zero: true,
};

const KOREAN: Calendar = Calendar {
    weekdays: ["월", "화", "수", "목", "금", "토", "일"],
    months: MONTHS_KOREAN,
    date: "{month} {day}일 ({weekday})",
    day: "{day}일 ({weekday})",
    am: "오전",
    pm: "오후",
    twelve_hour: "{period} {time}",
    hours_from_zero: false,
};

/// The calendar of the language `tag` (a tag of
/// [`LANGUAGES`](super::LANGUAGES)); `None` for another.
pub fn calendar(tag: &str) -> Option<&'static Calendar> {
    Some(match tag {
        "en" => &ENGLISH,
        "es" => &SPANISH,
        "fr" => &FRENCH,
        "de" => &GERMAN,
        "it" => &ITALIAN,
        "pt" => &PORTUGUESE,
        "ru" => &RUSSIAN,
        "zh" => &CHINESE_SIMPLIFIED,
        "zh-Hant" => &CHINESE_TRADITIONAL,
        "ja" => &JAPANESE,
        "ko" => &KOREAN,
        _ => return None,
    })
}

/// As [`calendar`], with English for a language that has none.
pub fn calendar_or_english(tag: &str) -> &'static Calendar {
    calendar(tag).unwrap_or(&ENGLISH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{LANGUAGES, message};

    /// A language added to the catalogue without a calendar would show
    /// English dates without a sign.
    #[test]
    fn every_catalogue_language_has_a_complete_calendar() {
        for language in LANGUAGES {
            let tag = language.tag;
            let calendar = calendar(tag).unwrap_or_else(|| panic!("no calendar for {tag}"));
            for name in calendar.weekdays.iter().chain(&calendar.months) {
                assert!(!name.trim().is_empty(), "{tag}");
            }
            let distinct = |names: &[&str]| {
                names
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            };
            assert_eq!(distinct(&calendar.weekdays), 7, "{tag}");
            assert_eq!(distinct(&calendar.months), 12, "{tag}");
            assert_ne!(calendar.am, calendar.pm, "{tag}");
            for (pattern, names) in [
                (calendar.date, vec!["day", "month", "weekday"]),
                (calendar.day, vec!["day", "weekday"]),
                (calendar.twelve_hour, vec!["period", "time"]),
            ] {
                let shape = message::shape(pattern).unwrap();
                assert_eq!(shape.names, names, "{tag}: {pattern}");
                assert!(shape.plurals.is_empty(), "{tag}: {pattern}");
            }
        }
        assert!(calendar("xx").is_none());
        assert_eq!(calendar_or_english("xx").weekdays[0], "Mon");
    }

    #[test]
    fn names_are_in_their_own_language() {
        assert_eq!(calendar("es").unwrap().weekdays[2], "mié");
        assert_eq!(calendar("fr").unwrap().months[7], "août");
        assert_eq!(calendar("de").unwrap().months[2], "März");
        assert_eq!(calendar("ru").unwrap().months[4], "мая");
        assert_eq!(calendar("zh").unwrap().weekdays[6], "周日");
        assert_eq!(calendar("zh-Hant").unwrap().weekdays[6], "週日");
        assert_eq!(calendar("ja").unwrap().months[11], "12月");
        assert_eq!(calendar("ko").unwrap().months[0], "1월");
    }
}
