//! The actual picker and playback controls with a generated Cast receiver.
use crate::{Headless, browse_fixture, personalization};
use iptv_core::{
    cast::{Command, Controls, Error, NetworkProtocol, Receiver, RemoteState},
    playback::PlaySource,
};
use iptv_runtime::cast::{Admitted, Authorization, Port, Prepared, Remote, Retirement, Task};
use iptv_settings::{Settings, Value};
use personalization::step;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Default)]
struct FakeCast {
    events: Arc<Mutex<Vec<&'static str>>>,
    states: Arc<Mutex<VecDeque<Result<RemoteState, Error>>>>,
    next_command: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    load_error: Option<Error>,
    prepare_error: Arc<Mutex<Option<Error>>>,
    allow_error: Option<Error>,
    initially_paused: bool,
    stop_unconfirmed: bool,
    discovered_name: Arc<Mutex<Option<String>>>,
    requires_pairing: bool,
    controls: Option<Controls>,
    changed_identity: Arc<AtomicBool>,
    airplay: bool,
    extra_airplay: bool,
    pending_forget: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
}

impl FakeCast {
    fn hold_next_command(&self) -> tokio::sync::oneshot::Sender<()> {
        let (send, receive) = tokio::sync::oneshot::channel();
        *self.next_command.lock().unwrap() = Some(receive);
        send
    }
}

impl Port for FakeCast {
    fn forget(&self, receiver: Receiver) -> Task<'_, ()> {
        Box::pin(async move {
            assert_eq!(receiver.protocol, NetworkProtocol::AirPlay);
            let wait = self.pending_forget.lock().unwrap().take();
            if let Some(wait) = wait {
                self.events.lock().unwrap().push("forget pending");
                wait.await.map_err(|_| Error::Cancelled)?;
            }
            assert!(self.changed_identity.swap(false, Ordering::SeqCst));
            self.events.lock().unwrap().push("forgot pairing");
            Ok(())
        })
    }

    fn discover(&self) -> Task<'_, Vec<Receiver>> {
        Box::pin(async move {
            let mut receivers = vec![Receiver {
                name: self
                    .discovered_name
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "Generated TV".into()),
                id: if self.airplay {
                    "airplay:generated"
                } else {
                    "googlecast:generated"
                }
                .into(),
                protocol: if self.airplay {
                    NetworkProtocol::AirPlay
                } else {
                    NetworkProtocol::GoogleCast
                },
            }];
            if self.extra_airplay {
                receivers.push(Receiver {
                    id: "airplay:second".into(),
                    name: "Generated AirPlay".into(),
                    protocol: NetworkProtocol::AirPlay,
                });
            }
            Ok(receivers)
        })
    }
    fn prepare(
        &self,
        receiver: Receiver,
        _source: PlaySource,
        pairing: iptv_runtime::cast::PairingPrompt,
    ) -> Task<'_, Box<dyn Prepared>> {
        assert!(matches!(
            receiver.name.as_str(),
            "Generated TV" | "Generated AirPlay"
        ));
        Box::pin(async move {
            if let Some(error) = self.prepare_error.lock().unwrap().take() {
                return Err(error);
            }
            if receiver.protocol == NetworkProtocol::AirPlay
                && self.changed_identity.load(Ordering::SeqCst)
            {
                self.events.lock().unwrap().push("identity changed");
                return Err(Error::PairingIdentityChanged);
            }
            if self.requires_pairing {
                self.events.lock().unwrap().push("pairing");
                let code = pairing.request().await?;
                assert_eq!(code.as_str(), "0123");
                self.events.lock().unwrap().push("paired");
            }
            Ok(Box::new(self.clone()) as Box<dyn Prepared>)
        })
    }
}
impl Prepared for FakeCast {
    fn load(&mut self) -> Task<'_, ()> {
        Box::pin(async {
            self.events.lock().unwrap().push("load");
            self.load_error.map_or(Ok(()), Err)
        })
    }
    fn allow(self: Box<Self>, authorization: &Authorization) -> Task<'_, Admitted> {
        Box::pin(authorization.while_current(async move {
            self.events.lock().unwrap().push("allow");
            if let Some(error) = self.allow_error {
                return Err(error);
            }
            Ok(Admitted {
                paused: self.initially_paused,
                remote: Arc::new(*self),
            })
        }))
    }
}
impl Remote for FakeCast {
    fn controls(&self) -> Option<Controls> {
        Some(self.controls.unwrap_or(Controls::ALL))
    }

    fn command(&self, command: Command) -> Task<'_, ()> {
        Box::pin(async move {
            let event = match command {
                Command::Pause => "pause",
                Command::Play => "play",
                Command::Stop => "disconnected at stop",
                _ => panic!("unexpected control {command:?}"),
            };
            self.events.lock().unwrap().push(event);
            let held = self.next_command.lock().unwrap().take();
            if let Some(held) = held {
                let _ = held.await;
            }
            if command == Command::Stop {
                Err(Error::Unavailable)
            } else {
                Ok(())
            }
        })
    }
    fn stop_checked(&self) -> Task<'_, ()> {
        Box::pin(async move {
            self.stop().await;
            if self.stop_unconfirmed {
                return Err(Error::StopUnconfirmed);
            }
            Ok(())
        })
    }
    fn state(&self) -> Task<'_, RemoteState> {
        Box::pin(async {
            self.states
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(RemoteState::Playing))
        })
    }
    fn retire(&self) -> Retirement<'_> {
        Box::pin(async {
            self.events.lock().unwrap().push("retire");
        })
    }
}

fn ready_controls(app: &mut Headless) {
    if !app.screen().matches("id:play-bar-back").unwrap() {
        step(app.press("id:video"));
    }
    step(app.wait_for("id:play-bar-back"));
    step(
        app.wait_until("Cast button accepts input", Duration::from_secs(10), |s| {
            s.accessibility_state("id:play-bar-cast")
                .is_ok_and(|state| !state.busy && !state.disabled)
        }),
    );
}
fn open_cast(app: &mut Headless) {
    ready_controls(app);
    step(app.play_bar_option("Cast to TV"));
}

fn select_generated_tv(home: crate::isolated::Home, fake: &FakeCast) -> personalization::Fixture {
    iptv_app::install_test_cast_port(Arc::new(fake.clone()));
    let mut settings = Settings::new();
    settings
        .set("guide.after_choosing", Value::choice("stay"))
        .unwrap();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    settings
        .set("playlists.max_connections", Value::Int(1))
        .unwrap();
    let mut fixture = personalization::start(home, settings, false);
    let app = &mut fixture.app;
    browse_fixture::choose_fullscreen(app);
    open_cast(app);
    let suffix = if fake.extra_airplay && !fake.airplay {
        "id:chooser-items >> text:Generated TV · Chromecast"
    } else {
        generated_tv_row(fake.airplay)
    };
    step(app.wait_for(suffix));
    step(app.press(suffix));
    fixture
}

fn generated_tv_row(airplay: bool) -> &'static str {
    if airplay {
        "id:chooser-items >> text:Generated TV · AirPlay"
    } else {
        "id:chooser-items >> text:Generated TV"
    }
}

fn pick_generated_tv(app: &mut Headless, airplay: bool) {
    let row = generated_tv_row(airplay);
    step(app.wait_for(row));
    step(app.press(row));
}

#[test]
fn pairing_cancel_keeps_local_playback_and_retry_preserves_leading_zeroes() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast {
        requires_pairing: true,
        controls: Some(Controls::NONE),
        airplay: true,
        ..Default::default()
    };
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    let pin = "id:cast-pairing-pin";
    step(app.wait_for(pin));
    assert!(iptv_app::player_has_source(0));
    assert_eq!(*fake.events.lock().unwrap(), ["pairing"]);
    step(app.press("text:Cancel"));
    step(
        app.wait_until("pairing was dismissed", Duration::from_secs(10), |screen| {
            !screen.matches(pin).unwrap()
        }),
    );
    assert!(iptv_app::player_has_source(0));
    assert_eq!(*fake.events.lock().unwrap(), ["pairing"]);

    open_cast(app);
    pick_generated_tv(app, true);
    step(app.wait_for(pin));
    step(app.type_text(pin, "012"));
    step(app.submit(pin));
    assert!(app.screen().matches(pin).unwrap());
    step(app.type_text(pin, "3"));
    step(app.submit(pin));
    step(
        app.wait_until("paired handoff", Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&"allow") && !iptv_app::player_has_source(0)
        }),
    );
    assert_eq!(
        *fake.events.lock().unwrap(),
        ["pairing", "pairing", "paired", "load", "allow"]
    );
    ready_controls(app);
    assert!(!app.screen().matches("id:play-bar-play-pause").unwrap());
    open_cast(app);
    step(app.press("id:chooser-items >> text:This device"));
    step(
        app.wait_until("local playback resumed", Duration::from_secs(10), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    assert_eq!(
        *fake.events.lock().unwrap(),
        [
            "pairing",
            "pairing",
            "paired",
            "load",
            "allow",
            "retire",
            "disconnected at stop"
        ]
    );
}

#[test]
fn a_changed_airplay_identity_requires_an_explicit_forget_and_new_selection() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast {
        changed_identity: Arc::new(AtomicBool::new(true)),
        airplay: true,
        ..Default::default()
    };
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    step(app.wait_for("text:Forget pairing"));
    assert!(iptv_app::player_has_source(0));
    step(app.press("text:Cancel"));
    assert!(fake.changed_identity.load(Ordering::SeqCst));

    open_cast(app);
    pick_generated_tv(app, true);
    step(app.wait_for("text:Forget pairing"));
    step(app.press("text:Forget pairing"));
    step(
        app.wait_until("old pairing removed", Duration::from_secs(10), |_| {
            !fake.changed_identity.load(Ordering::SeqCst)
        }),
    );
    assert!(iptv_app::player_has_source(0));
    assert_eq!(
        *fake.events.lock().unwrap(),
        ["identity changed", "identity changed", "forgot pairing"]
    );
    open_cast(app);
    pick_generated_tv(app, true);
    step(app.wait_until(
        "new route selection started playback",
        Duration::from_secs(10),
        |_| fake.events.lock().unwrap().contains(&"allow") && !iptv_app::player_has_source(0),
    ));
}

#[test]
fn an_unreadable_airplay_pairing_can_be_reset_without_ending_local_playback() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast {
        prepare_error: Arc::new(Mutex::new(Some(Error::PairingStorage))),
        changed_identity: Arc::new(AtomicBool::new(true)),
        airplay: true,
        ..Default::default()
    };
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    step(app.wait_for("text:Could not access the AirPlay pairing."));
    assert!(iptv_app::player_has_source(0));
    assert!(!app.screen().matches("text:Forget pairing").unwrap());
    *fake.prepare_error.lock().unwrap() = Some(Error::PairingInvalid);
    open_cast(app);
    pick_generated_tv(app, true);
    step(app.wait_for("text:The saved AirPlay pairing is invalid."));
    step(app.press("text:Forget pairing"));
    step(
        app.wait_until("invalid pairing removed", Duration::from_secs(10), |_| {
            !fake.changed_identity.load(Ordering::SeqCst)
        }),
    );
    assert!(iptv_app::player_has_source(0));
    assert_eq!(*fake.events.lock().unwrap(), ["forgot pairing"]);
}

#[test]
fn airplay_timing_errors_identify_the_udp_problem_and_keep_local_playback() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast {
        prepare_error: Arc::new(Mutex::new(Some(Error::AirPlayTimingSocket { port: None }))),
        airplay: true,
        ..Default::default()
    };
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    step(app.wait_for("text:Could not start AirPlay UDP timing."));
    assert!(iptv_app::player_has_source(0));

    for (error, notification) in [
        (
            Error::AirPlayTimingSocket { port: Some(54183) },
            "text:AirPlay UDP timing failed on port 54183.",
        ),
        (
            Error::AirPlayTiming { port: 54183 },
            "text:AirPlay timing did not arrive. Allow UDP port 54183 from the receiver.",
        ),
    ] {
        *fake.prepare_error.lock().unwrap() = Some(error);
        open_cast(app);
        pick_generated_tv(app, true);
        step(app.wait_for(notification));
        assert!(iptv_app::player_has_source(0));
    }
    assert!(fake.events.lock().unwrap().is_empty());
}

#[test]
fn casting_stop_is_honored_while_forgetting_another_receivers_pairing() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast {
        changed_identity: Arc::new(AtomicBool::new(true)),
        extra_airplay: true,
        ..Default::default()
    };
    let (release, wait) = tokio::sync::oneshot::channel();
    *fake.pending_forget.lock().unwrap() = Some(wait);
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    step(
        app.wait_until("first cast started", Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&"allow") && !iptv_app::player_has_source(0)
        }),
    );
    open_cast(app);
    step(app.wait_for("id:chooser-items >> text:Generated AirPlay · AirPlay"));
    step(app.press("id:chooser-items >> text:Generated AirPlay · AirPlay"));
    step(app.wait_for("text:Forget pairing"));
    step(app.press("text:Forget pairing"));
    step(
        app.wait_until("forget is pending", Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&"forget pending")
        }),
    );
    step(app.press_remote(nsh_tv::RemoteKey::Stop));
    release.send(()).unwrap();
    step(
        app.wait_until("active receiver retired", Duration::from_secs(10), |_| {
            fake.events
                .lock()
                .unwrap()
                .contains(&"disconnected at stop")
        }),
    );
    assert!(!iptv_app::player_has_source(0));
}

#[test]
fn remote_controls_and_play_here_recover_one_connection_after_disconnect() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast::default();
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    step(
        app.wait_until("handoff to TV", Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&"allow") && !iptv_app::player_has_source(0)
        }),
    );
    for event in ["pause", "play"] {
        step(app.press("id:play-bar-play-pause"));
        step(app.wait_until(event, Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&event)
        }));
    }
    open_cast(app);
    assert!(
        !app.screen()
            .matches("id:chooser-items >> text:Pause")
            .unwrap()
    );
    assert!(
        !app.screen()
            .matches("id:chooser-items >> text:Play")
            .unwrap()
    );
    step(app.press("id:chooser-items >> text:This device"));
    step(
        app.wait_until("local playback resumes", Duration::from_secs(10), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    assert_eq!(
        *fake.events.lock().unwrap(),
        [
            "load",
            "allow",
            "pause",
            "play",
            "retire",
            "disconnected at stop"
        ]
    );
}

#[test]
fn play_here_restores_local_playback_and_reports_unconfirmed_stop() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast {
        stop_unconfirmed: true,
        ..Default::default()
    };
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    open_cast(app);
    step(app.press("id:chooser-items >> text:This device"));
    step(app.wait_for("text:Could not confirm that the TV stopped playback."));
    step(
        app.wait_until("local playback resumes", Duration::from_secs(10), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    assert_eq!(
        *fake.events.lock().unwrap(),
        ["load", "allow", "retire", "disconnected at stop"]
    );
}

#[test]
fn an_unconfirmed_system_stop_reports_the_receiver_result_and_releases_the_cast() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast::default();
    fake.states
        .lock()
        .unwrap()
        .push_back(Err(Error::StopUnconfirmed));
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    step(app.wait_for("text:Could not confirm that the TV stopped playback."));
    step(app.wait_for("text:Casting ended."));
    assert!(!iptv_app::player_has_source(0));
    assert_eq!(*fake.events.lock().unwrap(), ["load", "allow", "retire"]);
    step(app.press("id:chooser-items >> text:Play here"));
    step(app.wait_until(
        "reopen after unconfirmed system stop",
        Duration::from_secs(10),
        |_| iptv_app::player_has_source(0),
    ));
}

#[test]
fn an_unconfirmed_system_stop_is_reported_while_forgetting_another_pairing() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast {
        changed_identity: Arc::new(AtomicBool::new(true)),
        extra_airplay: true,
        ..Default::default()
    };
    let (release, wait) = tokio::sync::oneshot::channel();
    *fake.pending_forget.lock().unwrap() = Some(wait);
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    open_cast(app);
    step(app.wait_for("id:chooser-items >> text:Generated AirPlay · AirPlay"));
    step(app.press("id:chooser-items >> text:Generated AirPlay · AirPlay"));
    step(app.wait_for("text:Forget pairing"));
    step(app.press("text:Forget pairing"));
    step(
        app.wait_until("forget is pending", Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&"forget pending")
        }),
    );
    fake.states
        .lock()
        .unwrap()
        .push_back(Err(Error::StopUnconfirmed));
    step(app.wait_for("text:Could not confirm that the TV stopped playback."));
    assert!(!iptv_app::player_has_source(0));
    release.send(()).unwrap();
    step(
        app.wait_until("pairing removed", Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&"forgot pairing")
        }),
    );
    assert_eq!(
        *fake.events.lock().unwrap(),
        [
            "load",
            "allow",
            "identity changed",
            "forget pending",
            "retire",
            "forgot pairing"
        ]
    );
}

#[test]
fn play_pause_uses_the_receivers_confirmed_pause_state_at_handoff() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast {
        initially_paused: true,
        ..Default::default()
    };
    // A later poll cannot supply the initial pause state for this test.
    fake.states
        .lock()
        .unwrap()
        .extend([Ok(RemoteState::Loading); 10]);
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    step(
        app.wait_until("handoff to paused receiver", Duration::from_secs(10), |s| {
            s.accessibility_state("id:play-bar-cast")
                .is_ok_and(|state| !state.busy && !state.disabled)
                && fake.events.lock().unwrap().contains(&"allow")
        }),
    );
    step(app.press("id:play-bar-play-pause"));
    step(
        app.wait_until("play the paused receiver", Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&"play")
        }),
    );
}

#[test]
fn no_receiver_request_keeps_local_playback() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast {
        load_error: Some(Error::NoRequest { relay_port: 53317 }),
        ..Default::default()
    };
    let mut fixture = select_generated_tv(home, &fake);
    step(fixture.app.wait_for(
        "text:The TV did not request the stream. Allow TCP port 53317 from the TV in your firewall and check Wi-Fi client isolation.",
    ));
    assert!(iptv_app::player_has_source(0));
    assert_eq!(*fake.events.lock().unwrap(), ["load"]);
}

#[test]
fn a_late_receiver_refusal_restores_local_playback() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast {
        allow_error: Some(Error::Refused),
        ..Default::default()
    };
    let mut fixture = select_generated_tv(home, &fake);
    step(
        fixture
            .app
            .wait_for("text:Could not start playback on the selected device."),
    );
    step(
        fixture
            .app
            .wait_until("restore local channel", Duration::from_secs(10), |_| {
                iptv_app::player_has_source(0)
            }),
    );
    assert_eq!(*fake.events.lock().unwrap(), ["load", "allow"]);
}

#[test]
fn a_successful_status_resets_the_disconnect_budget() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast::default();
    let missing = Err(Error::Unavailable);
    fake.states.lock().unwrap().extend([
        missing,
        missing,
        Ok(RemoteState::Playing),
        missing,
        missing,
        missing,
    ]);
    let mut fixture = select_generated_tv(home, &fake);
    step(
        fixture
            .app
            .wait_for_within("text:Casting ended.", Duration::from_secs(35)),
    );
    step(fixture.app.press("id:chooser-items >> text:Play here"));
    step(fixture.app.wait_until(
        "reopen after receiver loss",
        Duration::from_secs(10),
        |_| iptv_app::player_has_source(0),
    ));
    assert!(fake.states.lock().unwrap().is_empty());
    assert_eq!(*fake.events.lock().unwrap(), ["load", "allow", "retire"]);
}

#[test]
fn a_stale_network_route_cannot_retire_the_current_playback_during_a_channel_change() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast::default();
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    ready_controls(app);
    *fake.prepare_error.lock().unwrap() = Some(Error::Unavailable);
    step(app.press("id:play-bar-next-channel"));
    step(app.wait_for("text:Could not start playback on the selected device."));
    assert_eq!(*fake.events.lock().unwrap(), ["load", "allow"]);
    assert!(!iptv_app::player_has_source(0));
    ready_controls(app);
    step(app.press("id:play-bar-play-pause"));
    step(app.wait_until(
        "the existing receiver is still controllable",
        Duration::from_secs(10),
        |_| fake.events.lock().unwrap().contains(&"pause"),
    ));
}

#[test]
fn stop_during_a_failed_cast_channel_switch_keeps_local_playback_closed() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast::default();
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    open_cast(app);
    let reply = fake.hold_next_command();
    step(app.press("id:play-bar-next-channel"));
    step(
        app.wait_until("retiring old cast", Duration::from_secs(10), |_| {
            fake.events
                .lock()
                .unwrap()
                .contains(&"disconnected at stop")
        }),
    );
    // Global Stop invalidates the switch ticket and queues its own stop intent.
    // The outgoing receiver stays active in the owner until the worker returns.
    step(app.press_remote(nsh_tv::RemoteKey::Stop));
    reply.send(()).unwrap();
    step(app.wait_for("text:Playback is no longer authorized."));
    step(app.wait_for_gone("id:play-bar-next-channel", Duration::from_secs(3)));
    assert!(!iptv_app::player_has_source(0));
    assert_eq!(
        *fake.events.lock().unwrap(),
        ["load", "allow", "retire", "disconnected at stop"]
    );
}

#[test]
fn play_here_keeps_its_chooser_open_while_a_remote_command_finishes() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast::default();
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    open_cast(app);
    let reply = fake.hold_next_command();
    step(app.press("id:play-bar-play-pause"));
    step(
        app.wait_until("remote pause", Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&"pause")
        }),
    );
    let here = "id:chooser-items >> text:This device";
    step(app.press(here));
    assert!(app.screen().matches(here).unwrap());
    reply.send(()).unwrap();
    step(
        app.wait_until("remote pause finished", Duration::from_secs(10), |screen| {
            screen
                .accessibility_state("id:play-bar-cast")
                .is_ok_and(|state| !state.busy)
        }),
    );
    step(
        app.wait_until("local playback", Duration::from_secs(10), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    assert_eq!(
        *fake.events.lock().unwrap(),
        ["load", "allow", "pause", "retire", "disconnected at stop"]
    );
}

#[test]
fn queued_play_here_resumes_locally_after_an_unconfirmed_system_stop() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast::default();
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    open_cast(app);
    let reply = fake.hold_next_command();
    step(app.press("id:play-bar-play-pause"));
    step(
        app.wait_until("remote pause is pending", Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&"pause")
        }),
    );
    let here = "id:chooser-items >> text:This device";
    step(app.press(here));
    assert!(app.screen().matches(here).unwrap());

    fake.states
        .lock()
        .unwrap()
        .push_back(Err(Error::StopUnconfirmed));
    step(app.wait_for("text:Could not confirm that the TV stopped playback."));
    assert!(!iptv_app::player_has_source(0));
    assert_eq!(
        *fake.events.lock().unwrap(),
        ["load", "allow", "pause", "retire"]
    );

    reply.send(()).unwrap();
    step(app.wait_until(
        "queued Play here reopens the local channel",
        Duration::from_secs(10),
        |_| iptv_app::player_has_source(0),
    ));
    step(app.wait_for_gone(here, Duration::from_secs(3)));
    assert_eq!(
        *fake.events.lock().unwrap(),
        ["load", "allow", "pause", "retire"]
    );
}

#[test]
fn a_global_stop_queued_during_pause_uses_its_own_cast_intent() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast::default();
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    open_cast(app);
    let reply = fake.hold_next_command();
    step(app.press("id:play-bar-play-pause"));
    step(
        app.wait_until("remote pause", Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&"pause")
        }),
    );
    // Each Stop replaces the preceding intent while the same command is pending.
    step(app.press_remote(nsh_tv::RemoteKey::Stop));
    step(app.press_remote(nsh_tv::RemoteKey::Stop));
    reply.send(()).unwrap();
    step(
        app.wait_until("remote stopped", Duration::from_secs(10), |_| {
            fake.events
                .lock()
                .unwrap()
                .contains(&"disconnected at stop")
        }),
    );
    assert!(!iptv_app::player_has_source(0));
    assert_eq!(
        *fake.events.lock().unwrap(),
        ["load", "allow", "pause", "retire", "disconnected at stop"]
    );
}

#[derive(Clone)]
struct FakeNative {
    cast: FakeCast,
    cancel: Arc<tokio::sync::Notify>,
    wait_for_approval: bool,
    prepare_gate: Option<Arc<tokio::sync::Notify>>,
}
impl iptv_runtime::cast::NativePort for FakeNative {
    fn view_id(&self) -> Option<u64> {
        None
    }
    fn prepare(&self, _source: PlaySource) -> Task<'_, Box<dyn Prepared>> {
        let prepared = self.clone();
        Box::pin(async move {
            if let Some(gate) = &prepared.prepare_gate {
                prepared.cast.events.lock().unwrap().push("preparing");
                gate.notified().await;
            }
            Ok(Box::new(prepared) as Box<dyn Prepared>)
        })
    }
    fn cancel_pending(&self) {
        self.cast
            .events
            .lock()
            .unwrap()
            .push("cancel native picker");
        self.cancel.notify_one();
    }
}
impl Prepared for FakeNative {
    fn load(&mut self) -> Task<'_, ()> {
        self.cast.load()
    }
    fn allow(self: Box<Self>, authorization: &Authorization) -> Task<'_, Admitted> {
        Box::pin(authorization.while_current(async move {
            assert!(
                !iptv_app::player_has_source(0),
                "the one-stream account still has a local player source"
            );
            self.cast.events.lock().unwrap().push("native admission");
            if self.wait_for_approval {
                self.cancel.notified().await;
                return Err(Error::Cancelled);
            }
            Ok(Admitted {
                remote: Arc::new(self.cast),
                paused: false,
            })
        }))
    }
}
fn select_generated_airplay(
    home: crate::isolated::Home,
    wait_for_approval: bool,
) -> (personalization::Fixture, FakeCast) {
    let (mut fixture, fake) = start_generated_airplay(home, wait_for_approval, None);
    step(
        fixture
            .app
            .wait_until("native handoff admission", Duration::from_secs(10), |_| {
                fake.events.lock().unwrap().contains(&"native admission")
            }),
    );
    (fixture, fake)
}

fn start_generated_airplay(
    home: crate::isolated::Home,
    wait_for_approval: bool,
    prepare_gate: Option<Arc<tokio::sync::Notify>>,
) -> (personalization::Fixture, FakeCast) {
    let fake = FakeCast::default();
    iptv_app::install_test_cast_port(Arc::new(fake.clone()));
    iptv_app::install_test_native_cast_port(Arc::new(FakeNative {
        cast: fake.clone(),
        cancel: Arc::default(),
        wait_for_approval,
        prepare_gate,
    }));
    let mut settings = Settings::new();
    settings
        .set("guide.after_choosing", Value::choice("stay"))
        .unwrap();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    settings
        .set("playlists.max_connections", Value::Int(1))
        .unwrap();
    let mut fixture = personalization::start(home, settings, false);
    browse_fixture::choose_fullscreen(&mut fixture.app);
    open_cast(&mut fixture.app);
    step(
        fixture
            .app
            .wait_for("id:chooser-items >> text:Choose a device"),
    );
    step(
        fixture
            .app
            .press("id:chooser-items >> text:Choose a device"),
    );
    (fixture, fake)
}

#[test]
fn choosing_local_before_native_preparation_finishes_never_loads_the_receiver() {
    let Some(home) = isolated!() else { return };
    let gate = Arc::new(tokio::sync::Notify::new());
    let (mut fixture, fake) = start_generated_airplay(home, false, Some(gate.clone()));
    step(
        fixture
            .app
            .wait_until("native preparation", Duration::from_secs(10), |_| {
                fake.events.lock().unwrap().contains(&"preparing")
            }),
    );
    step(fixture.app.press("id:chooser-items >> text:Cancel"));
    gate.notify_one();
    ready_controls(&mut fixture.app);
    assert!(iptv_app::player_has_source(0));
    assert_eq!(
        *fake.events.lock().unwrap(),
        ["preparing", "cancel native picker"]
    );
}

#[test]
fn native_airplay_remote_controls_and_play_here_share_the_one_stream_handoff() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, fake) = select_generated_airplay(home, false);
    for event in ["pause", "play"] {
        step(fixture.app.press("id:play-bar-play-pause"));
        step(fixture.app.wait_until(event, Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&event)
        }));
    }
    open_cast(&mut fixture.app);
    step(fixture.app.press("id:chooser-items >> text:This device"));
    step(fixture.app.wait_until(
        "local playback after AirPlay",
        Duration::from_secs(10),
        |_| iptv_app::player_has_source(0),
    ));
}

#[test]
fn native_picker_cancel_retires_the_pending_handoff_and_restores_local_playback() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, fake) = select_generated_airplay(home, true);
    step(fixture.app.wait_for("id:chooser-items >> text:Cancel"));
    step(fixture.app.press("id:chooser-items >> text:Cancel"));
    step(fixture.app.wait_until(
        "local playback after cancelled receiver approval",
        Duration::from_secs(10),
        |_| iptv_app::player_has_source(0),
    ));
    assert!(
        fake.events
            .lock()
            .unwrap()
            .contains(&"cancel native picker")
    );
}

#[test]
fn native_channel_change_retires_the_old_stream_and_stop_does_not_reopen_locally() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, fake) = select_generated_airplay(home, false);
    step(fixture.app.play_bar_option("Channel list"));
    step(fixture.app.wait_for("id:channel-list >> text:/Local B/"));
    step(fixture.app.press("id:channel-list >> text:/Local B/"));
    step(
        fixture
            .app
            .wait_until("second native channel", Duration::from_secs(10), |_| {
                fake.events
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|event| **event == "native admission")
                    .count()
                    == 2
            }),
    );
    ready_controls(&mut fixture.app);
    step(fixture.app.press_remote(nsh_tv::RemoteKey::Stop));
    step(
        fixture
            .app
            .wait_until("native stop", Duration::from_secs(10), |_| {
                fake.events
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|event| **event == "retire")
                    .count()
                    == 2
            }),
    );
    assert!(!iptv_app::player_has_source(0));
}

#[test]
fn picking_the_selected_named_receiver_returns_playback_to_this_device() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast::default();
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    step(app.wait_until("handoff", Duration::from_secs(10), |_| {
        fake.events.lock().unwrap().contains(&"allow")
    }));
    *fake.discovered_name.lock().unwrap() = Some("Generated receiver renamed".to_owned());
    open_cast(app);
    let receiver = "id:chooser-items >> text:/Generated receiver renamed/";
    step(app.wait_for(receiver));
    assert!(
        !app.screen()
            .matches("id:chooser-items >> text:Chromecast")
            .unwrap()
    );
    assert!(
        !app.screen()
            .matches("id:chooser-items >> text:AirPlay")
            .unwrap()
    );
    step(app.press(receiver));
    step(
        app.wait_until("local playback", Duration::from_secs(10), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    assert!(fake.events.lock().unwrap().contains(&"retire"));
}

#[test]
fn explicit_media_keys_are_idempotent_and_share_the_play_bar_transport() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast::default();
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    ready_controls(app);
    step(app.press_remote(nsh_tv::RemoteKey::Play));
    step(app.press_remote(nsh_tv::RemoteKey::Play));
    assert!(!fake.events.lock().unwrap().contains(&"pause"));
    step(app.press_remote(nsh_tv::RemoteKey::Pause));
    step(app.wait_until("paused", Duration::from_secs(10), |screen| {
        fake.events.lock().unwrap().contains(&"pause")
            && screen
                .accessibility_state("id:play-bar-cast")
                .is_ok_and(|s| !s.busy)
    }));
    assert_eq!(
        app.screen()
            .accessibility_state("id:play-bar-play-pause")
            .unwrap()
            .pressed,
        Some(nsh_core::Checked::False)
    );
    step(app.press_remote(nsh_tv::RemoteKey::Pause));
    assert_eq!(
        fake.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == "pause")
            .count(),
        1
    );
    step(app.press("id:play-bar-play-pause"));
    step(app.wait_until("resumed", Duration::from_secs(10), |_| {
        fake.events.lock().unwrap().contains(&"play")
    }));
}

#[test]
fn stop_during_the_return_to_local_does_not_reopen_the_local_stream() {
    let Some(home) = isolated!() else { return };
    let fake = FakeCast::default();
    let mut fixture = select_generated_tv(home, &fake);
    let app = &mut fixture.app;
    open_cast(app);
    let reply = fake.hold_next_command();
    step(app.press("id:chooser-items >> text:This device"));
    step(
        app.wait_until("remote stop outstanding", Duration::from_secs(10), |_| {
            fake.events
                .lock()
                .unwrap()
                .contains(&"disconnected at stop")
        }),
    );
    assert!(
        !iptv_app::player_has_source(0),
        "local stream opened before remote retirement finished"
    );
    step(app.press_remote(nsh_tv::RemoteKey::Stop));
    reply.send(()).unwrap();
    step(app.wait_for_gone("id:play-bar-cast", Duration::from_secs(10)));
    assert!(
        !iptv_app::player_has_source(0),
        "a newer Stop intent was overridden by return-to-local"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn open_loopback_cast(
    home: crate::isolated::Home,
    receiver: &iptv_cast::test_support::Receiver,
) -> personalization::Fixture {
    iptv_app::install_test_loopback_cast(receiver.address());
    let hls = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nsegment.ts\n".to_owned();
    let origin = crate::support::Server::start(&[("/generated.m3u8", hls)]);
    let channel = origin.url("/generated.m3u8");
    let playlist = format!(
        "#EXTM3U\n#EXTINF:-1 group-title=\"News\",Local A\n{channel}\n\
         #EXTINF:-1 group-title=\"News\",Local B\n{channel}\n"
    );
    let mut settings = Settings::new();
    settings
        .set("guide.after_choosing", Value::choice("stay"))
        .unwrap();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    settings
        .set("playlists.max_connections", Value::Int(1))
        .unwrap();
    let mut fixture = personalization::start_with_playlist(home, settings, false, &playlist);
    browse_fixture::choose_fullscreen(&mut fixture.app);
    open_cast(&mut fixture.app);
    step(
        fixture
            .app
            .wait_for("id:chooser-items >> text:Generated Chromecast"),
    );
    step(
        fixture
            .app
            .press("id:chooser-items >> text:Generated Chromecast"),
    );
    fixture
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[test]
fn native_cast_admits_the_gated_relay_before_ack_then_stops_the_owned_media() {
    use iptv_cast::test_support::{Event, Load, Receiver};

    let Some(home) = isolated!() else { return };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let key_pair = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let receiver = runtime.block_on(Receiver::start(
        Load::FetchRelay,
        key_pair.cert.der().to_vec(),
        key_pair.key_pair.serialize_der(),
    ));
    let mut fixture = open_loopback_cast(home, &receiver);
    let app = &mut fixture.app;
    step(app.wait_until(
        "the receiver fetches the gated manifest",
        Duration::from_secs(10),
        |_| receiver.events().contains(&Event::FetchedRelay) && !iptv_app::player_has_source(0),
    ));
    ready_controls(app);
    step(app.press("id:play-bar-play-pause"));
    step(app.wait_until(
        "the receiver confirms pause",
        Duration::from_secs(10),
        |_| receiver.events().contains(&Event::Paused),
    ));
    open_cast(app);
    step(app.press("id:chooser-items >> text:This device"));
    step(
        app.wait_until("local playback resumes", Duration::from_secs(10), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    assert_eq!(
        receiver.events(),
        [Event::Load, Event::FetchedRelay, Event::Paused, Event::Stop]
    );
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), receiver.finish())
            .await
            .unwrap()
            .unwrap();
    });
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[test]
fn native_cast_refusal_without_a_fetch_reports_the_relay_port_and_keeps_playing_here() {
    use iptv_cast::test_support::{Event, Load, Receiver};

    let Some(home) = isolated!() else { return };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let key_pair = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let receiver = runtime.block_on(Receiver::start(
        Load::Refuse,
        key_pair.cert.der().to_vec(),
        key_pair.key_pair.serialize_der(),
    ));
    let mut fixture = open_loopback_cast(home, &receiver);
    step(fixture.app.wait_until(
        "a refusal reports the bound relay port",
        Duration::from_secs(10),
        |screen| {
            screen.texts().iter().any(|text| {
                text.strip_prefix("The TV did not request the stream. Allow TCP port ")
                    .and_then(|suffix| suffix.split(' ').next())
                    .and_then(|port| port.parse::<std::num::NonZeroU16>().ok())
                    .is_some()
            })
        },
    ));
    assert_eq!(receiver.events(), [Event::Load]);
    assert!(iptv_app::player_has_source(0));
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), receiver.finish())
            .await
            .unwrap()
            .unwrap();
    });
}
