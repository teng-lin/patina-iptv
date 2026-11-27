//! The actual picker and playback controls with a generated Cast receiver.
use crate::{Headless, browse_fixture, personalization};
use iptv_core::{
    cast::{Command, Error, Receiver, RemoteState},
    playback::PlaySource,
};
use iptv_runtime::cast::{Admitted, Authorization, Port, Prepared, Remote, Retirement, Task};
use iptv_settings::{Settings, Value};
use personalization::step;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
struct FakeCast {
    events: Arc<Mutex<Vec<&'static str>>>,
    states: Arc<Mutex<VecDeque<Result<RemoteState, Error>>>>,
    next_command: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    load_error: Option<Error>,
    allow_error: Option<Error>,
    initially_paused: bool,
    discovered_name: Arc<Mutex<Option<String>>>,
}

impl FakeCast {
    fn hold_next_command(&self) -> tokio::sync::oneshot::Sender<()> {
        let (send, receive) = tokio::sync::oneshot::channel();
        *self.next_command.lock().unwrap() = Some(receive);
        send
    }
}

impl Port for FakeCast {
    fn discover(&self) -> Task<'_, Vec<Receiver>> {
        Box::pin(async move {
            Ok(vec![Receiver {
                name: self
                    .discovered_name
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "Generated TV".into()),
                address: ([127, 0, 0, 1], 8009).into(),
            }])
        })
    }
    fn prepare(&self, receiver: Receiver, _source: PlaySource) -> Task<'_, Box<dyn Prepared>> {
        assert_eq!(receiver.name, "Generated TV");
        Box::pin(async { Ok(Box::new(self.clone()) as Box<dyn Prepared>) })
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
    step(app.wait_for("id:chooser-items >> text:Generated TV"));
    step(app.press("id:chooser-items >> text:Generated TV"));
    fixture
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
