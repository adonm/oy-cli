use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use crate::llm::protocols::shared::{assistant_parts, tool_result_text};
use crate::llm::protocols::utils::provider_options;
use crate::llm::schema::{FinishReason, LlmEvent, ToolCall, Usage};
use crate::llm::{GenerationOptions, LlmRequest, Message, MessageContent, ToolChoice, ToolSpec};

const ROUTE: &str = "gemini";

#[derive(Debug, Default)]
pub(crate) struct StreamState {
    finish_reason: Option<String>,
    has_tool_calls: bool,
    next_tool_call_id: usize,
    usage: Option<Usage>,
}

pub(crate) fn endpoint_path(model: &str) -> String {
    format!("models/{model}:streamGenerateContent")
}

pub(crate) fn request_body(request: &LlmRequest) -> Result<Value> {
    let tools_enabled =
        !request.tools.is_empty() && !matches!(request.tool_choice, Some(ToolChoice::None));
    let mut body = Map::from_iter([(
        "contents".to_string(),
        Value::Array(lower_messages(request)?),
    )]);

    if !request.system_prompt.trim().is_empty() {
        body.insert(
            "systemInstruction".to_string(),
            json!({"parts": [{"text": request.system_prompt}]}),
        );
    }
    if tools_enabled {
        body.insert(
            "tools".to_string(),
            json!([{ "functionDeclarations": lower_tools(&request.tools) }]),
        );
        if let Some(tool_config) = lower_tool_config(request.tool_choice.as_ref())? {
            body.insert("toolConfig".to_string(), tool_config);
        }
    }
    if let Some(generation_config) = lower_generation_options(request.generation.as_ref()) {
        body.insert("generationConfig".to_string(), generation_config);
    }
    provider_options::merge_json_body(ROUTE, &mut body, request.route.additional_params.as_ref())?;
    Ok(Value::Object(body))
}

pub(crate) fn parse_stream_event(state: &mut StreamState, event: &Value) -> Result<Vec<LlmEvent>> {
    let mut events = Vec::new();
    if let Some(usage) = event.get("usageMetadata") {
        state.usage = Some(Usage::from_gemini(usage));
    }
    let candidate = event
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|items| items.first());
    let Some(candidate) = candidate else {
        return Ok(events);
    };
    if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
        state.finish_reason = Some(reason.to_string());
    }
    let parts = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for part in parts {
        if let Some(text) = part
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            if part
                .get("thought")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                events.push(LlmEvent::ReasoningDelta {
                    text: text.to_string(),
                });
            } else {
                events.push(LlmEvent::TextDelta {
                    text: text.to_string(),
                });
            }
        }
        if let Some(call) = part.get("functionCall") {
            let name = call
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if name.trim().is_empty() {
                bail!("Gemini tool call is missing a name");
            }
            let id = format!("tool_{}", state.next_tool_call_id);
            state.next_tool_call_id += 1;
            let arguments =
                serde_json::to_string(call.get("args").unwrap_or(&Value::Object(Map::new())))?;
            let mut tool_call = ToolCall::from_raw_input(id, name, &arguments, ROUTE)?;
            tool_call.signature = thought_signature(part);
            state.has_tool_calls = true;
            events.push(LlmEvent::ToolCall {
                call: tool_call,
                provider_executed: false,
            });
        }
    }
    Ok(events)
}

fn thought_signature(part: &Value) -> Option<String> {
    part.get("thoughtSignature")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn finish_stream(state: &mut StreamState) -> Result<Vec<LlmEvent>> {
    if state.finish_reason.is_none() && state.usage.is_none() {
        return Ok(Vec::new());
    }
    Ok(vec![LlmEvent::StepFinish {
        reason: map_finish_reason(state.finish_reason.as_deref(), state.has_tool_calls),
        usage: state.usage.clone(),
    }])
}

fn lower_messages(request: &LlmRequest) -> Result<Vec<Value>> {
    let mut contents = Vec::new();
    for message in &request.messages {
        match message {
            Message::System { .. } => {}
            Message::User { content } => contents.push(json!({
                "role": "user",
                "parts": lower_user_content(content)?,
            })),
            Message::Assistant { content, .. } => contents.push(json!({
                "role": "model",
                "parts": lower_assistant_content(content)?,
            })),
        }
    }
    Ok(contents)
}

fn lower_user_content(content: &[MessageContent]) -> Result<Vec<Value>> {
    let mut parts = Vec::new();
    for item in content {
        match item {
            MessageContent::Text { text, .. } => parts.push(json!({"text": text})),
            MessageContent::ToolResult {
                id,
                call_id,
                content,
                ..
            } => {
                let name = call_id.as_ref().unwrap_or(id);
                parts.push(json!({
                    "functionResponse": {
                        "name": name,
                        "response": {
                            "name": name,
                            "content": tool_result_text(content.clone())?,
                        }
                    }
                }));
            }
            MessageContent::Opaque { value, .. } => {
                parts.push(json!({"text": serde_json::to_string(value)?}))
            }
            MessageContent::ToolCall { .. } => {
                bail!("Gemini user message cannot contain a tool call")
            }
            MessageContent::Reasoning { .. } => {
                bail!("Gemini user message cannot contain reasoning")
            }
        }
    }
    Ok(parts)
}

fn lower_assistant_content(content: &[MessageContent]) -> Result<Vec<Value>> {
    let assistant = assistant_parts(content.to_vec())?;
    let mut parts = Vec::new();
    if let Some(reasoning) = assistant.reasoning_content
        && let Some(text) = reasoning.as_str()
    {
        parts.push(json!({"text": text, "thought": true}));
    }
    if !assistant.text.is_empty() {
        parts.push(json!({"text": assistant.text}));
    }
    for call in assistant.tool_calls {
        let mut part = Map::from_iter([(
            "functionCall".to_string(),
            json!({"name": call.name, "args": call.arguments_value()?}),
        )]);
        insert_thought_signature(&mut part, call.signature.as_deref());
        parts.push(Value::Object(part));
    }
    if parts.is_empty() {
        parts.push(json!({"text": ""}));
    }
    Ok(parts)
}

fn insert_thought_signature(part: &mut Map<String, Value>, signature: Option<&str>) {
    if let Some(signature) = signature.filter(|signature| !signature.is_empty()) {
        part.insert(
            "thoughtSignature".to_string(),
            Value::String(signature.to_string()),
        );
    }
}

fn lower_tools(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": convert_tool_schema(&tool.parameters),
            })
        })
        .collect()
}

fn lower_tool_config(tool_choice: Option<&ToolChoice>) -> Result<Option<Value>> {
    Ok(match tool_choice {
        None | Some(ToolChoice::Auto) => Some(json!({"functionCallingConfig": {"mode": "AUTO"}})),
        Some(ToolChoice::None) => None,
        Some(ToolChoice::Required) => Some(json!({"functionCallingConfig": {"mode": "ANY"}})),
        Some(ToolChoice::Tool { name }) => {
            if name.trim().is_empty() {
                bail!("Gemini tool choice requires a tool name");
            }
            Some(json!({"functionCallingConfig": {"mode": "ANY", "allowedFunctionNames": [name]}}))
        }
    })
}

fn lower_generation_options(generation: Option<&GenerationOptions>) -> Option<Value> {
    let generation = generation?;
    let mut object = Map::new();
    if let Some(value) = generation.max_tokens {
        object.insert("maxOutputTokens".to_string(), json!(value));
    }
    if let Some(value) = generation.temperature {
        object.insert("temperature".to_string(), json!(value));
    }
    if let Some(value) = generation.top_p {
        object.insert("topP".to_string(), json!(value));
    }
    if let Some(value) = generation.top_k {
        object.insert("topK".to_string(), json!(value));
    }
    if let Some(stop) = generation.stop.as_ref().filter(|stop| !stop.is_empty()) {
        object.insert("stopSequences".to_string(), json!(stop));
    }
    (!object.is_empty()).then_some(Value::Object(object))
}

fn map_finish_reason(reason: Option<&str>, has_tool_calls: bool) -> FinishReason {
    match reason {
        Some("STOP") if has_tool_calls => FinishReason::ToolCalls,
        Some("STOP") => FinishReason::Stop,
        Some("MAX_TOKENS") => FinishReason::Length,
        Some(
            "IMAGE_SAFETY" | "RECITATION" | "SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII",
        ) => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
    }
}

fn convert_tool_schema(schema: &Value) -> Value {
    project_schema(&sanitize_schema(schema)).unwrap_or(Value::Null)
}

fn sanitize_schema(schema: &Value) -> Value {
    match schema {
        Value::Array(items) => Value::Array(items.iter().map(sanitize_schema).collect()),
        Value::Object(object) => {
            let mut result = object
                .iter()
                .map(|(key, value)| {
                    let value = if key == "enum" {
                        value
                            .as_array()
                            .map(|items| {
                                Value::Array(items.iter().map(stringify_schema_enum).collect())
                            })
                            .unwrap_or_else(|| sanitize_schema(value))
                    } else {
                        sanitize_schema(value)
                    };
                    (key.clone(), value)
                })
                .collect::<Map<_, _>>();
            let schema_type = result
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string);
            if matches!(schema_type.as_deref(), Some("integer" | "number"))
                && result.get("enum").is_some()
            {
                result.insert("type".to_string(), json!("string"));
            }
            if schema_type.as_deref() == Some("object")
                && let (Some(Value::Object(properties)), Some(Value::Array(required))) =
                    (result.get("properties"), result.get("required"))
            {
                let filtered = required
                    .iter()
                    .filter(|item| {
                        item.as_str()
                            .is_some_and(|field| properties.contains_key(field))
                    })
                    .cloned()
                    .collect();
                result.insert("required".to_string(), Value::Array(filtered));
            }
            if schema_type.as_deref() == Some("array")
                && !has_combiner(&Value::Object(result.clone()))
            {
                let items = result
                    .entry("items".to_string())
                    .or_insert_with(|| json!({}));
                if items
                    .as_object()
                    .is_some_and(|object| !has_schema_intent_object(object))
                {
                    *items = json!({"type": "string"});
                }
            }
            if let Some(kind) = schema_type.as_deref()
                && kind != "object"
                && !has_combiner(&Value::Object(result.clone()))
            {
                result.remove("properties");
                result.remove("required");
            }
            Value::Object(result)
        }
        value => value.clone(),
    }
}

fn stringify_schema_enum(value: &Value) -> Value {
    match value {
        Value::String(value) => Value::String(value.clone()),
        _ => Value::String(value.to_string()),
    }
}

fn project_schema(schema: &Value) -> Option<Value> {
    let object = schema.as_object()?;
    if object.get("type").and_then(Value::as_str) == Some("object")
        && !object.get("properties").is_some_and(Value::is_object)
        && !object.contains_key("additionalProperties")
    {
        return None;
    }
    let mut out = Map::new();
    copy_if_present(&mut out, object, "description");
    copy_if_present(&mut out, object, "required");
    copy_if_present(&mut out, object, "format");
    if let Some(value) = object.get("type") {
        if let Some(types) = value.as_array() {
            if let Some(kind) = types.iter().find(|item| item.as_str() != Some("null")) {
                out.insert("type".to_string(), kind.clone());
            }
            if types.iter().any(|item| item.as_str() == Some("null")) {
                out.insert("nullable".to_string(), Value::Bool(true));
            }
        } else {
            out.insert("type".to_string(), value.clone());
        }
    }
    if let Some(value) = object.get("const") {
        out.insert("enum".to_string(), Value::Array(vec![value.clone()]));
    } else {
        copy_if_present(&mut out, object, "enum");
    }
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        let projected = properties
            .iter()
            .filter_map(|(key, value)| project_schema(value).map(|value| (key.clone(), value)))
            .collect::<Map<_, _>>();
        out.insert("properties".to_string(), Value::Object(projected));
    }
    if let Some(items) = object.get("items") {
        let projected = if let Some(items) = items.as_array() {
            Value::Array(items.iter().filter_map(project_schema).collect())
        } else {
            project_schema(items).unwrap_or(Value::Null)
        };
        out.insert("items".to_string(), projected);
    }
    for key in ["allOf", "anyOf", "oneOf"] {
        if let Some(items) = object.get(key).and_then(Value::as_array) {
            out.insert(
                key.to_string(),
                Value::Array(items.iter().filter_map(project_schema).collect()),
            );
        }
    }
    copy_if_present(&mut out, object, "minLength");
    Some(Value::Object(out))
}

fn copy_if_present(out: &mut Map<String, Value>, object: &Map<String, Value>, key: &str) {
    if let Some(value) = object.get(key) {
        out.insert(key.to_string(), value.clone());
    }
}

fn has_combiner(schema: &Value) -> bool {
    schema.as_object().is_some_and(|object| {
        ["anyOf", "oneOf", "allOf"]
            .iter()
            .any(|key| object.get(*key).is_some_and(Value::is_array))
    })
}

fn has_schema_intent_object(object: &Map<String, Value>) -> bool {
    has_combiner(&Value::Object(object.clone()))
        || [
            "type",
            "properties",
            "items",
            "prefixItems",
            "enum",
            "const",
            "$ref",
            "additionalProperties",
            "patternProperties",
            "required",
            "not",
            "if",
            "then",
            "else",
        ]
        .iter()
        .any(|key| object.contains_key(*key))
}

#[cfg(test)]
#[path = "../test/protocols/gemini.rs"]
mod tests;
