//! Generated child schedules for combined-guide scenarios.

use std::rc::Rc;

use crate::{EpgSchedule, combined::SourceSchedule};
use iptv_epg::{
    ChannelKey, GuideStore, MatchIndex, Overrides, ParseOptions, PlaylistChannel, SourceBuilder,
    StoreOptions, parse_xmltv,
};
use iptv_providers::{SecretUrl, model::M3uSourceConfig};
pub const NOW: i64 = 1_800_000_000;
pub fn part(id: i64, name: &str, now: i64) -> SourceSchedule {
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 tvg-id=\"same\" group-title=\"News\" catchup=\"append\" catchup-days=\"3\" catchup-source=\"?utc={utc}\",Shared\nhttp://stream.example.test/shared\n",
    );
    let channel = &parsed.channels.items[0];
    let xml = format!(
        "<tv><channel id=\"same\"><display-name>Shared</display-name></channel>\
         <programme channel=\"same\" start=\"20270115080000 +0000\" \
         stop=\"20270115090000 +0000\"><title>{name} program</title>\
         <desc>{name} description</desc></programme></tv>"
    );
    let xml = parse_xmltv(xml.as_bytes(), ParseOptions::default()).unwrap();
    let mut builder = SourceBuilder::new(iptv_epg::FeedId(1), 0, StoreOptions::default(), now);
    for c in xml.channels {
        builder.push_channel(c);
    }
    for p in xml.programs {
        builder.push_program(p);
    }
    let feed = builder.finish();
    let mut index = MatchIndex::new();
    index.add_source(feed.id(), feed.priority(), feed.channels());
    let key = ChannelKey::new(channel.id.as_str());
    let matched = index.match_channels(
        [PlaylistChannel {
            key: &key,
            tvg_id: Some("same"),
            name: "Shared",
            tvg_name: None,
        }],
        &Overrides::default(),
    );
    let mut guide = GuideStore::new();
    guide.replace_source(feed);
    guide.set_matches(matched.matches);
    let context = crate::playback::PlaybackContext::m3u(&M3uSourceConfig {
        location: SecretUrl::new("http://playlist.example.test/list.m3u"),
        user_agent: Some(name.into()),
        referrer: None,
    });
    SourceSchedule {
        id,
        name: name.into(),
        schedule: Rc::new(
            EpgSchedule::from_catalog(&parsed.groups, &parsed.channels.items, guide, now)
                .with_playback(Some(context)),
        ),
    }
}
