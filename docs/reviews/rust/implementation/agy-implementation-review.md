# agy implementation diff review

Six independent packet reviews of the implementation diff, without tool access.
This is diff-only evidence; it does not establish full-source or test coverage.
Snapshots predate the later targeted policy-cache/catch-up corrections; independent
native and portable source reviews cover those corrections.

## Dispositions

- Packet 1: accepted. Unresolved search identities cannot compare equal to an absent
  playing row; use `is_some_and` before displaying Playing.
- Packet 5: accepted as public adapter hardening. The runtime caller correctly supplies
  initial rows only after creating a source, but the adapter should also preserve
  existing rows and durable reservations if a stale seed is supplied. The correction
  and three direct publication regressions are implemented. The suggested source
  existence check would reject legitimate new-source seeding because `add_source`
  precedes publication.
- Packets 2, 3, 4 and 6: no concrete finding in the supplied scope.

## Execution and immutable packet identities

- Packet 1: exit 0; 40.9 seconds; prompt SHA-256 `95b81f509d85d87435e45c6136210ef8dde5797f606929214d24a26dde1ed929`.
- Packet 2: exit 0; 44.32 seconds; prompt SHA-256 `2fb9ffa47e2074e68b8afe95ba1eb396464f8e40b06697cbbb306c6113ab0c9d`.
- Packet 3: exit 0; 50.63 seconds; prompt SHA-256 `6d404465d760237653fd2119b5ee49555c3ec994b6a83e15254ff5525440421b`.
- Packet 4: exit 0; 45.33 seconds; prompt SHA-256 `470cc7afe396e3a553e0663a12f5d739f928c33aedb09d7a035da745213385c9`.
- Packet 5: exit 0; 54.87 seconds; prompt SHA-256 `bb86672566b450babf728edf6f3060e4543fe5d7cd5008d780a8b25d93d808fa`.
- Packet 6: exit 0; 23.88 seconds; prompt SHA-256 `0a20849fc1634249f70e35abef1d4802e1fb8a7eb1e448cc41273a0581ea60e3`.

## Packet 1: unmodified review output

### Findings

1. **Medium — Unresolved search channel row matches `None == None` as `"Playing"`**
   - **Location:** [crates/iptv-app/src/search_view/view.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-app/src/search_view/view.rs#L319)
   - **Introduced vs. preexisting:** Introduced in this diff. Previously the guard compared `ui.playing.get() == Some(c)`, which could never match when `ui.playing.get()` was `None`.
   - **Failure scenario:** In `result_row`'s `line2` closure, `crate::runtime().resolve_search_channel(&id, &*crate::schedule()) == ui.playing.get()` compares two `Option<usize>` values. If `resolve_search_channel` returns `None` (e.g., after a schedule refresh/incarnation change or while `line2` re-evaluates reactively) and nothing is currently playing (`ui.playing.get() == None`), `None == None` evaluates to `true` and displays `"Playing"` on the row.
   - **Smallest fix:**
     ```rust
     Some((ResultId::Channel(id), _))
         if crate::runtime()
             .resolve_search_channel(&id, &*crate::schedule())
             .is_some_and(|c| ui.playing.get() == Some(c)) =>
     {
         tr("Playing").to_owned()
     }
     ```

### Scope & Validation Note
This review is limited strictly to the supplied diff packet; full repository source files, builds, and test suites were **not** inspected or executed (no tools used).

## Packet 2: unmodified review output

**No findings** in the supplied diff packet.

### Summary of Reviewed Changes
- **Diagnostic redaction** ([crates/iptv-guide/src/playback.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-guide/src/playback.rs#L121-L131), [crates/iptv-library/src/epg_sources.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-library/src/epg_sources.rs#L440-L454), [crates/iptv-library/src/guide_inspect.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-library/src/guide_inspect.rs#L262-L268), [crates/iptv-library/src/playlists.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-library/src/playlists.rs#L849-L854)): Custom `Debug` implementations redact `M3u` headers, `StalkerLink` commands/headers, and raw edit/link input fields, with synthetic regression tests.
- **Cancellation waker lifecycle** ([crates/iptv-import/src/cancel.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-import/src/cancel.rs#L46-L112)): `Cancelled` tracks its own `Option<usize>` slot in `Vec<Option<Waker>>`, replaces stale wakers on re-poll, unregisters on drop/completion, and drops wakers outside the mutex.
- **Guide identity & warning bounds** ([crates/iptv-library/src/guides.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-library/src/guides.rs#L67-L135), [crates/iptv-import/src/catalog.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-import/src/catalog.rs#L123-L185), [crates/iptv-import/src/epg.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-import/src/epg.rs#L111-L134), [crates/iptv-import/src/m3u.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-import/src/m3u.rs#L164-L218), [crates/iptv-import/src/stalker.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-import/src/stalker.rs#L349-L355)): Duplicate feeds/provider indices and `u32` identity exhaustion return `GuideFeedError` without renumbering; `reserved_through` high-water prevents ID reuse; M3U header guides and Stalker crawl warnings are bounded before validation/accumulation.
- **Private recording open & relay write deadlines** ([crates/iptv-net/src/file.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-net/src/file.rs#L72-L107), [crates/iptv-net/src/recording.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-net/src/recording.rs#L123-L145), [crates/iptv-net/src/recording_file.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-net/src/recording_file.rs#L37-L242), [crates/iptv-net/src/playback.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-net/src/playback.rs#L839-L850)): `open_private_regular` opens with `O_NOFOLLOW | O_NONBLOCK` (Unix) / `FILE_FLAG_OPEN_REPARSE_POINT` (Windows) and validates `is_file()` + length from the opened descriptor; `RecordingFile` reaps finished `JoinSet` tasks before admission and enforces a 15s `ProgressWriter` idle deadline across headers and body chunks.
- **MPV load identity tracking** ([crates/iptv-player-mpv/src/mpv/mod.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-player-mpv/src/mpv/mod.rs#L211-L216)): Replaces the `loads: u32` counter with `load: Option<LoadId>` and `load_pending: bool`, preventing superseded or stopped `Loading`/`LoadRejected`/`GuardFailed` replies from affecting a newer load.

### Scope & Limitations
- **Packet-only review**: Inspected only the supplied diff text with no tools used.
- **Not inspected or verified**: Surrounding repository source files, callers outside the diff, `cargo build`, `cargo test`, `wasm32-unknown-unknown` builds, and `iptv-nav` were not inspected or run.

## Packet 3: unmodified review output

No concrete findings or regressions identified in the supplied diff packet.

**Scope and limitations:**
- **Packet-only review:** Only the provided diff hunks across `iptv-player-mpv`, `iptv-providers`, and `iptv-runtime` were inspected.
- **Not inspected:** Full source files outside the diff, unpatched callers/callees, builds, and test execution were not run or verified.

## Packet 4: unmodified review output

**No findings** in the supplied diff packet.

Across the reviewed changes—`parental.rs` (`policy_revision` vs. grant `epoch` fencing and `inspection_*` / `native_allowed` checks), `recordings.rs` and `recordings/rules.rs` (`Admission` checks in accepting writers and post-await playback capability fencing), `vod.rs` (post-read `services.is_current()` and item/parent `parental_allowed` checks), and `search.rs` / `iptv-search` (`ChannelId` incarnation binding, `resolve_search_channel`, `ProgrammeIndex::assemble` / `search_programmes` overflow/range handling, and synthetic-fixture tests)—no concrete correctness, security, grant, cancellation, SQL snapshot, or source-identity regressions were identified within the diff hunks shown.

### Scope & Limitations
- **Packet-only review:** Only the diff hunks provided in the prompt were inspected; no tools were used.
- **Not inspected or validated:** Surrounding source files, unpatched callers/implementations (such as `iptv-store-sqlite` checked methods and `iptv-nav` / UI callers), compilation, and test execution were **not** inspected or run.

## Packet 5: unmodified review output

### Findings

1. **[Medium] `publish_catalog_stage_with_provider_guides` overwrites existing guide rows when `initial` is `Some`** — [catalog_stage.rs:355-391](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-store-sqlite/src/catalog_stage.rs#L355-L391), [guide_sources.rs:151-157](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-store-sqlite/src/guide_sources.rs#L151-L157) (introduced in this diff)
   - **Failure scenario:** `publish_catalog_stage_with_provider_guides` documents that "`initial` is used only for a newly created source; existing sources read their current rows so concurrent edits and durable historical reservations survive." However, `publish_stage` calls `sources::ensure_source(&tx, source)?` unconditionally and forwards `initial` to `plan_provider_guides_in(&tx, source, urls, guides, now)`. Inside `plan_provider_guides_in`, `match initial { Some(sources) => sources.to_vec(), None => guide_sources_in(conn, source)? }` never reads the database when `initial` is `Some(_)`. Any caller passing `initial: Some(...)` for an existing source silently clobbers existing `guide_sources` rows and any concurrent user edits. (The new test `final_catalog_publication_replans_after_an_intervening_guide_edit` in `guide_sources.rs` only exercises `initial = None`.)
   - **Smallest fix:** In `publish_stage` before `sources::ensure_source(&tx, source)?`, query whether `source` already exists in `sources` (e.g., `let is_new: bool = tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM sources WHERE id=?1)", [source.0], |r| r.get(0))?;`), and pass `guides.filter(|_| is_new)` as `initial` to `plan_provider_guides_in`.

### Scope Note
This review is strictly limited to the supplied diff packet. Full source files, unpatched callers/callees, builds, and test suites were **not** inspected or executed.

## Packet 6: unmodified review output

No findings in the supplied diff packet.

### Scope & Limitations
- **Packet-only review:** Only the provided diff hunks in [recordings/tests.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-store-sqlite/src/recordings/tests.rs), [vod.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-store-sqlite/src/vod.rs), [concurrency.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-store-sqlite/tests/it/concurrency.rs), [epg.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-store-sqlite/tests/it/epg.rs), and [migrations.rs](file:///home/blackmyth/src/patina-rust-review/patina-iptv/crates/iptv-store-sqlite/tests/it/migrations.rs) were inspected.
- **Not inspected / not run:** Underlying implementations referenced by the diff (such as `Store::read_snapshot`, the `*_checked` recording methods, `for_each_epg_title`, and `schema_migrations` definitions), full repository files, builds, and test suites were not inspected or executed.
