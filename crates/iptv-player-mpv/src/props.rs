//! The props of `tv.player`: the native component wire contract, independent
//! of either host toolkit.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wire::{wire_enum, wire_struct};

wire_struct! {
    /// The `source` prop: the stream to play.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct SourceProps {
        URL => url: String = "url",
        PUBLIC_ONLY => public_only: bool = "publicOnly",
        HEADERS => headers: BTreeMap<String, String> = "headers",
        TOKEN => token: u64 = "token",
        LIVE => live: bool = "live",
        RECONNECT => reconnect: bool = "reconnect",
    }
}

wire_struct! {
    /// The `pip` prop: picture in picture, for surface 0's player.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct PipProps {
        AUTO => auto: bool = "auto",
        ENTER => enter: u64 = "enter",
        WINDOW => window: bool = "window",
    }
}

/// A prop's reactive value, serialized as the hosts read it.
pub type WireValue = Box<dyn Fn() -> Value>;

// One declaration keeps the shell's typed reactive bindings, the serialized
// snapshot contract and the names both hosts match on in sync, without
// coupling the adapter to UI signals.
macro_rules! player_props {
    ($($variant:ident => $field:ident: $ty:ty = $wire:literal,)*) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct PlayerProps {
            $(#[serde(rename = $wire)] pub $field: $ty,)*
        }

        /// Independently evaluated properties: changing sound does not read a source.
        pub struct PlayerBindings {
            $(pub $field: Box<dyn Fn() -> $ty>,)*
        }

        wire_enum! {
            /// One prop of the contract. A host that matches on it handles
            /// every prop: a new one does not compile until it does.
            pub enum PlayerProp {
                $($variant = $wire,)*
            }
        }

        impl PlayerBindings {
            /// Each binding under its prop, its value serialized: what the
            /// shell hands the native component, in the order declared.
            pub fn into_wire(self) -> Vec<(PlayerProp, WireValue)> {
                vec![$({
                    let value = self.$field;
                    let wire: WireValue = Box::new(move || serde_json::json!(value()));
                    (PlayerProp::$variant, wire)
                },)*]
            }
        }
    };
}

player_props! {
    Source => source: Option<SourceProps> = "source",
    Paused => paused: bool = "paused",
    PictureSize => picture_size: String = "pictureSize",
    VideoTrack => video_track: String = "videoTrack",
    AudioTrack => audio_track: String = "audioTrack",
    TextTrack => text_track: String = "textTrack",
    Volume => volume: u8 = "volume",
    Muted => muted: bool = "muted",
    AudioDelay => audio_delay: i32 = "audioDelay",
    Pip => pip: Option<PipProps> = "pip",
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../fixtures/player-props.json")).unwrap()
    }

    fn keys(object: &Value) -> Vec<&str> {
        let mut keys: Vec<_> = object
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    fn constant<T: Clone + 'static>(value: &T) -> Box<dyn Fn() -> T> {
        let value = value.clone();
        Box::new(move || value.clone())
    }

    fn sorted(names: &[&'static str]) -> Vec<&'static str> {
        let mut names = names.to_vec();
        names.sort_unstable();
        names
    }

    #[test]
    fn shared_android_fixture_is_the_exact_rust_wire_format() {
        let fixture = fixture();
        let props: PlayerProps = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(props.source.as_ref().unwrap().token, 37);
        assert_eq!(
            props.source.as_ref().unwrap().headers["X-Synthetic"],
            "fixture"
        );
        assert_eq!(serde_json::to_value(props).unwrap(), fixture);
    }

    /// The names the hosts match on are the snapshot's keys, nested ones
    /// included: nothing more (a prop the app never sends) and nothing less.
    #[test]
    fn the_generated_names_are_the_fixtures_keys() {
        let fixture = fixture();
        let props: Vec<_> = PlayerProp::ALL.iter().map(|p| p.wire()).collect();
        assert_eq!(sorted(&props), keys(&fixture));
        assert_eq!(sorted(SourceProps::FIELDS), keys(&fixture["source"]));
        assert_eq!(sorted(PipProps::FIELDS), keys(&fixture["pip"]));
        for prop in PlayerProp::ALL {
            assert_eq!(PlayerProp::from_wire(prop.wire()), Some(*prop));
        }
        assert_eq!(PlayerProp::from_wire("userAgent"), None);
        assert_eq!(PlayerProp::from_wire("picture_size"), None);
        assert_eq!(SourceProps::PUBLIC_ONLY, "publicOnly");
        assert_eq!(PipProps::WINDOW, "window");
    }

    /// What the shell sets on the native component, prop by prop, is the
    /// snapshot: the same names, each with its own value.
    #[test]
    fn the_shells_bindings_send_the_snapshot() {
        let fixture = fixture();
        let props: PlayerProps = serde_json::from_value(fixture.clone()).unwrap();
        let bindings = PlayerBindings {
            source: constant(&props.source),
            paused: constant(&props.paused),
            picture_size: constant(&props.picture_size),
            video_track: constant(&props.video_track),
            audio_track: constant(&props.audio_track),
            text_track: constant(&props.text_track),
            volume: constant(&props.volume),
            muted: constant(&props.muted),
            audio_delay: constant(&props.audio_delay),
            pip: constant(&props.pip),
        };
        let wire = bindings.into_wire();
        let order: Vec<_> = wire.iter().map(|(prop, _)| *prop).collect();
        assert_eq!(order, PlayerProp::ALL, "in the order declared");
        let sent: serde_json::Map<String, Value> = wire
            .iter()
            .map(|(prop, value)| (prop.wire().to_owned(), value()))
            .collect();
        assert_eq!(Value::Object(sent), fixture);
    }
}
