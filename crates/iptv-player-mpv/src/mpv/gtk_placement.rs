//! GTK4 cannot position Wayland toplevels. The Linux prototype arranges
//! its own new floating PiPs through Hyprland IPC; it never changes compositor
//! rules or repositions a window that the user has already arranged. Other
//! Wayland compositors retain their own placement policy.
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

static PLACED: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);

fn socket() -> Option<PathBuf> {
    let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    if signature.is_empty() || signature.contains('/') || signature.contains("..") {
        return None;
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(
        PathBuf::from(runtime)
            .join("hypr")
            .join(signature)
            .join(".socket.sock"),
    )
}

fn request(path: &std::path::Path, command: &str) -> Option<String> {
    let mut stream = UnixStream::connect(path).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .ok()?;
    stream.write_all(command.as_bytes()).ok()?;
    let mut reply = String::new();
    stream
        .take(4 * 1024 * 1024)
        .read_to_string(&mut reply)
        .ok()?;
    Some(reply)
}

pub(super) fn arrange() {
    let Some(path) = socket() else { return };
    let _ = std::thread::Builder::new()
        .name("iptv-pip-placement".into())
        .spawn(move || {
            // Wait for the mapped toplevels and their compositor rules to settle.
            std::thread::sleep(Duration::from_millis(200));
            let Some(clients) =
                request(&path, "j/clients").and_then(|s| serde_json::from_str::<Value>(&s).ok())
            else {
                return;
            };
            let Some(monitors) =
                request(&path, "j/monitors").and_then(|s| serde_json::from_str::<Value>(&s).ok())
            else {
                return;
            };
            let Some(clients) = clients.as_array() else {
                return;
            };
            let own: Vec<_> = clients
                .iter()
                .filter(|c| {
                    c["pid"].as_u64() == Some(u64::from(std::process::id()))
                        && c["title"].as_str() == Some(iptv_core::pip_window::TITLE)
                        && c["floating"].as_bool() == Some(true)
                })
                .collect();
            let mut placed = PLACED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let placed = placed.get_or_insert_default();
            placed.retain(|address, _| own.iter().any(|c| c["address"].as_str() == Some(address)));
            for client in &own {
                let Some(address) = client["address"].as_str() else {
                    continue;
                };
                if placed.contains_key(address)
                    || !address.starts_with("0x")
                    || !address[2..].bytes().all(|b| b.is_ascii_hexdigit())
                {
                    continue;
                }
                let Some(monitor) = monitors
                    .as_array()
                    .and_then(|ms| ms.iter().find(|m| m["id"] == client["monitor"]))
                else {
                    continue;
                };
                let number = |key: &str| monitor[key].as_f64().unwrap_or(0.0) as f32;
                let scale = number("scale").max(1.0);
                let reserved = |i| monitor["reserved"][i].as_f64().unwrap_or(0.0) as f32;
                let area = (
                    number("x") + reserved(0),
                    number("y") + reserved(1),
                    number("width") / scale - reserved(0) - reserved(2),
                    number("height") / scale - reserved(1) - reserved(3),
                );
                // Reserve stable slots for all four players so adding another
                // popup does not move an existing window to make room.
                let index = (0..iptv_core::multiview::MAX_SURFACES)
                    .find(|slot| !placed.values().any(|used| used == slot))
                    .unwrap_or(0);
                let positions =
                    iptv_core::pip_window::popup_rects(area, iptv_core::multiview::MAX_SURFACES);
                let Some(&(x, y, w, h)) = positions.get(index) else {
                    continue;
                };
                let resize = format!(
                    "dispatch resizewindowpixel exact {} {},address:{address}",
                    w.round() as i32,
                    h.round() as i32
                );
                let move_to = format!(
                    "dispatch movewindowpixel exact {} {},address:{address}",
                    x.round() as i32,
                    y.round() as i32
                );
                let legacy = request(&path, &resize).is_some_and(|r| r.trim() == "ok")
                    && request(&path, &move_to).is_some_and(|r| r.trim() == "ok");
                // Hyprland's Lua dispatcher API replaced the legacy text
                // dispatcher syntax. Both requests target only our own window.
                let modern = || {
                    let command = format!(
                        concat!(
                            "eval hl.dispatch(hl.dsp.window.resize(",
                            "{{x={},y={},relative=false,window=\"address:{}\"}})); ",
                            "hl.dispatch(hl.dsp.window.move(",
                            "{{x={},y={},relative=false,window=\"address:{}\"}}))"
                        ),
                        w.round() as i32,
                        h.round() as i32,
                        address,
                        x.round() as i32,
                        y.round() as i32,
                        address
                    );
                    request(&path, &command).is_some_and(|r| r.trim() == "ok")
                };
                if legacy || modern() {
                    placed.insert(address.to_owned(), index);
                    log::info!("iptv pip: arranged new window in slot {index}");
                }
            }
        });
}
