use super::*;
use tokio::net::TcpListener;

#[test]
fn parses_fragmented_and_coalesced_messages() {
    let mut data = b"RTSP/1.0 200 OK\r\nCSeq: 8\r\nContent-Length: 2\r\n\r\nA".to_vec();
    assert!(Message::parse(&mut data).unwrap().is_none());
    data.extend(b"BPOST /event HTTP/1.1\r\nCSeq: 42\r\n\r\n");
    let response = Message::parse(&mut data)
        .unwrap()
        .unwrap()
        .require_ok()
        .unwrap();
    assert_eq!(response.sequence, Some(8));
    assert_eq!(response.body, b"AB");
    let request = Message::parse(&mut data).unwrap().unwrap();
    assert_eq!(request.version, Version::Http);
    assert_eq!(request.sequence, Some(42));
    assert_eq!(request.start, Start::Request);
    assert!(data.is_empty());
    assert!(matches!(request.require_ok(), Err(Fault::InvalidMessage)));

    let mut data = b"HTTP/1.0 403 Forbidden\r\n\r\n".to_vec();
    let refused = Message::parse(&mut data).unwrap().unwrap();
    assert_eq!(refused.version, Version::Http10);
    assert!(matches!(refused.require_ok(), Err(Fault::Refused(403))));
}

#[test]
fn rejects_ambiguous_or_oversized_messages() {
    assert!(
        Message::parse(&mut b"HTTP/1.1 200 OK\r\nCSeq".to_vec())
            .unwrap()
            .is_none()
    );
    for malformed in [
        "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 18446744073709551615\r\n\r\n",
        "HTTP/1.1 200 OK\r\nCSeq: 1\r\ncseq: 2\r\n\r\n",
        "HTTP/1.1 200 OK\r\nCSeq: no\r\n\r\n",
        "HTTP/1.1 200 OK\r\nCSeq 1\r\n\r\n",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
        "HTTP/1.1 20 INVALID\r\n\r\n",
        "HTTP/1.1 no INVALID\r\n\r\n",
        "HTTP/2 200 OK\r\n\r\n",
        "POST  HTTP/1.1\r\n\r\n",
        "POST /event\r\n\r\n",
        "GARBAGE\r\n\r\n",
    ] {
        assert!(matches!(
            Message::parse(&mut malformed.as_bytes().to_vec()),
            Err(Fault::InvalidMessage)
        ));
    }
    let mut excess = vec![b'z'; MAX_HEADER + 1];
    assert!(matches!(
        Message::parse(&mut excess),
        Err(Fault::InvalidMessage)
    ));
    excess.extend(b"\r\n\r\n");
    assert!(matches!(
        Message::parse(&mut excess),
        Err(Fault::InvalidMessage)
    ));
    assert!(matches!(
        Message::parse(&mut b"\xff\r\n\r\n".to_vec()),
        Err(Fault::InvalidMessage)
    ));
}

#[test]
fn encrypted_fragments_authenticate_and_keep_counter_across_frames() {
    let bytes = vec![99; 3 * FRAME - 10];
    let key = [12; 32];
    let encrypted = Direction::new(&key).seal(&bytes).unwrap();
    let chunks = [1, 7, FRAME - 9, FRAME + 4];
    let mut reader = Direction::new(&key);
    let mut output = Vec::new();
    let mut pending = Vec::new();
    let mut start = 0;
    for size in chunks {
        pending.extend(&encrypted[start..start + size]);
        reader.open(&mut pending, &mut output).unwrap();
        start += size;
    }
    pending.extend(&encrypted[start..]);
    reader.open(&mut pending, &mut output).unwrap();
    assert!(pending.is_empty());
    assert_eq!(output, bytes);

    let mut altered = encrypted;
    altered[4] ^= 1;
    assert_eq!(
        Direction::new(&key)
            .open(&mut altered, &mut Vec::new())
            .unwrap_err(),
        Fault::Integrity
    );
    for invalid in [0u16, 1025u16] {
        assert_eq!(
            Direction::new(&key)
                .open(&mut invalid.to_le_bytes().to_vec(), &mut Vec::new())
                .unwrap_err(),
            Fault::InvalidMessage
        );
    }
    let mut exhausted = Direction::new(&key);
    exhausted.counter = u64::MAX;
    assert_eq!(exhausted.next_nonce().unwrap_err(), Fault::Integrity);
}

#[tokio::test]
async fn wire_rejects_unexpected_response_protocol() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut server = Wire::new(stream);
        let mut request = server.receive().await.unwrap();
        assert!(server.ack_event(&request).await.is_ok());
        request.version = Version::Rtsp;
        assert!(matches!(
            server
                .ack_event(&Message {
                    start: Start::Response(200),
                    version: Version::Http,
                    sequence: None,
                    body: vec![],
                    request_target: String::new(),
                })
                .await,
            Err(Fault::InvalidMessage)
        ));
        let second = server.receive().await.unwrap();
        assert_eq!(second.request_target, "POST /again");
        server.respond(&request, 200, &[]).await;
    });
    let mut client = Wire::connect(address).await.unwrap();
    client
        .request(Version::Http, "POST", "/ok", &[], &[])
        .await
        .unwrap();
    let reply = client
        .request(Version::Http, "POST", "/again", &[], &[])
        .await;
    assert!(matches!(reply, Err(Fault::InvalidMessage)));
    server.await.unwrap();
}

#[test]
fn sanitizes_platform_io_errors() {
    for kind in [
        io::ErrorKind::UnexpectedEof,
        io::ErrorKind::BrokenPipe,
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::ConnectionAborted,
    ] {
        assert_eq!(
            Fault::from(io::Error::new(kind, "synthetic private payload")),
            Fault::Closed
        );
    }
    assert_eq!(
        Fault::from(io::Error::other("synthetic private payload")),
        Fault::Unreachable
    );
}
