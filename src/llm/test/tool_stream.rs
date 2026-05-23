use super::*;
use crate::llm::schema::MAX_LLM_TOOL_ARGUMENT_BYTES;
use serde_json::json;

#[test]
fn append_or_start_requires_identity_on_first_delta() {
    let mut tools = State::<usize>::new();

    let err = append_or_start(
        &mut tools,
        0,
        None,
        None,
        Some("{}"),
        "test-route",
        "missing identity",
    )
    .unwrap_err();

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
        "test-route",
        "missing",
    )
    .unwrap();
    append_or_start(
        &mut tools,
        0,
        Some("call-a"),
        Some("read"),
        Some("{\"path\":"),
        "test-route",
        "missing",
    )
    .unwrap();
    append_or_start(
        &mut tools,
        1,
        None,
        None,
        Some("\"B\"}"),
        "test-route",
        "missing",
    )
    .unwrap();
    append_or_start(
        &mut tools,
        0,
        None,
        None,
        Some("\"A\"}"),
        "test-route",
        "missing",
    )
    .unwrap();

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

#[test]
fn append_existing_rejects_oversized_tool_arguments() {
    let mut tools = State::new();
    start(
        &mut tools,
        0,
        PendingTool::new(
            "test-route",
            "call-a".to_string(),
            "read".to_string(),
            String::new(),
            false,
        )
        .unwrap(),
    );
    let text = "x".repeat(MAX_LLM_TOOL_ARGUMENT_BYTES + 1);

    let err = append_existing(&mut tools, &0, &text, "test-route", "missing").unwrap_err();

    assert!(
        err.to_string()
            .contains("tool arguments for test-route tool call read exceeded")
    );
}

#[test]
fn append_or_start_rejects_oversized_tool_arguments() {
    let mut tools = State::<usize>::new();
    let text = "x".repeat(MAX_LLM_TOOL_ARGUMENT_BYTES + 1);

    let err = append_or_start(
        &mut tools,
        0,
        Some("call-a"),
        Some("read"),
        Some(&text),
        "test-route",
        "missing",
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("tool arguments for test-route tool call read exceeded")
    );
}
