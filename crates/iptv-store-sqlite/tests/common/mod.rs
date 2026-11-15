//! Synthetic sources and catalogs. Hosts use reserved example domains.
#![allow(dead_code)]

use iptv_providers::model::{
    CatchUp, CatchUpKind, Channel, ContentKind, Group, M3uSourceConfig, MacAddress, Series, Source,
    SourceConfig, SourceKind, StalkerCredentials, StalkerLinkFlags, StreamLocator, VodItem,
    XtreamCredentials,
};
use iptv_providers::{ContentId, GroupId, Secret, SecretUrl};
use iptv_store_sqlite::{SourceId, Store};

pub const XT_PASSWORD: &str = "synthetic-pass-7f3a";
pub const XT_USER: &str = "synthetic-user-19";
pub const MAC: &str = "00:1A:79:AB:CD:EF";

pub fn xtream_source(name: &str) -> Source {
    Source {
        name: name.into(),
        config: SourceConfig::Xtream(XtreamCredentials {
            server_url: "http://panel.example.com:8080".into(),
            username: Secret::new(XT_USER),
            password: Secret::new(XT_PASSWORD),
            user_agent: Some("Synthetic/1.0".into()),
        }),
    }
}

pub fn m3u_source(name: &str) -> Source {
    Source {
        name: name.into(),
        config: SourceConfig::M3u(M3uSourceConfig {
            location: SecretUrl::new(format!(
                "http://lists.example.com/get.php?username={XT_USER}&password={XT_PASSWORD}"
            )),
            user_agent: None,
            referrer: Some("http://lists.example.com/".into()),
        }),
    }
}

pub fn stalker_source(name: &str) -> Source {
    Source {
        name: name.into(),
        config: SourceConfig::Stalker(StalkerCredentials {
            portal_url: "http://portal.example.org/c/".into(),
            mac: MacAddress::parse(MAC).unwrap(),
            serial: Some(Secret::new("SERIAL-SYNTH-001")),
            device_id: None,
            device_id2: None,
            signature: None,
            signature2: None,
            login: Some(Secret::new("synthetic-login")),
            password: Some(Secret::new(XT_PASSWORD)),
        }),
    }
}

pub fn group(i: usize) -> Group {
    Group {
        id: GroupId::from_m3u_title(&format!("Group {i}")),
        name: format!("Group {i}"),
        kind: ContentKind::Live,
        provider_id: None,
        adult: false,
    }
}

/// An Xtream-shaped M3U channel whose URL embeds the password; its id does
/// not depend on the password (iptv-providers rule 2).
pub fn m3u_channel(i: usize, groups: usize, password: &str) -> Channel {
    let url = format!(
        "http://tv.example.net:8080/live/{XT_USER}/{password}/{}.ts",
        1000 + i
    );
    let name = format!("Channel {i:05}");
    Channel {
        id: ContentId::from_m3u(&url, &name),
        provider_id: None,
        name,
        number: Some(i as u32 + 1),
        logo: Some(format!("http://img.example.net/{i}.png")),
        groups: vec![group(i % groups.max(1)).id],
        stream: StreamLocator::Url {
            url,
            headers: vec![("User-Agent".into(), "Synthetic/1.0".into())],
            properties: Vec::new(),
        },
        epg_id: Some(format!("ch{i}.example")),
        epg_name: None,
        epg_shift_minutes: None,
        catch_up: i.is_multiple_of(3).then_some(CatchUp {
            kind: CatchUpKind::Xtream,
            // Both catch-up fields round-trip through the store.
            days: if i % 6 == 3 { None } else { Some(3) },
            source: None,
            correction_secs: if i.is_multiple_of(2) { 0 } else { 3600 },
        }),
        radio: false,
    }
}

pub fn xtream_channel(stream_id: u64, name: &str, group: &GroupId) -> Channel {
    Channel {
        id: ContentId::from_provider(
            SourceKind::Xtream,
            ContentKind::Live,
            &stream_id.to_string(),
        ),
        provider_id: Some(stream_id.to_string()),
        name: name.into(),
        number: None,
        logo: None,
        groups: vec![group.clone()],
        stream: StreamLocator::Xtream {
            stream_id,
            container_extension: None,
        },
        epg_id: None,
        epg_name: None,
        epg_shift_minutes: None,
        catch_up: None,
        radio: false,
    }
}

pub struct Catalog {
    pub groups: Vec<Group>,
    pub channels: Vec<Channel>,
    pub vod: Vec<VodItem>,
    pub series: Vec<Series>,
}

pub fn catalog(channels: usize, groups: usize, password: &str) -> Catalog {
    let vod_group = Group {
        id: GroupId::from_provider(SourceKind::Xtream, ContentKind::Movie, "5"),
        name: "Movies".into(),
        kind: ContentKind::Movie,
        provider_id: Some("5".into()),
        adult: false,
    };
    let vod = (0..3)
        .map(|i| VodItem {
            id: ContentId::from_provider(SourceKind::Xtream, ContentKind::Movie, &i.to_string()),
            provider_id: Some(i.to_string()),
            name: format!("Movie {i}"),
            groups: vec![vod_group.id.clone()],
            poster: None,
            plot: Some("A synthetic plot.".into()),
            year: Some(2020 + i as u16),
            rating_x10: Some(70 + i as u16),
            added: Some(1_700_000_000 + i),
            duration_secs: Some(5400),
            stream: StreamLocator::Xtream {
                stream_id: i as u64,
                container_extension: Some("mkv".into()),
            },
            is_series: false,
            episode_numbers: Vec::new(),
        })
        .collect();
    let series = vec![Series {
        id: ContentId::from_provider(SourceKind::Xtream, ContentKind::Series, "77"),
        provider_id: Some("77".into()),
        name: "Synthetic Show".into(),
        groups: Vec::new(),
        poster: None,
        plot: None,
        year: Some(2021),
        rating_x10: None,
        last_modified: Some(1_700_000_500),
    }];
    let mut all_groups: Vec<Group> = (0..groups).map(group).collect();
    all_groups.push(vod_group);
    Catalog {
        groups: all_groups,
        channels: (0..channels)
            .map(|i| m3u_channel(i, groups, password))
            .collect(),
        vod,
        series,
    }
}

pub fn replace(store: &mut Store, source: SourceId, c: &Catalog) {
    store
        .replace_catalog(source, &c.groups, &c.channels, &c.vod, &c.series)
        .unwrap();
}

pub fn stalker_stream(cmd: &str) -> StreamLocator {
    StreamLocator::Stalker {
        cmd: cmd.into(),
        link: StalkerLinkFlags::default(),
    }
}
