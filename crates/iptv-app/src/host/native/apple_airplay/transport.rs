//! Runtime-owned AirPlay relay and the main-thread player's command mailbox.
use iptv_core::{
    cast::{Command, Error, RemoteState},
    playback::PlaySource,
};
use iptv_net::playback::{ExternalAdmission, PlaybackRelay};
use iptv_runtime::cast::{Admitted, Authorization, NativePort, Prepared, Remote, Retirement, Task};
use std::{
    collections::{HashMap, VecDeque},
    ffi::CStr,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::oneshot;

unsafe extern "C" {
    fn patina_airplay_interface(buffer: *mut std::ffi::c_char, length: usize) -> bool;
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static VIEWS: OnceLock<Mutex<HashMap<u64, Weak<Bridge>>>> = OnceLock::new();
fn views() -> &'static Mutex<HashMap<u64, Weak<Bridge>>> {
    VIEWS.get_or_init(Mutex::default)
}

pub(crate) struct Sender {
    bridge: Arc<Bridge>,
}
impl Sender {
    pub(crate) fn new() -> Self {
        let bridge = Arc::new(Bridge {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            attached: AtomicBool::new(false),
            inner: Mutex::new(State::default()),
        });
        let mut registry = views().lock().expect("AirPlay view registry");
        registry.retain(|_, view| view.strong_count() != 0);
        registry.insert(bridge.id, Arc::downgrade(&bridge));
        Self { bridge }
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        self.bridge.stop_current();
    }
}

#[derive(Default)]
struct State {
    generation: u64,
    preparing: bool,
    cancelled: bool,
    status: u8,
    external: bool,
    commands: VecDeque<Message>,
}
pub(super) enum Message {
    Prepare {
        generation: u64,
        url: String,
    },
    Command {
        generation: u64,
        command: Command,
        reply: oneshot::Sender<()>,
    },
    Stop {
        generation: u64,
        disconnect: bool,
    },
    /// Acknowledge all preceding main-thread player work.
    Flushed(oneshot::Sender<()>),
}
pub(super) struct Bridge {
    id: u64,
    attached: AtomicBool,
    inner: Mutex<State>,
}
impl Bridge {
    pub(super) fn attach(id: u64) -> Option<Arc<Self>> {
        let bridge = views().lock().ok()?.get(&id)?.upgrade()?;
        (!bridge.attached.swap(true, Ordering::AcqRel)).then_some(bridge)
    }
    pub(super) fn detach(&self) {
        self.stop_current();
        self.attached.store(false, Ordering::Release);
        // The departing view is invalidated on this same main-thread turn.
        // Release waiters whose queued commands can no longer be delivered.
        if let Ok(mut state) = self.inner.lock() {
            state.commands.clear();
        }
    }
    pub(super) fn drain(&self) -> Vec<Message> {
        self.inner
            .lock()
            .map(|mut s| s.commands.drain(..).collect())
            .unwrap_or_default()
    }
    fn begin(&self) -> Result<u64, Error> {
        let mut state = self.inner.lock().map_err(|_| Error::Unavailable)?;
        state.generation = state.generation.wrapping_add(1);
        state.cancelled = false;
        state.preparing = true;
        state.status = 1;
        state.external = false;
        Ok(state.generation)
    }
    fn prepare_player(&self, generation: u64, url: String) -> bool {
        let Ok(mut state) = self.inner.lock() else {
            return false;
        };
        if state.generation != generation || state.cancelled {
            return false;
        }
        state
            .commands
            .push_back(Message::Prepare { generation, url });
        true
    }
    pub(super) fn accepts(&self, generation: u64) -> bool {
        self.inner
            .lock()
            .is_ok_and(|s| s.generation == generation && !s.cancelled)
    }
    fn ended_error(&self, generation: u64) -> Error {
        self.inner
            .lock()
            .map(|s| {
                if s.generation != generation {
                    Error::Denied
                } else if s.cancelled {
                    Error::Cancelled
                } else {
                    Error::Refused
                }
            })
            .unwrap_or(Error::Unavailable)
    }
    pub(super) fn current(&self, generation: u64) -> bool {
        self.inner.lock().is_ok_and(|s| s.generation == generation)
    }
    pub(super) fn publish(&self, generation: u64, external: bool, status: u8) {
        if let Ok(mut s) = self.inner.lock()
            && s.generation == generation
            && !s.cancelled
        {
            // A source replacement publishes idle synchronously before Play primes
            // the new item. That intermediate state cannot end the new session.
            if status != 0 {
                s.external = external;
                s.status = status;
            }
        }
    }
    fn state(&self, generation: u64) -> Result<RemoteState, Error> {
        if !self.attached.load(Ordering::Acquire) {
            return Ok(RemoteState::Ended);
        }
        let s = self.inner.lock().map_err(|_| Error::Unavailable)?;
        if s.generation != generation || s.cancelled || matches!(s.status, 4 | 5) {
            return Ok(RemoteState::Ended);
        }
        Ok(match (s.external, s.status) {
            (true, 2) => RemoteState::Playing,
            (true, 3) => RemoteState::Paused,
            _ => RemoteState::Loading,
        })
    }
    fn stop(&self, generation: u64, disconnect: bool) {
        if let Ok(mut s) = self.inner.lock()
            && s.generation == generation
            && !s.cancelled
        {
            s.cancelled = true;
            s.preparing = false;
            s.commands.push_back(Message::Stop {
                generation,
                disconnect,
            });
        }
    }
    fn flush(&self) -> Option<oneshot::Receiver<()>> {
        let mut state = self.inner.lock().ok()?;
        if !self.attached.load(Ordering::Acquire) {
            return None;
        }
        let (send, receive) = oneshot::channel();
        state.commands.push_back(Message::Flushed(send));
        Some(receive)
    }
    async fn flush_main_thread(&self) {
        if let Some(flushed) = self.flush() {
            let _ = flushed.await;
        }
    }
    fn stop_current(&self) {
        let generation = self.inner.lock().map(|s| s.generation).unwrap_or(0);
        self.stop(generation, true);
    }
}
impl NativePort for Sender {
    fn view_id(&self) -> Option<u64> {
        Some(self.bridge.id)
    }
    fn cancel_pending(&self) {
        let generation = self
            .bridge
            .inner
            .lock()
            .ok()
            .filter(|s| s.preparing)
            .map(|s| s.generation);
        if let Some(generation) = generation {
            self.bridge.stop(generation, true);
        }
    }
    fn flush(&self) -> Retirement<'_> {
        Box::pin(self.bridge.flush_main_thread())
    }
    fn prepare(&self, source: PlaySource) -> Task<'_, Box<dyn Prepared>> {
        Box::pin(async move {
            if !self.bridge.attached.load(Ordering::Acquire) {
                return Err(Error::Unavailable);
            }
            // Mark the intent before interface/relay setup yields, so Cancel
            // also covers a click made during that blocking setup.
            let generation = self.bridge.begin()?;
            let result = tokio::task::spawn_blocking(move || {
                let mut address = [0_i8; 64];
                // SAFETY: the native helper only writes a terminated IPv4 address
                // into the supplied buffer, using public interface APIs.
                if !unsafe { patina_airplay_interface(address.as_mut_ptr(), address.len()) } {
                    return Err(Error::Unavailable);
                }
                // SAFETY: a successful helper call guarantees a trailing NUL.
                let ip: IpAddr = unsafe { CStr::from_ptr(address.as_ptr()) }
                    .to_str()
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .ok_or(Error::Unavailable)?;
                PlaybackRelay::start_airplay(
                    &source.url,
                    &source.headers,
                    super::super::transport::policy(source.public_only),
                    SocketAddr::new(ip, 0),
                )
                .map_err(|_| Error::Unavailable)
            })
            .await
            .map_err(|_| Error::Unavailable)
            .and_then(|result| result);
            let (relay, gate) = match result {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.bridge.stop(generation, true);
                    return Err(error);
                }
            };
            if !self
                .bridge
                .prepare_player(generation, relay.url().to_owned())
            {
                relay.shutdown().await;
                return Err(Error::Cancelled);
            }
            Ok(Box::new(Handoff {
                bridge: self.bridge.clone(),
                generation,
                relay: Some(relay),
                gate: Some(gate),
                committed: false,
                guard: None,
            }) as Box<dyn Prepared>)
        })
    }
}
struct Handoff {
    bridge: Arc<Bridge>,
    generation: u64,
    relay: Option<PlaybackRelay>,
    gate: Option<ExternalAdmission>,
    committed: bool,
    guard: Option<Box<dyn Fn() -> bool + Send + Sync>>,
}
impl Drop for Handoff {
    fn drop(&mut self) {
        if !self.committed {
            self.bridge.stop(self.generation, true);
        }
    }
}
impl Handoff {
    fn check_guard(&self) -> Result<(), Error> {
        self.guard
            .as_ref()
            .is_some_and(|current| current())
            .then_some(())
            .ok_or(Error::Denied)
    }
    async fn admit(&mut self) -> Result<bool, Error> {
        self.check_guard()?;
        if self.bridge.state(self.generation)? == RemoteState::Ended {
            return Err(self.bridge.ended_error(self.generation));
        }
        if !self.gate.take().is_some_and(ExternalAdmission::allow) {
            return Err(Error::Denied);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            self.check_guard()?;
            match self.bridge.state(self.generation)? {
                RemoteState::Playing => return Ok(false),
                RemoteState::Paused => return Ok(true),
                RemoteState::Ended => return Err(self.bridge.ended_error(self.generation)),
                RemoteState::Loading => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Unknown);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
impl Prepared for Handoff {
    fn guard(&mut self, current: Box<dyn Fn() -> bool + Send + Sync>) {
        self.guard = Some(current);
    }
    fn load(&mut self) -> Task<'_, ()> {
        Box::pin(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
            loop {
                self.check_guard()?;
                if self.bridge.state(self.generation)? == RemoteState::Ended {
                    return Err(self.bridge.ended_error(self.generation));
                }
                let gate = self.gate.as_mut().ok_or(Error::Unavailable)?;
                tokio::select! {
                    connected = gate.connected() => return if connected { Ok(()) } else { Err(Error::NoRequest) },
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(Error::NoRequest);
                }
            }
        })
    }
    fn allow(mut self: Box<Self>, authorization: &Authorization) -> Task<'_, Admitted> {
        Box::pin(async move {
            let paused = match authorization.while_current(self.admit()).await {
                Ok(paused) => paused,
                Err(error) => {
                    self.bridge.stop(self.generation, true);
                    if let Some(relay) = self.relay.take() {
                        relay.shutdown().await;
                    }
                    return Err(error);
                }
            };
            if let Ok(mut s) = self.bridge.inner.lock()
                && s.generation == self.generation
            {
                s.preparing = false;
            }
            self.committed = true;
            Ok(Admitted {
                paused,
                remote: Arc::new(Playing {
                    bridge: self.bridge.clone(),
                    generation: self.generation,
                    relay: tokio::sync::Mutex::new(self.relay.take()),
                    disconnect_on_drop: AtomicBool::new(true),
                }),
            })
        })
    }
}
struct Playing {
    bridge: Arc<Bridge>,
    generation: u64,
    relay: tokio::sync::Mutex<Option<PlaybackRelay>>,
    disconnect_on_drop: AtomicBool,
}
impl Drop for Playing {
    fn drop(&mut self) {
        if self.disconnect_on_drop.load(Ordering::Acquire) {
            self.bridge.stop(self.generation, true);
        }
    }
}
impl Remote for Playing {
    fn command(&self, command: Command) -> Task<'_, ()> {
        Box::pin(async move {
            if self.bridge.state(self.generation)? == RemoteState::Ended {
                return Err(Error::Unavailable);
            }
            let (reply, received) = oneshot::channel();
            self.bridge
                .inner
                .lock()
                .map_err(|_| Error::Unavailable)?
                .commands
                .push_back(Message::Command {
                    generation: self.generation,
                    command,
                    reply,
                });
            tokio::time::timeout(Duration::from_secs(5), received)
                .await
                .map_err(|_| Error::Unknown)?
                .map_err(|_| Error::Unavailable)
        })
    }
    fn state(&self) -> Task<'_, RemoteState> {
        Box::pin(async move { self.bridge.state(self.generation) })
    }
    fn retire(&self) -> Retirement<'_> {
        Box::pin(async move {
            self.bridge.stop(self.generation, true);
            if let Some(relay) = self.relay.lock().await.take() {
                relay.shutdown().await;
            }
        })
    }
    fn stop(&self) -> Retirement<'_> {
        Box::pin(async move {
            self.retire().await;
            // In particular, let AVAudioSession deactivate before local mpv
            // reopens the channel and acquires that same session.
            self.bridge.flush_main_thread().await;
        })
    }
    fn retire_for_replacement(&self) -> Retirement<'_> {
        Box::pin(async move {
            self.disconnect_on_drop.store(false, Ordering::Release);
            self.bridge.stop(self.generation, false);
            if let Some(relay) = self.relay.lock().await.take() {
                relay.shutdown().await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_player_callbacks_and_queued_controls_cannot_restart_a_retired_session() {
        let sender = Sender::new();
        let bridge = &sender.bridge;
        bridge.attached.store(true, Ordering::Release);
        {
            let mut state = bridge.inner.lock().unwrap();
            state.generation = 7;
            state.status = 1;
        }
        bridge.publish(6, true, 2);
        assert_eq!(bridge.state(7), Ok(RemoteState::Loading));
        bridge.publish(7, true, 2);
        assert_eq!(bridge.state(7), Ok(RemoteState::Playing));
        bridge.stop(7, true);
        assert!(!bridge.accepts(7));
        bridge.publish(7, true, 2);
        assert_eq!(bridge.state(7), Ok(RemoteState::Ended));
        assert_eq!(bridge.ended_error(7), Error::Cancelled);
        {
            let mut state = bridge.inner.lock().unwrap();
            state.generation = 8;
            state.cancelled = false;
            state.status = 1;
            state.external = false;
        }
        bridge.stop(7, true);
        assert!(bridge.accepts(8));
        assert_eq!(bridge.state(8), Ok(RemoteState::Loading));
    }

    #[test]
    fn cancelling_during_relay_setup_cannot_later_prime_a_native_player() {
        let sender = Sender::new();
        let generation = sender.bridge.begin().unwrap();
        sender.cancel_pending();
        assert!(
            !sender
                .bridge
                .prepare_player(generation, "http://192.0.2.1/generated/stream.m3u8".into())
        );
        assert!(
            sender
                .bridge
                .drain()
                .iter()
                .all(|message| !matches!(message, Message::Prepare { .. }))
        );
    }

    #[test]
    fn each_runtime_has_one_native_view_and_cannot_attach_another_owners_player() {
        let first = Sender::new();
        let second = Sender::new();
        let view = Bridge::attach(first.bridge.id).unwrap();
        assert!(Bridge::attach(first.bridge.id).is_none());
        assert!(Bridge::attach(second.bridge.id).is_some());
        view.detach();
        assert!(Bridge::attach(first.bridge.id).is_some());
        assert!(Bridge::attach(0).is_none());
    }
    #[tokio::test]
    async fn retiring_a_channel_does_not_disconnect_the_route_when_the_old_remote_is_dropped() {
        let sender = Sender::new();
        sender.bridge.attached.store(true, Ordering::Release);
        let generation = sender.bridge.begin().unwrap();
        let remote = Playing {
            bridge: sender.bridge.clone(),
            generation,
            relay: tokio::sync::Mutex::new(None),
            disconnect_on_drop: AtomicBool::new(true),
        };
        remote.retire_for_replacement().await;
        drop(remote);
        let messages = sender.bridge.drain();
        assert_eq!(messages.len(), 1);
        assert!(matches!(
            messages[0],
            Message::Stop {
                disconnect: false,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn abandoning_an_unpublished_native_session_disconnects_its_route() {
        let sender = Sender::new();
        sender.bridge.attached.store(true, Ordering::Release);
        let generation = sender.bridge.begin().unwrap();
        let remote = Playing {
            bridge: sender.bridge.clone(),
            generation,
            relay: tokio::sync::Mutex::new(None),
            disconnect_on_drop: AtomicBool::new(true),
        };
        remote.retire().await;
        assert!(matches!(
            sender.bridge.drain().as_slice(),
            [Message::Stop {
                disconnect: true,
                ..
            }]
        ));
    }

    #[tokio::test]
    async fn explicit_stop_waits_for_the_main_thread_to_release_the_audio_session() {
        let sender = Sender::new();
        sender.bridge.attached.store(true, Ordering::Release);
        let generation = sender.bridge.begin().unwrap();
        let remote = Playing {
            bridge: sender.bridge.clone(),
            generation,
            relay: tokio::sync::Mutex::new(None),
            disconnect_on_drop: AtomicBool::new(true),
        };
        let mut stop = remote.stop();
        tokio::select! {
            biased;
            () = &mut stop => panic!("stop completed before the player consumed disconnect"),
            () = std::future::ready(()) => {}
        }
        let mut commands = sender.bridge.drain().into_iter();
        assert!(matches!(
            commands.next(),
            Some(Message::Stop {
                disconnect: true,
                ..
            })
        ));
        let Some(Message::Flushed(reply)) = commands.next() else {
            panic!("missing disconnect acknowledgement")
        };
        reply.send(()).unwrap();
        stop.await;
    }

    #[tokio::test]
    async fn revoked_authority_cannot_admit_a_primed_native_request() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (relay, mut gate) = PlaybackRelay::start_external(
            &format!(
                "http://{}/generated/index.m3u8",
                upstream.local_addr().unwrap()
            ),
            &[],
            iptv_net::playback::PlaybackPolicy {
                public_only: false,
                ..Default::default()
            },
        )
        .unwrap();
        let (authority, path) = relay
            .url()
            .strip_prefix("http://")
            .unwrap()
            .split_once('/')
            .unwrap();
        let (authority, path) = (authority.to_owned(), path.to_owned());
        let fetch = tokio::spawn(async move {
            let mut socket = tokio::net::TcpStream::connect(&authority).await.unwrap();
            socket
                .write_all(format!("GET /{path} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut response = Vec::new();
            let _ = socket.read_to_end(&mut response).await;
            response
        });
        assert!(
            tokio::time::timeout(Duration::from_secs(5), gate.connected())
                .await
                .unwrap()
        );
        let sender = Sender::new();
        sender.bridge.attached.store(true, Ordering::Release);
        let generation = sender.bridge.begin().unwrap();
        let mut prepared = Box::new(Handoff {
            bridge: sender.bridge.clone(),
            generation,
            relay: Some(relay),
            gate: Some(gate),
            committed: false,
            guard: Some(Box::new(|| false)),
        });
        assert!(matches!(prepared.admit().await, Err(Error::Denied)));
        drop(prepared);
        assert!(!String::from_utf8_lossy(&fetch.await.unwrap()).starts_with("HTTP/1.1 200"));
        assert!(
            tokio::time::timeout(Duration::from_millis(80), upstream.accept())
                .await
                .is_err()
        );
    }
}
