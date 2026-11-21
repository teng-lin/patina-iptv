//! Values that must never reach logs: passwords, tokens, MACs, serials.
//!
//! Both types overwrite their text with zeros when dropped, so a secret
//! does not stay readable in freed memory (a core dump, a later
//! allocation). This covers the value they own; a copy made from
//! [`Secret::expose`] (a request URL, a header) is the caller's to wipe.

use std::fmt;

use zeroize::Zeroize;

/// A secret string. `Debug` and `Display` print `***`; read the value with
/// [`Secret::expose`] only where it goes on the wire or into secure storage.
///
/// Deliberately not `serde::Serialize`: persisting a secret is an explicit
/// adapter decision (keystore), never a side effect of serializing a model.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct Secret(String);

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret value. Callers must not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A copy with surrounding whitespace removed.
    pub fn trimmed(&self) -> Self {
        Self(self.0.trim().to_owned())
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// A URL that may carry credentials (an Xtream `get.php` playlist URL, a
/// tokenized CDN link). `Debug`/`Display` show only `scheme://host`.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct SecretUrl(String);

impl Drop for SecretUrl {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl SecretUrl {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    /// `scheme://host[:port]` without path, query or userinfo, for display.
    pub fn display_origin(&self) -> String {
        match url::Url::parse(self.0.trim()) {
            Ok(url) => match url.host_str() {
                Some(host) => match url.port() {
                    Some(port) => format!("{}://{host}:{port}", url.scheme()),
                    None => format!("{}://{host}", url.scheme()),
                },
                None => format!("{}:", url.scheme()),
            },
            Err(_) => "(local)".to_owned(),
        }
    }
}

impl fmt::Debug for SecretUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretUrl({}/***)", self.display_origin())
    }
}

impl fmt::Display for SecretUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/***", self.display_origin())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_and_display_redact() {
        let s = Secret::new("hunter2-synthetic");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert_eq!(format!("{s}"), "***");
        assert_eq!(s.expose(), "hunter2-synthetic");
    }

    #[test]
    fn secrets_are_wiped_when_dropped() {
        // Freed memory cannot be read back safely, so this checks the two
        // halves: both types run code on drop, and that code (`zeroize` on
        // the owned text) overwrites and empties the value in place.
        assert!(std::mem::needs_drop::<Secret>() && std::mem::needs_drop::<SecretUrl>());
        let mut secret = Secret::new("hunter2-synthetic");
        let copy = secret.clone();
        secret.0.zeroize();
        assert!(secret.is_empty());
        assert_eq!(
            copy.expose(),
            "hunter2-synthetic",
            "a clone owns its own text"
        );
        let mut url = SecretUrl::new("http://tv.example.com/get.php?password=synthetic");
        url.0.zeroize();
        assert_eq!(url.expose(), "");
    }

    #[test]
    fn secret_url_shows_origin_only() {
        let u = SecretUrl::new("http://user:pw@tv.example.com:8080/get.php?username=u&password=p");
        let shown = format!("{u:?} {u}");
        assert!(shown.contains("http://tv.example.com:8080"));
        assert!(!shown.contains("password"));
        assert!(!shown.contains("pw"));
    }
}
