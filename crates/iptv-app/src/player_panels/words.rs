//! The player's words: waiting and failures (short, no addresses), the
//! action cards' labels and the keys' hints over the video.

use crate::i18n::{tr, trf};

use iptv_core::panels::{QuickAction, Shown};
use iptv_core::playback::{Cause, ErrorKind, Waiting};
use iptv_nav::Profile;

/// An action card's label (ours); the favourite's shows its state.
pub(crate) fn quick_label(action: QuickAction, favorite: bool, detour: bool) -> &'static str {
    match action {
        QuickAction::Guide => tr("TV guide"),
        QuickAction::History => tr("History"),
        QuickAction::Channels => tr("Channels"),
        QuickAction::Favorite if favorite => tr("★ Favourite"),
        QuickAction::Favorite => tr("☆ Favourite"),
        QuickAction::Multiview if detour => tr("Back to multiview"),
        QuickAction::Multiview => tr("Multiview"),
        QuickAction::Controls => tr("More controls"),
    }
}

/// What the keys do over the video now, for `profile`'s keys: over the
/// card (`card_focused`: one of its actions has the focus), the strip, or
/// a replay's card alone (`seek`: the `catchup.seek_keys` choice).
pub(crate) fn hint(
    profile: Profile,
    shown: Shown,
    card_focused: bool,
    seek: Option<&str>,
) -> &'static str {
    let remote = profile == Profile::Remote;
    match (shown, card_focused) {
        (Shown::Card, false) if remote => "◀ ▶ actions   OK hide   Hold OK more controls",
        (Shown::Card, false) => "◀ ▶ actions   Enter hide   M more controls",
        (Shown::Card, true) if remote => "◀ ▶ move   OK choose   Back hide",
        (Shown::Card, true) => "◀ ▶ move   Enter choose   Esc hide",
        (Shown::Controls, _) if remote => "◀ ▶ move   OK choose   Menu or Back hide",
        (Shown::Controls, _) => "◀ ▶ move   Enter choose   M or Esc hide",
        (Shown::Info, _) => match seek {
            Some("left_right") if remote => "◀ ▶ seek (hold: faster)   OK play or pause",
            Some("left_right") => "◀ ▶ seek   Enter play or pause",
            Some("down_up") if remote => "▼ ▲ seek (hold: faster)   OK play or pause",
            Some("down_up") => "▼ ▲ seek   Enter play or pause",
            Some(_) => "⏪ ⏩ seek",
            None => "",
        },
        _ => "",
    }
}

/// Our own wording for a failure.
pub(crate) fn failure_message(kind: &ErrorKind, catch_up: bool) -> String {
    let fixed = match kind {
        ErrorKind::Unavailable if catch_up => tr("This programme can't be replayed."),
        ErrorKind::Unavailable => tr("This channel has no stream address."),
        ErrorKind::Account => tr("The provider refused this account."),
        ErrorKind::Http(404 | 410) => tr("The provider doesn't have this stream."),
        _ if kind.cause() == Cause::Refused => return refused_message(),
        ErrorKind::Http(_) => tr("The provider answered with an error."),
        ErrorKind::Network => tr("Can't reach the provider. Check the connection."),
        ErrorKind::Format => tr("This stream's format isn't supported."),
        ErrorKind::Ended => tr("The stream ended."),
        ErrorKind::Stalled => tr("The stream stopped sending video."),
        ErrorKind::Resolve => tr("The portal didn't give a stream address."),
        ErrorKind::Other => tr("Playback stopped with an error."),
    };
    fixed.to_owned()
}

/// The provider refuses the stream for now: most often every stream the
/// account allows plays elsewhere.
fn refused_message() -> String {
    match crate::multiview::account_limit() {
        Some(n) => trf(
            "{count, plural, \
             one {Your provider allows # stream at once — another device may be using it.} \
             other {Your provider allows # streams at once — another device may be using one.}}",
            &[("count", n.into())],
        ),
        None => "The provider is refusing this stream for now.".into(),
    }
}

/// The waiting pill's words.
pub(crate) fn waiting_text(waiting: &Waiting) -> String {
    match waiting {
        Waiting::Loading => tr("Loading…").into(),
        Waiting::Buffering => tr("Buffering…").into(),
        Waiting::Reconnecting { .. } => tr("Reconnecting…").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_name_the_keys_that_act_now() {
        let r = Profile::Remote;
        assert!(hint(r, Shown::Card, false, None).contains("OK hide"));
        assert!(hint(r, Shown::Card, false, None).contains("Hold OK"));
        assert!(hint(r, Shown::Card, true, None).contains("OK choose"));
        assert!(hint(r, Shown::Controls, false, None).contains("Menu or Back"));
        let k = Profile::Keyboard;
        assert!(
            !hint(k, Shown::Card, false, None).contains("Hold"),
            "no long presses"
        );
        assert!(hint(k, Shown::Card, false, None).contains("M more controls"));
        // A replay's card alone names its seek keys; nothing over the banner.
        assert!(hint(r, Shown::Info, false, Some("left_right")).starts_with("◀ ▶ seek"));
        assert!(hint(r, Shown::Info, false, Some("down_up")).starts_with("▼ ▲ seek"));
        assert_eq!(hint(r, Shown::Info, false, None), "");
        assert_eq!(hint(r, Shown::Banner, false, None), "");
        assert_eq!(
            quick_label(QuickAction::Favorite, true, false),
            "★ Favourite"
        );
    }

    #[test]
    fn a_keyboards_replay_hints_and_the_detours_card() {
        let k = Profile::Keyboard;
        let info = |seek| hint(k, Shown::Info, false, Some(seek));
        assert_eq!(info("left_right"), "◀ ▶ seek   Enter play or pause");
        assert_eq!(info("down_up"), "▼ ▲ seek   Enter play or pause");
        assert_eq!(info("transport"), "⏪ ⏩ seek");
        assert_eq!(
            hint(Profile::Remote, Shown::Info, false, Some("down_up")),
            "▼ ▲ seek (hold: faster)   OK play or pause"
        );
        assert_eq!(
            quick_label(QuickAction::Multiview, false, true),
            "Back to multiview"
        );
        assert_eq!(
            quick_label(QuickAction::Multiview, false, false),
            "Multiview"
        );
    }

    #[test]
    fn every_failure_has_its_own_words_without_addresses() {
        let replay = failure_message(&ErrorKind::Unavailable, true);
        assert_eq!(replay, "This programme can't be replayed.");
        let cases = [
            (
                ErrorKind::Unavailable,
                "This channel has no stream address.",
            ),
            (ErrorKind::Account, "The provider refused this account."),
            (
                ErrorKind::Http(410),
                "The provider doesn't have this stream.",
            ),
            (ErrorKind::Http(500), "The provider answered with an error."),
            (ErrorKind::Format, "This stream's format isn't supported."),
            (ErrorKind::Ended, "The stream ended."),
            (ErrorKind::Stalled, "The stream stopped sending video."),
            (
                ErrorKind::Resolve,
                "The portal didn't give a stream address.",
            ),
            (ErrorKind::Other, "Playback stopped with an error."),
        ];
        for (kind, words) in cases {
            assert_eq!(failure_message(&kind, false), words, "{kind:?}");
        }
        assert_eq!(waiting_text(&Waiting::Loading), "Loading…");
        assert_eq!(waiting_text(&Waiting::Buffering), "Buffering…");
    }

    #[test]
    fn a_refusal_names_the_accounts_streams_when_known() {
        crate::multiview::set_account_streams(None);
        let refused = failure_message(&ErrorKind::Http(509), false);
        assert_eq!(refused, "The provider is refusing this stream for now.");
        crate::multiview::set_account_streams(Some(2));
        let full = failure_message(&ErrorKind::Http(403), false);
        assert!(
            full.starts_with("Your provider allows 2 streams at once"),
            "{full}"
        );
        crate::multiview::set_account_streams(Some(1));
        let one = failure_message(&ErrorKind::Http(429), false);
        assert!(
            one.starts_with("Your provider allows 1 stream at once"),
            "{one}"
        );
        assert_eq!(
            failure_message(&ErrorKind::Http(404), false),
            "The provider doesn't have this stream."
        );
        let network = failure_message(&ErrorKind::Network, false);
        assert!(!network.contains("://"), "no addresses");
        assert_eq!(
            waiting_text(&Waiting::Reconnecting { attempt: 9 }),
            "Reconnecting…"
        );
        crate::multiview::set_account_streams(None);
    }
}
