//! Receiver-owned PIN approval and desktop host configuration.
#[cfg(any(target_os = "linux", windows))]
use iptv_cast::CastBuilder;
use iptv_cast::{Connection, Pairing, PinCode};
use iptv_core::cast::Error;
use iptv_runtime::cast::PairingPrompt;
use std::time::Duration;
#[cfg(any(target_os = "linux", windows))]
use std::{
    ffi::{OsStr, OsString},
    num::NonZeroU16,
    path::Path,
};
use tokio::time::{Instant, timeout_at};

#[cfg(any(target_os = "linux", windows))]
const DEFAULT_TIMING_PORT: NonZeroU16 = NonZeroU16::new(49153).expect("non-zero timing port");

#[cfg(any(target_os = "linux", windows))]
pub(super) struct Configuration {
    #[cfg(not(feature = "test-support"))]
    store: crate::host::native::credential_key::CastStore,
    timing_port: Option<OsString>,
}

#[cfg(any(target_os = "linux", windows))]
impl Configuration {
    pub(super) fn new(directory: &Path, pairing_label: String) -> Self {
        #[cfg(feature = "test-support")]
        let _ = (directory, pairing_label);
        Self {
            #[cfg(not(feature = "test-support"))]
            store: crate::host::native::credential_key::CastStore::new(directory, pairing_label),
            timing_port: std::env::var_os("PATINA_AIRPLAY_TIMING_PORT"),
        }
    }

    pub(super) fn configure(&self, builder: CastBuilder) -> CastBuilder {
        // The timing configuration is checked when an AirPlay route is chosen
        // so a bad value cannot prevent Chromecast discovery.
        let builder = builder
            .airplay_enabled(true)
            .airplay_timing_port(timing_port(self.timing_port.as_deref()).ok())
            // Only an explicit port is a hard requirement.
            .airplay_timing_port_fallback(self.timing_port.is_none());
        #[cfg(not(feature = "test-support"))]
        {
            builder.credential_store(self.store.clone())
        }
        #[cfg(feature = "test-support")]
        {
            builder
        }
    }

    pub(super) fn validate(&self) -> Result<(), Error> {
        timing_port(self.timing_port.as_deref()).map(drop)
    }
}

#[cfg(any(target_os = "linux", windows))]
fn timing_port(value: Option<&OsStr>) -> Result<NonZeroU16, Error> {
    let Some(value) = value else {
        return Ok(DEFAULT_TIMING_PORT);
    };
    value
        .to_str()
        .and_then(|port| port.parse().ok())
        .ok_or(Error::InvalidAirPlayTimingPort)
}

pub(super) async fn pair(
    pairing: Pairing,
    prompt: PairingPrompt,
    current: impl FnOnce() -> Result<(), Error>,
) -> Result<Connection, Error> {
    let deadline = pairing.expires_at().map_or_else(
        || Instant::now() + Duration::from_secs(30),
        Instant::from_std,
    );
    let code = match timeout_at(deadline, prompt.request()).await {
        Ok(Ok(code)) => code,
        Ok(Err(error)) => {
            pairing.cancel().await;
            return Err(error);
        }
        Err(_) => {
            pairing.cancel().await;
            return Err(Error::PairingRejected);
        }
    };
    let pin = match PinCode::parse(&code) {
        Ok(pin) => pin,
        Err(error) => {
            pairing.cancel().await;
            return Err(super::map_error(&error));
        }
    };
    if let Err(error) = current() {
        pairing.cancel().await;
        return Err(error);
    }
    pairing
        .submit(pin)
        .await
        .map_err(|error| super::map_error(&error))
}

#[cfg(all(test, any(target_os = "linux", windows)))]
mod tests {
    use super::*;

    #[test]
    fn timing_listener_uses_default_or_explicit_port() {
        let parse = |value: Option<&str>| timing_port(value.map(OsStr::new));
        assert_eq!(parse(None::<&str>), Ok(DEFAULT_TIMING_PORT));
        assert_eq!(parse(Some("47475")), Ok(NonZeroU16::new(47475).unwrap()));
        for bad in ["", "0", "65536", "12x"] {
            assert_eq!(parse(Some(bad)), Err(Error::InvalidAirPlayTimingPort));
        }
    }
}
