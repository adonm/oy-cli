use super::*;

use crate::llm::{GenerationOptions, ToolChoice, ToolSpec};

fn read_tool_spec() -> ToolSpec {
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

#[test]
fn chat_request_serializes_openai_tool_golden() {
    let messages = openai_chat::messages_from_llm(
        "system",
        vec![
            Message::user_text("inspect"),
            Message::Assistant {
                id: None,
                content: vec![MessageContent::ToolCall {
                    id: "call-1".to_string(),
                    call_id: None,
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
    )
    .unwrap();
    let body = openai_chat::request_body(
        "gpt-4.1-mini",
        &messages,
        &[read_tool_spec()],
        None,
        None,
        Some(&json!({"reasoning_effort": "low"})),
    )
    .unwrap();

    let actual = body;
    let expected = r#"{
  "messages": [
    {
      "content": "system",
      "role": "system"
    },
    {
      "content": "inspect",
      "role": "user"
    },
    {
      "content": null,
      "role": "assistant",
      "tool_calls": [
        {
          "function": {
            "arguments": "{\"path\":\"README.md\"}",
            "name": "read"
          },
          "id": "call-1",
          "type": "function"
        }
      ]
    },
    {
      "content": "ok",
      "role": "tool",
      "tool_call_id": "call-1"
    }
  ],
  "model": "gpt-4.1-mini",
  "reasoning_effort": "low",
  "stream": true,
  "stream_options": {
    "include_usage": true
  },
  "tools": [
    {
      "function": {
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
        }
      },
      "type": "function"
    }
  ]
}"#;
    assert_eq!(actual, serde_json::from_str::<Value>(expected).unwrap());
}

#[test]
fn chat_request_lowers_opencode_tool_choice_and_generation_options() {
    let messages =
        openai_chat::messages_from_llm("system", vec![Message::user_text("inspect")]).unwrap();

    let body = openai_chat::request_body(
        "gpt-4.1-mini",
        &messages,
        &[read_tool_spec()],
        Some(&ToolChoice::Tool {
            name: "read".to_string(),
        }),
        Some(&GenerationOptions {
            max_tokens: Some(1000),
            temperature: Some(0.2),
            top_p: Some(0.9),
            frequency_penalty: Some(0.1),
            presence_penalty: Some(0.3),
            seed: Some(42),
            stop: Some(vec!["END".to_string()]),
            ..GenerationOptions::default()
        }),
        None,
    )
    .unwrap();

    assert_eq!(
        body["tool_choice"],
        json!({"type": "function", "function": {"name": "read"}})
    );
    assert_eq!(body["max_tokens"], json!(1000));
    assert_eq!(body["temperature"], json!(0.2));
    assert_eq!(body["top_p"], json!(0.9));
    assert_eq!(body["frequency_penalty"], json!(0.1));
    assert_eq!(body["presence_penalty"], json!(0.3));
    assert_eq!(body["seed"], json!(42));
    assert_eq!(body["stop"], json!(["END"]));
}

#[test]
fn chat_stream_accumulates_tool_call_argument_deltas() {
    let mut state = openai_chat::StreamState::default();
    let first = openai_chat::parse_stream_event(
        &mut state,
        &json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call-1",
                "function": {"name": "read", "arguments": "{\"path\":"}
            }]}}]
        }),
    )
    .unwrap();
    let second = openai_chat::parse_stream_event(&mut state, &json!({
            "choices": [{
                "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"README.md\"}"}}]},
                "finish_reason": "tool_calls"
            }]
        }))
        .unwrap();
    let mut step = StepAccumulator::default();
    let finished = openai_chat::finish_stream(&mut state).unwrap();
    for event in first.into_iter().chain(second).chain(finished) {
        step.push(event).unwrap();
    }
    let parsed = parsed_assistant_from_step(step);
    assert_eq!(parsed.tool_calls[0].call_id, "call-1");
    assert_eq!(
        parsed.tool_calls[0].arguments_value().unwrap(),
        json!({"path": "README.md"})
    );

    let transcript = assistant_message_from_calls(&parsed.text, None, &parsed.tool_calls).unwrap();
    let Message::Assistant { content, .. } = transcript else {
        panic!("expected assistant transcript message");
    };
    assert!(matches!(
        &content[0],
        MessageContent::ToolCall { arguments, .. } if arguments == &json!({"path": "README.md"})
    ));
}

#[test]
fn chat_stream_preserves_reasoning_content_for_tool_call_echo() {
    let mut state = openai_chat::StreamState::default();
    let events = [
        json!({
            "choices": [{"delta": {"reasoning_content": "thinking "}}]
        }),
        json!({
            "choices": [{"delta": {"reasoning_content": "more"}}]
        }),
        json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call-1",
                "function": {"name": "echo", "arguments": "{}"}
            }]}, "finish_reason": "tool_calls"}]
        }),
    ];
    let mut step = StepAccumulator::default();
    for event in events {
        for parsed in openai_chat::parse_stream_event(&mut state, &event).unwrap() {
            step.push(parsed).unwrap();
        }
    }
    for parsed in openai_chat::finish_stream(&mut state).unwrap() {
        step.push(parsed).unwrap();
    }

    let parsed = parsed_assistant_from_step(step);
    assert_eq!(parsed.reasoning_content, Some(json!("thinking more")));
    let wire = openai_chat::assistant_wire_message(
        &parsed.text,
        parsed.reasoning_content.as_ref(),
        &parsed.tool_calls,
    )
    .unwrap();
    assert_eq!(wire["reasoning_content"], json!("thinking more"));
}

#[test]
fn chat_stream_defers_finish_until_usage_arrives_after_finish_reason() {
    let mut state = openai_chat::StreamState::default();
    let mut step = StepAccumulator::default();

    for event in openai_chat::parse_stream_event(
        &mut state,
        &json!({
            "choices": [{"delta": {"content": "hi"}, "finish_reason": "stop"}],
            "usage": null
        }),
    )
    .unwrap()
    {
        step.push(event).unwrap();
    }
    assert_eq!(step.text, "hi");
    assert_eq!(step.finish_reason, None);

    assert!(
        openai_chat::parse_stream_event(
            &mut state,
            &json!({
                "choices": [],
                "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}
            }),
        )
        .unwrap()
        .is_empty()
    );
    for event in openai_chat::finish_stream(&mut state).unwrap() {
        step.push(event).unwrap();
    }

    assert_eq!(
        step.finish_reason,
        Some(crate::llm::schema::FinishReason::Stop)
    );
    assert_eq!(step.usage.unwrap().total_tokens, Some(12));
}

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
        &[read_tool_spec()],
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
        &[read_tool_spec()],
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
