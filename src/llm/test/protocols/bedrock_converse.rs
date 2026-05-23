use super::*;
use crate::llm::{
    CacheHint, CachePolicy, CachePolicyObject, GenerationOptions, ModelRoute, Protocol, RouteAuth,
    ToolChoice,
};

#[test]
fn request_body_lowers_cache_points_in_tools_system_messages_order() {
    let mut request = test_request();
    request = crate::llm::cache_policy::apply(request);

    let body = request_body(&request).unwrap();

    assert_eq!(
        body["toolConfig"]["tools"][1],
        json!({"cachePoint": {"type": "default"}})
    );
    assert_eq!(
        body["system"][1],
        json!({"cachePoint": {"type": "default"}})
    );
    assert_eq!(
        body["messages"][2]["content"][1],
        json!({"cachePoint": {"type": "default", "ttl": "1h"}})
    );
}

#[test]
fn request_body_caps_cache_points_at_four() {
    let mut request = test_request();
    request.cache = Some(CachePolicy::Object(CachePolicyObject {
        tools: true,
        system: true,
        messages: Some(crate::llm::MessageCachePolicy::Tail { count: 3 }),
        ..CachePolicyObject::default()
    }));
    request = crate::llm::cache_policy::apply(request);

    let body = request_body(&request).unwrap();
    let rendered = body.to_string();

    assert_eq!(rendered.matches("cachePoint").count(), 4);
}

#[test]
fn request_body_lowers_opencode_tool_choice_and_generation_options() {
    let mut request = test_request();
    request.tool_choice = Some(ToolChoice::Tool {
        name: "read".to_string(),
    });
    request.generation = Some(GenerationOptions {
        max_tokens: Some(1024),
        temperature: Some(0.2),
        top_p: Some(0.9),
        stop: Some(vec!["END".to_string()]),
        ..GenerationOptions::default()
    });

    let body = request_body(&request).unwrap();

    assert_eq!(
        body["toolConfig"]["toolChoice"],
        json!({"tool": {"name": "read"}})
    );
    assert_eq!(
        body["inferenceConfig"],
        json!({
            "maxTokens": 1024,
            "temperature": 0.2,
            "topP": 0.9,
            "stopSequences": ["END"]
        })
    );
}

#[test]
fn request_body_omits_bedrock_tools_when_tool_choice_is_none() {
    let mut request = test_request();
    request.tool_choice = Some(ToolChoice::None);

    let body = request_body(&request).unwrap();

    assert!(body.get("toolConfig").is_none());
}

#[test]
fn stream_parser_defers_finish_until_usage_and_maps_bedrock_tool_calls() {
    let mut state = StreamState::default();
    let mut events = Vec::new();
    events.extend(parse_stream_event(&mut state, &json!({
            "contentBlockStart": {"contentBlockIndex": 0, "start": {"toolUse": {"toolUseId": "tool-1", "name": "read"}}}
        })).unwrap());
    events.extend(parse_stream_event(&mut state, &json!({
            "contentBlockDelta": {"contentBlockIndex": 0, "delta": {"toolUse": {"input": "{\"path\":"}}}
        })).unwrap());
    events.extend(parse_stream_event(&mut state, &json!({
            "contentBlockDelta": {"contentBlockIndex": 0, "delta": {"toolUse": {"input": "\"README.md\"}"}}}
        })).unwrap());
    events.extend(
        parse_stream_event(
            &mut state,
            &json!({"contentBlockStop": {"contentBlockIndex": 0}}),
        )
        .unwrap(),
    );
    events.extend(
        parse_stream_event(
            &mut state,
            &json!({"messageStop": {"stopReason": "end_turn"}}),
        )
        .unwrap(),
    );
    events.extend(parse_stream_event(&mut state, &json!({"metadata": {"usage": {"inputTokens": 10, "outputTokens": 3, "cacheReadInputTokens": 4, "cacheWriteInputTokens": 1}}})).unwrap());
    events.extend(finish_stream(&mut state).unwrap());

    let call = events
        .iter()
        .find_map(|event| match event {
            LlmEvent::ToolCall { call, .. } => Some(call),
            _ => None,
        })
        .unwrap();
    assert_eq!(call.call_id, "tool-1");
    assert_eq!(
        call.arguments_value().unwrap(),
        json!({"path": "README.md"})
    );
    let finish = events
        .iter()
        .find_map(|event| match event {
            LlmEvent::StepFinish { reason, usage } => Some((reason, usage.as_ref().unwrap())),
            _ => None,
        })
        .unwrap();
    assert_eq!(finish.0, &FinishReason::ToolCalls);
    assert_eq!(finish.1.non_cached_input_tokens, Some(5));
    assert_eq!(finish.1.cache_read_input_tokens, Some(4));
    assert_eq!(finish.1.cache_write_input_tokens, Some(1));
}

#[test]
fn endpoint_path_encodes_model_id() {
    assert_eq!(
        endpoint_path("anthropic.claude/sonnet 4"),
        "model/anthropic.claude%2Fsonnet%204/converse-stream"
    );
}

fn test_request() -> LlmRequest {
    LlmRequest {
        route: ModelRoute {
            protocol: Protocol::BedrockConverse,
            model: "anthropic.claude-sonnet-4".to_string(),
            base_url: Some("https://bedrock-runtime.us-east-1.amazonaws.com".to_string()),
            auth: RouteAuth::ApiKey("test".to_string()),
            query_params: None,
            additional_params: None,
            default_output_tokens: None,
        },
        system_prompt: "system".to_string(),
        system_cache: None,
        messages: vec![
            Message::user_text("hello"),
            Message::assistant_text("hi"),
            Message::User {
                content: vec![MessageContent::ToolResult {
                    id: "tool-1".to_string(),
                    call_id: Some("tool-1".to_string()),
                    content: vec![ToolResultContent::Text {
                        text: "ok".to_string(),
                    }],
                    cache: Some(CacheHint::Ephemeral {
                        ttl_seconds: Some(3600),
                    }),
                }],
            },
        ],
        tools: vec![ToolSpec {
            name: "read".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            cache: None,
        }],
        max_turns: 1,
        tool_choice: None,
        generation: None,
        cache: None,
    }
}
