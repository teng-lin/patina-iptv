use iptv_core::playback::PlaySource;
use iptv_net::playback::{ExternalAdmission, PlaybackRelay};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub(crate) const EXTERNAL_PLAYER: bool = cfg!(any(target_os = "android", target_os = "linux"));

/// What the headless tests say about the handler, in place of the probe:
/// 0 probes, 1 reports none, 2 reports one.
#[cfg(feature = "test-support")]
static TEST_HANDLER: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Pretends a handler is (`Some(true)`) or is not (`Some(false)`) installed,
/// or probes again (`None`), for the headless tests.
#[cfg(feature = "test-support")]
pub(crate) fn set_external_player_handler(handler: Option<bool>) {
    let value = match handler {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    };
    TEST_HANDLER.store(value, std::sync::atomic::Ordering::Relaxed);
}

/// Whether another player could take a handoff now: an app that resolves
/// the handoff's intent on Android, VLC or mpv on Linux. The answer can
/// change while Patina runs (a player installed or removed), so callers ask
/// each time they offer or start a handoff rather than keeping it.
pub(crate) fn external_player_available() -> bool {
    #[cfg(feature = "test-support")]
    match TEST_HANDLER.load(std::sync::atomic::Ordering::Relaxed) {
        1 => return false,
        2 => return true,
        _ => {}
    }
    #[cfg(target_os = "android")]
    return android::handler_installed();
    #[cfg(target_os = "linux")]
    return linux::player_installed();
    #[allow(unreachable_code)]
    false
}

struct Connection {
    relay: PlaybackRelay,
    lease: Option<Box<dyn Send>>,
}

struct Session {
    connection: Mutex<Option<Connection>>,
    retired: watch::Sender<bool>,
}

struct Retirement {
    connection: Option<Connection>,
    session: Arc<Session>,
}

impl Drop for Retirement {
    fn drop(&mut self) {
        // The account slot stays occupied until its HTTP worker has joined.
        if let Some(Connection { relay, lease }) = self.connection.take() {
            relay.shutdown_blocking();
            drop(lease);
            // Lets Patina's process be cached again (`ExternalPlaybackService`).
            #[cfg(target_os = "android")]
            android::released();
        }
        self.session.retired.send_replace(true);
    }
}

#[derive(Clone)]
pub(crate) struct ExternalSession(Arc<Session>);

impl ExternalSession {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn new(relay: PlaybackRelay) -> Self {
        Self(Arc::new(Session {
            connection: Mutex::new(Some(Connection { relay, lease: None })),
            retired: watch::Sender::new(false),
        }))
    }

    pub(crate) fn is_open(&self) -> bool {
        self.0
            .connection
            .lock()
            .is_ok_and(|connection| connection.is_some())
    }

    pub(crate) fn is_retired(&self) -> bool {
        *self.0.retired.borrow()
    }

    fn retire(&self) -> Option<Retirement> {
        let connection = self
            .0
            .connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()?;
        Some(Retirement {
            connection: Some(connection),
            session: self.0.clone(),
        })
    }

    #[cfg(target_os = "linux")]
    fn close_blocking(&self) {
        drop(self.retire());
    }

    pub(crate) fn close(&self) {
        if let Some(retirement) = self.retire() {
            let _ = super::background("iptv-retire-external-player", move || drop(retirement));
        }
    }

    pub(crate) async fn close_and_wait(&self) {
        // Another caller may already be retiring this session.
        self.close();
        self.wait_retired().await;
    }

    pub(crate) async fn wait_retired(&self) {
        let mut retired = self.0.retired.subscribe();
        let _ = retired.wait_for(|done| *done).await;
    }
}

#[cfg(any(target_os = "android", target_os = "linux"))]
struct ExternalPlayback(PlaybackRelay, ExternalAdmission);
pub(crate) struct ExternalHandoff {
    admission: Option<ExternalAdmission>,
    session: ExternalSession,
}

impl Drop for ExternalHandoff {
    fn drop(&mut self) {
        if self.admission.is_some() {
            self.session.close();
        }
    }
}

impl ExternalHandoff {
    #[cfg(any(target_os = "android", target_os = "linux"))]
    async fn connected(&mut self) -> bool {
        match &mut self.admission {
            Some(admission) => admission.connected().await,
            None => false,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.session.is_open()
    }

    pub(crate) fn allow(mut self, lease: Box<dyn Send>) -> Option<ExternalSession> {
        let allowed = {
            let mut state = self.session.0.connection.lock().ok()?;
            let connection = state.as_mut()?;
            let allowed = self.admission.take()?.allow();
            if allowed {
                connection.lease = Some(lease);
            }
            allowed
        };
        if allowed {
            Some(self.session.clone())
        } else {
            self.session.close();
            None
        }
    }
}

#[cfg(any(target_os = "android", target_os = "linux"))]
async fn prepare_external_player(source: PlaySource) -> Option<ExternalPlayback> {
    let (send, receive) = futures_channel::oneshot::channel();
    super::background("iptv-external-playback", move || {
        let playback = PlaybackRelay::start_external(
            &source.url,
            &source.headers,
            super::transport::policy(source.public_only),
        )
        .ok()
        .map(|(relay, admission)| ExternalPlayback(relay, admission));
        let _ = send.send(playback);
    })
    .ok()?;
    receive.await.ok().flatten()
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{ExternalHandoff, ExternalPlayback, ExternalSession, PlaySource};
    use std::{
        io::{self, Write},
        process::{Child, Command, ExitStatus, Stdio},
        sync::mpsc,
        time::Duration,
    };

    enum Player {
        Vlc,
        Mpv,
    }

    impl Player {
        const ALL: [Self; 2] = [Self::Vlc, Self::Mpv];

        fn program(&self) -> &'static str {
            match self {
                Self::Vlc => "vlc",
                Self::Mpv => "mpv",
            }
        }
    }

    /// Whether VLC or mpv is a file on `PATH` that Patina may execute, the
    /// players a handoff launches.
    pub(super) fn player_installed() -> bool {
        std::env::var_os("PATH").is_some_and(|path| installed_in(&path))
    }

    fn installed_in(path: &std::ffi::OsStr) -> bool {
        std::env::split_paths(path).any(|dir| {
            Player::ALL
                .iter()
                .any(|player| executable(&dir.join(player.program())))
        })
    }

    /// A regular file this process may execute: `access(2)` checks the
    /// caller's own permission and a `noexec` mount, not just a mode bit.
    fn executable(file: &std::path::Path) -> bool {
        use std::os::unix::ffi::OsStrExt;
        if !std::fs::metadata(file).is_ok_and(|metadata| metadata.is_file()) {
            return false;
        }
        let Ok(file) = std::ffi::CString::new(file.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `file` is a NUL-terminated path that outlives the call.
        unsafe { libc::access(file.as_ptr(), libc::X_OK) == 0 }
    }

    struct LaunchedPlayer {
        handoff: ExternalHandoff,
        /// Send to let the player stay open. Dropping this stops an unclaimed player.
        keep: mpsc::Sender<()>,
    }

    pub(crate) async fn connect_external_player(
        source: PlaySource,
        before_launch: impl Fn() -> bool,
    ) -> Option<ExternalHandoff> {
        connect_with(source, before_launch, launch).await
    }

    async fn connect_with(
        source: PlaySource,
        before_launch: impl Fn() -> bool,
        mut launch: impl FnMut(ExternalPlayback, Player) -> Option<LaunchedPlayer>,
    ) -> Option<ExternalHandoff> {
        for player in Player::ALL {
            let playback = super::prepare_external_player(source.clone()).await?;
            if !before_launch() {
                return None;
            }
            let Some(LaunchedPlayer { mut handoff, keep }) = launch(playback, player) else {
                continue;
            };
            if handoff.connected().await && handoff.is_open() && keep.send(()).is_ok() {
                return Some(handoff);
            }
            // The watcher reaps the failed child before retiring the relay.
            drop(keep);
            handoff.session.wait_retired().await;
            if handoff
                .admission
                .as_ref()
                .is_some_and(|admission| admission.received_request())
            {
                // A player that reached the relay already claimed the launch.
                return None;
            }
        }
        None
    }

    fn launch(playback: ExternalPlayback, player: Player) -> Option<LaunchedPlayer> {
        let ExternalPlayback(relay, admission) = playback;
        // The protected playlist keeps the stream capability out of argv.
        let mut playlist = tempfile::Builder::new().suffix(".m3u").tempfile().ok()?;
        writeln!(playlist, "#EXTM3U\n{}", relay.url()).ok()?;
        let mut command = Command::new(player.program());
        match player {
            Player::Vlc => {
                command
                    .args(["--no-one-instance", "--"])
                    .arg(playlist.path());
            }
            Player::Mpv => {
                let mut path = std::ffi::OsString::from("--playlist=");
                path.push(playlist.path());
                command.arg(path);
            }
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let session = ExternalSession::new(relay);
        let watched_session = session.clone();
        // Starting the child inside its watcher gives it an owner even
        // when creating the thread fails.
        let keep = start_watcher(command, move || {
            drop(playlist);
            watched_session.close_blocking();
        })?;
        Some(LaunchedPlayer {
            handoff: ExternalHandoff {
                admission: Some(admission),
                session,
            },
            keep,
        })
    }

    fn start_watcher(
        mut command: Command,
        retired: impl FnOnce() + Send + 'static,
    ) -> Option<mpsc::Sender<()>> {
        let (keep, decision) = mpsc::channel();
        super::super::background("iptv-external-player", move || {
            if let Ok(child) = command.spawn()
                && let Err(error) = watch_child(child, decision)
            {
                log::warn!("iptv external player: waiting for child failed: {error}");
            }
            retired();
        })
        .ok()?;
        Some(keep)
    }

    fn watch_child(mut child: Child, decision: mpsc::Receiver<()>) -> io::Result<ExitStatus> {
        // Poll only while waiting for the relay's first request. After one
        // arrives, the external player owns its window and can exit normally.
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            match decision.recv_timeout(Duration::from_millis(100)) {
                Ok(()) => return child.wait(),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = child.kill();
                    return child.wait();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{net::TcpStream, os::unix::process::ExitStatusExt};

        #[derive(Clone, Copy)]
        enum VlcStartup {
            Exited,
            TimedOut,
            Connected,
        }

        #[test]
        fn only_an_executable_vlc_or_mpv_on_the_path_counts_as_a_player() {
            use std::os::unix::fs::PermissionsExt;
            let empty = tempfile::tempdir().unwrap();
            let other = tempfile::tempdir().unwrap();
            let found = tempfile::tempdir().unwrap();
            let program = |dir: &tempfile::TempDir, name: &str, mode: u32| {
                let file = dir.path().join(name);
                std::fs::write(&file, "#!/bin/sh\n").unwrap();
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            };
            program(&other, "generated-player", 0o755);
            program(&other, "vlc", 0o644);
            std::fs::create_dir(other.path().join("mpv")).unwrap();
            let path = |dirs: &[&tempfile::TempDir]| {
                std::env::join_paths(dirs.iter().map(|dir| dir.path())).unwrap()
            };
            assert!(!installed_in(&path(&[&empty, &other])));
            program(&found, "mpv", 0o755);
            assert!(installed_in(&path(&[&empty, &other, &found])));
            assert!(!installed_in(std::ffi::OsStr::new("")));
        }

        #[test]
        fn external_player_retries_an_early_exit_with_a_fresh_capability() {
            assert_eq!(attempts(VlcStartup::Exited), 2);
        }

        #[test]
        fn external_player_reaps_an_unclaimed_child_before_falling_back() {
            assert_eq!(attempts(VlcStartup::TimedOut), 2);
        }

        #[test]
        fn external_player_keeps_vlc_when_it_connects() {
            assert_eq!(attempts(VlcStartup::Connected), 1);
        }

        fn attempts(vlc: VlcStartup) -> usize {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            runtime.block_on(async {
                let source = PlaySource {
                    public_only: true,
                    url: "https://stream.example.test/generated.ts".to_owned(),
                    headers: Vec::new(),
                };
                let mut capabilities = Vec::new();
                let mut receiver = None;
                let mut reaped_vlc: Option<mpsc::Receiver<io::Result<ExitStatus>>> = None;
                let handoff = connect_with(
                    source,
                    || true,
                    |playback, player| {
                        let ExternalPlayback(relay, admission) = playback;
                        let capability = relay.url().to_owned();
                        let session = ExternalSession::new(relay);
                        capabilities.push(capability.clone());
                        let (keep, decision) = mpsc::channel();
                        let admission = match (player, vlc) {
                            (Player::Vlc, VlcStartup::Exited) => {
                                session.close();
                                Some(admission)
                            }
                            (Player::Vlc, VlcStartup::TimedOut) => {
                                let child = Command::new("sleep").arg("60").spawn().unwrap();
                                let watched = session.clone();
                                let (done, status) = mpsc::channel();
                                std::thread::spawn(move || {
                                    done.send(watch_child(child, decision)).unwrap();
                                    watched.close_blocking();
                                });
                                reaped_vlc = Some(status);
                                // Model the terminal admission result directly instead
                                // of waiting for the relay's real startup deadline.
                                None
                            }
                            _ => {
                                if let Some(status) = reaped_vlc.take() {
                                    assert_eq!(
                                        status.try_recv().unwrap().unwrap().signal(),
                                        Some(libc::SIGKILL)
                                    );
                                }
                                let (address, path) = capability
                                    .strip_prefix("http://")
                                    .unwrap()
                                    .split_once('/')
                                    .unwrap();
                                let mut stream = TcpStream::connect(address).unwrap();
                                write!(stream, "GET /{path} HTTP/1.1\r\nHost: {address}\r\n\r\n")
                                    .unwrap();
                                receiver = Some((stream, decision));
                                Some(admission)
                            }
                        };
                        Some(LaunchedPlayer {
                            handoff: ExternalHandoff { admission, session },
                            keep,
                        })
                    },
                )
                .await
                .expect("one synthetic player requested the relay");
                assert!(handoff.is_open());
                if capabilities.len() == 2 {
                    assert_ne!(capabilities[0], capabilities[1]);
                }
                handoff.session.close_and_wait().await;
                drop(receiver);
                capabilities.len()
            })
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::connect_external_player;

#[cfg(target_os = "android")]
mod android {
    use super::{ExternalHandoff, ExternalPlayback, ExternalSession, PlaySource};
    use jni::{
        JavaVM,
        objects::{GlobalRef, JClass, JValue},
    };
    use std::sync::{Mutex, OnceLock};

    static HOST: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();
    static PLAYBACK: Mutex<Option<ExternalSession>> = Mutex::new(None);

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_hantekllc_iptvrs_host_ExternalPlayerHost_initialize(
        env: jni::JNIEnv,
        class: JClass,
    ) {
        if let (Ok(vm), Ok(class)) = (env.get_java_vm(), env.new_global_ref(class)) {
            let _ = HOST.set((vm, class));
        }
    }

    /// Calls a static `ExternalPlayerHost` method that takes nothing and
    /// returns a boolean (`()Z`) or nothing (`()V`).
    fn call(name: &str, signature: &str) -> Option<bool> {
        let (vm, class) = HOST.get()?;
        let mut env = vm.attach_current_thread().ok()?;
        let class: &JClass = class.as_obj().into();
        let result = env.call_static_method(class, name, signature, &[]);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        result.ok().map(|value| value.z().unwrap_or(false))
    }

    /// Asks `ExternalPlayerHost.available` whether another app resolves the
    /// handoff's intent.
    pub(super) fn handler_installed() -> bool {
        call("available", "()Z") == Some(true)
    }

    /// Tells the host that a relay `open` started has shut down.
    pub(super) fn released() {
        let _ = call("released", "()V");
    }

    pub(crate) async fn connect_external_player(
        source: PlaySource,
        before_launch: impl Fn() -> bool,
    ) -> Option<ExternalHandoff> {
        let playback = super::prepare_external_player(source).await?;
        if !before_launch() {
            return None;
        }
        let mut handoff = open_external_player(playback)?;
        (handoff.connected().await && handoff.is_open()).then_some(handoff)
    }

    fn open_external_player(playback: ExternalPlayback) -> Option<ExternalHandoff> {
        let ExternalPlayback(relay, admission) = playback;
        let open = || {
            let (vm, class) = HOST.get()?;
            let mut env = vm.attach_current_thread().ok()?;
            let url = env.new_string(relay.url()).ok()?;
            let class: &JClass = class.as_obj().into();
            let result = env.call_static_method(
                class,
                "open",
                "(Ljava/lang/String;)Z",
                &[JValue::Object(&url)],
            );
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            result.ok()?.z().ok()
        };
        if open() == Some(true) {
            // The host now holds its service for this session: it must
            // exist, so its retirement releases the service.
            let mut current = PLAYBACK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let session = ExternalSession::new(relay);
            if let Some(previous) = current.replace(session.clone()) {
                previous.close();
            }
            Some(ExternalHandoff {
                admission: Some(admission),
                session,
            })
        } else {
            None
        }
    }

    pub(crate) fn external_player_returned() {
        if let Ok(mut current) = PLAYBACK.lock()
            && let Some(session) = current.take()
        {
            session.close();
        }
    }
}

#[cfg(target_os = "android")]
pub(crate) use android::{connect_external_player, external_player_returned};

#[cfg(not(target_os = "android"))]
pub(crate) fn external_player_returned() {}

#[cfg(not(any(target_os = "android", target_os = "linux")))]
pub(crate) async fn connect_external_player(
    _source: PlaySource,
    _before_launch: impl Fn() -> bool,
) -> Option<ExternalHandoff> {
    None
}
