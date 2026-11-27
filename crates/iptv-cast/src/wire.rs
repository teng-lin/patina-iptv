//! Length-prefixed Cast v2 protobuf envelopes.
use prost::Message;
use std::io;
use tokio::io::{AsyncWrite, AsyncWriteExt};
#[derive(Clone, PartialEq, Message)]
pub(crate) struct Envelope {
    #[prost(int32, required, tag = "1")]
    pub version: i32,
    #[prost(string, required, tag = "2")]
    pub source: String,
    #[prost(string, required, tag = "3")]
    pub destination: String,
    #[prost(string, required, tag = "4")]
    pub namespace: String,
    #[prost(int32, required, tag = "5")]
    pub kind: i32,
    #[prost(string, optional, tag = "6")]
    pub message: Option<String>,
}
pub(crate) const MAX_FRAME: usize = 64 * 1024;
pub(crate) async fn send(
    socket: &mut (impl AsyncWrite + Unpin),
    source: &str,
    namespace: &str,
    destination: &str,
    message: serde_json::Value,
) -> io::Result<()> {
    let bytes = Envelope {
        version: 0,
        source: source.into(),
        destination: destination.into(),
        namespace: namespace.into(),
        kind: 0,
        message: Some(message.to_string()),
    }
    .encode_to_vec();
    if bytes.len() > MAX_FRAME {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    socket.write_u32(bytes.len() as u32).await?;
    socket.write_all(&bytes).await
}

#[cfg(test)]
mod tests;
