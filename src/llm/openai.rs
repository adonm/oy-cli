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
    anthropic_messages, bedrock_converse, bedrock_event_stream, gemini, openai_chat,
    openai_responses,
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
        Protocol::Gemini => run_gemini(request, tools).await,
    }
}

async fn run_gemini(request: LlmRequest, tools: LlmTools) -> Result<LlmResponse> {
    run_message_protocol(request, tools, MessageProtocol::Gemini).await
}

async fn run_anthropic_messages(request: LlmRequest, tools: LlmTools) -> Result<LlmResponse> {
    run_message_protocol(request, tools, MessageProtocol::AnthropicMessages).await
}

async fn run_bedrock_converse(request: LlmRequest, tools: LlmTools) -> Result<LlmResponse> {
    run_message_protocol(request, tools, MessageProtocol::BedrockConverse).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageProtocol {
    Gemini,
    AnthropicMessages,
    BedrockConverse,
}

async fn run_message_protocol(
    request: LlmRequest,
    tools: LlmTools,
    protocol: MessageProtocol,
) -> Result<LlmResponse> {
    let endpoint = message_protocol_endpoint(protocol, &request)?;
    let client = reqwest::Client::new();
    let tools_by_name = tool_runtime::tools_by_name(tools);
    let mut request = request;
    let mut transcript = Vec::new();
    let mut loop_state = tool_runtime::ToolLoopState::default();

    for turn in 0..=request.max_turns {
        let body = message_protocol_request_body(protocol, &request)?;
        let assistant = retry_transient_http_call(|| {
            stream_message_protocol_assistant(
                protocol,
                &client,
                &endpoint,
                &request.route.auth,
                &body,
            )
        })
        .await?;
        let assistant_message = assistant_message_from_calls(
            &assistant.text,
            assistant_reasoning_content(protocol, &assistant),
            &assistant.tool_calls,
        )?;

        if assistant.tool_calls.is_empty() {
            transcript.push(assistant_message);
            return Ok(LlmResponse {
                output: assistant.text,
                messages: Some(transcript),
            });
        }
        ensure_tool_round_budget(turn, request.max_turns, message_protocol_label(protocol))?;
        loop_state.note_assistant_turn(&assistant.text, &assistant.tool_calls)?;

        request.messages.push(assistant_message.clone());
        transcript.push(assistant_message);
        for call in assistant.tool_calls {
            let outcome =
                tool_runtime::execute_tool_call(&tools_by_name, &mut loop_state, &call).await;
            let result =
                message_protocol_tool_result_message(protocol, &call, outcome.output.clone());
            request.messages.push(result.clone());
            transcript.push(result);
        }
    }

    unreachable!("bounded tool loop exits from inside the loop")
}

fn message_protocol_endpoint(protocol: MessageProtocol, request: &LlmRequest) -> Result<String> {
    match protocol {
        MessageProtocol::Gemini => super::route::endpoint::render_with_query(
            request.route.base_url.as_deref(),
            crate::llm::providers::GEMINI_BASE_URL,
            &gemini::endpoint_path(&request.route.model),
            Some(&[("alt".to_string(), "sse".to_string())]),
        ),
        MessageProtocol::AnthropicMessages => super::route::endpoint::render_with_query(
            request.route.base_url.as_deref(),
            "https://api.anthropic.com/v1",
            "messages",
            request.route.query_params.as_deref(),
        ),
        MessageProtocol::BedrockConverse => super::route::endpoint::render_with_query(
            request.route.base_url.as_deref(),
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            &bedrock_converse::endpoint_path(&request.route.model),
            request.route.query_params.as_deref(),
        ),
    }
}

fn message_protocol_request_body(protocol: MessageProtocol, request: &LlmRequest) -> Result<Value> {
    match protocol {
        MessageProtocol::Gemini => gemini::request_body(request),
        MessageProtocol::AnthropicMessages => anthropic_messages::request_body(request),
        MessageProtocol::BedrockConverse => bedrock_converse::request_body(request),
    }
}

async fn stream_message_protocol_assistant(
    protocol: MessageProtocol,
    client: &reqwest::Client,
    endpoint: &str,
    auth: &RouteAuth,
    body: &Value,
) -> Result<ParsedAssistant> {
    match protocol {
        MessageProtocol::Gemini => stream_gemini_assistant(client, endpoint, auth, body).await,
        MessageProtocol::AnthropicMessages => {
            stream_anthropic_assistant(client, endpoint, auth, body).await
        }
        MessageProtocol::BedrockConverse => {
            stream_bedrock_assistant(client, endpoint, auth, body).await
        }
    }
}

fn assistant_reasoning_content<'a>(
    protocol: MessageProtocol,
    assistant: &'a ParsedAssistant,
) -> Option<&'a Value> {
    match protocol {
        MessageProtocol::BedrockConverse => None,
        MessageProtocol::Gemini | MessageProtocol::AnthropicMessages => {
            assistant.reasoning_content.as_ref()
        }
    }
}

fn message_protocol_tool_result_message(
    protocol: MessageProtocol,
    call: &NativeToolCall,
    output: String,
) -> Message {
    match protocol {
        MessageProtocol::Gemini => gemini_tool_result_message(call, output),
        MessageProtocol::AnthropicMessages | MessageProtocol::BedrockConverse => {
            tool_result_message(call, output)
        }
    }
}

fn message_protocol_label(protocol: MessageProtocol) -> &'static str {
    match protocol {
        MessageProtocol::Gemini => "Gemini",
        MessageProtocol::AnthropicMessages => "Anthropic Messages",
        MessageProtocol::BedrockConverse => "Bedrock Converse",
    }
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
    let store = request
        .route
        .additional_params
        .as_ref()
        .and_then(|params| params.get("store"))
        .and_then(Value::as_bool);
    let mut input = openai_responses::input_from_llm_with_store(
        &request.system_prompt,
        request.messages,
        store,
    )?;
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
        let assistant_message = assistant_message_from_calls(
            &response.text,
            response.reasoning_content.as_ref(),
            &response.tool_calls,
        )?;

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
        bail!("native {protocol} exceeded the tool round budget");
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

async fn stream_gemini_assistant(
    client: &reqwest::Client,
    endpoint: &str,
    auth: &RouteAuth,
    body: &Value,
) -> Result<ParsedAssistant> {
    let response =
        super::route::transport::post_json_streaming(client, endpoint, auth, body).await?;
    let step = super::route::transport::stream_json_sse_events(
        response,
        gemini::StreamState::default(),
        |state, event| gemini::parse_stream_event(state, &event),
        gemini::finish_stream,
    )
    .await?;
    Ok(parsed_assistant_from_step(step))
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
        reasoning_content: step.reasoning_content,
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

fn gemini_tool_result_message(call: &NativeToolCall, output: String) -> Message {
    Message::User {
        content: vec![MessageContent::ToolResult {
            id: call.name.clone(),
            call_id: None,
            content: vec![ToolResultContent::Text { text: output }],
            cache: None,
        }],
    }
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
    reasoning_content: Option<Value>,
    tool_calls: Vec<NativeToolCall>,
}

#[cfg(test)]
#[path = "test/executor.rs"]
mod tests;
