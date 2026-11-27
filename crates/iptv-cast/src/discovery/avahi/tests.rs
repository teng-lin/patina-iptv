//! A private message bus and generated services exercise Avahi's asynchronous API.
use super::*;
use dbus::{
    Message,
    channel::{Channel, Sender},
};
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
};

struct Bus(Child);
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn connection(address: &str) -> Connection {
    let mut channel = Channel::open_private(address).unwrap();
    channel.register().unwrap();
    channel.into()
}

fn private_bus() -> (Bus, String) {
    let mut bus = Bus(Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap());
    let mut address = String::new();
    BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    (bus, address.trim().to_owned())
}

#[test]
fn an_unavailable_avahi_service_returns_an_error() {
    let (_bus, address) = private_bus();
    assert!(
        browse(
            &connection(&address),
            Instant::now() + Duration::from_secs(2)
        )
        .is_err()
    );
}

#[test]
fn public_discovery_keeps_early_avahi_signals_and_filters_unusable_services() {
    const CHILD: &str = "PATINA_CAST_DISCOVERY_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let receivers = runtime.block_on(crate::discovery::discover()).unwrap();
        assert_eq!(receivers.len(), 1);
        let tv = &receivers[0];
        assert!(tv.name == "Test TV", "the fixture TV should be selected");
        assert!(
            tv.address == ([127, 0, 0, 1], 8009).into()
                || tv.address == ([127, 0, 0, 2], 8009).into()
        );
        return;
    }
    let (_bus, address) = private_bus();
    let server = connection(&address);
    server.request_name(AVAHI, false, true, false).unwrap();
    let worker = std::thread::spawn(move || {
        let mut resolved = 0;
        while resolved < 5 {
            let m = server
                .channel()
                .blocking_pop_message(Duration::from_secs(5))
                .unwrap()
                .expect("generated method call");
            if m.interface().as_deref() != Some(SERVER) {
                continue;
            }
            match m.member().as_deref() {
                Some("ServiceBrowserNew") => {
                    let path = Path::new("/browser").unwrap();
                    server
                        .send(Message::new_signal(&*path, BROWSER, "CacheExhausted").unwrap())
                        .unwrap();
                    server
                        .send(
                            Message::new_signal(&*path, BROWSER, "ItemNew")
                                .unwrap()
                                .append3(1_i32, 1_i32, "IPv6 only")
                                .append3(SERVICE, "local", 0_u32),
                        )
                        .unwrap();
                    for (name, link, event) in [
                        ("Test TV", 1, "ItemNew"),
                        ("Test TV", 2, "ItemNew"),
                        ("Test speaker", 1, "ItemNew"),
                        ("Bad address", 1, "ItemNew"),
                        ("Resolution failed", 1, "ItemNew"),
                        ("Removed", 1, "ItemNew"),
                        ("Removed", 1, "ItemRemove"),
                    ] {
                        let signal = Message::new_signal(&*path, BROWSER, event)
                            .unwrap()
                            .append3(link, IPV4, name)
                            .append3(SERVICE, "local", 0_u32);
                        server.send(signal).unwrap();
                    }
                    server
                        .send(Message::new_signal(&*path, BROWSER, "AllForNow").unwrap())
                        .unwrap();
                    server.send(m.method_return().append1(path)).unwrap();
                }
                Some("ServiceResolverNew") => {
                    let (link, _, name): (i32, i32, String) = m.read3().unwrap();
                    assert_ne!(name, "Removed");
                    let path = Path::new(format!("/resolve{resolved}")).unwrap();
                    resolved += 1;
                    if name == "Resolution failed" {
                        server
                            .send(Message::new_signal(&*path, RESOLVER, "Failure").unwrap())
                            .unwrap();
                        server.send(m.method_return().append1(path)).unwrap();
                        continue;
                    }
                    let cap = if name == "Test speaker" {
                        "ca=4"
                    } else {
                        "ca=5"
                    };
                    let txt = vec![format!("fn={name}").into_bytes(), cap.as_bytes().to_vec()];
                    let ip = if name == "Bad address" {
                        "invalid".into()
                    } else {
                        format!("127.0.0.{link}")
                    };
                    let found = Message::new_signal(&*path, RESOLVER, "Found")
                        .unwrap()
                        .append3(link, IPV4, name)
                        .append3(SERVICE, "local", "test.local")
                        .append3(IPV4, ip, 8009_u16)
                        .append2(txt, 0_u32);
                    server.send(found).unwrap();
                    server.send(m.method_return().append1(path)).unwrap();
                }
                other => panic!("unexpected Avahi request {other:?}"),
            }
        }
        server.channel().flush();
    });
    // libdbus reads the system bus address from the environment. Use a child so
    // every other test keeps its own process environment.
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            concat!(
                module_path!(),
                "::public_discovery_keeps_early_avahi_signals_and_filters_unusable_services"
            )
            .split_once("::")
            .unwrap()
            .1,
        ])
        .env(CHILD, "1")
        .env("DBUS_SYSTEM_BUS_ADDRESS", address)
        .output()
        .unwrap();
    worker.join().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "public discovery failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
