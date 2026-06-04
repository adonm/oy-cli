use super::*;

use crate::llm::{GenerationOptions, ToolChoice};

#[test]
fn responses_request_serializes_function_call_output_golden() {
    let input = openai_responses::input_from_llm_with_store(
        "system",
        vec![
            Message::user_text("inspect"),
            Message::Assistant {
                id: None,
                content: vec![MessageContent::ToolCall {
                    id: "fc_1".to_string(),
                    call_id: Some("call-1".to_string()),
                    name: "read".to_string(),
                    arguments: json!({"path": "README.md"}),
                    signature: None,
                    additional_params: None,
                }],
            },
            Message::User {
                content: vec![MessageContent::ToolResult {
                    id: "result-1".to_string(),
                    call_id: Some("call-1".to_string()),
                    content: vec![ToolResultContent::Text {
                        text: "ok".to_string(),
                    }],
                    cache: None,
                }],
            },
        ],
        Some(false),
    )
    .unwrap();
    let body = openai_responses::request_body(
        "gpt-5.1",
        &input,
        &[super::read_tool_spec()],
        None,
        None,
        Some(&json!({"reasoning": {"effort": "low"}})),
    )
    .unwrap();

    let actual = body;
    let expected = r#"{
  "input": [
    {
      "content": "system",
      "role": "system"
    },
    {
      "content": [
        {
          "text": "inspect",
          "type": "input_text"
        }
      ],
      "role": "user"
    },
    {
      "arguments": "{\"path\":\"README.md\"}",
      "call_id": "call-1",
      "name": "read",
      "type": "function_call"
    },
    {
      "call_id": "call-1",
      "output": "ok",
      "type": "function_call_output"
    }
  ],
  "model": "gpt-5.1",
  "reasoning": {
    "effort": "low"
  },
  "stream": true,
  "tools": [
    {
      "description": "Read a file",
      "name": "read",
      "parameters": {
        "properties": {
          "path": {
            "type": "string"
          }
        },
        "required": [
          "path"
        ],
        "type": "object"
      },
      "type": "function"
    }
  ]
}"#;
    assert_eq!(actual, serde_json::from_str::<Value>(expected).unwrap());
}

#[test]
fn responses_request_lowers_opencode_tool_choice_and_generation_options() {
    let input = openai_responses::input_from_llm_with_store(
        "system",
        vec![Message::user_text("inspect")],
        Some(false),
    )
    .unwrap();

    let body = openai_responses::request_body(
        "gpt-5.1",
        &input,
        &[super::read_tool_spec()],
        Some(&ToolChoice::Tool {
            name: "read".to_string(),
        }),
        Some(&GenerationOptions {
            max_tokens: Some(1000),
            temperature: Some(0.2),
            top_p: Some(0.9),
            ..GenerationOptions::default()
        }),
        None,
    )
    .unwrap();

    assert_eq!(
        body["tool_choice"],
        json!({"type": "function", "name": "read"})
    );
    assert_eq!(body["max_output_tokens"], json!(1000));
    assert_eq!(body["temperature"], json!(0.2));
    assert_eq!(body["top_p"], json!(0.9));
    assert!(body.get("frequency_penalty").is_none());
}

#[test]
fn responses_request_round_trips_reasoning_item_metadata() {
    let input = openai_responses::input_from_llm_with_store(
        "",
        vec![Message::Assistant {
            id: None,
            content: vec![MessageContent::Reasoning {
                value: json!({
                    "text": "summary",
                    "openai": {
                        "itemId": "rs_1",
                        "reasoningEncryptedContent": "encrypted"
                    }
                }),
            }],
        }],
        Some(false),
    )
    .unwrap();

    assert_eq!(
        input,
        vec![json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{"type": "summary_text", "text": "summary"}],
            "encrypted_content": "encrypted"
        })]
    );
}

#[test]
fn responses_request_allows_reasoning_without_encrypted_state_when_store_true() {
    let messages = vec![Message::Assistant {
        id: None,
        content: vec![MessageContent::Reasoning {
            value: json!({
                "text": "persisted summary",
                "openai": {"itemId": "rs_persisted"}
            }),
        }],
    }];

    let skipped =
        openai_responses::input_from_llm_with_store("", messages.clone(), Some(false)).unwrap();
    assert!(skipped.is_empty());

    let input = openai_responses::input_from_llm_with_store("", messages, Some(true)).unwrap();
    assert_eq!(
        input,
        vec![json!({
            "type": "reasoning",
            "id": "rs_persisted",
            "summary": [{"type": "summary_text", "text": "persisted summary"}],
            "encrypted_content": null
        })]
    );
}

#[test]
fn responses_stream_parses_reasoning_text_delta() {
    let mut state = openai_responses::StreamState::default();
    let events = openai_responses::parse_stream_event(
        &mut state,
        &json!({"type": "response.reasoning_text.delta", "delta": "thinking"}),
    )
    .unwrap();

    assert!(matches!(
        events.as_slice(),
        [crate::llm::schema::LlmEvent::ReasoningDelta { text }] if text == "thinking"
    ));
}

#[test]
fn responses_stream_parses_text_and_tool_calls() {
    let mut state = openai_responses::StreamState::default();
    let events = [
        json!({"type": "response.output_text.delta", "delta": "hello"}),
        json!({"type": "response.output_item.added", "item": {"type": "function_call", "id": "fc_1", "call_id": "call-2", "name": "read", "arguments": ""}}),
        json!({"type": "response.function_call_arguments.delta", "item_id": "fc_1", "delta": "{\"path\":"}),
        json!({"type": "response.output_item.done", "item": {"type": "function_call", "id": "fc_1", "call_id": "call-2", "name": "read", "arguments": "{\"path\":\"Cargo.toml\"}"}}),
        json!({"type": "response.completed", "response": {"usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}}}),
    ];
    let mut step = StepAccumulator::default();
    for event in events {
        for parsed in openai_responses::parse_stream_event(&mut state, &event).unwrap() {
            step.push(parsed).unwrap();
        }
    }
    let response = parsed_response_from_step(step);
    assert_eq!(response.text, "hello");
    assert_eq!(response.tool_calls[0].call_id, "call-2");
    assert_eq!(
        response.tool_calls[0].arguments_value().unwrap(),
        json!({"path": "Cargo.toml"})
    );
}

#[test]
fn responses_stream_preserves_reasoning_item_metadata_and_nested_errors() {
    let mut state = openai_responses::StreamState::default();
    let mut step = StepAccumulator::default();
    for event in openai_responses::parse_stream_event(
        &mut state,
        &json!({
            "type": "response.output_item.done",
            "item": {
                "type": "reasoning",
                "id": "rs_1",
                "summary": [{"type": "summary_text", "text": "summary"}],
                "encrypted_content": "encrypted"
            }
        }),
    )
    .unwrap()
    {
        step.push(event).unwrap();
    }
    assert_eq!(
        step.reasoning_content,
        Some(json!({
            "text": "summary",
            "openai": {
                "itemId": "rs_1",
                "reasoningEncryptedContent": "encrypted"
            }
        }))
    );

    let error = openai_responses::parse_stream_event(
        &mut state,
        &json!({
            "type": "response.failed",
            "response": {"error": {"code": "bad_request", "message": "nested failure"}}
        }),
    )
    .unwrap();
    assert!(matches!(
        &error[0],
        crate::llm::schema::LlmEvent::ProviderError { message, .. } if message == "nested failure"
    ));
}

#[test]
fn responses_stream_accepts_argument_delta_before_item_start() {
    let mut state = openai_responses::StreamState::default();
    let events = [
        json!({"type": "response.function_call_arguments.delta", "item_id": "fc_1", "delta": "{\"path\":"}),
        json!({"type": "response.output_item.added", "item": {"type": "function_call", "id": "fc_1", "call_id": "call-2", "name": "read", "arguments": ""}}),
        json!({"type": "response.function_call_arguments.delta", "item_id": "fc_1", "delta": "\"Cargo.toml\"}"}),
        json!({"type": "response.output_item.done", "item": {"type": "function_call", "id": "fc_1", "call_id": "call-2", "name": "read", "arguments": "{\"path\":\"Cargo.toml\"}"}}),
        json!({"type": "response.completed", "response": {"usage": {"input_tokens": 10, "output_tokens": 5}}}),
    ];
    let mut step = StepAccumulator::default();

    for event in events {
        for parsed in openai_responses::parse_stream_event(&mut state, &event).unwrap() {
            step.push(parsed).unwrap();
        }
    }

    let response = parsed_response_from_step(step);
    assert_eq!(response.tool_calls[0].call_id, "call-2");
    assert_eq!(
        response.tool_calls[0].arguments_value().unwrap(),
        json!({"path": "Cargo.toml"})
    );
}

#[test]
fn responses_stream_accepts_done_without_added_item() {
    let mut state = openai_responses::StreamState::default();
    let events = openai_responses::parse_stream_event(
        &mut state,
        &json!({
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "id": "fc_1",
                "call_id": "call-2",
                "name": "read",
                "arguments": "{\"path\":\"Cargo.toml\"}"
            }
        }),
    )
    .unwrap();
    let mut step = StepAccumulator::default();

    for event in events {
        step.push(event).unwrap();
    }

    let response = parsed_response_from_step(step);
    assert_eq!(response.tool_calls[0].call_id, "call-2");
    assert_eq!(
        response.tool_calls[0].arguments_value().unwrap(),
        json!({"path": "Cargo.toml"})
    );
}

#[test]
fn responses_hosted_tools_emit_provider_executed_call_and_result_without_dispatch() {
    let mut state = openai_responses::StreamState::default();
    let events = openai_responses::parse_stream_event(
        &mut state,
        &json!({
            "type": "response.output_item.done",
            "item": {
                "type": "web_search_call",
                "id": "ws_1",
                "action": {"query": "rust"},
                "status": "completed",
                "results": [{"title": "Rust"}]
            }
        }),
    )
    .unwrap();

    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        crate::llm::schema::LlmEvent::ToolCall { call, provider_executed: true }
            if call.call_id == "ws_1"
                && call.name == "web_search"
                && call.arguments_value().unwrap() == json!({"query": "rust"})
    ));
    assert!(matches!(
        &events[1],
        crate::llm::schema::LlmEvent::ToolResult { call_id, name, provider_executed: true, output }
            if call_id == "ws_1"
                && name == "web_search"
                && output["type"] == json!("json")
    ));

    let mut step = StepAccumulator::default();
    for event in events {
        step.push(event).unwrap();
    }
    assert!(step.tool_calls.is_empty());
}
