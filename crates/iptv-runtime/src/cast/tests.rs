use super::*;
use iptv_core::parental::Target;
use iptv_guide::playback::SourceAuthority;
use iptv_providers::{
    model::{M3uSourceConfig, SourceConfig},
    secret::SecretUrl,
};
use std::{
    task::{Context, Wake, Waker},
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(5);

struct Ready(mpsc::Sender<()>);
impl Wake for Ready {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send(());
    }
}

struct Handoff {
    started: mpsc::Sender<()>,
    allow: oneshot::Receiver<()>,
    remote: Arc<Relay>,
}
impl Prepared for Handoff {
    fn load(&mut self) -> Task<'_, ()> {
        unreachable!()
    }
    fn allow(self: Box<Self>) -> Task<'static, Admitted> {
        Box::pin(async move {
            self.started.send(()).unwrap();
            self.allow.await.unwrap();
            Ok(Admitted {
                remote: self.remote,
                paused: false,
            })
        })
    }
}

struct Relay {
    entering: mpsc::Sender<()>,
    close: Mutex<Option<oneshot::Receiver<()>>>,
    stop_reply: Mutex<Option<oneshot::Receiver<()>>>,
}
impl Remote for Relay {
    fn command(&self, command: Command) -> Task<'_, ()> {
        assert_eq!(command, Command::Stop);
        let reply = self.stop_reply.lock().unwrap().take().expect("one STOP");
        Box::pin(async {
            // Keep the control connection unresponsive until after the owner has drained.
            let _ = reply.await;
            Ok(())
        })
    }
    fn state(&self) -> Task<'_, RemoteState> {
        unreachable!()
    }
    fn retire(&self) -> Retirement<'_> {
        let close = self.close.lock().unwrap().take().expect("one retirement");
        Box::pin(async move {
            self.entering.send(()).unwrap();
            close.await.unwrap();
        })
    }
}

type Admission<'a> = Pin<Box<dyn Future<Output = Result<bool, Error>> + 'a>>;

struct Trial<'a> {
    admission: Option<Admission<'a>>,
    allow: Option<oneshot::Sender<()>>,
    notified: mpsc::Receiver<()>,
    waker: Waker,
    retirement: RetirementCheck,
    _stop_reply: oneshot::Sender<()>,
}

struct RetirementCheck {
    stopping: mpsc::Receiver<()>,
    release: oneshot::Sender<()>,
    released: mpsc::Receiver<()>,
}

impl RetirementCheck {
    fn finish(self) {
        self.stopping.recv_timeout(WAIT).unwrap();
        assert_eq!(
            self.released.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "retain claim while the upstream closes"
        );
        self.release.send(()).unwrap();
        assert_eq!(
            self.released.recv_timeout(WAIT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }
}

fn owner() -> Runtime {
    let owner = Runtime::new(crate::Ports::for_tests());
    owner.start_services();
    owner
}

impl<'a> Trial<'a> {
    fn begin(owner: &'a Runtime) -> Self {
        let source = SourceAuthority::new(
            1,
            "incarnation".into(),
            SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        );
        let catalog = iptv_providers::m3u::parse_m3u(
            "#EXTM3U\n#EXTINF:-1,Generated\nhttps://example.test/live.ts\n",
        );
        let authority = source.resolve(&catalog.channels.items[0]);
        let target = Target::live(1, authority.channel().id.as_str());
        // Apply the owner's initial policy, then take the same grant snapshot as a handoff.
        owner.parental_policy_changed();
        owner.parental.borrow_mut().access.unlock(
            target.clone(),
            crate::time::since_epoch().as_millis() as u64,
        );
        let grant = ExternalPlaybackAuthorization::new(
            owner.parental_snapshot(),
            target.clone(),
            vec![target],
        );
        assert!(grant.is_current());
        let (started, admitting) = mpsc::channel();
        let (allow, gate) = oneshot::channel();
        let (entering, stopping) = mpsc::channel();
        let (release, close) = oneshot::channel();
        let (stop_reply, wait_for_stop) = oneshot::channel();
        let prepared = Handoff {
            started,
            allow: gate,
            remote: Arc::new(Relay {
                entering,
                close: Mutex::new(Some(close)),
                stop_reply: Mutex::new(Some(wait_for_stop)),
            }),
        };
        let (claim, released) = mpsc::channel::<()>();
        let dest = Destination {
            receiver: Receiver {
                name: "Generated TV".into(),
                address: ([127, 0, 0, 1], 8009).into(),
            },
            key: "generated".into(),
            authority,
        };
        let admission = Box::pin(owner.admit_cast(
            owner.next_cast_intent(),
            Box::new(prepared),
            dest,
            Box::new(claim),
            grant,
        ));
        let (wake, notified) = mpsc::channel();
        let waker = Waker::from(Arc::new(Ready(wake)));
        let mut trial = Self {
            admission: Some(admission),
            allow: Some(allow),
            notified,
            waker,
            _stop_reply: stop_reply,
            retirement: RetirementCheck {
                stopping,
                release,
                released,
            },
        };
        assert!(trial.poll().is_pending());
        admitting.recv_timeout(WAIT).unwrap();
        trial
    }

    fn poll(&mut self) -> std::task::Poll<Result<bool, Error>> {
        self.admission
            .as_mut()
            .unwrap()
            .as_mut()
            .poll(&mut Context::from_waker(&self.waker))
    }

    fn offer(&mut self) {
        self.allow.take().unwrap().send(()).unwrap();
        self.notified.recv_timeout(WAIT).unwrap();
    }
}

#[test]
fn cancel_during_allow_keeps_the_claim_until_relay_shutdown() {
    let owner = owner();
    let mut trial = Trial::begin(&owner);
    drop(trial.admission.take());
    assert_eq!(
        trial.retirement.released.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    );
    trial.allow.take().unwrap().send(()).unwrap();
    trial.retirement.finish();
    assert!(owner.drain_services());
}

#[test]
fn cancel_before_publication_keeps_the_claim_until_relay_shutdown() {
    let owner = owner();
    let mut trial = Trial::begin(&owner);
    trial.offer();
    drop(trial.admission.take());
    trial.retirement.finish();
    assert!(owner.cast_target().is_none());
    assert!(owner.drain_services());
}

#[test]
fn revoked_grant_cannot_publish_an_offered_relay() {
    let owner = owner();
    let mut trial = Trial::begin(&owner);
    trial.offer();
    owner.parental_lock();
    assert!(trial.poll().is_pending());
    let mut admission = trial.admission.take().unwrap();
    trial.retirement.finish();
    trial.notified.recv_timeout(WAIT).unwrap();
    assert_eq!(
        admission
            .as_mut()
            .poll(&mut Context::from_waker(&trial.waker)),
        std::task::Poll::Ready(Err(Error::Denied))
    );
    assert!(owner.cast_target().is_none());
    assert!(owner.drain_services());
}

#[test]
fn owner_shutdown_retires_a_suspended_cast_admission() {
    let owner = owner();
    let mut trial = Trial::begin(&owner);
    trial.offer();
    let mut admission = trial.admission.take().unwrap();
    std::thread::scope(|scope| {
        scope.spawn(move || trial.retirement.finish());
        assert!(owner.drain_services());
    });
    assert!(owner.cast_target().is_none());
    assert_eq!(
        admission
            .as_mut()
            .poll(&mut Context::from_waker(&trial.waker)),
        std::task::Poll::Ready(Err(Error::Denied))
    );
}

#[test]
fn owner_shutdown_retires_an_active_cast_without_waiting_for_the_receiver_stop_reply() {
    let owner = owner();
    let mut trial = Trial::begin(&owner);
    trial.offer();
    assert_eq!(trial.poll(), std::task::Poll::Ready(Ok(false)));
    assert!(owner.cast_target().is_some());
    std::thread::scope(|scope| {
        scope.spawn(move || trial.retirement.finish());
        assert!(owner.drain_services());
    });
    assert!(owner.cast_target().is_none());
}

#[test]
fn dropping_an_undrained_owner_retires_its_active_cast() {
    let owner = owner();
    let retirement = {
        let mut trial = Trial::begin(&owner);
        trial.offer();
        assert_eq!(trial.poll(), std::task::Poll::Ready(Ok(false)));
        trial.retirement
    };
    std::thread::scope(|scope| {
        scope.spawn(move || retirement.finish());
        drop(owner);
    });
}
