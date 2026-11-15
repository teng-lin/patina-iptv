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
            arrange_at(&path, &PLACED);
        });
}

// Hold the cache lock from snapshot acquisition through the placement updates.
// Otherwise an older snapshot can prune a newer worker's placements.
fn arrange_at(path: &std::path::Path, cache: &Mutex<Option<HashMap<String, usize>>>) {
    let mut placed = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(clients) =
        request(path, "j/clients").and_then(|s| serde_json::from_str::<Value>(&s).ok())
    else {
        return;
    };
    let Some(monitors) =
        request(path, "j/monitors").and_then(|s| serde_json::from_str::<Value>(&s).ok())
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
        let area = work_area(monitor);
        // Reserve stable slots for all four players so adding another
        // popup does not move an existing window to make room.
        let index = (0..iptv_core::multiview::MAX_SURFACES)
            .find(|slot| !placed.values().any(|used| used == slot))
            .unwrap_or(0);
        let positions =
            iptv_core::pip_window::popup_rects(area, iptv_core::multiview::MAX_SURFACES);
        let Some(&slot) = positions.get(index) else {
            continue;
        };
        let size = (
            client["size"][0].as_f64().unwrap_or(0.0) as f32,
            client["size"][1].as_f64().unwrap_or(0.0) as f32,
        );
        let Some((x, y, w, h)) = fit_window(slot, size) else {
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
        let legacy = request(path, &resize).is_some_and(|r| r.trim() == "ok")
            && request(path, &move_to).is_some_and(|r| r.trim() == "ok");
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
            request(path, &command).is_some_and(|r| r.trim() == "ok")
        };
        if legacy || modern() {
            placed.insert(address.to_owned(), index);
            log::info!("iptv pip: arranged new window in slot {index}");
        }
    }
}

fn fit_window(slot: (f32, f32, f32, f32), size: (f32, f32)) -> Option<(f32, f32, f32, f32)> {
    let (w, h) = size;
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let scale = (slot.2 / w).min(slot.3 / h).min(1.0);
    let (w, h) = (w * scale, h * scale);
    Some((slot.0 + slot.2 - w, slot.1 + slot.3 - h, w, h))
}

fn work_area(monitor: &Value) -> (f32, f32, f32, f32) {
    let number = |key: &str| monitor[key].as_f64().unwrap_or(0.0) as f32;
    let scale = monitor["scale"]
        .as_f64()
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0) as f32;
    // Hyprland reports the pixel-mode size separately from its transform.
    let (width, height) = if monitor["transform"].as_u64().unwrap_or(0) % 2 == 1 {
        (number("height"), number("width"))
    } else {
        (number("width"), number("height"))
    };
    let reserved = |i| monitor["reserved"][i].as_f64().unwrap_or(0.0) as f32;
    (
        number("x") + reserved(0),
        number("y") + reserved(1),
        width / scale - reserved(0) - reserved(2),
        height / scale - reserved(1) - reserved(3),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;

    #[test]
    fn mixed_video_shapes_fit_separate_slots_without_stretching() {
        for area in [(0.0, 0.0, 1920.0, 1080.0), (-720.0, -200.0, 720.0, 1280.0)] {
            let slots = iptv_core::pip_window::popup_rects(area, 4);
            let sizes = [
                (600.0, 450.0),
                (600.0, 338.0),
                (600.0, 257.0),
                (600.0, 450.0),
            ];
            let mut placed = Vec::new();
            for (slot, size) in slots.into_iter().zip(sizes) {
                let (x, y, w, h) = fit_window(slot, size).unwrap();
                assert!((w / h - size.0 / size.1).abs() < 0.001);
                assert!(w <= size.0 && h <= size.1);
                assert!(
                    x >= area.0
                        && y >= area.1
                        && x + w <= area.0 + area.2 + 0.01
                        && y + h <= area.1 + area.3 + 0.01
                );
                for &(a, b, c, d) in &placed {
                    assert!(x + w <= a || a + c <= x || y + h <= b || b + d <= y);
                }
                placed.push((x, y, w, h));
            }
        }
        assert_eq!(
            fit_window((0.0, 0.0, 600.0, 450.0), (600.0, 450.0))
                .unwrap()
                .2,
            600.0
        );
    }

    #[test]
    fn transformed_monitor_slots_stay_in_the_scaled_work_area() {
        for transform in 0..8 {
            let monitor = json!({"x":-1280, "y":-240, "width":1920, "height":1080,
                "scale":1.5, "reserved":[12,24,36,48], "transform":transform});
            let area = work_area(&monitor);
            let expected = if transform % 2 == 1 {
                (672.0, 1208.0)
            } else {
                (1232.0, 648.0)
            };
            assert_eq!(area, (-1268.0, -216.0, expected.0, expected.1));
            for (x, y, w, h) in iptv_core::pip_window::popup_rects(area, 4) {
                assert!(x >= area.0 && y >= area.1);
                assert!(x + w <= area.0 + area.2 + 0.01 && y + h <= area.1 + area.3 + 0.01);
            }
        }
    }

    #[test]
    fn snapshot_and_cache_update_are_one_serial_operation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hypr.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let cache = Arc::new(Mutex::new(Some(HashMap::from([("0x1".into(), 0)]))));
        let worker_cache = cache.clone();
        let worker = std::thread::spawn(move || arrange_at(&path, &worker_cache));
        for command in ["j/clients", "j/monitors"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut data = [0; 64];
            let n = stream.read(&mut data).unwrap();
            assert_eq!(&data[..n], command.as_bytes());
            // Neither a stale worker nor a cache update can overtake this
            // pending snapshot, even while IPC is waiting on the compositor.
            assert!(cache.try_lock().is_err());
            stream.write_all(b"[]").unwrap();
        }
        worker.join().unwrap();
        assert!(cache.lock().unwrap().as_ref().unwrap().is_empty());
    }
}
