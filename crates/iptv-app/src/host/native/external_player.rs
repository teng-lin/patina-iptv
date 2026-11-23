use iptv_core::playback::PlaySource;
use iptv_net::playback::{ExternalAdmission, PlaybackRelay};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub(crate) const EXTERNAL_PLAYER: bool = cfg!(any(target_os = "android", target_os = "linux"));

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
        io::Write,
        process::{Command, Stdio},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    enum Player {
        Vlc,
        Mpv,
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
        mut launch: impl FnMut(ExternalPlayback, Player) -> Option<(ExternalHandoff, Arc<AtomicBool>)>,
    ) -> Option<ExternalHandoff> {
        for player in [Player::Vlc, Player::Mpv] {
            let playback = super::prepare_external_player(source.clone()).await?;
            if !before_launch() {
                return None;
            }
            let Some((mut handoff, exited)) = launch(playback, player) else {
                continue;
            };
            if handoff.connected().await && handoff.is_open() {
                return Some(handoff);
            }
            handoff.session.close_and_wait().await;
            // Fall back only when the process exited without reaching this
            // relay. A player that already selected the stream owns the choice.
            if !exited.load(Ordering::Acquire)
                || handoff
                    .admission
                    .as_ref()
                    .is_some_and(|admission| admission.received_request())
            {
                return None;
            }
        }
        None
    }

    fn launch(
        playback: ExternalPlayback,
        player: Player,
    ) -> Option<(ExternalHandoff, Arc<AtomicBool>)> {
        let ExternalPlayback(relay, admission) = playback;
        // The protected playlist keeps the stream capability out of argv.
        let mut playlist = tempfile::Builder::new().suffix(".m3u").tempfile().ok()?;
        writeln!(playlist, "#EXTM3U\n{}", relay.url()).ok()?;
        let mut command = match player {
            Player::Vlc => {
                let mut command = Command::new("vlc");
                command
                    .args(["--no-one-instance", "--"])
                    .arg(playlist.path());
                command
            }
            Player::Mpv => {
                let mut command = Command::new("mpv");
                let mut path = std::ffi::OsString::from("--playlist=");
                path.push(playlist.path());
                command.arg(path);
                command
            }
        };
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let session = ExternalSession::new(relay);
        let watched_session = session.clone();
        let exited = Arc::new(AtomicBool::new(false));
        let watched_exit = exited.clone();
        super::super::background("iptv-external-player", move || {
            let _ = child.wait();
            watched_exit.store(true, Ordering::Release);
            drop(playlist);
            watched_session.close_blocking();
        })
        .ok()?;
        Some((
            ExternalHandoff {
                admission: Some(admission),
                session,
            },
            exited,
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::net::TcpStream;

        #[test]
        fn external_player_retries_an_early_exit_with_a_fresh_capability() {
            assert_eq!(attempts(false), 2);
        }

        #[test]
        fn external_player_keeps_vlc_when_it_connects() {
            assert_eq!(attempts(true), 1);
        }

        fn attempts(vlc_connects: bool) -> usize {
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
                let handoff = connect_with(
                    source,
                    || true,
                    |playback, player| {
                        let ExternalPlayback(relay, admission) = playback;
                        let capability = relay.url().to_owned();
                        let session = ExternalSession::new(relay);
                        capabilities.push(capability.clone());
                        let exited = Arc::new(AtomicBool::new(false));
                        if matches!(player, Player::Vlc) && !vlc_connects {
                            exited.store(true, Ordering::Release);
                            session.close();
                        } else {
                            let (address, path) = capability
                                .strip_prefix("http://")
                                .unwrap()
                                .split_once('/')
                                .unwrap();
                            let mut stream = TcpStream::connect(address).unwrap();
                            write!(stream, "GET /{path} HTTP/1.1\r\nHost: {address}\r\n\r\n")
                                .unwrap();
                            receiver = Some(stream);
                        }
                        Some((
                            ExternalHandoff {
                                admission: Some(admission),
                                session,
                            },
                            exited,
                        ))
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
            let mut current = PLAYBACK.lock().ok()?;
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
