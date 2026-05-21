use super::*;
use crate::llm::{CacheHint, GenerationOptions, ModelRoute, Protocol, RouteAuth};

fn request() -> LlmRequest {
    LlmRequest {
        route: ModelRoute {
            protocol: Protocol::AnthropicMessages,
            model: "claude-sonnet-4-5".to_string(),
            base_url: Some("https://api.anthropic.com/v1".to_string()),
            auth: RouteAuth::Header {
                name: "x-api-key".to_string(),
                value: "secret".to_string(),
            },
            query_params: None,
            additional_params: None,
        },
        system_prompt: "You are terse".to_string(),
        system_cache: Some(CacheHint::Ephemeral {
            ttl_seconds: Some(3600),
        }),
        messages: vec![Message::user_text("hello")],
        tools: vec![ToolSpec {
            name: "read".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
            cache: None,
        }],
        max_turns: 4,
        tool_choice: Some(ToolChoice::Required),
        generation: Some(GenerationOptions {
            max_tokens: Some(123),
            temperature: Some(0.2),
            ..Default::default()
        }),
        cache: None,
    }
}

#[test]
fn request_body_lowers_messages_tools_cache_and_generation() {
    let body = request_body(&request()).unwrap();

    assert_eq!(body["model"], "claude-sonnet-4-5");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 123);
    assert_eq!(body["temperature"], 0.2);
    assert_eq!(body["tool_choice"], json!({"type": "any"}));
    assert_eq!(
        body["system"][0]["cache_control"],
        json!({"type": "ephemeral", "ttl": "1h"})
    );
    assert_eq!(body["tools"][0]["name"], "read");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(
        body["messages"][0]["content"][0],
        json!({"type":"text","text":"hello"})
    );
}

#[test]
fn stream_parser_maps_text_tool_usage_and_finish() {
    let mut state = StreamState::default();
    let mut events = Vec::new();
    events.extend(parse_stream_event(&mut state, &json!({"type":"message_start","message":{"usage":{"input_tokens":2,"cache_read_input_tokens":3}}})).unwrap());
    events.extend(
        parse_stream_event(
            &mut state,
            &json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}),
        )
        .unwrap(),
    );
    events.extend(parse_stream_event(&mut state, &json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read"}})).unwrap());
    events.extend(parse_stream_event(&mut state, &json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}})).unwrap());
    events.extend(parse_stream_event(&mut state, &json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"README.md\"}"}})).unwrap());
    events.extend(
        parse_stream_event(&mut state, &json!({"type":"content_block_stop","index":1})).unwrap(),
    );
    events.extend(parse_stream_event(&mut state, &json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}})).unwrap());
    events.extend(finish_stream(&mut state).unwrap());

    assert!(matches!(events[0], LlmEvent::TextDelta { ref text } if text == "hi"));
    let call = events
        .iter()
        .find_map(|event| match event {
            LlmEvent::ToolCall { call, .. } => Some(call),
            _ => None,
        })
        .unwrap();
    assert_eq!(call.call_id, "toolu_1");
    assert_eq!(
        call.arguments_value().unwrap(),
        json!({"path": "README.md"})
    );
    assert!(events.iter().any(|event| matches!(
        event,
        LlmEvent::StepFinish {
            reason: FinishReason::ToolCalls,
            ..
        }
    )));
}
