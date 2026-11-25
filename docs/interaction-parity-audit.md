# Guide input consistency audit

This audit compares TV remote actions, tablet touch and desktop pointer input.
It follows normal handlers and the existing synthetic application harness;
matching labels alone do not establish matching targets or side effects.

| Operation | Shared behavior and deliberate differences | Evidence |
| --- | --- | --- |
| Guide timeline context | Resolved Menu/Long OK, touch hold and right-click open the same compact channel actions. The focused channel and time block are captured, not the playing channel. | `input.rs`, `grid/cell.rs`, `menu_view::open_timeline_context`; `tv_remote_journey` |
| Channel header context | Hold/right-click captures the row while retaining the guide time. It uses the same compact action builder. | `grid/channel.rs`, `menu_view::open_at` |
| Player channel-list context | Menu, Long OK, row hold/right-click and the row options button use the same focused-row action. Picking a multiview channel blocks context actions. | `channels_view::key`, `open_channel_menu`; `tv_remote_journey`, `tv_shared_controls` |
| Advanced program actions | Compact **More options** opens the existing full menu for the captured channel, program and list. Back restores the compact selection; Menu dismisses the context. Live replay and management remain reachable by D-pad. The program-header gesture is an explicit full-menu entry. | `menu_view::perform`, `back`, `close`; `tv_remote_journey`, `tv_replay` |
| Context and autoplay | Opening timeline or channel-row context holds automatic browsing playback. Closing that context does not unexpectedly tune the browsed row. A fresh explicit choice still plays. | `browse_playback::hold`; `personalization_playback` |
| Primary guide activation | Remote Select, a cell tap and a desktop click converge on `Ui::watch`: new live selection, return to the existing live player, archive replay, or details according to the selected program. A channel-name activation intentionally targets now rather than an arbitrary timeline block. | `lists::ok`, `grid/cell.rs`, `grid/channel.rs`, `Ui::watch` |
| Groups and rail | Named-group context actions share the group-options handler. The documented single-playlist All channels shortcut retains Menu/M to add a playlist; hold/right-click offers list options. Desktop group activation enters the group; touch can first preview a pane. Rail choices converge on portable browse state and `lists::open`. | `panels.rs`, `lists.rs`, `iptv-presentation::browse` |
| Guide preview | Remote activation and preview touch return the existing player to full screen. Preview Menu opens that player's controls rather than acting on the guide row behind it. | `header.rs`, `lists::ok`, `input.rs` |
| Recordings and on-demand | Recording context gestures/options and Menu converge on `select_job`; activation converges on `activate_job`. On-demand poster selection and details controls share their selection/activation handlers. No alternate context-menu mutation path was found there. | `recordings_view`, `vod_view`, `vod_view/browser.rs` |
| Shared play-bar controls | The shared play bar’s pointer and remote controls converge on `play_bar::activate`, including favorites, recording and picture in picture. Platform support and available width determine which controls appear; those are capability/layout differences. | `play_bar.rs` |
| Return timeline to now | Ruler activation and the Home action use `Focus::jump_to_now`; a pointer pan additionally cancels its active fling. | `grid/pan.rs`, `grid/keys.rs` |

The fixes remove the timeline's device-type branch, the player-list physical
Menu exception, and the different post-channel-context autoplay behavior. They retain
custom remote assignments, short-OK selection, preview/full-screen routing,
group behavior, and the persistent player surface.

## Additional audit findings

- A raw-key regression reproduced a favorite toggle running twice for one
  held key: both Down and Repeat changed the favorite. Resolved favorite
  actions now ignore repeats, including custom short bindings; a fresh
  press and a configured long action still act once. Navigation and seek
  continue to repeat. This fixes an operation-level inconsistency without
  suppressing every repeat globally.
- Archive selection in `Ui::watch` consults the group's external-player
  preference, while the existing **Play from the beginning** menu handler
  directly requests internal catch-up playback. This is a policy difference
  to investigate on a host with external playback, not a confirmed cause of
  the TV context-menu bug. A macOS headless pass cannot establish Android or
  Linux external-player handoff behavior.

Android acceptance uses a dedicated synthetic validation APK. Emulator and
physical-device evidence are recorded separately; see
[disposable Android validation](android-validation.md). This audit does not
claim physical-device or screen-reader acceptance from headless tests.
