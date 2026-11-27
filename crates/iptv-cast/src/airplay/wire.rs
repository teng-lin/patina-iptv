//! Bounded HTTP and RTSP messages over a dedicated AirPlay connection.
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use std::{fmt::Write as _, io, net::SocketAddr};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const FRAME: usize = 1024;
const TAG: usize = 16;
const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 256 * 1024;
const HEADER_END: &[u8] = b"\r\n\r\n";

pub(super) type Result<T> = std::result::Result<T, Fault>;

// Keep peer text, request bodies, and transport error payloads out of diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Fault {
    Closed,
    Unreachable,
    InvalidMessage,
    Integrity,
    Refused(u16),
}

impl From<io::Error> for Fault {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::UnexpectedEof
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted => Self::Closed,
            _ => Self::Unreachable,
        }
    }
}

pub(super) fn encrypt(
    key: &LessSafeKey,
    nonce: [u8; 12],
    aad: &[u8],
    mut data: Vec<u8>,
) -> Result<Vec<u8>> {
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(aad),
        &mut data,
    )
    .map_err(|_| Fault::Integrity)?;
    Ok(data)
}

pub(super) fn decrypt<'a>(
    key: &LessSafeKey,
    nonce: [u8; 12],
    aad: &[u8],
    data: &'a mut [u8],
) -> Result<&'a [u8]> {
    key.open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad), data)
        .map(|data| &*data)
        .map_err(|_| Fault::Integrity)
}

pub(super) fn key(bytes: &[u8; 32]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&aead::CHACHA20_POLY1305, bytes).expect("fixed key size"))
}

struct Direction {
    key: LessSafeKey,
    counter: u64,
}

impl Direction {
    fn new(bytes: &[u8; 32]) -> Self {
        Self {
            key: key(bytes),
            counter: 0,
        }
    }

    fn next_nonce(&mut self) -> Result<[u8; 12]> {
        let mut nonce = [0; 12];
        nonce[4..].copy_from_slice(&self.counter.to_le_bytes());
        self.counter = self.counter.checked_add(1).ok_or(Fault::Integrity)?;
        Ok(nonce)
    }

    fn seal(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(data.len());
        for chunk in data.chunks(FRAME) {
            let length = (chunk.len() as u16).to_le_bytes();
            let nonce = self.next_nonce()?;
            out.extend(length);
            out.extend(encrypt(&self.key, nonce, &length, chunk.to_vec())?);
        }
        Ok(out)
    }

    fn open(&mut self, wire: &mut Vec<u8>, out: &mut Vec<u8>) -> Result<()> {
        let mut consumed = 0;
        while let Some(length) = wire[consumed..].first_chunk::<2>() {
            let size = usize::from(u16::from_le_bytes(*length));
            if size == 0 || size > FRAME {
                return Err(Fault::InvalidMessage);
            }
            let end = consumed + 2 + size + TAG;
            if wire.len() < end {
                break;
            }
            let length = *length;
            let nonce = self.next_nonce()?;
            out.extend_from_slice(decrypt(
                &self.key,
                nonce,
                &length,
                &mut wire[consumed + 2..end],
            )?);
            consumed = end;
        }
        wire.drain(..consumed);
        Ok(())
    }
}

struct Cipher {
    read: Direction,
    write: Direction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Version {
    Http,
    Http10,
    Rtsp,
}

impl Version {
    fn parse(token: &str) -> Result<Self> {
        match token {
            "HTTP/1.1" => Ok(Self::Http),
            "HTTP/1.0" => Ok(Self::Http10),
            "RTSP/1.0" => Ok(Self::Rtsp),
            _ => Err(Fault::InvalidMessage),
        }
    }

    fn token(self) -> &'static str {
        match self {
            Self::Http => "HTTP/1.1",
            Self::Http10 => "HTTP/1.0",
            Self::Rtsp => "RTSP/1.0",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Start {
    Request,
    Response(u16),
}

pub(super) struct Message {
    pub(super) start: Start,
    pub(super) version: Version,
    pub(super) sequence: Option<u32>,
    pub(super) body: Vec<u8>,
    #[cfg(test)]
    pub(super) request_target: String,
}

impl Message {
    fn parse(data: &mut Vec<u8>) -> Result<Option<Self>> {
        let Some(header_end) = data
            .windows(HEADER_END.len())
            .position(|part| part == HEADER_END)
        else {
            if data.len() > MAX_HEADER {
                let excess = data.len() - MAX_HEADER;
                let split_delimiter =
                    (excess..HEADER_END.len()).any(|length| data.ends_with(&HEADER_END[..length]));
                if !split_delimiter {
                    return Err(Fault::InvalidMessage);
                }
            }
            return Ok(None);
        };
        if header_end > MAX_HEADER {
            return Err(Fault::InvalidMessage);
        }
        let head = std::str::from_utf8(&data[..header_end]).map_err(|_| Fault::InvalidMessage)?;
        let mut lines = head.split("\r\n");
        let start_line = lines.next().ok_or(Fault::InvalidMessage)?;
        let mut fields = start_line.splitn(3, ' ');
        let first = fields.next().ok_or(Fault::InvalidMessage)?;
        let second = fields.next().ok_or(Fault::InvalidMessage)?;
        let (start, version) = if first.contains('/') {
            let status = second.parse::<u16>().map_err(|_| Fault::InvalidMessage)?;
            if !(100..=599).contains(&status) {
                return Err(Fault::InvalidMessage);
            }
            (Start::Response(status), Version::parse(first)?)
        } else {
            if first.is_empty() || second.is_empty() {
                return Err(Fault::InvalidMessage);
            }
            (
                Start::Request,
                Version::parse(fields.next().ok_or(Fault::InvalidMessage)?)?,
            )
        };
        #[cfg(test)]
        let request_target = if start == Start::Request {
            format!("{first} {second}")
        } else {
            String::new()
        };

        let mut length: Option<usize> = None;
        let mut sequence = None;
        for line in lines {
            let (key, value) = line.split_once(':').ok_or(Fault::InvalidMessage)?;
            let value = value.trim();
            if key.eq_ignore_ascii_case("content-length") {
                let size = value.parse().map_err(|_| Fault::InvalidMessage)?;
                if size > MAX_BODY || length.replace(size).is_some() {
                    return Err(Fault::InvalidMessage);
                }
            } else if key.eq_ignore_ascii_case("cseq") {
                let id = value.parse().map_err(|_| Fault::InvalidMessage)?;
                if sequence.replace(id).is_some() {
                    return Err(Fault::InvalidMessage);
                }
            } else if key.eq_ignore_ascii_case("transfer-encoding") {
                return Err(Fault::InvalidMessage);
            }
        }
        let body_start = header_end + HEADER_END.len();
        let end = body_start + length.unwrap_or(0);
        if data.len() < end {
            return Ok(None);
        }

        let body = data[body_start..end].to_vec();
        data.drain(..end);
        Ok(Some(Self {
            start,
            version,
            sequence,
            body,
            #[cfg(test)]
            request_target,
        }))
    }

    pub(super) fn require_ok(self) -> Result<Self> {
        match self.start {
            Start::Response(200) => Ok(self),
            Start::Response(status) => Err(Fault::Refused(status)),
            Start::Request => Err(Fault::InvalidMessage),
        }
    }
}

pub(super) struct Wire {
    stream: TcpStream,
    cipher: Option<Cipher>,
    encrypted: Vec<u8>,
    decoded: Vec<u8>,
    frame_fault: Option<Fault>,
}

impl Wire {
    pub(super) async fn connect(address: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect(address).await?;
        Ok(Self::new(stream))
    }

    pub(super) fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            cipher: None,
            encrypted: Vec::new(),
            decoded: Vec::new(),
            frame_fault: None,
        }
    }

    pub(super) fn addresses(&self) -> Result<(SocketAddr, SocketAddr)> {
        Ok((self.stream.local_addr()?, self.stream.peer_addr()?))
    }

    pub(super) fn enable_encryption(&mut self, write: &[u8; 32], read: &[u8; 32]) {
        self.cipher = Some(Cipher {
            read: Direction::new(read),
            write: Direction::new(write),
        });
        // Encryption may start immediately after the last plaintext message in the same read.
        self.encrypted = std::mem::take(&mut self.decoded);
    }

    async fn send(&mut self, head: String, body: &[u8]) -> Result<()> {
        let mut data = head.into_bytes();
        data.extend_from_slice(body);
        let data = match &mut self.cipher {
            Some(cipher) => cipher.write.seal(&data)?,
            None => data,
        };
        self.stream.write_all(&data).await?;
        Ok(())
    }

    pub(super) async fn receive(&mut self) -> Result<Message> {
        let mut buffer = [0; 4096];
        loop {
            if self.frame_fault.is_none()
                && let Some(cipher) = &mut self.cipher
            {
                self.frame_fault = cipher
                    .read
                    .open(&mut self.encrypted, &mut self.decoded)
                    .err();
            }
            // A read can contain both authenticated messages and a later bad frame.
            if let Some(message) = Message::parse(&mut self.decoded)? {
                return Ok(message);
            }
            if let Some(fault) = self.frame_fault {
                return Err(fault);
            }

            let count = self.stream.read(&mut buffer).await?;
            if count == 0 {
                return Err(Fault::Closed);
            }
            let pending = if self.cipher.is_some() {
                &mut self.encrypted
            } else {
                &mut self.decoded
            };
            pending.extend_from_slice(&buffer[..count]);
        }
    }

    pub(super) async fn request(
        &mut self,
        version: Version,
        verb: &str,
        target: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<Message> {
        self.send_request(version, verb, target, headers, body)
            .await?;
        self.receive_response(version).await
    }

    pub(super) async fn send_request(
        &mut self,
        version: Version,
        verb: &str,
        target: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<()> {
        if let Some(fault) = self.frame_fault {
            return Err(fault);
        }
        let mut head = format!("{verb} {target} {}\r\n", version.token());
        // Request fields are supplied by the sender, and values from the receiver
        // are parsed into typed IDs before they are rendered here.
        for (name, value) in headers {
            write!(head, "{name}: {value}\r\n").expect("infallible String write");
        }
        write!(head, "Content-Length: {}\r\n\r\n", body.len()).expect("infallible String write");
        self.send(head, body).await
    }

    pub(super) async fn receive_response(&mut self, version: Version) -> Result<Message> {
        loop {
            let response = self.receive().await?;
            let response_version_matches = response.version == version
                || (version == Version::Http && response.version == Version::Http10);
            let Start::Response(status) = response.start else {
                return Err(Fault::InvalidMessage);
            };
            if !response_version_matches {
                return Err(Fault::InvalidMessage);
            }
            if version == Version::Rtsp || status >= 200 {
                return Ok(response);
            }
            if status == 101 || !response.body.is_empty() {
                return Err(Fault::InvalidMessage);
            }
        }
    }

    pub(super) async fn ack_event(&mut self, request: &Message) -> Result<()> {
        if request.start != Start::Request {
            return Err(Fault::InvalidMessage);
        }
        let mut head = format!(
            "{} 200 OK\r\nContent-Length: 0\r\nAudio-Latency: 0\r\n",
            request.version.token()
        );
        if let Some(sequence) = request.sequence {
            write!(head, "CSeq: {sequence}\r\n").expect("infallible String write");
        }
        head.push_str("\r\n");
        self.send(head, &[]).await
    }

    #[cfg(test)]
    pub(super) async fn respond(&mut self, request: &Message, code: u16, body: &[u8]) {
        let mut head = format!(
            "{} {code} Synthetic\r\nContent-Length: {}\r\n",
            request.version.token(),
            body.len()
        );
        if let Some(sequence) = request.sequence {
            write!(head, "CSeq: {sequence}\r\n").unwrap();
        }
        head.push_str("\r\n");
        self.send(head, body).await.unwrap();
    }
}

#[cfg(test)]
mod tests;
