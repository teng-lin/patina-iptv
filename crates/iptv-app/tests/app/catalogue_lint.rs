//! Text the user reads goes through the message catalogue
//! (`crates/iptv-core/src/i18n/messages.tsv`), or is on a list here.
//!
//! Three failures, all found by reading the sources:
//!
//! 1. **A message with no catalogue row.** A string literal passed to `tr`,
//!    `trf` or `key` (or to one of the helpers and message types listed
//!    below, which translate what they are given) must be a row's source
//!    text. Without a row it would stay English in every language, without
//!    a sign.
//! 2. **A message the lint cannot see.** `tr(label)` is given a name, not
//!    a literal; each such call is listed in [`CARRIED_MESSAGES`] beside
//!    the rule that checks what the name carries.
//! 3. **A user-facing literal that goes around the catalogue.** Any other
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
//! costs one `[not-shown]` line. It does not see text assembled from
//! pieces that are not literals, nor a single lower-case word outside a
//! call that shows it ([`TEXT_SINKS`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use iptv_core::i18n::is_catalogued;

/// How many `[untranslated]` entries there are. Lower it with the list;
/// it is never raised.
const UNTRANSLATED: usize = 443;

/// Source directories read, relative to the workspace's `crates/`.
const ROOTS: &[&str] = &[
    "iptv-app/src",
    "iptv-core/src",
    "iptv-guide/src",
    "iptv-library/src",
    // What a settings row says under the list when it is focused.
    "iptv-settings/src/catalogue",
];

/// The catalogue itself and its formatter: messages about messages.
const NOT_READ: &[&str] = &["iptv-core/src/i18n.rs", "iptv-core/src/i18n/"];

/// Calls whose literal is a message: it must be a catalogue row.
/// `translate` and `format` are the core's own (`i18n::format`; the
/// `format!` macro is another name).
const MESSAGE_CALLS: &[&str] = &["tr", "trf", "key", "translate", "format"];

/// Messages that travel as data and are translated where they are shown:
/// `RuntimeEvent::Notify("…")` is shown by `runtime_events.rs` through
/// `tr`.
const MESSAGE_TYPES: &[&str] = &["Notify"];

/// Functions that pass a `&'static str` argument to `tr` themselves:
/// file, function, and which argument (from 0). A literal there is a
/// message.
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
    "iptv-app/src/runtime/backup.rs",
    "iptv-app/src/runtime/backup/instance.rs",
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

/// Where `tr` or `trf` is given something other than a literal: file, and
/// the name its argument starts with. Each is a message written elsewhere
/// that the rules above already check: a `key(…)` constant, a helper's
/// label, a backup error's text, a notice, a built-in list's name. A new
/// one needs the same before it is listed.
const CARRIED_MESSAGES: &[(&str, &str)] = &[
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
/// the screen.
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
    "concat!",
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
}

/// A Rust source as the tokens this lint needs: identifiers, punctuation
/// and string literals with their escapes resolved. Comments, character
/// literals, lifetimes and numbers are dropped.
fn tokens(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
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
            i = after_char_or_lifetime(&chars, i);
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
            } else {
                out.push(Token::Ident(word));
            }
        } else if c.is_whitespace() || c.is_ascii_digit() {
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

/// The index after a character literal (`'a'`, `'\n'`, `'\u{1F}'`) or
/// after the quote of a lifetime (`'static`).
fn after_char_or_lifetime(chars: &[char], at: usize) -> usize {
    match (chars.get(at + 1), chars.get(at + 2)) {
        (Some('\\'), _) => (at + 2..chars.len())
            .find(|i| chars[*i] == '\'' && chars[*i - 1] != '\\' || *i > at + 12)
            .map_or(chars.len(), |i| i + 1),
        (Some(_), Some('\'')) => at + 3,
        _ => at + 1,
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
    out
}

/// The string literals of `source` outside test code and attributes, each
/// with the call it is in.
fn literals(source: &str) -> Vec<Literal> {
    let tokens = code(source);
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
                });
            }
            _ => {}
        }
    }
    out
}

/// What `tr(…)` and `trf(…)` are given where it is not a literal: the
/// name the argument starts with (`label` of `tr(label)`, `error` of
/// `tr(error.message())`). A `match` or an `if` there chooses between
/// literals, which [`literals`] reads.
fn carried_messages(source: &str) -> Vec<String> {
    let tokens = code(source);
    let mut out = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let call = matches!(token, Token::Ident(name) if name == "tr" || name == "trf");
        let declared = i > 0 && tokens[i - 1] == Token::Ident("fn".to_owned());
        if !call || declared || tokens.get(i + 1) != Some(&Token::Punct('(')) {
            continue;
        }
        let first = tokens[i + 2..]
            .iter()
            .find(|token| !matches!(token, Token::Punct('&' | '*')));
        match first {
            Some(Token::Ident(name)) if name != "match" && name != "if" => out.push(name.clone()),
            _ => {}
        }
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
    if (is(MESSAGE_CALLS) && literal.argument <= 1 && message_argument(call, literal))
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
        let judged: Vec<_> = literals(&source)
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
        for name in carried_messages(&source) {
            if CARRIED_MESSAGES.contains(&(file.as_str(), name.as_str())) {
                seen.insert((file.clone(), name));
            } else {
                unknown.push(format!("{file}: tr({name}…)"));
            }
        }
    }
    assert!(
        unknown.is_empty(),
        "these calls translate something the lint cannot see. Give `tr` the literal, or make \
         sure what it is given is a checked message and list it in CARRIED_MESSAGES:\n{}",
        unknown.join("\n")
    );
    for (file, name) in CARRIED_MESSAGES {
        assert!(
            seen.contains(&((*file).to_owned(), (*name).to_owned())),
            "{file} no longer has tr({name}…); remove it from CARRIED_MESSAGES"
        );
    }
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
    assert_eq!(
        carried_messages(
            "pub fn tr(source: &str) {} fn f() { tr(label); trf(&error.text(), &[]); \
             tr(\"Literal\"); tr(match x { _ => \"A\" }); tr(if y { \"A\" } else { \"B\" }); } \
             #[cfg(test)] mod tests { fn g() { tr(hidden); } }"
        ),
        ["label", "error"]
    );
    let found: Vec<_> = literals(source)
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

#[test]
fn the_verdicts_tell_messages_from_text_and_from_the_rest() {
    let literal = |text: &str, call: Option<&str>, argument| Literal {
        text: text.to_owned(),
        call: call.map(str::to_owned),
        argument,
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
    let backup = "iptv-app/src/runtime/backup.rs";
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
