# Platform Back and exit acceptance

This is the acceptance ledger for [#277](https://github.com/teng-lin/patina-iptv/issues/277).
The implementation landed in [#288](https://github.com/teng-lin/patina-iptv/pull/288)
(Android and desktop) and [#295](https://github.com/teng-lin/patina-iptv/pull/295)
(both Tizen hosts). The issue remains open until the device checks below pass.
Headless tests and emulator results do not establish physical-device acceptance.

## Samsung policy decision

Rechecked the official pages on **2026-11-25 UTC**. They still conflict:

| Official source | Top-level Return policy |
| --- | --- |
| [Terminating Applications, Return Key Policy](https://developer.samsung.com/smarttv/develop/guides/fundamentals/terminating-applications.html) | Show an exit confirmation on the application home page; terminate only after Yes. Do not register the Return/Exit long press. |
| [Common certification checklist, CO-US-05](https://developer.samsung.com/smarttv/develop/development-checklist/common.html) | Return on the application home page closes the application. For 2016 and later models, the destination is the TV channel screen. The checklist links back to the termination guide. |

**Owner decision pending:** retain the termination-guide Yes/No popup, or adopt
the checklist's immediate close on top-level Return for certification. #295
implemented the popup requested in #277. That is the current behavior; it is
not a determination that Samsung certification accepts it. Do not change it
without recording the owner's choice and the applicable Samsung guidance.

If immediate close is chosen, both hosts must change together: the NSH host's
`iptv_app::confirm_leaving` hook in `hosts/tizen-nsh/src/lib.rs` and the bridge's
`leave` event/popup flow in `crates/iptv-web` and `hosts/tizen/app/main.js`.
Recheck the first-run welcome and recovery roots as well as the guide's rail. The Exit key remains
unregistered and is handled by the system under either policy. A policy decision
does not replace the independent device checks.

CO-US-04 also says Exit closes an open popup or IME. Its wording does not
exclude app-rendered popups. Patina's HTML/NSH exit question is part of the app,
not a system popup, and neither host registers Exit. App termination over that
question is therefore the expected **inference from the implementation**, not a
verified TV result or a claim that firmware dismisses the app's popup. This is
a potential CO-US-04 certification conflict requiring owner/Samsung
clarification. Test a system-owned IME separately. No Exit binding or window
policy changes here resolve either pending certification question.

## Evidence at the audited revision

The audit baseline is master `0c76709d45fb6a1b8c194023c8ff2ce362a3ed1a`.
The local checks below pass for production source revision `2cc73bf4` after the
root-Back and exit-direction fixes, with NSH pinned to
`413cfb65bf87a7bd9937f223fdc56896d8a718c8`. The documentation commit adds no
production change; the pull request records its exact reviewed head. Tests use
generated fixtures only.

The complete app harness at documentation head `e1f51e34` passed **329 tests,
0 failed, 4 intentionally ignored**, in 713.00 seconds with ordinary waits and
`IPTV_RS_TEST_JOBS=3`. Merge `cfb3fd35` brings in master `28b21c21` and preserves
both changelog entries; it changes no Back/exit production behavior. Later
review corrections below are documentation and comments, not new device proof.

Subsequent merge `9513c431` integrates master `45ea24a9`, including its More
chooser, device, scale and Android changes. At that integrated source, core
tests (37), presentation tests (465 unit, 53 integration, 2 doctests), both wasm
builds and the app build pass. The focused Back scenarios (5 overlay, 1 wizard,
1 recovery), `chooser::tests::every_chooser_and_picture_size_has_its_own_name`,
strict app/presentation Clippy and line/diff
checks also pass. Independent native review verifies that the More reconciliation
stays separate from the fixed No/Yes chooser. Native APK evidence from an earlier
source revision must not be treated as validation of this combined base.

### Check table: production revision `2cc73bf4`

The counts and commands in this table are the initial source revision only;
separate integration paragraphs record later reruns.

| Check | Result and limit |
| --- | --- |
| `cargo test -p iptv-core -p iptv-presentation` | Pass: 37 core tests; 463 presentation unit tests, 53 integration tests and 2 doctests. |
| `cargo build -p iptv-core -p iptv-presentation --target wasm32-unknown-unknown` | Pass. |
| `cargo build -p iptv-app` | Pass on macOS. The optional `rust-objcopy` step reports the existing missing toolchain `libLLVM.dylib` warning; it does not fail the build. |
| `cargo test -p iptv-web` | Pass: 61 tests, including rail Back emitting the localized exit question and the explicit first-launch `leave` command. This does not execute the TV's Worker or its package-file XHR. |
| `cargo test -p iptv-app --features test-support --test app personalization_overlay::` | Pass: 5 scenarios, including idle-rail system handoff, top-level confirmation and first-run welcome confirmation. This does not exercise Android's compositor or the Tizen page renderer. |
| `cargo test -p iptv-app --features test-support --test app cov_settings_wizard::the_wizard_and_the_choosers` | Pass: welcome delegates Back, the next form handles it, and returning to welcome delegates again. |
| `cargo test -p iptv-app --features test-support --test app cold_start::a_newer_database_shows_recovery_details_instead_of_the_first_run_wizard` | Pass: recovery delegates Back, opening details handles it, and closing details delegates again. |
| `cargo clippy -p iptv-app --all-targets --features test-support -- -D warnings`, line-length gate and `git diff --check` | Pass. |
| `cargo test -p iptv-app --features test-support --test app cov_settings_screen::menus_favorites_and_settings` | Pass: the normal guide-rail entry point leaves Back to the platform and offers neither removed exit setting. |
| `node --check` on `hosts/tizen/app/main.js`, `hosts/tizen/app/worker.js`, `hosts/tizen-nsh/app/patina-host.js` | Pass: JavaScript syntax only. |

[#288's emulator evidence](https://github.com/teng-lin/patina-iptv/pull/288)
records a disposable API 36 `tv36` returning to its launcher with one top-level
Back, and a disposable API 36 `tab36` gesture returning home with system animation
events. It predates this audit and the follow-up idle-overlay-rail fix in that
PR; it is not a new run or evidence from an Android phone. Keep any later runs
separate, with their own commit, artifact hash and operation manifest.

The final integration includes master
`fade0783421934c08ae1def1e5a0026b7a166a3a`, after #313 merged and #282
closed automatically. Both changelog entries are retained. The Back predicates
and fixed No/Yes selection are unchanged by that integration. Final-head checks
and APK operation manifests must name their own exact source revision; the
pre-integration device results below remain scoped to `cc494298`.

## Final integrated Android evidence at `984417e6`

At application `984417e683e56bd3e01679a244108dee2f0b8483`, the required core
and presentation tests (37 core; 468 presentation unit, 53 integration,
2 doctests), both wasm builds, app build, focused Back scenarios, 61 web tests,
strict Clippy, JavaScript syntax and line/target-budget checks pass.
The final APK SHA-256 is
`de34843f738bd44a00ef92bc0a323867b3ed12133082e3360f9ce95653adc15a`.
The exact NSH pin, CLI and runtime hashes were reverified after building it.
Later documentation/test-only review fixes do not change Android production
code; this artifact remains attributed to `984417e6`, not a later commit.

Both final API 36/Android 16 ARM64 owned copies passed their bounded root,
form and recovery-details checks with no application crash or refused player
load. The TV guest (`sdk_google_atv64_arm64`, port 5590) returned to its launcher
with one Back at the guide rail, welcome and recovery root. The tablet guest
(`sdk_gphone64_arm64`, `tab36`, port 5592) retained Patina on cancellation and
returned to Nexus Launcher on commit at all three roots. Visual review of all
six final recordings confirmed the rounded root window over the launcher,
including guide cancellation. This supersedes the earlier tablet's incomplete
visual result for emulator evidence only; it establishes no physical-phone or
Tizen acceptance.

The retained workspace root is `target/android-validation/skywalker-277/` in
this owned worktree. Final records are `patina277-final9844-tv/run-5ur3vdco` and
`patina277-final9844-tab/run-1oi33ohq`, alongside the APK, provenance JSON,
frozen helpers, checksums and a README index. The tablet's six MP4s are in its
`platform-back` directory; `final9844-review-frames` holds the reviewed frames.
These generated artifacts are retained outside Git for local review, and the
pull request records their source and result. Keep this evidence-bearing
worktree when cleaning up worktrees.

## Owned emulator follow-up on 2026-11-25 UTC

These runs used application `cc494298c8ff3cc0fed9d68699f273ae1a848c61`,
the pinned NSH revision above, and validation APK SHA-256
`f693d48b640d740eb9ad3f96d03eca16fa1329fc934538d68a9977975eb0aa0d`.
They predate the `9513c431` base integration. They do not certify a later APK.
The CLI reported `nsh 0.6.0 (413cfb6)`; its SHA-256 and the rebuilt Android
runtime hashes are retained in each operation manifest. A shared older CLI was
excluded from accepted artifacts.

Only fresh owned copies of `tv36`/`tab36` and the dedicated validation package
were changed. The source profiles, other sessions and ordinary viewer profiles
were untouched. Both copies were disposed after recording logs, XML and images.
These were focused Back journeys; they did not run the standard playback smoke
or establish either Tizen host's physical acceptance.

| Emulator journey | Observed result |
| --- | --- |
| API 36 `tv36`, owned port 5590 | Guide rail, first-run welcome and recovery with details closed each returned to the TV launcher with one Back press. A form and open recovery details consumed Back before returning to their roots. No application crash or refused player load. |
| API 36 `tab36`, owned port 5592 | Guest model `sdk_gphone64_arm64`, Android 16, ARM64, gesture navigation. Cancel retained Patina and commit returned to Nexus Launcher at all three roots. Forms and recovery details consumed Back. No application crash or refused player load. |
| Tablet visual inspection | Rounded Patina-over-launcher previews were observed for committed guide Back and for committed and canceled welcome/recovery Back. The guide cancellation recording showed the system Back arrow, with Patina retained, but did not establish the rounded home preview. All-root predictive animation acceptance is therefore still incomplete. |

Retained workspace evidence is under
`target/android-validation/skywalker-277/` in the owned Back worktree. The
historical TV record is `patina277-focused-tv/run-cua4xt11`; the historical
tablet record is `patina277-focused-tab-final/run-_b8wncn0`. Each has an
`operation.json`, native action log, application/crash logs, XML and screenshots;
the tablet run also retains six gesture MP4s, guest properties, navigation
settings and frozen Python/Java helpers with hashes. The app was alive at each
successful runner's final guard. The tablet's structural runner pass is separate
from the qualified visual result above.

Earlier tablet attempts remain failed setup/diagnostic records. Input controls
on the same fresh profile showed that the injector and standard display-0 edge
swipe could leave Android Settings. A normal Android full-screen education
prompt (`Got it`) and immersive transient bars had to be handled before the
observed Patina gesture. After dismissing that prompt and revealing the bars,
the welcome preview and committed return home were recorded. Those diagnostics
justify the input setup; they are not evidence of a new product defect, phone
acceptance, or a policy change to Patina's window behavior.

The follow-up in this change addresses two additional source-level defects:

- First-run welcome and library recovery without open details still registered
  an app Back listener, even though Back had no previous page. NSH then handled
  the gesture until release, preventing Android's system animation. These root
  screens now leave Back to the system; form steps and open recovery details
  retain their handlers. Tizen's explicit confirmation hook retains its handler.
- The NSH exit chooser previously accepted only Up/Down. Left now selects No
  and Right selects Yes, without changing the other choosers.

Existing app scenarios were extended for welcome/form transitions, recovery
details and repeated Left/Right selections. The initial new regression assertions failed on the
audited baseline. Additional existing-harness scenarios cover Left/Right
remaining inert in an ordinary chooser and the Tizen hook confirming recovery
only after details close. Their headless observations establish callback registration
and chooser behavior, not a physical predictive animation or TV result.

## Available hardware and blockers

Read-only inventory on 2026-11-25 UTC found:

- SkyWalker: no connected Android device and no registered `tab36`/`tv36` AVDs.
- HanSolo: `tab36` and `tv36` source profiles exist. An active Android emulator
  belongs to another session; it was not used by this audit. Later focused runs
  used separate owned copies, as recorded above.
- Gigabyte: a OnePlus **CPH2583**, Android **16/API 36**, gesture navigation
  (`navigation_mode=2`), and a Samsung **SM-X210** tablet. The phone was
  dozing with keyguard showing, and had no dedicated Patina validation package.
  It was not woken, unlocked, installed to or otherwise changed. Physical phone
  acceptance needs an unlocked dedicated test session.
- Gigabyte has Tizen Studio's SDB, but `sdb devices` lists no connected TV.
  Neither Tizen host has new physical-TV evidence. A connected, authorized test
  TV is required, including a Tizen 5.5 set for the bridge Worker check. No LAN
  scan, new pairing or change to a TV was performed.

## Remaining device acceptance

Use only synthetic playlists/EPG and a dedicated test application/profile. Never
clear or reuse a household viewer profile. Follow [Android validation](android-validation.md)
for the validation package and owned emulator copies. Record physical hardware
and emulators in separate evidence records.

| Target | Required observations | Status |
| --- | --- | --- |
| `tv36` | From the guide, Back reaches groups and rail; one further Back leaves to the launcher without a second press or app exit notice. Repeat one-press exit from first-run welcome and recovery with details closed. A form or open recovery details consumes the first Back, then its root hands the next Back to the system. | Final integrated `984417e6` owned TV run passes all three roots and form/details checks; exact artifact and retained record above. |
| Android phone, API 34+ | At each root (guide rail, first-run welcome, recovery with details closed), observe committed and canceled edge gestures: the predictive home preview shows, commit returns home, cancel keeps Patina on the same screen. Forms and recovery details still consume Back. A Back key event or launcher-at-end assertion alone is insufficient. | Pending unlocked phone session; no physical run. |
| Tizen bridge | On the guide's rail and first-launch setup, Return shows the localized question with No focused. Left/Right select the corresponding answer; Up/Down change selection; OK on No and Back dismiss without exiting; Yes exits. | Pending connected test TV and policy choice. |
| Tizen NSH host | On the guide's rail, first-run welcome and recovery with details closed, Return shows the question with No focused. Left or Up selects No; Right or Down selects Yes, with no wrapping. OK chooses, Back dismisses and Yes exits. Open recovery details consumes Return before the exit question. | Pending connected test TV and policy choice. |
| Both Tizen hosts, Exit on an app page or app-rendered popup | Confirm Exit remains unregistered. App-page termination is expected; termination over Patina's own popup is an implementation inference to verify. CO-US-04 has no stated app-popup exclusion, so this may conflict with its popup-closing rule. | Pending TV observation and owner/Samsung clarification of CO-US-04. |
| Both Tizen hosts, Exit with system IME/popup | Open the TV's own keyboard or popup and record whether Exit dismisses it before app termination, as CO-US-04 requires. This is separate from Patina's rendered exit chooser. | Pending connected test TV; no firmware result. |
| Both Tizen hosts, held Return/Exit | Hold the physical remote's Return/Exit key until the firmware's forced termination. It must terminate through the system without requiring Yes. Exercise an application page and an open confirmation; record any app action before the firmware takes over. | Pending connected test TV; no firmware result. |
| Tizen 5.5 bridge localization | Start in a non-English TV language; confirm the Worker's synchronous XHR loads packaged `i18n/<tag>.tsv` and the popup's title/answers use that language. Inspect the packaged app/Worker on the TV, not a desktop mock. | Pending Tizen 5.5 hardware. |

The Android animation setup follows the current
[official predictive Back guide](https://developer.android.com/guide/navigation/custom-back/predictive-back-gesture),
rechecked on 2026-11-25 UTC: system root animations require the app's callbacks
to be disabled; Android 13/14 exposes the developer animation option, while
Android 15+ enables those animations by default for opted-in apps.

The termination guide explicitly reserves the Return/Exit long press for
firmware-forced termination. Handling short Return events or observing a local
Long Back action does not establish whether the system's long press remains
intact. No new long-press binding is added here; verify the physical-key path on
each host before closing this acceptance item.

For each run, retain date, application commit, NSH revision, APK/WGT SHA-256,
device model, OS/firmware, navigation/input mode, configured language, generated
fixture identity, exact steps and observations, and screenshot/video/log paths.
Capture the phone animation as a recording that can be visually inspected.
Record any changed test settings and restore them. Do not publish device pairing
details, certificates, credentials or household content.

The pinned NSH Tizen renderer does not carry the app's accessibility metadata
into its page DOM, as recorded in #295. Physical popup operability and Voice
Guide announcements are separate checks; do not describe headless button roles
as verified spoken output. A renderer fix belongs in `nsh-platform-tizen`.
