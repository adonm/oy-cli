use anyhow::{Context, Result, anyhow, bail};
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_bedrockruntime::operation::converse::ConverseOutput;
use aws_sdk_bedrockruntime::types::{
    ContentBlock as BedrockContentBlock, ConversationRole, InferenceConfiguration, Message,
    ReasoningContentBlock, SystemContentBlock, TokenUsage, Tool as BedrockTool, ToolConfiguration,
    ToolInputSchema, ToolResultBlock, ToolResultContentBlock, ToolSpecification, ToolUseBlock,
};
use aws_smithy_types::Document;
use aws_types::region::Region;
use genai::adapter::AdapterKind;
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatResponse, ChatRole, ContentPart, MessageContent,
    StopReason, ToolCall, ToolResponse, Usage,
};
use genai::{ModelIden, ModelName};
use serde_json::{Value, json};
use std::env;
use std::process::{Command, Stdio};

const BEDROCK_NAMESPACE: &str = "bedrock";
const DEFAULT_REGION: &str = "ap-southeast-2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwsAuthAvailability {
    Present,
    AutoConfigured,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsAuthStatus {
    pub availability: AwsAuthAvailability,
    pub source: String,
    pub detail: String,
}

struct ConverseParts {
    system: Option<Vec<SystemContentBlock>>,
    messages: Vec<Message>,
    inference_config: Option<InferenceConfiguration>,
    tool_config: Option<ToolConfiguration>,
}

pub fn is_bedrock_model(model_spec: &str) -> bool {
    crate::config::split_model_spec(model_spec).0 == Some(BEDROCK_NAMESPACE)
}

pub fn region() -> String {
    env_value("BEDROCK_REGION")
        .or_else(|| env_value("AWS_REGION"))
        .or_else(|| env_value("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

pub fn auth_status() -> AwsAuthStatus {
    if env_credentials_present() {
        return AwsAuthStatus {
            availability: AwsAuthAvailability::Present,
            source: "env".to_string(),
            detail: format!(
                "AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY detected for Bedrock in {}.",
                region()
            ),
        };
    }

    if aws_cli_available() {
        let profile = aws_profile();
        let sso = profile.as_deref().is_some_and(profile_looks_like_sso);
        return AwsAuthStatus {
            availability: AwsAuthAvailability::AutoConfigured,
            source: "aws-sdk/aws-cli".to_string(),
            detail: match (profile.as_deref(), sso) {
                (Some(profile), true) => format!(
                    "AWS profile `{profile}` appears to use SSO; oy will run `aws sso login --profile {profile}` if SDK credential loading reports expired credentials."
                ),
                (Some(profile), false) => format!(
                    "AWS SDK profile `{profile}` available for Bedrock in {}; SSO login will be attempted if the SDK reports it is needed.",
                    region()
                ),
                (None, _) => format!(
                    "AWS SDK default credential chain available for Bedrock in {}; SSO login will be attempted if the SDK reports it is needed.",
                    region()
                ),
            },
        };
    }

    AwsAuthStatus {
        availability: AwsAuthAvailability::Missing,
        source: "missing".to_string(),
        detail: "No AWS env credentials or AWS CLI detected for Bedrock.".to_string(),
    }
}

pub async fn exec_chat(
    model_spec: &str,
    req: ChatRequest,
    options: Option<&ChatOptions>,
) -> Result<ChatResponse> {
    let (_, model_id) = crate::config::split_model_spec(model_spec);
    if model_id.trim().is_empty() {
        bail!("Bedrock model id is empty; use `bedrock::<model-id>`");
    }

    let parts = converse_parts(req, options)?;
    let client = bedrock_client().await?;
    match send_converse(&client, model_id, &parts).await {
        Ok(response) => chat_response_from_bedrock(model_id, response),
        Err(first_err) if should_try_sso_login(&first_err, aws_profile().as_deref()) => {
            let profile = aws_profile();
            run_sso_login(profile.as_deref())?;
            let client = bedrock_client().await?;
            let response = send_converse(&client, model_id, &parts)
                .await
                .with_context(|| "AWS SSO login completed, but Bedrock Converse still failed")?;
            chat_response_from_bedrock(model_id, response)
        }
        Err(err) => Err(err),
    }
}

async fn bedrock_client() -> Result<BedrockClient> {
    let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(Region::new(region()))
        .load()
        .await;
    let mut config = aws_sdk_bedrockruntime::config::Builder::from(&sdk_config);
    if let Some(endpoint) = env_value("BEDROCK_RUNTIME_ENDPOINT") {
        config.set_endpoint_url(Some(endpoint));
    }
    Ok(BedrockClient::from_conf(config.build()))
}

async fn send_converse(
    client: &BedrockClient,
    model_id: &str,
    parts: &ConverseParts,
) -> Result<ConverseOutput> {
    client
        .converse()
        .model_id(model_id)
        .set_system(parts.system.clone())
        .set_messages(Some(parts.messages.clone()))
        .set_inference_config(parts.inference_config.clone())
        .set_tool_config(parts.tool_config.clone())
        .send()
        .await
        .map_err(|err| anyhow!("Bedrock Converse request failed: {err}"))
}

fn converse_parts(req: ChatRequest, options: Option<&ChatOptions>) -> Result<ConverseParts> {
    let system = req
        .join_systems()
        .filter(|value| !value.trim().is_empty())
        .map(|value| vec![SystemContentBlock::Text(value)]);
    let tools = req.tools.clone().filter(|tools| !tools.is_empty());

    Ok(ConverseParts {
        system,
        messages: messages_to_bedrock(req.messages)?,
        inference_config: inference_config(options),
        tool_config: tools.map(tools_to_bedrock).transpose()?,
    })
}

fn inference_config(options: Option<&ChatOptions>) -> Option<InferenceConfiguration> {
    let options = options?;
    if options.max_tokens.is_none()
        && options.temperature.is_none()
        && options.top_p.is_none()
        && options.stop_sequences.is_empty()
    {
        return None;
    }

    let mut builder = InferenceConfiguration::builder();
    if let Some(max_tokens) = options.max_tokens {
        builder = builder.max_tokens(max_tokens as i32);
    }
    if let Some(temperature) = options.temperature {
        builder = builder.temperature(temperature as f32);
    }
    if let Some(top_p) = options.top_p {
        builder = builder.top_p(top_p as f32);
    }
    if !options.stop_sequences.is_empty() {
        builder = builder.set_stop_sequences(Some(options.stop_sequences.clone()));
    }
    Some(builder.build())
}

fn tools_to_bedrock(tools: Vec<genai::chat::Tool>) -> Result<ToolConfiguration> {
    let tools = tools
        .into_iter()
        .map(|tool| {
            let schema = tool.schema.unwrap_or_else(|| json!({ "type": "object" }));
            let spec = ToolSpecification::builder()
                .name(tool.name)
                .set_description(tool.description)
                .input_schema(ToolInputSchema::Json(value_to_document(schema)?))
                .build()?;
            Ok(BedrockTool::ToolSpec(spec))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ToolConfiguration::builder()
        .set_tools(Some(tools))
        .build()?)
}

fn messages_to_bedrock(messages: Vec<ChatMessage>) -> Result<Vec<Message>> {
    let mut out = Vec::new();
    for message in messages {
        match message.role {
            ChatRole::System => {
                if let Some(text) = message.content.joined_texts() {
                    out.push(message_to_bedrock(
                        ConversationRole::User,
                        vec![BedrockContentBlock::Text(text)],
                    )?);
                }
            }
            ChatRole::User => out.push(message_to_bedrock(
                ConversationRole::User,
                content_to_bedrock(&message.content)?,
            )?),
            ChatRole::Assistant => out.push(message_to_bedrock(
                ConversationRole::Assistant,
                content_to_bedrock(&message.content)?,
            )?),
            ChatRole::Tool => out.push(message_to_bedrock(
                ConversationRole::User,
                tool_results_to_bedrock(&message.content)?,
            )?),
        }
    }
    if out.is_empty() {
        bail!("Bedrock request has no messages");
    }
    Ok(out)
}

fn message_to_bedrock(
    role: ConversationRole,
    content: Vec<BedrockContentBlock>,
) -> Result<Message> {
    Ok(Message::builder()
        .role(role)
        .set_content(Some(content))
        .build()?)
}

fn content_to_bedrock(content: &MessageContent) -> Result<Vec<BedrockContentBlock>> {
    let mut out = Vec::new();
    for part in content {
        match part {
            ContentPart::Text(text) => out.push(BedrockContentBlock::Text(text.clone())),
            ContentPart::ToolCall(call) => out.push(BedrockContentBlock::ToolUse(
                ToolUseBlock::builder()
                    .tool_use_id(call.call_id.clone())
                    .name(call.fn_name.clone())
                    .input(value_to_document(call.fn_arguments.clone())?)
                    .build()?,
            )),
            ContentPart::ToolResponse(response) => {
                out.push(BedrockContentBlock::ToolResult(tool_result_to_bedrock(
                    response,
                )?));
            }
            ContentPart::ThoughtSignature(_)
            | ContentPart::ReasoningContent(_)
            | ContentPart::Custom(_) => {}
            ContentPart::Binary(_) => {
                bail!("Bedrock Converse binary inputs are not supported by oy yet")
            }
        }
    }
    if out.is_empty() {
        out.push(BedrockContentBlock::Text(String::new()));
    }
    Ok(out)
}

fn tool_results_to_bedrock(content: &MessageContent) -> Result<Vec<BedrockContentBlock>> {
    let mut out = Vec::new();
    for part in content {
        match part {
            ContentPart::ToolResponse(response) => {
                out.push(BedrockContentBlock::ToolResult(tool_result_to_bedrock(
                    response,
                )?));
            }
            ContentPart::Text(text) => out.push(BedrockContentBlock::Text(text.clone())),
            ContentPart::ThoughtSignature(_)
            | ContentPart::ReasoningContent(_)
            | ContentPart::Custom(_) => {}
            ContentPart::ToolCall(_) => {
                bail!("tool calls are not valid inside Bedrock tool-result messages")
            }
            ContentPart::Binary(_) => {
                bail!("Bedrock Converse binary tool results are not supported by oy yet")
            }
        }
    }
    if out.is_empty() {
        bail!("Bedrock tool-result message has no content");
    }
    Ok(out)
}

fn tool_result_to_bedrock(response: &ToolResponse) -> Result<ToolResultBlock> {
    Ok(ToolResultBlock::builder()
        .tool_use_id(response.call_id.clone())
        .content(ToolResultContentBlock::Text(response.content.clone()))
        .build()?)
}

fn chat_response_from_bedrock(model_id: &str, response: ConverseOutput) -> Result<ChatResponse> {
    let mut parts = Vec::new();
    let mut reasoning = Vec::new();

    if let Some(aws_sdk_bedrockruntime::types::ConverseOutput::Message(message)) = response.output()
    {
        for item in message.content() {
            match item {
                BedrockContentBlock::Text(text) => parts.push(ContentPart::Text(text.clone())),
                BedrockContentBlock::ReasoningContent(ReasoningContentBlock::ReasoningText(
                    text,
                )) => reasoning.push(text.text().to_string()),
                BedrockContentBlock::ToolUse(tool) => parts.push(ContentPart::ToolCall(ToolCall {
                    call_id: tool.tool_use_id().to_string(),
                    fn_name: tool.name().to_string(),
                    fn_arguments: document_to_value(tool.input())?,
                    thought_signatures: None,
                })),
                _ => {}
            }
        }
    }

    let model_iden = ModelIden::new(
        AdapterKind::Anthropic,
        ModelName::from(model_id.to_string()),
    );
    Ok(ChatResponse {
        content: MessageContent::from_parts(parts),
        reasoning_content: (!reasoning.is_empty()).then(|| reasoning.join("\n")),
        model_iden: model_iden.clone(),
        provider_model_iden: model_iden,
        stop_reason: Some(StopReason::from(
            response.stop_reason().as_str().to_string(),
        )),
        usage: usage_from_bedrock(response.usage()),
        captured_raw_body: None,
        response_id: None,
    })
}

fn usage_from_bedrock(usage: Option<&TokenUsage>) -> Usage {
    Usage {
        prompt_tokens: usage.map(|usage| usage.input_tokens),
        prompt_tokens_details: None,
        completion_tokens: usage.map(|usage| usage.output_tokens),
        completion_tokens_details: None,
        total_tokens: usage.map(|usage| usage.total_tokens),
    }
}

fn value_to_document(value: Value) -> Result<Document> {
    let bytes = serde_json::to_vec(&value)?;
    let mut tokens = aws_smithy_json::deserialize::json_token_iter(&bytes).peekable();
    aws_smithy_json::deserialize::token::expect_document(&mut tokens)
        .map_err(|err| anyhow!("failed to convert JSON value to AWS document: {err}"))
}

fn document_to_value(document: &Document) -> Result<Value> {
    let mut json = String::new();
    aws_smithy_json::serialize::JsonValueWriter::new(&mut json).document(document);
    serde_json::from_str(&json).context("failed to convert AWS document to JSON value")
}
fn env_credentials_present() -> bool {
    env_value("AWS_ACCESS_KEY_ID").is_some() && env_value("AWS_SECRET_ACCESS_KEY").is_some()
}

fn run_sso_login(profile: Option<&str>) -> Result<()> {
    if !crate::config::can_prompt() {
        bail!(
            "AWS SSO credentials are expired or missing, but this run cannot prompt. Run `aws sso login{}` first.",
            profile
                .map(|p| format!(" --profile {p}"))
                .unwrap_or_default()
        );
    }
    let mut cmd = Command::new("aws");
    cmd.arg("sso").arg("login");
    if let Some(profile) = profile.filter(|profile| !profile.trim().is_empty()) {
        cmd.arg("--profile").arg(profile);
    }
    let status = cmd.status().context("failed to run `aws sso login`")?;
    if !status.success() {
        bail!("`aws sso login` failed with status {status}");
    }
    Ok(())
}

fn should_try_sso_login(err: &anyhow::Error, profile: Option<&str>) -> bool {
    let text = format!("{err:#}").to_ascii_lowercase();
    text.contains("sso")
        || text.contains("aws sso login")
        || text.contains("token has expired")
        || text.contains("token is expired")
        || profile.is_some_and(profile_looks_like_sso)
}

fn aws_profile() -> Option<String> {
    env_value("AWS_PROFILE")
        .or_else(|| env_value("AWS_DEFAULT_PROFILE"))
        .filter(|profile| profile != "default")
}

fn aws_cli_available() -> bool {
    Command::new("aws")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn profile_looks_like_sso(profile: &str) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let config = home.join(".aws").join("config");
    let Ok(text) = std::fs::read_to_string(config) else {
        return false;
    };
    let headers = [
        format!("[profile {profile}]"),
        format!("[sso-session {profile}]"),
        if profile == "default" {
            "[default]".to_string()
        } else {
            String::new()
        },
    ];
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = headers
                .iter()
                .any(|header| !header.is_empty() && trimmed == header);
            continue;
        }
        if in_section && (trimmed.starts_with("sso_") || trimmed.starts_with("sso_session")) {
            return true;
        }
    }
    false
}

fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_document_round_trips_tool_arguments() -> Result<()> {
        let value = json!({
            "text": "hello",
            "array": [true, null, 42, -7, 1.5],
            "nested": { "ok": true }
        });
        assert_eq!(
            document_to_value(&value_to_document(value.clone())?)?,
            value
        );
        Ok(())
    }

    #[test]
    fn detects_sso_login_errors() {
        let err = anyhow!("SSO token has expired; run aws sso login");
        assert!(should_try_sso_login(&err, None));
        let err = anyhow!("access denied");
        assert!(!should_try_sso_login(&err, None));
    }

    #[test]
    fn detects_native_bedrock_namespace_only() {
        assert!(is_bedrock_model(
            "bedrock::anthropic.claude-sonnet-4-5-20250929-v1:0"
        ));
        assert!(!is_bedrock_model("bedrock-mantle::openai.gpt-oss-120b"));
    }
}
