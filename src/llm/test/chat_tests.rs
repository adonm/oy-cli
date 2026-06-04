use super::*;

use crate::llm::{GenerationOptions, ToolChoice};

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
        &[super::read_tool_spec()],
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
        &[super::read_tool_spec()],
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
fn chat_request_omits_non_string_reasoning_content() {
    let wire = openai_chat::assistant_wire_message(
        "",
        Some(&json!({
            "text": "summary",
            "openai": {
                "itemId": "rs_1",
                "reasoningEncryptedContent": "encrypted"
            }
        })),
        &[NativeToolCall {
            id: "call-1".to_string(),
            call_id: "call-1".to_string(),
            name: "echo".to_string(),
            arguments: "{}".to_string(),
            signature: None,
        }],
    )
    .unwrap();

    assert!(wire.get("reasoning_content").is_none());
    assert_eq!(wire["content"], Value::Null);
    assert_eq!(wire["tool_calls"][0]["id"], json!("call-1"));
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
