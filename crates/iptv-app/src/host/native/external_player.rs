use iptv_core::playback::PlaySource;
use iptv_net::playback::{ExternalAdmission, PlaybackRelay};
use std::sync::{Arc, Mutex};

pub(crate) const EXTERNAL_PLAYER: bool = cfg!(any(target_os = "android", target_os = "linux"));

struct Connection {
    relay: PlaybackRelay,
    lease: Option<Box<dyn Send>>,
}

#[derive(Clone)]
pub(crate) struct ExternalSession(Arc<Mutex<Option<Connection>>>);

impl ExternalSession {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn new(relay: PlaybackRelay) -> Self {
        Self(Arc::new(Mutex::new(Some(Connection {
            relay,
            lease: None,
        }))))
    }

    pub(crate) fn is_open(&self) -> bool {
        self.0.lock().is_ok_and(|connection| connection.is_some())
    }

    fn take(&self) -> Option<Connection> {
        self.0.lock().ok()?.take()
    }

    #[cfg(target_os = "linux")]
    fn close_blocking(&self) {
        if let Some(Connection { relay, lease }) = self.take() {
            relay.shutdown_blocking();
            drop(lease);
        }
    }

    pub(crate) fn close(&self) {
        if let Some(Connection { relay, lease }) = self.take() {
            let _ = super::background("iptv-retire-external-player", move || {
                relay.shutdown_blocking();
                drop(lease);
            });
        }
    }
}

pub(crate) struct ExternalPlayback(PlaybackRelay, ExternalAdmission);
pub(crate) struct ExternalHandoff {
    admission: ExternalAdmission,
    session: ExternalSession,
}

impl ExternalHandoff {
    pub(crate) async fn connected(&mut self) -> bool {
        self.admission.connected().await
    }

    pub(crate) fn is_open(&self) -> bool {
        self.session.is_open()
    }

    pub(crate) fn allow(self, lease: Box<dyn Send>) -> Option<ExternalSession> {
        {
            let mut connection = self.session.0.lock().ok()?;
            connection.as_mut()?.lease = Some(lease);
            self.admission.allow();
        }
        Some(self.session)
    }
}

pub(crate) async fn prepare_external_player(source: PlaySource) -> Option<ExternalPlayback> {
    if !EXTERNAL_PLAYER {
        return None;
    }
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
pub(crate) fn open_external_player(playback: ExternalPlayback) -> Option<ExternalHandoff> {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let ExternalPlayback(relay, admission) = playback;
    // The protected playlist keeps the stream capability out of argv.
    let mut playlist = tempfile::Builder::new().suffix(".m3u").tempfile().ok()?;
    writeln!(
        playlist,
        "#EXTM3U
{}",
        relay.url()
    )
    .ok()?;
    let mut mpv_playlist = std::ffi::OsString::from("--playlist=");
    mpv_playlist.push(playlist.path());
    for (app, args) in [
        (
            "vlc",
            vec![
                std::ffi::OsString::from("--no-one-instance"),
                std::ffi::OsString::from("--"),
                playlist.path().into(),
            ],
        ),
        ("mpv", vec![mpv_playlist]),
    ] {
        let child = Command::new(app)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            let session = ExternalSession::new(relay);
            let player = session.clone();
            super::background("iptv-external-player", move || {
                let _ = child.wait();
                drop(playlist);
                player.close_blocking();
            })
            .ok()?;
            return Some(ExternalHandoff { admission, session });
        }
    }
    None
}

#[cfg(target_os = "android")]
mod android {
    use super::{ExternalHandoff, ExternalPlayback, ExternalSession};
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

    pub(crate) fn open_external_player(playback: ExternalPlayback) -> Option<ExternalHandoff> {
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
            Some(ExternalHandoff { admission, session })
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
pub(crate) use android::{external_player_returned, open_external_player};

#[cfg(not(target_os = "android"))]
pub(crate) fn external_player_returned() {}

#[cfg(not(any(target_os = "android", target_os = "linux")))]
pub(crate) fn open_external_player(playback: ExternalPlayback) -> Option<ExternalHandoff> {
    drop(playback.0);
    None
}
