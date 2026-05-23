use super::*;

#[test]
fn sse_decoder_extracts_json_payloads_and_drops_done_markers() {
    let mut decoder = SseDecoder::default();
    let frame = "event: response.output_text.delta\r\ndata: {\"delta\":\"hi\"}\r\n\r\n: keepalive\ndata: [DONE]\n\n";

    assert_eq!(
        decoder.push_chunk(frame.as_bytes()).unwrap(),
        vec!["{\"delta\":\"hi\"}".to_string()]
    );
    assert!(decoder.finish().unwrap().is_empty());
}

#[test]
fn sse_decoder_handles_split_frames() {
    let mut decoder = SseDecoder::default();

    assert!(decoder.push_chunk(b"data: {\"a\":").unwrap().is_empty());
    assert_eq!(
        decoder.push_chunk(b"1}\n\n").unwrap(),
        vec!["{\"a\":1}".to_string()]
    );
}

#[test]
fn sse_decoder_rejects_oversized_frame() {
    let mut decoder = SseDecoder::default();
    let chunk = vec![b'x'; crate::llm::schema::MAX_LLM_EVENT_BYTES + 1];

    let err = decoder.push_chunk(&chunk).unwrap_err();

    assert!(err.to_string().contains("SSE event frame exceeded"));
}
