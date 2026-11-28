//! A bounded snapshot of receivers on the current network.
#[cfg(all(target_os = "linux", feature = "avahi"))]
pub(crate) mod avahi;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::{collections::BTreeMap, io, net::SocketAddr, time::Duration};
use tokio::time::Instant;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receiver {
    pub name: String,
    pub address: SocketAddr,
    pub(crate) id: Option<String>,
}

impl Receiver {
    fn from_record(
        address: SocketAddr,
        name: Option<&str>,
        capabilities: Option<&str>,
        id: Option<&str>,
    ) -> Option<Self> {
        if capabilities
            .and_then(|value| value.parse::<u32>().ok())
            .is_some_and(|caps| caps & 1 == 0)
        {
            return None;
        }
        let name: String = name
            .unwrap_or_default()
            .chars()
            .filter(|ch| !ch.is_control())
            .take(80)
            .collect();
        Some(Self {
            name: if name.trim().is_empty() {
                "Chromecast".into()
            } else {
                name
            },
            address,
            id: id
                .filter(|id| {
                    !id.is_empty()
                        && id.len() <= 128
                        && id.bytes().all(|byte| byte.is_ascii_graphic())
                })
                .map(str::to_ascii_lowercase),
        })
    }
}

pub async fn discover() -> io::Result<Vec<Receiver>> {
    discover_until(Instant::now() + Duration::from_secs(5)).await
}

pub(crate) async fn discover_until(deadline: Instant) -> io::Result<Vec<Receiver>> {
    #[cfg(all(target_os = "linux", feature = "avahi"))]
    {
        // Keep part of the common budget for mDNS if the system resolver
        // immediately fails or cannot see this network.
        let avahi_deadline =
            std::time::Instant::now() + deadline.saturating_duration_since(Instant::now()) / 2;
        if let Ok(Ok(found)) =
            tokio::task::spawn_blocking(move || avahi::discover(avahi_deadline)).await
            && !found.is_empty()
        {
            return Ok(found);
        }
    }
    let daemon = ServiceDaemon::new().map_err(io::Error::other)?;
    if deadline <= Instant::now() {
        return Err(io::ErrorKind::TimedOut.into());
    }
    browse_mdns(daemon, deadline).await
}

async fn browse_mdns(daemon: ServiceDaemon, until: Instant) -> io::Result<Vec<Receiver>> {
    const SERVICE: &str = "_googlecast._tcp.local.";
    struct Browse(ServiceDaemon);
    impl Drop for Browse {
        fn drop(&mut self) {
            let _ = self.0.stop_browse(SERVICE);
            let _ = self.0.shutdown();
        }
    }
    let browse = Browse(daemon);
    let events = browse.0.browse(SERVICE).map_err(io::Error::other)?;
    let mut found = BTreeMap::new();
    while let Ok(Ok(event)) = tokio::time::timeout_at(until, events.recv_async()).await {
        record_event(&mut found, event);
    }
    Ok(found.into_values().flatten().collect())
}

fn record_event(found: &mut BTreeMap<String, Vec<Receiver>>, event: ServiceEvent) {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            let mut addresses = info.get_addresses_v4().into_iter().collect::<Vec<_>>();
            addresses.sort_unstable();
            let receivers: Vec<_> = addresses
                .into_iter()
                .filter_map(|ip| {
                    Receiver::from_record(
                        SocketAddr::from((ip, info.get_port())),
                        info.get_property_val_str("fn"),
                        info.get_property_val_str("ca"),
                        info.get_property_val_str("id"),
                    )
                })
                .collect();
            if receivers.is_empty() {
                found.remove(info.get_fullname());
            } else {
                found.insert(info.get_fullname().to_owned(), receivers);
            }
        }
        ServiceEvent::ServiceRemoved(_, name) => {
            found.remove(&name);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
