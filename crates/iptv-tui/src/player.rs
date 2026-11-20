use std::{
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

use base64::Engine as _;
use flate2::{Compression, write::ZlibEncoder};
use nsh_core::{Command, JsonValue};
use nsh_platform_tui::NativeComponent;
use ratatui_core::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};
use tempfile::TempDir;

static NEXT_FRAME_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoMode {
    /// Crisp pixel video via Kitty Graphics Protocol (Ghostty, Kitty, WezTerm).
    Kitty,
    /// Render live video inside the terminal via ffmpeg RGB24 -> half-block (▀).
    Inline,
    /// Pop up a separate native mpv window on the local desktop.
    Popup,
    /// Audio/video disabled (headless guide & DVR browsing).
    None,
}

pub fn is_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

fn is_allowed_stream_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && !url.contains('\0')
        && !url.contains('\r')
        && !url.contains('\n')
}

impl VideoMode {
    pub fn detect_default() -> Self {
        if std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
            || std::env::var_os("KITTY_WINDOW_ID").is_some()
            || std::env::var_os("WEZTERM_EXECUTABLE").is_some()
            || std::env::var("TERM")
                .map(|t| t.contains("kitty") || t.contains("ghostty"))
                .unwrap_or(false)
        {
            Self::Kitty
        } else {
            Self::Inline
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Kitty if is_ssh() => "kitty-ssh",
            Self::Kitty => "kitty-hd",
            Self::Inline => "inline",
            Self::Popup => "popup",
            Self::None => "none",
        }
    }
}

pub struct RgbFrame {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
    pub seq: u64,
}

/// Shared state between `TuiPlayer` (mounted inside NSH) and the outer terminal
/// loop so Kitty graphics sequences can be emitted cleanly right after each
/// Ratatui `terminal.draw(...)` commit.
pub struct KittyOverlay {
    frame: Mutex<Option<Arc<RgbFrame>>>,
    rect: Mutex<Option<Rect>>,
    active: AtomicBool,
    last_sent_seq: AtomicU64,
    active_id: AtomicU64,
    last_rect: Mutex<Option<Rect>>,
    shm_dir: Option<TempDir>,
}

impl KittyOverlay {
    pub fn new() -> Arc<Self> {
        // Private 0700 directory in /dev/shm (Linux) when local; falls back to
        // inline t=d streaming on macOS/BSD or if /dev/shm is unavailable.
        let shm_dir = if !is_ssh() {
            tempfile::Builder::new()
                .prefix("patina-tui-")
                .tempdir_in("/dev/shm")
                .ok()
        } else {
            None
        };
        Arc::new(Self {
            frame: Mutex::new(None),
            rect: Mutex::new(None),
            active: AtomicBool::new(false),
            last_sent_seq: AtomicU64::new(0),
            active_id: AtomicU64::new(0),
            last_rect: Mutex::new(None),
            shm_dir,
        })
    }

    fn shm_path(&self, slot: u64) -> Option<PathBuf> {
        self.shm_dir
            .as_ref()
            .map(|dir| dir.path().join(format!("frame-{}.rgb", slot & 1)))
    }

    /// Emits Kitty Graphics APC escape sequence with double-buffered image IDs
    /// (42 / 43) and DEC synchronized output so frame swaps are atomic with
    /// zero flashing, both locally and over SSH.
    pub fn flush(&self, out: &mut impl Write) -> io::Result<()> {
        if !self.active.load(Ordering::Relaxed) {
            let prev_id = self.active_id.swap(0, Ordering::Relaxed);
            if prev_id != 0 {
                write!(out, "\x1b_Ga=d,d=I,i={prev_id},q=2\x1b\\")?;
                out.flush()?;
            }
            self.last_sent_seq.store(0, Ordering::Relaxed);
            return Ok(());
        }
        let rect = match self.rect.lock().ok().and_then(|r| *r) {
            Some(r) if r.width > 0 && r.height > 0 => r,
            _ => return Ok(()),
        };
        let frame = match self.frame.lock().ok().and_then(|f| f.clone()) {
            Some(f) => f,
            None => {
                // Clear any previous channel's frozen image while buffering.
                let prev_id = self.active_id.swap(0, Ordering::Relaxed);
                if prev_id != 0 {
                    write!(out, "\x1b_Ga=d,d=I,i={prev_id},q=2\x1b\\")?;
                    out.flush()?;
                }
                return Ok(());
            }
        };
        let prev_rect = self.last_rect.lock().ok().and_then(|r| *r);
        let prev_seq = self.last_sent_seq.load(Ordering::Relaxed);
        if frame.seq == prev_seq && prev_rect == Some(rect) {
            return Ok(());
        }

        let prev_id = self.active_id.load(Ordering::Relaxed);
        let next_id = if prev_id == 42 { 43 } else { 42 };

        // Prepare payload completely BEFORE opening DEC synchronized output.
        enum Payload {
            Shm(String),
            ZlibB64(String),
        }
        let payload = if let Some(path) = self.shm_path(next_id) {
            if fs::write(&path, &frame.data).is_err() {
                return Ok(());
            }
            Payload::Shm(
                base64::engine::general_purpose::STANDARD.encode(path.to_string_lossy().as_bytes()),
            )
        } else {
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
            if enc.write_all(&frame.data).is_err() {
                return Ok(());
            }
            let Ok(compressed) = enc.finish() else {
                return Ok(());
            };
            Payload::ZlibB64(base64::engine::general_purpose::STANDARD.encode(&compressed))
        };

        write!(out, "\x1b[?2026h\x1b7\x1b[{};{}H", rect.y + 1, rect.x + 1)?;
        match payload {
            Payload::Shm(b64_path) => {
                write!(
                    out,
                    "\x1b_Ga=T,t=f,f=24,s={},v={},c={},r={},i={next_id},z=-1,q=2,C=1;{b64_path}\x1b\\",
                    frame.width, frame.height, rect.width, rect.height,
                )?;
            }
            Payload::ZlibB64(b64) => {
                let bytes = b64.as_bytes();
                let mut offset = 0usize;
                while offset < bytes.len() {
                    let end = (offset + 4096).min(bytes.len());
                    let more = if end < bytes.len() { 1 } else { 0 };
                    let chunk = std::str::from_utf8(&bytes[offset..end]).unwrap_or("");
                    if offset == 0 {
                        write!(
                            out,
                            "\x1b_Ga=T,t=d,f=24,o=z,s={},v={},c={},r={},i={next_id},z=-1,q=2,C=1,m={more};{chunk}\x1b\\",
                            frame.width, frame.height, rect.width, rect.height,
                        )?;
                    } else {
                        write!(out, "\x1b_Gm={more},q=2;{chunk}\x1b\\")?;
                    }
                    offset = end;
                }
            }
        }
        // Delete previous image ID only AFTER next_id is transmitted and placed.
        if prev_id != 0 {
            write!(out, "\x1b_Ga=d,d=I,i={prev_id},q=2\x1b\\")?;
        }
        write!(out, "\x1b8\x1b[?2026l")?;
        out.flush()?;

        self.active_id.store(next_id, Ordering::Relaxed);
        self.last_sent_seq.store(frame.seq, Ordering::Relaxed);
        if let Ok(mut lr) = self.last_rect.lock() {
            *lr = Some(rect);
        }
        Ok(())
    }

    pub fn clear(&self, out: &mut impl Write) -> io::Result<()> {
        write!(
            out,
            "\x1b_Ga=d,d=I,i=42,q=2\x1b\\\x1b_Ga=d,d=I,i=43,q=2\x1b\\"
        )?;
        out.flush()
    }
}

pub struct TuiPlayer {
    mode: VideoMode,
    kitty: Arc<KittyOverlay>,
    current_url: Option<String>,
    paused: bool,
    muted: bool,
    frame: Arc<Mutex<Option<Arc<RgbFrame>>>>,
    stop_flag: Arc<AtomicBool>,
    child: Option<Child>,
    reader_thread: Option<JoinHandle<()>>,
    last_area: (u16, u16),
    is_fullscreen: bool,
}

impl TuiPlayer {
    pub fn new(mode: VideoMode, kitty: Arc<KittyOverlay>) -> Self {
        Self {
            mode,
            kitty,
            current_url: None,
            paused: false,
            muted: false,
            frame: Arc::new(Mutex::new(None)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            child: None,
            reader_thread: None,
            last_area: (80, 24),
            is_fullscreen: false,
        }
    }

    fn stop_playback(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
        if let Ok(mut f) = self.frame.lock() {
            *f = None;
        }
        self.kitty.active.store(false, Ordering::Relaxed);
        if let Ok(mut f) = self.kitty.frame.lock() {
            *f = None;
        }
    }

    fn reap_child(&mut self) {
        if let Some(child) = self.child.as_mut()
            && matches!(child.try_wait(), Ok(Some(_)))
        {
            self.child = None;
            if let Some(handle) = self.reader_thread.take() {
                let _ = handle.join();
            }
        }
    }

    fn start_playback(&mut self) {
        self.stop_playback();
        if self.paused {
            return;
        }
        let Some(url) = self.current_url.clone() else {
            return;
        };
        if !is_allowed_stream_url(&url) {
            log::warn!("patina-tui: rejected unsupported stream URL scheme");
            return;
        }
        match self.mode {
            VideoMode::None => {}
            VideoMode::Popup => {
                let mut cmd = ProcessCommand::new("mpv");
                cmd.args([
                    "--no-terminal",
                    "--force-window=yes",
                    "--keep-open=yes",
                    "--title=Patina",
                ]);
                if self.muted {
                    cmd.arg("--mute=yes");
                }
                // `--` prevents option injection from untrusted playlist URLs.
                cmd.arg("--")
                    .arg(&url)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                match cmd.spawn() {
                    Ok(child) => self.child = Some(child),
                    Err(err) => log::warn!("patina-tui: failed to spawn mpv: {err}"),
                }
            }
            VideoMode::Kitty | VideoMode::Inline => {
                if !self.is_fullscreen {
                    return;
                }
                let (w, h, fps) = if self.mode == VideoMode::Kitty {
                    if is_ssh() {
                        (1280usize, 720usize, 20)
                    } else {
                        (1280usize, 720usize, 30)
                    }
                } else {
                    let (cols, rows) = self.last_area;
                    (
                        usize::from(cols.clamp(32, 240)),
                        usize::from(rows.clamp(12, 100)) * 2,
                        15,
                    )
                };
                let mut cmd = ProcessCommand::new("ffmpeg");
                cmd.args([
                    "-loglevel",
                    "quiet",
                    "-protocol_whitelist",
                    "http,https,tcp,tls,crypto,hls",
                    "-re",
                    "-i",
                    &url,
                    "-vf",
                    &format!("scale={w}:{h}:flags=lanczos,fps={fps}"),
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgb24",
                    "-",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());

                match cmd.spawn() {
                    Ok(mut child) => {
                        if let Some(mut stdout) = child.stdout.take() {
                            let frame_arc = self.frame.clone();
                            let kitty_arc = self.kitty.clone();
                            let is_kitty = self.mode == VideoMode::Kitty;
                            let stop = Arc::new(AtomicBool::new(false));
                            self.stop_flag = stop.clone();
                            if is_kitty {
                                kitty_arc.active.store(true, Ordering::Relaxed);
                            }
                            self.reader_thread = Some(thread::spawn(move || {
                                let frame_bytes = w * h * 3;
                                let mut buf = vec![0u8; frame_bytes];
                                while !stop.load(Ordering::Relaxed) {
                                    if stdout.read_exact(&mut buf).is_err() {
                                        break;
                                    }
                                    if stop.load(Ordering::Relaxed) {
                                        break;
                                    }
                                    let seq = NEXT_FRAME_SEQ.fetch_add(1, Ordering::Relaxed);
                                    let frame = Arc::new(RgbFrame {
                                        width: w,
                                        height: h,
                                        data: buf.clone(),
                                        seq,
                                    });
                                    if is_kitty {
                                        if let Ok(mut guard) = kitty_arc.frame.lock() {
                                            *guard = Some(frame);
                                        }
                                    } else if let Ok(mut guard) = frame_arc.lock() {
                                        *guard = Some(frame);
                                    }
                                }
                            }));
                        }
                        self.child = Some(child);
                    }
                    Err(err) => log::warn!("patina-tui: failed to spawn ffmpeg: {err}"),
                }
            }
        }
    }
}

impl Drop for TuiPlayer {
    fn drop(&mut self) {
        self.stop_playback();
    }
}

impl NativeComponent for TuiPlayer {
    fn render(&mut self, area: Rect, buffer: &mut Buffer) {
        self.reap_child();
        if area.width == 0 || area.height == 0 {
            self.kitty.active.store(false, Ordering::Relaxed);
            return;
        }
        let is_fullscreen = area.height >= buffer.area.height.saturating_sub(4)
            && area.width >= buffer.area.width.saturating_sub(4);

        let mode_changed = is_fullscreen != self.is_fullscreen;
        self.is_fullscreen = is_fullscreen;

        // No preview player in Guide mode for terminal video modes; keep popup mpv alive.
        if !is_fullscreen {
            if mode_changed && self.mode != VideoMode::Popup {
                self.stop_playback();
            }
            self.kitty.active.store(false, Ordering::Relaxed);
            return;
        }

        let draw_area = area;
        self.last_area = (draw_area.width, draw_area.height);
        if mode_changed && self.mode != VideoMode::Popup && self.current_url.is_some() {
            self.start_playback();
        }

        // For Kitty graphics with z=-1, cells must use Color::Reset (default
        // background) so the image is visible behind the cell layer rather than
        // occluded by an opaque RGB cell background.
        let bg = if self.mode == VideoMode::Kitty && self.current_url.is_some() && !self.paused {
            Style::default().bg(Color::Reset).fg(Color::Reset)
        } else {
            Style::default()
                .bg(Color::Rgb(12, 14, 18))
                .fg(Color::Rgb(165, 178, 196))
        };
        for y in draw_area.top()..draw_area.bottom() {
            for x in draw_area.left()..draw_area.right() {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_char(' ').set_style(bg);
                }
            }
        }

        if self.mode == VideoMode::Kitty {
            if let Ok(mut r) = self.kitty.rect.lock() {
                *r = Some(draw_area);
            }
            if self.current_url.is_some() && !self.paused {
                self.kitty.active.store(true, Ordering::Relaxed);
                return;
            }
        }

        if self.mode == VideoMode::Inline
            && let Ok(guard) = self.frame.lock()
            && let Some(frame) = guard.as_ref()
        {
            for dy in 0..draw_area.height {
                let y_top =
                    (usize::from(dy) * 2 * frame.height) / (usize::from(draw_area.height) * 2);
                let y_bot = ((usize::from(dy) * 2 + 1) * frame.height)
                    / (usize::from(draw_area.height) * 2);
                let y_top = y_top.min(frame.height.saturating_sub(1));
                let y_bot = y_bot.min(frame.height.saturating_sub(1));
                for dx in 0..draw_area.width {
                    let fx = (usize::from(dx) * frame.width) / usize::from(draw_area.width);
                    let fx = fx.min(frame.width.saturating_sub(1));
                    let i_top = (y_top * frame.width + fx) * 3;
                    let i_bot = (y_bot * frame.width + fx) * 3;
                    let fg_c = Color::Rgb(
                        frame.data[i_top],
                        frame.data[i_top + 1],
                        frame.data[i_top + 2],
                    );
                    let bg_c = Color::Rgb(
                        frame.data[i_bot],
                        frame.data[i_bot + 1],
                        frame.data[i_bot + 2],
                    );
                    if let Some(cell) =
                        buffer.cell_mut((draw_area.left() + dx, draw_area.top() + dy))
                    {
                        cell.set_char('▀')
                            .set_style(Style::default().fg(fg_c).bg(bg_c));
                    }
                }
            }
            return;
        }

        let label = match (self.mode, &self.current_url, self.paused) {
            (_, None, _) => "[No stream]",
            (_, Some(_), true) => "[Paused]",
            (VideoMode::Popup, Some(_), false) => "[Playing in mpv]",
            (VideoMode::Inline, Some(_), false) => "[Buffering...]",
            (VideoMode::Kitty, Some(_), false) | (VideoMode::None, Some(_), false) => "[Video off]",
        };
        let cy = draw_area.top() + draw_area.height / 2;
        let max_len = usize::from(draw_area.width);
        let shown: String = label.chars().take(max_len).collect();
        let start_x = draw_area.left() + draw_area.width.saturating_sub(shown.len() as u16) / 2;
        for (i, ch) in shown.chars().enumerate() {
            let x = start_x + i as u16;
            if x < draw_area.right()
                && let Some(cell) = buffer.cell_mut((x, cy))
            {
                cell.set_char(ch).set_style(bg);
            }
        }
    }

    fn set_props(&mut self, props: &[(Arc<str>, Option<JsonValue>)]) {
        let mut restart = false;
        for (key, value) in props {
            match key.as_ref() {
                "source" => {
                    let new_url = value
                        .as_ref()
                        .and_then(|v| v.as_object())
                        .and_then(|obj| obj.get("url"))
                        .and_then(|u| u.as_str())
                        .map(ToOwned::to_owned);
                    if new_url != self.current_url {
                        self.current_url = new_url;
                        restart = true;
                    }
                }
                "paused" => {
                    let p = value.as_ref().and_then(|v| v.as_bool()).unwrap_or(false);
                    if p != self.paused {
                        self.paused = p;
                        restart = true;
                    }
                }
                "muted" => {
                    let m = value.as_ref().and_then(|v| v.as_bool()).unwrap_or(false);
                    if m != self.muted {
                        self.muted = m;
                        if self.mode == VideoMode::Popup {
                            restart = true;
                        }
                    }
                }
                _ => {}
            }
        }
        if restart {
            self.start_playback();
        }
    }

    fn command(&mut self, _command: &Command) -> bool {
        false
    }
}
