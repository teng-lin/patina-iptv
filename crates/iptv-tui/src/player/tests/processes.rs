use std::os::unix::{fs::PermissionsExt, net::UnixListener};
use std::time::{Duration, Instant};

use super::overlay;
use crate::player::{KittyOverlay, TuiPlayer, VideoMode};
use nsh_core::JsonValue;
use nsh_platform_tui::NativeComponent;
use ratatui_core::{buffer::Buffer, layout::Rect, style::Color};
use std::{fs, io, io::Read, sync::Arc};

fn in_fake_player_child(test: &str, command: &str, script: &str) -> bool {
    const CHILD: &str = "PATINA_TUI_TEST_FAKE_PLAYER";
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    // The system temporary directory can be mounted with noexec.
    let test_binary = std::env::current_exe().expect("TUI test binary");
    let directory = tempfile::Builder::new()
        .prefix("patina-player-")
        .tempdir_in(test_binary.parent().expect("TUI test binary directory"))
        .expect("isolated fake player directory");
    let executable = directory.path().join(command);
    fs::write(&executable, script).expect("write fake player");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
        .expect("make fake player executable");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(directory.path().to_owned()).chain(std::env::split_paths(&path)),
    )
    .expect("prepend synthetic command directory");
    let output = crate::tests::child_output(
        crate::tests::subprocess(test)
            .current_dir(directory.path())
            .env(CHILD, "1")
            .env("PATH", path)
            .env("SSH_TTY", "/synthetic/pty"),
    )
    .expect("run isolated player test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

fn source(url: &str) -> (Arc<str>, Option<JsonValue>) {
    let headers = [(
        "X-Test-Provider".into(),
        JsonValue::from("synthetic-header-value"),
    )];
    let fields = [
        ("url".into(), JsonValue::from(url)),
        ("token".into(), JsonValue::from(1)),
        (
            "headers".into(),
            JsonValue::Object(headers.into_iter().collect()),
        ),
    ];
    (
        "source".into(),
        Some(JsonValue::Object(fields.into_iter().collect())),
    )
}

const SYNTHETIC_STREAM: &str = "http://127.0.0.1:19999/synthetic.ts";

fn child_args(player: &TuiPlayer) -> String {
    let args = fs::read_to_string("player-args").expect("fake player recorded its arguments");
    let relay = player
        .relay
        .as_ref()
        .expect("the child uses the playback relay");
    assert!(args.lines().any(|arg| arg == relay.url()));
    assert!(args.lines().all(|arg| arg != SYNTHETIC_STREAM));
    assert!(!args.contains("synthetic-header-value"));
    args
}

#[test]
fn popup_keeps_its_player_in_guide_mode_and_retries_controls_when_ipc_appears() {
    if !in_fake_player_child(
        "player::tests::processes::popup_keeps_its_player_in_guide_mode_and_retries_controls_when_ipc_appears",
        "mpv",
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > player-args\nexec sleep 30\n",
    ) {
        return;
    }

    let mut player = TuiPlayer::new(VideoMode::Popup, overlay(None));
    let fullscreen = Rect::new(0, 0, 32, 12);
    let mut buffer = Buffer::empty(fullscreen);
    player.set_props(&[source("file:///synthetic/stream.ts")]);
    assert!(
        player.child.is_none() && player.relay.is_none(),
        "the relay must accept the source before mpv starts"
    );
    player.set_props(&[
        source(SYNTHETIC_STREAM),
        ("paused".into(), Some(JsonValue::Bool(true))),
        ("muted".into(), Some(JsonValue::Bool(true))),
    ]);
    let pid = player.child.as_ref().expect("popup launched mpv").id();
    let deadline = Instant::now() + Duration::from_secs(5);
    // The shell writes the final argument as the last line.
    let relay_url = player.relay.as_ref().expect("popup relay").url();
    wait_for(deadline, || {
        fs::read_to_string("player-args")
            .is_ok_and(|args| args.ends_with(&format!("{relay_url}\n")))
    });
    let args = child_args(&player);
    assert!(args.lines().any(|arg| arg == "--pause=yes"));
    assert!(args.lines().any(|arg| arg == "--mute=yes"));

    player.render(fullscreen, &mut buffer);
    player.render(Rect::new(0, 0, 8, 3), &mut buffer);
    assert_eq!(
        player.child.as_ref().map(std::process::Child::id),
        Some(pid)
    );

    player.set_props(&[("paused".into(), None), ("muted".into(), None)]);
    assert_eq!(player.pending_mpv_pause, Some(false));
    assert_eq!(player.pending_mpv_mute, Some(false));
    player.set_props(&[("paused".into(), Some(JsonValue::Bool(true)))]);

    let socket = player.mpv_ipc_path.clone().expect("mpv IPC path");
    let listener = UnixListener::bind(&socket).expect("fake mpv IPC server");
    listener.set_nonblocking(true).expect("bounded accepts");
    player.render(fullscreen, &mut buffer);
    for expected in [
        "{\"command\":[\"set_property\",\"pause\",true]}\n",
        "{\"command\":[\"set_property\",\"mute\",false]}\n",
    ] {
        let (mut stream, _) = listener.accept().expect("queued command reached mpv");
        let mut command = String::new();
        stream
            .read_to_string(&mut command)
            .expect("read mpv command");
        assert_eq!(command, expected);
    }
    player.render(fullscreen, &mut buffer);
    assert_eq!(
        listener
            .accept()
            .expect_err("no duplicate IPC command")
            .kind(),
        io::ErrorKind::WouldBlock
    );

    player.set_props(&[("source".into(), None)]);
    assert!(player.child.is_none());
    assert!(player.relay.is_none());
    assert!(
        !socket.exists(),
        "clearing the source removes the player IPC directory"
    );
}

#[test]
fn terminal_video_reads_a_complete_frame_then_reaps_the_decoder() {
    if !in_fake_player_child(
        "player::tests::processes::terminal_video_reads_a_complete_frame_then_reaps_the_decoder",
        "ffmpeg",
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > player-args\nexec cat frame.rgb\n",
    ) {
        return;
    }

    let full = Rect::new(0, 0, 32, 12);
    let preview = Rect::new(0, 0, 8, 3);
    let mut buffer = Buffer::empty(full);
    for (mode, width, height, fps) in [
        (VideoMode::Inline, 32, 24, 15),
        (VideoMode::Kitty, 1280, 720, 20),
    ] {
        let pixels = [8, 13, 21].repeat(width * height);
        fs::write("frame.rgb", &pixels).expect("synthetic RGB24 frame");
        let overlay = KittyOverlay::new();
        let mut player = TuiPlayer::new(mode, overlay.clone());
        player.set_props(&[source(SYNTHETIC_STREAM)]);
        player.render(preview, &mut buffer);
        assert!(
            player.child.is_none(),
            "preview defers launching a terminal decoder"
        );

        player.render(full, &mut buffer);
        let child = player.child.as_mut().expect("fullscreen launched ffmpeg");
        wait_for(Instant::now() + Duration::from_secs(5), || {
            child
                .try_wait()
                .expect("decoder process status")
                .is_some_and(|status| {
                    assert!(status.success(), "fake decoder should exit cleanly");
                    true
                })
        });
        let args = child_args(&player);
        let filter = format!("scale={width}:{height}:flags=lanczos,fps={fps}");
        assert!(args.lines().any(|arg| arg == filter));
        assert!(args.lines().any(|arg| arg == "rgb24"));

        player.render(full, &mut buffer);
        assert!(player.child.is_none());
        assert!(player.reader_thread.is_none());
        assert!(player.relay.is_none());
        if mode == VideoMode::Inline {
            assert_eq!(buffer[(0, 0)].symbol(), "▀");
            assert_eq!(buffer[(0, 0)].fg, Color::Rgb(8, 13, 21));
        } else {
            let frame = overlay.frame.lock().expect("overlay frame lock");
            assert_eq!(frame.as_ref().expect("decoded Kitty frame").data, pixels);
            assert_eq!(buffer[(0, 0)].bg, Color::Reset);
        }

        player.render(preview, &mut buffer);
        assert!(player.frame.lock().expect("inline frame lock").is_none());
        assert!(overlay.frame.lock().expect("overlay frame lock").is_none());
    }
}

fn wait_for(deadline: Instant, mut ready: impl FnMut() -> bool) {
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "fake player did not finish its step"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
