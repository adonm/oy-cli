use anyhow::{Result, bail};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

use crate::llm::protocols::utils::{bedrock_cache, provider_options, tool_stream};
use crate::llm::schema::{FinishReason, LlmEvent, Usage};
use crate::llm::{
    GenerationOptions, LlmRequest, Message, MessageContent, ToolChoice, ToolResultContent, ToolSpec,
};

const ROUTE: &str = "bedrock-converse";

#[derive(Debug, Default)]
pub(crate) struct StreamState {
    tools: HashMap<usize, tool_stream::PendingTool>,
    pending_finish: Option<(FinishReason, Option<Usage>)>,
    has_tool_calls: bool,
}

pub(crate) fn endpoint_path(model_id: &str) -> String {
    format!(
        "model/{}/converse-stream",
        percent_encode_path_segment(model_id)
    )
}

pub(crate) fn request_body(request: &LlmRequest) -> Result<Value> {
    let mut breakpoints = bedrock_cache::breakpoints();
    let mut body = Map::from_iter([
        ("modelId".to_string(), json!(request.route.model)),
        (
            "messages".to_string(),
            Value::Array(lower_messages(&request.messages, &mut breakpoints)?),
        ),
    ]);
    let system = lower_system(
        &request.system_prompt,
        request.system_cache.as_ref(),
        &request.messages,
        &mut breakpoints,
    );
    if !system.is_empty() {
        body.insert("system".to_string(), Value::Array(system));
    }
    if !request.tools.is_empty() && !matches!(request.tool_choice, Some(ToolChoice::None)) {
        let mut tool_config = Map::from_iter([(
            "tools".to_string(),
            Value::Array(lower_tools(&request.tools, &mut breakpoints)?),
        )]);
        if let Some(tool_choice) = lower_tool_choice(request.tool_choice.as_ref())? {
            tool_config.insert("toolChoice".to_string(), tool_choice);
        }
        body.insert("toolConfig".to_string(), Value::Object(tool_config));
    }
    if let Some(inference_config) = lower_inference_config(request.generation.as_ref()) {
        body.insert("inferenceConfig".to_string(), inference_config);
    }
    provider_options::merge_json_body(ROUTE, &mut body, request.route.additional_params.as_ref())?;
    Ok(Value::Object(body))
}

pub(crate) fn parse_stream_event(state: &mut StreamState, event: &Value) -> Result<Vec<LlmEvent>> {
    let mut events = Vec::new();
    if let Some(tool_use) = event
        .pointer("/contentBlockStart/start/toolUse")
        .and_then(Value::as_object)
    {
        let index = event
            .pointer("/contentBlockStart/contentBlockIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let id = tool_use
            .get("toolUseId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let name = tool_use
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        tool_stream::start(
            &mut state.tools,
            index,
            tool_stream::PendingTool::new(ROUTE, id.clone(), name.clone(), String::new(), false)?,
        );
        events.push(LlmEvent::ToolInputStart { id, name });
    }
    if let Some(delta) = event.pointer("/contentBlockDelta/delta") {
        if let Some(text) = delta.get("text").and_then(Value::as_str) {
            events.push(LlmEvent::TextDelta {
                text: text.to_string(),
            });
        }
        if let Some(text) = delta
            .pointer("/reasoningContent/text")
            .and_then(Value::as_str)
        {
            events.push(LlmEvent::ReasoningDelta {
                text: text.to_string(),
            });
        }
        if let Some(input) = delta.pointer("/toolUse/input").and_then(Value::as_str) {
            let index = event
                .pointer("/contentBlockDelta/contentBlockIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            events.extend(tool_stream::append_existing(
                &mut state.tools,
                &index,
                input,
                ROUTE,
                "Bedrock Converse tool delta is missing its tool call",
            )?);
        }
    }
    if let Some(index) = event
        .pointer("/contentBlockStop/contentBlockIndex")
        .and_then(Value::as_u64)
    {
        let finished = tool_stream::finish(ROUTE, &mut state.tools, &(index as usize))?;
        state.has_tool_calls |= finished
            .iter()
            .any(|event| matches!(event, LlmEvent::ToolCall { .. }));
        events.extend(finished);
    }
    if let Some(reason) = event
        .pointer("/messageStop/stopReason")
        .and_then(Value::as_str)
    {
        state.pending_finish = Some((map_finish_reason(reason), state.pending_finish_usage()));
    }
    if let Some(usage) = event.pointer("/metadata/usage") {
        let reason = state
            .pending_finish
            .as_ref()
            .map(|(reason, _)| reason.clone())
            .unwrap_or(FinishReason::Stop);
        state.pending_finish = Some((reason, Some(Usage::from_bedrock(usage))));
    }
    if let Some(error) = provider_error(event) {
        events.push(error);
    }
    Ok(events)
}

pub(crate) fn finish_stream(state: &mut StreamState) -> Result<Vec<LlmEvent>> {
    let Some((reason, usage)) = state.pending_finish.take() else {
        return Ok(Vec::new());
    };
    Ok(vec![LlmEvent::StepFinish {
        reason: if matches!(reason, FinishReason::Stop) && state.has_tool_calls {
            FinishReason::ToolCalls
        } else {
            reason
        },
        usage,
    }])
}

fn lower_tool_choice(tool_choice: Option<&ToolChoice>) -> Result<Option<Value>> {
    Ok(match tool_choice {
        None | Some(ToolChoice::None) => None,
        Some(ToolChoice::Auto) => Some(json!({"auto": {}})),
        Some(ToolChoice::Required) => Some(json!({"any": {}})),
        Some(ToolChoice::Tool { name }) => {
            if name.trim().is_empty() {
                bail!("Bedrock Converse tool choice requires a tool name");
            }
            Some(json!({"tool": {"name": name}}))
        }
    })
}

fn lower_inference_config(generation: Option<&GenerationOptions>) -> Option<Value> {
    let generation = generation?;
    let mut config = Map::new();
    if let Some(value) = generation.max_tokens {
        config.insert("maxTokens".to_string(), json!(value));
    }
    if let Some(value) = generation.temperature {
        config.insert("temperature".to_string(), json!(value));
    }
    if let Some(value) = generation.top_p {
        config.insert("topP".to_string(), json!(value));
    }
    if let Some(stop) = generation.stop.as_ref().filter(|stop| !stop.is_empty()) {
        config.insert("stopSequences".to_string(), json!(stop));
    }
    (!config.is_empty()).then_some(Value::Object(config))
}

fn lower_tools(
    tools: &[ToolSpec],
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    for tool in tools {
        if !tool.parameters.is_object() {
            bail!(
                "Bedrock Converse tool `{}` parameters must be a JSON object schema",
                tool.name
            );
        }
        result.push(json!({
            "toolSpec": {
                "name": tool.name,
                "description": tool.description,
                "inputSchema": {"json": tool.parameters},
            }
        }));
        if let Some(cache_point) = bedrock_cache::block(breakpoints, tool.cache.as_ref()) {
            result.push(cache_point);
        }
    }
    Ok(result)
}

fn lower_system(
    system_prompt: &str,
    system_cache: Option<&crate::llm::CacheHint>,
    messages: &[Message],
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) -> Vec<Value> {
    let mut blocks = Vec::new();
    if !system_prompt.trim().is_empty() {
        blocks.push(json!({"text": system_prompt}));
        if let Some(cache_point) = bedrock_cache::block(breakpoints, system_cache) {
            blocks.push(cache_point);
        }
    }
    for message in messages {
        if let Message::System { content, cache } = message {
            blocks.push(json!({"text": content}));
            if let Some(cache_point) = bedrock_cache::block(breakpoints, cache.as_ref()) {
                blocks.push(cache_point);
            }
        }
    }
    blocks
}

fn lower_messages(
    messages: &[Message],
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    for message in messages {
        match message {
            Message::System { .. } => {}
            Message::User { content } => result.push(json!({
                "role": "user",
                "content": lower_user_content(content, breakpoints)?,
            })),
            Message::Assistant { content, .. } => result.push(json!({
                "role": "assistant",
                "content": lower_assistant_content(content, breakpoints)?,
            })),
        }
    }
    Ok(result)
}

fn lower_user_content(
    content: &[MessageContent],
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) -> Result<Vec<Value>> {
    let mut blocks = Vec::new();
    for item in content {
        match item {
            MessageContent::Text { text, cache } => {
                push_text_with_cache(&mut blocks, text, cache.as_ref(), breakpoints)
            }
            MessageContent::ToolResult {
                id, content, cache, ..
            } => {
                blocks.push(json!({
                    "toolResult": {
                        "toolUseId": id,
                        "content": lower_tool_result_content(content),
                        "status": tool_result_status(content),
                    }
                }));
                if let Some(cache_point) = bedrock_cache::block(breakpoints, cache.as_ref()) {
                    blocks.push(cache_point);
                }
            }
            MessageContent::Opaque { value, cache } => push_text_with_cache(
                &mut blocks,
                &serde_json::to_string(value)?,
                cache.as_ref(),
                breakpoints,
            ),
            MessageContent::ToolCall { .. } => {
                bail!("user message cannot contain a Bedrock tool call")
            }
            MessageContent::Reasoning { .. } => {
                bail!("user message cannot contain Bedrock reasoning content")
            }
        }
    }
    Ok(blocks)
}

fn lower_assistant_content(
    content: &[MessageContent],
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) -> Result<Vec<Value>> {
    let mut blocks = Vec::new();
    for item in content {
        match item {
            MessageContent::Text { text, cache } => {
                push_text_with_cache(&mut blocks, text, cache.as_ref(), breakpoints)
            }
            MessageContent::Reasoning { value } => blocks.push(json!({
                "reasoningContent": {"reasoningText": {"text": value.as_str().unwrap_or_default()}}
            })),
            MessageContent::ToolCall {
                id,
                name,
                arguments,
                ..
            } => blocks.push(json!({
                "toolUse": {"toolUseId": id, "name": name, "input": arguments}
            })),
            MessageContent::Opaque { value, cache } => push_text_with_cache(
                &mut blocks,
                &serde_json::to_string(value)?,
                cache.as_ref(),
                breakpoints,
            ),
            MessageContent::ToolResult { .. } => {
                bail!("assistant message cannot contain a Bedrock tool result")
            }
        }
    }
    Ok(blocks)
}

fn push_text_with_cache(
    blocks: &mut Vec<Value>,
    text: &str,
    cache: Option<&crate::llm::CacheHint>,
    breakpoints: &mut crate::llm::cache_policy::Breakpoints,
) {
    blocks.push(json!({"text": text}));
    if let Some(cache_point) = bedrock_cache::block(breakpoints, cache) {
        blocks.push(cache_point);
    }
}

fn lower_tool_result_content(content: &[ToolResultContent]) -> Vec<Value> {
    content
        .iter()
        .map(|item| match item {
            ToolResultContent::Text { text } => json!({"text": text}),
            ToolResultContent::Opaque { value } => json!({"json": value}),
        })
        .collect()
}

fn tool_result_status(content: &[ToolResultContent]) -> &'static str {
    if content
        .iter()
        .any(|item| matches!(item, ToolResultContent::Opaque { value } if value.get("type").and_then(Value::as_str) == Some("error")))
    {
        "error"
    } else {
        "success"
    }
}

fn map_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "end_turn" | "stop_sequence" => FinishReason::Stop,
        "max_tokens" => FinishReason::Length,
        "tool_use" => FinishReason::ToolCalls,
        "content_filtered" | "guardrail_intervened" => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
    }
}

fn provider_error(event: &Value) -> Option<LlmEvent> {
    for (key, retryable) in [
        ("internalServerException", true),
        ("modelStreamErrorException", true),
        ("serviceUnavailableException", true),
        ("validationException", false),
        ("throttlingException", true),
    ] {
        if let Some(message) = event
            .pointer(&format!("/{key}/message"))
            .and_then(Value::as_str)
        {
            return Some(LlmEvent::ProviderError {
                message: message.to_string(),
                retryable,
            });
        }
    }
    None
}

fn percent_encode_path_segment(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

impl StreamState {
    fn pending_finish_usage(&self) -> Option<Usage> {
        self.pending_finish
            .as_ref()
            .and_then(|(_, usage)| usage.clone())
    }
}

#[cfg(test)]
#[path = "../test/protocols/bedrock_converse.rs"]
mod tests;
