use anyhow::{Result, bail};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

use crate::llm::protocols::shared::{assistant_parts, tool_result_text};
use crate::llm::protocols::utils::{provider_options, tool_stream};
use crate::llm::schema::{FinishReason, LlmEvent, Usage};
use crate::llm::{
    CacheHint, GenerationOptions, LlmRequest, Message, MessageContent, ToolChoice, ToolSpec,
};

const ROUTE: &str = "anthropic-messages";
const BREAKPOINT_CAP: usize = 4;

#[derive(Debug, Default)]
pub(crate) struct StreamState {
    tools: HashMap<usize, tool_stream::PendingTool>,
    usage: Option<Usage>,
    finish_reason: Option<FinishReason>,
    has_tool_calls: bool,
}

pub(crate) fn request_body(request: &LlmRequest) -> Result<Value> {
    let mut breakpoints = crate::llm::cache_policy::Breakpoints::new(BREAKPOINT_CAP);
    let system = lower_system(
        &request.system_prompt,
        request.system_cache.as_ref(),
        &request.messages,
        &mut breakpoints,
    );
    let messages = lower_messages(&request.messages, &mut breakpoints)?;
    let mut body = Map::from_iter([
        ("model".to_string(), json!(request.route.model)),
        ("messages".to_string(), Value::Array(messages)),
        ("stream".to_string(), Value::Bool(true)),
        (
            "max_tokens".to_string(),
            json!(
                request
                    .generation
                    .as_ref()
                    .and_then(|generation| generation.max_tokens)
                    .unwrap_or(4096)
            ),
        ),
    ]);
    if !system.is_empty() {
        body.insert("system".to_string(), Value::Array(system));
    }
    if !request.tools.is_empty() && !matches!(request.tool_choice, Some(ToolChoice::None)) {
        body.insert(
            "tools".to_string(),
            Value::Array(lower_tools(&request.tools, &mut breakpoints)?),
        );
    }
    if let Some(tool_choice) = lower_tool_choice(request.tool_choice.as_ref())? {
        body.insert("tool_choice".to_string(), tool_choice);
    }
    lower_generation_options(&mut body, request.generation.as_ref());
    provider_options::merge_json_body(ROUTE, &mut body, request.route.additional_params.as_ref())?;
    Ok(Value::Object(body))
}

pub(crate) fn parse_stream_event(state: &mut StreamState, event: &Value) -> Result<Vec<LlmEvent>> {
    let mut events = Vec::new();
    match event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "message_start" => {
            if let Some(usage) = event.pointer("/message/usage") {
                state.usage = merge_usage(state.usage.clone(), Usage::from_anthropic(usage));
            }
        }
        "content_block_start" => {
            let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let Some(block) = event.get("content_block") else {
                return Ok(events);
            };
            match block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    tool_stream::start(
                        &mut state.tools,
                        index,
                        tool_stream::PendingTool {
                            id: id.clone(),
                            name: name.clone(),
                            input: String::new(),
                        },
                    );
                    events.push(LlmEvent::ToolInputStart { id, name });
                }
                "text" => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        events.push(LlmEvent::TextDelta {
                            text: text.to_string(),
                        });
                    }
                }
                "thinking" => {
                    if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                        events.push(LlmEvent::ReasoningDelta {
                            text: text.to_string(),
                        });
                    }
                }
                _ => {}
            }
        }
        "content_block_delta" => {
            let Some(delta) = event.get("delta") else {
                return Ok(events);
            };
            match delta
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "text_delta" => {
                    if let Some(text) = delta.get("text").and_then(Value::as_str) {
                        events.push(LlmEvent::TextDelta {
                            text: text.to_string(),
                        });
                    }
                }
                "thinking_delta" => {
                    if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                        events.push(LlmEvent::ReasoningDelta {
                            text: text.to_string(),
                        });
                    }
                }
                "input_json_delta" => {
                    if let Some(text) = delta.get("partial_json").and_then(Value::as_str) {
                        let index =
                            event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        events.extend(tool_stream::append_existing(
                            &mut state.tools,
                            &index,
                            text,
                            "Anthropic Messages tool argument delta is missing its tool call",
                        )?);
                    }
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            if let Some(index) = event.get("index").and_then(Value::as_u64) {
                let finished = tool_stream::finish(ROUTE, &mut state.tools, &(index as usize))?;
                state.has_tool_calls |= finished
                    .iter()
                    .any(|event| matches!(event, LlmEvent::ToolCall { .. }));
                events.extend(finished);
            }
        }
        "message_delta" => {
            if let Some(usage) = event.get("usage") {
                state.usage = merge_usage(state.usage.clone(), Usage::from_anthropic(usage));
            }
            state.finish_reason = Some(map_finish_reason(
                event.pointer("/delta/stop_reason").and_then(Value::as_str),
            ));
        }
        "error" => {
            events.push(LlmEvent::ProviderError {
                message: event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Anthropic Messages stream error")
                    .to_string(),
                retryable: false,
            });
        }
        _ => {}
    }
    Ok(events)
}

pub(crate) fn finish_stream(state: &mut StreamState) -> Result<Vec<LlmEvent>> {
    if state.finish_reason.is_none() && !state.tools.is_empty() {
        let finished = tool_stream::finish_all(ROUTE, &mut state.tools)?;
        state.has_tool_calls |= finished
            .iter()
            .any(|event| matches!(event, LlmEvent::ToolCall { .. }));
        state.finish_reason = Some(FinishReason::ToolCalls);
        let mut events = finished;
        events.push(LlmEvent::StepFinish {
            reason: FinishReason::ToolCalls,
            usage: state.usage.clone(),
        });
        return Ok(events);
    }
    let Some(reason) = state.finish_reason.clone() else {
        return Ok(Vec::new());
    };
    Ok(vec![LlmEvent::StepFinish {
        reason: if matches!(reason, FinishReason::Stop) && state.has_tool_calls {
            FinishReason::ToolCalls
        } else {
            reason
        },
        usage: state.usage.clone(),
    }])
}

fn lower_system(
    system_prompt: &str,
    system_cache: Option<&CacheHint>,
    messages: &[Message],
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) -> Vec<Value> {
    let mut system = Vec::new();
    if !system_prompt.trim().is_empty() {
        system.push(text_block(system_prompt, system_cache, breakpoints));
    }
    for message in messages {
        if let Message::System { content, cache } = message
            && !content.trim().is_empty()
        {
            system.push(text_block(content, cache.as_ref(), breakpoints));
        }
    }
    system
}

fn lower_messages(
    messages: &[Message],
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for message in messages {
        match message {
            Message::System { .. } => {}
            Message::User { content } => out.push(
                json!({"role": "user", "content": lower_user_content(content, breakpoints)?}),
            ),
            Message::Assistant { content, .. } => {
                out.push(json!({"role": "assistant", "content": lower_assistant_content(content)?}))
            }
        }
    }
    Ok(out)
}

fn lower_user_content(
    content: &[MessageContent],
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) -> Result<Vec<Value>> {
    let mut blocks = Vec::new();
    for item in content {
        match item {
            MessageContent::Text { text, cache } => {
                blocks.push(text_block(text, cache.as_ref(), breakpoints))
            }
            MessageContent::ToolResult {
                id,
                call_id,
                content,
                cache,
            } => {
                let mut block = Map::from_iter([
                    ("type".to_string(), json!("tool_result")),
                    (
                        "tool_use_id".to_string(),
                        json!(call_id.as_ref().unwrap_or(id)),
                    ),
                    (
                        "content".to_string(),
                        json!(tool_result_text(content.clone())?),
                    ),
                ]);
                insert_cache_control(&mut block, cache.as_ref(), breakpoints);
                blocks.push(Value::Object(block));
            }
            MessageContent::Opaque { value, cache } => blocks.push(text_block(
                &serde_json::to_string(value)?,
                cache.as_ref(),
                breakpoints,
            )),
            MessageContent::ToolCall { .. } => {
                bail!("Anthropic user message cannot contain a tool call")
            }
            MessageContent::Reasoning { .. } => {
                bail!("Anthropic user message cannot contain reasoning")
            }
        }
    }
    Ok(blocks)
}

fn lower_assistant_content(content: &[MessageContent]) -> Result<Vec<Value>> {
    let assistant = assistant_parts(content.to_vec())?;
    let mut blocks = Vec::new();
    if let Some(reasoning) = assistant.reasoning_content
        && let Some(text) = reasoning.as_str()
    {
        blocks.push(json!({"type": "thinking", "thinking": text}));
    }
    if !assistant.text.is_empty() {
        blocks.push(json!({"type": "text", "text": assistant.text}));
    }
    for call in assistant.tool_calls {
        blocks.push(json!({"type": "tool_use", "id": call.call_id, "name": call.name, "input": call.arguments_value()?}));
    }
    if blocks.is_empty() {
        blocks.push(json!({"type": "text", "text": ""}));
    }
    Ok(blocks)
}

fn lower_tools(
    tools: &[ToolSpec],
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for tool in tools {
        if !tool.parameters.is_object() {
            bail!(
                "Anthropic Messages tool `{}` parameters must be a JSON object schema",
                tool.name
            );
        }
        let mut block = Map::from_iter([
            ("name".to_string(), json!(tool.name)),
            ("description".to_string(), json!(tool.description)),
            ("input_schema".to_string(), tool.parameters.clone()),
        ]);
        insert_cache_control(&mut block, tool.cache.as_ref(), breakpoints);
        out.push(Value::Object(block));
    }
    Ok(out)
}

fn lower_tool_choice(tool_choice: Option<&ToolChoice>) -> Result<Option<Value>> {
    Ok(match tool_choice {
        None => None,
        Some(ToolChoice::Auto) => Some(json!({"type": "auto"})),
        Some(ToolChoice::None) => None,
        Some(ToolChoice::Required) => Some(json!({"type": "any"})),
        Some(ToolChoice::Tool { name }) => {
            if name.trim().is_empty() {
                bail!("Anthropic Messages tool choice requires a tool name");
            }
            Some(json!({"type": "tool", "name": name}))
        }
    })
}

fn lower_generation_options(body: &mut Map<String, Value>, generation: Option<&GenerationOptions>) {
    let Some(generation) = generation else {
        return;
    };
    if let Some(value) = generation.temperature {
        body.insert("temperature".to_string(), json!(value));
    }
    if let Some(value) = generation.top_p {
        body.insert("top_p".to_string(), json!(value));
    }
    if let Some(value) = generation.top_k {
        body.insert("top_k".to_string(), json!(value));
    }
    if let Some(stop) = generation.stop.as_ref().filter(|stop| !stop.is_empty()) {
        body.insert("stop_sequences".to_string(), json!(stop));
    }
}

fn text_block(
    text: &str,
    cache: Option<&CacheHint>,
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) -> Value {
    let mut block = Map::from_iter([
        ("type".to_string(), json!("text")),
        ("text".to_string(), json!(text)),
    ]);
    insert_cache_control(&mut block, cache, breakpoints);
    Value::Object(block)
}

fn insert_cache_control(
    block: &mut Map<String, Value>,
    cache: Option<&CacheHint>,
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) {
    if !crate::llm::cache_policy::cache_point_allowed(breakpoints, cache) {
        return;
    }
    let ttl = match cache {
        Some(CacheHint::Ephemeral { ttl_seconds })
        | Some(CacheHint::Persistent { ttl_seconds }) => {
            crate::llm::cache_policy::ttl_bucket(*ttl_seconds)
        }
        None => None,
    };
    let mut control = Map::from_iter([("type".to_string(), json!("ephemeral"))]);
    if let Some(ttl) = ttl {
        control.insert("ttl".to_string(), json!(ttl));
    }
    block.insert("cache_control".to_string(), Value::Object(control));
}

fn map_finish_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("end_turn" | "stop_sequence" | "pause_turn") => FinishReason::Stop,
        Some("max_tokens") => FinishReason::Length,
        Some("tool_use") => FinishReason::ToolCalls,
        Some("refusal") => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
    }
}

fn merge_usage(left: Option<Usage>, right: Option<Usage>) -> Option<Usage> {
    match (left, right) {
        (None, right) => right,
        (left, None) => left,
        (Some(left), Some(right)) => Some(right.merge_prefer_defined(left)),
    }
}

#[cfg(test)]
#[path = "../test/protocols/anthropic_messages.rs"]
mod tests;
