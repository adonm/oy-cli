use super::*;
use crate::llm::{GenerationOptions, ModelRoute, Protocol, RouteAuth, ToolResultContent};

fn request() -> LlmRequest {
    LlmRequest {
        route: ModelRoute {
            protocol: Protocol::Gemini,
            model: "gemini-2.5-flash".to_string(),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_string()),
            auth: RouteAuth::Header {
                name: "x-goog-api-key".to_string(),
                value: "secret".to_string(),
            },
            query_params: None,
            additional_params: None,
        },
        system_prompt: "You are concise.".to_string(),
        system_cache: None,
        messages: vec![Message::user_text("Say hello.")],
        tools: vec![ToolSpec {
            name: "lookup".to_string(),
            description: "Lookup data".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["status", "missing"],
                "properties": {
                    "status": {"type": "integer", "enum": [1, 2]},
                    "tags": {"type": "array"},
                    "name": {"type": "string", "properties": {"ignored": {"type": "string"}}, "required": ["ignored"]}
                }
            }),
            cache: None,
        }],
        max_turns: 4,
        tool_choice: Some(ToolChoice::Tool {
            name: "lookup".to_string(),
        }),
        generation: Some(GenerationOptions {
            max_tokens: Some(20),
            temperature: Some(0.0),
            top_p: Some(0.9),
            top_k: Some(40),
            stop: Some(vec!["END".to_string()]),
            ..Default::default()
        }),
        cache: None,
    }
}

#[test]
fn request_body_lowers_messages_tools_and_generation() {
    let body = request_body(&request()).unwrap();

    assert_eq!(
        body,
        json!({
            "contents": [{"role": "user", "parts": [{"text": "Say hello."}]}],
            "systemInstruction": {"parts": [{"text": "You are concise."}]},
            "tools": [{"functionDeclarations": [{
                "name": "lookup",
                "description": "Lookup data",
                "parameters": {
                    "type": "object",
                    "required": ["status"],
                    "properties": {
                        "status": {"type": "string", "enum": ["1", "2"]},
                        "tags": {"type": "array", "items": {"type": "string"}},
                        "name": {"type": "string"}
                    }
                }
            }]}],
            "toolConfig": {"functionCallingConfig": {"mode": "ANY", "allowedFunctionNames": ["lookup"]}},
            "generationConfig": {
                "maxOutputTokens": 20,
                "temperature": 0.0,
                "topP": 0.9,
                "topK": 40,
                "stopSequences": ["END"]
            }
        })
    );
}

#[test]
fn request_body_lowers_tool_result_with_function_name() {
    let mut request = request();
    request.messages = vec![
        Message::Assistant {
            id: None,
            content: vec![MessageContent::ToolCall {
                id: "tool_0".to_string(),
                call_id: Some("tool_0".to_string()),
                name: "lookup".to_string(),
                arguments: json!({"query": "weather"}),
                signature: None,
                additional_params: None,
            }],
        },
        Message::User {
            content: vec![MessageContent::ToolResult {
                id: "lookup".to_string(),
                call_id: None,
                content: vec![ToolResultContent::Text {
                    text: "sunny".to_string(),
                }],
                cache: None,
            }],
        },
    ];

    let body = request_body(&request).unwrap();

    assert_eq!(
        body["contents"],
        json!([
            {"role": "model", "parts": [{"functionCall": {"name": "lookup", "args": {"query": "weather"}}}]},
            {"role": "user", "parts": [{"functionResponse": {"name": "lookup", "response": {"name": "lookup", "content": "sunny"}}}]}
        ])
    );
}

#[test]
fn request_body_omits_tools_when_tool_choice_is_none() {
    let mut request = request();
    request.tool_choice = Some(ToolChoice::None);

    let body = request_body(&request).unwrap();

    assert!(body.get("tools").is_none());
    assert!(body.get("toolConfig").is_none());
}

#[test]
fn stream_parser_maps_text_reasoning_tool_usage_and_finish() {
    let mut state = StreamState::default();
    let mut events = Vec::new();
    events.extend(parse_stream_event(&mut state, &json!({
        "candidates": [{"content": {"role": "model", "parts": [{"text": "thinking", "thought": true}]}}]
    })).unwrap());
    events.extend(parse_stream_event(&mut state, &json!({
        "candidates": [{"content": {"role": "model", "parts": [{"text": "Hello"}, {"functionCall": {"name": "lookup", "args": {"query": "weather"}}}]}, "finishReason": "STOP"}],
        "usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 2, "thoughtsTokenCount": 1, "cachedContentTokenCount": 1, "totalTokenCount": 8}
    })).unwrap());
    events.extend(finish_stream(&mut state).unwrap());

    assert!(matches!(events[0], LlmEvent::ReasoningDelta { ref text } if text == "thinking"));
    assert!(matches!(events[1], LlmEvent::TextDelta { ref text } if text == "Hello"));
    let call = events
        .iter()
        .find_map(|event| match event {
            LlmEvent::ToolCall { call, .. } => Some(call),
            _ => None,
        })
        .unwrap();
    assert_eq!(call.call_id, "tool_0");
    assert_eq!(call.arguments_value().unwrap(), json!({"query": "weather"}));
    assert!(events.iter().any(|event| matches!(
        event,
        LlmEvent::StepFinish {
            reason: FinishReason::ToolCalls,
            usage: Some(Usage {
                input_tokens: Some(5),
                output_tokens: Some(3),
                cache_read_input_tokens: Some(1),
                reasoning_tokens: Some(1),
                total_tokens: Some(8),
                ..
            }),
        }
    )));
}

#[test]
fn endpoint_path_matches_opencode_gemini_route() {
    assert_eq!(
        endpoint_path("gemini-2.5-flash"),
        "models/gemini-2.5-flash:streamGenerateContent"
    );
}
