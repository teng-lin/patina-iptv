//! Receiver-owned PIN approval and Linux host configuration.
#[cfg(target_os = "linux")]
use iptv_cast::CastBuilder;
use iptv_cast::{Connection, Pairing, PinCode};
use iptv_core::cast::Error;
use iptv_runtime::cast::PairingPrompt;
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::{
    ffi::{OsStr, OsString},
    num::NonZeroU16,
    path::{Path, PathBuf},
};
use tokio::time::{Instant, timeout_at};

#[cfg(target_os = "linux")]
pub(super) struct Configuration {
    directory: PathBuf,
    pairing_label: String,
    timing_port: Option<OsString>,
}

#[cfg(target_os = "linux")]
impl Configuration {
    pub(super) fn from_env(directory: &Path, pairing_label: String) -> Option<Self> {
        (std::env::var_os("PATINA_RUST_AIRPLAY").as_deref() == Some(OsStr::new("1"))).then(|| {
            Self {
                directory: directory.to_owned(),
                pairing_label,
                timing_port: std::env::var_os("PATINA_AIRPLAY_TIMING_PORT"),
            }
        })
    }

    pub(super) fn configure(&self, builder: CastBuilder) -> CastBuilder {
        // The timing configuration is checked when an AirPlay route is chosen
        // so a bad value cannot prevent Chromecast discovery.
        let builder = builder
            .airplay_enabled(true)
            .airplay_timing_port(timing_port(self.timing_port.as_deref()).unwrap_or_default());
        #[cfg(not(feature = "test-support"))]
        {
            use crate::host::native::credential_key::CastStore;
            builder.credential_store(CastStore::new(&self.directory, self.pairing_label.clone()))
        }
        #[cfg(feature = "test-support")]
        {
            let _ = (&self.directory, &self.pairing_label);
            builder
        }
    }

    pub(super) fn validate(&self) -> Result<(), Error> {
        timing_port(self.timing_port.as_deref()).map(drop)
    }
}

#[cfg(target_os = "linux")]
fn timing_port(value: Option<&OsStr>) -> Result<Option<NonZeroU16>, Error> {
    value
        .map(|value| {
            value
                .to_str()
                .and_then(|port| port.parse().ok())
                .ok_or(Error::InvalidAirPlayTimingPort)
        })
        .transpose()
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn timing_listener_can_use_an_ephemeral_or_explicit_port() {
        let parse = |value: Option<&str>| timing_port(value.map(OsStr::new));
        assert_eq!(parse(None::<&str>), Ok(None));
        assert_eq!(parse(Some("47475")), Ok(NonZeroU16::new(47475)));
        for bad in ["", "0", "65536", "12x"] {
            assert_eq!(parse(Some(bad)), Err(Error::InvalidAirPlayTimingPort));
        }
    }
}
