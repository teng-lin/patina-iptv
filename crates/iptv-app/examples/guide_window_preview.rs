//! Synthetic launcher for the actual Patina App, including its guide/context menu.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() {
    iptv_app::install_test_key([0x54; 32]);
    if std::env::args().any(|a| a == "seed") {
        use iptv_providers::{
            SecretUrl,
            model::{M3uSourceConfig, Source, SourceConfig},
        };
        use iptv_store_sqlite::{DatabaseKey, SourcePolicy, Store};
        let root = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(
            if cfg!(target_os = "macos") {
                "Library/Application Support/iptv-rs"
            } else {
                ".local/share/iptv-rs"
            },
        );
        std::fs::create_dir_all(&root).unwrap();
        let mut store = Store::open_encrypted(
            root.join("iptv.sqlite"),
            &DatabaseKey::from_bytes([0x54; 32]),
        )
        .unwrap();
        let source = store
            .add_source(&Source {
                name: "Generated demo".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("http://127.0.0.1:18091/generated.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        store
            .set_source_policy(
                source,
                SourcePolicy {
                    refresh_on_start: false,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut text = String::from("#EXTM3U\n");
        for (i, name) in ["Synthetic One", "Synthetic Two", "Synthetic Three"]
            .iter()
            .enumerate()
        {
            text.push_str(&format!("#EXTINF:-1 group-title=\"Generated\",{name}\nhttp://127.0.0.1:18091/ch{i}/index.m3u8\n"));
        }
        for (group_index, group) in ["Documentaries", "Music", "Sports"].iter().enumerate() {
            for index in 0..12 {
                let stream = index % 3;
                let channel = index + 1;
                text.push_str(&format!(
                    "#EXTINF:-1 group-title=\"{group}\",{group} {channel:02}\n\
                     http://127.0.0.1:18091/ch{stream}/index.m3u8?group={group_index}&channel={channel}\n"
                ));
            }
        }
        let parsed = iptv_providers::m3u::parse_m3u(&text);
        store
            .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
            .unwrap();
        let sample_time = 1_791_000_000;
        for group in &parsed.groups {
            let members = parsed
                .channels
                .items
                .iter()
                .filter(|channel| channel.groups.contains(&group.id));
            for (index, (channel, minutes)) in members.zip([45, 15, 90]).enumerate() {
                store
                    .add_channel_watch_time(source, &channel.id, minutes * 60_000)
                    .unwrap();
                store
                    .record_watch(
                        source,
                        &channel.id,
                        iptv_providers::model::ContentKind::Live,
                        sample_time + index as i64,
                        None,
                    )
                    .unwrap();
            }
        }
        store.set_active_source(source).unwrap();
        let keys: Vec<_> = parsed
            .channels
            .items
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        std::fs::write(
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
                .join("demo-channel-keys.json"),
            serde_json::to_vec(&keys).unwrap(),
        )
        .unwrap();

        if std::env::var_os("PATINA_GUIDE_WINDOW_WALKTHROUGH").is_none() {
            let mut settings = iptv_settings::Settings::new();
            settings
                .set(
                    iptv_settings::keys::RESUME_LAST_CHANNEL.as_str(),
                    iptv_settings::Value::Bool(false),
                )
                .unwrap();
            std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
        }
        std::fs::write(
            root.join(format!("last_channel-{}", source.0)),
            parsed.channels.items[0].id.as_str(),
        )
        .unwrap();
        return;
    }
    env_logger::init();
    #[cfg(target_os = "linux")]
    use nsh_platform_linux as platform;
    #[cfg(target_os = "macos")]
    use nsh_platform_macos as platform;
    platform::set_components(iptv_app::components);
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<iptv_app::App>()),
        platform::AppOptions {
            title: "Patina · Guide window prototype".into(),
            app_id: "com.hantekllc.iptvrs.guidepreview".into(),
            size: (1280, 850),
            ..Default::default()
        },
    );
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() {
    eprintln!("This prototype currently runs on Linux and macOS.");
}
