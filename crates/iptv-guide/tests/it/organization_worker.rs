use iptv_core::guide::Schedule;
use iptv_epg::{ChannelKey, GuideStore};
use iptv_guide::{EpgSchedule, GuideChannel, Organization};

#[test]
fn organization_of_a_hundred_thousand_channels_is_prepared_on_a_worker() {
    let groups = (0..1_000)
        .map(|group| {
            (
                format!("Generated group {group}"),
                (0..100)
                    .map(|slot| {
                        let id = group * 100 + slot;
                        GuideChannel {
                            key: ChannelKey::new(format!("generated-{id}")),
                            name: format!("Generated channel {id}"),
                            number: id as u32,
                            catchup_days: 0,
                        }
                    })
                    .collect(),
            )
        })
        .collect();
    let original = EpgSchedule::new(groups, GuideStore::default(), 0);
    let input = original.organization_builder();
    let prepared = std::thread::spawn(move || {
        let organization = Organization {
            channels: (0..100_000)
                .rev()
                .map(|id| (format!("generated-{id}"), format!("Renamed {id}"), false))
                .collect(),
            groups: Vec::new(),
        };
        input.prepare(organization, Vec::new())
    })
    .join()
    .unwrap();
    let edited = original.with_prepared_organization(prepared);
    assert_eq!(original.channel_name(0), "Generated channel 0");
    assert_eq!(edited.channel_name(0), "Renamed 0");
    assert_eq!(edited.channel_count(), 100_000);
    assert_eq!(original.channel_key(99_999), edited.channel_key(99_999));
}
