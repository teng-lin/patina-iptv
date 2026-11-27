use super::*;

#[tokio::test]
async fn an_oversized_frame_writes_nothing_to_the_socket() {
    let mut output = Vec::new();
    let error = send(
        &mut output,
        "sender-0",
        "synthetic.namespace",
        "synthetic-destination",
        serde_json::Value::String("x".repeat(MAX_FRAME)),
    )
    .await
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(output.is_empty());
}
