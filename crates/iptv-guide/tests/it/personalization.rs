use iptv_core::{
    guide::Schedule,
    lists::{ChannelList, FIRST_GROUP, ListId},
};
use iptv_epg::GuideStore;
use iptv_guide::{EpgSchedule, Organization};
#[test]
fn display_groups_share_catalog_identity_and_preserve_membership_order() {
    let m = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"News\",A\nhttp://example.test/a\n#EXTINF:-1 group-title=\"News\",B\nhttp://example.test/b\n#EXTINF:-1 group-title=\"Sport\",C\nhttp://example.test/c\n",
    );
    let s = EpgSchedule::from_catalog(&m.groups, &m.channels.items, GuideStore::new(), 1700000000);
    let keys: Vec<_> = m
        .channels
        .items
        .iter()
        .map(|c| c.id.as_str().to_owned())
        .collect();
    let a = s.find_channel(&keys[0]).unwrap();
    let b = s.find_channel(&keys[1]).unwrap();
    let c = s.find_channel(&keys[2]).unwrap();
    let stream = s.stream_url(a);
    s.set_organization(Organization::default());
    let lists = vec![
        (
            "provider".into(),
            "Duplicate".into(),
            vec![keys[1].clone(), keys[0].clone()],
        ),
        (
            "custom:1".into(),
            "Duplicate".into(),
            vec![
                keys[2].clone(),
                keys[0].clone(),
                keys[2].clone(),
                "absent".into(),
            ],
        ),
        ("custom:2".into(), "Empty".into(), vec![]),
    ];
    s.set_group_lists(lists.clone());
    assert_eq!(s.group_count(), 3);
    assert_eq!(s.group_channels(0), [b, a]);
    assert_eq!(s.group_channels(1), [c, a]);
    assert!(s.group_channels(2).is_empty());
    let identity = ListId::of(FIRST_GROUP + 1, &s);
    let mut reversed = lists;
    reversed.reverse();
    s.set_group_lists(reversed);
    assert_eq!(identity.find(&s), FIRST_GROUP + 1);
    assert_eq!(s.channel_count(), 3);
    assert_eq!(s.stream_url(a), stream);
    assert_eq!(
        ChannelList::build(FIRST_GROUP + 1, &s, &[], &[]).channels(),
        [c, a]
    );
    s.set_group_lists(vec![]);
    assert_eq!(s.group_count(), 0);
    assert!(s.group_channels(99).is_empty());
    assert_eq!(identity.find(&s), iptv_core::lists::ALL);
}
