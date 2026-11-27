//! Browse through the system responder when it owns the multicast socket.
use super::Receiver;
use dbus::{Path, blocking::Connection};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};

const AVAHI: &str = "org.freedesktop.Avahi";
const SERVER: &str = "org.freedesktop.Avahi.Server";
const BROWSER: &str = "org.freedesktop.Avahi.ServiceBrowser";
const RESOLVER: &str = "org.freedesktop.Avahi.ServiceResolver";
const SERVICE: &str = "_googlecast._tcp";
const IPV4: i32 = 0;

pub(super) fn discover(deadline: Instant) -> Result<Vec<Receiver>, dbus::Error> {
    browse(&Connection::new_system()?, deadline)
}

fn browse(bus: &Connection, deadline: Instant) -> Result<Vec<Receiver>, dbus::Error> {
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
        (-1_i32, IPV4, SERVICE, "local", 0_u32),
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
                if ty != SERVICE || domain != "local" || protocol != IPV4 {
                    continue;
                }
                let service = (interface, name);
                if member == "ItemRemove" {
                    services.remove(&service);
                } else {
                    services.insert(service);
                }
            }
            _ => {}
        }
    }
    resolve(bus, services, deadline)
}

fn resolve(
    bus: &Connection,
    services: BTreeSet<(i32, String)>,
    deadline: Instant,
) -> Result<Vec<Receiver>, dbus::Error> {
    bus.add_match_no_cb("type='signal',sender='org.freedesktop.Avahi',interface='org.freedesktop.Avahi.ServiceResolver'")?;
    let until = (Instant::now() + Duration::from_secs(2)).min(deadline);
    let mut pending = BTreeSet::new();
    // Start every lookup before waiting on a TV that may have left the LAN.
    for (interface, name) in services {
        let Some(remaining) = until.checked_duration_since(Instant::now()) else {
            break;
        };
        let server = bus.with_proxy(AVAHI, "/", remaining);
        let resolver: Result<(Path<'static>,), _> = server.method_call(
            SERVER,
            "ServiceResolverNew",
            (interface, IPV4, name, SERVICE, "local", IPV4, 0_u32),
        );
        if let Ok((path,)) = resolver {
            pending.insert(path);
        }
    }
    let mut found = BTreeMap::new();
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
        let (_, _, name, _, _, _, _, ip, port, txt, _): Resolved = message.read_all()?;
        if let Ok(ip) = ip.parse::<Ipv4Addr>() {
            let field = |key| {
                txt.iter()
                    .filter_map(|v| std::str::from_utf8(v).ok())
                    .find_map(|value| value.strip_prefix(key))
            };
            let address = SocketAddr::from((ip, port));
            if let Some(receiver) =
                Receiver::from_record(address, field("fn="), field("ca="), field("id="))
            {
                found.entry(name).or_insert(receiver);
            }
        }
    }
    // Avahi releases the browser and resolvers when this private D-Bus connection closes.
    Ok(found.into_values().collect())
}

#[cfg(test)]
mod tests;
