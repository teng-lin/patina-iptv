//! Android LAN leases, media controls and Keystore callbacks.
use super::{Playing, finished};
use iptv_cast::{
    Availability, Credential, CredentialKey, CredentialStore, Feature, Link, Playback, StoreError,
    Updates,
};
use iptv_core::cast::{Command, Error};
use iptv_runtime::cast::Remote;
use jni::{
    JNIEnv, JavaVM,
    errors::Result as JniResult,
    objects::{GlobalRef, JByteArray, JClass, JString, JValue},
    sys::{jint, jlong},
};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        Arc, LazyLock, Mutex, OnceLock,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, watch};

static HOST: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_CastHost_initialize(
    env: JNIEnv,
    class: JClass,
) {
    if let (Ok(vm), Ok(class)) = (env.get_java_vm(), env.new_global_ref(class)) {
        let _ = HOST.set((vm, class));
    }
}

pub(in crate::host::native) fn registered() -> bool {
    HOST.get().is_some()
}

fn call<T>(operation: impl FnOnce(&mut JNIEnv<'_>, &JClass<'_>) -> JniResult<T>) -> Option<T> {
    let (vm, class) = HOST.get()?;
    let mut env = vm.attach_current_thread().ok()?;
    let class: &JClass = class.as_obj().into();
    let result = env.with_local_frame(8, |env| operation(env, class));
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    result.ok()
}

pub(super) fn topology() -> Result<Option<String>, Error> {
    call(|env, class| {
        let value = env
            .call_static_method(class, "topology", "()Ljava/lang/String;", &[])?
            .l()?;
        if value.is_null() {
            return Ok(None);
        }
        let value = JString::from(value);
        Ok(Some(env.get_string(&value)?.into()))
    })
    .ok_or(Error::Unavailable)
}

/// Android filters multicast on Wi-Fi until the app takes this lock. Keep
/// it for a bounded scan, with release on every exit including cancellation.
pub(super) struct Multicast(GlobalRef);

impl Multicast {
    pub(super) fn acquire() -> Result<Option<Self>, Error> {
        call(|env, class| {
            let lock = env
                .call_static_method(
                    class,
                    "multicast",
                    "()Landroid/net/wifi/WifiManager$MulticastLock;",
                    &[],
                )?
                .l()?;
            if lock.is_null() {
                return Ok(None);
            }
            env.new_global_ref(lock).map(Self).map(Some)
        })
        .ok_or(Error::Unavailable)
    }
}

impl Drop for Multicast {
    fn drop(&mut self) {
        let _ = call(|env, _| env.call_method(self.0.as_obj(), "release", "()V", &[])?.v());
    }
}

fn store_error() -> StoreError {
    StoreError::new(std::io::Error::other("Android pairing store unavailable"))
}

pub(super) struct CastStore;

impl CredentialStore for CastStore {
    async fn load(&self, key: &CredentialKey) -> Result<Option<Credential>, StoreError> {
        let key = key.storage_key().to_owned();
        let payload = tokio::task::spawn_blocking(move || {
            call(|env, class| {
                let key = env.new_string(key)?;
                let payload = env
                    .call_static_method(
                        class,
                        "loadCredential",
                        "(Ljava/lang/String;)[B",
                        &[JValue::Object(&key)],
                    )?
                    .l()?;
                let payload = JByteArray::from(payload);
                let bytes = env.convert_byte_array(&payload)?;
                if bytes.len() > 1 {
                    env.set_byte_array_region(&payload, 0, &vec![0; bytes.len()])?;
                }
                Ok(bytes)
            })
            .ok_or_else(store_error)
        })
        .await
        .map_err(|_| store_error())??;
        decode_credential(payload)
    }

    async fn save(&self, key: &CredentialKey, value: Credential) -> Result<(), StoreError> {
        let key = key.storage_key().to_owned();
        tokio::task::spawn_blocking(move || {
            call(|env, class| {
                let key = env.new_string(key)?;
                let value = env.byte_array_from_slice(value.as_bytes())?;
                env.call_static_method(
                    class,
                    "saveCredential",
                    "(Ljava/lang/String;[B)V",
                    &[JValue::Object(&key), JValue::Object(&value)],
                )?
                .v()
            })
            .ok_or_else(store_error)
        })
        .await
        .map_err(|_| store_error())?
    }

    async fn remove(&self, key: &CredentialKey) -> Result<(), StoreError> {
        let key = key.storage_key().to_owned();
        tokio::task::spawn_blocking(move || {
            call(|env, class| {
                let key = env.new_string(key)?;
                env.call_static_method(
                    class,
                    "forgetCredential",
                    "(Ljava/lang/String;)V",
                    &[JValue::Object(&key)],
                )?
                .v()
            })
            .ok_or_else(store_error)
        })
        .await
        .map_err(|_| store_error())?
    }
}

fn decode_credential(mut payload: Vec<u8>) -> Result<Option<Credential>, StoreError> {
    match payload.as_slice() {
        [0] => Ok(None),
        [1, value @ ..] if !value.is_empty() && value.len() <= 4096 => {
            // Retain the allocation instead of making a second plaintext copy.
            payload.remove(0);
            Ok(Some(Credential::from_bytes(payload)))
        }
        _ => {
            payload.fill(0);
            Err(StoreError::invalid_credential())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ServiceState {
    Pending,
    Ready,
    Stopped,
}

struct HostEvents {
    state: watch::Sender<ServiceState>,
    commands: mpsc::Sender<Command>,
}

impl HostEvents {
    fn notify(&self, event: jint) {
        match event {
            0 => {
                self.state.send_if_modified(|state| {
                    if *state != ServiceState::Pending {
                        return false;
                    }
                    *state = ServiceState::Ready;
                    true
                });
            }
            1 => {
                // Terminal events cannot be displaced by queued key repeats.
                self.state.send_replace(ServiceState::Stopped);
            }
            2 | 3 if *self.state.borrow() == ServiceState::Ready => {
                let command = if event == 2 {
                    Command::Play
                } else {
                    Command::Pause
                };
                let _ = self.commands.try_send(command);
            }
            _ => {}
        }
    }
}

static LEASES: LazyLock<Mutex<HashMap<jlong, HostEvents>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_LEASE: AtomicI64 = AtomicI64::new(1);

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_CastHost_playbackEvent(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
    event: jint,
) {
    if let Ok(leases) = LEASES.lock()
        && let Some(host) = leases.get(&id)
    {
        host.notify(event);
    }
}

pub(super) struct PlaybackLease {
    id: jlong,
    state: watch::Receiver<ServiceState>,
    commands: Option<mpsc::Receiver<Command>>,
}

impl PlaybackLease {
    pub(super) async fn start(address: IpAddr) -> Result<Self, Error> {
        let id = NEXT_LEASE.fetch_add(1, Ordering::Relaxed);
        let (state, events) = watch::channel(ServiceState::Pending);
        let (commands, input) = mpsc::channel(4);
        LEASES
            .lock()
            .map_err(|_| Error::Unavailable)?
            .insert(id, HostEvents { state, commands });
        let lease = Self {
            id,
            state: events,
            commands: Some(input),
        };
        let started = call(|env, class| {
            let address = env.new_string(address.to_string())?;
            env.call_static_method(
                class,
                "start",
                "(JLjava/lang/String;)Z",
                &[JValue::Long(id), JValue::Object(&address)],
            )?
            .z()
        });
        if started != Some(true) {
            return Err(Error::Unavailable);
        }
        // Do not submit LOAD unless Android has promoted the service and
        // verified the interface of this specific control connection.
        let mut state = lease.state();
        let acknowledged = tokio::time::timeout(
            Duration::from_secs(4),
            state.wait_for(|state| *state != ServiceState::Pending),
        )
        .await;
        if !matches!(acknowledged, Ok(Ok(state)) if *state == ServiceState::Ready) {
            return Err(Error::Unavailable);
        }
        Ok(lease)
    }

    pub(super) fn state(&self) -> watch::Receiver<ServiceState> {
        self.state.clone()
    }

    pub(super) fn activate(&mut self, remote: &Arc<Playing>, mut updates: Updates) {
        let Some(mut commands) = self.commands.take() else {
            return;
        };
        let mut state = self.state();
        let id = self.id;
        let remote = Arc::downgrade(remote);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    stop = stopped(&mut state) => {
                        if stop && let Some(remote) = remote.upgrade() {
                            let _ = remote.stop_checked().await;
                        }
                        break;
                    }
                    snapshot = updates.next() => {
                        let Some(remote) = remote.upgrade() else { break };
                        match snapshot {
                            Some(ref snapshot) if finished(snapshot) => {
                                remote.retire().await;
                                break;
                            }
                            Some(ref snapshot) if snapshot.link() != Link::Disconnected => {
                                let state = match snapshot.playback() {
                                    Playback::Playing => 1,
                                    Playback::Paused => 2,
                                    _ => 0,
                                };
                                let available = |feature| snapshot.availability(feature) == Availability::Available;
                                let controls = i32::from(available(Feature::Play))
                                    | (i32::from(available(Feature::Pause)) << 1);
                                update_playback(id, state, controls);
                            }
                            _ => {
                                let _ = remote.stop_checked().await;
                                break;
                            }
                        }
                    }
                    command = commands.recv() => {
                        let Some(command) = command else { break };
                        let Some(remote) = remote.upgrade() else { break };
                        tokio::select! {
                            stop = stopped(&mut state) => {
                                if stop {
                                    let _ = remote.stop_checked().await;
                                }
                                break;
                            }
                            _ = remote.command(command) => {}
                        }
                    }
                }
            }
        });
    }
}

impl Drop for PlaybackLease {
    fn drop(&mut self) {
        if let Ok(mut leases) = LEASES.lock() {
            leases.remove(&self.id);
        }
        let _ = call(|env, class| {
            env.call_static_method(class, "release", "(J)V", &[JValue::Long(self.id)])?
                .v()
        });
    }
}

pub(super) async fn stopped(state: &mut watch::Receiver<ServiceState>) -> bool {
    state
        .wait_for(|state| *state == ServiceState::Stopped)
        .await
        .is_ok()
}

fn update_playback(id: jlong, state: jint, controls: jint) {
    let _ = call(|env, class| {
        env.call_static_method(
            class,
            "playback",
            "(JII)V",
            &[JValue::Long(id), JValue::Int(state), JValue::Int(controls)],
        )?
        .v()
    });
}
