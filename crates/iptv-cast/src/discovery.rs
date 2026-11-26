//! A bounded snapshot of receivers on the current network.
#[cfg(target_os = "linux")]
mod avahi;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::{collections::BTreeMap, io, net::SocketAddr, time::Duration};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receiver {
    pub name: String,
    pub address: SocketAddr,
}

impl Receiver {
    fn from_record(
        address: SocketAddr,
        name: Option<&str>,
        capabilities: Option<&str>,
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
        })
    }
}

pub async fn discover() -> io::Result<Vec<Receiver>> {
    #[cfg(target_os = "linux")]
    if let Ok(Ok(found)) = tokio::task::spawn_blocking(avahi::discover).await
        && !found.is_empty()
    {
        return Ok(found);
    }
    let daemon = ServiceDaemon::new().map_err(io::Error::other)?;
    browse_mdns(daemon, Duration::from_secs(3)).await
}

async fn browse_mdns(daemon: ServiceDaemon, duration: Duration) -> io::Result<Vec<Receiver>> {
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
    let until = tokio::time::Instant::now() + duration;
    let mut found = BTreeMap::new();
    while let Ok(Ok(event)) = tokio::time::timeout_at(until, events.recv_async()).await {
        record_event(&mut found, event);
    }
    Ok(found.into_values().collect())
}

fn record_event(found: &mut BTreeMap<String, Receiver>, event: ServiceEvent) {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            if let Some(ip) = info.get_addresses_v4().into_iter().min()
                && let Some(receiver) = Receiver::from_record(
                    SocketAddr::from((ip, info.get_port())),
                    info.get_property_val_str("fn"),
                    info.get_property_val_str("ca"),
                )
            {
                found.insert(info.get_fullname().to_owned(), receiver);
            } else {
                found.remove(info.get_fullname());
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
