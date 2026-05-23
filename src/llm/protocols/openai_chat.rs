use anyhow::{Result, bail};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

use crate::llm::protocols::shared::{assistant_parts, tool_result_text};
use crate::llm::protocols::utils::provider_options;
use crate::llm::protocols::utils::tool_stream;
use crate::llm::schema::{FinishReason, LlmEvent, ToolCall, Usage};
use crate::llm::{GenerationOptions, Message, MessageContent, ToolChoice, ToolSpec};

const ROUTE: &str = "openai-chat";

#[derive(Debug, Default)]
pub(crate) struct StreamState {
    tools: HashMap<usize, tool_stream::PendingTool>,
    tool_call_events: Vec<LlmEvent>,
    usage: Option<Usage>,
    finish_reason: Option<FinishReason>,
}

pub(crate) fn request_body(
    model: &str,
    messages: &[Value],
    tools: &[ToolSpec],
    tool_choice: Option<&ToolChoice>,
    generation: Option<&GenerationOptions>,
    additional_params: Option<&Value>,
) -> Result<Value> {
    let mut body = Map::from_iter([
        ("model".to_string(), json!(model)),
        ("messages".to_string(), Value::Array(messages.to_vec())),
        ("stream".to_string(), Value::Bool(true)),
        ("stream_options".to_string(), json!({"include_usage": true})),
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

pub(crate) fn messages_from_llm(system_prompt: &str, messages: Vec<Message>) -> Result<Vec<Value>> {
    let mut wire = Vec::new();
    if !system_prompt.trim().is_empty() {
        wire.push(json!({"role": "system", "content": system_prompt}));
    }
    for message in messages {
        match message {
            Message::System { content, .. } => {
                wire.push(json!({"role": "system", "content": content}))
            }
            Message::User { content } => append_user_content(&mut wire, content)?,
            Message::Assistant { content, .. } => {
                let assistant = assistant_parts(content)?;
                wire.push(assistant_wire_message(
                    &assistant.text,
                    assistant.reasoning_content.as_ref(),
                    &assistant.tool_calls,
                )?);
            }
        }
    }
    Ok(wire)
}

pub(crate) fn assistant_wire_message(
    text: &str,
    reasoning_content: Option<&Value>,
    tool_calls: &[ToolCall],
) -> Result<Value> {
    let mut message = Map::from_iter([("role".to_string(), json!("assistant"))]);
    if !text.is_empty() || tool_calls.is_empty() {
        message.insert("content".to_string(), json!(text));
    } else {
        message.insert("content".to_string(), Value::Null);
    }
    if let Some(reasoning_content) = reasoning_content.and_then(Value::as_str) {
        message.insert("reasoning_content".to_string(), json!(reasoning_content));
    }
    if !tool_calls.is_empty() {
        message.insert(
            "tool_calls".to_string(),
            Value::Array(
                tool_calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.call_id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments,
                            }
                        })
                    })
                    .collect(),
            ),
        );
    }
    Ok(Value::Object(message))
}

pub(crate) fn tool_result_wire_message(call: &ToolCall, output: &str) -> Value {
    json!({
        "role": "tool",
        "tool_call_id": call.call_id,
        "content": output,
    })
}

pub(crate) fn parse_stream_event(state: &mut StreamState, event: &Value) -> Result<Vec<LlmEvent>> {
    let mut events = Vec::new();
    if let Some(usage) = event.get("usage").filter(|usage| !usage.is_null()) {
        state.usage = Some(Usage::from_openai_chat(usage));
    }
    let choice = event
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first());
    let Some(choice) = choice else {
        return Ok(events);
    };
    if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
        state.finish_reason = Some(map_finish_reason(Some(finish_reason)));
    }
    let delta = choice.get("delta").filter(|delta| !delta.is_null());
    if let Some(text) = delta
        .and_then(|delta| delta.get("content"))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        events.push(LlmEvent::TextDelta {
            text: text.to_string(),
        });
    }
    if let Some(text) = delta
        .and_then(|delta| delta.get("reasoning_content"))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        events.push(LlmEvent::ReasoningDelta {
            text: text.to_string(),
        });
    }
    for tool in delta
        .and_then(|delta| delta.get("tool_calls"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let index = tool.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let function = tool.get("function");
        events.extend(tool_stream::append_or_start(
            &mut state.tools,
            index,
            tool.get("id").and_then(Value::as_str),
            function
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str),
            function
                .and_then(|function| function.get("arguments"))
                .and_then(Value::as_str),
            ROUTE,
            "OpenAI Chat tool call delta is missing id or name",
        )?);
    }
    if state.finish_reason.is_some() && state.tool_call_events.is_empty() && !state.tools.is_empty()
    {
        state.tool_call_events = tool_stream::finish_all(ROUTE, &mut state.tools)?;
    }
    Ok(events)
}

pub(crate) fn finish_stream(state: &mut StreamState) -> Result<Vec<LlmEvent>> {
    let mut events = Vec::new();
    if state.tool_call_events.is_empty() && !state.tools.is_empty() {
        state.tool_call_events = tool_stream::finish_all(ROUTE, &mut state.tools)?;
    }
    let has_tool_calls = !state.tool_call_events.is_empty();
    events.append(&mut state.tool_call_events);
    if let Some(reason) = state.finish_reason.clone() {
        events.push(LlmEvent::StepFinish {
            reason: if matches!(reason, FinishReason::Stop) && has_tool_calls {
                FinishReason::ToolCalls
            } else {
                reason
            },
            usage: state.usage.clone(),
        });
    }
    Ok(events)
}

fn lower_tool_choice(tool_choice: Option<&ToolChoice>) -> Result<Option<Value>> {
    Ok(match tool_choice {
        None => None,
        Some(ToolChoice::Auto) => Some(json!("auto")),
        Some(ToolChoice::None) => Some(json!("none")),
        Some(ToolChoice::Required) => Some(json!("required")),
        Some(ToolChoice::Tool { name }) => {
            if name.trim().is_empty() {
                bail!("OpenAI Chat tool choice requires a tool name");
            }
            Some(json!({"type": "function", "function": {"name": name}}))
        }
    })
}

fn lower_generation_options(body: &mut Map<String, Value>, generation: Option<&GenerationOptions>) {
    let Some(generation) = generation else {
        return;
    };
    if let Some(value) = generation.max_tokens {
        body.insert("max_tokens".to_string(), json!(value));
    }
    if let Some(value) = generation.temperature {
        body.insert("temperature".to_string(), json!(value));
    }
    if let Some(value) = generation.top_p {
        body.insert("top_p".to_string(), json!(value));
    }
    if let Some(value) = generation.frequency_penalty {
        body.insert("frequency_penalty".to_string(), json!(value));
    }
    if let Some(value) = generation.presence_penalty {
        body.insert("presence_penalty".to_string(), json!(value));
    }
    if let Some(value) = generation.seed {
        body.insert("seed".to_string(), json!(value));
    }
    if let Some(stop) = generation.stop.as_ref().filter(|stop| !stop.is_empty()) {
        body.insert("stop".to_string(), json!(stop));
    }
}

fn map_finish_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("stop") => FinishReason::Stop,
        Some("length") => FinishReason::Length,
        Some("content_filter") => FinishReason::ContentFilter,
        Some("function_call" | "tool_calls") => FinishReason::ToolCalls,
        _ => FinishReason::Unknown,
    }
}

fn tool_spec(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": spec.parameters,
        }
    })
}

fn append_user_content(wire: &mut Vec<Value>, content: Vec<MessageContent>) -> Result<()> {
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
                    wire.push(json!({"role": "user", "content": text.join("\n")}));
                    text.clear();
                }
                wire.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id.unwrap_or(id),
                    "content": tool_result_text(content)?,
                }));
            }
            MessageContent::Opaque { value, .. } => {
                text.push(serde_json::to_string(&value)?);
            }
            MessageContent::ToolCall { .. } => bail!("user message cannot contain a tool call"),
            MessageContent::Reasoning { .. } => bail!("user message cannot contain reasoning"),
        }
    }
    if !text.is_empty() {
        wire.push(json!({"role": "user", "content": text.join("\n")}));
    }
    Ok(())
}
