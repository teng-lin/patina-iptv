# Bundled fonts

The app's text is set in Roboto and Roboto Condensed (`src/fonts.rs`
registers them at startup on every platform). They come from Google's
official release, not from any other app:

| File | Source | Licence |
| --- | --- | --- |
| `Roboto-Regular.woff2` | `unhinted/static/Roboto-Regular.ttf` | SIL OFL 1.1 |
| `Roboto-Medium.woff2` | `unhinted/static/Roboto-Medium.ttf` | SIL OFL 1.1 |
| `RobotoCondensed-Regular.woff2` | `unhinted/static/RobotoCondensed-Regular.ttf` | SIL OFL 1.1 |
| `OFL.txt` | the licence, from the same tag | – |

- Release: <https://github.com/googlefonts/roboto-3-classic/releases/download/v3.016/Roboto_v3.016.zip>
  (SHA-256 `1653dbe12f248da8fb0b9920db7b9496cd677ed3981154f6f15285c8bd4e334f`)
- Licence: <https://raw.githubusercontent.com/googlefonts/roboto-3-classic/v3.016/OFL.txt>
  (SHA-256 `061402327a96aadb0bfb694a960ed289ecd38d383e396243831ab81feb109c41`)

`tools/fetch-fonts.sh` rebuilds these files: it checks both downloads
against the sums above and subsets each face with fontTools 4.60.1 to
Latin (Latin-1, Extended-A/B and Additional), Greek, Cyrillic, punctuation,
currency and the symbols Roboto has, as WOFF2, keeping every name record
(copyright and licence included). The output is deterministic:

| File | Bytes | SHA-256 |
| --- | --- | --- |
| `Roboto-Regular.woff2` | 87 812 | `18dc5170d439bd471fb4084e591d42c9dc216e1b51d0e1a162167b89de104e93` |
| `Roboto-Medium.woff2` | 91 100 | `33547fd4c2b23120aed714713a2674418195ecae3ff3e9f26abadf6dd059b6be` |
| `RobotoCondensed-Regular.woff2` | 88 644 | `0ef9cb1f5dfb3b6b4f8e28dac1e1a023ad61d43819d6e24c70f04546c214804a` |

Characters outside the subset (Arabic, Hebrew, CJK, Thai, Devanagari, ▶,
★, emoji) are drawn with the system's fonts: every family list the app
names ends in `sans-serif`, and the platforms fall back per character.

## Chinese, Japanese and Korean

Install system fonts covering simplified Chinese, traditional Chinese,
Japanese and Korean when running Patina on a minimal desktop installation.
For example, a distribution's Noto CJK package supplies these scripts.
Android and Apple systems normally supply their own fallback fonts.
Patina relies on those fonts for both interface text and playlist content.
Bundling a subset of the interface catalogue would not cover arbitrary
channel names or programme titles, so it would still require system fonts.
Check all four CJK interface choices with synthetic channel and programme
names when changing font registration or packaging.
