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
Recheck the first-run screens as well as the guide's rail. The Exit key remains
unregistered and is handled by the system under either policy. A policy decision
does not replace the independent device checks.

CO-US-04 distinguishes an open popup/IME (Exit closes it) from an application
page (Exit closes the app). Patina must leave those transitions to the system;
do not infer that pressing Exit over every popup always terminates the app.

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

The follow-up in this change addresses two additional source-level defects:

- First-run welcome and library recovery without open details still registered
  an app Back listener, even though Back had no previous page. NSH then handled
  the gesture until release, preventing Android's system animation. These root
  screens now leave Back to the system; form steps and open recovery details
  retain their handlers. Tizen's explicit confirmation hook retains its handler.
- The NSH exit chooser previously accepted only Up/Down. Left now selects No
  and Right selects Yes, without changing the other choosers.

Existing app scenarios were extended for welcome/form transitions, recovery
details and repeated Left/Right selections. Each new regression failed on the
audited baseline. Their headless observations establish callback registration
and chooser behavior, not a physical predictive animation or TV result.

## Available hardware and blockers

Read-only inventory on 2026-11-25 UTC found:

- SkyWalker: no connected Android device and no registered `tab36`/`tv36` AVDs.
- HanSolo: `tab36` and `tv36` source profiles exist. An active Android emulator
  belongs to another session; it was not used by this audit.
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
| `tv36` | From the guide, Back reaches groups and rail; one further Back leaves to the launcher without a second press or app exit notice. Repeat one-press exit from first-run welcome and recovery with details closed. A form or open recovery details consumes the first Back, then its root hands the next Back to the system. | Historical rail result in #288; welcome/recovery not newly run in this audit. |
| Android phone, API 34+ | At each root (guide rail, first-run welcome, recovery with details closed), observe committed and canceled edge gestures: the predictive home preview shows, commit returns home, cancel keeps Patina on the same screen. Forms and recovery details still consume Back. A Back key event or launcher-at-end assertion alone is insufficient. | Pending unlocked phone session; no physical run. |
| Tizen bridge | On the guide's rail and first-launch setup, Return shows the localized question with No focused. Left/Right select the corresponding answer; Up/Down change selection; OK on No and Back dismiss without exiting; Yes exits. | Pending connected test TV and policy choice. |
| Tizen NSH host | On the guide's rail, first-run welcome and recovery with details closed, Return shows the question with No focused. Left/Right select No/Yes, Up/Down change selection, OK chooses, Back dismisses and Yes exits. Open recovery details consumes Return before the exit question. | Pending connected test TV and policy choice. |
| Both Tizen hosts, system Exit | From an application page, verify the system closes the app and returns to the TV channel screen. With a popup/IME open, CO-US-04 expects it to close first; record the actual firmware transition and report a mismatch rather than claiming unconditional app termination. Confirm the page does not register or consume Exit. | Pending connected test TV; no firmware result. |
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
