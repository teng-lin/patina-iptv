use std::rc::Rc;

use crate::combined::tests::{NOW, part};
use crate::combined::{CombinedSchedule, SourceSchedule};
use crate::{EpgSchedule, GuideChannel};
use iptv_core::guide::{GroupName, Schedule};
use iptv_epg::{ChannelKey, GuideStore};

#[test]
fn collapsing_one_playlist_preserves_parent_channels_and_other_group_identity() {
    use iptv_core::lists::{ChannelList, Entry, ListId, group_of};
    let all = CombinedSchedule::new(vec![part(1, "Alpha", NOW), part(2, "Beta", NOW)], NOW);
    let beta_group = ListId::of(Entry::Group(3).index(), &all);
    let beta_channel = all.channel_key(1).unwrap();
    assert_eq!(all.all_channels_label(), GroupName::AllPlaylists);
    assert_eq!(group_of(&all, 0), Some(1));
    assert!(all.toggle_playlist(1).is_none());
    let collapsed = all.toggle_playlist(0).unwrap();
    let folded = CombinedSchedule::new(vec![part(1, "Renamed", NOW), part(2, "Beta", NOW)], NOW)
        .with_collapsed_sources(collapsed);
    assert_eq!(folded.group_count(), 3);
    assert_eq!(folded.group_key(0), "playlist:1");
    assert!(folded.group_is_collapsed(0));
    assert_eq!(folded.group_label(0), GroupName::Named("Renamed".into()));
    assert_eq!(group_of(&folded, 0), Some(0));
    assert_eq!(beta_group.find(&folded), Entry::Group(2).index());
    assert_eq!(folded.find_channel(&beta_channel), Some(1));
    assert_eq!(folded.channel_count(), 2);
    assert_eq!(
        ChannelList::build(Entry::Group(0).index(), &folded, &[], &[]).channels(),
        &[0]
    );
    assert!(folded.toggle_playlist(0).unwrap().is_empty());
}

#[test]
fn independent_browser_expansion_keeps_the_guides_selected_category_and_channel_indices() {
    use iptv_core::lists::{ChannelList, Entry, ListId};
    let guide = CombinedSchedule::new(vec![part(1, "Alpha", NOW), part(2, "Beta", NOW)], NOW)
        .with_collapsed_sources([2]);
    let selected = Entry::Group(1).index();
    let category = ListId::of(selected, &guide);
    let keys: Vec<_> = (0..guide.channel_count())
        .map(|c| guide.channel_key(c).unwrap())
        .collect();
    let picker = guide.with_browser_expansion(guide.toggle_playlist(0).unwrap());
    assert_eq!(picker.group_count(), 2);
    assert_eq!(picker.collapsed_sources().collect::<Vec<_>>(), [1, 2]);
    assert_eq!(guide.collapsed_sources().collect::<Vec<_>>(), [2]);
    assert_eq!(guide.group_count(), 3);
    assert_eq!(category.find(&guide), selected);
    assert_eq!(
        ChannelList::build(selected, &guide, &[], &[]).channels(),
        &[0]
    );

    // Expanding a previously folded projection restores its category rows.
    let expanded = picker.with_browser_expansion(picker.toggle_playlist(0).unwrap());
    assert_eq!(expanded.group_count(), 3);
    assert_eq!(category.find(&expanded), selected);
    assert_eq!(expanded.collapsed_sources().collect::<Vec<_>>(), [2]);
    for projection in [&picker, &expanded] {
        assert_eq!(projection.epoch_unix(), guide.epoch_unix());
        for (channel, key) in keys.iter().enumerate() {
            assert_eq!(projection.find_channel(key), Some(channel));
            let (original, local) = guide.channel_source(channel).unwrap();
            let (projected, projected_local) = projection.channel_source(channel).unwrap();
            assert_eq!((projected.id, projected_local), (original.id, local));
            assert!(Rc::ptr_eq(&projected.schedule, &original.schedule));
        }
    }
}

#[test]
fn collapsing_playlist_sections_does_not_change_category_zapping() {
    use iptv_core::playback::{group_range, zap_target};
    let source = |id, name: &str| {
        let channel = |n| GuideChannel {
            key: ChannelKey::new(format!("generated-{n}")),
            name: format!("{name} {n}"),
            number: n,
            catchup_days: 0,
        };
        SourceSchedule {
            id,
            name: name.into(),
            schedule: Rc::new(EpgSchedule::new(
                vec![
                    ("News".into(), vec![channel(1), channel(2)]),
                    ("Sport".into(), vec![channel(3)]),
                ],
                GuideStore::default(),
                NOW,
            )),
        }
    };
    let expanded = CombinedSchedule::new(vec![source(1, "Alpha"), source(2, "Beta")], NOW);
    let collapsed = CombinedSchedule::new(vec![source(1, "Alpha"), source(2, "Beta")], NOW)
        .with_collapsed_sources([1, 2]);
    assert_eq!(expanded.group_count(), 6);
    assert_eq!(collapsed.group_count(), 2);
    for schedule in [&expanded, &collapsed] {
        // Fallback zapping is used when the selected browser list does
        // not contain the playing channel. It must still wrap within
        // the owning category, even when that category's row is hidden.
        assert_eq!(zap_target(schedule, 1, 1), 0);
        assert_eq!(zap_target(schedule, 0, -1), 1);
        assert_eq!(zap_target(schedule, 4, 1), 3);
        assert_eq!(zap_target(schedule, 5, 1), 5);
        assert_eq!(group_range(schedule, 1), 0..2);
        assert_eq!(group_range(schedule, 4), 3..5);
    }
}
