//! The actual picker and playback controls with a generated Cast receiver.
use crate::{Headless, browse_fixture, personalization};
use iptv_core::{
    cast::{Command, Error, Receiver, RemoteState},
    playback::PlaySource,
};
use iptv_runtime::cast::{Admitted, Port, Prepared, Remote, Retirement, Task};
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
        Box::pin(async {
            Ok(vec![Receiver {
                name: "Generated TV".into(),
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
    fn allow(self: Box<Self>) -> Task<'static, Admitted> {
        Box::pin(async move {
            self.events.lock().unwrap().push("allow");
            if let Some(error) = self.allow_error {
                return Err(error);
            }
            Ok(Admitted {
                paused: self.initially_paused,
                remote: Arc::new(*self),
            })
        })
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

fn open_cast(app: &mut Headless) {
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
    for (label, event) in [
        ("Pause", "pause"),
        ("Play", "play"),
        ("Play here", "retire"),
    ] {
        open_cast(app);
        let row = format!("id:chooser-items >> text:{label}");
        step(app.wait_for(&row));
        step(app.press(&row));
        step(app.wait_until(event, Duration::from_secs(10), |_| {
            fake.events.lock().unwrap().contains(&event)
        }));
    }
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
        load_error: Some(Error::NoRequest),
        ..Default::default()
    };
    let mut fixture = select_generated_tv(home, &fake);
    step(fixture.app.wait_for(
        "text:The TV did not request the stream. Check the firewall and Wi-Fi client isolation.",
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
            .wait_for("text:The Chromecast refused this stream."),
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
    let here = "id:chooser-items >> text:Play here";
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
    step(app.press(here));
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
    step(app.press("id:chooser-items >> text:Pause"));
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
