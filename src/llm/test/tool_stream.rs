use super::*;
use serde_json::json;

#[test]
fn append_or_start_requires_identity_on_first_delta() {
    let mut tools = State::<usize>::new();

    let err =
        append_or_start(&mut tools, 0, None, None, Some("{}"), "missing identity").unwrap_err();

    assert_eq!(err.to_string(), "missing identity");
}

#[test]
fn finish_all_parses_inputs_in_key_order() {
    let mut tools = State::new();
    append_or_start(
        &mut tools,
        1,
        Some("call-b"),
        Some("read"),
        Some("{\"path\":"),
        "missing",
    )
    .unwrap();
    append_or_start(
        &mut tools,
        0,
        Some("call-a"),
        Some("read"),
        Some("{\"path\":"),
        "missing",
    )
    .unwrap();
    append_or_start(&mut tools, 1, None, None, Some("\"B\"}"), "missing").unwrap();
    append_or_start(&mut tools, 0, None, None, Some("\"A\"}"), "missing").unwrap();

    let events = finish_all("test-route", &mut tools).unwrap();
    let calls = events
        .into_iter()
        .filter_map(|event| match event {
            LlmEvent::ToolCall { call, .. } => Some(call),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(calls[0].call_id, "call-a");
    assert_eq!(calls[0].arguments_value().unwrap(), json!({"path": "A"}));
    assert_eq!(calls[1].call_id, "call-b");
    assert_eq!(tools.len(), 0);
}
