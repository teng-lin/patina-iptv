//! Text the user reads goes through the message catalogue
//! (`crates/iptv-core/src/i18n/messages.tsv`), or is on a list here.
//!
//! Five failures, all found by reading the sources:
//!
//! 1. **A message with no catalogue row.** A string literal passed to `tr`,
//!    `trf` or `key` (or to one of the helpers and message types listed
//!    below, which translate what they are given) must be a row's source
//!    text. Without a row it would stay English in every language, without
//!    a sign.
//! 2. **A message the lint cannot see.** `tr(label)` is given a name, not
//!    a literal; each such name is listed in [`CARRIED_MESSAGES`] beside
//!    the rule that checks what the name carries.
//! 3. **A message the lint cannot follow.** What a message call is given
//!    is a literal, a name, or a parenthesis, a block, an `if`/`else` or a
//!    `match` that ends in these ([`uses`]). Anything else is refused
//!    rather than guessed at: `tr(a + b)`, `tr(&format!(…))`, `.map(tr)`,
//!    `use … tr as t`.
//! 4. **A literal message and values that disagree.** `trf("{count}
//!    items", &[("cnt", …)])` would show `{count}` as written.
//! 5. **A user-facing literal that goes around the catalogue.** Any other
//!    literal that reads as text for a person must be in
//!    `catalogue_allowlist.txt`: under `[untranslated]` when it is shown
//!    and not translated yet, under `[not-shown]` when it never reaches the
//!    screen (a log line, an internal reason, markup, a font name).
//!
//! `[untranslated]` is what the translation does not reach yet. It may
//! only shrink: its size is pinned in [`UNTRANSLATED`], and an entry whose
//! literal is gone from the source fails until it is removed. A new
//! literal is added to the catalogue, not to the list.
//!
//! What reads as text is a heuristic ([`reads_as_text`]): the lint knows
//! no types. It errs towards asking: a literal it wrongly takes for text
//! costs one `[not-shown]` line.
//!
//! # What it does not see
//!
//! The lint reads tokens. It resolves no names and knows no types, so:
//!
//! - **One word outside a call that shows it.** `"ok"`, `"OK"` or `"EPG"`
//!   given to a call that is not in [`TEXT_SINKS`] (`Button::new("OK")`,
//!   a struct field, a `const`) is taken for an identifier, as most such
//!   literals are. Only a word that starts with a capital and goes on in
//!   lower case, or two words, read as text everywhere.
//! - **Text assembled from pieces that are not literals**, and a
//!   `format!` whose literal is placeholders and punctuation only.
//! - **A carrier is a file and a name, not a binding.** Once
//!   `("view.rs", "label")` is listed, every `tr(label)` of that file
//!   passes, also one whose `label` is another variable. The list is
//!   short and each line is reviewed; the lint only keeps it complete.
//! - **A function that translates its argument and is not listed** in
//!   [`TRANSLATING_HELPERS`]: its `tr(label)` asks for a carrier line,
//!   and whoever adds that line has to list the helper too. A helper is
//!   known by its file and name: one called through a `use`, or through
//!   a module path the lint does not match, is not followed.
//! - **A message call under a name the lint does not know**: a macro
//!   that expands to `tr`, a method called `tr` on another type (read as
//!   `tr`, so it errs towards asking), or `translate` and `format` called
//!   as methods (not read as the core's).
//! - **Values not written at the call.** `trf("…", &values)` is checked
//!   against a `let values = [("name", …)];` earlier in the same
//!   function, and not at all when the values come from further away.
//! - **Messages kept as data** ([`MESSAGE_FILES`], [`MESSAGE_CALLS_IN`])
//!   are the literals that read as text there; a text an error builds
//!   with `format!` has no row to be found by.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use iptv_core::i18n::is_catalogued;

/// How many `[untranslated]` entries there are. Lower it with the list;
/// it is never raised.
const UNTRANSLATED: usize = 439;

/// Source directories read, relative to the workspace's `crates/`.
const ROOTS: &[&str] = &[
    "iptv-app/src",
    "iptv-core/src",
    "iptv-guide/src",
    "iptv-library/src",
    "iptv-runtime/src",
    // What a settings row says under the list when it is focused.
    "iptv-settings/src/catalogue",
];

/// The catalogue itself and its formatter: messages about messages.
const NOT_READ: &[&str] = &[
    "iptv-core/src/i18n.rs",
    "iptv-core/src/i18n/",
    // Declared only by opening.rs's #[cfg(test)] mod revocation; synthetic fixtures.
    "iptv-runtime/src/opening/revocation.rs",
];

/// Calls whose literal is a message: it must be a catalogue row.
/// `translate` and `format` are the core's own (`i18n::format`; the
/// `format!` macro is another name). What they are given when it is not a
/// literal is followed by [`uses`].
const MESSAGE_CALLS: &[&str] = &["tr", "trf", "key", "translate", "format"];

/// Messages that travel as data and are translated where they are shown:
/// `RuntimeEvent::Notify("…")` is shown by `runtime_events.rs` through
/// `tr`. What one is made of is followed like `tr`'s argument; a pattern
/// that binds it (`Notify(message) => …`) reads the same, and its binding
/// is the carrier.
const MESSAGE_TYPES: &[&str] = &["Notify"];

/// Functions that pass a `&'static str` argument to `tr` themselves:
/// file, function, and which argument (from 0). A literal there is a
/// message, and anything else is followed like `tr`'s argument: the
/// helper's own `tr(label)` is only as good as what its callers give it.
/// Calls in the file itself are read, and those through the file's module
/// (`wizard_view::button(…)`, `super::button(…)`).
const TRANSLATING_HELPERS: &[(&str, &str, usize)] = &[
    ("iptv-app/src/fav_editor.rs", "button", 0),
    // `.label(move || tr(name))`, and the tile buttons' `tr(what)`.
    ("iptv-app/src/multiview_view/chrome.rs", "pill", 1),
    ("iptv-app/src/multiview_view/chrome.rs", "label", 0),
    ("iptv-app/src/play_bar.rs", "button", 1),
    ("iptv-app/src/search_view/view.rs", "button", 2),
    ("iptv-app/src/settings_view/editors.rs", "button", 2),
    ("iptv-app/src/settings_view/words.rs", "with_unit", 0),
    ("iptv-app/src/wizard_view.rs", "button", 2),
    ("iptv-app/src/wizard_view.rs", "hint", 2),
    ("iptv-app/src/wizard_view.rs", "hint_or_back", 2),
    ("iptv-app/src/wizard_view/entry.rs", "button", 2),
];

/// Files whose every literal that reads as text is a message shown later
/// through `tr`: a backup error keeps its English text for the log and is
/// translated when the user is told (`backup::report`, `App`).
const MESSAGE_FILES: &[&str] = &[
    "iptv-runtime/src/backup.rs",
    "iptv-runtime/src/backup/instance.rs",
];

/// The same for single calls of a file: the backup errors the view makes
/// itself are the literals of its `context` and `Err`.
const MESSAGE_CALLS_IN: &[(&str, &[&str])] = &[("iptv-app/src/backup.rs", &["context", "Err"])];

/// The names of the built-in lists, made where the lists are and
/// translated by the shell's `lists::entry_name`.
const LIST_NAMES: &[(&str, &str)] = &[
    ("iptv-core/src/guide.rs", "All channels"),
    ("iptv-core/src/lists.rs", "Favourites"),
    ("iptv-core/src/lists.rs", "Recently watched"),
    ("iptv-guide/src/combined.rs", "All playlists"),
];

/// Where a message call, a message type or a translating helper is given
/// something other than a literal: file, and the name it starts with (by
/// its path: `words::title` of `tr(words::title(kind))`). Each is a
/// message written elsewhere that the rules above already check: a
/// `key(…)` constant, a helper's label, a backup error's text, a notice,
/// a built-in list's name. A new one needs the same before it is listed.
const CARRIED_MESSAGES: &[(&str, &str)] = &[
    // What `tr` and `trf` hand to the core is what they were given.
    ("iptv-app/src/i18n.rs", "source"),
    // `key(…)` constants.
    ("iptv-app/src/backup.rs", "EXPORT_NOTE"),
    ("iptv-app/src/backup.rs", "PENDING_NOTE"),
    ("iptv-app/src/backup.rs", "RESTORE_NOTE"),
    ("iptv-app/src/backup.rs", "RESTART_NOTE"),
    ("iptv-app/src/custom_groups.rs", "CAPTION"),
    ("iptv-app/src/remote_editor.rs", "CAPTION"),
    // A backup error's text (MESSAGE_FILES, MESSAGE_CALLS_IN).
    ("iptv-app/src/backup.rs", "report"),
    ("iptv-app/src/lib.rs", "message"),
    // `RuntimeEvent::Notify` (MESSAGE_TYPES).
    ("iptv-app/src/runtime_events.rs", "message"),
    // The built-in lists (LIST_NAMES).
    ("iptv-app/src/lists.rs", "name"),
    // The labels of TRANSLATING_HELPERS.
    ("iptv-app/src/fav_editor.rs", "label"),
    ("iptv-app/src/multiview_view/chrome.rs", "name"),
    ("iptv-app/src/multiview_view/chrome.rs", "what"),
    ("iptv-app/src/play_bar.rs", "name"),
    ("iptv-app/src/search_view/view.rs", "label"),
    ("iptv-app/src/settings_view/editors.rs", "label"),
    ("iptv-app/src/settings_view/words.rs", "message"),
    ("iptv-app/src/wizard_view.rs", "keys"),
    ("iptv-app/src/wizard_view.rs", "label"),
    ("iptv-app/src/wizard_view/entry.rs", "label"),
];

/// Calls whose text goes to the log, a panic or the platform, never to
/// the screen. `concat!` is not one: it makes a literal like any other
/// ([`joined`]).
const SILENT_CALLS: &[&str] = &[
    // Logging and diagnostics.
    "trace!",
    "debug!",
    "info!",
    "warn!",
    "error!",
    "eprintln!",
    "println!",
    "panic!",
    "unreachable!",
    "unimplemented!",
    "todo!",
    "assert!",
    "assert_eq!",
    "assert_ne!",
    "debug_assert!",
    "debug_assert_eq!",
    "debug_assert_ne!",
    "expect",
    // `startup::mark("phase")`, and the operation `backup::report` logs.
    "mark",
    "mark_with",
    "report",
    // The class of an error's cause, kept for the log (`backup/error.rs`).
    "Step",
    "Store",
    // Environment and build.
    "var",
    "env!",
    "option_env!",
    "include_str!",
    "include_bytes!",
    "cfg!",
    // Identity for tests.
    "test_id",
];

/// Calls that show their argument: there a single word is text too
/// (`text("ended")`), which elsewhere would be taken for an identifier.
const TEXT_SINKS: &[&str] = &[
    "text",
    "title",
    "heading",
    "body",
    "caption",
    "label",
    "detail",
    "notify",
    "description",
    "placeholder",
    "header",
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Ident(String),
    Punct(char),
    /// A string literal's value; byte strings are not text.
    Str(String),
    /// A number or a character literal: a value that is never a message.
    Other,
}

/// A Rust source as the tokens this lint needs: identifiers, punctuation
/// and string literals with their escapes resolved; a number or a
/// character literal is only known to be there. Comments and the quote
/// of a lifetime are dropped.
fn tokens(source: &str) -> Vec<Token> {
    // As the compiler reads it: a line ends in `\n`, also in a checkout
    // with `\r\n`, where a continued string would keep its break.
    let chars: Vec<char> = source.replace("\r\n", "\n").chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 1;
            i += 2;
            while i < chars.len() && depth > 0 {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if c == '"' {
            let (text, end) = quoted(&chars, i + 1);
            out.push(Token::Str(text));
            i = end;
        } else if c == '\'' {
            i = match char_literal(&chars, i) {
                Some(end) => {
                    out.push(Token::Other);
                    end
                }
                None => i + 1,
            };
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            // `r"…"`, `r#"…"#`, `b"…"`, `br"…"`, `c"…"`.
            let raw = matches!(word.as_str(), "r" | "br" | "cr");
            let bytes = matches!(word.as_str(), "b" | "br");
            let prefix = raw || matches!(word.as_str(), "b" | "c");
            let hashes = chars[i..].iter().take_while(|c| **c == '#').count();
            if raw && chars.get(i + hashes) == Some(&'"') {
                let (text, end) = raw_quoted(&chars, i + hashes + 1, hashes);
                if !bytes {
                    out.push(Token::Str(text));
                }
                i = end;
            } else if prefix && chars.get(i) == Some(&'"') {
                let (text, end) = quoted(&chars, i + 1);
                if !bytes {
                    out.push(Token::Str(text));
                }
                i = end;
            } else if let Some(end) = (word == "b").then(|| char_literal(&chars, i)).flatten() {
                // `b'a'`.
                out.push(Token::Other);
                i = end;
            } else {
                out.push(Token::Ident(word));
            }
        } else if c.is_ascii_digit() {
            // `10`, `1_000u32`, `0x1F`, `1.5`; not the dot of `1.into()`.
            while i < chars.len()
                && (chars[i].is_alphanumeric()
                    || chars[i] == '_'
                    || chars[i] == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
            {
                i += 1;
            }
            out.push(Token::Other);
        } else if c.is_whitespace() {
            i += 1;
        } else {
            out.push(Token::Punct(c));
            i += 1;
        }
    }
    out
}

/// The value of a `"…"` literal from just after its opening quote, and
/// the index after its closing quote.
fn quoted(chars: &[char], mut i: usize) -> (String, usize) {
    let mut text = String::new();
    while i < chars.len() && chars[i] != '"' {
        if chars[i] != '\\' {
            text.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        match chars.get(i).copied() {
            Some('n') => text.push('\n'),
            Some('t') => text.push('\t'),
            Some('r') => text.push('\r'),
            Some('0') => text.push('\0'),
            // A line continuation: the break and the next line's indent go.
            Some('\n') => {
                while chars.get(i).is_some_and(|c| c.is_whitespace()) {
                    i += 1;
                }
                continue;
            }
            Some('u') if chars.get(i + 1) == Some(&'{') => {
                let end = (i..chars.len()).find(|j| chars[*j] == '}').unwrap_or(i);
                let hex: String = chars[i + 2..end].iter().collect();
                text.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                i = end;
            }
            Some('x') => {
                let hex: String = chars[i + 1..(i + 3).min(chars.len())].iter().collect();
                text.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                i += 2;
            }
            Some(other) => text.push(other),
            None => {}
        }
        i += 1;
    }
    (text, i + 1)
}

fn raw_quoted(chars: &[char], start: usize, hashes: usize) -> (String, usize) {
    let mut i = start;
    while i < chars.len() {
        let closes = chars[i] == '"'
            && chars[i + 1..]
                .iter()
                .take(hashes)
                .filter(|c| **c == '#')
                .count()
                == hashes;
        if closes {
            return (chars[start..i].iter().collect(), i + 1 + hashes);
        }
        i += 1;
    }
    (chars[start..].iter().collect(), chars.len())
}

/// The index after the character literal whose quote is at `at` (`'a'`,
/// `'\n'`, `'\\'`, `'\''`, `'\u{1F}'`); `None` for the quote of a
/// lifetime (`'static`).
fn char_literal(chars: &[char], at: usize) -> Option<usize> {
    match (chars.get(at + 1), chars.get(at + 2)) {
        // What a backslash escapes is not the closing quote, whatever it
        // is: the quote of `'\''`, the second backslash of `'\\'`. The
        // longest escape is `\u{10FFFF}`.
        (Some('\\'), Some(_)) => Some(
            (at + 3..chars.len().min(at + 13))
                .find(|i| chars[*i] == '\'')
                .map_or(at + 3, |i| i + 1),
        ),
        (Some(_), Some('\'')) => Some(at + 3),
        _ => None,
    }
}

/// One literal where the sources have it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Literal {
    text: String,
    /// The call it is an argument of, innermost: `tr`, `format!`, `Err`.
    /// `None` outside any call (a `const`, a `match` arm, a tuple).
    call: Option<String>,
    /// Which argument of that call it is in, from 0.
    argument: usize,
    /// It is what a message call, a message type or a translating helper
    /// is given ([`uses`]), however it is wrapped there.
    message: bool,
}

/// The tokens of `source` without its attributes and its test code
/// (`#[cfg(test)]` and `#[test]` items).
fn code(source: &str) -> Vec<Token> {
    let tokens = tokens(source);
    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == Token::Punct('#') && tokens.get(i + 1) == Some(&Token::Punct('[')) {
            let end = matching(&tokens, i + 1);
            let attribute = &tokens[i + 2..end];
            let has = |word: &str| attribute.contains(&Token::Ident(word.to_owned()));
            // `#[cfg(test)]` and `#[test]`, not `#[cfg(not(test))]`.
            i = if has("test") && !has("not") {
                after_item(&tokens, end + 1)
            } else {
                end + 1
            };
            continue;
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    // Innermost first: a `concat!` of a `concat!` takes two passes.
    loop {
        let before = out.len();
        out = joined(out);
        if out.len() == before {
            return out;
        }
    }
}

/// `tokens` with each `concat!("a", "b")` as the one literal the compiler
/// makes of it, so that it is read as any other. One with a piece that is
/// not a string literal stays as written: its pieces are read one by one,
/// and it is nothing a message call can be given.
fn joined(tokens: Vec<Token>) -> Vec<Token> {
    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        if let Some((text, end)) = concatenated(&tokens, i) {
            out.push(Token::Str(text));
            i = end;
        } else {
            out.push(tokens[i].clone());
            i += 1;
        }
    }
    out
}

/// The text of the `concat!` of string literals that starts at `at`, and
/// the index after it.
fn concatenated(tokens: &[Token], at: usize) -> Option<(String, usize)> {
    let opens = tokens.get(at) == Some(&Token::Ident("concat".to_owned()))
        && tokens.get(at + 1) == Some(&Token::Punct('!'))
        && matches!(tokens.get(at + 2), Some(Token::Punct('(' | '[' | '{')));
    if !opens {
        return None;
    }
    let end = matching(tokens, at + 2);
    let mut text = String::new();
    for (n, token) in tokens[at + 3..end].iter().enumerate() {
        match token {
            Token::Str(piece) if n % 2 == 0 => text.push_str(piece),
            Token::Punct(',') if n % 2 == 1 => {}
            _ => return None,
        }
    }
    Some((text, end + 1))
}

/// The string literals of `source` (the file `file`) outside test code
/// and attributes, each with the call it is in.
fn literals(file: &str, source: &str) -> Vec<Literal> {
    let tokens = code(source);
    let messages = uses(file, &tokens).messages;
    // Open delimiters: the call's name for a parenthesis that follows one,
    // and how many commas it has seen.
    let mut open: Vec<(char, Option<String>, usize)> = Vec::new();
    let mut out = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        match token {
            Token::Punct(c @ ('(' | '[' | '{')) => {
                let call = (*c == '(').then(|| callee(&tokens, i)).flatten();
                open.push((*c, call, 0));
            }
            Token::Punct(')' | ']' | '}') => {
                open.pop();
            }
            Token::Punct(',') => {
                if let Some(top) = open.last_mut() {
                    top.2 += 1;
                }
            }
            // A pattern (`"a" => …`, `"a" | "b" => …`) is compared, not shown.
            Token::Str(_)
                if matches!(
                    (tokens.get(i + 1), tokens.get(i + 2)),
                    (Some(Token::Punct('=')), Some(Token::Punct('>')))
                        | (Some(Token::Punct('|')), _)
                ) => {}
            Token::Str(text) => {
                // The nearest parenthesis, through blocks and brackets: a
                // `match` inside `tr(…)` is still its argument.
                let at = open.iter().rev().find(|(c, _, _)| *c == '(');
                out.push(Literal {
                    text: text.clone(),
                    call: at.and_then(|(_, call, _)| call.clone()),
                    argument: at.map_or(0, |(_, _, commas)| *commas),
                    message: messages.contains(&i),
                });
            }
            _ => {}
        }
    }
    out
}

/// How a file gives its messages to the calls that translate them.
#[derive(Debug, Default)]
struct Uses {
    /// The tokens that are a literal given as a message, however it is
    /// wrapped: `"A"` and `"B"` of `tr(if x { ("A") } else { "B" })`.
    messages: BTreeSet<usize>,
    /// What carries a message that is not a literal where it is given:
    /// the name the expression starts with, by its path (`label` of
    /// `tr(label)`, `error` of `tr(error.message())`, `report` of
    /// `tr(report(…))`, `words::title` of `tr(words::title(kind))`).
    carried: Vec<String>,
    /// What the lint cannot follow, each as the line a failure prints.
    unproven: Vec<String>,
    /// Literal messages whose placeholders are not the values given.
    misfilled: Vec<String>,
}

/// Reads every place where `tokens` (the code of `file`) hand a message
/// to something that translates it: `tr`, `trf` and `key`, the core's
/// `translate` and `format`, a message type, a translating helper.
///
/// What is given must be something the lint can follow to its end: a
/// literal, a name with its path, fields and calls (a carrier, for
/// [`CARRIED_MESSAGES`]), or a parenthesis, a block, an `if`/`else` or a
/// `match` whose every outcome is one of these. Anything else is refused,
/// not guessed at: this is no parser of Rust, and what it cannot prove it
/// asks to be written plainly.
fn uses(file: &str, tokens: &[Token]) -> Uses {
    let mut uses = Uses::default();
    let is =
        |i: usize, word: &str| matches!(tokens.get(i), Some(Token::Ident(name)) if name == word);
    let punct = |i: usize, c: char| tokens.get(i) == Some(&Token::Punct(c));
    let mut i = 0;
    while i < tokens.len() {
        let Token::Ident(name) = &tokens[i] else {
            i += 1;
            continue;
        };
        let name = name.as_str();
        // An import names the calls without calling them. Under another
        // name (`use crate::i18n::tr as t`) their calls would not be seen.
        if name == "use" && !punct(i + 1, '<') {
            let end = (i..tokens.len())
                .find(|j| punct(*j, ';'))
                .unwrap_or(tokens.len());
            let core = (i..end).any(|j| is(j, "i18n"));
            for (j, token) in tokens.iter().enumerate().take(end).skip(i) {
                let Token::Ident(word) = token else {
                    continue;
                };
                let word = word.as_str();
                let translates = matches!(word, "tr" | "trf")
                    || core && matches!(word, "key" | "translate" | "format")
                    || MESSAGE_TYPES.contains(&word);
                if translates && is(j + 1, "as") && !is(j + 2, "_") {
                    uses.unproven
                        .push(format!("use … {word} as …: import it under its own name"));
                }
            }
            i = end + 1;
            continue;
        }
        // An `enum` declares its variants (`Notify(&'static str)`).
        if name == "enum" {
            let body = (i..tokens.len()).find(|j| punct(*j, '{'));
            i = body.map_or(tokens.len(), |open| matching(tokens, open) + 1);
            continue;
        }
        let declared = i > 0 && is(i - 1, "fn");
        if declared {
            i += 1;
            continue;
        }
        if !punct(i + 1, '(') {
            // `.map(tr)`, `let say = i18n::format;`: what the function
            // is given later is out of sight. A field (`x.tr`) is not it.
            let core = i >= 3 && is(i - 3, "i18n") && punct(i - 2, ':') && punct(i - 1, ':');
            let named = matches!(name, "tr" | "trf") && !(i > 0 && punct(i - 1, '.'))
                || core && matches!(name, "key" | "translate" | "format");
            if named {
                uses.unproven.push(format!(
                    "{name} without its arguments: call it where the message is"
                ));
            }
            i += 1;
            continue;
        }
        let arguments = arguments(tokens, i + 1);
        let message = message_position(file, tokens, i, arguments.len())
            .and_then(|(kind, position)| Some((kind, position, *arguments.get(position)?)));
        if let Some((kind, position, (lo, hi))) = message {
            let given = &tokens[lo..hi];
            let skipped = match kind {
                // A pattern: `Notify(_)`.
                Translator::Type => {
                    matches!(given, [Token::Ident(any)] if any == "_")
                        || matches!(given, [Token::Punct('.'), Token::Punct('.')])
                }
                // A closure is no `&'static str`: such a call is not the
                // helper's (`.label(move || tr(name))`), and what the
                // closure shows is read at the calls inside it.
                Translator::Helper => match given {
                    [Token::Punct('|'), ..] => true,
                    [Token::Ident(word), Token::Punct('|'), ..] => word == "move",
                    _ => false,
                },
                Translator::Plain | Translator::Filled => false,
            };
            let mut leaves = Leaves::default();
            let proven = !skipped && leaves_of(tokens, lo, hi, &mut leaves);
            // `key` marks a literal. A `key(…)` of anything else is a
            // view's own function (`key(a.name)`) and translates nothing:
            // what it returns is not shown before it passes `tr`.
            let skipped = skipped || name == "key" && leaves.literals.is_empty();
            if skipped {
            } else if proven {
                let texts = || {
                    leaves.literals.iter().filter_map(|at| match &tokens[*at] {
                        Token::Str(text) => Some(text.as_str()),
                        _ => None,
                    })
                };
                match kind {
                    Translator::Filled => {
                        let values = arguments.get(position + 1);
                        let written = values.and_then(|(lo, hi)| value_names(tokens, *lo, *hi));
                        let shared = || values.and_then(|(lo, hi)| shared_names(tokens, *lo, *hi));
                        // A carrier among the outcomes may place the
                        // rest, and so may another call of shared values.
                        let all = leaves.carried.is_empty() && written.is_some();
                        let values = written.or_else(shared);
                        misfilled(name, texts(), values.as_deref(), all, &mut uses.misfilled);
                    }
                    // `tr` fills nothing: a placeholder would be shown.
                    Translator::Plain if matches!(name, "tr" | "translate") => {
                        misfilled(name, texts(), Some(&[]), false, &mut uses.misfilled);
                    }
                    _ => {}
                }
                uses.messages.extend(leaves.literals);
                uses.carried.extend(leaves.carried);
            } else {
                uses.unproven.push(format!("{name}({})", written(given)));
            }
        }
        i += 1;
    }
    uses
}

/// What translates the message it is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Translator {
    /// `tr`, `key`, the core's `translate`.
    Plain,
    /// `trf` and the core's `format`: the next argument is the values.
    Filled,
    /// One of [`MESSAGE_TYPES`].
    Type,
    /// One of [`TRANSLATING_HELPERS`].
    Helper,
}

/// Whether the call of the name at `at`, with `given` arguments, is one
/// that translates, and which argument is its message, from 0.
fn message_position(
    file: &str,
    tokens: &[Token],
    at: usize,
    given: usize,
) -> Option<(Translator, usize)> {
    let Token::Ident(name) = &tokens[at] else {
        return None;
    };
    let before = |back: usize| at.checked_sub(back).map(|i| &tokens[i]);
    let method = before(1) == Some(&Token::Punct('.'));
    // The path it is called through: `wizard_view` of
    // `wizard_view::button(…)`.
    let qualifier = match (before(3), before(2), before(1)) {
        (Some(Token::Ident(module)), Some(Token::Punct(':')), Some(Token::Punct(':'))) => {
            Some(module.as_str())
        }
        _ => None,
    };
    match name.as_str() {
        "tr" => Some((Translator::Plain, 0)),
        "trf" => Some((Translator::Filled, 0)),
        // `i18n::key` takes the message alone; a view's `key(ui, action)`
        // is a key press. The core's two take the language first.
        "key" if !method && given == 1 => Some((Translator::Plain, 0)),
        "translate" if !method && given == 2 => Some((Translator::Plain, 1)),
        "format" if !method && given == 3 => Some((Translator::Filled, 1)),
        name if MESSAGE_TYPES.contains(&name) => Some((Translator::Type, 0)),
        name => TRANSLATING_HELPERS
            .iter()
            .find(|(home, helper, _)| {
                *helper == name
                    && match qualifier {
                        None | Some("self" | "Self") => *home == file,
                        // `wizard_view/entry.rs` is in `wizard_view.rs`.
                        Some("super") => Path::new(file)
                            .parent()
                            .is_some_and(|dir| Path::new(home).with_extension("") == dir),
                        Some(module) => Path::new(home)
                            .file_stem()
                            .is_some_and(|stem| stem == module),
                    }
            })
            .map(|(_, _, position)| (Translator::Helper, *position)),
    }
}

/// What a message expression can evaluate to.
#[derive(Debug, Default)]
struct Leaves {
    /// The tokens of its literals.
    literals: Vec<usize>,
    /// The names its other outcomes start with.
    carried: Vec<String>,
}

/// Whether the lint can tell everything `tokens[lo..hi]` evaluates to,
/// which it then adds to `out`.
fn leaves_of(tokens: &[Token], mut lo: usize, hi: usize, out: &mut Leaves) -> bool {
    while lo < hi && matches!(tokens[lo], Token::Punct('&' | '*')) {
        lo += 1;
    }
    match &tokens[lo..hi] {
        [Token::Str(_)] => {
            out.literals.push(lo);
            true
        }
        [Token::Punct('('), ..] => {
            matching(tokens, lo) == hi - 1 && leaves_of(tokens, lo + 1, hi - 1, out)
        }
        [Token::Punct('{'), ..] => {
            matching(tokens, lo) == hi - 1 && block(tokens, lo + 1, hi - 1, out)
        }
        [Token::Ident(word), ..] if word == "if" => conditional(tokens, lo, hi, out),
        [Token::Ident(word), ..] if word == "match" => matched(tokens, lo, hi, out),
        // A name with its path, fields, calls and indices: what it holds
        // is checked where it is made (CARRIED_MESSAGES). `return` and
        // the like are no name.
        [Token::Ident(name), ..]
            if !NOT_A_NAME.contains(&name.as_str()) && chain(&tokens[lo + 1..hi]) =>
        {
            // By its whole path: `crate::words::title`, not `crate`.
            let mut name = name.clone();
            let mut at = lo + 1;
            while let [Token::Punct(':'), Token::Punct(':'), Token::Ident(part), ..] =
                &tokens[at..hi]
            {
                name = format!("{name}::{part}");
                at += 3;
            }
            out.carried.push(name);
            true
        }
        _ => false,
    }
}

/// Keywords an expression can start with that are not the name of what
/// carries a message (`self`, `crate` and `super` start one).
const NOT_A_NAME: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "dyn", "else", "false", "fn", "for",
    "impl", "in", "let", "loop", "move", "mut", "ref", "return", "static", "true", "unsafe",
    "where", "while", "yield",
];

/// `::name`, `.name`, `.0`, `?`, `(…)` and `[…]`, any number of them.
fn chain(mut rest: &[Token]) -> bool {
    loop {
        rest = match rest {
            [] => return true,
            [
                Token::Punct(':'),
                Token::Punct(':'),
                Token::Ident(_),
                rest @ ..,
            ]
            | [Token::Punct('.'), Token::Ident(_) | Token::Other, rest @ ..]
            | [Token::Punct('?'), rest @ ..] => rest,
            [Token::Punct('(' | '['), ..] => &rest[matching(rest, 0) + 1..],
            _ => return false,
        };
    }
}

/// The value of the block whose inside is `tokens[lo..hi]`: what follows
/// its last statement.
fn block(tokens: &[Token], lo: usize, hi: usize, out: &mut Leaves) -> bool {
    let mut tail = lo;
    let mut i = lo;
    while i < hi {
        match tokens[i] {
            Token::Punct('(' | '[' | '{') => i = matching(tokens, i),
            Token::Punct(';') => tail = i + 1,
            _ => {}
        }
        i += 1;
    }
    leaves_of(tokens, tail, hi, out)
}

/// `if … { a } else if … { b } else { c }` at `tokens[lo..hi]`.
fn conditional(tokens: &[Token], lo: usize, hi: usize, out: &mut Leaves) -> bool {
    let Some(open) = outside(tokens, lo + 1, hi, |i| tokens[i] == Token::Punct('{')) else {
        return false;
    };
    let close = matching(tokens, open);
    let other = close + 1;
    // Without an `else` there is nothing a message could be.
    if other + 1 >= hi || tokens[other] != Token::Ident("else".to_owned()) {
        return false;
    }
    block(tokens, open + 1, close, out)
        && match &tokens[other + 1] {
            Token::Ident(word) if word == "if" => conditional(tokens, other + 1, hi, out),
            Token::Punct('{') => {
                matching(tokens, other + 1) == hi - 1 && block(tokens, other + 2, hi - 1, out)
            }
            _ => false,
        }
}

/// `match … { pattern => a, pattern => { b } … }` at `tokens[lo..hi]`.
fn matched(tokens: &[Token], lo: usize, hi: usize, out: &mut Leaves) -> bool {
    let Some(open) = outside(tokens, lo + 1, hi, |i| tokens[i] == Token::Punct('{')) else {
        return false;
    };
    let end = hi - 1;
    if matching(tokens, open) != end {
        return false;
    }
    let mut arms = 0;
    let mut i = open + 1;
    while i < end {
        let arrow = outside(tokens, i, end, |i| {
            tokens[i] == Token::Punct('=') && tokens[i + 1] == Token::Punct('>')
        });
        let Some(value) = arrow.map(|arrow| arrow + 2) else {
            return false;
        };
        let after = if value < end && tokens[value] == Token::Punct('{') {
            matching(tokens, value) + 1
        } else {
            outside(tokens, value, end, |i| tokens[i] == Token::Punct(',')).unwrap_or(end)
        };
        // `{ … }.len()` is no block any more.
        let goes_on = after < end && matches!(tokens[after], Token::Punct('.' | '?'));
        if goes_on || !leaves_of(tokens, value, after, out) {
            return false;
        }
        i = after;
        if i < end && tokens[i] == Token::Punct(',') {
            i += 1;
        }
        arms += 1;
    }
    arms > 0
}

/// The first index in `from..to` where `found` holds, outside any
/// parenthesis, bracket or brace that opens in between.
fn outside(
    tokens: &[Token],
    from: usize,
    to: usize,
    found: impl Fn(usize) -> bool,
) -> Option<usize> {
    let mut i = from;
    while i < to {
        if found(i) {
            return Some(i);
        }
        if matches!(tokens[i], Token::Punct('(' | '[' | '{')) {
            i = matching(tokens, i);
        }
        i += 1;
    }
    None
}

/// The arguments of the call, or the elements of the slice, that opens at
/// `open`: where each starts and ends.
fn arguments(tokens: &[Token], open: usize) -> Vec<(usize, usize)> {
    let close = matching(tokens, open);
    let mut out = Vec::new();
    let mut start = open + 1;
    let mut i = start;
    while i < close {
        match &tokens[i] {
            Token::Punct('(' | '[' | '{') => i = matching(tokens, i),
            // The parameters of a closure: `|a, b| …`, `move |a, b| …`.
            Token::Punct('|') if i == start || tokens[i - 1] == Token::Ident("move".to_owned()) => {
                i = (i + 1..close)
                    .find(|j| tokens[*j] == Token::Punct('|'))
                    .unwrap_or(i);
            }
            // The types of a turbofish: `collect::<HashMap<_, _>>()`.
            Token::Punct('<')
                if i >= 2
                    && tokens[i - 1] == Token::Punct(':')
                    && tokens[i - 2] == Token::Punct(':') =>
            {
                let mut depth = 0usize;
                for j in i..close {
                    match tokens[j] {
                        Token::Punct('<') => depth += 1,
                        // Not the arrow of `fn() -> T`.
                        Token::Punct('>') if tokens[j - 1] != Token::Punct('-') => depth -= 1,
                        _ => {}
                    }
                    if depth == 0 {
                        i = j;
                        break;
                    }
                }
            }
            Token::Punct(',') => {
                out.push((start, i));
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < close {
        out.push((start, close));
    }
    out
}

/// The names `tokens[lo..hi]` gives values for when it is written as
/// `&[("name", value), …]`; `None` when the values are made elsewhere
/// (`&count`).
fn value_names(tokens: &[Token], mut lo: usize, hi: usize) -> Option<Vec<String>> {
    while lo < hi && tokens[lo] == Token::Punct('&') {
        lo += 1;
    }
    if tokens.get(lo) != Some(&Token::Punct('[')) || matching(tokens, lo) != hi - 1 {
        return None;
    }
    arguments(tokens, lo)
        .into_iter()
        .map(|(lo, hi)| match &tokens[lo..hi] {
            [Token::Punct('('), Token::Str(name), Token::Punct(','), ..] => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// As [`value_names`] for values several calls share: for `&args` at
/// `tokens[lo..hi]`, the names of the nearest `let args = [("name",
/// value), …];` before it in the same function.
fn shared_names(tokens: &[Token], mut lo: usize, hi: usize) -> Option<Vec<String>> {
    while lo < hi && tokens[lo] == Token::Punct('&') {
        lo += 1;
    }
    let [Token::Ident(name)] = &tokens[lo..hi] else {
        return None;
    };
    let word = |i: usize, word: &str| matches!(&tokens[i], Token::Ident(name) if name == word);
    let function = (0..lo).rev().find(|i| word(*i, "fn")).unwrap_or(0);
    let made = (function..lo.saturating_sub(3))
        .rev()
        .find(|i| word(*i, "let") && word(*i + 1, name) && tokens[*i + 2] == Token::Punct('='))?;
    let open = made + 3;
    if tokens[open] != Token::Punct('[') {
        return None;
    }
    value_names(tokens, open, matching(tokens, open) + 1)
}

/// Adds what is wrong between the literal `messages` given to `call` and
/// the names it has values for, `given`: a placeholder without a value is
/// shown as written (`{count}`), and a value no message places says the
/// two have drifted apart (only when `all` the messages are here). `None`
/// for values the lint does not see: then only the messages themselves
/// are checked.
fn misfilled<'a>(
    call: &str,
    messages: impl Iterator<Item = &'a str>,
    given: Option<&[String]>,
    all: bool,
    out: &mut Vec<String>,
) {
    let mut placed = BTreeSet::new();
    let mut all = all;
    for message in messages {
        match iptv_core::i18n::message::shape(message) {
            Ok(shape) => {
                for name in &shape.names {
                    if given.is_some_and(|given| !given.contains(name)) {
                        out.push(format!("{call}({message:?}…): no value for {{{name}}}"));
                    }
                }
                placed.extend(shape.names);
            }
            Err(error) => {
                // What it would have placed is not known.
                all = false;
                out.push(format!(
                    "{call}({message:?}…): {} at byte {}",
                    error.why, error.at
                ));
            }
        }
    }
    for name in given.filter(|_| all).unwrap_or_default() {
        if !placed.contains(name) {
            out.push(format!("{call}(…): the value {name:?} has no placeholder"));
        }
    }
}

/// `tokens` roughly as they were written, for a failure's line.
fn written(tokens: &[Token]) -> String {
    let mut out = String::new();
    for token in tokens {
        match token {
            Token::Ident(name) => {
                if out.ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                    out.push(' ');
                }
                out.push_str(name);
            }
            Token::Punct(c) => out.push(*c),
            Token::Str(text) => out.push_str(&format!("{text:?}")),
            Token::Other => out.push('…'),
        }
    }
    if out.chars().count() > 60 {
        out = out.chars().take(60).chain(['…']).collect();
    }
    out
}

/// The name called by the parenthesis at `at`: the identifier before it,
/// with `!` for a macro. `None` for a tuple or a grouping.
fn callee(tokens: &[Token], at: usize) -> Option<String> {
    match (at.checked_sub(1).map(|i| &tokens[i]), at.checked_sub(2)) {
        (Some(Token::Ident(name)), _) => Some(name.clone()),
        (Some(Token::Punct('!')), Some(before)) => match &tokens[before] {
            Token::Ident(name) => Some(format!("{name}!")),
            _ => None,
        },
        _ => None,
    }
}

/// The index of the delimiter that closes the one at `at`.
fn matching(tokens: &[Token], at: usize) -> usize {
    let mut depth = 0usize;
    for (i, token) in tokens.iter().enumerate().skip(at) {
        match token {
            Token::Punct('(' | '[' | '{') => depth += 1,
            Token::Punct(')' | ']' | '}') => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    tokens.len().saturating_sub(1)
}

/// The index after the item that starts at `at`: after its `;`, or after
/// the block that ends it.
fn after_item(tokens: &[Token], at: usize) -> usize {
    let mut i = at;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Punct(';') => return i + 1,
            Token::Punct('{') => return matching(tokens, i) + 1,
            Token::Punct('(' | '[') => i = matching(tokens, i),
            _ => {}
        }
        i += 1;
    }
    tokens.len()
}

/// Whether `text` reads as something written for a person: words, not an
/// identifier, a path or a format of placeholders only. `shown` is for a
/// literal handed to a call that displays it, where one word is enough.
fn reads_as_text(text: &str, shown: bool) -> bool {
    // What a placeholder names is not text: `{name} ({keyboard})`.
    let mut plain = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let plain = plain.trim();
    let mut run = 0;
    let mut word = false;
    for c in plain.chars() {
        run = if c.is_alphabetic() { run + 1 } else { 0 };
        word |= run >= 2;
    }
    if !word {
        return false;
    }
    let mut letters = plain.chars();
    let capitalised = letters.next().is_some_and(char::is_uppercase)
        && letters.next().is_some_and(char::is_lowercase);
    let spaced = plain.split_whitespace().count() > 1;
    capitalised || spaced || shown
}

/// What the lint decided about a literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// A message: it needs a catalogue row.
    Message,
    /// Text that goes around the catalogue: it needs an allowlist entry.
    Bypass,
    /// Neither.
    Ignored,
}

fn verdict(file: &str, literal: &Literal) -> Verdict {
    let call = literal.call.as_deref();
    let is = |names: &[&str]| call.is_some_and(|call| names.contains(&call));
    let helper = TRANSLATING_HELPERS.iter().any(|(path, name, argument)| {
        *path == file && call == Some(*name) && literal.argument == *argument
    });
    // Only the first argument of a message call is the message; the
    // values for its placeholders are in a slice of tuples of their own.
    if literal.message
        || (is(MESSAGE_CALLS) && literal.argument <= 1 && message_argument(call, literal))
        || is(MESSAGE_TYPES)
        || helper
        || LIST_NAMES.contains(&(file, literal.text.as_str()))
    {
        return Verdict::Message;
    }
    if is(SILENT_CALLS) {
        return Verdict::Ignored;
    }
    if !reads_as_text(&literal.text, is(TEXT_SINKS)) {
        return Verdict::Ignored;
    }
    let in_message_call = MESSAGE_CALLS_IN
        .iter()
        .any(|(path, calls)| *path == file && is(calls));
    if MESSAGE_FILES.contains(&file) || in_message_call {
        Verdict::Message
    } else {
        Verdict::Bypass
    }
}

/// `tr("…")` and `trf("…", …)` take the message first; the core's
/// `translate(language, "…")` and `format(language, "…", …)` second.
fn message_argument(call: Option<&str>, literal: &Literal) -> bool {
    match call {
        Some("translate" | "format") => literal.argument == 1,
        _ => literal.argument == 0,
    }
}

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

/// The sources the lint reads: `(file, text)`, the file relative to
/// `crates/` with `/`.
fn sources() -> Vec<(String, String)> {
    let crates = crates_dir();
    let mut found = Vec::new();
    for root in ROOTS {
        let mut files = Vec::new();
        rust_files(&crates.join(root), &mut files);
        files.sort();
        for path in files {
            let name = path
                .strip_prefix(&crates)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // Test modules in files of their own (`tests.rs`, `held_tests.rs`).
            let stem = path.file_stem().unwrap().to_string_lossy();
            if stem == "tests" || stem.ends_with("_tests") {
                continue;
            }
            if NOT_READ.iter().any(|skip| name.starts_with(skip)) {
                continue;
            }
            found.push((name, std::fs::read_to_string(&path).unwrap()));
        }
    }
    found
}

/// Every literal the lint has a say on, by file.
fn read_sources() -> BTreeMap<String, Vec<(Literal, Verdict)>> {
    let mut found = BTreeMap::new();
    for (name, source) in sources() {
        let judged: Vec<_> = literals(&name, &source)
            .into_iter()
            .map(|literal| {
                let verdict = verdict(&name, &literal);
                (literal, verdict)
            })
            .filter(|(_, verdict)| *verdict != Verdict::Ignored)
            .collect();
        if !judged.is_empty() {
            found.insert(name, judged);
        }
    }
    found
}

/// The allowlist: `(file, literal)` under each of its two headings. A
/// line is `file<TAB>literal`, the literal as written between its quotes
/// with `\n` and `\t` for a line break and a tab.
struct Allowlist {
    untranslated: BTreeSet<(String, String)>,
    not_shown: BTreeSet<(String, String)>,
}

fn allowlist() -> Allowlist {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/app/catalogue_allowlist.txt");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut list = Allowlist {
        untranslated: BTreeSet::new(),
        not_shown: BTreeSet::new(),
    };
    let mut section = None;
    for (number, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line {
            "[untranslated]" => section = Some(true),
            "[not-shown]" => section = Some(false),
            _ => {
                let (file, literal) = line
                    .split_once('\t')
                    .unwrap_or_else(|| panic!("line {}: no tab in {line:?}", number + 1));
                let entry = (file.to_owned(), unescaped(literal));
                let added = match section {
                    Some(true) => list.untranslated.insert(entry),
                    Some(false) => list.not_shown.insert(entry),
                    None => panic!("line {}: an entry before any heading", number + 1),
                };
                assert!(added, "line {}: {line:?} is listed twice", number + 1);
            }
        }
    }
    list
}

fn escaped(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

fn unescaped(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push(c),
        }
    }
    out
}

/// A message without a row stays English in every language.
#[test]
fn every_message_has_a_catalogue_row() {
    let mut missing = Vec::new();
    let mut messages = 0;
    for (file, literals) in read_sources() {
        for (literal, verdict) in literals {
            if verdict == Verdict::Message {
                messages += 1;
                if !is_catalogued(&literal.text) {
                    missing.push(format!("{file}: {:?}", literal.text));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{} messages have no row in crates/iptv-core/src/i18n/messages.tsv:\n{}",
        missing.len(),
        missing.join("\n")
    );
    assert!(messages >= 600, "only {messages} messages were found");
}

/// `tr(label)` cannot be checked where it is written: what it is given
/// is checked where that is written, and only for the carriers listed.
#[test]
fn a_message_that_is_not_a_literal_comes_from_a_known_carrier() {
    let mut unknown = Vec::new();
    let mut seen = BTreeSet::new();
    for (file, source) in sources() {
        for name in uses(&file, &code(&source)).carried {
            if CARRIED_MESSAGES.contains(&(file.as_str(), name.as_str())) {
                seen.insert((file.clone(), name));
            } else {
                unknown.push(format!("{file}: {name}…"));
            }
        }
    }
    assert!(
        unknown.is_empty(),
        "these are translated as messages and the lint cannot see what they hold. Give the \
         call the literal, or make sure what it is given is a checked message and list it in \
         CARRIED_MESSAGES:\n{}",
        unknown.join("\n")
    );
    for (file, name) in CARRIED_MESSAGES {
        assert!(
            seen.contains(&((*file).to_owned(), (*name).to_owned())),
            "{file} no longer gives {name}… as a message; remove it from CARRIED_MESSAGES"
        );
    }
}

/// What a message call is given is a literal, a carrier, or a choice
/// between these that the lint can follow to each of its ends. The rest
/// is refused: `tr(a + b)`, `tr(&format!(…))`, `.map(tr)`.
#[test]
fn a_message_is_given_in_a_way_the_lint_can_follow() {
    let mut unproven = Vec::new();
    let mut calls = 0;
    for (file, source) in sources() {
        let uses = uses(&file, &code(&source));
        calls += uses.messages.len() + uses.carried.len();
        unproven.extend(uses.unproven.iter().map(|line| format!("{file}: {line}")));
    }
    assert!(
        unproven.is_empty(),
        "the lint cannot tell which messages these translate. Give the call a literal, a \
         listed carrier, or an `if`/`else` or `match` with one of these in every arm:\n{}",
        unproven.join("\n")
    );
    assert!(calls >= 600, "only {calls} messages were followed");
}

/// `trf("{count} items", &[("cnt", …)])` would show `{count}` as written:
/// a literal message names exactly the values written beside it.
#[test]
fn a_literal_message_is_given_the_values_it_places() {
    let mut misfilled = Vec::new();
    for (file, source) in sources() {
        let uses = uses(&file, &code(&source));
        misfilled.extend(uses.misfilled.iter().map(|line| format!("{file}: {line}")));
    }
    assert!(
        misfilled.is_empty(),
        "these messages and the values they are given do not agree:\n{}",
        misfilled.join("\n")
    );
}

/// Text that goes around the catalogue is listed, and the list of what is
/// not translated yet only shrinks.
#[test]
fn text_outside_the_catalogue_is_listed_and_the_list_only_shrinks() {
    let list = allowlist();
    let mut bypasses = BTreeSet::new();
    for (file, literals) in read_sources() {
        for (literal, verdict) in literals {
            if verdict == Verdict::Bypass {
                bypasses.insert((file.clone(), literal.text));
            }
        }
    }
    let line = |(file, text): &(String, String)| format!("{file}\t{}", escaped(text));
    let unlisted: Vec<_> = bypasses
        .iter()
        .filter(|entry| !list.untranslated.contains(*entry) && !list.not_shown.contains(*entry))
        .map(line)
        .collect();
    assert!(
        unlisted.is_empty(),
        "{} literals read as text and are not messages. Pass each through `tr` or `trf` with a \
         row in messages.tsv; if it never reaches the screen, add its line under [not-shown] in \
         tests/app/catalogue_allowlist.txt:\n{}",
        unlisted.len(),
        unlisted.join("\n")
    );
    let stale: Vec<_> = list
        .untranslated
        .iter()
        .chain(&list.not_shown)
        .filter(|entry| !bypasses.contains(*entry))
        .map(line)
        .collect();
    assert!(
        stale.is_empty(),
        "{} entries of tests/app/catalogue_allowlist.txt match no literal any more; remove \
         them (and lower UNTRANSLATED):\n{}",
        stale.len(),
        stale.join("\n")
    );
    let both: Vec<_> = list
        .untranslated
        .intersection(&list.not_shown)
        .map(line)
        .collect();
    assert!(both.is_empty(), "under both headings:\n{}", both.join("\n"));
    assert_eq!(
        list.untranslated.len(),
        UNTRANSLATED,
        "[untranslated] may only shrink: translate rather than add; after removing entries, \
         lower UNTRANSLATED in catalogue_lint.rs to the new size"
    );
}

/// The reader itself: what it takes for a literal, a call and test code.
#[test]
fn the_reader_finds_literals_with_their_calls_outside_test_code() {
    let source = r##"
        // tr("in a comment")
        /* tr("in a block /* nested */ comment") */
        const NOTE: &str = key("A note \
                                on two lines.");
        fn f<'a>(x: &'a str) -> char {
            log::warn!("iptv x: {x}");
            let quote = '"';
            notify(tr(match x { "a" => "First", _ => "Second" }));
            let raw = r#"Raw "quoted" text"#;
            let bytes = b"Bytes are not text";
            trf("{count} of {all}", &[("count", 1.into()), ("all", "Seven".into())]);
            button(ui, size, "Done", act);
            Err("Refused".into())
        }
        #[cfg(test)]
        mod tests {
            fn g() { tr("In a test"); }
        }
        #[cfg(not(test))]
        fn h() { text("Shown") }
        #[ignore = "An attribute"]
        fn i() {}
    "##;
    let found: Vec<_> = literals("f.rs", source)
        .into_iter()
        .map(|l| (l.text, l.call, l.argument))
        .collect();
    let call = |name: &str| Some(name.to_owned());
    assert_eq!(
        found,
        [
            ("A note on two lines.".to_owned(), call("key"), 0),
            ("iptv x: {x}".to_owned(), call("warn!"), 0),
            ("First".to_owned(), call("tr"), 0),
            ("Second".to_owned(), call("tr"), 0),
            ("Raw \"quoted\" text".to_owned(), None, 0),
            ("{count} of {all}".to_owned(), call("trf"), 0),
            ("count".to_owned(), None, 0),
            ("all".to_owned(), None, 0),
            ("Seven".to_owned(), None, 1),
            ("Done".to_owned(), call("button"), 2),
            ("Refused".to_owned(), call("Err"), 0),
            ("Shown".to_owned(), call("text"), 0),
        ]
    );
}

/// A character literal ends at its own quote, whatever it escapes: one
/// that ran on would swallow the code after it, or open a string in it.
#[test]
fn the_reader_ends_a_character_at_its_quote_and_a_line_at_its_break() {
    let word = |name: &str| Token::Ident(name.to_owned());
    let text = |value: &str| Token::Str(value.to_owned());
    let source =
        r##"['\\', '\'', '"', b'\\', b'\'', '\u{10FFFF}', '\x41', 'a'] "After" <'a> "them""##;
    let mut expected = vec![Token::Punct('[')];
    for _ in 0..7 {
        expected.extend([Token::Other, Token::Punct(',')]);
    }
    expected.extend([Token::Other, Token::Punct(']'), text("After")]);
    expected.extend([
        Token::Punct('<'),
        word("a"),
        Token::Punct('>'),
        text("them"),
    ]);
    assert_eq!(tokens(source), expected);
    let found: Vec<_> = literals(
        "f.rs",
        r##"fn f<'a>(x: &'a str) { let slash = '\\'; let quote = '\''; text("After a backslash"); }"##,
    )
    .into_iter()
    .map(|l| (l.text, l.call))
    .collect();
    assert_eq!(
        found,
        [("After a backslash".to_owned(), Some("text".to_owned()))]
    );

    // A checkout with `\r\n`: the continued literal is the one the
    // catalogue has, and a break inside a literal is a `\n`.
    let source = "const NOTE: &str = key(\"A note \\\r\n    on two lines.\");\r\n\
                  fn f() { text(\"Shown\r\nbelow\"); let raw = r#\"Raw\r\ntext\"#; }\r\n";
    let found: Vec<_> = literals("f.rs", source)
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert_eq!(found, ["A note on two lines.", "Shown\nbelow", "Raw\ntext"]);

    // A number is one token, and no part of it a name.
    assert_eq!(
        tokens("f(1_000u32, 0x1F, 1.5, 2.into(), x.0)"),
        [
            word("f"),
            Token::Punct('('),
            Token::Other,
            Token::Punct(','),
            Token::Other,
            Token::Punct(','),
            Token::Other,
            Token::Punct(','),
            Token::Other,
            Token::Punct('.'),
            word("into"),
            Token::Punct('('),
            Token::Punct(')'),
            Token::Punct(','),
            word("x"),
            Token::Punct('.'),
            Token::Other,
            Token::Punct(')'),
        ]
    );
}

/// `concat!` of literals is the literal the compiler makes of it, read
/// where it stands; it is no way around the lint.
#[test]
fn a_concatenation_of_literals_is_read_as_the_literal_it_makes() {
    let view = "iptv-app/src/some_view.rs";
    let source = r#"
        fn f() {
            text(concat!("Two ", "pieces"));
            tr(concat!("One ", concat!("mes", "sage"),));
            log::warn!(concat!("iptv x: ", "Quiet words"));
            let version = concat!("Version ", env!("NUMBER"));
            tr(concat!("Built from ", NAME));
        }
    "#;
    let found: Vec<_> = literals(view, source)
        .into_iter()
        .map(|literal| {
            let verdict = verdict(view, &literal);
            (literal.text, literal.call, verdict)
        })
        .collect();
    let call = |name: &str| Some(name.to_owned());
    assert_eq!(
        found,
        [
            ("Two pieces".to_owned(), call("text"), Verdict::Bypass),
            ("One message".to_owned(), call("tr"), Verdict::Message),
            (
                "iptv x: Quiet words".to_owned(),
                call("warn!"),
                Verdict::Ignored
            ),
            // Not all literals: each piece is read by itself…
            ("Version ".to_owned(), call("concat!"), Verdict::Bypass),
            ("NUMBER".to_owned(), call("env!"), Verdict::Ignored),
            ("Built from ".to_owned(), call("concat!"), Verdict::Bypass),
        ]
    );
    // … and it is nothing `tr` can be given.
    assert_eq!(
        uses(view, &code(source)).unproven,
        [r#"tr(concat!("Built from ",NAME))"#]
    );
}

/// What a file's message calls are given, for the tests below: the
/// literals, the carriers, and what was refused.
fn followed(file: &str, source: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let tokens = code(source);
    let uses = uses(file, &tokens);
    let literals = uses
        .messages
        .iter()
        .map(|at| match &tokens[*at] {
            Token::Str(text) => text.clone(),
            other => panic!("{other:?} is no literal"),
        })
        .collect();
    (literals, uses.carried, uses.unproven)
}

/// A message is followed through whatever wraps it: a parenthesis, a
/// block, an `if`, a `match`. None of them hides a name from the list of
/// carriers, or a literal from the catalogue.
#[test]
fn a_message_is_followed_through_parentheses_blocks_and_control_flow() {
    let view = "iptv-app/src/some_view.rs";
    let all = |names: &[&str]| {
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    };
    let none = Vec::<String>::new();
    for (source, literals, carried) in [
        ("tr(label)", &[][..], &["label"][..]),
        ("trf(&error.text(), &[])", &[], &["error"]),
        (
            "crate::i18n::tr(report(\"saving\", error)?)",
            &[],
            &["report"],
        ),
        (
            "tr(crate::words::title(kind))",
            &[],
            &["crate::words::title"],
        ),
        ("tr(self.label)", &[], &["self"]),
        ("tr(names[0].title)", &[], &["names"]),
        ("tr(\"Literal\")", &["Literal"], &[]),
        // The reviewer's: each was dropped without a word.
        ("tr((label))", &[], &["label"]),
        ("tr({ label })", &[], &["label"]),
        ("tr(&*(({ label })))", &[], &["label"]),
        ("tr((\"Wrapped\"))", &["Wrapped"], &[]),
        ("tr({ prepare(); \"Ready\" })", &["Ready"], &[]),
        (
            "tr(if on { first } else { second })",
            &[],
            &["first", "second"],
        ),
        (
            "tr(if a { one } else if b { \"Two\" } else { three.text() })",
            &["Two"],
            &["one", "three"],
        ),
        (
            "tr(match x { A => first, B(_) => { second } C if n >= 1 => \"Third\", \
             D | E => (fourth), _ => if y { \"Fifth\" } else { sixth }, })",
            &["Third", "Fifth"],
            &["first", "second", "fourth", "sixth"],
        ),
        (
            "notify(tr(match result { Ok(()) => \"Saved\", Err(e) => report(\"saving\", e) }))",
            &["Saved"],
            &["report"],
        ),
    ] {
        let source = format!("fn f() {{ {source}; }}");
        assert_eq!(
            followed(view, &source),
            (all(literals), all(carried), none.clone()),
            "{source}"
        );
    }
    // A literal inside a wrapping is a message for the catalogue too.
    let wrapped = literals(
        view,
        "fn f() { tr(if x { (\"ended\") } else { \"begun\" }); }",
    );
    assert_eq!(wrapped.len(), 2);
    for literal in wrapped {
        assert_eq!(verdict(view, &literal), Verdict::Message, "{literal:?}");
    }
    // Test code and the declaration itself give no message.
    assert_eq!(
        followed(
            view,
            "pub fn tr(source: &str) -> &str { source } \
             #[cfg(test)] mod tests { fn g() { tr(hidden); } }"
        ),
        (none.clone(), none.clone(), none.clone())
    );
}

/// What the lint cannot follow to a literal or a name is refused where
/// it is written, not passed.
#[test]
fn a_message_the_lint_cannot_follow_is_refused() {
    let view = "iptv-app/src/some_view.rs";
    for source in [
        "tr(first + second)",
        "tr(&format!(\"{a} of {b}\"))",
        "tr(&[first, second].concat())",
        "tr(if on { \"On\" })",
        "tr(if let Some(text) = found { text })",
        "tr(match x { A => \"A\", _ => return })",
        "tr(match x { A => { \"A\" }.trim(), _ => \"B\" })",
        "tr(match x {})",
        "tr({ \"Dropped\"; })",
        "tr(|| \"Later\")",
        "tr(Words { text }.text)",
        "tr(text as &str)",
        "tr(loop { break text })",
        "tr(unsafe { text })",
        "tr(stringify!(text))",
        "tr((first, second).0)",
        "tr(pick::<Words>(text))",
        "trf(\"{a}\".repeat(n).as_str(), &[])",
        // The function without a call: what it is given is out of sight.
        "names.iter().copied().map(tr).collect::<Vec<_>>()",
        "let say = trf",
        "let say = crate::i18n::tr",
        "let say = iptv_core::i18n::format",
        "let say: fn(&Language, &str) -> &str = i18n::translate",
    ] {
        let (_, _, unproven) = followed(view, &format!("fn f() {{ {source}; }}"));
        assert_eq!(unproven.len(), 1, "{source}: {unproven:?}");
    }
    // An import under another name would hide every call.
    for (source, refused) in [
        ("use crate::i18n::{tr, trf, key};", 0),
        ("pub use iptv_core::i18n::{format, translate};", 0),
        ("use std::fmt::format as write;", 0),
        (
            "fn f<'a>(x: &'a str) -> impl Sized + use<'a> { tr(\"Kept\") }",
            0,
        ),
        ("use crate::i18n::tr as say;", 1),
        ("use crate::i18n::{key as mark, trf as fill};", 2),
        (
            "use iptv_core::i18n::{format as fill, translate as say};",
            2,
        ),
        ("use crate::runtime::RuntimeEvent::Notify as Tell;", 1),
    ] {
        let (_, _, unproven) = followed(view, source);
        assert_eq!(unproven.len(), refused, "{source}: {unproven:?}");
    }
}

/// The core's own `translate` and `format` are message calls like `tr`:
/// the message is their second argument.
#[test]
fn the_core_calls_are_followed_like_tr() {
    let core = "iptv-core/src/some_words.rs";
    let all = |names: &[&str]| {
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    };
    for (source, literals, carried, refused) in [
        ("i18n::translate(language, text)", &[][..], &["text"][..], 0),
        (
            "crate::i18n::translate(language(), \"Mute\")",
            &["Mute"],
            &[],
            0,
        ),
        ("translate(language, (text))", &[], &["text"], 0),
        (
            "iptv_core::i18n::format(language, if soon { \"{n} min left\" } else { later }, \
             &[(\"n\", n.into())])",
            &["{n} min left"],
            &["later"],
            0,
        ),
        (
            "i18n::format(language, &words.join(\" \"), &[])",
            &[],
            &["words"],
            0,
        ),
        (
            "i18n::translate(language, &format!(\"{a} {b}\"))",
            &[],
            &[],
            1,
        ),
        ("format(language, first + second, args)", &[], &[], 1),
        // Other things of these names: a closure, a method, a key press.
        ("let value = badges(&format())", &[], &[], 0),
        ("time.format(a, b, c)", &[], &[], 0),
        ("matrix.translate(x, y)", &[], &[], 0),
        ("key(ui, Action::Back)", &[], &[], 0),
        ("list.iter().find(|a| key(a.name) == variant)", &[], &[], 0),
        ("row.key()", &[], &[], 0),
        // `key` marks a literal, however it is wrapped.
        ("const NOTE: &str = key((\"A note\"))", &["A note"], &[], 0),
    ] {
        let source = format!("fn f() {{ {source}; }}");
        let (found, names, unproven) = followed(core, &source);
        assert_eq!((found, names), (all(literals), all(carried)), "{source}");
        assert_eq!(unproven.len(), refused, "{source}: {unproven:?}");
    }
}

/// A helper that translates its label is a message call for that
/// argument: a name given there is a carrier, and what cannot be followed
/// is refused, as for `tr`. So is what a message type is made of.
#[test]
fn a_translating_helper_and_a_message_type_are_followed_like_tr() {
    let wizard = "iptv-app/src/wizard_view.rs";
    let entry = "iptv-app/src/wizard_view/entry.rs";
    let chrome = "iptv-app/src/multiview_view/chrome.rs";
    let view = "iptv-app/src/some_view.rs";
    let all = |names: &[&str]| {
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    };
    for (file, source, literals, carried, refused) in [
        (
            wizard,
            "button(ui, size, \"Watch\", act)",
            &["Watch"][..],
            &[][..],
            0,
        ),
        (wizard, "button(ui, size, label, act)", &[], &["label"], 0),
        (wizard, "button(ui, size, (label), act)", &[], &["label"], 0),
        (
            wizard,
            "button(ui, size, if x { \"A\" } else { other }, act)",
            &["A"],
            &["other"],
            0,
        ),
        (
            wizard,
            "self::button(ui, size, name(), |ui, key| act(ui, key))",
            &[],
            &["name"],
            0,
        ),
        (wizard, "hint_or_back(ui, size, keys)", &[], &["keys"], 0),
        (
            wizard,
            "button(ui, size, &format!(\"{a} {b}\"), act)",
            &[],
            &[],
            1,
        ),
        (wizard, "button(ui, size, first + second, act)", &[], &[], 1),
        (wizard, "button(ui, size, first | second, act)", &[], &[], 1),
        // Its own declaration, and the kit's button it is made of.
        (
            wizard,
            "} fn button(ui: Ui, size: Scale, label: &'static str, act: fn(Ui)) -> View { \
             ui_kit::button(ui, size, move || tr(label), || false, move || act(ui))",
            &[],
            &["label"],
            0,
        ),
        // From the modules around it.
        (entry, "super::hint(ui, size, words)", &[], &["words"], 0),
        (
            view,
            "wizard_view::button(ui, size, label, act)",
            &[],
            &["label"],
            0,
        ),
        (
            view,
            "crate::wizard_view::button(ui, size, \"go\", act)",
            &["go"],
            &[],
            0,
        ),
        (
            view,
            "wizard_view::button(ui, size, text.trim(), act)",
            &[],
            &["text"],
            0,
        ),
        // Not a helper of this file.
        (view, "button(ui, size, label, act)", &[], &[], 0),
        // A closure is not the helper's `&'static str`.
        (chrome, "view().label(move || tr(name))", &[], &["name"], 0),
        (chrome, "view().label(|| describe(tile))", &[], &[], 0),
        (
            chrome,
            "pill(\"+  Add\", \"Add screen\", add)",
            &["Add screen"],
            &[],
            0,
        ),
        (chrome, "pill(\"+  Add\", name, add)", &[], &["name"], 0),
        // A message type.
        (
            view,
            "emit(RuntimeEvent::Notify(\"Saved\"))",
            &["Saved"],
            &[],
            0,
        ),
        (view, "emit(RuntimeEvent::Notify(text))", &[], &["text"], 0),
        (
            view,
            "emit(RuntimeEvent::Notify(if ok { \"Saved\" } else { why }))",
            &["Saved"],
            &["why"],
            0,
        ),
        (
            view,
            "emit(RuntimeEvent::Notify(&format!(\"{a} {b}\")))",
            &[],
            &[],
            1,
        ),
        (
            view,
            "matches!(event, RuntimeEvent::Notify(_) | RuntimeEvent::Notify(..))",
            &[],
            &[],
            0,
        ),
        (
            view,
            "} pub enum RuntimeEvent { Notify(&'static str), Other(u8) } fn g() {",
            &[],
            &[],
            0,
        ),
    ] {
        let source = format!("fn f() {{ {source}; }}");
        let (found, names, unproven) = followed(file, &source);
        assert_eq!(
            (found, names),
            (all(literals), all(carried)),
            "{file}: {source}"
        );
        assert_eq!(unproven.len(), refused, "{file}: {source}: {unproven:?}");
    }
    // A literal given to a helper from another file is a message there.
    let source = "fn f() { wizard_view::button(ui, size, \"go\", act); }";
    let literal = literals(view, source).remove(0);
    assert_eq!(verdict(view, &literal), Verdict::Message);
}

/// A placeholder with no value is shown as written, `{count}`: a literal
/// message and the values written beside it name the same things.
#[test]
fn a_literal_message_and_its_values_name_the_same_placeholders() {
    let view = "iptv-app/src/some_view.rs";
    for (source, wrong) in [
        (
            "trf(\"{count} of {all}\", &[(\"count\", 1.into()), (\"all\", (&all).into())])",
            0,
        ),
        ("trf(\"Nothing to place\", &[])", 0),
        (
            "trf(\"{count, plural, one {# of {all}} other {# of {all}}}\", \
             &[(\"all\", all.into()), (\"count\", n.into()),])",
            0,
        ),
        // The reviewer's: a name misspelt at the call.
        ("trf(\"{count} items\", &[(\"cnt\", 1.into())])", 2),
        ("trf(\"{count} of {all}\", &[(\"count\", 1.into())])", 1),
        ("trf(\"Nothing to place\", &[(\"count\", 1.into())])", 1),
        (
            "i18n::format(language, \"{minutes} min left\", &[(\"minute\", m.into())])",
            2,
        ),
        // Each outcome is checked; together they place every value.
        (
            "trf(if one { \"{a} alone\" } else { \"{a} and {b}\" }, \
             &[(\"a\", a.into()), (\"b\", b.into())])",
            0,
        ),
        (
            "trf(if one { \"{a} alone\" } else { \"{a} and {c}\" }, \
             &[(\"a\", a.into()), (\"b\", b.into())])",
            2,
        ),
        // A carrier may place the rest; values made elsewhere are not seen.
        (
            "trf(if one { \"{a} alone\" } else { other }, &[(\"a\", a.into()), (\"b\", b.into())])",
            0,
        ),
        (
            "trf(if one { \"{c} alone\" } else { other }, &[(\"a\", a.into())])",
            1,
        ),
        ("trf(\"{count} items\", &values)", 0),
        // Values made once for several calls, in the same function.
        (
            "let values = [(\"count\", n.into()), (\"all\", all.into())]; \
             if moving { trf(\"{count} moved\", &values) } else { trf(\"{count} of {all}\", &values) }",
            0,
        ),
        (
            "let values = [(\"cnt\", n.into())]; trf(\"{count} items\", &values)",
            1,
        ),
        (
            "let values = [(\"cnt\", n.into())]; let values = other(); trf(\"{count} items\", &values)",
            0,
        ),
        (
            "let values = [(\"cnt\", n.into())]; } fn g(values: &Args) { trf(\"{count} items\", values)",
            0,
        ),
        (
            "if done { let values = first; } trf(\"{count} items\", &values)",
            0,
        ),
        ("trf(\"{count} items\", &[first, second])", 0),
        // A message that is not well formed is shown as written.
        (
            "trf(\"{count, plural, one {# item}}\", &[(\"count\", n.into())])",
            1,
        ),
        (
            "trf(\"{count} items\", &[(\"count\", n.into())]); trf(\"{open\", &values)",
            1,
        ),
        // `tr` places nothing.
        ("tr(\"Delete {name}?\")", 1),
        ("i18n::translate(language, \"Delete {name}?\")", 1),
        ("tr(\"Delete it?\")", 0),
    ] {
        let source = format!("fn f() {{ {source}; }}");
        let misfilled = uses(view, &code(&source)).misfilled;
        assert_eq!(misfilled.len(), wrong, "{source}: {misfilled:?}");
    }
}

#[test]
fn the_verdicts_tell_messages_from_text_and_from_the_rest() {
    let literal = |text: &str, call: Option<&str>, argument| Literal {
        text: text.to_owned(),
        call: call.map(str::to_owned),
        argument,
        message: false,
    };
    let view = "iptv-app/src/some_view.rs";
    // A message call's literal is a message, whatever it looks like.
    assert_eq!(
        verdict(view, &literal("ended", Some("tr"), 0)),
        Verdict::Message
    );
    assert_eq!(
        verdict(view, &literal("{n} of {all}", Some("trf"), 0)),
        Verdict::Message
    );
    assert_eq!(
        verdict(view, &literal("Saved", Some("Notify"), 0)),
        Verdict::Message
    );
    assert_eq!(
        verdict(
            "iptv-core/src/player_info.rs",
            &literal("{minutes} min left", Some("format"), 1)
        ),
        Verdict::Message
    );
    // Text anywhere else goes around the catalogue.
    assert_eq!(
        verdict(view, &literal("Keep it", Some("action"), 2)),
        Verdict::Bypass
    );
    assert_eq!(
        verdict(view, &literal("Delete “{}”?", Some("format!"), 0)),
        Verdict::Bypass
    );
    assert_eq!(verdict(view, &literal("Hidden", None, 0)), Verdict::Bypass);
    assert_eq!(
        verdict(view, &literal("ended", Some("text"), 0)),
        Verdict::Bypass
    );
    // Identifiers, formats of placeholders only, and the log are not text.
    assert_eq!(verdict(view, &literal("ended", None, 0)), Verdict::Ignored);
    assert_eq!(
        verdict(view, &literal("restore-pending", Some("join"), 0)),
        Verdict::Ignored
    );
    assert_eq!(
        verdict(view, &literal("{name} ({keyboard})", Some("format!"), 0)),
        Verdict::Ignored
    );
    assert_eq!(
        verdict(view, &literal("{} · {}", Some("format!"), 0)),
        Verdict::Ignored
    );
    assert_eq!(
        verdict(view, &literal("iptv data: Lost it", Some("warn!"), 0)),
        Verdict::Ignored
    );
    // What a message call was given, wherever the reader found it.
    let wrapped = Literal {
        message: true,
        ..literal("ended", None, 0)
    };
    assert_eq!(verdict(view, &wrapped), Verdict::Message);
    // `concat!` does not make text silent.
    assert_eq!(
        verdict(view, &literal("Version ", Some("concat!"), 0)),
        Verdict::Bypass
    );
    // A helper that translates its label, and only that argument of it.
    let wizard = "iptv-app/src/wizard_view.rs";
    assert_eq!(
        verdict(wizard, &literal("Watch", Some("button"), 2)),
        Verdict::Message
    );
    assert_eq!(
        verdict(view, &literal("Watch", Some("button"), 2)),
        Verdict::Bypass
    );
    // A backup error's text is a message kept as data.
    let backup = "iptv-runtime/src/backup.rs";
    assert_eq!(
        verdict(backup, &literal("Backup is too large", Some("Err"), 0)),
        Verdict::Message
    );
    assert_eq!(
        verdict(backup, &literal("iptv.sqlite", Some("join"), 0)),
        Verdict::Ignored
    );
    assert_eq!(
        verdict(
            "iptv-app/src/backup.rs",
            &literal("Not readable", Some("context"), 0)
        ),
        Verdict::Message
    );
}

/// The extracted runtime still contributes stored messages and notices; its
/// typed settings failures are translated only at the shell boundary.
#[test]
fn extracted_runtime_messages_and_shell_failures_remain_checked() {
    let sources = read_sources();
    for (file, message) in [
        (
            "iptv-app/src/custom_groups.rs",
            "The list could not be reloaded. Reopen it and try again.",
        ),
        (
            "iptv-runtime/src/data/organization.rs",
            "The list could not be reloaded. Reopen it and try again.",
        ),
        (
            "iptv-app/src/organization_view.rs",
            "The change could not be saved. Reopen the list and try again.",
        ),
        (
            "iptv-runtime/src/backup.rs",
            "A backup operation is already running",
        ),
        (
            "iptv-runtime/src/backup/instance.rs",
            "Could not lock application storage",
        ),
        (
            "iptv-app/src/settings_view/words.rs",
            "Settings could not be saved. Check storage permissions and try again.",
        ),
        (
            "iptv-app/src/settings_view/words.rs",
            "Settings updated, but we could not confirm they were saved safely. Check storage and try again.",
        ),
    ] {
        assert!(
            sources.get(file).is_some_and(|literals| literals
                .iter()
                .any(|(literal, verdict)| literal.text == message && *verdict == Verdict::Message)),
            "{file}: message is no longer checked: {message}"
        );
    }
    // Moving orchestration retains the notices as checked catalogue messages.
    let (messages, carried, unproven) = followed(
        "iptv-runtime/src/data/organization.rs",
        "fn f() { emit(RuntimeEvent::Notify(\"Synthetic uncatalogued guide failure\")); }",
    );
    assert_eq!(messages, ["Synthetic uncatalogued guide failure"]);
    assert!(!is_catalogued(&messages[0]));
    assert!(carried.is_empty() && unproven.is_empty());
    // The moved data path rejects a missing catalogue row as before.
    let file = "iptv-runtime/src/backup.rs";
    let literal = literals(
        file,
        "fn f() { Err(\"Synthetic uncatalogued backup failure\") }",
    )
    .remove(0);
    assert_eq!(verdict(file, &literal), Verdict::Message);
    assert!(!is_catalogued(&literal.text));
    // Runtime notices retain the dynamic-call and wrapper rules, too.
    let (messages, carried, unproven) = followed(
        file,
        "fn f() { emit(RuntimeEvent::Notify(if ok { \"Saved\" } else { reason })); }",
    );
    assert_eq!(messages, ["Saved"]);
    assert_eq!(carried, ["reason"]);
    assert!(unproven.is_empty());
    let (_, _, unproven) = followed(
        file,
        "fn f() { emit(RuntimeEvent::Notify(&format!(\"{a} {b}\"))); }",
    );
    assert_eq!(unproven.len(), 1);
}

/// Prints the allowlist the sources need now, for a first list or after a
/// large change: `cargo test -p iptv-app --features test-support --test
/// app catalogue_lint::print -- --ignored --nocapture`. Each new line
/// still has to be sorted under its heading by a person.
#[test]
#[ignore = "a tool, not a check"]
fn print_the_literals_the_allowlist_would_need() {
    let list = allowlist();
    for (file, literals) in read_sources() {
        let mut seen = BTreeSet::new();
        for (literal, verdict) in literals {
            let entry = (file.clone(), literal.text.clone());
            if verdict == Verdict::Bypass && seen.insert(literal.text.clone()) {
                let heading = if list.not_shown.contains(&entry) {
                    "not-shown"
                } else if list.untranslated.contains(&entry) {
                    "untranslated"
                } else {
                    "new"
                };
                println!(
                    "{heading}\t{file}\t{}\t{}",
                    escaped(&literal.text),
                    literal.call.as_deref().unwrap_or("-")
                );
            }
        }
    }
}

/// Prints the messages with no catalogue row, one per line, for the
/// translators: as above, with `catalogue_lint::print_the_messages`.
#[test]
#[ignore = "a tool, not a check"]
fn print_the_messages_without_a_row() {
    let mut seen = BTreeSet::new();
    for (_, literals) in read_sources() {
        for (literal, verdict) in literals {
            if verdict == Verdict::Message
                && !is_catalogued(&literal.text)
                && seen.insert(literal.text.clone())
            {
                println!("{}", escaped(&literal.text));
            }
        }
    }
}
