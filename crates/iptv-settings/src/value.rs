//! Runtime values of settings.

/// A setting's value. The variant must match the setting's
/// [`ValueType`](crate::ValueType); durations are milliseconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    /// Milliseconds (signed).
    Duration(i64),
    /// A variant key of the setting's choice family.
    Enum(String),
    Text(String),
    /// A palette key.
    Color(String),
    KeyBindings(Vec<KeyBinding>),
}

impl Value {
    pub fn choice(variant: &str) -> Value {
        Value::Enum(variant.to_owned())
    }

    pub fn text(text: &str) -> Value {
        Value::Text(text.to_owned())
    }

    pub fn color(name: &str) -> Value {
        Value::Color(name.to_owned())
    }

    pub const fn ms(ms: i64) -> Value {
        Value::Duration(ms)
    }

    pub const fn secs(secs: i64) -> Value {
        Value::Duration(secs * 1_000)
    }

    pub const fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::Int(_) => "integer",
            Value::Duration(_) => "duration",
            Value::Enum(_) => "enum",
            Value::Text(_) => "text",
            Value::Color(_) => "colour",
            Value::KeyBindings(_) => "key_bindings",
        }
    }
}

/// Short or long press, as named in key bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PressKind {
    Short,
    Long,
}

impl PressKind {
    pub const fn key(self) -> &'static str {
        match self {
            PressKind::Short => "short",
            PressKind::Long => "long",
        }
    }

    pub fn from_key(key: &str) -> Option<PressKind> {
        match key {
            "short" => Some(PressKind::Short),
            "long" => Some(PressKind::Long),
            _ => None,
        }
    }
}

/// One user key override, by stable names. `context: None` applies in
/// every context. Names are tokens (`[a-z][a-z0-9_]*`, ≤ 32 chars): key
/// names follow `iptv_nav::RemoteKey::name` (`ok`, `channel_up`,
/// `digit_5`); context and action names are owned by the input layer,
/// which ignores bindings it does not recognise.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyBinding {
    pub context: Option<String>,
    pub key: String,
    pub press: PressKind,
    pub action: String,
}

impl KeyBinding {
    pub fn new(context: Option<&str>, key: &str, press: PressKind, action: &str) -> KeyBinding {
        KeyBinding {
            context: context.map(str::to_owned),
            key: key.to_owned(),
            press,
            action: action.to_owned(),
        }
    }
}
