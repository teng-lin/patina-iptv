//! Run the actual UI with a fresh, generated library. Never opens the user's profile.
//! cargo run -p iptv-app --features test-support --example library_preview -- /tmp/new-profile
#[cfg(target_os = "linux")]
fn main() {
    use iptv_providers::{HttpResponse, Secret, SecretUrl, model::*};
    use iptv_store_sqlite::{DatabaseKey, SourcePolicy, Store};
    let directory = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("fresh absolute profile path"),
    );
    iptv_app::install_test_key([0x54; 32]);
    iptv_app::install_test_data_dir(directory.clone()).expect("fresh isolated profile");
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("guide.autoplay", iptv_settings::Value::Bool(false))
        .unwrap();
    settings
        .set(
            "general.resume_last_channel",
            iptv_settings::Value::Bool(false),
        )
        .unwrap();
    std::fs::write(directory.join("settings.json"), settings.to_json_string()).unwrap();
    let mut store = Store::open_encrypted(
        directory.join("iptv.sqlite"),
        &DatabaseKey::from_bytes([0x54; 32]),
    )
    .unwrap();
    let live = store
        .add_source(&Source {
            name: "Generated live playlist".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let on_demand = store
        .add_source(&Source {
            name: "Generated library".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: "http://127.0.0.1:18094".into(),
                username: Secret::new("demo"),
                password: Secret::new("demo"),
                user_agent: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated channels\",Generated live channel\nhttps://example.test/live.ts\n",
    );
    store
        .replace_catalog(live, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    let movies: Vec<_> = (1..=48).map(|n| serde_json::json!({
        "stream_id": n, "name": format!("Generated Film {n:03}"), "category_id": (n % 4).to_string(),
        "container_extension": "mp4", "year": 2020 + n % 6, "rating": "7.5",
        "plot": "An original synthetic story used to verify poster browsing, categories and focus."
    })).collect();
    let movies = iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(
        serde_json::to_vec(&movies).unwrap(),
    ))
    .unwrap();
    let series: Vec<_> = (1..=8).map(|n| serde_json::json!({
        "series_id": n, "name": format!("Generated Show {n:03}"), "category_id": (n % 4).to_string(),
        "plot": "A generated series for checking the shared library layout.", "rating": "8.0"
    })).collect();
    let series = iptv_providers::xtream::parse_series_list(&HttpResponse::ok(
        serde_json::to_vec(&series).unwrap(),
    ))
    .unwrap();
    let category_rows: Vec<_> = (0..4).map(|n| serde_json::json!({"category_id": n.to_string(), "category_name": format!("Generated category {}", n+1)})).collect();
    let response = HttpResponse::ok(serde_json::to_vec(&category_rows).unwrap());
    let mut categories = iptv_providers::xtream::parse_categories(&response, ContentKind::Movie)
        .unwrap()
        .items;
    categories.extend(
        iptv_providers::xtream::parse_categories(&response, ContentKind::Series)
            .unwrap()
            .items,
    );
    store
        .replace_catalog(on_demand, &categories, &[], &movies.items, &series.items)
        .unwrap();
    for source in [live, on_demand] {
        store
            .set_source_policy(
                source,
                SourcePolicy {
                    refresh_on_start: false,
                    ..Default::default()
                },
            )
            .unwrap();
    }
    store.set_active_source(live).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for (n, status) in [
        iptv_core::recording::Status::Scheduled,
        iptv_core::recording::Status::Completed,
        iptv_core::recording::Status::Failed,
    ]
    .into_iter()
    .enumerate()
    {
        let request = iptv_core::recording::Request {
            source: live.0,
            channel: parsed.channels.items[0].id.as_str().into(),
            channel_name: "Generated live channel".into(),
            title: format!("Generated recording {}", n + 1),
            start: now + 3600 * (n as i64 + 1),
            end: now + 3600 * (n as i64 + 2),
        };
        let id = store
            .schedule_recording(iptv_core::recording::JobId(n as i64 + 1), &request, now)
            .unwrap();
        if status != iptv_core::recording::Status::Scheduled {
            store
                .transition_recording(id, iptv_core::recording::Status::Scheduled, status, 0, "")
                .unwrap();
        }
    }
    drop(store);
    env_logger::init();
    use nsh_platform_linux as platform;
    platform::set_components(iptv_app::components);
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<iptv_app::App>()),
        platform::AppOptions {
            title: "Patina · Generated library preview".into(),
            app_id: "com.hantekllc.patina.librarypreview".into(),
            size: (1280, 720),
            ..Default::default()
        },
    );
}
#[cfg(not(target_os = "linux"))]
fn main() {}
