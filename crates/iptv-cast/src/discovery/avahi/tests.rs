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

#[test]
fn early_signals_are_retained_and_removed_or_audio_only_services_are_hidden() {
    let mut bus = Bus(Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap());
    let mut address = String::new();
    BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let client = connection(address.trim());
    let server = connection(address.trim());
    server.request_name(AVAHI, false, true, false).unwrap();
    let worker = std::thread::spawn(move || {
        let mut resolved = 0;
        while resolved < 3 {
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
                    for (name, link, event) in [
                        ("Test TV", 1, "ItemNew"),
                        ("Test TV", 2, "ItemNew"),
                        ("Test speaker", 1, "ItemNew"),
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
                    let cap = if name == "Test speaker" {
                        "ca=4"
                    } else {
                        "ca=5"
                    };
                    let txt = vec![format!("fn={name}").into_bytes(), cap.as_bytes().to_vec()];
                    let found = Message::new_signal(&*path, RESOLVER, "Found")
                        .unwrap()
                        .append3(link, IPV4, name)
                        .append3(SERVICE, "local", "test.local")
                        .append3(IPV4, format!("127.0.0.{link}"), 8009_u16)
                        .append2(txt, 0_u32);
                    server.send(found).unwrap();
                    server.send(m.method_return().append1(path)).unwrap();
                }
                other => panic!("unexpected Avahi request {other:?}"),
            }
        }
        server.channel().flush();
    });
    let receivers = browse(&client).unwrap();
    worker.join().unwrap();
    assert_eq!(receivers.len(), 1);
    let tv = &receivers[0];
    assert_eq!(tv.name, "Test TV");
    assert!(
        tv.address == ([127, 0, 0, 1], 8009).into() || tv.address == ([127, 0, 0, 2], 8009).into()
    );
}
