//! Cast v2 control of the Default Media Receiver.
use crate::{
    Delivery, tls,
    wire::{self, Envelope, MAX_FRAME},
};
use prost::Message;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    net::TcpStream,
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{Instant, timeout, timeout_at},
};
use tokio_rustls::rustls::pki_types::ServerName;

pub(crate) const RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
pub(crate) const MEDIA: &str = "urn:x-cast:com.google.cast.media";
pub(crate) const CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
pub(crate) const HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
pub(crate) const ROOT: &str = "receiver-0";
pub(crate) const APP: &str = "CC1AD845";
const DEADLINE: Duration = Duration::from_secs(15);
const LATE_LOAD_REPLY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unreachable,
    Refused,
    LoadFailed,
    Unknown,
    Ended,
    OwnershipLost { dispatched: bool },
    TimedOut,
}
type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Volume(u8),
    SetMuted(bool),
    StepVolume(i8),
    ToggleMute,
    Play,
    Pause,
    Seek(Duration),
    Stop,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Playing,
    Paused,
    Buffering,
    Finished,
    Interrupted,
    Canceled,
    Failed,
    Idle,
    Unknown,
}
struct Request {
    namespace: &'static str,
    destination: String,
    body: Value,
    retention: Duration,
    reply: oneshot::Sender<Result<Value>>,
}
struct Pending {
    namespace: &'static str,
    destination: String,
    deadline: Instant,
    reply: oneshot::Sender<Result<Value>>,
}

/// A connection owns a single socket task. Dropping the last handle closes
/// the socket while leaving remote playback alone.
#[derive(Clone)]
pub struct Session {
    outgoing: mpsc::Sender<Request>,

    local_addr: SocketAddr,
    peer_addr: SocketAddr,
    driver: Option<Arc<JoinHandle<()>>>,
    transport: String,
}
impl Session {
    pub async fn connect(address: SocketAddr) -> Result<Self> {
        Self::open(address, true, &mut Delivery::NotSent).await
    }
    /// Attaches only to an already running instance of the media receiver.
    pub async fn rejoin(address: SocketAddr) -> Result<Self> {
        Self::open(address, false, &mut Delivery::NotSent).await
    }
    pub(crate) async fn connect_bounded(
        address: SocketAddr,
        deadline: Instant,
    ) -> std::result::Result<Self, (Error, Delivery)> {
        let mut delivery = Delivery::NotSent;
        timeout_at(deadline, Self::open(address, true, &mut delivery))
            .await
            .unwrap_or(Err(Error::TimedOut))
            .map_err(|error| (error, delivery))
    }

    async fn open(address: SocketAddr, launch: bool, delivery: &mut Delivery) -> Result<Self> {
        let (socket, local_addr) = timeout(DEADLINE, async {
            let socket = TcpStream::connect(address)
                .await
                .map_err(|_| Error::Unreachable)?;
            let local = socket.local_addr().map_err(|_| Error::Unreachable)?;
            let socket = tls::connector()
                .map_err(|_| Error::Unreachable)?
                .connect(ServerName::IpAddress(address.ip().into()), socket)
                .await
                .map_err(|_| Error::Unreachable)?;
            Ok((socket, local))
        })
        .await
        .map_err(|_| Error::Unreachable)??;
        let (tx, rx) = mpsc::channel(16);

        let sender = format!("sender-{:032x}", random_token()?);
        let driver = tokio::spawn(drive(socket, rx, sender));
        let mut session = Self {
            outgoing: tx,

            local_addr,
            peer_addr: address,
            driver: Some(Arc::new(driver)),
            transport: String::new(),
        };
        session.connect_channel(ROOT).await?;
        let status = session
            .request(RECEIVER, ROOT, json!({"type":"GET_STATUS"}))
            .await?;
        session.transport = if let Some(id) = app_transport(&status) {
            id
        } else if launch {
            session.launch(delivery).await?
        } else {
            return Err(Error::Ended);
        };
        session.connect_channel(&session.transport).await?;
        if !launch {
            let reply = session.media(json!({"type":"GET_STATUS"})).await?;
            media_entries(&reply)?;
        }
        Ok(session)
    }
    pub fn local_ip(&self) -> IpAddr {
        self.local_addr.ip()
    }
    pub(crate) fn control_addrs(&self) -> (SocketAddr, SocketAddr) {
        (self.local_addr, self.peer_addr)
    }
    pub(crate) async fn close(mut self) {
        let driver = self.driver.take().and_then(Arc::into_inner);
        drop(self.outgoing);
        if let Some(driver) = driver {
            let _ = driver.await;
        }
    }
    async fn connect_channel(&self, destination: &str) -> Result<()> {
        self.request(
            CONNECTION,
            destination,
            json!({"type":"CONNECT","origin":{}}),
        )
        .await
        .map(drop)
    }
    async fn request(
        &self,
        namespace: &'static str,
        destination: &str,
        body: Value,
    ) -> Result<Value> {
        let (reply, rx) = oneshot::channel();
        timeout(DEADLINE, async {
            self.outgoing
                .send(Request {
                    namespace,
                    destination: destination.into(),
                    body,
                    retention: DEADLINE,
                    reply,
                })
                .await
                .map_err(|_| Error::Unknown)?;
            rx.await.map_err(|_| Error::Unknown)?
        })
        .await
        .map_err(|_| Error::Unknown)?
    }
    pub(crate) async fn media(&self, body: Value) -> Result<Value> {
        self.request(MEDIA, &self.transport, body).await
    }
    /// Loads live HLS at the live edge. Only a reply for this request establishes ownership.
    pub async fn load(&self, url: &str) -> Result<i64> {
        let permit = self.request_slot().await?;
        let request = self.queue_load(permit, load_body(url, "application/x-mpegURL", true, None));
        let reply = timeout(DEADLINE, request)
            .await
            .map_err(|_| Error::Unknown)?
            .map_err(|_| Error::Unknown)??;
        load_instance(&reply, url, None)
    }
    async fn request_slot(&self) -> Result<mpsc::Permit<'_, Request>> {
        timeout(DEADLINE, self.outgoing.reserve())
            .await
            .map_err(|_| Error::Unknown)?
            .map_err(|_| Error::Unreachable)
    }

    fn queue_load(
        &self,
        permit: mpsc::Permit<'_, Request>,
        body: Value,
    ) -> oneshot::Receiver<Result<Value>> {
        Self::queue(
            permit,
            MEDIA,
            &self.transport,
            body,
            DEADLINE + LATE_LOAD_REPLY,
        )
    }

    fn queue(
        permit: mpsc::Permit<'_, Request>,
        namespace: &'static str,
        destination: &str,
        body: Value,
        retention: Duration,
    ) -> oneshot::Receiver<Result<Value>> {
        let (reply, received) = oneshot::channel();
        permit.send(Request {
            namespace,
            destination: destination.into(),
            body,
            retention,
            reply,
        });
        received
    }

    async fn launch(&self, delivery: &mut Delivery) -> Result<String> {
        let permit = self.request_slot().await?;
        *delivery = Delivery::MayHaveReachedReceiver;
        let received = Self::queue(
            permit,
            RECEIVER,
            ROOT,
            json!({"type":"LAUNCH","appId":APP}),
            DEADLINE,
        );
        match timeout(DEADLINE, received).await {
            Ok(Ok(Ok(response))) => {
                *delivery = Delivery::Acknowledged;
                app_transport(&response).ok_or(Error::Refused)
            }
            Ok(Ok(Err(Error::Refused))) => {
                *delivery = Delivery::Acknowledged;
                Err(Error::Refused)
            }
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(Error::Unreachable),
            Err(_) => Err(Error::TimedOut),
        }
    }

    /// Finds matching content for diagnostics. Use correlated load replies to establish ownership.
    pub async fn lookup(&self, url: &str) -> Result<Option<i64>> {
        let reply = self.media(json!({"type":"GET_STATUS"})).await?;
        let mut unidentified = false;
        for item in media_entries(&reply)? {
            match item.pointer("/media/contentId").and_then(Value::as_str) {
                Some(content) if content == url => return Ok(item["mediaSessionId"].as_i64()),
                Some(_) => {}
                // The receiver may omit unchanged media metadata.
                None => unidentified = true,
            }
        }
        if unidentified {
            return Err(Error::Unknown);
        }
        Ok(None)
    }

    pub async fn status(&self, id: i64) -> Result<Status> {
        let reply = self.media(json!({"type":"GET_STATUS"})).await?;
        state(&reply, id)
    }
    pub async fn command(&self, id: i64, cmd: Command) -> Result<()> {
        self.command_owned(id, cmd, None).await
    }

    pub(crate) async fn command_owned(
        &self,
        id: i64,
        cmd: Command,
        token: Option<&str>,
    ) -> Result<()> {
        let value = match cmd {
            Command::Volume(level) => Some(json!({"level":f64::from(level.min(100))/100.0})),
            Command::SetMuted(muted) => Some(json!({"muted":muted})),
            Command::StepVolume(_) | Command::ToggleMute => {
                let status = self.receiver_status().await?;
                let volume = &status["status"]["volume"];
                Some(if let Command::StepVolume(step) = cmd {
                    let level = volume["level"]
                        .as_f64()
                        .filter(|level| (0.0..=1.0).contains(level))
                        .ok_or(Error::Unknown)?;
                    json!({"level":(level+f64::from(step)/100.0).clamp(0.0,1.0)})
                } else {
                    json!({"muted":!volume["muted"].as_bool().ok_or(Error::Unknown)?})
                })
            }
            _ => None,
        };
        if let Some(volume) = value {
            // For receiver-wide changes this status check is the last wire
            // round trip before the write, including after reading output.
            let report = self.media(json!({"type":"GET_STATUS"})).await?;
            check_load_tag(&report, id, token, false)?;
            match state(&report, id)? {
                Status::Playing | Status::Paused | Status::Buffering => {}
                Status::Unknown => return Err(Error::Unknown),
                _ => return Err(Error::Ended),
            }
            self.request(RECEIVER, ROOT, json!({"type":"SET_VOLUME","volume":volume}))
                .await?;
            return Ok(());
        }
        let verb = match cmd {
            Command::Play => "PLAY",
            Command::Pause => "PAUSE",
            Command::Seek(_) => "SEEK",
            Command::Stop => "STOP",
            _ => return Err(Error::Refused),
        };
        let mut request = json!({"type":verb,"mediaSessionId":id});
        if let Command::Seek(position) = cmd {
            request["currentTime"] = position.as_secs_f64().into();
        }
        let reply = self.media(request).await?;
        check_load_tag(&reply, id, token, true)?;
        timeout(DEADLINE, async {
            let mut reply = if cmd == Command::Stop {
                self.media(json!({"type":"GET_STATUS"})).await?
            } else {
                reply
            };
            loop {
                check_load_tag(&reply, id, token, true)?;
                match (cmd, state(&reply, id)) {
                    (Command::Play, Ok(Status::Playing | Status::Buffering))
                    | (Command::Pause, Ok(Status::Paused))
                    | (Command::Stop, Ok(Status::Canceled)) => return Ok(()),
                    (Command::Stop, Err(Error::Ended)) if media_entries(&reply)?.is_empty() => {
                        return Ok(());
                    }
                    (Command::Seek(position), Ok(Status::Playing | Status::Paused))
                        if media_entry(&reply, id)?
                            .get("currentTime")
                            .and_then(Value::as_f64)
                            .is_some_and(|time| (time - position.as_secs_f64()).abs() < 1.0) =>
                    {
                        return Ok(());
                    }
                    (
                        _,
                        Ok(
                            Status::Finished
                            | Status::Interrupted
                            | Status::Canceled
                            | Status::Failed,
                        ),
                    ) => return Err(Error::Ended),
                    (_, Err(error)) => return Err(error),
                    (_, Ok(_)) => {}
                }
                // The acknowledgement can precede the actual playback change.
                tokio::time::sleep(Duration::from_millis(250)).await;
                reply = self.media(json!({"type":"GET_STATUS"})).await?;
            }
        })
        .await
        .map_err(|_| Error::Unknown)?
    }
    pub(crate) async fn receiver_status(&self) -> Result<Value> {
        self.request(RECEIVER, ROOT, json!({"type":"GET_STATUS"}))
            .await
    }
}

pub(crate) fn random_token() -> Result<u128> {
    let mut bytes = [0; 16];
    tokio_rustls::rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut bytes)
        .map_err(|_| Error::Unreachable)?;
    Ok(u128::from_be_bytes(bytes))
}

pub(crate) fn load_body(url: &str, content_type: &str, live: bool, token: Option<&str>) -> Value {
    let mut body = json!({
        "type":"LOAD",
        "autoplay":true,
        "media":{
            "contentId":url,
            "contentType":content_type,
            "streamType":if live {"LIVE"} else {"BUFFERED"}
        }
    });
    if let Some(token) = token {
        body["media"]["customData"] = json!({"castRsLoad":token});
    }
    body
}

/// A matching response needs a unique media instance. An echoed, different tag
/// identifies someone else's load even when both senders supplied the same URL.
pub(crate) fn load_instance(reply: &Value, url: &str, token: Option<&str>) -> Result<i64> {
    let mut matches = media_entries(reply)?.iter().filter(|entry| {
        entry.pointer("/media/contentId").and_then(Value::as_str) == Some(url)
            && load_tag(entry).is_none_or(|tag| Some(tag) == token)
    });
    let entry = matches.next().ok_or(Error::Unknown)?;
    if matches.next().is_some() {
        return Err(Error::Unknown);
    }
    entry["mediaSessionId"].as_i64().ok_or(Error::Unknown)
}

/// Status queries may recover a lost load reply only if its unique tag was
/// echoed by the receiver. The media URL alone does not distinguish senders.
pub(crate) fn tagged_instance(reply: &Value, token: &str) -> Result<Option<i64>> {
    let mut matches = media_entries(reply)?
        .iter()
        .filter(|entry| load_tag(entry) == Some(token));
    let id = matches
        .next()
        .and_then(|entry| entry["mediaSessionId"].as_i64());
    if matches.next().is_some() {
        return Err(Error::Unknown);
    }
    Ok(id)
}

fn check_load_tag(reply: &Value, id: i64, token: Option<&str>, dispatched: bool) -> Result<()> {
    if let Some(token) = token
        && different_load_tag(reply, id, token)
    {
        return Err(Error::OwnershipLost { dispatched });
    }
    Ok(())
}

pub(crate) fn different_load_tag(reply: &Value, id: i64, token: &str) -> bool {
    media_entry(reply, id)
        .is_ok_and(|entry| load_tag(entry).is_some_and(|observed| observed != token))
}

pub(crate) fn load_tag(entry: &Value) -> Option<&str> {
    entry
        .pointer("/media/customData/castRsLoad")
        .and_then(Value::as_str)
}

pub(crate) fn media_entries(reply: &Value) -> Result<&[Value]> {
    let entries = reply["status"].as_array().ok_or(Error::Unknown)?;
    // A malformed entry could represent the session we were asked to find.
    if entries
        .iter()
        .any(|entry| entry["mediaSessionId"].as_i64().is_none())
    {
        return Err(Error::Unknown);
    }
    Ok(entries)
}
pub(crate) fn media_entry(reply: &Value, id: i64) -> Result<&Value> {
    let mut entries = media_entries(reply)?
        .iter()
        .filter(|entry| entry["mediaSessionId"].as_i64() == Some(id));
    let entry = entries.next().ok_or(Error::Ended)?;
    if entries.next().is_some() {
        return Err(Error::Unknown);
    }
    Ok(entry)
}
pub(crate) fn state(reply: &Value, id: i64) -> Result<Status> {
    let item = media_entry(reply, id)?;
    Ok(match item["playerState"].as_str().ok_or(Error::Unknown)? {
        "PLAYING" => Status::Playing,
        "PAUSED" => Status::Paused,
        "BUFFERING" => Status::Buffering,
        "IDLE" => match item["idleReason"].as_str() {
            Some("FINISHED") => Status::Finished,
            Some("INTERRUPTED") => Status::Interrupted,
            Some("CANCELLED") => Status::Canceled,
            Some("ERROR") => Status::Failed,
            _ => Status::Idle,
        },
        _ => Status::Unknown,
    })
}
fn app_transport(v: &Value) -> Option<String> {
    v.pointer("/status/applications")?
        .as_array()?
        .iter()
        .find(|app| app["appId"] == APP)?["transportId"]
        .as_str()
        .map(str::to_owned)
}

async fn drive(
    mut socket: impl AsyncRead + AsyncWrite + Unpin,
    mut requests: mpsc::Receiver<Request>,
    sender: String,
) {
    let mut pending = HashMap::<u64, Pending>::new();
    let mut id = 0u64;
    let mut buffer = Vec::with_capacity(8192);
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut seen = Instant::now();
    loop {
        tokio::select! {
            _=tick.tick()=>{
                if seen.elapsed()>=DEADLINE{break;}
                pending.retain(|_,p|p.deadline>Instant::now() && !p.reply.is_closed());
                if !write(&mut socket,&sender,HEARTBEAT,ROOT,json!({"type":"PING"})).await{break;}
            }
            req=requests.recv()=>{
                let Some(mut req)=req else{break;};
                if req.reply.is_closed(){continue;}
                id+=1;
                if req.namespace!=CONNECTION{req.body["requestId"]=id.into();}
                if !write(&mut socket,&sender,req.namespace,&req.destination,req.body).await{break;}
                if req.namespace==CONNECTION{let _=req.reply.send(Ok(Value::Null));}
                else{pending.insert(id,Pending{namespace:req.namespace,destination:req.destination,reply:req.reply,deadline:Instant::now()+req.retention});}
            }
            read=socket.read_buf(&mut buffer)=>{
                if !matches!(read,Ok(n) if n>0){break;}
                while buffer.len()>=4 {
                    let n=u32::from_be_bytes([buffer[0],buffer[1],buffer[2],buffer[3]]) as usize;
                    if n>MAX_FRAME{return;}
                    if buffer.len()<4+n{break;}
                    let message=Envelope::decode(&buffer[4..4+n]);buffer.drain(..4+n);
                    let Ok(m)=message else{return;};
                    if m.kind!=0 || (m.destination!=sender && m.destination!="*"){continue;}
                    let Some(v)=m.message.and_then(|s|serde_json::from_str::<Value>(&s).ok()) else{continue;};
                    seen=Instant::now();
                    if m.namespace==HEARTBEAT && v["type"]=="PING" {
                        if !write(&mut socket,&sender,HEARTBEAT,&m.source,json!({"type":"PONG"})).await{return;}
                    } else if m.namespace==CONNECTION && v["type"]=="CLOSE" {return;}

                    if m.destination==sender && let Some(id)=v["requestId"].as_u64()
                        && pending.get(&id).is_some_and(|p|p.namespace==m.namespace && p.destination==m.source)
                        && let Some(p)=pending.remove(&id) {
                        let result=match (m.namespace.as_str(), v["type"].as_str()){
                            (_, Some("LOAD_FAILED"))=>Err(Error::LoadFailed),
                            (_, Some("LOAD_CANCELLED"|"INVALID_REQUEST"|"LAUNCH_ERROR"|"INVALID_PLAYER_STATE"))=>Err(Error::Refused),
                            (MEDIA, Some("MEDIA_STATUS")) | (RECEIVER, Some("RECEIVER_STATUS"))=>Ok(v),
                            _=>Err(Error::Unknown)
                        };
                        let _=p.reply.send(result);
                    }
                }
            }
        }
    }
}
async fn write(
    socket: &mut (impl AsyncWrite + Unpin),
    source: &str,
    ns: &str,
    dest: &str,
    v: Value,
) -> bool {
    matches!(
        timeout(DEADLINE, wire::send(socket, source, ns, dest, v)).await,
        Ok(Ok(()))
    )
}

#[cfg(test)]
mod tests;

pub(crate) mod adapter;
