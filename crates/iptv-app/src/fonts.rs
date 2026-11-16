//! The bundled fonts (Google's Roboto and Roboto Condensed, SIL OFL 1.1;
//! `fonts/README.md`) and the type of each text role, in sp at full size
//! (desktop windows smaller than full screen scale them, `scale.rs`). They are
//! registered at startup on every platform, and Roboto becomes the app's
//! default family (the tokens' `font-sans`). Every family list ends in
//! `sans-serif`: the scripts the subsets leave out (Arabic, Hebrew, CJK,
//! Thai, Devanagari, ▶, ★, emoji) come from the system's fonts.

use nsh_core::prelude::*;

/// Roboto: channel numbers and names, the ruler, the header.
pub const SANS: &str = "Roboto, sans-serif";
/// Roboto Condensed: programme titles in the grid.
pub const CONDENSED: &str = "'Roboto Condensed', Roboto, sans-serif";
/// The header's title (Roboto Medium).
pub const MEDIUM: FontWeight = FontWeight::MEDIUM;

/// Programme titles in the grid's cells.
pub const CELL_TITLE: f32 = 20.0;
/// Channel numbers and names.
pub const CHANNEL: f32 = 20.0;
/// The ruler's date and slot times.
pub const RULER: f32 = 18.0;
/// The header: the focused programme's title, its times line, and its
/// episode line and description.
pub const HEADER_TITLE: f32 = 22.0;
pub const HEADER_TIMES: f32 = 17.0;
pub const HEADER_BODY: f32 = 16.0;

/// The lists' type (the groups columns, the player's channel list, search,
/// the favourites editor, the menu): the guide's, so a name reads the same
/// in a list as in the guide's channel column (rows: `list_rows.rs`).
/// A list item's name: the guide's channel names.
pub const LIST_ITEM: f32 = CHANNEL;
/// An item's second line (what is on, a result's channel and times): the
/// header's description.
pub const LIST_SECONDARY: f32 = HEADER_BODY;
/// A list's heading ("Groups", the list's name, the menu's headings): the
/// header's times line.
pub const SECTION: f32 = HEADER_TIMES;
/// Hints, status lines and badges.
pub const CAPTION: f32 = 15.0;
/// A screen's title (Search, the favourites editor): the header's title.
pub const TITLE: f32 = HEADER_TITLE;
/// The focused on-demand title, above the poster grid.
pub const LIBRARY_TITLE: f32 = 34.0;

/// Over full-screen video (the player's panels and its status card), not
/// scaled with the window: read from across the room. The programme's
/// title on the info card: the header's title.
pub const OSD_TITLE: f32 = HEADER_TITLE;
/// The top bar, the banner's channel, a status card's message.
pub const OSD_TEXT: f32 = HEADER_BODY;
/// The card's lines (channel, times, next), its buttons and hints.
pub const OSD_DETAIL: f32 = CAPTION;
/// Badges over video (LIVE, FHD, 60 FPS) and a failure's detail line.
pub const OSD_BADGE: f32 = 12.0;

/// The font files, WOFF2 (NSH unpacks them when it registers them).
pub const FILES: [(&str, &[u8]); 3] = [
    (
        "Roboto",
        include_bytes!("../fonts/Roboto-Regular.woff2").as_slice(),
    ),
    (
        "Roboto",
        include_bytes!("../fonts/Roboto-Medium.woff2").as_slice(),
    ),
    (
        "Roboto Condensed",
        include_bytes!("../fonts/RobotoCondensed-Regular.woff2").as_slice(),
    ),
];

/// Registers the fonts and makes Roboto the default family. Text laid out
/// before a font arrives is laid out again with it.
///
/// The WOFF2 files are unpacked on their own thread first: NSH would unpack
/// each one twice on the UI thread (for its family name, then to register
/// it), about 0.7 s of a cold start on a 32-bit TV box.
pub fn register() {
    let started = std::time::Instant::now();
    let (tx, rx) = futures_channel::oneshot::channel();
    std::thread::spawn(move || {
        let unpacked: Vec<_> = FILES
            .iter()
            .map(|&(family, data)| (family, nsh_core::font_data(data).map(|d| d.into_owned())))
            .collect();
        let _ = tx.send(unpacked);
    });
    drop(spawn(async move {
        let Ok(unpacked) = rx.await else {
            return;
        };
        let unpacked_ms = started.elapsed().as_millis();
        for (family, data) in unpacked {
            let registered = match data {
                Ok(data) => register_font(data).await,
                Err(e) => Err(e),
            };
            if let Err(e) = registered {
                log::warn!("iptv font {family} not registered: {e}");
            }
        }
        FONTS_IN.store(true, std::sync::atomic::Ordering::Relaxed);
        let ms = started.elapsed().as_millis();
        crate::startup::mark_with(
            "fonts registered",
            format_args!("unpacked in {unpacked_ms} ms, registered {ms} ms after asking"),
        );
    }));
    let mut tokens = (*tokens()).clone();
    tokens.set_font(nsh_core::element::FONT_SANS, SANS);
    set_tokens(tokens);
}

/// `name` cut to fit `width` with two dots ("Long chann.."), as measured by
/// `measure`; whole when it fits.
///
/// Each measure is a text typeset by the platform (on Android a JNI call
/// that shapes it), and a guide row scrolled in fits its channel's name:
/// the cut is guessed from the whole name's width (widths grow about with
/// the length), so a name usually takes three or four measures where a
/// binary search took six; the result is the binary search's.
pub fn two_dots(name: &str, width: f32, measure: impl Fn(&str) -> f32) -> String {
    if width <= 0.0 {
        return name.to_owned();
    }
    let whole = measure(name);
    if whole <= width {
        return name.to_owned();
    }
    let ends: Vec<usize> = name.char_indices().map(|(i, _)| i).skip(1).collect();
    let cut = |k: usize| format!("{}..", name[..ends[k - 1]].trim_end());
    let fits = |k: usize| measure(&cut(k)) <= width;
    // The longest start that fits with the dots, `k` of `n` characters
    // kept (0: none fits): in `lo..=hi`. Up to two guesses, each scaled
    // from the last width measured, and the one after a guess that fits.
    let n = ends.len();
    let (mut lo, mut hi) = (0, n);
    let (mut at, mut measured) = (n + 1, whole);
    for _ in 0..2 {
        if lo >= hi {
            break;
        }
        let guess = ((at as f32 * width / measured) as usize).clamp(lo + 1, hi);
        let w = measure(&cut(guess));
        if w <= width {
            lo = guess;
            if guess < hi {
                if fits(guess + 1) {
                    lo = guess + 1;
                } else {
                    hi = guess;
                }
            }
            break;
        }
        hi = guess - 1;
        (at, measured) = (guess, w);
    }
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    match lo {
        0 => "..".to_owned(),
        k => cut(k),
    }
}

/// Names fitted once the bundled fonts are in, by name, width and size:
/// rows scrolled back into view fit theirs without measuring.
type Fitted = std::collections::HashMap<(String, u32, u32), String>;

thread_local! {
    static FITTED: std::cell::RefCell<Fitted> = Default::default();
}

/// Whether the bundled fonts are registered (names fitted before measure
/// with the system's and are not kept).
static FONTS_IN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// [`two_dots`] with the platform's text engine, in `size` sp Roboto.
pub fn fit(name: &str, width: f32, size: f32) -> String {
    let key = (name.to_owned(), width.to_bits(), size.to_bits());
    if let Some(hit) = FITTED.with(|f| f.borrow().get(&key).cloned()) {
        return hit;
    }
    let font = format!("{size}px Roboto");
    let fitted = two_dots(name, width, |s| measure_text(&font, s).0);
    if FONTS_IN.load(std::sync::atomic::Ordering::Relaxed) {
        FITTED.with(|f| {
            let mut f = f.borrow_mut();
            // A few thousand channels' names; a new window size starts over.
            if f.len() >= 8192 {
                f.clear();
            }
            f.insert(key, fitted.clone());
        });
    }
    fitted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_files_are_the_families_the_app_names() {
        for (family, data) in FILES {
            assert_eq!(nsh_core::font_family(data).as_deref(), Ok(family));
            assert!(nsh_core::font_data(data).unwrap().len() > data.len());
        }
        for list in [SANS, CONDENSED] {
            assert!(list.ends_with("sans-serif"), "{list}: system fallback");
        }
        assert!(CONDENSED.starts_with("'Roboto Condensed'"));
    }

    #[test]
    fn long_names_end_in_two_dots() {
        // Ten units a character.
        let measure = |s: &str| s.chars().count() as f32 * 10.0;
        assert_eq!(two_dots("Short", 100.0, measure), "Short");
        assert_eq!(two_dots("Exactly ten", 110.0, measure), "Exactly ten");
        assert_eq!(two_dots("A rather long name", 100.0, measure), "A rather..");
        assert_eq!(two_dots("Ελληνικό κανάλι", 80.0, measure), "Ελληνι..");
        assert_eq!(two_dots("Name", 25.0, measure), "..");
        assert_eq!(two_dots("Name", 0.0, measure), "Name");
    }

    /// The binary search `two_dots` replaced, for its results.
    fn searched(name: &str, width: f32, measure: impl Fn(&str) -> f32) -> String {
        if width <= 0.0 || measure(name) <= width {
            return name.to_owned();
        }
        let ends: Vec<usize> = name.char_indices().map(|(i, _)| i).skip(1).collect();
        let cut = |k: usize| format!("{}..", name[..ends[k - 1]].trim_end());
        let (mut lo, mut hi) = (0, ends.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if measure(&cut(mid)) <= width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        if lo == 0 { "..".to_owned() } else { cut(lo) }
    }

    #[test]
    fn names_are_cut_as_a_binary_search_cut_them_with_fewer_measures() {
        // Uneven widths: narrow i and l, wide W and M, a space, dots.
        let width_of: fn(char) -> f32 = |c| match c {
            'i' | 'l' | '.' | ' ' | ':' => 4.0,
            'W' | 'M' => 16.0,
            c if c.is_uppercase() => 12.0,
            _ => 9.0,
        };
        let names = [
            "US : COMEDY CENTRAL FHD 1080",
            "AU: 10 NICKELODEON (BRISBANE)",
            "Wide MMMM WWWW name with milli illi",
            "Ελληνικό κανάλι ενημέρωσης",
            "Short",
        ];
        let (mut ours, mut theirs) = (0, 0);
        for name in names {
            for width in (0..=300).step_by(7).map(|w| w as f32) {
                fn counted(
                    calls: &std::cell::Cell<usize>,
                    width_of: fn(char) -> f32,
                ) -> impl Fn(&str) -> f32 + '_ {
                    move |s: &str| {
                        calls.set(calls.get() + 1);
                        s.chars().map(width_of).sum::<f32>()
                    }
                }
                let (a, b) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
                assert_eq!(
                    two_dots(name, width, counted(&a, width_of)),
                    searched(name, width, counted(&b, width_of)),
                    "{name} in {width}"
                );
                assert!(
                    a.get() <= b.get() + 4,
                    "{name} in {width}: {} measures",
                    a.get()
                );
                (ours, theirs) = (ours + a.get(), theirs + b.get());
            }
        }
        assert!(
            ours * 4 < theirs * 3,
            "{ours} measures, the binary search {theirs}"
        );
        // A usual cut: the whole name, two guesses and a neighbour (the
        // binary search: six).
        let calls = std::cell::Cell::new(0);
        let measure = |s: &str| {
            calls.set(calls.get() + 1);
            s.chars().count() as f32 * 10.0
        };
        assert_eq!(
            two_dots("A rather long channel name", 120.0, measure),
            "A rather l.."
        );
        assert!(calls.get() <= 4, "{} measures", calls.get());
    }
}
