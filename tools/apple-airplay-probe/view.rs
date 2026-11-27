//! Development-only NSH root. It opens neither the catalog nor the normal player.
use std::net::SocketAddrV4;

use nsh_base::{button, input};
use nsh_core::prelude::*;
use serde_json::json;

fn fixture_url(address: &str, channel: u8) -> Option<String> {
    let address: SocketAddrV4 = address.trim().parse().ok()?;
    let ip = address.ip();
    if address.port() == 0 || !(ip.is_private() || ip.is_link_local()) {
        return None;
    }
    Some(format!("http://{address}/ch{channel}/index.m3u8"))
}

fn control(label: &'static str, on_press: impl Fn() + 'static) -> View {
    view().child(
        button()
            .on_press(on_press)
            .px_3()
            .py_2()
            .border_1()
            .rounded(px(6.0))
            .child(text(label)),
    )
}

/// Hosts the system route picker and reports AVPlayer's external playback state.
#[component]
pub fn AirPlayProbe() -> impl IntoElement {
    let address = signal(std::env::var("PATINA_AIRPLAY_PROBE_ORIGIN").unwrap_or_default());
    let source = signal(None::<String>);
    let channel_name = signal("No channel selected");
    let command = signal((0_u64, "stop"));
    let status = signal("Waiting for native picker".to_owned());
    let select = move |channel| {
        if let Some(url) = fixture_url(&address.get_untracked(), channel) {
            source.set(Some(url));
            channel_name.set(if channel == 0 {
                "Channel A"
            } else {
                "Channel B"
            });
        } else {
            source.set(None);
            channel_name.set("Enter a LAN IPv4 address and port");
        }
    };
    let send = move |action| {
        command.update(|(serial, last)| {
            *serial += 1;
            *last = action;
        })
    };
    view()
        .flex_1()
        .p_6()
        .gap_3()
        .justify_center()
        .child(text("Fixture server address and port"))
        .child(
            input()
                .value(move || address.get())
                .on_change(move |s: &str| address.set(s.to_owned()))
                .placeholder("IPv4:port")
                .label("Fixture server address and port")
                .border_1()
                .px_3()
                .py_2(),
        )
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(control("Channel A", move || select(0)))
                .child(control("Channel B", move || select(1))),
        )
        .child(text(move || channel_name.get()))
        .child(
            native("tv.airplay")
                .test_id("airplay-route-picker")
                .w(px(72.0))
                .h(px(48.0))
                .prop("source", move || json!(source.get()))
                .prop("command", move || {
                    let (serial, action) = command.get();
                    json!({"serial":serial,"action":action})
                })
                .on_native_event(move |event: &NativeEvent| {
                    if event.name.as_ref() == "state"
                        && let (Some(state), Some(external)) = (
                            event.data["status"].as_str(),
                            event.data["external"].as_bool(),
                        )
                    {
                        status.set(format!(
                            "{state} · external playback {}",
                            if external { "on" } else { "off" }
                        ));
                    }
                }),
        )
        .child(text(move || status.get()).test_id("airplay-probe-state"))
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(control("Play", move || send("play")))
                .child(control("Pause", move || send("pause")))
                .child(control("Stop", move || send("stop"))),
        )
}

#[cfg(test)]
mod tests {
    use super::fixture_url;

    #[test]
    fn generated_channels_accept_only_local_ipv4_authorities() {
        for address in ["192.168.9.11:8089", "10.0.0.1:1234", "169.254.2.2:80"] {
            assert_eq!(
                fixture_url(address, 1),
                Some(format!("http://{address}/ch1/index.m3u8"))
            );
        }
        for address in [
            "",
            "127.0.0.1:9000",
            "192.168.9.11:0",
            "198.51.100.10:8089",
            "fixture.invalid:8089",
            "http://10.0.0.1:8089",
            "test@10.0.0.1:80",
            "10.0.0.1:8089/other",
            "10.0.0.1:80?test",
            "10.0.0.1:80#test",
        ] {
            assert_eq!(fixture_url(address, 0), None, "{address}");
        }
    }
}
