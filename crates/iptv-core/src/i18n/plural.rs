//! Plural categories for the catalogue's languages: which form of a
//! message a count selects. The rules are the Unicode CLDR cardinal rules
//! for whole numbers; the app never shows a fractional count.
//!
//! They are written out here for the eleven languages of
//! [`LANGUAGES`](super::LANGUAGES). A language whose rules are none of
//! these needs its own arm in [`category`] and a row in [`forms`]; a test
//! fails for a catalogue language without one.

/// A CLDR plural category. `Other` is the general form every language has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    One,
    Few,
    Many,
    Other,
}

impl Category {
    /// The keyword a catalogue message names the category by.
    pub fn keyword(self) -> &'static str {
        match self {
            Self::One => "one",
            Self::Few => "few",
            Self::Many => "many",
            Self::Other => "other",
        }
    }

    pub fn from_keyword(word: &str) -> Option<Self> {
        [Self::One, Self::Few, Self::Many, Self::Other]
            .into_iter()
            .find(|category| category.keyword() == word)
    }
}

/// The forms a language's plural messages carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forms {
    /// Every plural message in this language has these.
    pub required: &'static [Category],
    /// A message may add these; a count that selects one a message does
    /// not have takes `other`.
    pub optional: &'static [Category],
}

/// A whole million in the Romance languages takes "de" before its noun
/// ("1 000 000 de chaînes"): CLDR's `many`.
fn whole_millions(n: u64) -> bool {
    n != 0 && n.is_multiple_of(1_000_000)
}

/// The category `n` selects in the language `tag` (a tag of
/// [`LANGUAGES`](super::LANGUAGES)); an unknown tag has the general form
/// only.
pub fn category(tag: &str, n: u64) -> Category {
    match tag {
        "en" | "de" => match n {
            1 => Category::One,
            _ => Category::Other,
        },
        "es" | "it" => match n {
            1 => Category::One,
            _ if whole_millions(n) => Category::Many,
            _ => Category::Other,
        },
        // Zero is singular in French and Portuguese ("0 chaîne").
        "fr" | "pt" => match n {
            0 | 1 => Category::One,
            _ if whole_millions(n) => Category::Many,
            _ => Category::Other,
        },
        // 1, 21, 31… канал; 2–4, 22–24… канала; 0, 5–20, 25–30… каналов.
        "ru" => match (n % 10, n % 100) {
            (1, tens) if tens != 11 => Category::One,
            (2..=4, tens) if !(12..=14).contains(&tens) => Category::Few,
            _ => Category::Many,
        },
        // Chinese, Japanese and Korean do not inflect for number.
        _ => Category::Other,
    }
}

/// The forms plural messages have in the language `tag`; `None` for a
/// language the rules above do not cover.
pub fn forms(tag: &str) -> Option<Forms> {
    use Category::{Few, Many, One, Other};
    Some(match tag {
        "en" | "de" => Forms {
            required: &[One, Other],
            optional: &[],
        },
        "es" | "it" | "fr" | "pt" => Forms {
            required: &[One, Other],
            optional: &[Many],
        },
        // `other` is the form of a fraction; a message names it so that
        // every plural has the general form.
        "ru" => Forms {
            required: &[One, Few, Many, Other],
            optional: &[],
        },
        "zh" | "zh-Hant" | "ja" | "ko" => Forms {
            required: &[Other],
            optional: &[],
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use Category::{Few, Many, One, Other};

    fn of(tag: &str, counts: &[u64]) -> Vec<Category> {
        counts.iter().map(|n| category(tag, *n)).collect()
    }

    #[test]
    fn english_and_german_have_one_and_other() {
        for tag in ["en", "de"] {
            assert_eq!(
                of(tag, &[0, 1, 2, 11, 21, 101, 1_000_000]),
                [Other, One, Other, Other, Other, Other, Other],
                "{tag}"
            );
        }
    }

    #[test]
    fn romance_languages_differ_at_zero_and_share_the_millions() {
        for tag in ["es", "it"] {
            assert_eq!(of(tag, &[0, 1, 2, 21]), [Other, One, Other, Other]);
        }
        for tag in ["fr", "pt"] {
            assert_eq!(of(tag, &[0, 1, 2, 21]), [One, One, Other, Other]);
        }
        for tag in ["es", "it", "fr", "pt"] {
            assert_eq!(
                of(tag, &[1_000_000, 3_000_000, 1_500_000, 999_999]),
                [Many, Many, Other, Other],
                "{tag}"
            );
        }
    }

    #[test]
    fn russian_follows_the_last_digits_except_in_the_teens() {
        assert_eq!(of("ru", &[1, 21, 31, 101, 1001]), [One, One, One, One, One]);
        assert_eq!(of("ru", &[2, 3, 4, 22, 24, 103]), [Few; 6]);
        assert_eq!(
            of(
                "ru",
                &[0, 5, 9, 10, 11, 12, 13, 14, 15, 20, 25, 100, 111, 112, 114]
            ),
            [Many; 15]
        );
    }

    #[test]
    fn chinese_japanese_and_korean_have_one_form() {
        for tag in ["zh", "zh-Hant", "ja", "ko"] {
            assert_eq!(of(tag, &[0, 1, 2, 5, 21, 1_000_000]), [Other; 6], "{tag}");
        }
    }

    /// Every category a language's rules can select is one its messages
    /// may carry, and each required form is selected by some count: a
    /// required form no count selects would be dead text in every row.
    #[test]
    fn every_catalogue_language_has_rules_and_forms_that_agree() {
        for language in crate::i18n::LANGUAGES {
            let forms = forms(language.tag)
                .unwrap_or_else(|| panic!("no plural forms for {}", language.tag));
            assert!(forms.required.contains(&Other), "{}", language.tag);
            let counts = (0..=200).chain([1_000_000, 2_000_000, 5_000_001]);
            let mut selected = Vec::new();
            for n in counts {
                let category = category(language.tag, n);
                assert!(
                    forms.required.contains(&category) || forms.optional.contains(&category),
                    "{}: {n} selects {category:?}",
                    language.tag
                );
                if !selected.contains(&category) {
                    selected.push(category);
                }
            }
            for form in forms.required.iter().chain(forms.optional) {
                // Russian's `other` is the form of a fraction.
                let fraction_only = language.tag == "ru" && *form == Other;
                assert!(
                    selected.contains(form) || fraction_only,
                    "{}: no count selects {form:?}",
                    language.tag
                );
            }
        }
        assert_eq!(forms("xx"), None);
        assert_eq!(category("xx", 1), Other);
    }

    #[test]
    fn keywords_round_trip() {
        for category in [One, Few, Many, Other] {
            assert_eq!(Category::from_keyword(category.keyword()), Some(category));
        }
        assert_eq!(Category::from_keyword("two"), None);
    }
}
