mod player;

use std::{
    collections::VecDeque,
    io,
    sync::Arc,
    time::{Duration, Instant},
};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use nsh_platform_tui::{
    AppOptions, AppView, Appearance, CellSize, ColorDepth, Input, Modifiers, Root, RunEvent,
};
use player::{KittyOverlay, TuiPlayer, VideoMode};
use ratatui_core::{
    backend::{Backend, TestBackend},
    layout::Rect,
    style::{Color, Style},
    terminal::{Frame, Terminal},
};
use ratatui_crossterm::CrosstermBackend;
use tempfile::TempDir;

fn parse_args() -> (VideoMode, bool, bool) {
    let mut mode = VideoMode::detect_default();
    let mut headless = false;
    let mut demo = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--video=kitty" => mode = VideoMode::Kitty,
            "--video=inline" => mode = VideoMode::Inline,
            "--video=popup" => mode = VideoMode::Popup,
            "--video=none" => mode = VideoMode::None,
            "--headless" => {
                headless = true;
                // Headless snapshots always use synthetic demo data so real
                // provider channel names are never printed to stdout.
                demo = true;
            }
            "--demo" => demo = true,
            "-h" | "--help" => {
                eprintln!(
                    "Usage: patina-tui [--video=kitty|inline|popup|none] [--headless] [--demo]\n\
                     \n\
                     --video=kitty   Fullscreen pixel video via Kitty Graphics Protocol (video-only)\n\
                     --video=inline  Fullscreen half-block (▀) video in any 24-bit terminal (video-only)\n\
                     --video=popup   Pop up a separate native mpv window on the local display\n\
                     --video=none    Headless guide/DVR browsing with no video output\n\
                     --headless      Render a 120x40 synthetic snapshot to stdout and exit\n\
                     --demo          Use an isolated temporary HOME with synthetic channels"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("Unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    (mode, headless, demo)
}

fn seed_demo_home() -> io::Result<TempDir> {
    let dir = tempfile::Builder::new()
        .prefix("patina-tui-demo-")
        .tempdir()?;
    let data = dir.path().join(".local/share/iptv-rs");
    std::fs::create_dir_all(&data)?;
    // SAFETY: called at startup before any background threads are spawned.
    unsafe {
        std::env::set_var("HOME", dir.path());
        std::env::set_var("XDG_DATA_HOME", dir.path().join(".local/share"));
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join(".config"));
    }
    let mut playlist = String::from("#EXTM3U\n");
    for i in 1..=15 {
        let group = if i <= 5 {
            "News"
        } else if i <= 10 {
            "Sports"
        } else {
            "Movies"
        };
        playlist.push_str(&format!(
            "#EXTINF:-1 tvg-id=\"ch.{i}\" group-title=\"{group}\",Synthetic Channel {i:02} HD\nhttp://127.0.0.1:19999/live/ch{i}.ts\n"
        ));
    }
    let m3u_path = dir.path().join("demo.m3u");
    std::fs::write(&m3u_path, &playlist)?;
    let mut store = iptv_store_sqlite::Store::open(data.join("iptv.sqlite"))
        .map_err(|e| io::Error::other(e.to_string()))?;
    let source = store
        .add_source(&iptv_providers::model::Source {
            name: "Synthetic Demo".into(),
            config: iptv_providers::model::SourceConfig::M3u(
                iptv_providers::model::M3uSourceConfig {
                    location: iptv_providers::SecretUrl::new(m3u_path.to_string_lossy()),
                    user_agent: None,
                    referrer: None,
                },
            ),
        })
        .map_err(|e| io::Error::other(e.to_string()))?;
    let parsed = iptv_providers::m3u::parse_m3u(&playlist);
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .map_err(|e| io::Error::other(e.to_string()))?;
    store
        .set_source_policy(
            source,
            iptv_store_sqlite::SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .map_err(|e| io::Error::other(e.to_string()))?;
    drop(store);
    let mut settings = iptv_settings::Settings::new();
    let _ = settings.set("epg.update_on_start", iptv_settings::Value::Bool(false));
    let _ = settings.set(
        "playback.resume_last_channel",
        iptv_settings::Value::Bool(false),
    );
    std::fs::write(data.join("settings.json"), settings.to_json_string())?;
    Ok(dir)
}

fn map_key_events(
    code: KeyCode,
    mods: KeyModifiers,
    kind: KeyEventKind,
    queue: &mut VecDeque<RunEvent>,
) {
    if kind == KeyEventKind::Release {
        return;
    }
    let ctrl_only = mods == KeyModifiers::CONTROL;
    if code == KeyCode::F(10) || (ctrl_only && matches!(code, KeyCode::Char('c' | 'q'))) {
        queue.push_back(RunEvent::Exit);
        return;
    }
    let mut m = Modifiers {
        shift: mods.contains(KeyModifiers::SHIFT),
        ctrl: mods.contains(KeyModifiers::CONTROL),
        alt: mods.contains(KeyModifiers::ALT),
        cmd: mods.contains(KeyModifiers::SUPER),
    };
    let name: Option<String> = match code {
        KeyCode::Up => Some("ArrowUp".into()),
        KeyCode::Down => Some("ArrowDown".into()),
        KeyCode::Left => Some("ArrowLeft".into()),
        KeyCode::Right => Some("ArrowRight".into()),
        KeyCode::Enter => Some("Enter".into()),
        KeyCode::Esc => Some("Escape".into()),
        KeyCode::Backspace => Some("Backspace".into()),
        KeyCode::Delete => Some("Delete".into()),
        KeyCode::Tab => Some("Tab".into()),
        KeyCode::BackTab => {
            m.shift = true;
            Some("Tab".into())
        }
        KeyCode::Home => Some("Home".into()),
        KeyCode::End => Some("End".into()),
        KeyCode::PageUp => Some("PageUp".into()),
        KeyCode::PageDown => Some("PageDown".into()),
        KeyCode::Char(c) => Some(c.to_string()),
        _ => None,
    };
    if let Some(key) = name {
        if kind == KeyEventKind::Repeat {
            queue.push_back(RunEvent::Input(Input::KeyRepeat { key, modifiers: m }));
        } else {
            // Synthesize Down + Up so tap recognizers complete in terminals
            // that do not emit Kitty key-release events.
            queue.push_back(RunEvent::Input(Input::Key {
                key: key.clone(),
                modifiers: m,
                release: false,
            }));
            queue.push_back(RunEvent::Input(Input::Key {
                key,
                modifiers: m,
                release: true,
            }));
        }
    }
}

fn draw_with_footer(frame: &mut Frame<'_>, view: &mut AppView, video_mode: VideoMode) {
    let full = frame.area();
    if full.height <= 1 {
        view.render(frame, full);
        return;
    }
    let app_area = Rect {
        x: full.x,
        y: full.y,
        width: full.width,
        height: full.height - 1,
    };
    let footer_y = full.y + full.height - 1;
    view.render(frame, app_area);

    let mode_str = video_mode.label();
    let text = format!(
        " Patina TUI [{mode_str}] │ Arrows: Nav │ Enter: Select │ Esc/Bksp: Back/Guide │ f: Fullscreen │ m: Menu │ F10 / Ctrl+Q: Quit "
    );
    let style = Style::default()
        .bg(Color::Rgb(28, 34, 44))
        .fg(Color::Rgb(215, 224, 236));
    let buf = frame.buffer_mut();
    for x in full.left()..full.right() {
        if let Some(cell) = buf.cell_mut((x, footer_y)) {
            cell.set_char(' ').set_style(style);
        }
    }
    for (i, ch) in text.chars().enumerate() {
        let x = full.left() + i as u16;
        if x >= full.right() {
            break;
        }
        if let Some(cell) = buf.cell_mut((x, footer_y)) {
            cell.set_char(ch).set_style(style);
        }
    }
}

fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    view: &mut AppView,
    video_mode: VideoMode,
    mut on_drawn: impl FnMut(&mut Terminal<B>) -> Result<(), B::Error>,
    mut poll: impl FnMut(Duration) -> Result<RunEvent, B::Error>,
) -> Result<(), B::Error> {
    const POLL_INTERVAL: Duration = Duration::from_millis(16);
    let result = (|| {
        let mut redraw = true;
        loop {
            if redraw
                || view
                    .next_frame_deadline()
                    .is_some_and(|due| due <= Instant::now())
            {
                terminal.draw(|frame| draw_with_footer(frame, view, video_mode))?;
                redraw = false;
            }
            on_drawn(terminal)?;
            if view.exit_requested() {
                return Ok(());
            }
            let wait = view.next_frame_deadline().map_or(POLL_INTERVAL, |due| {
                due.saturating_duration_since(Instant::now())
                    .min(POLL_INTERVAL)
            });
            match poll(wait)? {
                RunEvent::Input(input) => {
                    view.handle(input);
                    redraw = true;
                }
                RunEvent::Redraw => redraw = true,
                RunEvent::Idle => {}
                RunEvent::Exit => return Ok(()),
                _ => {}
            }
        }
    })();
    view.stop();
    result
}

fn restore_terminal(kitty: Option<&KittyOverlay>) {
    let mut stdout = io::stdout();
    if let Some(k) = kitty {
        let _ = k.clear(&mut stdout);
    }
    let _ = disable_raw_mode();
    let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
}

struct TerminalGuard {
    kitty: Option<Arc<KittyOverlay>>,
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal(self.kitty.as_deref());
    }
}

fn main() -> io::Result<()> {
    env_logger::init();
    iptv_app::set_tui_mode(true);
    let (video_mode, headless, demo) = parse_args();
    let _demo_dir = if demo { Some(seed_demo_home()?) } else { None };

    let mut options = AppOptions::default();
    options.cells =
        CellSize::new(nsh_core::Points(8.0), nsh_core::Points(16.0)).map_err(io::Error::other)?;
    options.appearance = Appearance::Dark;
    options.capabilities.colors = ColorDepth::TrueColor;
    options.capabilities.background_color = Some([16, 20, 28]);
    options.capabilities.foreground_color = Some([238, 242, 248]);

    let kitty = KittyOverlay::new();
    let kitty_player = kitty.clone();
    let kitty_pip = KittyOverlay::new();

    let mut view = AppView::new(options);
    view.register_component(
        iptv_player_mpv::PLAYER_VIEW,
        Box::new(move || Box::new(TuiPlayer::new(video_mode, kitty_player.clone()))),
    )
    .map_err(io::Error::other)?;
    view.register_component(
        iptv_player_mpv::PIP_VIEW,
        Box::new(move || Box::new(TuiPlayer::new(VideoMode::None, kitty_pip.clone()))),
    )
    .map_err(io::Error::other)?;

    if headless {
        let mut terminal = Terminal::new(TestBackend::new(120, 40))
            .map_err(|e| io::Error::other(e.to_string()))?;
        terminal
            .draw(|frame| draw_with_footer(frame, &mut view, video_mode))
            .map_err(|e| io::Error::other(e.to_string()))?;
        view.load(Root::of::<iptv_app::App>())?;
        let deadline = Instant::now() + Duration::from_millis(1200);
        run_loop(
            &mut terminal,
            &mut view,
            video_mode,
            |_| Ok(()),
            |timeout| {
                if Instant::now() >= deadline {
                    return Ok(RunEvent::Exit);
                }
                std::thread::sleep(timeout.min(Duration::from_millis(16)));
                Ok(RunEvent::Redraw)
            },
        )
        .map_err(|e| io::Error::other(e.to_string()))?;
        let buf = terminal.backend().buffer();
        for y in 0..buf.area.height {
            let mut line = String::with_capacity(usize::from(buf.area.width));
            for x in 0..buf.area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            println!("{}", line.trim_end());
        }
        return Ok(());
    }

    let panic_kitty = (video_mode == VideoMode::Kitty).then(|| Arc::downgrade(&kitty));
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let upgraded = panic_kitty.as_ref().and_then(std::sync::Weak::upgrade);
        restore_terminal(upgraded.as_deref());
        prev_hook(info);
    }));

    enable_raw_mode()?;
    let _guard = TerminalGuard {
        kitty: (video_mode == VideoMode::Kitty).then(|| kitty.clone()),
    };
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut queue: VecDeque<RunEvent> = VecDeque::new();
    let kitty_draw = kitty.clone();
    terminal.draw(|frame| draw_with_footer(frame, &mut view, video_mode))?;
    view.load(Root::of::<iptv_app::App>())?;
    run_loop(
        &mut terminal,
        &mut view,
        video_mode,
        |_| {
            if video_mode == VideoMode::Kitty {
                kitty_draw.flush(&mut io::stdout())?;
            }
            Ok(())
        },
        |timeout| {
            if let Some(ev) = queue.pop_front() {
                return Ok(ev);
            }
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(k) => {
                        map_key_events(k.code, k.modifiers, k.kind, &mut queue);
                        if let Some(ev) = queue.pop_front() {
                            return Ok(ev);
                        }
                    }
                    Event::Resize(_, _) => return Ok(RunEvent::Redraw),
                    _ => {}
                }
            }
            Ok(if video_mode == VideoMode::Inline {
                RunEvent::Redraw
            } else {
                RunEvent::Idle
            })
        },
    )
}
