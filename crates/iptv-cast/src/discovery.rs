//! A bounded snapshot of receivers on the current network.
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::{collections::BTreeMap, io, net::SocketAddr, time::Duration};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receiver {
    pub name: String,
    pub address: SocketAddr,
}

pub async fn discover() -> io::Result<Vec<Receiver>> {
    const SERVICE: &str = "_googlecast._tcp.local.";
    let daemon = ServiceDaemon::new().map_err(io::Error::other)?;
    struct Browse(ServiceDaemon);
    impl Drop for Browse {
        fn drop(&mut self) {
            let _ = self.0.stop_browse(SERVICE);
            let _ = self.0.shutdown();
        }
    }
    let browse = Browse(daemon);
    let events = browse.0.browse(SERVICE).map_err(io::Error::other)?;
    let until = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut found = BTreeMap::new();
    while let Ok(Ok(event)) = tokio::time::timeout_at(until, events.recv_async()).await {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                if info
                    .get_property_val_str("ca")
                    .and_then(|caps| caps.parse::<u32>().ok())
                    .is_some_and(|caps| caps & 1 == 0)
                {
                    continue;
                }
                if let Some(ip) = info.get_addresses_v4().into_iter().min() {
                    let name = info.get_property_val_str("fn").unwrap_or("Chromecast");
                    found.insert(
                        info.get_fullname().to_owned(),
                        Receiver {
                            name: name
                                .chars()
                                .filter(|ch| !ch.is_control())
                                .take(80)
                                .collect(),
                            address: SocketAddr::from((ip, info.get_port())),
                        },
                    );
                }
            }
            ServiceEvent::ServiceRemoved(_, name) => {
                found.remove(&name);
            }
            _ => {}
        }
    }
    Ok(found.into_values().collect())
}
