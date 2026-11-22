# Layer boundaries: refactoring plan

Proposed on 2026-11-22. The local spike established the inventory and
commit order below. The delivery uses three pull requests.
[Architecture](../architecture.md) states the layers, the rules and the
departures this plan removes. Read its Terms first: Domain, Application,
presentation state, state holder, View, front end.

## Goal

After three pull requests:

- `iptv-core` holds Domain values and rules only.
- A new crate, `iptv-presentation`, holds every feature's state holder, its
  display data and the localization. Only the two front ends depend on it,
  and a manifest check enforces that, including feature aliases.
- `iptv-runtime`, the adapters and the Domain crates cannot name a state
  holder, a form field or a translation function, because no crate they
  depend on has one.
- Layout policy, type sizes and gesture distances are in `iptv-app`, each
  declared once.

What the three pull requests leave open is listed under
[Not in this plan](#not-in-this-plan): `iptv-web` in two layers, the
wizard issuing an import's identity, refusals worded as sentences, and the
browser catalog read inside a lookup. The architecture's departures table
keeps those rows.

## Shape: three pull requests

The repository merges with merge commits, so a pull request's commits
survive. The unit of work is the commit: each one compiles, passes the
tests of the crates it touches and does one thing. The pull request is the
unit of review, CI and revert.

| Pull request | What it is | Behavior |
| --- | --- | --- |
| 1. Untangle in place | The lower layers stop naming presentation state, each mixed file is split in two inside its crate, and View code goes to `iptv-app`. The two checks arrive. | Unchanged |
| 2. Listings as values | The schedule returns a title, a gap or a loading slot instead of one string. | Changes at one edge, which is a bug today |
| 3. The crate | `iptv-presentation` is created and the presentation files move into it. Renames only. | Unchanged |

After pull requests 1 and 2 the runtime and the adapters name no state
holder, and no crate below the front ends but `iptv-core` translates. That
is a place to stop with the value kept, if 3 has to wait.

## What does not change

- One state holder per feature. No holder is split by kind of state.
- `iptv-nav`, the adapters, the runtime's services and the front ends'
  routing (`input.rs`, `overlays.rs`, the bridge's `app.rs`).
- Stored data. No schema, snapshot or settings format changes.
- The bridge's own small application, and `iptv-web` as one crate.

## Constraints

- [AGENTS.md](../../AGENTS.md): synthetic data only; portable crates build
  for `wasm32-unknown-unknown`; at most two integration test targets per
  crate (`tools/ci/test-check-plan.py`); check before finishing.
- The catalog lint (`crates/iptv-app/tests/app/catalog_lint.rs`) keys
  `ROOTS` (directories), `NOT_READ`, `MESSAGE_FILES`, `LIST_NAMES` and
  `catalog_allowlist.txt` by source path, and a stale line fails.
  Twenty-two allowlist lines name `iptv-core` files and seven name
  `iptv-library` holders.
- Four lockfiles: `Cargo.lock`, `tools/ci/package.Cargo.lock` (the generated
  runner workspace's, compared member by member by
  `tools/ci/check-package-lock.py`), `hosts/tizen-nsh/Cargo.lock` and
  `fuzz/Cargo.lock`.
- `cargo doc` runs with `-D warnings`: a doc link to a moved item fails.
- A crate with no covered lines fails its coverage floor
  (`tools/ci/check-coverage-floors.py`).
- These paths are busy: about 588 commits touched them in the last 30 days,
  and the worktree `patina-iptv-issue-214` is live.

## Placing an item

The architecture's placement questions decide. Where they leave a doubt, an
item that only the front ends name moves, and an item the runtime, an
adapter or a Domain crate names in shipped code stays, with what it needs.

Two rules of Rust shape every split:

- **A type's inherent methods live in its crate.** A method of a Domain
  type that takes or returns a presentation type becomes a function in the
  presentation. A holder's `impl` blocks all move with it (`Focus` has them
  in `browse.rs`, `guide/floor.rs` and `guide/moves.rs`).
- **Private items do not cross a crate.** A private field, helper or
  constant both halves use needs a shared API. The visibility changes
  are listed below. Guide interaction fixtures used within presentation
  stay private there.

## Every commit is whole

In all three pull requests a commit carries everything its change breaks:
the paths that name what it moved, the tests, the catalog lint's path
lists and allowlist lines, the doc links, and the documents that name a
moved module. Nothing is left for a later commit to repair.

## Before pull request 1: a spike

A throwaway branch does all three pull requests crudely, in the commit
order proposed here. It is never merged. At each proposed commit it runs
the per-commit checks of [Verification](#verification), with the
test-support features on, so that a gated test that names a moved item is
found, not only shipped code. Its output
replaces the lists below in this file before pull request 1 opens:

- for each mixed file, what stays and what moves;
- every method that must become a function and every visibility that must
  widen;
- every holder function that takes points or a distance on screen, which
  the compiler cannot tell from any other number;
- the commit order of pull request 3;
- the fixtures `iptv-guide`'s tests need exported.

The entries below record the spike's splits, path changes, and test exports.

## Pull request 1: untangle in place

No behavior changes. Front-end paths do not change: a split file's Domain
half re-exports the other half under the old path until pull request 3.

1B lands first and is reviewed as logic. 1C to 1E are moves on busy files
(`vod.rs` and `guide.rs` took 63 commits between them in 30 days), so they
are announced, pushed together at the end and merged promptly, and the
live worktree rebases after this pull request too.

### 1A. Checks

1.1 **Manifest layers.** `tools/ci/test-layers.py` holds each workspace
crate's layer and the layers it may use, as the architecture's dependency
table. It reads `cargo metadata`, so target-specific, optional, build and
renamed dependencies are seen, and judges dev-dependencies by the same
table. It also reads `hosts/tizen-nsh/Cargo.toml` and `fuzz/Cargo.toml`. It follows feature aliases that enable the bridge.
The listed exceptions carry their reason. It is named `test-*.py` and
listed in `tools/check.sh`, which is what makes CI run it.

1.2 **Inventory.** One test in `iptv-core`'s existing integration target
reads the lower crates' sources and holds the exact list of presentation
items they name, shipped and test-only apart. It is a migration inventory,
not the boundary: a source scan misses aliases, re-exports and inferred
types. The crate edge of pull request 3 is the boundary, and this test is
deleted there. The inventory also covers names in imports, localization, the guide,
test-only callers and temporary re-exports. The two commands whose
signatures name a holder are `favorites_history` and `edit_organization`. The
compile-checked contracts in `iptv-runtime/tests/it/command_values.rs` are
tightened in the commits that change those signatures, 1.4 and 1.5, not
here, so that this commit passes on today's code.

### 1B. Changes of type

One commit each. Each removes its entry from the inventory.

1.3 **Message ids.** `i18n::key` moves to a Domain module of `iptv-core`
(`message`). The runtime (six files), `iptv-guide` and `capabilities` mark
message ids with it. The lint's alias and unapplied-use guards test for the
path segment `i18n` and follow the new path.

1.4 **Recent channels.** `Recent::history` and the runtime's
`favorites::history` and `Runtime::favorites_history` return `History`,
which marks the playing row. They are removed. The shell asks
`Runtime::recent_channels`, which exists, and a presentation function
builds `History` from the answer.

1.5 **Organization edits.** `edit_organization` takes the items, a Domain
edit and the selection. Today it takes `Editor` and reads its selection,
and takes the editor's own `Action`, of which it ignores eight variants.
The runtime's `command_journeys`, `relative_organization_moves` and
`command_values` tests follow.

1.6 **The import report.** `ImportStage`, `ImportProgress`, `ImportCounts`,
`FailureKind`, `ImportFailure`, `ImportId`, `SecretText`, `UrlText`,
`PlaylistDraft` and `DraftSource` go to a Domain module of `iptv-core`
named for what they are (`add_playlist`).

- `ImportFailure::field` becomes which input failed (the playlist's
  address, its credentials, the guide's address). The wizard maps it to a
  field; `imports.rs` and `wizard_map.rs` stop naming `Field`.
- The wizard builds `SecretText` and `UrlText` through their constructors;
  today it uses their private fields.
- `validate_url` gets a Domain error. `FieldError` stays the form's.
- `PlaylistDraft::kind`, which returns the form's `PlaylistKind`, has only
  test callers and is deleted.
- `ImportId` becomes a Domain type. Who issues it does not change.

1.7 **The default playlist name.** The runtime derives a new playlist's
name: the name given, else a host or file stem with no credentials in it,
else the translated word "Playlist". Only the last step translates. The
caller passes that word in, already translated. The derivation, its
precedence and its tests stay in the runtime, and an omitted name stays
`None` in the draft.

1.8 **Tracks.** `Tracks::{entries, entries_in, summary, summary_in}` call
the label functions and the catalog. They become functions beside the
labels. `Kind`, `has_choice`, `selected_entry` and `pick` stay with
`Tracks`.

1.9 **`Program::select`** becomes `ProgramSelection::for_program`. The
native app and bridge both call it.

1.10 **Settings.** `backup::decode` repairs out-of-range values with
`screen::grid` in shipped code, and the screen calls the crate-private
`clamp_wide_to_grid`. Both are exposed from `iptv_settings::validate`.

1.11 **The picture-in-picture window.** `TITLE`, `WIDTH` and `popup_rects`
move to a module of `iptv-player-mpv` that every target builds, beside
`PIP_VIEW`. `size_for` moves to `iptv-app` with its tests and reads `WIDTH`
from there. `TITLE` is a window's identity: the adapter compares it with
the compositor's title and the shell sets it untranslated. It loses its
`key` marker, and any lint entry for it goes.

1.12 **Tests.** `iptv-runtime`'s tests that build `vod::Screen` use the
Domain values the commands return.

### 1C. One layer per file

1.13 Each mixed file of `iptv-core` is split in two, in place: the Domain
half keeps the file's name, the holder goes to a sibling file, and the
Domain file re-exports it so that no path changes. Unit tests go with the
half they test. This makes pull request 3's commits renames, which rebase
over a change on master; a split made during the move would have to be
redone by hand.

Each re-export is temporary and is listed in the inventory. The commit of
pull request 3 that moves a holder removes its re-export; one left behind
would make `iptv-core` depend on the crate above it.

| File | Stays in the Domain half | Goes to the holder's file |
| --- | --- | --- |
| `guide` | `Program`, `Airing`, `Schedule`, the synthetic schedule and its constants, guide time (`now_minute`, `unix_of`, `secs_to_next_minute`, `DAY`, `HOUR`), `HORIZON`, `HISTORY`, `VISIBLE`, `catchup_days`, `program_at`, `group_start`, `floor_of` | `Focus` with every `impl` block, `Pane`, `PreviewOrigin`, `Move`, `moves`, `home_window`, `view_range`, `PAST`, `STEP`, `page_row`, `ProgramSelection` |
| `lists` | `group_of`, `scan_group_of` | the fixed entries, `Entry`, `entry_count`, `entry_name`, `ListId`, `ChannelList`, `ListView`, `resume_entry` |
| `favorites` | `Favorites`, `Recent`, `KeyIndex`, `RECENT_LIMIT`, `diff` | `History` |
| `playback` | `PlaySource`, the host's `PlayerState`, `ErrorKind`, `Cause`, `Failure`, `WatchTime`, `SourceOwnership`, `Publication` | `Health` with `Phase`, `Retry`, `RetryPolicy` and `Waiting`; `ChannelHistory`; the zap targets (`ZapOrigin`, `zap_from_list`, `zap_target`, `group_range`) |
| `organization` | `Sort`, `Block`, `Blocks`, `BlockKind`, `Item`, the Domain edit of 1.5 | `Editor`, `Mode`, `Action`, `ACTIONS`, `GROUP_SORTS`, `LEAVE_CHOICES`, `step_choice` |
| `parental` | `ContentKind`, `Target`, `UnlockWindow`, `Policy`, `Access`, `Pin` | `Phase`, `Dialog`, `Submission` and masked input |
| `on_demand` | Retained-media `Progress` | `Timeline` and its transport state |
| `interruption` | The host's `Interruption` event | `Interruptions` and its transition effects |
| `recording` | the limits, `WakeCapability`, `JobId`, `Status`, `Request`, `Repeat`, `RuleId`, `RecurrenceTiming`, `Rule`, `Job` | `Screen`, `Draft`, `EntryField`, `Entry`, status labels and recurrence selection |
| `vod` | `Kind`, `Key`, `Card`, `Progress`, `CONTINUE_MIN_MS`, `Shelf`, the discovery query and page, `Category`, `Source`, `Sort`, `Query`, `Episode`, `Details`, `Playing` | `Screen`, `Control`, `Operation`, `Retried`, `Destination`, `Handoff`, `vod::library` and the sort button's cycling order |
| `tracks` | the track records, `Tracks` with `Kind`, `AUTO`, `OFF` | `Choice`, the chooser entries and every label function |
| `multiview` | `MAX_SURFACES` | everything else, with its `menu` and `layout` (fractions of the area) |
| `reorder` | `move_to_present` | everything else |
| `membership` | `Member` | `Membership` |
| `wizard` | nothing after 1.6 | the state machine and its form |
| `capabilities` | `Platform`, `Host`, `Recording` | `Reason`, `Availability`, `pip`, `multiview` and recording capability wording |
| `pip_window` | nothing after 1.11 | `Hover`, `Exit`, the two timing constants |

`VISIBLE` stays in the Domain: `EpgSchedule::browsable` guarantees one
window past now, and the window's length is that rule's constant.
`guide::visible_programs` and `playback::RECENT_MAX` have no shipped caller
and are deleted.

### 1D. View code to `iptv-app`

Moves, with their unit tests. None depends on the new crate.

1.14 **Layout.** `layout::{Class, GuideHeader, ListPanel, Shortcuts}` and
their constants go beside `fonts.rs` and `scale.rs`. `fonts.rs` and the
header geometry read one declaration of each size. Three of the app's own
scenario tests (`tests/app/ipad_windows.rs`, `phone_windows.rs`,
`pane_collapse.rs`) import the layout from `iptv_app`. `InputMode` stays,
as presentation state. The doc link to `layout::Class` in `vod.rs` is
rewritten.

1.15 **Distances on screen.** `timeline` takes a drag's distance and speed
in dp and holds its slop and friction. `playback_windows::reachable` takes
rectangles in points with its grab thresholds. `play_bar::seek_target`
turns a touch's x and the bar's measured width into a position. Only
`iptv-app` uses them. They go to `iptv-app` with their tests; the holders
they feed (`Focus`, the window `Workspace`, `PlayBar`) keep taking guide
minutes, identities and positions. Of the two `iptv-core` tests that build
a `Timeline`, one uses that file's private fixtures: its `Timeline` lines
move and its `Focus` half stays. The window geometry becomes `iptv_app::desktop::display_bounds` and
the touch seek conversion becomes private to `play_bar::progress`. The
doc link to `timeline` in `guide/moves.rs` is rewritten, and `docs/interaction.md` and
`docs/desktop.md`, which name these modules, follow in the same commits.

### 1E. `iptv-guide`'s tests

1.16 `iptv-guide`'s unit tests that drive `Focus`, the lists and the zap
targets over its schedule are split from the tests of the schedule itself,
into unit-test modules of their own. No test target is added. They stay in
`iptv-guide` until pull request 3.

Exit: the runtime and the adapters name no state holder. The inventory
holds `iptv-guide`'s translations, `iptv-library`'s own holders, the
re-exports of 1.13 and the test-only entries. Every file of `iptv-core` is
one layer apart from those re-exports and `Schedule`'s language arguments,
which go in pull request 2.

## Pull request 2: listings as values

Today `Schedule` words five things as strings:

- `title` and `title_in` return a stored title, "No information",
  "Loading…", or nothing for a row with no channel;
- `group_name` and `group_name_in` return a stored name, the guide's own
  "Other" bucket, or a list's fixed entry ("Favorites", "Recently
  watched");
- `all_channels_name` returns "All channels" or "All playlists", which
  `lists.rs` compares as strings.

Value methods replace all five:

- a listing, or none for a row with no channel: a title with the stored
  text, no information, or loading;
- a group's name: a stored name (a provider's, a renamed group's, or a
  playlist's in a combined schedule), the guide's ungrouped bucket, a fixed
  entry, or one of the two all-channels kinds.

Functions in `listings.rs` word them for presentation. `iptv-guide` stops translating and
drops `NO_INFORMATION` and `LOADING`. The bridge stops comparing titles
with strings (`iptv-web/src/grid.rs`, `app.rs`) and words the ungrouped
bucket itself. It has no remaining catalog lookup, so the linker drops the
embedded translation data. Eleven `impl Schedule` blocks change, four shipped and seven
in tests, and about 90 call sites.

What must stay the same, each with a test:

- A stored title is passed on unchanged, with the superscript folding that
  old stored guides still need. An empty one stays blank in the bridge.
- "Favorites", "Recently watched", "All channels" and "All playlists" read
  as today in every interface language.
- The ungrouped bucket is the one `iptv-guide` makes itself, by the checks
  it uses today: its key, its generated flag and its name not yet changed,
  also through a combined schedule. A group the import stored under the
  name "Other" or "Category n" is a stored name, as today. No stored data
  changes.
- A recording made from a gap keeps the name it gets today
  (`recordings_view.rs` stores the listing's wording in the request).
- The bridge's JSON keeps its three shapes for a gap or a loading slot: a
  blank grid cell, no player program object, and a null focused title.

What changes, with a test in both front ends: a program whose stored title
is "Loading…" or "No information" shows its title and is treated as a
program.

Exit: no crate below the front ends but `iptv-core` names `translate`,
`Language` or `LANGUAGES`, and no Domain file of `iptv-core` does. The
inventory holds `iptv-library`'s holders, the re-exports and the test-only
entries.

## Pull request 3: the crate

Renames, each commit whole as above: it moves its files, removes their
re-exports from `iptv-core`, and changes the front ends' paths to them.

A holder still in `iptv-core` cannot use one that has already moved, so
holders move dependents first and the localization moves last. The spike established the following commit order.

3.1 **Skeleton.** `iptv-presentation` as a workspace member with all its
dependencies declared (`iptv-core`, `iptv-epg`, `iptv-providers`,
`iptv-library`, `iptv-search`, `iptv-settings`, `zeroize`), and the two
front ends depending on it. A commit that changes a manifest carries its
lockfiles, which the `--locked` check holds: at least this one, 3.3, 3.5
and any manifest dependency changes that follow. With it:
`tools/ci/check-plan.py` (`PORTABLE`), `docs/contributing.md`'s portable
block, `.github/pull_request_template.md`, the catalog lint's roots, and
the manifest check: only `iptv-app` and `iptv-web` (under its `bridge`
feature) may depend on the crate. The crate is exempt from a coverage
floor while it is empty; 3.2 gives it one.

3.2 **Holders nothing else builds on.** `panels`, `play_bar`, `seek_hold`,
`channel_number`, `library_recovery`, `player_options`, `player_info`,
`pip` with `capabilities::{pip, multiview}`, the window `Workspace`,
`editor`, `browse_playback`, `InputMode`, `Membership`, the parental
`Dialog`, the recording `Screen` and `Draft`, the on-demand `Screen` with
`vod::library`, the organization `Editor` and `Action`, `Health`,
`ChannelHistory`, `favorites::History`, the retained-media `Timeline`
and the interruption holder beside their domain records. The membership
scale scenario moves into the presentation integration harness.

3.3 **The other crates' holders.** `iptv_library::{details, epg_sources,
guide_assign, guide_inspect, guide_repair_screen, playlists}`, which use
the wizard's field types and so move before it;
`iptv_search::{state, selection}`; `iptv_settings::screen` with
`tests/it/screen.rs`. `iptv-library` no longer needs `iptv-core` after
this, which changes `fuzz/Cargo.lock`.

3.4 **Holders others built on.** `menu`, `multiview` with `heard`, `sound`,
the picture-in-picture `Hover` and `Exit`, the track labels, `reorder`,
`wizard`. `MAX_SURFACES` and `move_to_present` stay in core. Core exposes
its redacting `add_playlist::url_origin` to the moved wizard.

3.5 **The guide.** Move `guide`'s holder and its inherent implementations,
`browse`, `lists`, `listings`, `channel_list`, `channel_picker` and
`zap` together. Browsing and list unit tests use the same private test
catalog. The upper-layer personalization scenarios from core and guide
and the interaction scenarios from 1.16 join presentation's existing
integration harness. The app keeps its public `guide` path for callers.

Presentation uses `iptv-guide` with its `test-support` feature as a
dev-dependency. Guide exports these fixture modules only for tests or
when that feature is enabled:

| Module | Fixture API |
| --- | --- |
| `combined` | `NOW`, `part` |
| `pages` | `NOW`, `LEN`, `FIRST`, `Requests`, `store_between`, `title_at`, `paged`, `deliver`, `minute_of` |
| `schedule` | `NOW`, `xmltv`, `store_for`, `store_with`, `store_from`, `schedule`, `mid_minute_schedule` |

3.6 **Localization.** `i18n`, `clock`, `messages.tsv` and the
`external-catalog` feature. `iptv-app`'s `browser-package` names the new
crate's feature; `iptv-app/src/i18n.rs` and `i18n/pseudo.rs` include the
catalog from its new path; `hosts/tizen-nsh/catalog.py` and `build.sh`
follow.

3.7 **Documents and the inventory.** The inventory test is deleted.
`docs/architecture.md` (the departures table keeps what is left, the
diagram loses its asterisk), `AGENTS.md` ("State lives in the core" and
"check before finishing" name the layers and both crates),
`docs/contributing.md`, `CONTRIBUTING.md`, `docs/localization.md`,
`docs/interaction.md`, `docs/accessibility.md`, `docs/capabilities.md`,
`docs/desktop.md`, `hosts/tizen/README.md`, `README.md`,
`docs/plans/2026-11-22-shared-interaction-layer.md`, and the crates'
descriptions and crate-level docs.

3.8 **Remaining display helpers.** Move the viewport's `PAST`, the
playback health states, recording capability wording, recording status
labels and recurrence selection, PIN masking, and VOD sort cycling.
Exercise the external reader in isolation so the native all-feature
tests use the embedded catalog, as native production builds do.

Two things to watch in every commit of this pull request:

- `compile_fail` doctests name their new paths, or they pass for the wrong
  reason.
- A test goes with what it tests; a test of two layers goes to the upper
  one.

Pull request 3 is announced before it opens, has no other change to the
moved files open against it, and lands within a day. The live worktree
rebases after it.

## Verification

Every commit:

- `cargo check --workspace --all-targets --locked --features
  iptv-runtime/test-support,iptv-app/test-support`. Without the features
  the app's scenario target is skipped, and with it the catalog lint and
  the scenarios that import the layout;
- `cargo test` for the crates it touches, and `cargo test -p iptv-web`;
- the catalog lint (`cargo test -p iptv-app --features test-support --test
  app catalog_lint`) when it moves or splits a file under a lint root, or
  touches a message;
- `cargo doc` when it moves a module;
- the wasm32 build of the portable crates.

CI checks only a pull request's head, so each pull request records a run
of those checks over its commits with `git rebase -x`, and does not merge
without it.

Every pull request, before it merges:

- `tools/check.sh` at the full tier, `cargo doc`, and the coverage floors
  (coverage is not measured on pull requests, so it is run for these);
- the full app scenario suite, and `iptv-tui`'s build;
- a run on `tab36` or `tv36`, since every Android build links these crates;
- `hosts/tizen/build.sh` and `hosts/tizen-nsh/build.sh`.

Also:

- Pull request 1: the desktop picture-in-picture scenario on GTK, and
  touch panning of the guide on a tablet.
- Pull request 2: the bridge on the supported TV, for the guide and the
  player's program labels.
- Pull request 3: the NSH Tizen package's launch.

A move is reviewed as a move: its commit edits no function body, and
`git diff --color-moved` shows it.

## Measurements

Recorded before pull request 1 and after each pull request:

- the bridge's module size on the staged Tizen build, which is the release
  build with link-time optimization (2,059,133 bytes at `b0fc1d48`). CI's
  `web-core-wasm` step is a development build and shows a trend only;
- the NSH Tizen package's code and data sizes and its cold start;
- `cargo build -p iptv-app --timings`, clean and after a one-line change to
  the catalog.

A regression above 1% in a staged size or a start time is explained or
fixed before the merge.

## Rollback

Pull requests 1 and 2 revert on their own. Pull request 3 reverts as a
whole through its merge commit while its freeze holds. After that, at the
rate these paths change, a revert conflicts at once: a fault is found by
bisecting the pull request's commits and fixed forward. If everything but
the skeleton is reverted, the empty crate is listed among the coverage
exemptions until it has code.

## Risks

| Risk | Guard |
| --- | --- |
| The lists are wrong again | The spike checks every target at every proposed commit before pull request 1 opens. |
| A change of type alters behavior | 1B's commits are one change each with its tests; pull request 2 lists what must stay the same, each with a test. |
| A commit in the middle does not build | Every commit is whole, and the per-commit run is recorded. |
| Pull request 3 conflicts with work in flight | It is renames only, announced, short-lived, with no competing change to the moved files. |
| The bridge's module grows | Measured per pull request on the staged build, with a threshold. |
| The catalog lint's path lists rot | Updated in the commit that moves the file; the lint fails otherwise. |
| The new crate becomes the next catch-all | Only the two front ends may depend on it, so nothing below can use it to share a value. Its modules are one per feature. |

## Not in this plan

Each is its own issue and stays a row of the architecture's departures
table. None blocks the three pull requests.

- **An import's identity** (rule 5). Each `Wizard` numbers its imports from
  1, the callback captures that number, and the runtime's cleanup clears
  its slot whatever the number. The fix has the runtime issue the identity
  when it accepts the import, tags replies at their source and makes
  cleanup conditional. It changes a protocol, and its tests are a late
  progress, a completion and a cancellation from one wizard's import
  arriving after another wizard started its own.
- **Refusals as sentences** (rule 7). `recording::Request::validate`
  returns English sentences that reach the screen through the store's
  error, the runtime's admission and the recordings view. A typed reason
  replaces them.
- **The browser catalog** (rule 4). In a browser package the first lookup
  of a language reads a package file inside `translate`, and a failed read
  stays an empty catalog. Loading it in the host belongs with the
  [browser TV plan](2026-11-21-browser-tv-architecture.md).
- **A message id type** (rule 7). `key` is a marker function, so an
  unmarked sentence looks the same to the compiler. A `MessageId` type
  would make the runtime's notices checkable.
- **`iptv-web` in two crates** (rule 1). It needs an owner for the wasm
  entry points both workers call (`lib.rs`), a test-support API for the
  bridge's fake host, `iptv-web` among `check-package-lock.py`'s unpackaged
  members, and the CI steps that name the adapters build. The bridge
  retires with Tizen 5.5, so this waits for that decision.

## Decisions to confirm

1. The crate's name, `iptv-presentation`.
2. `guide::VISIBLE` staying in the Domain as the schedule's rule constant.
3. `Health` and `ChannelHistory` moving up: only `iptv-app` names them, and
   they last while a person watches.
4. `timeline` and the window grab geometry going to `iptv-app` as View
   code, because they take distances on screen. The alternative keeps them
   as presentation state and lets a holder take device-independent
   distances.
5. Whether the import report stays a second vocabulary beside
   `iptv-import`'s own `Progress` and `FailureKind`, with `wizard_map.rs`
   translating. This plan keeps both; merging them is a follow-up.
