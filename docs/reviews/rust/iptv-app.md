# Rust review of iptv-app

This is the NSH application shell, even though it has a library entry point for
native hosts and headless tests. Application simplicity rules apply; its local
futures, Rc ownership and signal handles are intentional runtime contracts.
The retained findings concern asynchronous screen ownership, image replies and
playback controls. Source code and tests remain unchanged.

## Baseline and validation

- Reviewed revision: `0e197073a272eceb0631e4d53b08d2823950c416`, 2026-11-16.
- Rust edition 2024; workspace MSRV/toolchain 1.99.1; unpublished application.
- Coordinated workspace formatting and strict Clippy with both runtime/app
  test-support features passed. Parent reports app unit tests: 129 passed;
  app integration harness: 173 passed, 4 ignored. App build and 13 workspace
  doctests passed. Native-player tests have
  environment/decoder failures; these do not establish native playback success.
- `tools/check.sh` stops at NSH revision drift (actual
  `32769658ad34ad364b960a134cc73c754d50f2d9`, pinned
  `9ed782cbf362ab63ba18c47dc657fe944c405e50`). Core wasm target is absent.
  Android/iOS/macOS/Windows branches were read where listed below, not executed.
- Review probes use generated data outside the repository. No source fixes,
  test changes, Cargo changes or commits were made. Parent owns command logs
  under `/tmp/patina-rust-review-20261116/`.

## Must fix

No independent app security or soundness defect retained. Runtime and portable
library findings below still require coordinated fixes in their owning crates.

## Should fix

### iptv-app-001 A queued artwork cache hit drops its reply

**Locations:** `crates/iptv-app/src/content_image.rs:103`,
`crates/iptv-app/src/content_image.rs:157`.

**Rule:** Correctness and error handling (rust-review): every accepted asynchronous
operation must complete through the channel its consumer actually observes.

**Trigger and consequence:** Two recycled or simultaneous rows request the same
uncached logo while the four download permits are occupied. The first download
caches it. The second fetch acquires its permit, finds the cached image, and
returns `Some(value)` without sending `wanted`. `source` ignores the network
future return value and only observes the oneshot receiver, which instead gets
Canceled. That row remains on the placeholder until a later URL/effect change.

**Evidence:** The exact Cache/fetch implementation was extracted into the
synthetic `/tmp/patina-portable-review/app_image_reply.rs` probe with an unused
network stub. Prepopulating the cache, then polling fetch with a fresh reply,
produces `cached fetch returned image; receiver = Err(Canceled)`. This isolates
the branch; it is not a native screenshot or full concurrent-download test.

**Minimal proposed fix (report only):** Send on both successful paths.

```rust
if let Some(value) = cache().get(&url) {
    let _ = wanted.send(value.clone());
    return Some(value);
}
```

**Regression:** Occupy permits, enqueue two views for one generated URL, release
permits, and assert both receivers publish the image while the mock server sees
one download. Also exercise cancellation while queued.

### iptv-app-002 An old guide assignment closes a newly opened picker

**Location:** `crates/iptv-app/src/guide_assign_view.rs:159`.

**Rule:** Correctness of asynchronous ownership (rust-review); AGENTS.md state
and content identity rule. Results belong to the screen that accepted them.

**Trigger and consequence:** Choose a mapping for channel A while the writer is
busy, press Back, and open the picker for B. Back is explicitly accepted while
busy by `iptv-library/src/guide_assign.rs:248`. The old save completion always
calls `close(ui)`, closing B and incrementing REQUEST so B's in-flight load is
also discarded. The initial open-load path already checks REQUEST; the save
callback lacks that check. Runtime `data/assign.rs::choose` invokes the callback
after the save/show awaits even when the picker was closed.

**Minimal proposed fix (report only):** Use the existing monotonic request token
for a choice, which also invalidates an unfinished initial picker load.

```rust
let request = next_request();
assign::choose(&crate::runtime(), playlist, key, choice, move |applied| {
    crate::settings_view::notify(applied_words(applied));
    if REQUEST.with(|current| current.get()) == request {
        close(ui);
    }
});
```

**Regression:** Gate the writer, choose A, close/reopen B, then release A. B must
remain open and its own load must publish. Saving A still completes normally.
**Evidence/disposition:** Source-traced and independently reviewed by the native
reviewer; no new headless test or source modification was made.

### iptv-app-003 Repair results can match a different screen incarnation

**Locations:** `crates/iptv-app/src/guide_repair_view.rs:277`,
`crates/iptv-app/src/guide_repair_view.rs:288`,
`crates/iptv-library/src/guide_repair_screen.rs:58`.

**Rule:** Correctness of asynchronous ownership (rust-review); AGENTS.md state
identity. A request counter reset with a new screen cannot identify its owner.

**Trigger and consequence:** Load history for A (generation 1), then start its
status request (generation 2) and hold the reply. Close it, open history for B
(new generation 1), then start B's status request (generation 2). Both status
requests now have generation 2 because Screen::new resets the counter to zero.
The old status operation ignores the cancellation control returned by begin;
its completion only compares generation. It can populate the new screen's
outcome, operation and verification with the old commit, offering Verify/Undo
for that commit. This is stale UI ownership, not a claimed runtime authorization
bypass. Other repair callbacks share the generation-only check and need the
same ownership audit.

**Minimal proposed fix (report only):** In status, retain begin's existing
control and reject its canceled result before applying success or failure.
Every close or newer begin cancels that specific control, independently of a
replacement Screen's numeric generation.

```rust
let Some((screen, generation, control)) = begin(ui) else { return; };
// Keep existing id validation and owner capture.
crate::data::spawn_in_app(async move {
    let result = owner.guide_repair_operation(id).await;
    if control.cancel.is_cancelled() || !is_open(ui) {
        return;
    }
    match result {
        // Existing success/error handling, including generation checks.
        // ...
    }
});
```

Apply the same cancellation-ticket check before every repair callback mutates
state or calls finish; clone its cancel token before moving control into a
runtime request. A shell-owned monotonic instance ticket is an alternative.
Stale successful preview replies must release their plan before being dropped.

**Regression:** Hold old status completion, close/open history for another
source, complete its history load and issue its first status request, then release
the old completion, assert new state and
commit actions are unchanged. Repeat with preview, history and error replies.
**Evidence/disposition:** Source-traced and independently reviewed; no new
headless execution. The proposed ticket helpers are a design sketch, not an
existing API.

### iptv-app-004 Programme Details always displays fixture description text

**Location:** `crates/iptv-app/src/menu_view.rs:485`.

**Rule:** Correctness (rust-review); AGENTS.md views render application state.

**Trigger and consequence:** Import generated XMLTV with a description, then
open that programme's Details. The title/time use the selected programme, but
the description is an unconditional synthetic placeholder, even though
`header.rs:192` already reads the real EpgSchedule/CombinedSchedule description.
The same wrong prose is shown when no description exists.

**Minimal proposed fix (report only):** Share the header's description lookup,
including page invalidation, and render its returned description or empty state.

```rust
// Extract the header's CombinedSchedule/EpgSchedule lookup into this helper.
text(move || {
    ui.guide_data();
    program()
        .and_then(|(channel, programme)| schedule_description(channel, programme))
        .unwrap_or_default()
})
```

**Regression:** Details must show distinct generated descriptions for selected
programmes in single and combined schedules and an empty state when absent.
**Evidence/disposition:** Direct source proof; native reviewer confirmed the
mismatch independently. No artwork/reference content is proposed.

### iptv-app-005 Stop and sleep omit active playback variants

**Locations:** `crates/iptv-app/src/player_keys.rs:171`,
`crates/iptv-app/src/chooser.rs:275`,
`crates/iptv-app/src/video.rs:169`.

**Rule:** Correctness (rust-review): state transitions must handle every supported
playback variant, and one user action should use the common stop operation.

**Trigger and consequence:** A VOD movie/episode sets playing=None and recorded=None.
Stop/Long Back therefore returns early in stop_playback even while vod is Some.
Separately, a fired sleep timer clears only playing and changes mode. Recorded,
VOD and early-start playback remain admitted by video::source_prop, which checks
those variants explicitly, so their native source remains open after the timer.
The timer also skips the common flush, cancellation and publication cleanup.

**Minimal proposed fix (report only):** Admit all active variants to the common
stop path, and call it when sleep fires.

```rust
if ui.playing.get_untracked().is_none()
    && !multiview
    && !crate::on_demand::active(ui)
    && !crate::player::resume::playing(ui)
{
    return;
}
// Also clear ui.player.early in player::stop (close already clears publication).
// This prevents later source_ready/adopt from reviving a stopped early session.
// In start_sleep's fired branch, replace its partial state writes with:
crate::player_keys::stop_playback(ui);
```

**Regression:** Stop a movie and episode by remote Stop and Long Back; assert
source removal and progress persistence. Fire sleep with live, recorded, VOD and
early-start sessions; assert no source, no reconnect and a closed reservation.
**Evidence/disposition:** Native reviewer identified both paths; primary reviewer
checked on_demand::active, play_vod/play_recording, player::stop/close and the
video admission predicate. No native decoder run was added.

## Consider

- Challenge early resume against a VOD selection while its stored-channel read
  is pending: early's post-await checks include recorded/playing/wizard and
  browser_generation but omit vod. Also challenge tuning while early identity
  remains set before source_ready/adopt. These remain unconfirmed reachability
  candidates, not retained defects or automatic fix instructions.
- Input long-press cancellation on overlay/owner transitions was examined by the
  native reviewer; no concrete product sequence was retained. Coordinate with
  the separate iptv-nav review rather than making a speculative duplicate.

## What's already good

- Persistent surface zero is composed once; modes change geometry. Additional
  multiview/desktop players have explicit ownership, and PiP reuses the player.
- publish_source checks the current parental permission immediately before
  source publication. VOD also revalidates publication authority after async
  preparation. `tests/app/recordings_transport.rs:493` covers grant revocation
  during preparation. Do not duplicate the runtime consumer API gap as an app
  decoder bypass.
- Runtime-owner rebinding surrounds local task polls. The shell keeps framework
  Rc/local futures instead of adding unjustified Send bounds or extra runtimes.
- Credential retrieval, backup dialogs, platform storage and native FFI are
  isolated; Linux provider identifiers are absent from OS recording hooks.
- Artwork travels through the guarded network client as bytes/data URIs, with
  byte limits, active-download permits and stale-view publication checks.
- The binary test harness is disabled; app scenarios share one integration
  executable and reuse it for child-process isolation.

## Cross-crate disposition

- Search catalogue refresh preserves numeric result IDs; see iptv-search-001.
  App search_view/view.rs refreshes indexes and set_results on retained state,
  and selection uses the current numeric row. Fix core identity and adapters
  together; existing search history tests omit reorder-on-refresh.
- Guide page stale replies and error-as-empty completion are owned by the guide
  and runtime reports. App guide_pages tests cover successful load/evict/reload,
  not failure followed by retry.
- Portable credential Debug issues are owned by their leaf crates; app consumers
  must remain redacted when those values are nested in diagnostic state.

## Coverage and remaining review work

Checklist entries distinguish production reads from test-only audit. Platform
branches marked read were not platform-executed. Full line-by-line auditing of
all integration/unit tests remains pending; passing tests are separate evidence.
All 110 production source files received a production-path read across the
primary and supplemental reviewers. P denotes the primary reviewer; N denotes
the native supplemental reviewer. These checkmarks do not imply exhaustive
test-body or platform execution coverage.

### Source files

- [x] `src/android_transport.rs` — P: production read.
- [x] `src/backup.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/browse_playback.rs` — P: production read.
- [x] `src/capabilities.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/channels_view.rs` — N: production read.
- [x] `src/chooser.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/content_image.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/credential_key/android.rs` — P: production read.
- [x] `src/credential_key/linux.rs` — P: production read.
- [x] `src/credential_key.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/custom_groups.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/data/library.rs` — P: production read.
- [x] `src/data/pages.rs` — P: production read.
- [x] `src/data/stored.rs` — P: production read.
- [x] `src/data.rs` — P: production read.
- [x] `src/desktop/pip_window.rs` — N: production read.
- [x] `src/desktop/player_windows.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/desktop.rs` — N: production read; cfg branches read, host execution only.
- [x] `src/epg_sources_view/words.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/epg_sources_view.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/fav_editor.rs` — N: production read.
- [x] `src/favorites.rs` — N: production read.
- [x] `src/fonts.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/grid/cell.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/grid/channel.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/grid/keys.rs` — N: production read.
- [x] `src/grid/pan.rs` — N: production read.
- [x] `src/grid/ruler.rs` — N: production read.
- [x] `src/grid/wheel.rs` — N: production read; cfg branches read, host execution only.
- [x] `src/grid.rs` — N: production read; cfg branches read, host execution only.
- [x] `src/guide_assign_view.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/guide_inspect_view.rs` — N: production read.
- [x] `src/guide_repair_view.rs` — N: production read.
- [x] `src/guide_window_walkthrough.rs` — P: production read.
- [x] `src/header.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/i18n/pseudo.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/i18n.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/input/back.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/input/keys.rs` — N: production read.
- [x] `src/input.rs` — N: production read; cfg branches read, host execution only.
- [x] `src/lib.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/list_rows.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/lists.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/main.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/media.rs` — P: production read.
- [x] `src/menu_view.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/multiview/lifecycle.rs` — N: production read.
- [x] `src/multiview/pointer.rs` — N: production read.
- [x] `src/multiview.rs` — N: production read.
- [x] `src/multiview_picker.rs` — N: production read.
- [x] `src/multiview_view/chrome.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/multiview_view/menu.rs` — N: production read.
- [x] `src/multiview_view.rs` — N: production read.
- [x] `src/numbers.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/on_demand.rs` — P: production read.
- [x] `src/organization_view.rs` — N: production read.
- [x] `src/overlays.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/panels.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/parental_view.rs` — P: production read.
- [x] `src/performance_probe.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/pip.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/platform_data.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/play_bar/icons.rs` — N: production read.
- [x] `src/play_bar/progress.rs` — N: production read.
- [x] `src/play_bar.rs` — N: production read.
- [x] `src/player/recover.rs` — P: production read.
- [x] `src/player/resume.rs` — P: production read.
- [x] `src/player.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/player_keys.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/player_panels/card.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/player_panels/status.rs` — N: production read.
- [x] `src/player_panels/words.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/player_panels.rs` — N: production read.
- [x] `src/playlists_view/details_card.rs` — P: production read.
- [x] `src/playlists_view/words.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/playlists_view.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/rail.rs` — N: production read.
- [x] `src/recording_host/android.rs` — P: production read.
- [x] `src/recording_host/startup.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/recording_host/windows.rs` — P: production read.
- [x] `src/recording_host.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/recordings_view/draft.rs` — P: production read.
- [x] `src/recordings_view/rows.rs` — P: production read.
- [x] `src/recordings_view/style.rs` — P: production read.
- [x] `src/recordings_view.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/remote_editor.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/runtime_events.rs` — P: production read.
- [x] `src/scale.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/search_view/pick.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/search_view/programme_index.rs` — P: production read.
- [x] `src/search_view/view.rs` — P: production read.
- [x] `src/search_view.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/seek.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/settings.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/settings_view/editors.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/settings_view/remote_keys.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/settings_view/view.rs` — N: production read.
- [x] `src/settings_view/words.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/settings_view.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/startup.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/theme.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/time.rs` — N: production read; cfg branches read, host execution only.
- [x] `src/transport.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/ui_kit.rs` — N: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/video.rs` — P: production read; cfg branches read, host execution only.
- [x] `src/vod_view/browser.rs` — P: production read.
- [x] `src/vod_view.rs` — P: production read.
- [x] `src/wizard_view/entry.rs` — P: production read.
- [x] `src/wizard_view/words.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.
- [x] `src/wizard_view.rs` — P: production read; embedded tests inspected; exhaustive assertion audit pending; cfg branches read, host execution only.

### Targets, assets and tests

- [x] `Cargo.toml`, `build.rs`, `build_version.rs` — entry points, host/feature
  dependencies, Windows resource and git build metadata read.
- [x] `examples/guide_window_preview.rs`, `examples/library_preview.rs` — full
  source read, synthetic profiles/data and cfg fallbacks. Not launched.
- [x] `nsh.json`, `windows.rc`, fonts/icons inventory — manifest/resource
  references inspected; binary font/icon contents were not audited as Rust.
- [x] `tests/app/main.rs` — sole scenario harness inventory checked.
- [x] `tests/app/isolated.rs`, `backup_restore.rs` — same-executable child
  isolation and two-phase restore structure inspected by supplemental reader.
- [x] `tests/app/search_history.rs`, `guide_pages.rs`, `parental.rs`,
  `library_refresh.rs` — targeted bodies read by supplemental reader.
- [x] `tests/app/cov_settings_wizard.rs:1` through line 318,
  `recordings_transport.rs:493` through line 552,
  `vod_library.rs:1` through line 160 and line 1235 through 1292 — targeted
  validation, final permission and source authority assertions inspected.
- [ ] Remaining integration scenario bodies and unit-test-only tails —
  inventoried below; full line-by-line assertion audit pending. This is a
  coverage limitation, not a claim that baseline tests were unexecuted.
- [ ] `tests/native_player.rs` — target/host boundary inventoried; platform
  decoder assertions not independently reviewed or rerun by this reviewer.

Every test path is inventoried separately below. “Targeted read” means the
specific bodies above; “inventory only” means no exhaustive body review.

- `tests/app/accessibility_visibility.rs` — inventory only; assertion audit pending.
- `tests/app/architecture.rs` — inventory only; assertion audit pending.
- `tests/app/backup_restore.rs` — targeted read.
- `tests/app/browse_fixture.rs` — inventory only; assertion audit pending.
- `tests/app/catalogue_allowlist.txt` — inventory only; assertion audit pending.
- `tests/app/catalogue_lint.rs` — inventory only; assertion audit pending.
- `tests/app/cold_start.rs` — inventory only; assertion audit pending.
- `tests/app/combined_playlists.rs` — inventory only; assertion audit pending.
- `tests/app/combined_resume.rs` — inventory only; assertion audit pending.
- `tests/app/cov_data.rs` — inventory only; assertion audit pending.
- `tests/app/cov_data_player.rs` — inventory only; assertion audit pending.
- `tests/app/cov_mpv_pip_window.rs` — inventory only; assertion audit pending.
- `tests/app/cov_nav.rs` — inventory only; assertion audit pending.
- `tests/app/cov_nav_touch.rs` — inventory only; assertion audit pending.
- `tests/app/cov_settings_screen.rs` — inventory only; assertion audit pending.
- `tests/app/cov_settings_wizard.rs` — targeted read.
- `tests/app/cov_sources.rs` — inventory only; assertion audit pending.
- `tests/app/cov_sources_assign.rs` — inventory only; assertion audit pending.
- `tests/app/foundation_inspection.rs` — inventory only; assertion audit pending.
- `tests/app/foundation_repair.rs` — inventory only; assertion audit pending.
- `tests/app/guide_check_recovery.rs` — inventory only; assertion audit pending.
- `tests/app/guide_look.rs` — inventory only; assertion audit pending.
- `tests/app/guide_nav.rs` — inventory only; assertion audit pending.
- `tests/app/guide_pages.rs` — targeted read.
- `tests/app/guide_player_windows.rs` — inventory only; assertion audit pending.
- `tests/app/guide_scroll_cost.rs` — inventory only; assertion audit pending.
- `tests/app/headless.rs` — inventory only; assertion audit pending.
- `tests/app/interface_languages.rs` — inventory only; assertion audit pending.
- `tests/app/isolated.rs` — targeted read.
- `tests/app/library_cached_epg.rs` — inventory only; assertion audit pending.
- `tests/app/library_management.rs` — inventory only; assertion audit pending.
- `tests/app/library_refresh.rs` — targeted read.
- `tests/app/main.rs` — targeted read.
- `tests/app/multiview_menu.rs` — inventory only; assertion audit pending.
- `tests/app/parental.rs` — targeted read.
- `tests/app/performance.rs` — inventory only; assertion audit pending.
- `tests/app/personalization.rs` — inventory only; assertion audit pending.
- `tests/app/personalization_appearance.rs` — inventory only; assertion audit pending.
- `tests/app/personalization_groups.rs` — inventory only; assertion audit pending.
- `tests/app/personalization_order.rs` — inventory only; assertion audit pending.
- `tests/app/personalization_overlay.rs` — inventory only; assertion audit pending.
- `tests/app/personalization_playback.rs` — inventory only; assertion audit pending.
- `tests/app/personalization_selection.rs` — inventory only; assertion audit pending.
- `tests/app/personalization_zapping.rs` — inventory only; assertion audit pending.
- `tests/app/pip_control.rs` — inventory only; assertion audit pending.
- `tests/app/pointer.rs` — inventory only; assertion audit pending.
- `tests/app/recorder_startup.rs` — inventory only; assertion audit pending.
- `tests/app/recordings.rs` — inventory only; assertion audit pending.
- `tests/app/recordings_transport.rs` — targeted read.
- `tests/app/screens_look.rs` — inventory only; assertion audit pending.
- `tests/app/search_history.rs` — targeted read.
- `tests/app/stalker_cancel.rs` — inventory only; assertion audit pending.
- `tests/app/store_latency.rs` — inventory only; assertion audit pending.
- `tests/app/support.rs` — inventory only; assertion audit pending.
- `tests/app/tablet_rows.rs` — inventory only; assertion audit pending.
- `tests/app/tv_player.rs` — inventory only; assertion audit pending.
- `tests/app/tv_replay.rs` — inventory only; assertion audit pending.
- `tests/app/tv_shared_controls.rs` — inventory only; assertion audit pending.
- `tests/app/type_drift.rs` — inventory only; assertion audit pending.
- `tests/app/vod_library.rs` — targeted read.
- `tests/native_player.rs` — inventory only; assertion audit pending.
