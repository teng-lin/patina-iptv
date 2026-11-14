//! Declares a name on the wire once and generates every use of it.
//!
//! A prop, a nested field or an event is written here with its wire name as
//! a string literal. The serialized form, the constants the mpv adapter reads
//! with and the enums the shell and the adapter match on all come from that
//! one literal, so none of them can drift from the others.

/// An enum whose variants each have one name on the wire. Two variants with
/// the same name do not compile under `-D warnings` (an unreachable arm).
macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$vmeta:meta])* $variant:ident = $wire:literal,)*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        $vis enum $name {
            $($(#[$vmeta])* $variant,)*
        }

        impl $name {
            /// Every variant, in the order declared.
            pub const ALL: &'static [$name] = &[$($name::$variant,)*];

            /// Its name on the wire.
            pub const fn wire(self) -> &'static str {
                match self {
                    $($name::$variant => $wire,)*
                }
            }

            /// The variant with this name on the wire.
            pub fn from_wire(name: &str) -> Option<Self> {
                match name {
                    $($wire => Some($name::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

/// A serialized object whose fields each have one name on the wire: the
/// struct serde reads and writes, and a constant per field (`$konst`) for
/// code that reads the JSON leniently instead (the mpv adapter).
macro_rules! wire_struct {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $($konst:ident => $field:ident: $ty:ty = $wire:literal,)*
        }
    ) => {
        $(#[$meta])*
        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        $vis struct $name {
            $(#[serde(rename = $wire)] pub $field: $ty,)*
        }

        impl $name {
            $(
                #[doc = concat!("The wire name of `", stringify!($field), "`.")]
                pub const $konst: &'static str = $wire;
            )*

            /// Every field's wire name, in the order declared.
            pub const FIELDS: &'static [&'static str] = &[$($wire,)*];
        }
    };
}

pub(crate) use {wire_enum, wire_struct};
