//! Runs iptv-rs on the desktop backends (Linux, macOS, Windows), for fast iteration
//! with a keyboard's arrows, Enter and Escape. Android runs `App` through
//! `nsh run`; `nsh run --target macos` / `nsh package --target macos` build
//! their own runner around the same crate.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
fn main() -> impl std::process::Termination {
    env_logger::init();
    use nsh_platform_linux as platform;
    platform::set_components(iptv_app::components);
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<iptv_app::App>()),
        platform::AppOptions {
            title: "Patina".to_owned(),
            app_id: "com.hantekllc.iptvrs".to_owned(),
            size: (960, 540),
            schemes: vec!["patina".into()],
            ..Default::default()
        },
    )
}

#[cfg(target_os = "macos")]
fn main() -> impl std::process::Termination {
    env_logger::init();
    use nsh_platform_macos as platform;
    platform::set_components(iptv_app::components);
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<iptv_app::App>()),
        platform::AppOptions {
            title: "Patina".to_owned(),
            app_id: "com.hantekllc.iptvrs".to_owned(),
            size: (1280, 720),
            schemes: vec!["patina".into()],
            ..Default::default()
        },
    )
}

#[cfg(windows)]
fn main() -> impl std::process::Termination {
    env_logger::init();
    use nsh_platform_windows as platform;
    platform::set_components(iptv_app::components);
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<iptv_app::App>()),
        platform::AppOptions {
            title: "Patina".to_owned(),
            app_id: "com.hantekllc.iptvrs".to_owned(),
            size: (1280, 720),
            schemes: vec!["patina".into()],
            ..Default::default()
        },
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn main() {}
