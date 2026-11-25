use iptv_library::{FeedResult, GuideSource};

use super::*;
use iptv_library::guides;

const P: SourceId = SourceId(7);
const USER: Feed = guides::FIRST_USER_FEED;
const PROVIDER: Feed = Feed(2);

fn url(s: &str) -> SecretUrl {
    SecretUrl::new(format!("http://{s}.example.test/guide.xml"))
}

fn sources() -> Vec<GuideSource> {
    guides::sync_provider(
        &[guides::user_source(USER, url("mine"), None)],
        &[SecretUrl::new(
            "http://panel.example.test/xmltv.php?password=p",
        )],
    )
    .unwrap()
}

fn lines(sources: &[GuideSource]) -> Vec<SourceLine> {
    sources
        .iter()
        .map(|s| SourceLine {
            source: s.clone(),
            result: Some(FeedResult {
                feed: s.feed,
                attempted_at: 0,
                error: None,
                channel_count: 1,
                program_count: 2,
                listed_count: 3,
            }),
        })
        .collect()
}

fn screen() -> Screen {
    Screen::new(P, lines(&sources()))
}

/// What the app does with an edit: the store's new lines.
fn applied(s: &mut Screen, effect: Option<Effect>) {
    let Some(Effect::Edit(edit)) = effect else {
        panic!("no edit: {effect:?}");
    };
    let now: Vec<GuideSource> = s.lines.iter().map(|l| l.source.clone()).collect();
    s.set_lines(lines(&guides::apply(&now, &edit).unwrap().unwrap()));
}

#[test]
fn the_list_moves_by_identity_and_exposes_update_and_inspection() {
    let mut s = screen();
    assert_eq!(s.focus(), Item::Source(USER));
    assert_eq!(
        s.items(),
        [
            Item::Source(USER),
            Item::Source(PROVIDER),
            Item::Add,
            Item::Update,
            Item::Inspect,
            Item::Clear
        ]
    );
    s.key(Key::Down);
    s.key(Key::Down);
    s.key(Key::Down);
    assert_eq!(s.key(Key::Ok), Some(Effect::Update));
    s.key(Key::Down);
    assert_eq!(s.key(Key::Ok), Some(Effect::Inspect));
    s.key(Key::Down);
    assert_eq!(s.focus(), Item::Clear, "no wrap");
    assert_eq!(s.key(Key::Back), Some(Effect::Close));
    assert_eq!(Screen::new(P, vec![]).focus(), Item::Add);
}

#[test]
fn moving_a_source_keeps_it_focused() {
    let mut s = screen();
    s.key(Key::Down);
    s.key(Key::Ok);
    assert_eq!(
        s.layer(),
        Layer::Actions {
            feed: PROVIDER,
            focus: Act::Switch
        }
    );
    assert_eq!(
        s.acts(PROVIDER),
        [Act::Switch, Act::Up, Act::Down, Act::Shift, Act::Rename],
        "the provider's guide is switched off, not removed"
    );
    s.key(Key::Down);
    let up = s.key(Key::Ok);
    assert_eq!(up, Some(Effect::Edit(Edit::Move(PROVIDER, -1))));
    applied(&mut s, up);
    assert_eq!(s.items()[0], Item::Source(PROVIDER));
    assert_eq!(
        s.layer(),
        Layer::Actions {
            feed: PROVIDER,
            focus: Act::Up
        }
    );
    s.key(Key::Back);
    assert_eq!(
        (s.layer(), s.focus()),
        (Layer::List, Item::Source(PROVIDER))
    );
}

#[test]
fn switch_and_time_shift() {
    let mut s = screen();
    s.key(Key::Ok);
    let off = s.key(Key::Ok);
    assert_eq!(off, Some(Effect::Edit(Edit::SetEnabled(USER, false))));
    applied(&mut s, off);
    assert!(!s.line(USER).unwrap().source.enabled);
    assert_eq!(
        s.act(USER, Act::Switch),
        Some(Effect::Edit(Edit::SetEnabled(USER, true)))
    );
    // Shift: Left/Right step it; OK steps it later.
    s.act(USER, Act::Shift);
    s.set_lines(lines(&sources()));
    assert_eq!(
        s.key(Key::Left),
        Some(Effect::Edit(Edit::Shift(USER, -SHIFT_STEP)))
    );
    assert_eq!(
        s.key(Key::Right),
        Some(Effect::Edit(Edit::Shift(USER, SHIFT_STEP)))
    );
    // Elsewhere Left/Right do nothing.
    s.key(Key::Up);
    assert_eq!(s.key(Key::Left), None);
    let mut far = sources();
    far[0].shift_minutes = MAX_SHIFT_MINUTES;
    s.set_lines(lines(&far));
    assert_eq!(s.shift(USER, 1), None, "at the end of the range");
}

#[test]
fn adding_asks_for_a_valid_new_link_then_a_name() {
    let mut s = screen();
    s.tap(Item::Add);
    assert_eq!(s.editing().map(|e| e.kind.clone()), Some(EditKind::AddLink));
    s.set_text("guide.example.test/epg.xml");
    assert_eq!(s.key(Key::Ok), None);
    assert_eq!(s.notice(), Some(Notice::Link(FieldError::MissingScheme)));
    assert_eq!(s.editing(), None);
    // The next try starts from the refused text.
    assert_eq!(s.key(Key::Ok), None);
    assert_eq!(
        s.editing().map(|e| e.text.as_str()),
        Some("guide.example.test/epg.xml")
    );
    s.set_text("http://mine.example.test/guide.xml");
    assert_eq!(s.key(Key::Back), None);
    assert_eq!(s.notice(), Some(Notice::Duplicate));
    s.start_add();
    s.set_text(" http://second.example.test/guide.xml ");
    assert_eq!(s.key(Key::Ok), None);
    assert!(matches!(
        s.editing().map(|e| &e.kind),
        Some(EditKind::AddName { .. })
    ));
    s.set_text("Second");
    assert_eq!(
        s.key(Key::Ok),
        Some(Effect::Edit(Edit::Add {
            url: url("second"),
            name: Some("Second".into())
        }))
    );
    // A name may stay empty.
    s.start_add();
    s.set_text("http://third.example.test/guide.xml");
    s.commit();
    assert_eq!(
        s.key(Key::Back),
        Some(Effect::Edit(Edit::Add {
            url: url("third"),
            name: None
        }))
    );
}

#[test]
fn removing_asks_first_and_focus_moves_on() {
    let mut s = screen();
    assert_eq!(s.act(USER, Act::Remove), None);
    assert_eq!(
        s.layer(),
        Layer::Confirm {
            feed: USER,
            remove: false
        }
    );
    assert_eq!(s.key(Key::Ok), None, "keeping it is the default");
    s.act(USER, Act::Remove);
    s.key(Key::Down);
    let removed = s.key(Key::Ok);
    assert_eq!(removed, Some(Effect::Edit(Edit::Remove(USER))));
    applied(&mut s, removed);
    assert_eq!(
        (s.layer(), s.focus()),
        (Layer::List, Item::Source(PROVIDER))
    );
}

#[test]
fn renaming_and_a_new_link() {
    let mut s = screen();
    s.act(PROVIDER, Act::Rename);
    s.set_text(" Provider ");
    assert_eq!(
        s.commit(),
        Some(Effect::Edit(Edit::Rename(
            PROVIDER,
            Some("Provider".into())
        )))
    );
    s.act(USER, Act::Rename);
    assert_eq!(s.commit(), None, "unchanged");
    // A plain link is shown for editing; left as it is, nothing changes.
    s.act(USER, Act::Relink);
    assert_eq!(
        s.editing().map(|e| e.text.as_str()),
        Some("http://mine.example.test/guide.xml")
    );
    assert_eq!(s.commit(), None);
    s.act(USER, Act::Relink);
    s.set_text("http://other.example.test/guide.xml");
    assert_eq!(
        s.commit(),
        Some(Effect::Edit(Edit::Relink(USER, url("other"))))
    );
    // A link with an account in it starts empty; left empty, it stays.
    let mut secret = sources();
    secret[0].url = SecretUrl::new("http://mine.example.test/g.xml?token=t");
    s.set_lines(lines(&secret));
    s.act(USER, Act::Relink);
    assert_eq!(s.editing().map(|e| e.text.as_str()), Some(""));
    assert_eq!(s.commit(), None);
}

#[test]
fn a_source_removed_elsewhere_closes_its_sheet() {
    let mut s = screen();
    s.act(USER, Act::Shift);
    s.set_lines(lines(&sources()[1..]));
    assert_eq!(
        (s.layer(), s.focus()),
        (Layer::List, Item::Source(PROVIDER))
    );
    s.set_lines(vec![]);
    assert_eq!(s.focus(), Item::Add);
    assert_eq!(s.dismiss(), None);
}

#[test]
fn without_downloads_no_guide_is_added_or_updated() {
    let mut s = screen().without_downloads();
    assert!(!s.items().contains(&Item::Add));
    assert!(!s.items().contains(&Item::Update));
    assert!(s.items().contains(&Item::Inspect));
    s.start_add();
    assert_ne!(s.focus(), Item::Add);
    let empty = Screen::new(P, Vec::new()).without_downloads();
    assert_eq!(empty.focus(), Item::Inspect);
}

#[test]
fn clear_requires_explicit_confirmation_and_keeps_playlist_identity() {
    let mut s = screen();
    assert_eq!(s.tap(Item::Clear), None);
    assert_eq!(s.layer(), Layer::Clear { confirm: false });
    assert_eq!(s.key(Key::Ok), None);
    s.tap(Item::Clear);
    s.key(Key::Down);
    assert_eq!(s.dismiss(), None);
    assert_eq!(s.layer(), Layer::List);
    s.tap(Item::Clear);
    s.key(Key::Down);
    assert_eq!(s.key(Key::Ok), Some(Effect::Clear));
    assert_eq!(s.playlist(), P);
    assert!(!screen().without_downloads().items().contains(&Item::Clear));
}

#[test]
fn failed_enabled_sources_offer_retry_beside_the_source_to_change() {
    let mut s = screen();
    s.lines[0].result.as_mut().unwrap().error = Some(iptv_library::ImportErrorKind::Network);
    assert!(s.acts(USER).contains(&Act::Retry));
    assert_eq!(s.act(USER, Act::Retry), Some(Effect::Update));
    s.lines[0].source.enabled = false;
    assert!(!s.acts(USER).contains(&Act::Retry));
}

#[test]
fn retry_focus_follows_the_offered_actions_after_success_or_disable() {
    let mut s = screen();
    s.lines[0].result.as_mut().unwrap().error = Some(iptv_library::ImportErrorKind::Network);
    s.tap(Item::Source(USER));
    assert_eq!(
        s.layer(),
        Layer::Actions {
            feed: USER,
            focus: Act::Retry
        }
    );
    let mut updated = s.lines.clone();
    updated[0].result.as_mut().unwrap().error = None;
    s.set_lines(updated);
    assert_eq!(
        s.layer(),
        Layer::Actions {
            feed: USER,
            focus: Act::Switch
        }
    );
    assert_eq!(s.act(USER, Act::Retry), None);
}
