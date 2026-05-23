use anyhow::{Result, bail};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

use crate::llm::protocols::shared::{assistant_parts, tool_result_text};
use crate::llm::protocols::utils::provider_options;
use crate::llm::protocols::utils::tool_stream;
use crate::llm::schema::{FinishReason, LlmEvent, ToolCall, Usage};
use crate::llm::{GenerationOptions, Message, MessageContent, ToolChoice, ToolSpec};

const ROUTE: &str = "openai-responses";

#[derive(Debug, Default)]
pub(crate) struct StreamState {
    tools: HashMap<String, tool_stream::PendingTool>,
    pending_argument_deltas: HashMap<String, String>,
    has_function_call: bool,
}

pub(crate) fn request_body(
    model: &str,
    input: &[Value],
    tools: &[ToolSpec],
    tool_choice: Option<&ToolChoice>,
    generation: Option<&GenerationOptions>,
    additional_params: Option<&Value>,
) -> Result<Value> {
    let mut body = Map::from_iter([
        ("model".to_string(), json!(model)),
        ("input".to_string(), Value::Array(input.to_vec())),
        ("stream".to_string(), Value::Bool(true)),
    ]);
    if !tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(tools.iter().map(tool_spec).collect()),
        );
    }
    if let Some(tool_choice) = lower_tool_choice(tool_choice)? {
        body.insert("tool_choice".to_string(), tool_choice);
    }
    lower_generation_options(&mut body, generation);
    provider_options::merge_json_body(ROUTE, &mut body, additional_params)?;
    Ok(Value::Object(body))
}

pub(crate) fn input_from_llm_with_store(
    system_prompt: &str,
    messages: Vec<Message>,
    store: Option<bool>,
) -> Result<Vec<Value>> {
    let mut input = Vec::new();
    if !system_prompt.trim().is_empty() {
        input.push(json!({"role": "system", "content": system_prompt}));
    }
    for message in messages {
        match message {
            Message::System { content, .. } => {
                input.push(response_message("system", "input_text", content))
            }
            Message::User { content } => append_user_content(&mut input, content)?,
            Message::Assistant { content, .. } => {
                append_assistant_content(&mut input, content, store)?
            }
        }
    }
    Ok(input)
}

pub(crate) fn append_assistant_output(input: &mut Vec<Value>, text: &str, tool_calls: &[ToolCall]) {
    if !text.is_empty() {
        input.push(response_message(
            "assistant",
            "output_text",
            text.to_string(),
        ));
    }
    for call in tool_calls {
        input.push(function_call(call));
    }
}

pub(crate) fn tool_result_input(call: &ToolCall, output: &str) -> Value {
    json!({
        "type": "function_call_output",
        "call_id": call.call_id,
        "output": output,
    })
}

pub(crate) fn parse_stream_event(state: &mut StreamState, event: &Value) -> Result<Vec<LlmEvent>> {
    let mut events = Vec::new();
    match event.get("type").and_then(Value::as_str) {
        Some("response.output_text.delta") => {
            if let Some(text) = event.get("delta").and_then(Value::as_str) {
                events.push(LlmEvent::TextDelta {
                    text: text.to_string(),
                });
            }
        }
        Some("response.reasoning_text.delta" | "response.reasoning_summary_text.delta") => {
            if let Some(text) = event.get("delta").and_then(Value::as_str) {
                events.push(LlmEvent::ReasoningDelta {
                    text: text.to_string(),
                });
            }
        }
        Some("response.output_item.added") => {
            let item = event.get("item").unwrap_or(&Value::Null);
            if item.get("type").and_then(Value::as_str) == Some("function_call")
                && let Some(item_id) = item.get("id").and_then(Value::as_str)
            {
                let id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or(item_id);
                let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
                let input = initial_tool_input(state, item_id, item);
                tool_stream::start(
                    &mut state.tools,
                    item_id.to_string(),
                    tool_stream::PendingTool {
                        id: id.to_string(),
                        name: name.to_string(),
                        input,
                        provider_executed: false,
                    },
                );
                events.push(LlmEvent::ToolInputStart {
                    id: id.to_string(),
                    name: name.to_string(),
                });
            }
        }
        Some("response.function_call_arguments.delta") => {
            if let (Some(item_id), Some(delta)) = (
                event.get("item_id").and_then(Value::as_str),
                event.get("delta").and_then(Value::as_str),
            ) {
                let key = item_id.to_string();
                if state.tools.contains_key(&key) {
                    events.extend(tool_stream::append_existing(
                        &mut state.tools,
                        &key,
                        delta,
                        "OpenAI Responses function call argument delta arrived before item start",
                    )?);
                } else if !delta.is_empty() {
                    state
                        .pending_argument_deltas
                        .entry(key)
                        .or_default()
                        .push_str(delta);
                }
            }
        }
        Some("response.output_item.done") => {
            let item = event.get("item").unwrap_or(&Value::Null);
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                let Some(item_id) = item.get("id").and_then(Value::as_str) else {
                    return Ok(events);
                };
                let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
                    return Ok(events);
                };
                let Some(name) = item.get("name").and_then(Value::as_str) else {
                    return Ok(events);
                };
                state.has_function_call = true;
                if !state.tools.contains_key(item_id) {
                    let input = initial_tool_input(state, item_id, item);
                    tool_stream::start(
                        &mut state.tools,
                        item_id.to_string(),
                        tool_stream::PendingTool {
                            id: call_id.to_string(),
                            name: name.to_string(),
                            input,
                            provider_executed: false,
                        },
                    );
                }
                let key = item_id.to_string();
                if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
                    events.extend(tool_stream::finish_with_input(
                        ROUTE,
                        &mut state.tools,
                        &key,
                        arguments,
                    )?);
                } else {
                    events.extend(tool_stream::finish(ROUTE, &mut state.tools, &key)?);
                }
            } else if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                if let Some(reasoning) = reasoning_item(item) {
                    events.push(LlmEvent::ReasoningItem { value: reasoning });
                }
            } else if let Some(hosted) = hosted_tool_events(item) {
                events.extend(hosted);
            }
        }
        Some("response.completed" | "response.incomplete") => {
            events.push(LlmEvent::StepFinish {
                reason: map_finish_reason(event, state.has_function_call),
                usage: event
                    .pointer("/response/usage")
                    .map(Usage::from_openai_responses),
            });
        }
        Some("response.failed") => events.push(LlmEvent::ProviderError {
            message: event
                .pointer("/response/error/message")
                .or_else(|| event.pointer("/response/error/code"))
                .or_else(|| event.get("message"))
                .or_else(|| event.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("OpenAI Responses response failed")
                .to_string(),
            retryable: false,
        }),
        Some("error") => events.push(LlmEvent::ProviderError {
            message: event
                .get("message")
                .or_else(|| event.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("OpenAI Responses stream error")
                .to_string(),
            retryable: false,
        }),
        _ => {}
    }
    Ok(events)
}

pub(crate) fn finish_stream(_state: &mut StreamState) -> Result<Vec<LlmEvent>> {
    Ok(Vec::new())
}

fn lower_tool_choice(tool_choice: Option<&ToolChoice>) -> Result<Option<Value>> {
    Ok(match tool_choice {
        None => None,
        Some(ToolChoice::Auto) => Some(json!("auto")),
        Some(ToolChoice::None) => Some(json!("none")),
        Some(ToolChoice::Required) => Some(json!("required")),
        Some(ToolChoice::Tool { name }) => {
            if name.trim().is_empty() {
                bail!("OpenAI Responses tool choice requires a tool name");
            }
            Some(json!({"type": "function", "name": name}))
        }
    })
}

fn lower_generation_options(body: &mut Map<String, Value>, generation: Option<&GenerationOptions>) {
    let Some(generation) = generation else {
        return;
    };
    if let Some(value) = generation.max_tokens {
        body.insert("max_output_tokens".to_string(), json!(value));
    }
    if let Some(value) = generation.temperature {
        body.insert("temperature".to_string(), json!(value));
    }
    if let Some(value) = generation.top_p {
        body.insert("top_p".to_string(), json!(value));
    }
}

fn map_finish_reason(event: &Value, has_function_call: bool) -> FinishReason {
    match event
        .pointer("/response/incomplete_details/reason")
        .and_then(Value::as_str)
    {
        None => {
            if has_function_call {
                FinishReason::ToolCalls
            } else {
                FinishReason::Stop
            }
        }
        Some("max_output_tokens") => FinishReason::Length,
        Some("content_filter") => FinishReason::ContentFilter,
        Some(_) if has_function_call => FinishReason::ToolCalls,
        Some(_) => FinishReason::Unknown,
    }
}

fn initial_tool_input(state: &mut StreamState, item_id: &str, item: &Value) -> String {
    state
        .pending_argument_deltas
        .remove(item_id)
        .unwrap_or_else(|| {
            item.get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
}

fn hosted_tool_events(item: &Value) -> Option<Vec<LlmEvent>> {
    let kind = item.get("type").and_then(Value::as_str)?;
    let id = item.get("id").and_then(Value::as_str)?.to_string();
    let (name, input) = match kind {
        "web_search_call" => (
            "web_search",
            item.get("action").cloned().unwrap_or_else(|| json!({})),
        ),
        "web_search_preview_call" => (
            "web_search_preview",
            item.get("action").cloned().unwrap_or_else(|| json!({})),
        ),
        "file_search_call" => (
            "file_search",
            json!({"queries": item.get("queries").cloned().unwrap_or_else(|| json!([]))}),
        ),
        "code_interpreter_call" => (
            "code_interpreter",
            json!({
                "code": item.get("code").cloned().unwrap_or(Value::Null),
                "container_id": item.get("container_id").cloned().unwrap_or(Value::Null),
            }),
        ),
        "computer_use_call" => (
            "computer_use",
            item.get("action").cloned().unwrap_or_else(|| json!({})),
        ),
        "image_generation_call" => ("image_generation", json!({})),
        "mcp_call" => (
            "mcp",
            json!({
                "server_label": item.get("server_label").cloned().unwrap_or(Value::Null),
                "name": item.get("name").cloned().unwrap_or(Value::Null),
                "arguments": item.get("arguments").cloned().unwrap_or(Value::Null),
            }),
        ),
        "local_shell_call" => (
            "local_shell",
            item.get("action").cloned().unwrap_or_else(|| json!({})),
        ),
        _ => return None,
    };
    let call = ToolCall {
        call_id: id.clone(),
        id: id.clone(),
        name: name.to_string(),
        arguments: input.to_string(),
    };
    let output = if item.get("error").is_some_and(|error| !error.is_null()) {
        json!({"type": "error", "value": item.get("error").cloned().unwrap_or(Value::Null)})
    } else {
        json!({"type": "json", "value": item})
    };
    Some(vec![
        LlmEvent::ToolCall {
            call,
            provider_executed: true,
        },
        LlmEvent::ToolResult {
            call_id: id,
            name: name.to_string(),
            output,
            provider_executed: true,
        },
    ])
}

fn tool_spec(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "name": spec.name,
        "description": spec.description,
        "parameters": spec.parameters,
    })
}

fn append_user_content(input: &mut Vec<Value>, content: Vec<MessageContent>) -> Result<()> {
    let mut text = Vec::new();
    for item in content {
        match item {
            MessageContent::Text { text: value, .. } => text.push(value),
            MessageContent::ToolResult {
                id,
                call_id,
                content,
                ..
            } => {
                if !text.is_empty() {
                    input.push(response_message("user", "input_text", text.join("\n")));
                    text.clear();
                }
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id.unwrap_or(id),
                    "output": tool_result_text(content)?,
                }));
            }
            MessageContent::Opaque { value, .. } => text.push(serde_json::to_string(&value)?),
            MessageContent::ToolCall { .. } => bail!("user message cannot contain a tool call"),
            MessageContent::Reasoning { .. } => bail!("user message cannot contain reasoning"),
        }
    }
    if !text.is_empty() {
        input.push(response_message("user", "input_text", text.join("\n")));
    }
    Ok(())
}

fn append_assistant_content(
    input: &mut Vec<Value>,
    content: Vec<MessageContent>,
    store: Option<bool>,
) -> Result<()> {
    let assistant = assistant_parts(content)?;
    if let Some(reasoning) = assistant.reasoning_content.as_ref()
        && let Some(item) = lower_reasoning(reasoning, store)
    {
        input.push(item);
    }
    append_assistant_output(input, &assistant.text, &assistant.tool_calls);
    Ok(())
}

fn lower_reasoning(value: &Value, store: Option<bool>) -> Option<Value> {
    let object = value.as_object()?;
    let item_id = object
        .get("openai")
        .and_then(Value::as_object)
        .unwrap_or(object)
        .get("itemId")
        .and_then(Value::as_str)?;
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| object.get("summary").and_then(Value::as_str))
        .unwrap_or_default();
    let encrypted_content = object
        .get("openai")
        .and_then(Value::as_object)
        .unwrap_or(object)
        .get("reasoningEncryptedContent")
        .cloned();
    if store == Some(false) && !encrypted_content.as_ref().is_some_and(Value::is_string) {
        return None;
    }

    Some(json!({
        "type": "reasoning",
        "id": item_id,
        "summary": if text.is_empty() {
            Value::Array(Vec::new())
        } else {
            json!([{"type": "summary_text", "text": text}])
        },
        "encrypted_content": encrypted_content.unwrap_or(Value::Null),
    }))
}

fn reasoning_item(item: &Value) -> Option<Value> {
    let id = item.get("id").and_then(Value::as_str)?;
    let text = item
        .get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|summary| summary.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    Some(json!({
        "text": text,
        "openai": {
            "itemId": id,
            "reasoningEncryptedContent": item.get("encrypted_content").cloned().unwrap_or(Value::Null),
        }
    }))
}

fn function_call(call: &ToolCall) -> Value {
    json!({
        "type": "function_call",
        "call_id": call.call_id,
        "name": call.name,
        "arguments": call.arguments,
    })
}

fn response_message(role: &str, content_type: &str, text: String) -> Value {
    json!({
        "role": role,
        "content": [{"type": content_type, "text": text}],
    })
}
