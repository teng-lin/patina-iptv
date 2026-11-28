//! Browse through the system responder when it owns the multicast socket.
use super::Receiver;
use dbus::{Path, blocking::Connection};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant},
};

const AVAHI: &str = "org.freedesktop.Avahi";
const SERVER: &str = "org.freedesktop.Avahi.Server";
const BROWSER: &str = "org.freedesktop.Avahi.ServiceBrowser";
const RESOLVER: &str = "org.freedesktop.Avahi.ServiceResolver";
const SERVICE: &str = "_googlecast._tcp";
#[cfg(feature = "airplay")]
const AIRPLAY_SERVICE: &str = "_airplay._tcp";
const PROTO_UNSPEC: i32 = -1;
const IPV4: i32 = 0;
const IPV6: i32 = 1;

pub(crate) struct ResolvedRecord {
    #[cfg_attr(
        not(feature = "airplay"),
        expect(dead_code, reason = "used by AirPlay IPv6 scope resolution")
    )]
    pub(crate) interface: i32,
    pub(crate) name: String,
    pub(crate) address: IpAddr,
    pub(crate) port: u16,
    pub(crate) txt: Vec<Vec<u8>>,
}

pub(super) fn discover(deadline: Instant) -> Result<Vec<Receiver>, dbus::Error> {
    browse(&Connection::new_system()?, deadline)
}

#[cfg(feature = "airplay")]
pub(crate) fn discover_airplay(deadline: Instant) -> Result<Vec<ResolvedRecord>, dbus::Error> {
    browse_records(
        &Connection::new_system()?,
        AIRPLAY_SERVICE,
        PROTO_UNSPEC,
        deadline,
    )
}

fn browse(bus: &Connection, deadline: Instant) -> Result<Vec<Receiver>, dbus::Error> {
    let records = browse_records(bus, SERVICE, IPV4, deadline)?;
    let mut found = BTreeMap::new();
    for record in records {
        if let IpAddr::V4(ip) = record.address {
            let field = |key| {
                record
                    .txt
                    .iter()
                    .filter_map(|v| std::str::from_utf8(v).ok())
                    .find_map(|value| value.strip_prefix(key))
            };
            let address = SocketAddr::from((ip, record.port));
            if let Some(receiver) =
                Receiver::from_record(address, field("fn="), field("ca="), field("id="))
            {
                found.entry((record.name, address)).or_insert(receiver);
            }
        }
    }
    Ok(found.into_values().collect())
}

fn browse_records(
    bus: &Connection,
    service_type: &str,
    protocol_filter: i32,
    deadline: Instant,
) -> Result<Vec<ResolvedRecord>, dbus::Error> {
    bus.add_match_no_cb("type='signal',sender='org.freedesktop.Avahi',interface='org.freedesktop.Avahi.ServiceBrowser'")?;
    let server = bus.with_proxy(
        AVAHI,
        "/",
        deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(1)),
    );
    let (browser,): (Path<'static>,) = server.method_call(
        SERVER,
        "ServiceBrowserNew",
        (-1_i32, protocol_filter, service_type, "local", 0_u32),
    )?;
    let until = (Instant::now() + Duration::from_secs(2)).min(deadline);
    let mut services = BTreeSet::new();
    while let Some(remaining) = until.checked_duration_since(Instant::now()) {
        let Some(message) = bus.channel().blocking_pop_message(remaining)? else {
            break;
        };
        if message.path().as_ref() != Some(&browser)
            || message.interface().as_deref() != Some(BROWSER)
        {
            continue;
        }
        match message.member().as_deref() {
            Some("AllForNow") => break,
            Some("Failure") => return Err(dbus::Error::new_failed("Avahi discovery failed")),
            Some(member @ ("ItemNew" | "ItemRemove")) => {
                let (interface, protocol, name, ty, domain, _): (
                    i32,
                    i32,
                    String,
                    String,
                    String,
                    u32,
                ) = message.read_all()?;
                let protocol_matches = if protocol_filter == PROTO_UNSPEC {
                    protocol == IPV4 || protocol == IPV6
                } else {
                    protocol == protocol_filter
                };
                if ty != service_type || domain != "local" || !protocol_matches {
                    continue;
                }
                let service = (interface, protocol, name);
                if member == "ItemRemove" {
                    services.remove(&service);
                } else {
                    services.insert(service);
                }
            }
            _ => {}
        }
    }
    resolve_records(bus, service_type, services, deadline)
}

fn resolve_records(
    bus: &Connection,
    service_type: &str,
    services: BTreeSet<(i32, i32, String)>,
    deadline: Instant,
) -> Result<Vec<ResolvedRecord>, dbus::Error> {
    bus.add_match_no_cb("type='signal',sender='org.freedesktop.Avahi',interface='org.freedesktop.Avahi.ServiceResolver'")?;
    let until = (Instant::now() + Duration::from_secs(2)).min(deadline);
    let mut pending = BTreeSet::new();
    // Start every lookup before waiting on a TV that may have left the LAN.
    for (interface, protocol, name) in services {
        let Some(remaining) = until.checked_duration_since(Instant::now()) else {
            break;
        };
        let server = bus.with_proxy(AVAHI, "/", remaining);
        let resolver: Result<(Path<'static>,), _> = server.method_call(
            SERVER,
            "ServiceResolverNew",
            (
                interface,
                protocol,
                name,
                service_type,
                "local",
                protocol,
                0_u32,
            ),
        );
        if let Ok((path,)) = resolver {
            pending.insert(path);
        }
    }
    let mut found = Vec::new();
    while !pending.is_empty() {
        let Some(remaining) = until.checked_duration_since(Instant::now()) else {
            break;
        };
        let Some(message) = bus.channel().blocking_pop_message(remaining)? else {
            break;
        };
        if message.interface().as_deref() != Some(RESOLVER) {
            continue;
        }
        let Some(path) = message.path() else { continue };
        if !pending.remove(&path.into_static()) || message.member().as_deref() != Some("Found") {
            continue;
        }
        type Resolved = (
            i32,
            i32,
            String,
            String,
            String,
            String,
            i32,
            String,
            u16,
            Vec<Vec<u8>>,
            u32,
        );
        let (interface, _, name, _, _, _, _, ip, port, txt, _): Resolved = message.read_all()?;
        if let Ok(address) = ip.parse::<IpAddr>() {
            found.push(ResolvedRecord {
                interface,
                name,
                address,
                port,
                txt,
            });
        }
    }
    // Avahi releases the browser and resolvers when this private D-Bus connection closes.
    Ok(found)
}

#[cfg(test)]
mod tests;
