//! Cast v2 control of the Default Media Receiver.
use crate::{
    tls,
    wire::{self, Envelope, MAX_FRAME},
};
use prost::Message;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    net::TcpStream,
    sync::{mpsc, oneshot},
    time::{Instant, timeout},
};
use tokio_rustls::rustls::pki_types::ServerName;

const RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
const MEDIA: &str = "urn:x-cast:com.google.cast.media";
const CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
const HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const ROOT: &str = "receiver-0";
const APP: &str = "CC1AD845";
const DEADLINE: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unreachable,
    Refused,
    LoadFailed,
    Unknown,
    Ended,
}
type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Volume(u8),
    StepVolume(i8),
    ToggleMute,
    Play,
    Pause,
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

    local_ip: IpAddr,
    transport: String,
}
impl Session {
    pub async fn connect(address: SocketAddr) -> Result<Self> {
        Self::open(address, true).await
    }
    /// Attaches only to an already running instance of the media receiver.
    pub async fn rejoin(address: SocketAddr) -> Result<Self> {
        Self::open(address, false).await
    }
    async fn open(address: SocketAddr, launch: bool) -> Result<Self> {
        let (socket, local_ip) = timeout(DEADLINE, async {
            let socket = TcpStream::connect(address)
                .await
                .map_err(|_| Error::Unreachable)?;
            let local = socket.local_addr().map_err(|_| Error::Unreachable)?.ip();
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

        tokio::spawn(drive(socket, rx));
        let mut session = Self {
            outgoing: tx,

            local_ip,
            transport: String::new(),
        };
        session.connect_channel(ROOT).await?;
        let status = session
            .request(RECEIVER, ROOT, json!({"type":"GET_STATUS"}))
            .await?;
        session.transport = if let Some(id) = app_transport(&status) {
            id
        } else if launch {
            app_transport(
                &session
                    .request(RECEIVER, ROOT, json!({"type":"LAUNCH","appId":APP}))
                    .await?,
            )
            .ok_or(Error::Refused)?
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
        self.local_ip
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
                    reply,
                })
                .await
                .map_err(|_| Error::Unknown)?;
            rx.await.map_err(|_| Error::Unknown)?
        })
        .await
        .map_err(|_| Error::Unknown)?
    }
    async fn media(&self, body: Value) -> Result<Value> {
        self.request(MEDIA, &self.transport, body).await
    }
    /// Loads live HLS at the live edge. Only a reply for this URL establishes ownership.
    pub async fn load(&self, url: &str) -> Result<i64> {
        let reply=self.media(json!({"type":"LOAD","autoplay":true,"media":{"contentId":url,"contentType":"application/x-mpegURL","streamType":"LIVE"}})).await?;
        media_entries(&reply)?
            .iter()
            .find(|item| item.pointer("/media/contentId").and_then(Value::as_str) == Some(url))
            .and_then(|item| item["mediaSessionId"].as_i64())
            .ok_or(Error::Unknown)
    }
    /// Returns the receiver's current ownership of one opaque relay capability.
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
        let value = match cmd {
            Command::Volume(level) => Some(json!({"level":f64::from(level.min(100))/100.0})),
            Command::StepVolume(_) | Command::ToggleMute => {
                let status = self
                    .request(RECEIVER, ROOT, json!({"type":"GET_STATUS"}))
                    .await?;
                let volume = &status["status"]["volume"];
                Some(if let Command::StepVolume(step) = cmd {
                    let level = volume["level"].as_f64().ok_or(Error::Unknown)?;
                    json!({"level":(level+f64::from(step)/100.0).clamp(0.0,1.0)})
                } else {
                    json!({"muted":!volume["muted"].as_bool().ok_or(Error::Unknown)?})
                })
            }
            _ => None,
        };
        if let Some(volume) = value {
            match self.status(id).await? {
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
            Command::Stop => "STOP",
            _ => return Err(Error::Refused),
        };
        let reply = self.media(json!({"type":verb,"mediaSessionId":id})).await?;
        timeout(DEADLINE, async {
            let mut observed = if cmd == Command::Stop {
                self.status(id).await
            } else {
                state(&reply, id)
            };
            loop {
                match (cmd, observed) {
                    (Command::Play, Ok(Status::Playing | Status::Buffering))
                    | (Command::Pause, Ok(Status::Paused))
                    | (
                        Command::Stop,
                        Err(Error::Ended)
                        | Ok(
                            Status::Idle
                            | Status::Finished
                            | Status::Interrupted
                            | Status::Canceled
                            | Status::Failed,
                        ),
                    ) => return Ok(()),
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
                observed = self.status(id).await;
            }
        })
        .await
        .map_err(|_| Error::Unknown)?
    }
}
fn media_entries(reply: &Value) -> Result<&[Value]> {
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
fn state(reply: &Value, id: i64) -> Result<Status> {
    let item = media_entries(reply)?
        .iter()
        .find(|entry| entry["mediaSessionId"].as_i64() == Some(id))
        .ok_or(Error::Ended)?;
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
                if !write(&mut socket,HEARTBEAT,ROOT,json!({"type":"PING"})).await{break;}
            }
            req=requests.recv()=>{
                let Some(mut req)=req else{break;};
                if req.reply.is_closed(){continue;}
                id+=1;
                if req.namespace!=CONNECTION{req.body["requestId"]=id.into();}
                if !write(&mut socket,req.namespace,&req.destination,req.body).await{break;}
                if req.namespace==CONNECTION{let _=req.reply.send(Ok(Value::Null));}
                else{pending.insert(id,Pending{namespace:req.namespace,destination:req.destination,reply:req.reply,deadline:Instant::now()+DEADLINE});}
            }
            read=socket.read_buf(&mut buffer)=>{
                if !matches!(read,Ok(n) if n>0){break;}
                while buffer.len()>=4 {
                    let n=u32::from_be_bytes([buffer[0],buffer[1],buffer[2],buffer[3]]) as usize;
                    if n>MAX_FRAME{return;}
                    if buffer.len()<4+n{break;}
                    let message=Envelope::decode(&buffer[4..4+n]);buffer.drain(..4+n);
                    let Ok(m)=message else{return;};
                    if m.kind!=0 || !matches!(m.destination.as_str(),"sender-0"|"*"){continue;}
                    let Some(v)=m.message.and_then(|s|serde_json::from_str::<Value>(&s).ok()) else{continue;};
                    seen=Instant::now();
                    if m.namespace==HEARTBEAT && v["type"]=="PING" {
                        if !write(&mut socket,HEARTBEAT,&m.source,json!({"type":"PONG"})).await{return;}
                    } else if m.namespace==CONNECTION && v["type"]=="CLOSE" {return;}

                    if let Some(id)=v["requestId"].as_u64()
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
async fn write(socket: &mut (impl AsyncWrite + Unpin), ns: &str, dest: &str, v: Value) -> bool {
    matches!(
        timeout(DEADLINE, wire::send(socket, ns, dest, v)).await,
        Ok(Ok(()))
    )
}

#[cfg(test)]
mod tests;
