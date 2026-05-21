//! Native OpenAI Chat Completions and Responses backends with an
//! OpenCode-shaped HTTP+SSE protocol layer and hardened tool loop.
//!
//! This is the default transport: it lowers [`LlmRequest`] into
//! provider-native request bodies, frames streaming SSE responses into
//! step events, runs the native tool loop with error recovery, blocks
//! repeated identical failed calls, caps model-visible tool output, and
//! shares tool-round budget checks across both protocols.

use super::{
    ChatBackend, ChatFuture, LlmRequest, LlmResponse, LlmTools, Message, MessageContent, Protocol,
    RouteAuth, ToolResultContent,
};
use anyhow::{Context, Result, bail};
use backon::Retryable;
use serde_json::{Value, json};
use std::future::Future;

use super::protocols::{
    anthropic_messages, bedrock_converse, bedrock_event_stream, openai_chat, openai_responses,
};
use super::schema::{StepAccumulator, ToolCall as NativeToolCall};
use super::tool_runtime;

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct NativeOpenAiBackend;

impl ChatBackend for NativeOpenAiBackend {
    type Tools = LlmTools;

    fn chat<'a>(&'a self, request: LlmRequest, tools: Self::Tools) -> ChatFuture<'a> {
        Box::pin(async move { execute_native_chat(request, tools).await })
    }
}

async fn execute_native_chat(request: LlmRequest, tools: LlmTools) -> Result<LlmResponse> {
    let request = super::cache_policy::apply(request);
    match request.route.protocol {
        Protocol::OpenAiChat => run_chat_completions(request, tools).await,
        Protocol::OpenAiResponses => run_responses(request, tools).await,
        Protocol::AnthropicMessages => run_anthropic_messages(request, tools).await,
        Protocol::BedrockConverse => run_bedrock_converse(request, tools).await,
    }
}

async fn run_anthropic_messages(request: LlmRequest, tools: LlmTools) -> Result<LlmResponse> {
    let endpoint = super::route::endpoint::render_with_query(
        request.route.base_url.as_deref(),
        "https://api.anthropic.com/v1",
        "messages",
        request.route.query_params.as_deref(),
    )?;
    let client = reqwest::Client::new();
    let tools_by_name = tool_runtime::tools_by_name(tools);
    let mut request = request;
    let mut transcript = Vec::new();
    let mut loop_state = tool_runtime::ToolLoopState::default();

    for turn in 0..=request.max_turns {
        let body = anthropic_messages::request_body(&request)?;
        let assistant = retry_transient_http_call(|| {
            stream_anthropic_assistant(&client, &endpoint, &request.route.auth, &body)
        })
        .await?;
        let assistant_message = assistant_message_from_calls(
            &assistant.text,
            assistant.reasoning_content.as_ref(),
            &assistant.tool_calls,
        )?;

        if assistant.tool_calls.is_empty() {
            transcript.push(assistant_message);
            return Ok(LlmResponse {
                output: assistant.text,
                messages: Some(transcript),
            });
        }
        ensure_tool_round_budget(turn, request.max_turns, "Anthropic Messages")?;
        loop_state.note_assistant_turn(&assistant.text, &assistant.tool_calls)?;

        request.messages.push(assistant_message.clone());
        transcript.push(assistant_message);
        for call in assistant.tool_calls {
            let outcome =
                tool_runtime::execute_tool_call(&tools_by_name, &mut loop_state, &call).await;
            let result = tool_result_message(&call, outcome.output.clone());
            request.messages.push(result.clone());
            transcript.push(result);
        }
    }

    unreachable!("bounded tool loop exits from inside the loop")
}

async fn run_bedrock_converse(request: LlmRequest, tools: LlmTools) -> Result<LlmResponse> {
    let endpoint = super::route::endpoint::render_with_query(
        request.route.base_url.as_deref(),
        "https://bedrock-runtime.us-east-1.amazonaws.com",
        &bedrock_converse::endpoint_path(&request.route.model),
        request.route.query_params.as_deref(),
    )?;
    let client = reqwest::Client::new();
    let tools_by_name = tool_runtime::tools_by_name(tools);
    let mut request = request;
    let mut transcript = Vec::new();
    let mut loop_state = tool_runtime::ToolLoopState::default();

    for turn in 0..=request.max_turns {
        let body = bedrock_converse::request_body(&request)?;
        let assistant = retry_transient_http_call(|| {
            stream_bedrock_assistant(&client, &endpoint, &request.route.auth, &body)
        })
        .await?;
        let assistant_message =
            assistant_message_from_calls(&assistant.text, None, &assistant.tool_calls)?;

        if assistant.tool_calls.is_empty() {
            transcript.push(assistant_message);
            return Ok(LlmResponse {
                output: assistant.text,
                messages: Some(transcript),
            });
        }
        ensure_tool_round_budget(turn, request.max_turns, "Bedrock Converse")?;
        loop_state.note_assistant_turn(&assistant.text, &assistant.tool_calls)?;

        request.messages.push(assistant_message.clone());
        transcript.push(assistant_message);
        for call in assistant.tool_calls {
            let outcome =
                tool_runtime::execute_tool_call(&tools_by_name, &mut loop_state, &call).await;
            let result = tool_result_message(&call, outcome.output.clone());
            request.messages.push(result.clone());
            transcript.push(result);
        }
    }

    unreachable!("bounded tool loop exits from inside the loop")
}

async fn run_chat_completions(request: LlmRequest, tools: LlmTools) -> Result<LlmResponse> {
    let endpoint = super::route::endpoint::render_with_query(
        request.route.base_url.as_deref(),
        OPENAI_BASE_URL,
        "chat/completions",
        request.route.query_params.as_deref(),
    )?;
    let client = reqwest::Client::new();
    let tool_specs = request.tools.clone();
    let tools_by_name = tool_runtime::tools_by_name(tools);
    let mut messages = openai_chat::messages_from_llm(&request.system_prompt, request.messages)?;
    let mut transcript = Vec::new();
    let mut loop_state = tool_runtime::ToolLoopState::default();

    for turn in 0..=request.max_turns {
        let body = openai_chat::request_body(
            &request.route.model,
            &messages,
            &tool_specs,
            request.tool_choice.as_ref(),
            request.generation.as_ref(),
            request.route.additional_params.as_ref(),
        )?;
        let assistant = retry_transient_http_call(|| {
            stream_chat_assistant(&client, &endpoint, &request.route.auth, &body)
        })
        .await?;
        let assistant_message = assistant_message_from_calls(
            &assistant.text,
            assistant.reasoning_content.as_ref(),
            &assistant.tool_calls,
        )?;

        if assistant.tool_calls.is_empty() {
            transcript.push(assistant_message);
            return Ok(LlmResponse {
                output: assistant.text,
                messages: Some(transcript),
            });
        }
        ensure_tool_round_budget(turn, request.max_turns, "chat")?;
        loop_state.note_assistant_turn(&assistant.text, &assistant.tool_calls)?;

        messages.push(openai_chat::assistant_wire_message(
            &assistant.text,
            assistant.reasoning_content.as_ref(),
            &assistant.tool_calls,
        )?);
        transcript.push(assistant_message);
        for call in assistant.tool_calls {
            let outcome =
                tool_runtime::execute_tool_call(&tools_by_name, &mut loop_state, &call).await;
            let result = tool_result_message(&call, outcome.output.clone());
            messages.push(openai_chat::tool_result_wire_message(
                &call,
                &outcome.output,
            ));
            transcript.push(result);
        }
    }

    unreachable!("bounded tool loop exits from inside the loop")
}

async fn run_responses(request: LlmRequest, tools: LlmTools) -> Result<LlmResponse> {
    let endpoint = super::route::endpoint::render_with_query(
        request.route.base_url.as_deref(),
        OPENAI_BASE_URL,
        "responses",
        request.route.query_params.as_deref(),
    )?;
    let client = reqwest::Client::new();
    let tool_specs = request.tools.clone();
    let tools_by_name = tool_runtime::tools_by_name(tools);
    let mut input = openai_responses::input_from_llm(&request.system_prompt, request.messages)?;
    let mut transcript = Vec::new();
    let mut loop_state = tool_runtime::ToolLoopState::default();

    for turn in 0..=request.max_turns {
        let body = openai_responses::request_body(
            &request.route.model,
            &input,
            &tool_specs,
            request.tool_choice.as_ref(),
            request.generation.as_ref(),
            request.route.additional_params.as_ref(),
        )?;
        let response = retry_transient_http_call(|| {
            stream_responses_output(&client, &endpoint, &request.route.auth, &body)
        })
        .await?;
        let assistant_message =
            assistant_message_from_calls(&response.text, None, &response.tool_calls)?;

        if response.tool_calls.is_empty() {
            transcript.push(assistant_message);
            return Ok(LlmResponse {
                output: response.text,
                messages: Some(transcript),
            });
        }
        ensure_tool_round_budget(turn, request.max_turns, "Responses")?;
        loop_state.note_assistant_turn(&response.text, &response.tool_calls)?;

        openai_responses::append_assistant_output(&mut input, &response.text, &response.tool_calls);
        transcript.push(assistant_message);
        for call in response.tool_calls {
            let outcome =
                tool_runtime::execute_tool_call(&tools_by_name, &mut loop_state, &call).await;
            transcript.push(tool_result_message(&call, outcome.output.clone()));
            input.push(openai_responses::tool_result_input(&call, &outcome.output));
        }
    }

    unreachable!("bounded tool loop exits from inside the loop")
}

fn ensure_tool_round_budget(turn: usize, max_turns: usize, protocol: &str) -> Result<()> {
    if turn >= max_turns {
        bail!("native OpenAI {protocol} exceeded the tool round budget");
    }
    Ok(())
}

async fn retry_transient_http_call<T, Fut, F>(operation: F) -> Result<T>
where
    Fut: Future<Output = Result<T>>,
    F: FnMut() -> Fut,
{
    operation
        .retry(crate::agent::retry::llm_backoff())
        .when(crate::agent::retry::is_transient_error)
        .notify(|_, dur| {
            crate::ui::err_line(format_args!(
                "retrying LLM HTTP call in {:.0}s…",
                dur.as_secs_f64()
            ))
        })
        .await
}

async fn stream_anthropic_assistant(
    client: &reqwest::Client,
    endpoint: &str,
    auth: &RouteAuth,
    body: &Value,
) -> Result<ParsedAssistant> {
    let response =
        super::route::transport::post_json_streaming(client, endpoint, auth, body).await?;
    let step = super::route::transport::stream_json_sse_events(
        response,
        anthropic_messages::StreamState::default(),
        |state, event| anthropic_messages::parse_stream_event(state, &event),
        anthropic_messages::finish_stream,
    )
    .await?;
    Ok(parsed_assistant_from_step(step))
}

async fn stream_chat_assistant(
    client: &reqwest::Client,
    endpoint: &str,
    auth: &RouteAuth,
    body: &Value,
) -> Result<ParsedAssistant> {
    let response =
        super::route::transport::post_json_streaming(client, endpoint, auth, body).await?;
    let step = super::route::transport::stream_json_sse_events(
        response,
        openai_chat::StreamState::default(),
        |state, event| openai_chat::parse_stream_event(state, &event),
        openai_chat::finish_stream,
    )
    .await?;
    Ok(parsed_assistant_from_step(step))
}

async fn stream_responses_output(
    client: &reqwest::Client,
    endpoint: &str,
    auth: &RouteAuth,
    body: &Value,
) -> Result<ParsedResponse> {
    let response =
        super::route::transport::post_json_streaming(client, endpoint, auth, body).await?;
    let step = super::route::transport::stream_json_sse_events(
        response,
        openai_responses::StreamState::default(),
        |state, event| openai_responses::parse_stream_event(state, &event),
        openai_responses::finish_stream,
    )
    .await?;
    Ok(parsed_response_from_step(step))
}

async fn stream_bedrock_assistant(
    client: &reqwest::Client,
    endpoint: &str,
    auth: &RouteAuth,
    body: &Value,
) -> Result<ParsedAssistant> {
    let response =
        super::route::transport::post_bedrock_json_streaming(client, endpoint, auth, body).await?;
    let mut stream = response.bytes_stream();
    let mut decoder = bedrock_event_stream::Decoder::default();
    let mut state = bedrock_converse::StreamState::default();
    let mut step = StepAccumulator::default();

    while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
        let chunk = chunk.context("failed to read native Bedrock event-stream chunk")?;
        for event in decoder.push_chunk(&chunk)? {
            for event in bedrock_converse::parse_stream_event(&mut state, &event)? {
                step.push(event)?;
            }
        }
    }
    for event in bedrock_converse::finish_stream(&mut state)? {
        step.push(event)?;
    }
    Ok(parsed_assistant_from_step(step))
}

fn parsed_assistant_from_step(step: StepAccumulator) -> ParsedAssistant {
    ParsedAssistant {
        text: step.text,
        reasoning_content: step.reasoning_content,
        tool_calls: step.tool_calls,
    }
}

fn parsed_response_from_step(step: StepAccumulator) -> ParsedResponse {
    ParsedResponse {
        text: step.text,
        tool_calls: step.tool_calls,
    }
}

fn assistant_message_from_calls(
    text: &str,
    reasoning_content: Option<&Value>,
    tool_calls: &[NativeToolCall],
) -> Result<Message> {
    let mut content = Vec::new();
    if let Some(value) = reasoning_content {
        content.push(MessageContent::Reasoning {
            value: value.clone(),
        });
    }
    if !text.is_empty() {
        content.push(MessageContent::Text {
            text: text.to_string(),
            cache: None,
        });
    }
    for call in tool_calls {
        let arguments = call.arguments_value().unwrap_or_else(|err| {
            json!({
                "invalid_json_arguments": call.arguments,
                "error": err.to_string(),
            })
        });
        content.push(MessageContent::ToolCall {
            id: call.id.clone(),
            call_id: Some(call.call_id.clone()),
            name: call.name.clone(),
            arguments,
            signature: None,
            additional_params: None,
        });
    }
    if content.is_empty() {
        content.push(MessageContent::Text {
            text: String::new(),
            cache: None,
        });
    }
    Ok(Message::Assistant { id: None, content })
}

fn tool_result_message(call: &NativeToolCall, output: String) -> Message {
    Message::User {
        content: vec![MessageContent::ToolResult {
            id: format!("result-{}", call.call_id),
            call_id: Some(call.call_id.clone()),
            content: vec![ToolResultContent::Text { text: output }],
            cache: None,
        }],
    }
}

#[derive(Debug, Clone)]
struct ParsedAssistant {
    text: String,
    reasoning_content: Option<Value>,
    tool_calls: Vec<NativeToolCall>,
}

#[derive(Debug, Clone)]
struct ParsedResponse {
    text: String,
    tool_calls: Vec<NativeToolCall>,
}

#[cfg(test)]
mod tests {
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
            "native OpenAI chat exceeded the tool round budget"
        );

        let responses = ensure_tool_round_budget(1, 1, "Responses").unwrap_err();
        assert_eq!(
            responses.to_string(),
            "native OpenAI Responses exceeded the tool round budget"
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

        let transcript =
            assistant_message_from_calls(&parsed.text, None, &parsed.tool_calls).unwrap();
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
        let input = openai_responses::input_from_llm(
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
        let input = openai_responses::input_from_llm("system", vec![Message::user_text("inspect")])
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
}
