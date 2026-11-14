//! Messages with named placeholders and plural forms.
//!
//! A message is text with `{name}` where a value goes, so a translation can
//! put it where its grammar wants it:
//!
//! ```text
//! Delete “{name}”?
//! ```
//!
//! A count chooses between whole alternatives, one per plural category of
//! the language ([`plural`](super::plural)). Inside an alternative `#` is
//! the number, and other placeholders work as usual:
//!
//! ```text
//! {count, plural, one {# channel in {group}} other {# channels in {group}}}
//! ```
//!
//! `=0`, `=1`… name an exact count and win over a category ("No
//! channels"). This is the plural subset of ICU MessageFormat, which
//! translators and their tools know; there is no quoting, so a message
//! cannot show a literal brace or, inside a plural, a literal `#`.

use super::plural::{self, Category};

/// A value for a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg<'a> {
    Text(&'a str),
    /// A whole number: shown as digits, and what a plural selects on.
    Count(u64),
}

impl<'a> From<&'a str> for Arg<'a> {
    fn from(text: &'a str) -> Self {
        Self::Text(text)
    }
}

impl<'a> From<&'a String> for Arg<'a> {
    fn from(text: &'a String) -> Self {
        Self::Text(text)
    }
}

macro_rules! count_from {
    ($($number:ty),*) => {$(
        impl From<$number> for Arg<'_> {
            fn from(n: $number) -> Self {
                Self::Count(n as u64)
            }
        }
    )*};
}
count_from!(u8, u16, u32, u64, usize);

/// Named values for a message's placeholders.
pub type Args<'a> = [(&'a str, Arg<'a>)];

/// One piece of a parsed message.
#[derive(Debug, PartialEq, Eq)]
enum Part<'a> {
    Text(&'a str),
    /// `#` inside a plural: its count.
    Number,
    Value(&'a str),
    Plural {
        name: &'a str,
        forms: Vec<(Selector, Vec<Part<'a>>)>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Selector {
    Exact(u64),
    Category(Category),
}

/// Why a message is not well formed; `at` is a byte offset into it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub at: usize,
    pub why: &'static str,
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Parser<'a> {
    fn error<T>(&self, why: &'static str) -> Result<T, SyntaxError> {
        Err(SyntaxError { at: self.at, why })
    }

    fn rest(&self) -> &'a str {
        &self.text[self.at..]
    }

    fn skip_spaces(&mut self) {
        self.at += self.rest().len() - self.rest().trim_start().len();
    }

    /// Text and placeholders up to the end, or up to the `}` that closes
    /// a plural's alternative when `nested`.
    fn parts(&mut self, nested: bool) -> Result<Vec<Part<'a>>, SyntaxError> {
        let mut parts = Vec::new();
        loop {
            let rest = self.rest();
            let stop = rest
                .find(|c| c == '{' || c == '}' || (nested && c == '#'))
                .unwrap_or(rest.len());
            if stop > 0 {
                parts.push(Part::Text(&rest[..stop]));
                self.at += stop;
            }
            match self.rest().chars().next() {
                None if nested => return self.error("a plural form is not closed"),
                None => return Ok(parts),
                Some('}') if nested => return Ok(parts),
                Some('}') => return self.error("a closing brace without an opening one"),
                Some('#') => {
                    self.at += 1;
                    parts.push(Part::Number);
                }
                Some(_) => {
                    self.at += 1;
                    parts.push(self.placeholder()?);
                }
            }
        }
    }

    /// After `{`: `name}` or `name, plural, …}`.
    fn placeholder(&mut self) -> Result<Part<'a>, SyntaxError> {
        let rest = self.rest();
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let name = &rest[..end];
        if name.is_empty() {
            return self.error("a placeholder needs a name");
        }
        self.at += end;
        self.skip_spaces();
        if let Some(rest) = self.rest().strip_prefix('}') {
            self.at = self.text.len() - rest.len();
            return Ok(Part::Value(name));
        }
        if !self.rest().starts_with(',') {
            return self.error("expected } or , after the placeholder's name");
        }
        self.at += 1;
        self.skip_spaces();
        let Some(rest) = self.rest().strip_prefix("plural") else {
            return self.error("only plural is understood after the name");
        };
        self.at = self.text.len() - rest.len();
        self.skip_spaces();
        if !self.rest().starts_with(',') {
            return self.error("expected , after plural");
        }
        self.at += 1;
        let mut forms: Vec<(Selector, Vec<Part<'a>>)> = Vec::new();
        loop {
            self.skip_spaces();
            if self.rest().starts_with('}') {
                self.at += 1;
                break;
            }
            if self.rest().is_empty() {
                return self.error("a plural is not closed");
            }
            let rest = self.rest();
            let end = rest
                .find(|c: char| c == '{' || c.is_whitespace())
                .unwrap_or(rest.len());
            let selector = match rest[..end].strip_prefix('=') {
                Some(exact) => match exact.parse() {
                    Ok(n) => Selector::Exact(n),
                    Err(_) => return self.error("=N needs a whole number"),
                },
                None => match Category::from_keyword(&rest[..end]) {
                    Some(category) => Selector::Category(category),
                    None => return self.error("not a plural category"),
                },
            };
            if forms.iter().any(|(known, _)| *known == selector) {
                return self.error("a plural form is given twice");
            }
            self.at += end;
            self.skip_spaces();
            if !self.rest().starts_with('{') {
                return self.error("a plural form's text goes in braces");
            }
            self.at += 1;
            let text = self.parts(true)?;
            self.at += 1;
            forms.push((selector, text));
        }
        if !forms
            .iter()
            .any(|(selector, _)| *selector == Selector::Category(Category::Other))
        {
            return self.error("a plural needs its other form");
        }
        Ok(Part::Plural { name, forms })
    }
}

fn parse(message: &str) -> Result<Vec<Part<'_>>, SyntaxError> {
    Parser {
        text: message,
        at: 0,
    }
    .parts(false)
}

fn value<'a>(args: &Args<'a>, name: &str) -> Option<Arg<'a>> {
    args.iter()
        .find(|(known, _)| *known == name)
        .map(|(_, arg)| *arg)
}

fn write(out: &mut String, parts: &[Part<'_>], tag: &str, args: &Args<'_>, number: Option<u64>) {
    for part in parts {
        match part {
            Part::Text(text) => out.push_str(text),
            Part::Number => match number {
                Some(n) => out.push_str(&n.to_string()),
                None => out.push('#'),
            },
            Part::Value(name) => match value(args, name) {
                Some(Arg::Text(text)) => out.push_str(text),
                Some(Arg::Count(n)) => out.push_str(&n.to_string()),
                // Shown as written, so a missing value is seen, not lost.
                None => {
                    out.push('{');
                    out.push_str(name);
                    out.push('}');
                }
            },
            Part::Plural { name, forms } => {
                let Some(Arg::Count(n)) = value(args, name) else {
                    out.push('{');
                    out.push_str(name);
                    out.push('}');
                    continue;
                };
                let form = |selector| {
                    forms
                        .iter()
                        .find(|(known, _)| *known == selector)
                        .map(|(_, text)| text)
                };
                let chosen = form(Selector::Exact(n))
                    .or_else(|| form(Selector::Category(plural::category(tag, n))))
                    .or_else(|| form(Selector::Category(Category::Other)));
                if let Some(text) = chosen {
                    write(out, text, tag, args, Some(n));
                }
            }
        }
    }
}

/// `message` with its placeholders filled from `args`, its plurals chosen
/// by the rules of the language `tag`. A message that is not well formed
/// is returned as written.
pub fn render(message: &str, tag: &str, args: &Args<'_>) -> String {
    let Ok(parts) = parse(message) else {
        return message.to_owned();
    };
    let mut out = String::with_capacity(message.len() + 16);
    write(&mut out, &parts, tag, args, None);
    out
}

/// What a message asks of its caller and of its language, for checking a
/// catalogue: see [`shape`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Shape {
    /// The names of its placeholders, plural ones included; sorted, once
    /// each.
    pub names: Vec<String>,
    /// Per plural, in order: the placeholder it selects on and the
    /// categories it has forms for.
    pub plurals: Vec<(String, Vec<Category>)>,
}

fn collect(parts: &[Part<'_>], shape: &mut Shape) {
    for part in parts {
        match part {
            Part::Text(_) | Part::Number => {}
            Part::Value(name) => shape.names.push((*name).to_owned()),
            Part::Plural { name, forms } => {
                shape.names.push((*name).to_owned());
                let categories = forms
                    .iter()
                    .filter_map(|(selector, _)| match selector {
                        Selector::Category(category) => Some(*category),
                        Selector::Exact(_) => None,
                    })
                    .collect();
                shape.plurals.push(((*name).to_owned(), categories));
                for (_, text) in forms {
                    collect(text, shape);
                }
            }
        }
    }
}

/// The placeholders and plural forms of `message`, or why it is not well
/// formed.
pub fn shape(message: &str) -> Result<Shape, SyntaxError> {
    let mut shape = Shape::default();
    collect(&parse(message)?, &mut shape);
    shape.names.sort();
    shape.names.dedup();
    Ok(shape)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANNELS: &str = "{count, plural, one {# channel} other {# channels}}";

    #[test]
    fn placeholders_take_their_values_by_name_in_any_order() {
        let args = [("name", "Synthetic".into()), ("group", "News".into())];
        assert_eq!(
            render("Move “{name}” to {group}?", "en", &args),
            "Move “Synthetic” to News?"
        );
        // A translation is free to reorder and to repeat.
        assert_eq!(
            render("{group}: «{name}» ({group})", "ru", &args),
            "News: «Synthetic» (News)"
        );
        assert_eq!(
            render(
                "{n} of {all}",
                "en",
                &[("all", 7u32.into()), ("n", 2u32.into())]
            ),
            "2 of 7"
        );
        assert_eq!(render("No placeholders", "en", &[]), "No placeholders");
    }

    #[test]
    fn a_count_selects_the_form_of_its_language() {
        let of = |tag: &str, message: &str, n: u64| render(message, tag, &[("count", n.into())]);
        assert_eq!(of("en", CHANNELS, 1), "1 channel");
        assert_eq!(of("en", CHANNELS, 0), "0 channels");
        assert_eq!(of("en", CHANNELS, 21), "21 channels");
        let french = "{count, plural, one {# chaîne} other {# chaînes}}";
        assert_eq!(of("fr", french, 0), "0 chaîne");
        assert_eq!(of("fr", french, 2), "2 chaînes");
        let russian = "{count, plural, one {# канал} few {# канала} many {# каналов} \
                       other {# канала}}";
        assert_eq!(of("ru", russian, 1), "1 канал");
        assert_eq!(of("ru", russian, 3), "3 канала");
        assert_eq!(of("ru", russian, 11), "11 каналов");
        assert_eq!(of("ru", russian, 21), "21 канал");
        assert_eq!(of("ru", russian, 25), "25 каналов");
        assert_eq!(
            of("ja", "{count, plural, other {#チャンネル}}", 1),
            "1チャンネル"
        );
    }

    #[test]
    fn a_missing_category_takes_the_general_form() {
        // A whole million is `many` in French; few messages need it.
        let french = "{count, plural, one {# chaîne} other {# chaînes}}";
        let with_many = "{count, plural, one {# chaîne} many {# de chaînes} other {# chaînes}}";
        let million = [("count", 1_000_000u64.into())];
        assert_eq!(render(french, "fr", &million), "1000000 chaînes");
        assert_eq!(render(with_many, "fr", &million), "1000000 de chaînes");
        // English text read by Russian rules: never a missing form.
        assert_eq!(
            render(CHANNELS, "ru", &[("count", 3u64.into())]),
            "3 channels"
        );
    }

    #[test]
    fn an_exact_count_wins_over_its_category() {
        let message = "{count, plural, =0 {No channels} one {# channel} other {# channels}}";
        let of = |n: u64| render(message, "en", &[("count", n.into())]);
        assert_eq!(of(0), "No channels");
        assert_eq!(of(1), "1 channel");
        assert_eq!(of(2), "2 channels");
    }

    #[test]
    fn text_and_placeholders_surround_and_fill_a_plural() {
        let message = "{name}: {count, plural, one {# channel in {group}} \
                       other {# channels in {group}}} · {when}";
        let args = [
            ("name", "Local A".into()),
            ("count", 12u32.into()),
            ("group", "News".into()),
            ("when", "18:00".into()),
        ];
        assert_eq!(
            render(message, "en", &args),
            "Local A: 12 channels in News · 18:00"
        );
        // Two counts in one message, each with its own forms.
        let both = "{channels, plural, one {# channel} other {# channels}} in \
                    {groups, plural, one {# group} other {# groups}}";
        let counts = [("channels", 1u32.into()), ("groups", 3u32.into())];
        assert_eq!(render(both, "en", &counts), "1 channel in 3 groups");
        // Outside a plural `#` is only a character.
        assert_eq!(
            render("Channel #{n}", "en", &[("n", 4u8.into())]),
            "Channel #4"
        );
    }

    #[test]
    fn a_missing_or_mistyped_value_shows_its_placeholder() {
        assert_eq!(render("Delete “{name}”?", "en", &[]), "Delete “{name}”?");
        // A plural needs a count to select on.
        assert_eq!(
            render(CHANNELS, "en", &[("count", "three".into())]),
            "{count}"
        );
    }

    #[test]
    fn a_malformed_message_is_shown_as_written_and_reported_by_shape() {
        for (message, why) in [
            (
                "An {unclosed",
                "expected } or , after the placeholder's name",
            ),
            ("A stray } brace", "a closing brace without an opening one"),
            ("{}", "a placeholder needs a name"),
            (
                "{n, select, a {x} other {y}}",
                "only plural is understood after the name",
            ),
            ("{n, plural, one {x}}", "a plural needs its other form"),
            ("{n, plural, two {x} other {y}}", "not a plural category"),
            (
                "{n, plural, one {x} one {y} other {z}}",
                "a plural form is given twice",
            ),
            ("{n, plural, =x {x} other {y}}", "=N needs a whole number"),
            ("{n, plural, other {y}", "a plural is not closed"),
            ("{n, plural, other {y", "a plural form is not closed"),
            (
                "{n, plural, other y}",
                "a plural form's text goes in braces",
            ),
            ("{n, plural other {y}}", "expected , after plural"),
        ] {
            assert_eq!(shape(message).unwrap_err().why, why, "{message}");
            assert_eq!(render(message, "en", &[("n", 1u8.into())]), message);
        }
    }

    #[test]
    fn shape_lists_placeholders_and_plural_forms() {
        let message = "{name}: {count, plural, =0 {none in {group}} one {# in {group}} \
                       other {# in {group}}}";
        let shape = shape(message).unwrap();
        assert_eq!(shape.names, ["count", "group", "name"]);
        assert_eq!(
            shape.plurals,
            [("count".to_owned(), vec![Category::One, Category::Other])]
        );
        assert_eq!(super::shape("Plain text").unwrap(), Shape::default());
    }
}
