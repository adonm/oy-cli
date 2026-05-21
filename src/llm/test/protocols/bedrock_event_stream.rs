use super::*;
use serde_json::json;

#[test]
fn decoder_rewraps_split_bedrock_event_stream_frames() {
    let frame = encode_test_event(
        "contentBlockDelta",
        &json!({"delta": {"text": "hi"}, "p": "pad"}),
    );
    let split = frame.len() / 2;
    let mut decoder = Decoder::default();

    assert!(decoder.push_chunk(&frame[..split]).unwrap().is_empty());
    assert_eq!(
        decoder.push_chunk(&frame[split..]).unwrap(),
        vec![json!({"contentBlockDelta": {"delta": {"text": "hi"}}})]
    );
}

#[test]
fn decoder_rejects_invalid_crc() {
    let mut frame = encode_test_event("metadata", &json!({"usage": {"inputTokens": 1}}));
    let len = frame.len();
    frame[len - 1] ^= 0xff;

    let err = Decoder::default().push_chunk(&frame).unwrap_err();

    assert!(err.to_string().contains("invalid message CRC"));
}
