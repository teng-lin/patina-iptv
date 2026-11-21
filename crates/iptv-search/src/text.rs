//! Text folding, query tokenization and the one matcher every search uses.
//!
//! Folding is iptv-epg's [`normalize_name`] applied word by word: the same
//! compatibility decomposition, diacritic removal, lowercasing and special
//! letters (`ß` → `ss`, ...), with `&` → `and` and `+` → `plus`. Search keeps
//! the word boundaries that EPG matching drops, and keeps quality words such
//! as `HD`, so a user can type them. Concatenating [`tokens`] gives
//! `normalize_name` for any name without a trailing quality tag.

use iptv_epg::normalize_name;
use unicode_normalization::char::is_combining_mark;

/// Query tokens shorter than this (in characters) only match at the start
/// of a word; longer ones also match inside words.
pub const MIN_INFIX: usize = 3;

/// How well a query matches a text; higher is better.
///
/// - `Exact`: the folded text equals the folded query (spaces ignored).
/// - `Prefix`: the folded text starts with the folded query.
/// - `Word`: every query token is a whole word of the text.
/// - `WordPrefix`: as `Word`, but the last token is only the start of a word.
/// - `Substring`: some token matches only inside a word (or, before the last
///   token, only as the start of a word).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchQuality {
    Substring,
    WordPrefix,
    Word,
    Prefix,
    Exact,
}

/// Folded words of `text`, in order.
///
/// ```
/// use iptv_search::tokens;
/// assert_eq!(tokens("Ch\u{e2}ine Écho-Deux HD"), ["chaine", "echo", "deux", "hd"]);
/// assert_eq!(tokens("Alpha & Beta +1"), ["alpha", "and", "beta", "plus", "1"]);
/// ```
pub fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<String>| {
        if !word.is_empty() {
            let folded = normalize_name(word);
            if !folded.is_empty() {
                out.push(folded);
            }
            word.clear();
        }
    };
    for c in text.chars() {
        match c {
            '&' | '+' => {
                flush(&mut word, &mut out);
                out.push(if c == '&' { "and" } else { "plus" }.to_owned());
            }
            c if c.is_alphanumeric() || is_combining_mark(c) => word.push(c),
            _ => flush(&mut word, &mut out),
        }
    }
    flush(&mut word, &mut out);
    out
}

/// The folded form used to compare queries (history de-duplication).
pub fn fold(text: &str) -> String {
    tokens(text).join(" ")
}

/// A parsed search query: all tokens must match, the last one may be the
/// start of a word (as the user types).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    tokens: Vec<String>,
    infix: Vec<bool>,
    joined: String,
}

impl Query {
    pub fn parse(text: &str) -> Query {
        let tokens = tokens(text);
        let infix = tokens
            .iter()
            .map(|t| t.chars().count() >= MIN_INFIX)
            .collect();
        let joined = tokens.concat();
        Query {
            tokens,
            infix,
            joined,
        }
    }

    /// No letters or digits: matches nothing.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    pub fn tokens(&self) -> &[String] {
        &self.tokens
    }

    /// The tokens without spaces (the `Exact` / `Prefix` key).
    pub fn joined(&self) -> &str {
        &self.joined
    }

    /// Whether token `i` may match inside a word.
    pub(crate) fn infix(&self, i: usize) -> bool {
        self.infix[i]
    }

    /// Matches folded words `0..n` (from `word`) against the query.
    pub fn match_words<'a>(
        &self,
        n: usize,
        word: impl Fn(usize) -> &'a str,
    ) -> Option<MatchQuality> {
        if self.is_empty() || n == 0 {
            return None;
        }
        match joined_cmp(&self.joined, n, &word) {
            Joined::Equal => return Some(MatchQuality::Exact),
            Joined::Prefix => return Some(MatchQuality::Prefix),
            Joined::No => {}
        }
        let last = self.tokens.len() - 1;
        let mut worst = MatchQuality::Word;
        for (i, q) in self.tokens.iter().enumerate() {
            let best = (0..n)
                .filter_map(|j| {
                    let w = word(j);
                    if w == q {
                        Some(MatchQuality::Word)
                    } else if w.starts_with(q.as_str()) {
                        Some(if i == last {
                            MatchQuality::WordPrefix
                        } else {
                            MatchQuality::Substring
                        })
                    } else if self.infix[i] && w.contains(q.as_str()) {
                        Some(MatchQuality::Substring)
                    } else {
                        None
                    }
                })
                .max()?;
            worst = worst.min(best);
        }
        Some(worst)
    }

    /// Matches a slice of folded words.
    pub fn match_tokens<S: AsRef<str>>(&self, words: &[S]) -> Option<MatchQuality> {
        self.match_words(words.len(), |i| words[i].as_ref())
    }
}

enum Joined {
    Equal,
    Prefix,
    No,
}

/// Compares `q` with the concatenation of the words, without allocating.
fn joined_cmp<'a>(q: &str, n: usize, word: &impl Fn(usize) -> &'a str) -> Joined {
    let mut rest = q;
    for i in 0..n {
        let w = word(i);
        if rest.len() <= w.len() {
            return match w.strip_prefix(rest) {
                Some("") if i == n - 1 => Joined::Equal,
                Some(_) => Joined::Prefix,
                None => Joined::No,
            };
        }
        match rest.strip_prefix(w) {
            Some(r) => rest = r,
            None => return Joined::No,
        }
    }
    // Words exhausted with query left over.
    Joined::No
}

#[cfg(test)]
mod tests {
    use super::MatchQuality::*;
    use super::*;

    fn q(query: &str, text: &str) -> Option<MatchQuality> {
        Query::parse(query).match_tokens(&tokens(text))
    }

    #[test]
    fn folding_agrees_with_epg_matching() {
        for name in [
            "Ålpha  ÖNE",
            "Straße Zwei",
            "Łódź Info",
            "Ｆｕｌｌｗｉｄｔｈ Kanal",
            "Alpha & Beta",
            "Alpha +1",
            "Альфа ТВ",
            "Cafe\u{301} Noir",
            "Bob's Kitchen",
        ] {
            assert_eq!(tokens(name).concat(), normalize_name(name), "{name}");
        }
        assert_eq!(tokens("Cafe\u{301} Noir"), ["cafe", "noir"]);
        assert_eq!(tokens("Alpha.One HD"), ["alpha", "one", "hd"]);
        assert!(tokens(" -- ").is_empty());
        assert_eq!(fold("  ÉCHO   deux! "), "echo deux");
    }

    #[test]
    fn quality_levels() {
        assert_eq!(q("alpha one", "Alpha One"), Some(Exact));
        assert_eq!(q("alphaone", "Alpha-One"), Some(Exact));
        assert_eq!(q("alp", "Alpha One"), Some(Prefix));
        assert_eq!(q("alpha o", "Alpha One"), Some(Prefix));
        assert_eq!(q("one", "Alpha One"), Some(Word));
        assert_eq!(q("one alpha", "Alpha One"), Some(Word));
        assert_eq!(q("one alp", "Alpha One"), Some(WordPrefix));
        assert_eq!(q("on", "Alpha One"), Some(WordPrefix));
        assert_eq!(q("pha", "Alpha One"), Some(Substring));
        // Before the last token a word start is only a substring match.
        assert_eq!(q("on alpha", "Alpha One"), Some(Substring));
    }

    #[test]
    fn every_token_must_match() {
        assert_eq!(q("alpha two", "Alpha One"), None);
        assert_eq!(
            q("ph", "Alpha One"),
            None,
            "short tokens only at word starts"
        );
        assert_eq!(q("alpha one two", "Alpha One"), None);
        assert_eq!(q("", "Alpha One"), None);
        assert_eq!(q("alpha", " - "), None);
    }

    #[test]
    fn diacritics_and_case_fold_both_ways() {
        assert_eq!(q("ECHO", "Écho Deux"), Some(Prefix));
        assert_eq!(q("écho", "Echo Deux"), Some(Prefix));
        assert_eq!(q("strasse", "Straße Zwei"), Some(Prefix));
        assert_eq!(q("a&b", "A and B"), Some(Exact));
    }
}
