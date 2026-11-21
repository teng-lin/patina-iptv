//! Superscript markers in program titles. Providers append badges such
//! as "ᴸᶦᵛᵉ" or "ᴺᵉʷ" in Unicode modifier letters, which many fonts lack in
//! part (a missing "ᶦ" makes "Live" read wrong). A run of two or more such
//! letters that spells a word is folded to its plain letters, set off from
//! the title by a space: "Match Dayᴸᶦᵛᵉ" reads "Match Day Live".

use std::borrow::Cow;

use unicode_normalization::char::decompose_compatible;

/// Folds every run of two or more superscript, subscript or modifier
/// letters (and digits, with at least one letter among them) to the plain
/// characters. Text without such runs comes back borrowed; single marks
/// ("H₂O", "m²") stay.
///
/// ```
/// use iptv_epg::fold_superscripts;
/// assert_eq!(fold_superscripts("Morning Show ᴸᶦᵛᵉ"), "Morning Show Live");
/// assert_eq!(fold_superscripts("Quizᴺᵉʷ"), "Quiz New");
/// assert_eq!(fold_superscripts("Plain title"), "Plain title");
/// ```
pub fn fold_superscripts(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_marker) {
        return Cow::Borrowed(text);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        let run = chars[i..]
            .iter()
            .take_while(|&&c| is_marker(c) && base(c).is_some())
            .count();
        let bases: String = chars[i..i + run].iter().filter_map(|&c| base(c)).collect();
        if run >= 2 && bases.chars().any(|c| c.is_ascii_alphabetic()) {
            if out.chars().next_back().is_some_and(char::is_alphabetic) {
                out.push(' ');
            }
            out.push_str(&bases);
            if chars.get(i + run).is_some_and(|c| c.is_alphabetic()) {
                out.push(' ');
            }
            changed = true;
            i += run;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// Spacing modifier letters, phonetic-extension modifier letters, and the
/// superscripts and subscripts block.
fn is_marker(c: char) -> bool {
    matches!(c, '\u{2B0}'..='\u{2FF}' | '\u{1D2C}'..='\u{1D6A}' | '\u{1D9C}'..='\u{1DBF}' | '\u{2070}'..='\u{209F}')
}

/// The plain ASCII letter or digit a marker stands for.
fn base(c: char) -> Option<char> {
    let mut parts = Vec::with_capacity(1);
    decompose_compatible(c, |d| parts.push(d));
    let [d] = parts[..] else {
        return None;
    };
    let plain = match d {
        // Small capitals, as the modifier forms of i, l, n, r, u decompose.
        '\u{26A}' => 'i',
        '\u{29F}' => 'l',
        '\u{274}' => 'n',
        '\u{280}' | '\u{281}' => 'r',
        '\u{1D1C}' => 'u',
        d => d,
    };
    (plain != c && plain.is_ascii_alphanumeric()).then_some(plain)
}

#[cfg(test)]
mod tests {
    use super::fold_superscripts as fold;

    #[test]
    fn badges_fold_to_their_words() {
        assert_eq!(fold("Evening News ᴸᶦᵛᵉ"), "Evening News Live");
        assert_eq!(fold("Evening Newsᴸᶦᵛᵉ"), "Evening News Live");
        assert_eq!(fold("Garden Hour ᴺᵉʷ"), "Garden Hour New");
        assert_eq!(fold("ᴺᵉʷ Garden Hour"), "New Garden Hour");
        assert_eq!(fold("ᴺᵉʷGarden Hour"), "New Garden Hour");
        assert_eq!(fold("Film Night ᴾʳᵉᵐⁱᵉʳᵉ"), "Film Night Premiere");
        assert_eq!(fold("Cup Final ᴿᵉᵖˡᵃʸ"), "Cup Final Replay");
        assert_eq!(fold("Quiz ᴴᴰ"), "Quiz HD");
    }

    #[test]
    fn single_marks_and_plain_text_stay() {
        assert!(matches!(fold("Plain title"), std::borrow::Cow::Borrowed(_)));
        assert_eq!(fold("H₂O explained"), "H₂O explained");
        assert_eq!(fold("Area in m²"), "Area in m²");
        assert_eq!(fold("Mʳ Smith"), "Mʳ Smith");
        // Digits alone are no word.
        assert_eq!(fold("x⁽²⁾"), "x⁽²⁾");
        // Ordinals keep their number.
        assert_eq!(fold("The 5ᵗʰ Round"), "The 5th Round");
    }
}
