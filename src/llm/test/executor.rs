mod chat_tests;
mod responses_tests;

use super::*;

use crate::llm::ToolSpec;

pub(super) fn read_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "read".to_string(),
        description: "Read a file".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }),
        cache: None,
    }
}

#[test]
fn shared_tool_round_budget_helper_preserves_protocol_messages() {
    assert!(ensure_tool_round_budget(0, 1, "chat").is_ok());

    let chat = ensure_tool_round_budget(1, 1, "chat").unwrap_err();
    assert_eq!(
        chat.to_string(),
        "native chat exceeded the tool round budget"
    );

    let responses = ensure_tool_round_budget(1, 1, "Responses").unwrap_err();
    assert_eq!(
        responses.to_string(),
        "native Responses exceeded the tool round budget"
    );
}

#[test]
fn chat_and_responses_tool_error_wire_payloads_match_transcript() {
    let call = NativeToolCall {
        id: "fc_1".to_string(),
        call_id: "call-1".to_string(),
        name: "read".to_string(),
        arguments: "{}".to_string(),
        signature: None,
    };
    let output = "TOOL_ERROR: blocked\nRECOVERY: choose another tool";

    let chat = openai_chat::tool_result_wire_message(&call, output);
    let responses = openai_responses::tool_result_input(&call, output);
    let transcript = tool_result_message(&call, output.to_string());

    assert_eq!(chat["content"], json!(output));
    assert_eq!(responses["output"], json!(output));
    let Message::User { content } = transcript else {
        panic!("expected tool result transcript message");
    };
    assert!(matches!(
        &content[0],
        MessageContent::ToolResult { content, .. }
            if content == &vec![ToolResultContent::Text { text: output.to_string() }]
    ));
}

#[test]
fn invalid_tool_arguments_still_round_trip_in_transcript() {
    let message = assistant_message_from_calls(
        "",
        None,
        &[NativeToolCall {
            id: "call-1".to_string(),
            call_id: "call-1".to_string(),
            name: "read".to_string(),
            arguments: "{not-json".to_string(),
            signature: None,
        }],
    )
    .unwrap();

    let Message::Assistant { content, .. } = message else {
        panic!("expected assistant message");
    };
    assert!(matches!(
        &content[0],
        MessageContent::ToolCall { arguments, .. }
            if arguments["invalid_json_arguments"] == json!("{not-json")
    ));
}
