use super::*;

#[test]
fn sse_decoder_extracts_json_payloads_and_drops_done_markers() {
    let mut decoder = SseDecoder::default();
    let frame = "event: response.output_text.delta\r\ndata: {\"delta\":\"hi\"}\r\n\r\n: keepalive\ndata: [DONE]\n\n";

    assert_eq!(
        decoder.push_chunk(frame.as_bytes()),
        vec!["{\"delta\":\"hi\"}".to_string()]
    );
    assert!(decoder.finish().is_empty());
}

#[test]
fn sse_decoder_handles_split_frames() {
    let mut decoder = SseDecoder::default();

    assert!(decoder.push_chunk(b"data: {\"a\":").is_empty());
    assert_eq!(decoder.push_chunk(b"1}\n\n"), vec!["{\"a\":1}".to_string()]);
}
