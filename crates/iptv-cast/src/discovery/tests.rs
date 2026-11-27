use super::*;

#[test]
fn records_filter_audio_only_devices_and_sanitize_display_names() {
    let address = SocketAddr::from(([127, 0, 0, 1], 8009));
    assert!(Receiver::from_record(address, Some("Generated speaker"), Some("4"), None).is_none());
    let tv = Receiver::from_record(address, Some("Generated\nTV"), Some("5"), Some("GENERATED"))
        .unwrap();
    assert_eq!(tv.name, "GeneratedTV");
    assert_eq!(tv.id.as_deref(), Some("generated"));
    let unnamed = Receiver::from_record(address, Some("\0 "), None, None).unwrap();
    assert_eq!(unnamed.name, "Chromecast");
}

const SERVICE: &str = "_googlecast._tcp.local.";

fn resolved(name: &str, ips: &str, port: u16, capabilities: &str) -> ServiceEvent {
    let record = mdns_sd::ServiceInfo::new(
        SERVICE,
        name,
        "generated.local.",
        ips,
        port,
        &[("fn", name), ("ca", capabilities)][..],
    )
    .unwrap();
    ServiceEvent::ServiceResolved(Box::new(record.as_resolved_service()))
}

#[test]
fn updated_records_replace_addresses_and_removed_services_leave_the_picker() {
    let mut found = BTreeMap::new();
    record_event(&mut found, resolved("B", "127.0.0.9", 8008, "1"));
    record_event(&mut found, resolved("A", "127.0.0.5", 8009, "1"));
    record_event(&mut found, resolved("B", "127.0.0.8,127.0.0.2", 8010, "5"));
    assert_eq!(
        found.values().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["A", "B"]
    );
    assert_eq!(
        found[&format!("B.{SERVICE}")].address,
        SocketAddr::from(([127, 0, 0, 2], 8010))
    );
    record_event(
        &mut found,
        ServiceEvent::ServiceRemoved(SERVICE.into(), format!("B.{SERVICE}")),
    );
    assert_eq!(
        found.into_values().map(|r| r.name).collect::<Vec<_>>(),
        ["A"]
    );
}

#[test]
fn a_receiver_that_loses_video_or_ipv4_is_removed_from_the_snapshot() {
    let mut found = BTreeMap::new();
    for (ips, capabilities) in [("127.0.0.1", "4"), ("::1", "1")] {
        record_event(&mut found, resolved("Generated", "127.0.0.1", 8009, "5"));
        assert_eq!(found.len(), 1);
        record_event(&mut found, resolved("Generated", ips, 8009, capabilities));
        assert!(
            found.is_empty(),
            "stale entry for {ips}, capabilities {capabilities}"
        );
    }
}

#[tokio::test]
async fn empty_mdns_scan_returns_at_its_deadline_and_stops_the_daemon() {
    let daemon = ServiceDaemon::new_with_port(0).unwrap();
    daemon.disable_interface(mdns_sd::IfKind::All).unwrap();
    let monitor = daemon.monitor().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        let duration = Duration::from_millis(40);
        let started = tokio::time::Instant::now();
        let receivers = browse_mdns(daemon.clone(), started + duration)
            .await
            .unwrap();
        assert!(receivers.is_empty());
        assert!(
            started.elapsed() >= duration,
            "scan ended before its deadline"
        );
        // Keep a handle alive to verify the browse guard explicitly shut down the worker.
        while monitor.recv_async().await.is_ok() {}
        drop(daemon);
    })
    .await
    .expect("the scan and its mDNS worker should both finish");
}

#[tokio::test]
async fn dropping_a_pending_scan_stops_its_daemon() {
    let daemon = ServiceDaemon::new_with_port(0).unwrap();
    daemon.disable_interface(mdns_sd::IfKind::All).unwrap();
    let monitor = daemon.monitor().unwrap();
    {
        let scan = browse_mdns(daemon.clone(), Instant::now() + Duration::from_secs(30));
        tokio::pin!(scan);
        tokio::select! {
            biased;
            result = &mut scan => panic!("the scan should still be waiting: {result:?}"),
            () = std::future::ready(()) => {}
        }
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while monitor.recv_async().await.is_ok() {}
    })
    .await
    .expect("dropping the scan should turn off the mDNS daemon");
    drop(daemon);
}
