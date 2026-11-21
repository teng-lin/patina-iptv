//! The provider details form of one playlist: its link (M3U), server and
//! login (Xtream), or portal and MAC address (Stalker). Edits change a copy
//! of the stored source; checking it imports the copy, and only an import
//! that works replaces the stored details (`data::try_details`).
//!
//! Logins and MAC addresses are never put into a text field and never
//! shown (typing replaces them); links are put into the field only when
//! they carry no account or token. Pure; unit-tested below.

use crate::SourceId;
use iptv_core::wizard::{Field, FieldError, normalize_mac, validate_field};
use iptv_providers::model::{MacAddress, Source, SourceConfig, SourceKind};
use iptv_providers::{Secret, SecretUrl};

use crate::playlists::plain_link;

/// One detail of a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Param {
    Link,
    Server,
    Username,
    Password,
    Portal,
    Mac,
}

impl Param {
    pub fn of(kind: SourceKind) -> &'static [Param] {
        match kind {
            SourceKind::M3u => &[Param::Link],
            SourceKind::Xtream => &[Param::Server, Param::Username, Param::Password],
            SourceKind::Stalker => &[Param::Portal, Param::Mac],
        }
    }

    pub fn field(self) -> Field {
        match self {
            Param::Link => Field::PlaylistUrl,
            Param::Server => Field::Server,
            Param::Username => Field::Username,
            Param::Password => Field::Password,
            Param::Portal => Field::PortalUrl,
            Param::Mac => Field::Mac,
        }
    }

    /// Never shown, never put into a field.
    pub fn secret(self) -> bool {
        self.field().is_secret()
    }

    pub fn is_link(self) -> bool {
        matches!(self, Param::Link | Param::Server | Param::Portal)
    }
}

/// Server and portal addresses may omit the scheme; it is `https` then.
fn with_scheme(address: &str) -> String {
    if address.contains("://") {
        address.to_owned()
    } else {
        format!("https://{address}")
    }
}

/// The form: the stored source and the edited copy. Focus `None` is the
/// "Check and save" button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    pub id: SourceId,
    original: Source,
    edited: Source,
    pub focus: Option<Param>,
    pub error: Option<(Param, FieldError)>,
    /// The copy is being imported.
    pub checking: bool,
}

impl Form {
    pub fn new(id: SourceId, source: Source) -> Self {
        let first = Param::of(source.kind()).first().copied();
        Self {
            id,
            original: source.clone(),
            edited: source,
            focus: first,
            error: None,
            checking: false,
        }
    }

    pub fn kind(&self) -> SourceKind {
        self.edited.kind()
    }

    pub fn params(&self) -> &'static [Param] {
        Param::of(self.kind())
    }

    pub fn edited(&self) -> &Source {
        &self.edited
    }

    pub fn changed(&self) -> bool {
        self.edited != self.original
    }

    /// The value as stored or typed (secrets included: for display through
    /// masking only).
    pub fn value(&self, p: Param) -> String {
        match (&self.edited.config, p) {
            (SourceConfig::M3u(c), Param::Link) => c.location.expose().to_owned(),
            (SourceConfig::Xtream(c), Param::Server) => c.server_url.clone(),
            (SourceConfig::Xtream(c), Param::Username) => c.username.expose().to_owned(),
            (SourceConfig::Xtream(c), Param::Password) => c.password.expose().to_owned(),
            (SourceConfig::Stalker(c), Param::Portal) => c.portal_url.clone(),
            (SourceConfig::Stalker(c), Param::Mac) => c.mac.expose().to_owned(),
            _ => String::new(),
        }
    }

    /// What text entry starts with: the value when it can be shown.
    pub fn prefill(&self, p: Param) -> Option<String> {
        let value = self.value(p);
        let shown = !p.secret() && (!p.is_link() || plain_link(&value));
        shown.then_some(value)
    }

    pub fn move_focus(&mut self, step: isize) {
        let mut items: Vec<Option<Param>> = self.params().iter().copied().map(Some).collect();
        items.push(None);
        let at = items.iter().position(|i| *i == self.focus).unwrap_or(0);
        let to = (at as isize + step).clamp(0, items.len() as isize - 1) as usize;
        self.focus = items[to];
    }

    /// Typed `text` for `p`: checked as the wizard checks it, then kept in
    /// the copy. An error leaves the copy as it was.
    pub fn set(&mut self, p: Param, text: &str) -> Result<(), FieldError> {
        self.error = None;
        let checked = validate_field(p.field(), text).and_then(|()| {
            let text = if p == Param::Password {
                text.to_owned()
            } else {
                text.trim().to_owned()
            };
            let mac = || normalize_mac(&text).and_then(|m| MacAddress::parse(&m));
            match (&mut self.edited.config, p) {
                (SourceConfig::M3u(c), Param::Link) => c.location = SecretUrl::new(text),
                (SourceConfig::Xtream(c), Param::Server) => c.server_url = with_scheme(&text),
                (SourceConfig::Xtream(c), Param::Username) => c.username = Secret::new(text),
                (SourceConfig::Xtream(c), Param::Password) => c.password = Secret::new(text),
                (SourceConfig::Stalker(c), Param::Portal) => c.portal_url = with_scheme(&text),
                (SourceConfig::Stalker(c), Param::Mac) => {
                    c.mac = mac().ok_or(FieldError::InvalidMac)?;
                }
                _ => {}
            }
            Ok(())
        });
        if let Err(e) = checked {
            self.error = Some((p, e));
        }
        checked
    }
}

#[cfg(test)]
mod tests {
    use iptv_providers::model::{M3uSourceConfig, StalkerCredentials, XtreamCredentials};

    use super::*;

    const ID: SourceId = SourceId(4);

    fn xtream() -> Source {
        Source {
            name: "Synthetic panel".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: "http://panel.example.test:8080".into(),
                username: Secret::new("synthetic-user"),
                password: Secret::new("synthetic-pass"),
                user_agent: Some("Synthetic/1".into()),
            }),
        }
    }

    #[test]
    fn logins_are_never_put_into_a_field() {
        let form = Form::new(ID, xtream());
        assert_eq!(
            form.params(),
            [Param::Server, Param::Username, Param::Password]
        );
        assert_eq!(
            form.prefill(Param::Server).as_deref(),
            Some("http://panel.example.test:8080")
        );
        assert_eq!(form.prefill(Param::Username), None);
        assert_eq!(form.prefill(Param::Password), None);
        // Debug never shows them either.
        assert!(!format!("{form:?}").contains("synthetic-pass"));
        let m3u = Source {
            name: "List".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("http://lists.example.test/get.php?username=u&password=p"),
                user_agent: None,
                referrer: None,
            }),
        };
        assert_eq!(Form::new(ID, m3u).prefill(Param::Link), None);
    }

    #[test]
    fn edits_are_checked_and_kept_in_the_copy_only() {
        let mut form = Form::new(ID, xtream());
        assert!(!form.changed());
        assert_eq!(
            form.set(Param::Server, "panel.example.test:x"),
            Err(FieldError::InvalidPort)
        );
        assert_eq!(form.error, Some((Param::Server, FieldError::InvalidPort)));
        assert!(!form.changed());
        form.set(Param::Server, " backup.example.test:8080 ")
            .unwrap();
        assert_eq!(
            form.value(Param::Server),
            "https://backup.example.test:8080"
        );
        assert_eq!(form.set(Param::Username, " "), Err(FieldError::Required));
        form.set(Param::Password, " new pass ").unwrap();
        assert_eq!(form.value(Param::Password), " new pass ", "kept as typed");
        assert!(form.changed());
        // The rest of the source (its User-Agent) is untouched.
        match &form.edited().config {
            SourceConfig::Xtream(c) => assert_eq!(c.user_agent.as_deref(), Some("Synthetic/1")),
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_mac_address_is_normalized() {
        let stalker = Source {
            name: "Portal".into(),
            config: SourceConfig::Stalker(StalkerCredentials {
                portal_url: "http://portal.example.test/c/".into(),
                mac: MacAddress::from_stored("00:1A:79:00:00:01"),
                serial: None,
                device_id: None,
                device_id2: None,
                signature: None,
                signature2: None,
                login: None,
                password: None,
            }),
        };
        let mut form = Form::new(ID, stalker);
        assert_eq!(
            form.set(Param::Mac, "00-1a-79"),
            Err(FieldError::InvalidMac)
        );
        form.set(Param::Mac, "001a.7900.0002").unwrap();
        assert_eq!(form.value(Param::Mac), "00:1A:79:00:00:02");
        form.move_focus(1);
        assert_eq!(form.focus, Some(Param::Mac));
        form.move_focus(5);
        assert_eq!(form.focus, None, "the check button is last");
        form.move_focus(-9);
        assert_eq!(form.focus, Some(Param::Portal));
    }
}
