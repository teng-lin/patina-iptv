use super::*;
use std::{
    io::{Read, Seek, SeekFrom},
    process::{Command, Output},
};

pub(crate) fn subprocess(test: &str) -> Command {
    let mut child = Command::new(std::env::current_exe().expect("TUI test binary"));
    child.args(["--exact", test, "--nocapture"]);
    child
}

pub(crate) fn child_output(command: &mut Command) -> io::Result<Output> {
    // Files let the child report failures without blocking on a full pipe.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "TUI test child timed out",
                ));
            }
            Err(error) => break Err(error),
        }
    };
    if status.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    stdout.seek(SeekFrom::Start(0))?;
    stderr.seek(SeekFrom::Start(0))?;
    let mut captured_stdout = Vec::new();
    let mut captured_stderr = Vec::new();
    stdout.read_to_end(&mut captured_stdout)?;
    stderr.read_to_end(&mut captured_stderr)?;
    let status = status.map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "{error}\n{}\n{}",
                String::from_utf8_lossy(&captured_stdout),
                String::from_utf8_lossy(&captured_stderr),
            ),
        )
    })?;
    if !String::from_utf8_lossy(&captured_stdout).contains("running 1 test") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "TUI child did not run its selected test",
        ));
    }
    Ok(Output {
        status,
        stdout: captured_stdout,
        stderr: captured_stderr,
    })
}

#[test]
fn cli_selects_video_and_keeps_headless_output_on_demo_data() {
    assert_eq!(
        parse_args(["--demo".into(), "--video=popup".into()]),
        (VideoMode::Popup, false, true)
    );
    assert_eq!(
        parse_args(["--video=kitty".into()]),
        (VideoMode::Kitty, false, false)
    );
    assert_eq!(
        parse_args([
            "--video=popup".into(),
            "--headless".into(),
            "--video=inline".into()
        ]),
        (VideoMode::Inline, true, true)
    );
}

#[test]
fn video_defaults_follow_the_terminal_and_ssh_session() {
    const EXPECTED_LABEL: &str = "PATINA_TUI_TEST_VIDEO_LABEL";
    if let Ok(expected) = std::env::var(EXPECTED_LABEL) {
        let (mode, headless, demo) = parse_args([]);
        assert_eq!(mode.label(), expected);
        assert!(!headless && !demo);
        return;
    }

    for (terminal, ssh, label) in [
        ("xterm-256color", false, "inline"),
        ("xterm-kitty", false, "kitty-hd"),
        ("xterm-ghostty", true, "kitty-ssh"),
    ] {
        let mut child = subprocess("tests::video_defaults_follow_the_terminal_and_ssh_session");
        child
            .env(EXPECTED_LABEL, label)
            .env("TERM", terminal)
            .env_remove("GHOSTTY_RESOURCES_DIR")
            .env_remove("KITTY_WINDOW_ID")
            .env_remove("WEZTERM_EXECUTABLE")
            .env_remove("SSH_CONNECTION")
            .env_remove("SSH_TTY");
        if ssh {
            child.env("SSH_TTY", "/synthetic/pty");
        }
        let output = child_output(&mut child).expect("read isolated terminal defaults");
        assert!(
            output.status.success(),
            "{terminal}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn cli_help_succeeds_and_an_unknown_option_reports_a_usage_error() {
    const ARGUMENT: &str = "PATINA_TUI_TEST_CLI_ARGUMENT";
    if let Ok(argument) = std::env::var(ARGUMENT) {
        let _ = parse_args([argument]);
        panic!("help and invalid arguments should exit before launch");
    }

    for (argument, code, diagnostic) in [
        ("--help", 0, "Usage: patina-tui"),
        (
            "--synthetic-invalid",
            2,
            "Unknown argument: --synthetic-invalid",
        ),
    ] {
        let output = child_output(
            subprocess("tests::cli_help_succeeds_and_an_unknown_option_reports_a_usage_error")
                .env(ARGUMENT, argument),
        )
        .expect("run isolated argument parser");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(code), "{argument}: {stderr}");
        assert!(stderr.contains(diagnostic), "{stderr}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn headless_startup_renders_the_seeded_demo_catalog() {
    const DEMO_ROOT: &str = "PATINA_TUI_TEST_DEMO_ROOT";
    if let Some(directory) = std::env::var_os(DEMO_ROOT) {
        let (mode, headless, demo) = parse_args(["--headless".into(), "--video=none".into()]);
        assert!(demo, "headless startup uses a synthetic catalog");
        iptv_app::install_test_key([0x71; 32]);
        seed_demo(Path::new(&directory)).expect("seed synthetic demo profile");
        run(mode, headless.then_some(Duration::from_secs(4))).expect("render headless terminal");
        return;
    }

    let directory = tempfile::tempdir().expect("isolated demo directory");
    let output = child_output(
        subprocess("tests::headless_startup_renders_the_seeded_demo_catalog")
            .env(DEMO_ROOT, directory.path())
            .env("HOME", directory.path())
            .env("XDG_DATA_HOME", directory.path().join(".local/share"))
            .env("XDG_CONFIG_HOME", directory.path().join(".config"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/synthetic/patina-test-no-bus",
            ),
    )
    .expect("run isolated headless app");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("Synthetic Channel 01 HD")),
        "the app must load the demo catalog from its isolated profile:\n{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with(" Patina TUI [none]"))
    );
}

#[test]
fn terminal_presses_deliver_taps_and_repeats_keep_their_modifiers() {
    let mut events = VecDeque::new();
    for (code, expected) in [
        (KeyCode::Up, "ArrowUp"),
        (KeyCode::Down, "ArrowDown"),
        (KeyCode::Left, "ArrowLeft"),
        (KeyCode::Right, "ArrowRight"),
        (KeyCode::Enter, "Enter"),
        (KeyCode::Esc, "Escape"),
        (KeyCode::Backspace, "Backspace"),
        (KeyCode::Delete, "Delete"),
        (KeyCode::Tab, "Tab"),
        (KeyCode::Home, "Home"),
        (KeyCode::End, "End"),
        (KeyCode::PageUp, "PageUp"),
        (KeyCode::PageDown, "PageDown"),
        (KeyCode::Char('f'), "f"),
    ] {
        map_key_events(code, KeyModifiers::NONE, KeyEventKind::Press, &mut events);
        for release in [false, true] {
            assert!(
                matches!(
                    events.pop_front(),
                    Some(RunEvent::Input(Input::Key { key, modifiers, release: released }))
                        if key == expected && released == release && modifiers == Modifiers::default()
                ),
                "{code:?} should deliver {expected} with release={release}"
            );
        }
        assert!(events.is_empty(), "{code:?} queued an extra event");
    }

    map_key_events(
        KeyCode::BackTab,
        KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::CONTROL,
        KeyEventKind::Repeat,
        &mut events,
    );
    assert!(matches!(
        events.pop_front(),
        Some(RunEvent::Input(Input::KeyRepeat { key, modifiers }))
            if key == "Tab" && modifiers == Modifiers { shift: true, alt: true, cmd: true, ctrl: true }
    ));
    assert!(events.is_empty());
}

#[test]
fn quit_shortcuts_exit_once_and_ignore_physical_releases() {
    let mut events = VecDeque::new();
    for (code, modifiers) in [
        (KeyCode::F(10), KeyModifiers::NONE),
        (KeyCode::Char('c'), KeyModifiers::CONTROL),
        (KeyCode::Char('q'), KeyModifiers::CONTROL),
    ] {
        map_key_events(code, modifiers, KeyEventKind::Press, &mut events);
        map_key_events(code, modifiers, KeyEventKind::Release, &mut events);
        assert!(
            matches!(events.pop_front(), Some(RunEvent::Exit)),
            "{code:?}"
        );
        assert!(events.is_empty(), "{code:?} queued an extra event");
    }

    map_key_events(
        KeyCode::F(9),
        KeyModifiers::NONE,
        KeyEventKind::Press,
        &mut events,
    );
    assert!(
        events.is_empty(),
        "unmapped keys should leave the loop idle"
    );
    map_key_events(
        KeyCode::Char('q'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        KeyEventKind::Press,
        &mut events,
    );
    assert!(matches!(
        events.pop_front(),
        Some(RunEvent::Input(Input::Key { key, release: false, .. })) if key == "q"
    ));
}

#[test]
fn the_terminal_loop_draws_the_footer_and_exits_on_the_quit_event() {
    let mut terminal = Terminal::new(TestBackend::new(40, 3)).expect("in-memory terminal");
    let mut view = AppView::new(AppOptions::default());
    let mut events = [RunEvent::Idle, RunEvent::Redraw, RunEvent::Exit].into_iter();
    let mut iterations = 0;
    run_loop(
        &mut terminal,
        &mut view,
        VideoMode::None,
        |terminal| {
            iterations += 1;
            let buffer = terminal.backend().buffer();
            let footer: String = (0..40).map(|x| buffer[(x, 2)].symbol()).collect();
            assert!(footer.starts_with(" Patina TUI [none]"));
            Ok(())
        },
        |_| {
            Ok(events
                .next()
                .expect("the loop exits on the queued quit event"))
        },
    )
    .expect("terminal loop");
    assert_eq!(iterations, 3, "quit ends the loop immediately");
}
