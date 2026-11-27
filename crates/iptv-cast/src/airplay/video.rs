//! AirPlay v2 URL playback on a verified, dedicated transport.
use super::{
    hap::{self, AuthFault, Verified},
    wire::{Fault, Start, Version, Wire},
};
use plist::{
    Dictionary, Value,
    stream::{Event, Reader},
};
use std::{
    io::{self, Cursor},
    net::{SocketAddr, SocketAddrV6},
    num::NonZeroU16,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::UdpSocket,
    sync::Notify,
    task::JoinHandle,
    time::{self, timeout},
};

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const FEEDBACK_PERIOD: Duration = Duration::from_secs(2);
const PLIST_TYPE: &str = "application/x-apple-binary-plist";
const RTSP_AGENT: (&str, &str) = ("User-Agent", "AirPlay/550.10");
const VIDEO_AGENT: (&str, &str) = ("User-Agent", "AirPlay/870.14.1");
const MAX_PLIST_EVENTS: usize = 4_096;
const MAX_PLIST_DEPTH: usize = 24;
const MAX_PLIST_DATA: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum VideoFault {
    Wire(Fault),
    TimedOut,
    Timing { local: SocketAddr, timed_out: bool },
    Unavailable,
}

impl From<Fault> for VideoFault {
    fn from(error: Fault) -> Self {
        Self::Wire(error)
    }
}

impl From<AuthFault> for VideoFault {
    fn from(_: AuthFault) -> Self {
        Self::Unavailable
    }
}

type Result<T> = std::result::Result<T, VideoFault>;

fn dictionary<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Dictionary(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn pack(value: &Value) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    value
        .to_writer_binary(&mut out)
        .map_err(|_| Fault::InvalidMessage)?;
    Ok(out)
}

fn unpack(data: &[u8]) -> Result<Value> {
    if !data.starts_with(b"bplist00") {
        return Err(Fault::InvalidMessage.into());
    }
    // A short object table can reference the same subtree many times. Bound
    // the expanded event stream, including repeated payloads, before the
    // Value builder allocates its arrays and dictionaries.
    let mut events = Vec::new();
    let mut depth = 0usize;
    let mut total_bytes = 0usize;
    for event in Reader::new(Cursor::new(data)) {
        if events.len() == MAX_PLIST_EVENTS {
            return Err(Fault::InvalidMessage.into());
        }
        let event = event.map_err(|_| Fault::InvalidMessage)?;
        match &event {
            Event::StartArray(_) | Event::StartDictionary(_) => {
                depth += 1;
                if depth > MAX_PLIST_DEPTH {
                    return Err(Fault::InvalidMessage.into());
                }
            }
            Event::EndCollection => {
                depth = depth.checked_sub(1).ok_or(Fault::InvalidMessage)?;
            }
            Event::Data(bytes) => total_bytes += bytes.len(),
            Event::String(text) => total_bytes += text.len(),
            _ => {}
        }
        if total_bytes > MAX_PLIST_DATA {
            return Err(Fault::InvalidMessage.into());
        }
        events.push(event);
    }
    Value::from_events(events.into_iter().map(Ok)).map_err(|_| Fault::InvalidMessage.into())
}

fn find<'a>(value: &'a Value, field: &str) -> Option<&'a Value> {
    value.as_dictionary()?.get(field)
}

fn ntp_timestamp(now: SystemTime) -> [u8; 8] {
    let elapsed = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = (elapsed.as_secs() as u32).wrapping_add(2_208_988_800);
    let fraction = ((u64::from(elapsed.subsec_nanos()) << 32) / 1_000_000_000) as u32;
    let mut timestamp = [0; 8];
    timestamp[..4].copy_from_slice(&seconds.to_be_bytes());
    timestamp[4..].copy_from_slice(&fraction.to_be_bytes());
    timestamp
}

fn timing_reply(request: &[u8], now: SystemTime) -> Option<[u8; 32]> {
    if request.len() != 32 || request[0] >> 6 != 2 || request[1] & 0x7f != 0x52 {
        return None;
    }
    let mut reply = [0; 32];
    reply[..4].copy_from_slice(&[request[0], 0xd3, 0, 7]);
    reply[8..16].copy_from_slice(&request[24..32]);
    let received = ntp_timestamp(now);
    reply[16..24].copy_from_slice(&received);
    reply[24..].copy_from_slice(&received);
    Some(reply)
}

fn same_host(left: SocketAddr, right: SocketAddr) -> bool {
    match (left, right) {
        (SocketAddr::V6(left), SocketAddr::V6(right)) => {
            left.ip() == right.ip() && left.scope_id() == right.scope_id()
        }
        _ => left.ip() == right.ip(),
    }
}

fn datagram_result<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::Interrupted
                    | io::ErrorKind::WouldBlock
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::NetworkUnreachable
                    | io::ErrorKind::HostUnreachable
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

async fn timing_loop(
    socket: UdpSocket,
    peer: SocketAddr,
    replied: &AtomicBool,
) -> std::io::Result<()> {
    // Receive the entire UDP datagram. Some platforms return an error
    // instead of truncating packets that exceed the receive buffer.
    let mut data = vec![0; u16::MAX.into()];
    loop {
        let Some((len, source)) = datagram_result(socket.recv_from(&mut data).await)? else {
            continue;
        };
        if same_host(source, peer)
            && let Some(response) = timing_reply(&data[..len], SystemTime::now())
            && datagram_result(socket.send_to(&response, source).await)?.is_some()
        {
            replied.store(true, Ordering::Release);
        }
    }
}

struct Task(JoinHandle<()>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort()
    }
}

struct Rtsp {
    wire: Verified,
    session: String,
    uri: String,
    instance: String,
    active_remote: String,
    sequence: u32,
}

impl Rtsp {
    fn new(wire: Verified, address: SocketAddr) -> Result<Self> {
        let session = hap::uuid()?;
        let instance = session[..8].to_owned();
        let active_remote = u32::from_str_radix(&instance, 16)
            .expect("UUID starts with hexadecimal")
            .to_string();
        let host = match address {
            SocketAddr::V4(address) => address.ip().to_string(),
            SocketAddr::V6(address) => format!("[{}]", address.ip()),
        };
        let uri = format!("rtsp://{host}/{active_remote}");
        Ok(Self {
            wire,
            session,
            uri,
            instance,
            active_remote,
            sequence: 0,
        })
    }

    async fn exchange(
        &mut self,
        action: &str,
        path: Option<&str>,
        body: &[u8],
    ) -> Result<super::wire::Message> {
        let current = self.sequence;
        self.sequence = self.sequence.checked_add(1).ok_or(Fault::InvalidMessage)?;
        let sequence = current.to_string();
        let mut headers = vec![
            RTSP_AGENT,
            ("CSeq", sequence.as_str()),
            ("DACP-ID", self.instance.as_str()),
            ("Active-Remote", self.active_remote.as_str()),
            ("Client-Instance", self.instance.as_str()),
        ];
        if !body.is_empty() {
            headers.push(("Content-Type", PLIST_TYPE));
        }
        let response = timeout(
            EXCHANGE_TIMEOUT,
            self.wire.wire.request(
                Version::Rtsp,
                action,
                path.unwrap_or(&self.uri),
                &headers,
                body,
            ),
        )
        .await
        .map_err(|_| VideoFault::TimedOut)??;
        if response.sequence != Some(current) {
            return Err(Fault::InvalidMessage.into());
        }
        Ok(response)
    }

    async fn setup(&mut self, fields: &Value) -> Result<Value> {
        let body = pack(fields)?;
        let reply = self.exchange("SETUP", None, &body).await?.require_ok()?;
        unpack(&reply.body)
    }

    async fn command(&mut self, stream: &str, body: &[u8]) -> Result<()> {
        let headers = [
            VIDEO_AGENT,
            ("Content-Type", PLIST_TYPE),
            ("X-Apple-ProtocolVersion", "1"),
            ("X-Apple-Session-ID", self.session.as_str()),
            ("X-Apple-StreamID", stream),
            ("CSeq", "1"),
        ];
        timeout(
            EXCHANGE_TIMEOUT,
            self.wire
                .wire
                .request(Version::Http, "POST", "/command", &headers, body),
        )
        .await
        .map_err(|_| VideoFault::TimedOut)??
        .require_ok()?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MediaState {
    Buffering,
    Playing,
    Paused,
    Ended,
    Absent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Evidence {
    pub(super) state: MediaState,
    pub(super) position: Option<Duration>,
    pub(super) duration: Option<Duration>,
}

/// Coalesce progress updates, retaining the last one ahead of a terminal event.
/// The dedicated event channel can report before the control channel acknowledges
/// insertion. These observations become usable only after startup succeeds.
#[derive(Default)]
struct Observations {
    pending: Mutex<PendingObservations>,
    ready: Notify,
}

#[derive(Default)]
struct PendingObservations {
    accepted: bool,
    observed_active: bool,
    latest: Option<Evidence>,
    terminal: Option<Result<Evidence>>,
}

impl Observations {
    fn accept(&self) {
        self.pending.lock().unwrap().accepted = true;
        self.ready.notify_one();
    }

    fn media(&self, evidence: Evidence, explicitly_owned: bool) {
        let mut pending = self.pending.lock().unwrap();
        if pending.terminal.is_some() {
            return;
        }
        if matches!(evidence.state, MediaState::Ended | MediaState::Absent) {
            if !explicitly_owned && !pending.observed_active {
                return;
            }
            pending.terminal = Some(Ok(evidence));
        } else {
            pending.observed_active = true;
            pending.latest = Some(evidence);
        }
        drop(pending);
        self.ready.notify_one();
    }

    fn fault(&self, fault: VideoFault) {
        self.pending
            .lock()
            .unwrap()
            .terminal
            .get_or_insert(Err(fault));
        self.ready.notify_one();
    }

    async fn next(&self) -> Result<Evidence> {
        loop {
            let observation = {
                let mut pending = self.pending.lock().unwrap();
                if pending.accepted {
                    pending.latest.take().map(Ok).or(pending.terminal)
                } else {
                    None
                }
            };
            if let Some(observation) = observation {
                return observation;
            }
            self.ready.notified().await;
        }
    }
}

fn time_property(dictionary: &Dictionary, property: &str) -> Option<Duration> {
    let value = dictionary.get(property)?;
    let seconds = value
        .as_real()
        .or_else(|| value.as_unsigned_integer().map(|number| number as f64))?;
    Duration::try_from_secs_f64(seconds).ok()
}

fn parse_event(body: &[u8], item_id: &str) -> Option<(Evidence, bool)> {
    let packet = unpack(body).ok()?;
    let bytes = find(find(&packet, "params")?, "data")?.as_data()?;
    let event = unpack(bytes).ok()?;
    if find(&event, "type")?.as_string()? != "playbackState" || find(&event, "kind").is_some() {
        return None;
    }
    let parameters = find(&event, "params").and_then(Value::as_dictionary);
    // The event socket and stream belong exclusively to this load. When the
    // receiver also supplies an explicit item ID, reject a different item.
    let explicitly_owned = if let Some(item) = parameters.and_then(|fields| fields.get("item")) {
        if find(item, "uuid")?.as_string()? != item_id {
            return None;
        }
        true
    } else {
        false
    };
    let observed = parameters
        .and_then(|fields| fields.get("playbackState"))
        .or_else(|| find(&event, "name"))?;
    let state = match observed.as_string()? {
        "buffering" => MediaState::Buffering,
        "playing" => MediaState::Playing,
        "paused" => MediaState::Paused,
        "ended" => MediaState::Ended,
        "stopped" => MediaState::Absent,
        _ => return None,
    };
    Some((
        Evidence {
            state,
            position: parameters.and_then(|fields| time_property(fields, "position")),
            duration: parameters.and_then(|fields| time_property(fields, "duration")),
        },
        explicitly_owned,
    ))
}

async fn event_loop(
    mut wire: Wire,
    item: String,
    dispatched: Arc<AtomicBool>,
    evidence: &Observations,
) -> Result<()> {
    loop {
        let message = wire.receive().await?;
        let was_dispatched = dispatched.load(Ordering::Acquire);
        let acknowledged = timeout(EXCHANGE_TIMEOUT, wire.ack_event(&message)).await;
        if was_dispatched
            && message.start == Start::Request
            && let Some((observation, explicitly_owned)) = parse_event(&message.body, &item)
        {
            evidence.media(observation, explicitly_owned);
        }
        acknowledged.map_err(|_| VideoFault::TimedOut)??;
    }
}

async fn event_connection(mut address: SocketAddr, port: u16, verified: &Verified) -> Result<Wire> {
    address.set_port(port);
    let mut wire = timeout(EVENT_TIMEOUT, async {
        // The event listener can become ready slightly after the SETUP reply.
        loop {
            match Wire::connect(address).await {
                Ok(wire) => break Ok(wire),
                Err(Fault::Unreachable) => time::sleep(Duration::from_millis(150)).await,
                Err(error) => break Err(error),
            }
        }
    })
    .await
    .map_err(|_| VideoFault::TimedOut)??;
    verified.encrypt_events(&mut wire);
    Ok(wire)
}

fn event_port(response: &Value) -> Result<u16> {
    find(response, "eventPort")
        .and_then(Value::as_unsigned_integer)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0)
        .ok_or(Fault::InvalidMessage.into())
}

fn stream_id(response: &Value) -> Result<String> {
    find(response, "streams")
        .and_then(Value::as_array)
        .and_then(|streams| (streams.len() == 1).then(|| &streams[0]))
        .and_then(|stream| find(stream, "streamID"))
        .and_then(Value::as_unsigned_integer)
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
        .ok_or(Fault::InvalidMessage.into())
}

fn base_fields(device: &str, session: &str, timing: u16) -> Result<Value> {
    // These media-session setup fields follow the pinned pyatv v2 URL path.
    Ok(dictionary([
        ("deviceID", device.into()),
        ("macAddress", device.into()),
        ("sessionUUID", session.into()),
        ("sessionCorrelationUUID", hap::uuid()?.into()),
        ("timingPort", u64::from(timing).into()),
        ("timingProtocol", "NTP".into()),
        ("isMultiSelectAirPlay", true.into()),
        ("groupContainsGroupLeader", false.into()),
        ("model", "iPhone14,3".into()),
        ("name", "Patina".into()),
        ("osBuildVersion", "20F66".into()),
        ("osName", "iPhone OS".into()),
        ("osVersion", "16.5".into()),
        ("senderSupportsRelay", false.into()),
        ("sourceVersion", "690.7.1".into()),
        ("statsCollectionEnabled", false.into()),
    ]))
}

fn video_fields(device: &str) -> Result<Value> {
    let stream = dictionary([
        ("clientUUID", hap::uuid()?.into()),
        (
            "clientTypeUUID",
            "A6B27562-B43A-4F2D-B75F-82391E250194".into(),
        ),
        ("channelID", format!("{device}-RCS-1").into()),
        ("controlType", 1u64.into()),
        ("type", 130u64.into()),
    ]);
    Ok(dictionary([("streams", Value::Array(vec![stream]))]))
}

fn command_body(body: Value) -> Result<Vec<u8>> {
    pack(&dictionary([(
        "params",
        dictionary([("data", Value::Data(pack(&body)?))]),
    )]))
}

pub(super) struct Queue {
    stream: String,
    commands: [Vec<u8>; 4],
}

pub(super) struct Video {
    control: Option<Rtsp>,
    addresses: (SocketAddr, SocketAddr),
    item: String,
    dispatched: Arc<AtomicBool>,
    observations: Arc<Observations>,
    tasks: Vec<Task>,
}

fn timing_address(
    mut local: SocketAddr,
    remote: SocketAddr,
    port: Option<NonZeroU16>,
) -> SocketAddr {
    // Accepted TCP sockets do not always carry a scope on their local
    // address, but a link-local UDP bind requires one.
    if let (SocketAddr::V6(local_v6), SocketAddr::V6(remote_v6)) = (local, remote)
        && local_v6.ip().is_unicast_link_local()
        && local_v6.scope_id() == 0
    {
        local = SocketAddr::V6(SocketAddrV6::new(
            *local_v6.ip(),
            0,
            0,
            remote_v6.scope_id(),
        ));
    }
    local.set_port(port.map(NonZeroU16::get).unwrap_or(0));
    local
}

impl Video {
    pub(super) fn new(verified: Verified) -> Result<Self> {
        let addresses = verified.wire.addresses()?;
        let control = Rtsp::new(verified, addresses.0)?;
        Ok(Self {
            control: Some(control),
            addresses,
            item: hap::uuid()?,
            dispatched: Arc::new(AtomicBool::new(false)),
            observations: Arc::new(Observations::default()),
            tasks: Vec::new(),
        })
    }

    pub(super) fn addresses(&self) -> (SocketAddr, SocketAddr) {
        self.addresses
    }

    pub(super) async fn prepare(
        &mut self,
        timing_port: Option<NonZeroU16>,
        url: &str,
    ) -> Result<Queue> {
        let (local, remote) = self.addresses;
        let local = timing_address(local, remote, timing_port);
        let socket = UdpSocket::bind(local)
            .await
            .map_err(|_| VideoFault::Timing {
                local,
                timed_out: false,
            })?;
        let local = socket.local_addr().map_err(|_| VideoFault::Timing {
            local,
            timed_out: false,
        })?;
        let observations = self.observations.clone();
        let timing_replied = Arc::new(AtomicBool::new(false));
        let timing_observed = Arc::clone(&timing_replied);
        self.tasks.push(Task(tokio::spawn(async move {
            if timing_loop(socket, remote, &timing_observed).await.is_err() {
                observations.fault(VideoFault::Timing {
                    local,
                    timed_out: false,
                });
            }
        })));

        let control = self.control.as_mut().ok_or(VideoFault::Unavailable)?;
        let body = base_fields(&control.wire.device_id, &control.session, local.port())?;
        let session = control.setup(&body).await.map_err(|fault| match fault {
            VideoFault::TimedOut if !timing_replied.load(Ordering::Acquire) => VideoFault::Timing {
                local,
                timed_out: true,
            },
            other => other,
        })?;
        let event = event_connection(remote, event_port(&session)?, &control.wire).await?;
        let observations = self.observations.clone();
        let dispatched = Arc::clone(&self.dispatched);
        let item = self.item.clone();
        self.tasks.push(Task(tokio::spawn(async move {
            if let Err(error) = event_loop(event, item, dispatched, &observations).await {
                observations.fault(error);
            }
        })));

        // /info is optional. A refusal is fine; a transport failure invalidates
        // the connection and must not be followed by a write on the same socket.
        control.exchange("GET", Some("/info"), &[]).await?;
        control.exchange("RECORD", None, &[]).await?.require_ok()?;
        let stream = stream_id(
            &control
                .setup(&video_fields(&control.wire.device_id)?)
                .await?,
        )?;
        let item = dictionary([("uuid", self.item.as_str().into())]);
        let queued_item = dictionary([
            ("uuid", self.item.as_str().into()),
            ("mediaType", "file".into()),
            ("Content-Location", url.into()),
        ]);
        let commands = [
            command_body(dictionary([
                ("type", "insertPlayQueueItem".into()),
                ("item", queued_item),
            ]))?,
            command_body(dictionary([
                ("type", "setProperty".into()),
                ("value", true.into()),
                ("property", "isInterestedInDateRange".into()),
                ("item", item),
            ]))?,
            command_body(dictionary([
                ("type", "setProperty".into()),
                ("value", 1u64.into()),
                ("property", "actionAtItemEnd".into()),
            ]))?,
            command_body(dictionary([
                ("type", "setRate".into()),
                ("rate", 1.0f64.into()),
            ]))?,
        ];
        Ok(Queue { stream, commands })
    }

    pub(super) async fn start(&mut self, queue: Queue) -> Result<()> {
        let control = self.control.as_mut().ok_or(VideoFault::Unavailable)?;
        self.dispatched.store(true, Ordering::Release);
        for command in queue.commands {
            control.command(&queue.stream, &command).await?;
        }
        self.observations.accept();
        let mut control = self.control.take().expect("control still owned");
        let observations = self.observations.clone();
        self.tasks.push(Task(tokio::spawn(async move {
            loop {
                time::sleep(FEEDBACK_PERIOD).await;
                // Some receivers decline feedback. A present reply is enough
                // to establish liveness of this dedicated control transport.
                if let Err(error) = control.exchange("POST", Some("/feedback"), &[]).await {
                    observations.fault(error);
                    break;
                }
            }
        })));
        Ok(())
    }

    pub(super) async fn observe(&mut self) -> Result<Evidence> {
        self.observations.next().await
    }

    pub(super) async fn close(&mut self) {
        drop(self.control.take());
        for task in &self.tasks {
            task.0.abort();
        }
        for mut task in self.tasks.drain(..) {
            let _ = (&mut task.0).await;
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
