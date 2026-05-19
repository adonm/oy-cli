use super::{
    ChatBackend, ChatFuture, LlmRequest, LlmResponse, LlmTool, LlmTools, Message, MessageContent,
    Protocol, RouteAuth, ToolResultContent, ToolSpec,
};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::StatusCode;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct NativeOpenAiBackend;

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeToolCall {
    id: String,
    call_id: String,
    name: String,
    arguments: String,
}

impl NativeToolCall {
    fn arguments_value(&self) -> Result<Value> {
        serde_json::from_str(&self.arguments)
            .with_context(|| format!("tool `{}` supplied invalid JSON arguments", self.name))
    }
}

impl ChatBackend for NativeOpenAiBackend {
    type Tools = LlmTools;

    fn chat<'a>(&'a self, request: LlmRequest, tools: Self::Tools) -> ChatFuture<'a> {
        Box::pin(async move { execute_native_chat(request, tools).await })
    }
}

async fn execute_native_chat(request: LlmRequest, tools: LlmTools) -> Result<LlmResponse> {
    match request.route.protocol {
        Protocol::OpenAiChat => run_chat_completions(request, tools).await,
        Protocol::OpenAiResponses => run_responses(request, tools).await,
    }
}

async fn run_chat_completions(request: LlmRequest, tools: LlmTools) -> Result<LlmResponse> {
    let api_key = api_key(&request.route.auth);
    let endpoint = endpoint_url(request.route.base_url.as_deref(), "chat/completions")?;
    let client = reqwest::Client::new();
    let tool_specs = request.tools.clone();
    let tools_by_name = tools_by_name(tools);
    let mut messages = chat_messages_from_llm(&request.system_prompt, request.messages)?;
    let mut transcript = Vec::new();

    for turn in 0..=request.max_turns {
        let body = chat_request_body(
            &request.route.model,
            &messages,
            &tool_specs,
            request.route.additional_params.as_ref(),
        )?;
        let value = post_json(&client, &endpoint, api_key, &body).await?;
        let assistant = parse_chat_assistant(&value)?;
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
        if turn >= request.max_turns {
            bail!("native OpenAI chat exceeded the tool round budget");
        }

        messages.push(chat_assistant_wire_message(
            &assistant.text,
            assistant.reasoning_content.as_ref(),
            &assistant.tool_calls,
        )?);
        transcript.push(assistant_message);
        for call in assistant.tool_calls {
            let output = call_tool(&tools_by_name, &call).await;
            let result = tool_result_message(&call, output.clone());
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.call_id,
                "content": output,
            }));
            transcript.push(result);
        }
    }

    unreachable!("bounded tool loop exits from inside the loop")
}

async fn run_responses(request: LlmRequest, tools: LlmTools) -> Result<LlmResponse> {
    let api_key = api_key(&request.route.auth);
    let endpoint = endpoint_url(request.route.base_url.as_deref(), "responses")?;
    let client = reqwest::Client::new();
    let tool_specs = request.tools.clone();
    let tools_by_name = tools_by_name(tools);
    let mut input = responses_input_from_llm(request.messages)?;
    let mut transcript = Vec::new();

    for turn in 0..=request.max_turns {
        let body = responses_request_body(
            &request.route.model,
            &request.system_prompt,
            &input,
            &tool_specs,
            request.route.additional_params.as_ref(),
        )?;
        let value = post_json(&client, &endpoint, api_key, &body).await?;
        let response = parse_responses_output(&value)?;
        let assistant_message =
            assistant_message_from_calls(&response.text, None, &response.tool_calls)?;

        if response.tool_calls.is_empty() {
            transcript.push(assistant_message);
            return Ok(LlmResponse {
                output: response.text,
                messages: Some(transcript),
            });
        }
        if turn >= request.max_turns {
            bail!("native OpenAI Responses chat exceeded the tool round budget");
        }

        append_responses_assistant_output(&mut input, &response.text, &response.tool_calls);
        transcript.push(assistant_message);
        for call in response.tool_calls {
            let output = call_tool(&tools_by_name, &call).await;
            transcript.push(tool_result_message(&call, output.clone()));
            input.push(json!({
                "type": "function_call_output",
                "call_id": call.call_id,
                "output": output,
            }));
        }
    }

    unreachable!("bounded tool loop exits from inside the loop")
}

fn api_key(auth: &RouteAuth) -> &str {
    match auth {
        RouteAuth::ApiKey(api_key) => api_key,
    }
}

fn endpoint_url(base_url: Option<&str>, path: &str) -> Result<String> {
    let base_url = base_url
        .unwrap_or(OPENAI_BASE_URL)
        .trim()
        .trim_end_matches('/');
    if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
        bail!("native OpenAI base URL must be http or https");
    }
    Ok(format!("{base_url}/{}", path.trim_start_matches('/')))
}

async fn post_json(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    body: &Value,
) -> Result<Value> {
    let response = client
        .post(endpoint)
        .bearer_auth(api_key)
        .json(body)
        .send()
        .await
        .with_context(|| format!("failed to send native OpenAI request to {endpoint}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .context("failed to read native OpenAI response body")?;
    if !status.is_success() {
        bail!(
            "native OpenAI request failed ({}): {}",
            status,
            provider_error_message(status, &text)
        );
    }
    serde_json::from_str(&text).context("failed to parse native OpenAI response JSON")
}

fn provider_error_message(status: StatusCode, text: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(text)
        && let Some(message) = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty())
    {
        return message.to_string();
    }
    let text = text.trim();
    if text.is_empty() {
        status
            .canonical_reason()
            .unwrap_or("empty provider error")
            .to_string()
    } else {
        text.chars().take(500).collect()
    }
}

type ToolMap = HashMap<String, Box<dyn LlmTool>>;

fn tools_by_name(tools: LlmTools) -> ToolMap {
    tools
        .into_iter()
        .map(|tool| (tool.name().to_string(), tool))
        .collect()
}

async fn call_tool(tools: &ToolMap, call: &NativeToolCall) -> String {
    let result = async {
        let tool = tools
            .get(&call.name)
            .ok_or_else(|| anyhow!("model requested unknown tool `{}`", call.name))?;
        tool.call(call.arguments.clone())
            .await
            .map_err(|err| anyhow!("tool `{}` failed: {err}", call.name))
    }
    .await;

    match result {
        Ok(output) => output,
        Err(err) => tool_failure_output(&call.name, &err),
    }
}

fn tool_failure_output(name: &str, err: &anyhow::Error) -> String {
    format!(
        "tool `{name}` failed: {err}\nDo not retry the same tool call unchanged. Fix the arguments, choose another tool, or report this blocker."
    )
}

fn chat_request_body(
    model: &str,
    messages: &[Value],
    tools: &[ToolSpec],
    additional_params: Option<&Value>,
) -> Result<Value> {
    let mut body = Map::from_iter([
        ("model".to_string(), json!(model)),
        ("messages".to_string(), Value::Array(messages.to_vec())),
    ]);
    if !tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(tools.iter().map(chat_tool_spec).collect()),
        );
    }
    merge_additional_params(&mut body, additional_params)?;
    Ok(Value::Object(body))
}

fn responses_request_body(
    model: &str,
    instructions: &str,
    input: &[Value],
    tools: &[ToolSpec],
    additional_params: Option<&Value>,
) -> Result<Value> {
    let mut body = Map::from_iter([
        ("model".to_string(), json!(model)),
        ("input".to_string(), Value::Array(input.to_vec())),
    ]);
    if !instructions.trim().is_empty() {
        body.insert("instructions".to_string(), json!(instructions));
    }
    if !tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(tools.iter().map(responses_tool_spec).collect()),
        );
    }
    merge_additional_params(&mut body, additional_params)?;
    Ok(Value::Object(body))
}

fn merge_additional_params(
    body: &mut Map<String, Value>,
    additional_params: Option<&Value>,
) -> Result<()> {
    let Some(additional_params) = additional_params else {
        return Ok(());
    };
    let Value::Object(extra) = additional_params else {
        bail!("native OpenAI additional route params must be a JSON object");
    };
    for (key, value) in extra {
        if body.contains_key(key) {
            bail!("native OpenAI additional route param `{key}` conflicts with the request body");
        }
        body.insert(key.clone(), value.clone());
    }
    Ok(())
}

fn chat_tool_spec(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": spec.parameters,
        }
    })
}

fn responses_tool_spec(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "name": spec.name,
        "description": spec.description,
        "parameters": spec.parameters,
    })
}

fn chat_messages_from_llm(system_prompt: &str, messages: Vec<Message>) -> Result<Vec<Value>> {
    let mut wire = Vec::new();
    if !system_prompt.trim().is_empty() {
        wire.push(json!({"role": "system", "content": system_prompt}));
    }
    for message in messages {
        match message {
            Message::System { content } => wire.push(json!({"role": "system", "content": content})),
            Message::User { content } => append_chat_user_content(&mut wire, content)?,
            Message::Assistant { content, .. } => {
                let assistant = assistant_parts(content)?;
                wire.push(chat_assistant_wire_message(
                    &assistant.text,
                    assistant.reasoning_content.as_ref(),
                    &assistant.tool_calls,
                )?);
            }
        }
    }
    Ok(wire)
}

fn append_chat_user_content(wire: &mut Vec<Value>, content: Vec<MessageContent>) -> Result<()> {
    let mut text = Vec::new();
    for item in content {
        match item {
            MessageContent::Text { text: value } => text.push(value),
            MessageContent::ToolResult {
                id,
                call_id,
                content,
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
            MessageContent::Opaque { value } => {
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

fn responses_input_from_llm(messages: Vec<Message>) -> Result<Vec<Value>> {
    let mut input = Vec::new();
    for message in messages {
        match message {
            Message::System { content } => {
                input.push(response_message("system", "input_text", content))
            }
            Message::User { content } => append_responses_user_content(&mut input, content)?,
            Message::Assistant { content, .. } => {
                append_responses_assistant_content(&mut input, content)?
            }
        }
    }
    Ok(input)
}

fn append_responses_user_content(
    input: &mut Vec<Value>,
    content: Vec<MessageContent>,
) -> Result<()> {
    let mut text = Vec::new();
    for item in content {
        match item {
            MessageContent::Text { text: value } => text.push(value),
            MessageContent::ToolResult {
                id,
                call_id,
                content,
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
            MessageContent::Opaque { value } => text.push(serde_json::to_string(&value)?),
            MessageContent::ToolCall { .. } => bail!("user message cannot contain a tool call"),
            MessageContent::Reasoning { .. } => bail!("user message cannot contain reasoning"),
        }
    }
    if !text.is_empty() {
        input.push(response_message("user", "input_text", text.join("\n")));
    }
    Ok(())
}

fn append_responses_assistant_content(
    input: &mut Vec<Value>,
    content: Vec<MessageContent>,
) -> Result<()> {
    let assistant = assistant_parts(content)?;
    append_responses_assistant_output(input, &assistant.text, &assistant.tool_calls);
    Ok(())
}

fn append_responses_assistant_output(
    input: &mut Vec<Value>,
    text: &str,
    tool_calls: &[NativeToolCall],
) {
    if !text.is_empty() {
        input.push(response_message(
            "assistant",
            "output_text",
            text.to_string(),
        ));
    }
    for call in tool_calls {
        input.push(responses_function_call(call));
    }
}

fn responses_function_call(call: &NativeToolCall) -> Value {
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

#[derive(Debug, Clone)]
struct NativeAssistantContent {
    text: String,
    reasoning_content: Option<Value>,
    tool_calls: Vec<NativeToolCall>,
}

fn assistant_parts(content: Vec<MessageContent>) -> Result<NativeAssistantContent> {
    let mut text = Vec::new();
    let mut reasoning_content = None;
    let mut tool_calls = Vec::new();
    for item in content {
        match item {
            MessageContent::Text { text: value } => text.push(value),
            MessageContent::ToolCall {
                id,
                call_id,
                name,
                arguments,
                ..
            } => {
                let arguments = serde_json::to_string(&arguments)?;
                tool_calls.push(NativeToolCall {
                    call_id: call_id.unwrap_or_else(|| id.clone()),
                    id,
                    name,
                    arguments,
                });
            }
            MessageContent::Reasoning { value } => {
                reasoning_content.get_or_insert(value);
            }
            MessageContent::Opaque { value } => text.push(serde_json::to_string(&value)?),
            MessageContent::ToolResult { .. } => {
                bail!("assistant message cannot contain a tool result")
            }
        }
    }
    Ok(NativeAssistantContent {
        text: text.join("\n"),
        reasoning_content,
        tool_calls,
    })
}

fn chat_assistant_wire_message(
    text: &str,
    reasoning_content: Option<&Value>,
    tool_calls: &[NativeToolCall],
) -> Result<Value> {
    let mut message = Map::from_iter([("role".to_string(), json!("assistant"))]);
    if !text.is_empty() || tool_calls.is_empty() {
        message.insert("content".to_string(), json!(text));
    } else {
        message.insert("content".to_string(), Value::Null);
    }
    if let Some(reasoning_content) = reasoning_content {
        message.insert("reasoning_content".to_string(), reasoning_content.clone());
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
        }],
    }
}

fn tool_result_text(content: Vec<ToolResultContent>) -> Result<String> {
    content
        .into_iter()
        .map(|item| match item {
            ToolResultContent::Text { text } => Ok(text),
            ToolResultContent::Opaque { value } => {
                serde_json::to_string(&value).map_err(Into::into)
            }
        })
        .collect::<Result<Vec<_>>>()
        .map(|items| items.join("\n"))
}

#[derive(Debug, Clone)]
struct ParsedAssistant {
    text: String,
    reasoning_content: Option<Value>,
    tool_calls: Vec<NativeToolCall>,
}

fn parse_chat_assistant(value: &Value) -> Result<ParsedAssistant> {
    let message = value
        .pointer("/choices/0/message")
        .ok_or_else(|| anyhow!("native OpenAI chat response did not include a message"))?;
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let reasoning_content = message
        .get("reasoning_content")
        .filter(|value| !value.is_null())
        .cloned();
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(chat_tool_call)
        .collect::<Result<Vec<_>>>()?;
    Ok(ParsedAssistant {
        text,
        reasoning_content,
        tool_calls,
    })
}

fn chat_tool_call(value: &Value) -> Result<NativeToolCall> {
    let id = required_string(value, "id", "chat tool call")?;
    let function = value
        .get("function")
        .ok_or_else(|| anyhow!("chat tool call `{id}` did not include a function"))?;
    Ok(NativeToolCall {
        id: id.clone(),
        call_id: id,
        name: required_string(function, "name", "chat tool call function")?,
        arguments: string_or_json(function.get("arguments")).unwrap_or_else(|| "{}".to_string()),
    })
}

#[derive(Debug, Clone)]
struct ParsedResponse {
    text: String,
    tool_calls: Vec<NativeToolCall>,
}

fn parse_responses_output(value: &Value) -> Result<ParsedResponse> {
    let mut text = value
        .get("output_text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut tool_calls = Vec::new();

    for item in value
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("message") if text.is_empty() => {
                text = response_message_text(item)?;
            }
            Some("function_call") => tool_calls.push(response_tool_call(item)?),
            _ => {}
        }
    }

    Ok(ParsedResponse { text, tool_calls })
}

fn response_message_text(item: &Value) -> Result<String> {
    item.get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|content| {
            matches!(
                content.get("type").and_then(Value::as_str),
                Some("output_text") | Some("text")
            )
        })
        .map(|content| {
            content
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| anyhow!("Responses message text item did not include text"))
        })
        .collect::<Result<Vec<_>>>()
        .map(|items| items.join("\n"))
}

fn response_tool_call(item: &Value) -> Result<NativeToolCall> {
    let id = required_string(item, "id", "Responses function call")?;
    Ok(NativeToolCall {
        id: id.clone(),
        call_id: item
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_string(),
        name: required_string(item, "name", "Responses function call")?,
        arguments: string_or_json(item.get("arguments")).unwrap_or_else(|| "{}".to_string()),
    })
}

fn required_string(value: &Value, key: &str, context: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{context} did not include `{key}`"))
}

fn string_or_json(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.clone()),
        value => Some(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn read_tool_spec() -> ToolSpec {
        ToolSpec {
            name: "read".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        }
    }

    struct FailingTool;

    impl LlmTool for FailingTool {
        fn name(&self) -> &str {
            "fail"
        }

        fn call<'a>(&'a self, _args: String) -> crate::llm::LlmToolFuture<'a> {
            Box::pin(async move { Err(anyhow!("boom")) })
        }
    }

    #[tokio::test]
    async fn tool_call_failure_is_returned_to_model_as_tool_output() {
        let tools: ToolMap = HashMap::from([(
            "fail".to_string(),
            Box::new(FailingTool) as Box<dyn LlmTool>,
        )]);
        let call = NativeToolCall {
            id: "call-1".to_string(),
            call_id: "call-1".to_string(),
            name: "fail".to_string(),
            arguments: "{}".to_string(),
        };

        let output = call_tool(&tools, &call).await;

        assert!(output.contains("tool `fail` failed: boom"));
        assert!(output.contains("Do not retry the same tool call unchanged"));
    }

    #[tokio::test]
    async fn unknown_tool_is_returned_to_model_as_tool_output() {
        let tools: ToolMap = HashMap::new();
        let call = NativeToolCall {
            id: "call-1".to_string(),
            call_id: "call-1".to_string(),
            name: "missing".to_string(),
            arguments: "{}".to_string(),
        };

        let output = call_tool(&tools, &call).await;

        assert!(output.contains("model requested unknown tool `missing`"));
        assert!(output.contains("Fix the arguments"));
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
        let messages = chat_messages_from_llm(
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
                    }],
                },
            ],
        )
        .unwrap();
        let body = chat_request_body(
            "gpt-4.1-mini",
            &messages,
            &[read_tool_spec()],
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
    fn chat_round_trips_deepseek_reasoning_content_for_tool_calls() {
        let parsed = parse_chat_assistant(&json!({
            "choices": [{
                "message": {
                    "content": null,
                    "reasoning_content": "thinking before tool call",
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": "read", "arguments": "{\"path\":\"README.md\"}"}
                    }]
                }
            }]
        }))
        .unwrap();
        assert_eq!(
            parsed.reasoning_content.as_ref(),
            Some(&json!("thinking before tool call"))
        );

        let transcript = assistant_message_from_calls(
            &parsed.text,
            parsed.reasoning_content.as_ref(),
            &parsed.tool_calls,
        )
        .unwrap();
        let Message::Assistant { content, .. } = transcript else {
            panic!("expected assistant transcript message");
        };
        assert!(matches!(
            &content[0],
            MessageContent::Reasoning { value } if value == &json!("thinking before tool call")
        ));

        let wire = chat_assistant_wire_message(
            &parsed.text,
            parsed.reasoning_content.as_ref(),
            &parsed.tool_calls,
        )
        .unwrap();
        assert_eq!(
            wire["reasoning_content"],
            json!("thinking before tool call")
        );
        assert_eq!(wire["tool_calls"][0]["id"], json!("call-1"));
    }

    #[test]
    fn responses_request_serializes_function_call_output_golden() {
        let input = responses_input_from_llm(vec![
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
                }],
            },
        ])
        .unwrap();
        let body = responses_request_body(
            "gpt-5.1",
            "system",
            &input,
            &[read_tool_spec()],
            Some(&json!({"reasoning": {"effort": "low"}})),
        )
        .unwrap();

        let actual = body;
        let expected = r#"{
  "input": [
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
  "instructions": "system",
  "model": "gpt-5.1",
  "reasoning": {
    "effort": "low"
  },
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
    fn parses_chat_and_responses_tool_calls() {
        let chat = parse_chat_assistant(&json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": "read", "arguments": "{\"path\":\"README.md\"}"}
                    }]
                }
            }]
        }))
        .unwrap();
        assert_eq!(chat.text, "");
        assert_eq!(chat.tool_calls[0].call_id, "call-1");
        assert_eq!(
            chat.tool_calls[0].arguments_value().unwrap(),
            json!({"path": "README.md"})
        );

        let responses = parse_responses_output(&json!({
            "id": "resp_1",
            "output": [
                {"type": "function_call", "id": "fc_1", "call_id": "call-2", "name": "read", "arguments": "{\"path\":\"Cargo.toml\"}"}
            ]
        }))
        .unwrap();
        assert_eq!(responses.tool_calls[0].call_id, "call-2");
    }

    #[test]
    fn rejects_invalid_native_base_url() {
        let err = endpoint_url(Some("file:///tmp/socket"), "responses").unwrap_err();
        assert!(err.to_string().contains("must be http or https"));
    }
}
