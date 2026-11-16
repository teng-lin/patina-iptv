# Interface languages

Choose **Settings → General → Language**. The choice applies immediately and
is saved in settings and backups. **System language** uses the device's preferred
locale when the app starts. Unsupported device languages fall back to English.

The initial catalogue provides English, Spanish, French, German, Italian,
Portuguese, Russian, Simplified Chinese, Traditional Chinese, Japanese and Korean.
Regional variants use the base language (for example `es-MX` and `pt-BR`). Chinese
script tags take precedence over regions: `zh-Hant` uses Traditional Chinese;
`zh-TW`, `zh-HK` and `zh-MO` also use Traditional Chinese unless `Hans` is explicit.

## What is translated

The catalogue now covers the known application-owned UI inventory in all eleven
languages: navigation, settings names/choices/help, playlist and guide setup,
backup/restore, channel organisation, guide/search/multiview hints, playback and
track controls, recordings, parental controls and movie/series flows. Counts and
dates use the selected language's formatting rules.

The original untranslated inventory contained 437 source literals. These domain
batches reduced its pinned baseline to zero:

| Domain | Original literals | Remaining |
|---|---:|---:|
| Settings names, choices and help | 305 | 0 |
| Palette names | 18 | 0 |
| Guide and channel browsing | 19 | 0 |
| Playback, tracks and picture controls | 64 | 0 |
| Search | 12 | 0 |
| Multiview | 10 | 0 |
| Favourites and channel menus | 5 | 0 |
| Desktop menus | 3 | 0 |
| Generated playlist name | 1 | 0 |
| **Total** | **437** | **0** |

436 entries are displayed messages. One entry, the persisted locale token
`system`, is a configuration value that the picker displays as the translated
**System language**; its classification is documented under `[not-shown]`.
No displayed message was moved there to reduce the count.

This is catalogue coverage, not completed human language review. The new
translations are machine-drafted and reviewed for source meaning, conditions,
placeholders and application context. Fluent speakers still need to review all
ten non-English locales for grammar, natural wording and terminology consistency.
Identical source/translation values do not establish completion: standard codec,
protocol and brand names remain unchanged intentionally. The desktop
`Picture-in-Picture` window title also stays stable for compositor window rules;
its controls have localized accessible labels.

Unknown messages still fall back to their English source. The token-based lint
has the limitations described below, so a zero baseline does not prove that no
unrecognised or dynamically built text exists. New messages must use the existing
catalogue helpers and complete locale columns. Provider channel/playlist names,
programme titles/descriptions and track labels remain opaque. Generated guide
loading/gap labels and the ungrouped bucket are localized by provenance; a real
provider title named **No information** or a provider group named **Other** stays
unchanged. An automatically generated playlist name is localized when created
and then behaves like any other saved name.

Messages shown before the settings load (a failed start, a restore that could
not be applied) use the device's language.

## Counts

A count chooses the form its language needs: `1 channel`, `5 channels`;
`1 канал`, `2 канала`, `5 каналов`, `21 канал`; `5 个频道`. The rules are the
Unicode CLDR's cardinal rules for whole numbers, written out for the eleven
languages in `crates/iptv-core/src/i18n/plural.rs`:

| Languages | Forms |
|---|---|
| English, German | `one` (1), `other` |
| Spanish, Italian | `one` (1), `other`; optionally `many` (whole millions) |
| French, Portuguese | `one` (0 and 1), `other`; optionally `many` |
| Russian | `one` (1, 21, 31…), `few` (2–4, 22–24…), `many` (0, 5–20…), `other` (fractions) |
| Chinese (both), Japanese, Korean | `other` |

A twelfth language whose rules are none of these needs an arm there; if its
rules are more than a few lines, that is the point to take a library.

## Dates and times

Weekday and month names, their order, and the 12-hour day's two halves come
from `crates/iptv-core/src/i18n/calendar.rs` (`Tue 10 Nov`, `Di., 10. Nov.`,
`11月10日(火)`). **Settings → General → Clock** chooses the clock:

- **As the language writes it** (the default): 12-hour for English outside
  the United Kingdom, Ireland and South Africa, for Korean and for
  Traditional Chinese; 24-hour for the rest. With **System language**, the
  device's locale decides (`en-GB` is 24-hour); with a chosen language, that
  language does. A device language the app has no translation for keeps the
  24-hour clock. The app does not read Android's own 12/24-hour switch.
- **12-hour** or **24-hour**, whatever the language.

Durations (`1:05:30`), time shifts (`+1:30 h`) and stream figures are numbers,
not times of day, and are the same in every language.

## Maintaining translations

The portable lookup, formatting and language resolution live in
`crates/iptv-core/src/i18n.rs` and `crates/iptv-core/src/i18n/`; platform
locale discovery is confined to `crates/iptv-app/src/i18n.rs`. `sys-locale`
supplies the native device locale. No network service is involved in
translating messages, and the core takes no dependency for it.

`crates/iptv-core/src/i18n/messages.tsv` is UTF-8 with one message per line.
Columns follow `LANGUAGES` in `i18n.rs`: English source, Spanish, French, German,
Italian, Portuguese, Russian, Simplified Chinese, Traditional Chinese, Japanese,
Korean. Each row must have every column, with no empty translations or duplicate
source messages. Tabs and newlines cannot appear inside messages.

### Calls

| Call | For |
|---|---|
| `tr("English source")` | A fixed message. |
| `trf("Delete “{name}”?", &[("name", name.into())])` | A message with values, counts or both. |
| `key("English source")` | A message kept as data (a `const`, an error's text) and shown later through `tr`. |

Use them only for app-owned messages. Call them inside a reactive closure when
text must change while the view remains mounted. Never apply `tr` to arbitrary
text in a shared text renderer: that could translate provider content. Provider
content may be a *value* of `trf`; it is never the message.

Do not build a sentence from translated pieces with `format!`: word order
differs. Give the whole sentence to `trf` and let the translation place the
values. Joining independent facts with ` · ` is fine.

### Message syntax

A message given to `trf` is the plural subset of ICU MessageFormat:

```text
Delete “{name}”?
{count, plural, one {# channel} other {# channels}}
{count, plural, one {Updated {when} · # channel} other {Updated {when} · # channels}}
```

- `{name}` is replaced by the value of that name. A translation keeps every
  placeholder of its source, with the same names, in any order.
- `{count, plural, …}` chooses one alternative by the count; `#` is the number.
  Each alternative is the whole phrase, so every word can agree with the
  number. A translation has the forms of its own language (the table above),
  not the source's: Russian has four, Japanese one.
- `=0 {…}` names an exact count and wins over the category.
- There is no quoting: a message cannot show a literal `{`, `}` or, inside a
  plural, `#`.

Tests in `i18n.rs` check every row: the same placeholders as the source, each
plural with exactly the forms its language uses, and no braces in a message
that `tr` shows as written.

### The lint

`crates/iptv-app/tests/app/catalogue_lint.rs` reads the sources of the app,
the core, the guide and library crates and the settings catalogue, and fails
on:

- a literal given to `tr`, `trf` or `key`, to the core's `translate` or
  `format`, or to a helper or message type it knows, that has no catalogue
  row. `concat!("…", "…")` is the literal it makes;
- a message given as a name (`tr(label)`, `button(ui, size, label, act)`)
  that is not a listed carrier of checked messages;
- a message given in a way the lint cannot follow. It follows a literal, a
  name with its path, fields and calls, and a parenthesis, a block, an
  `if`/`else` or a `match` whose every outcome is one of these. Everything
  else is refused, not guessed at: `tr(a + b)`, `tr(&format!(…))`,
  `tr(if x { "On" })`, `.map(tr)`, `use crate::i18n::tr as t`;
- a literal message whose placeholders are not the values beside it:
  `trf("{count} items", &[("cnt", n.into())])`, or `tr` of a message with a
  placeholder;
- a literal that reads as text for a person and is neither a message nor on
  `catalogue_allowlist.txt`;
- an allowlist line that matches no literal any more, or an `[untranslated]`
  section larger or smaller than the size pinned in the test.

So new text is translated when it is written, and the untranslated list only
shrinks. To translate a listed literal: wrap it in `tr` or `trf`, add its row
with all eleven columns, delete its allowlist line and lower `UNTRANSLATED`.
A literal that never reaches the screen (a log line, an internal reason,
markup) goes under `[not-shown]`, in the group that says why.

When the lint refuses a message call, write it plainly: give the call the
literal, or choose between literals with an `if`/`else` or a `match` inside
the call. A function that translates a label it is given is listed in
`TRANSLATING_HELPERS`, and the name it gives to `tr` in `CARRIED_MESSAGES`.

The lint reads tokens: it resolves no names and knows no types. It does not
catch:

- one word in a call it does not know to show text: `Button::new("OK")`,
  `"EPG"` in a struct field. Outside the calls listed in `TEXT_SINKS`, only
  a capitalised word (`Cancel`) or several words read as text;
- text assembled from pieces that are not literals;
- a listed carrier name reused for another variable in the same file: a
  carrier is a file and a name, not a binding;
- a new function that translates its argument, until its `tr(label)` asks
  for a carrier line and a person lists the helper;
- a message call behind a macro, or a helper reached through a `use` or a
  module path other than its file's;
- values for `trf` that are made outside the function that calls it.

These are for review. The lint's header lists them with the reasons.

Adding a language also requires adding its tag to
`iptv_settings::catalogue::choices::LANGUAGE`, its plural forms to `plural.rs`
and its calendar to `calendar.rs`; tests check each of the three.
The setting remains text-valued so older backups and unlisted locale tags survive.

Run `cargo test -p iptv-core -p iptv-settings`, the core wasm build, the app
build, `cargo test -p iptv-app --features test-support --test app catalogue_lint`
and `cargo test -p iptv-app --features test-support --test app interface_languages`. The `interface_languages` headless
scenarios check switching languages without remounting settings, persistence,
returning to the system preference, and the translated flows, counts, dates
and clock. Headless tests pin their system locale through
`IPTV_RS_TEST_LOCALE`, available only in test builds.

## Layout and regression fixtures

`test-support` builds can set `IPTV_RS_TEST_PSEUDOLOCALE=1` to double and accent
fixed catalogue labels. This test-only map accepts catalogue keys; it never
expands provider values or changes persisted language choices. It does not alter
formatted/plural messages: Russian plural cases and real Chinese/Japanese text
are exercised separately. Production builds contain neither this map nor the
environment override.

The interface-language scenarios cover mounted labels, settings help and
accessible labels while switching Spanish → Traditional Chinese → system.
The archive/restart scenario runs in English and Traditional Chinese and replaces
the current preference before activating the archive, proving that the restored
choice comes from the backup. A pseudolocalized scenario exercises large app text
plus system font scaling at TV, tablet and desktop sizes. Headless geometry and
synthetic emulator checks do not substitute for native font rendering, TalkBack,
VoiceOver or real hardware checks; record those results separately.
