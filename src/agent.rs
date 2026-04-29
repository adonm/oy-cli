// === bedrock ===
pub(crate) mod bedrock {
    use anyhow::{Context, Result, anyhow, bail};
    use aws_sdk_bedrockruntime::Client as BedrockClient;
    use aws_sdk_bedrockruntime::operation::converse::ConverseOutput;
    use aws_sdk_bedrockruntime::types::{
        ContentBlock as BedrockContentBlock, ConversationRole, InferenceConfiguration, Message,
        ReasoningContentBlock, SystemContentBlock, TokenUsage, Tool as BedrockTool,
        ToolConfiguration, ToolInputSchema, ToolResultBlock, ToolResultContentBlock,
        ToolSpecification, ToolUseBlock,
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

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct AwsAuthStatus {
        pub present: bool,
        pub source: String,
        pub detail: String,
        pub auto_configured: bool,
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
                present: true,
                source: "env".to_string(),
                detail: format!(
                    "AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY detected for Bedrock in {}.",
                    region()
                ),
                auto_configured: false,
            };
        }

        if aws_cli_available() {
            let profile = aws_profile();
            let sso = profile.as_deref().is_some_and(profile_looks_like_sso);
            return AwsAuthStatus {
                present: true,
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
                auto_configured: true,
            };
        }

        AwsAuthStatus {
            present: false,
            source: "missing".to_string(),
            detail: "No AWS env credentials or AWS CLI detected for Bedrock.".to_string(),
            auto_configured: false,
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
                    .with_context(
                        || "AWS SSO login completed, but Bedrock Converse still failed",
                    )?;
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

    fn chat_response_from_bedrock(
        model_id: &str,
        response: ConverseOutput,
    ) -> Result<ChatResponse> {
        let mut parts = Vec::new();
        let mut reasoning = Vec::new();

        if let Some(aws_sdk_bedrockruntime::types::ConverseOutput::Message(message)) =
            response.output()
        {
            for item in message.content() {
                match item {
                    BedrockContentBlock::Text(text) => parts.push(ContentPart::Text(text.clone())),
                    BedrockContentBlock::ReasoningContent(
                        ReasoningContentBlock::ReasoningText(text),
                    ) => reasoning.push(text.text().to_string()),
                    BedrockContentBlock::ToolUse(tool) => {
                        parts.push(ContentPart::ToolCall(ToolCall {
                            call_id: tool.tool_use_id().to_string(),
                            fn_name: tool.name().to_string(),
                            fn_arguments: document_to_value(tool.input())?,
                            thought_signatures: None,
                        }))
                    }
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
}

// === model ===
pub(crate) mod model {
    use crate::config;
    use anyhow::{Result, anyhow, bail};
    use genai::adapter::AdapterKind;
    use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
    use genai::{Client, ModelIden, ServiceTarget};
    use serde::Serialize;
    use serde_json::Value;
    use std::collections::BTreeSet;
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    #[derive(Debug, Clone, Serialize)]
    pub struct AuthStatus {
        pub adapter: String,
        pub env_var: Option<String>,
        pub present: bool,
        pub source: String,
        pub detail: String,
        pub auto_configured: bool,
    }

    #[derive(Debug, Clone, Serialize)]
    pub struct ModelListing {
        pub current: Option<String>,
        pub current_shim: Option<String>,
        pub auth: Vec<AuthStatus>,
        pub recommended: Vec<String>,
        pub dynamic: Vec<AdapterModels>,
        pub hints: Vec<String>,
        pub all_models: Vec<String>,
    }

    #[derive(Debug, Clone, Serialize)]
    pub struct AdapterModels {
        pub adapter: String,
        pub ok: bool,
        pub source: String,
        pub count: usize,
        pub models: Vec<String>,
        pub error: Option<String>,
    }

    #[derive(Debug, Clone)]
    struct OpenAiCompatibleEndpoint {
        adapter: String,
        base_url: String,
        api_key: String,
        shim: Option<String>,
        source: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ShimEndpointConfig {
        shim: String,
        base_url: String,
        api_key: String,
        source: String,
    }

    const SHIM_OPENAI: &str = "openai";
    const SHIM_COPILOT: &str = "copilot";
    const SHIM_BEDROCK_MANTLE: &str = "bedrock-mantle";
    const SHIM_OPENCODE: &str = "opencode";
    const SHIM_OPENCODE_GO: &str = "opencode-go";
    const SHIM_ORDER: &[&str] = &[
        SHIM_OPENAI,
        SHIM_COPILOT,
        SHIM_BEDROCK_MANTLE,
        SHIM_OPENCODE,
        SHIM_OPENCODE_GO,
    ];

    pub fn resolve_model(configured: Option<&str>) -> Result<String> {
        if let Some(value) = configured.filter(|v| !v.trim().is_empty()) {
            return Ok(canonical_model_spec(value));
        }
        if let Ok(value) = env::var("OY_MODEL")
            && !value.trim().is_empty()
        {
            return Ok(canonical_model_spec(&value));
        }
        if let Some(model) = config::load_model_config()?.model {
            return Ok(canonical_model_spec(&model));
        }
        bail!(no_model_message())
    }

    fn no_model_message() -> String {
        let mut lines = vec!["No model configured.".to_string()];
        if let Some(choice) = recommended_models().first() {
            lines.push(format!("Detected provider auth. Try: oy model {choice}"));
        } else {
            lines.push("No provider auth detected. Run `oy doctor` for setup help.".to_string());
        }
        lines.push("Then run: oy \"inspect this repo\"".to_string());
        lines.push(
            "Advanced: use `oy model` to list options or set OY_MODEL for one run.".to_string(),
        );
        lines.join("\n")
    }

    pub fn resolve_shim() -> Result<Option<String>> {
        if let Ok(value) = env::var("OY_SHIM")
            && !value.trim().is_empty()
        {
            return Ok(Some(value));
        }
        Ok(config::load_model_config()?.shim)
    }

    pub fn recommended_models() -> Vec<String> {
        let mut out = Vec::new();
        let auth = auth_statuses();
        if auth.iter().any(|item| item.adapter == SHIM_OPENAI) {
            out.push("gpt-4.1-mini".to_string());
        }
        if auth.iter().any(|item| item.adapter == "github") {
            out.push("copilot::gpt-4.1-mini".to_string());
        }
        if auth.iter().any(|item| item.adapter == "bedrock") {
            out.push("bedrock::global.amazon.nova-2-lite-v1:0".to_string());
        }
        if auth.iter().any(|item| item.adapter == SHIM_BEDROCK_MANTLE) {
            out.push("bedrock-mantle::moonshotai.kimi-k2.5".to_string());
        }
        if auth.iter().any(|item| item.adapter == SHIM_OPENCODE) {
            out.push("opencode::gpt-5.1-codex-max".to_string());
        }
        if auth.iter().any(|item| item.adapter == SHIM_OPENCODE_GO) {
            out.push("opencode-go::kimi-k2.5".to_string());
        }
        if auth
            .iter()
            .any(|item| item.adapter == "local-openai-compatible")
        {
            out.push("local-8080::qwen3.5".to_string());
        }
        out.sort();
        out.dedup();
        out
    }

    pub fn list_builtin_model_hints() -> Vec<String> {
        vec![
            "openai_resp::gpt-5.5".to_string(),
            "gpt-5.4-mini".to_string(),
            "gpt-4.1-mini".to_string(),
            "copilot::gpt-4.1-mini".to_string(),
            "bedrock::global.amazon.nova-2-lite-v1:0".to_string(),
            "bedrock::au.anthropic.claude-sonnet-4-5-20250929-v1:0".to_string(),
            "bedrock::au.anthropic.claude-haiku-4-5-20251001-v1:0".to_string(),
            "bedrock::global.anthropic.claude-sonnet-4-5-20250929-v1:0".to_string(),
            "bedrock::openai.gpt-oss-120b-1:0".to_string(),
            "bedrock-mantle::moonshotai.kimi-k2.5".to_string(),
            "bedrock-mantle::moonshot.kimi-k2-thinking".to_string(),
            "bedrock-mantle::openai.gpt-oss-120b".to_string(),
            "opencode::gpt-5.1-codex-max".to_string(),
            "opencode::kimi-k2.5".to_string(),
            "opencode::gpt-5-nano".to_string(),
            "opencode-go::kimi-k2.5".to_string(),
            "local-8080::qwen3.5".to_string(),
            "local-11434::qwen3.5".to_string(),
        ]
    }

    pub async fn inspect_models() -> Result<ModelListing> {
        let current = resolve_model(None).ok();
        let current_shim = resolve_shim().ok().flatten();
        let auth = auth_statuses()
            .into_iter()
            .filter(|item| item.present || item.auto_configured)
            .collect::<Vec<_>>();
        let recommended = recommended_models();
        let dynamic = inspect_openai_compatible_models().await;
        let hints = list_builtin_model_hints();
        let all_models = collect_all_models(&dynamic, &hints);
        Ok(ModelListing {
            current,
            current_shim,
            auth,
            recommended,
            dynamic,
            hints,
            all_models,
        })
    }

    fn collect_all_models(dynamic: &[AdapterModels], hints: &[String]) -> Vec<String> {
        let mut items = dynamic
            .iter()
            .filter(|group| group.ok)
            .flat_map(|group| group.models.iter().cloned())
            .chain(hints.iter().cloned())
            .collect::<Vec<_>>();
        items.sort();
        items.dedup();
        items
    }

    pub fn canonical_model_spec(spec: &str) -> String {
        spec.trim().to_string()
    }

    pub fn to_genai_model_spec(spec: &str) -> String {
        canonical_model_spec(spec)
    }

    pub fn default_reasoning_effort(model_spec: &str) -> Option<&'static str> {
        let (_, model) = config::split_model_spec(model_spec);
        let (inline_effort, _) = split_reasoning_effort_suffix(model);
        inline_effort.or_else(|| reasoning_effort_option(model_spec))
    }

    pub fn reasoning_effort_option(model_spec: &str) -> Option<&'static str> {
        if env::var("OY_THINKING").is_ok() || env::var("OY_REASONING_EFFORT").is_ok() {
            return configured_reasoning_effort();
        }
        let (_, model) = config::split_model_spec(model_spec);
        let (inline_effort, base_model) = split_reasoning_effort_suffix(model);
        if inline_effort.is_some() {
            return None;
        }
        reasoning_capable_model(base_model).then_some("high")
    }

    fn configured_reasoning_effort() -> Option<&'static str> {
        env_value("OY_THINKING")
            .or_else(|| env_value("OY_REASONING_EFFORT"))
            .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
                "" | "auto" => None,
                "off" | "false" | "0" | "none" => Some("none"),
                "minimal" => Some("minimal"),
                "low" => Some("low"),
                "medium" => Some("medium"),
                "high" | "true" | "1" | "on" => Some("high"),
                _ => None,
            })
    }

    fn split_reasoning_effort_suffix(model: &str) -> (Option<&'static str>, &str) {
        if let Some((base, suffix)) = model.rsplit_once('-') {
            let effort = match suffix.to_ascii_lowercase().as_str() {
                "none" => Some("none"),
                "minimal" => Some("minimal"),
                "low" => Some("low"),
                "medium" => Some("medium"),
                "high" => Some("high"),
                _ => None,
            };
            if let Some(effort) = effort {
                return (Some(effort), base);
            }
        }
        (None, model)
    }

    fn reasoning_capable_model(model: &str) -> bool {
        let model = model
            .rsplit_once('/')
            .map(|(_, name)| name)
            .unwrap_or(model)
            .to_ascii_lowercase();
        model.starts_with("gpt-5")
            || model.contains("codex")
            || model.starts_with("o1")
            || model.starts_with("o3")
            || model.starts_with("o4")
            || model.starts_with("claude-3-7")
            || model.starts_with("claude-4")
            || model.starts_with("claude-sonnet-4")
            || model.starts_with("claude-opus-4")
            || model.starts_with("gemini-3")
    }

    pub fn auth_statuses() -> Vec<AuthStatus> {
        let mut items = Vec::new();
        if let Some(status) = bearer_shim_status(SHIM_OPENAI, Some("OPENAI_API_KEY")) {
            items.push(status);
        }
        if let Some(status) = local_auth_status() {
            items.push(status);
        }
        items.push(github_status());
        items.push(bedrock_status());
        items
            .into_iter()
            .filter(|item| item.present || item.auto_configured)
            .collect()
    }

    fn bearer_shim_status(shim: &str, env_var: Option<&str>) -> Option<AuthStatus> {
        let config = shim_endpoint_config(shim)?;
        Some(AuthStatus {
            adapter: shim.to_string(),
            env_var: env_var.map(ToOwned::to_owned),
            present: true,
            source: config.source,
            detail: format!("using {}", normalize_base_url(&config.base_url)),
            auto_configured: false,
        })
    }

    fn local_auth_status() -> Option<AuthStatus> {
        let _local = env_value("LOCAL_API_KEY")?;
        Some(AuthStatus {
            adapter: "local-openai-compatible".to_string(),
            env_var: Some("LOCAL_API_KEY".to_string()),
            present: true,
            source: "env".to_string(),
            detail: "LOCAL_API_KEY detected for OpenAI-compatible local endpoints.".to_string(),
            auto_configured: false,
        })
    }

    async fn inspect_openai_compatible_models() -> Vec<AdapterModels> {
        let mut out = Vec::new();
        for endpoint in openai_compatible_endpoints() {
            match fetch_openai_compatible_models(&endpoint).await {
                Ok(models) if !models.is_empty() => out.push(AdapterModels {
                    adapter: endpoint.adapter,
                    ok: true,
                    source: endpoint.source,
                    count: models.len(),
                    models,
                    error: None,
                }),
                Ok(_) => {}
                Err(err) => out.push(AdapterModels {
                    adapter: endpoint.adapter,
                    ok: false,
                    source: endpoint.source,
                    count: 0,
                    models: Vec::new(),
                    error: Some(err.to_string()),
                }),
            }
        }
        out
    }

    fn openai_compatible_endpoints() -> Vec<OpenAiCompatibleEndpoint> {
        let mut endpoints = Vec::new();
        let mut seen = BTreeSet::new();
        for shim in SHIM_ORDER
            .iter()
            .copied()
            .chain(extra_local_shims().iter().map(String::as_str))
        {
            if let Some(config) = shim_endpoint_config(shim) {
                push_endpoint(
                    &mut endpoints,
                    &mut seen,
                    OpenAiCompatibleEndpoint {
                        adapter: config.shim.clone(),
                        source: format!("GET {}/models", normalize_base_url(&config.base_url)),
                        base_url: config.base_url,
                        api_key: config.api_key,
                        shim: Some(config.shim),
                    },
                );
            }
        }
        endpoints
    }

    fn shim_endpoint_config(shim: &str) -> Option<ShimEndpointConfig> {
        match shim {
            SHIM_OPENAI => env_value("OPENAI_API_KEY").map(|api_key| ShimEndpointConfig {
                shim: SHIM_OPENAI.to_string(),
                base_url: env_value("OPENAI_BASE_URL")
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
                api_key,
                source: "OPENAI_API_KEY".to_string(),
            }),
            SHIM_COPILOT => github_token().map(|api_key| ShimEndpointConfig {
                shim: SHIM_COPILOT.to_string(),
                base_url: env_value("COPILOT_BASE_URL")
                    .unwrap_or_else(|| "https://api.githubcopilot.com".to_string()),
                api_key,
                source: "GitHub token".to_string(),
            }),
            SHIM_BEDROCK_MANTLE => bearer_endpoint_config(
                SHIM_BEDROCK_MANTLE,
                || {
                    env_value("BEDROCK_MANTLE_BASE_URL").unwrap_or_else(|| {
                        format!(
                            "https://bedrock-mantle.{}.api.aws/v1",
                            crate::bedrock::region()
                        )
                    })
                },
                &[
                    (
                        "BEDROCK_MANTLE_API_KEY",
                        env_value("BEDROCK_MANTLE_API_KEY"),
                    ),
                    (
                        "AWS_BEARER_TOKEN_BEDROCK",
                        env_value("AWS_BEARER_TOKEN_BEDROCK"),
                    ),
                ],
            ),
            SHIM_OPENCODE => opencode_endpoint_config(SHIM_OPENCODE, "https://opencode.ai/zen/v1"),
            SHIM_OPENCODE_GO => {
                opencode_endpoint_config(SHIM_OPENCODE_GO, "https://opencode.ai/zen/go/v1")
            }
            value if value.starts_with("local-") => value
                .strip_prefix("local-")
                .and_then(|port| port.parse::<u16>().ok())
                .map(|_| ShimEndpointConfig {
                    shim: value.to_string(),
                    base_url: local_base_url(value),
                    api_key: local_api_key(),
                    source: "local OpenAI-compatible endpoint".to_string(),
                }),
            _ => None,
        }
    }

    fn bearer_endpoint_config(
        shim: &str,
        base_url: impl FnOnce() -> String,
        credentials: &[(&str, Option<String>)],
    ) -> Option<ShimEndpointConfig> {
        let (source, api_key) = credentials
            .iter()
            .find_map(|(source, value)| value.as_ref().map(|api_key| (*source, api_key.clone())))?;
        Some(ShimEndpointConfig {
            shim: shim.to_string(),
            base_url: base_url(),
            api_key,
            source: source.to_string(),
        })
    }

    fn opencode_endpoint_config(shim: &str, default_base_url: &str) -> Option<ShimEndpointConfig> {
        bearer_endpoint_config(
            shim,
            || opencode_base_url(shim, default_base_url),
            &[
                ("OPENCODE_API_KEY", env_value("OPENCODE_API_KEY")),
                ("opencode auth.json", opencode_auth_key(shim)),
            ],
        )
    }

    fn opencode_base_url(shim: &str, default_base_url: &str) -> String {
        let shim_env = format!("{}_BASE_URL", shim.to_ascii_uppercase().replace('-', "_"));
        env_value(&shim_env)
            .or_else(|| env_value("OPENCODE_BASE_URL"))
            .unwrap_or_else(|| default_base_url.to_string())
    }

    fn opencode_auth_key(shim: &str) -> Option<String> {
        let provider = if shim == SHIM_OPENCODE_GO {
            SHIM_OPENCODE_GO
        } else {
            SHIM_OPENCODE
        };
        opencode_auth_key_from_path(provider, opencode_auth_path())
    }

    fn opencode_auth_path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("opencode")
            .join("auth.json")
    }

    fn opencode_auth_key_from_path(provider: &str, path: PathBuf) -> Option<String> {
        let value = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
        opencode_auth_key_from_value(provider, &value)
    }

    fn opencode_auth_key_from_value(provider: &str, value: &Value) -> Option<String> {
        let provider_value = value.get(provider).or_else(|| {
            provider
                .strip_suffix('/')
                .and_then(|trimmed| value.get(trimmed))
        })?;
        match provider_value.get("type").and_then(Value::as_str) {
            Some("api") => provider_value
                .get("key")
                .and_then(Value::as_str)
                .filter(|key| !key.trim().is_empty())
                .map(ToOwned::to_owned),
            Some("wellknown") => provider_value
                .get("token")
                .or_else(|| provider_value.get("key"))
                .and_then(Value::as_str)
                .filter(|key| !key.trim().is_empty())
                .map(ToOwned::to_owned),
            _ => None,
        }
    }

    fn extra_local_shims() -> Vec<String> {
        let mut items = BTreeSet::new();
        for value in [
            resolve_model(None).ok(),
            env_value("OY_MODEL"),
            resolve_shim().ok().flatten(),
        ]
        .into_iter()
        .flatten()
        {
            let (shim, _) = config::split_model_spec(&value);
            if let Some(shim) = shim.filter(|s| s.starts_with("local-")) {
                items.insert(shim.to_string());
            }
        }
        items.into_iter().collect()
    }

    fn push_endpoint(
        endpoints: &mut Vec<OpenAiCompatibleEndpoint>,
        seen: &mut BTreeSet<String>,
        endpoint: OpenAiCompatibleEndpoint,
    ) {
        let key = format!(
            "{}
{}
{}",
            endpoint.adapter,
            normalize_base_url(&endpoint.base_url),
            endpoint.shim.clone().unwrap_or_default()
        );
        if seen.insert(key) {
            endpoints.push(endpoint);
        }
    }

    fn local_base_url(shim: &str) -> String {
        if let Some(port) = shim.strip_prefix("local-") {
            let env_name = format!("OY_LOCAL_{}_URL", port);
            if let Some(url) = env_value(&env_name) {
                return url;
            }
            return format!("http://127.0.0.1:{port}/v1");
        }
        "http://127.0.0.1:8080/v1".to_string()
    }

    fn local_api_key() -> String {
        env_value("LOCAL_API_KEY").unwrap_or_else(|| "oy-local".to_string())
    }

    async fn fetch_openai_compatible_models(
        endpoint: &OpenAiCompatibleEndpoint,
    ) -> Result<Vec<String>> {
        let url = format!("{}/models", normalize_base_url(&endpoint.base_url));
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?
            .get(&url)
            .bearer_auth(&endpoint.api_key)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("GET {url} failed with HTTP {}", response.status());
        }
        let value = response.json::<Value>().await?;
        let models = extract_model_ids(&value)
            .into_iter()
            .map(|id| match endpoint.shim.as_deref() {
                Some(prefix) => format!("{prefix}::{id}"),
                None => id,
            })
            .collect::<Vec<_>>();
        Ok(models)
    }

    fn extract_model_ids(value: &Value) -> Vec<String> {
        let data = if let Some(items) = value.get("data").and_then(Value::as_array) {
            items.clone()
        } else if let Some(items) = value.as_array() {
            items.clone()
        } else {
            Vec::new()
        };
        let mut ids = data
            .into_iter()
            .filter_map(|item| {
                item.get("id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<Vec<_>>();
        ids.sort();
        ids.dedup();
        ids
    }

    fn normalize_base_url(base_url: &str) -> String {
        base_url.trim_end_matches('/').to_string()
    }

    fn github_status() -> AuthStatus {
        let copilot = env_value("COPILOT_GITHUB_TOKEN");
        let gh = env_value("GH_TOKEN");
        let github = env_value("GITHUB_TOKEN");
        let auto = github_token_auto_configured();
        let present = copilot.is_some() || gh.is_some() || github.is_some() || auto;
        let detail = match (copilot.as_deref(), gh.as_deref(), github.as_deref(), auto) {
            (Some(_), _, _, _) => {
                "COPILOT_GITHUB_TOKEN detected; copilot-compatible auth available.".to_string()
            }
            (None, Some(_), _, _) => {
                "GH_TOKEN detected; copilot-compatible auth available.".to_string()
            }
            (None, None, Some(_), _) => {
                "GITHUB_TOKEN detected; copilot-compatible auth available.".to_string()
            }
            (None, None, None, true) => "GitHub token available from `gh auth token`.".to_string(),
            (None, None, None, false) => "No GitHub auth token detected.".to_string(),
        };
        AuthStatus {
            adapter: "github".to_string(),
            env_var: Some("COPILOT_GITHUB_TOKEN, GH_TOKEN, GITHUB_TOKEN".to_string()),
            present,
            source: if auto {
                "gh"
            } else if copilot.is_some() || gh.is_some() || github.is_some() {
                "env"
            } else {
                "missing"
            }
            .to_string(),
            detail,
            auto_configured: auto,
        }
    }

    fn bedrock_status() -> AuthStatus {
        let status = crate::bedrock::auth_status();
        AuthStatus {
            adapter: "bedrock".to_string(),
            env_var: Some("AWS_ACCESS_KEY_ID, AWS_PROFILE".to_string()),
            present: status.present,
            source: status.source,
            detail: status.detail,
            auto_configured: status.auto_configured,
        }
    }

    fn env_value(name: &str) -> Option<String> {
        env::var(name).ok().filter(|v| !v.trim().is_empty())
    }

    fn github_token() -> Option<String> {
        env_value("COPILOT_GITHUB_TOKEN")
            .or_else(|| env_value("GH_TOKEN"))
            .or_else(|| env_value("GITHUB_TOKEN"))
            .or_else(gh_auth_token)
    }

    fn github_token_auto_configured() -> bool {
        env_value("COPILOT_GITHUB_TOKEN").is_none()
            && env_value("GH_TOKEN").is_none()
            && env_value("GITHUB_TOKEN").is_none()
            && gh_auth_token().is_some()
    }

    fn gh_auth_token() -> Option<String> {
        let output = std::process::Command::new("gh")
            .arg("auth")
            .arg("token")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!token.is_empty()).then_some(token)
    }

    pub fn build_client() -> Result<Client> {
        let mut builder = Client::builder();
        if let Some(resolver) = service_target_resolver()? {
            builder = builder.with_service_target_resolver(resolver);
        }
        if let Some(auth) = auth_resolver()? {
            builder = builder.with_auth_resolver(auth);
        }
        Ok(builder.build())
    }

    fn auth_resolver() -> Result<Option<AuthResolver>> {
        let Some(api_key) = env_value("OPENAI_API_KEY") else {
            return Ok(None);
        };
        let resolver = AuthResolver::from_resolver_fn(move |model: ModelIden| {
            if openai_env_applies_to_model(&model) {
                Ok(Some(AuthData::from_single(api_key.clone())))
            } else {
                Ok(None)
            }
        });
        Ok(Some(resolver))
    }

    fn openai_adapter_for_model(model: &str) -> AdapterKind {
        if config::is_openai_responses_model(model) {
            AdapterKind::OpenAIResp
        } else {
            AdapterKind::OpenAI
        }
    }

    fn is_openai_adapter(kind: AdapterKind) -> bool {
        matches!(kind, AdapterKind::OpenAI | AdapterKind::OpenAIResp)
    }

    fn openai_env_applies_to_model(model: &ModelIden) -> bool {
        is_openai_adapter(model.adapter_kind) && openai_env_applies_to_model_name(&model.model_name)
    }

    fn openai_env_applies_to_model_name(model_name: &str) -> bool {
        let (namespace, _) = config::split_model_spec(model_name);
        matches!(namespace, None | Some("openai_resp")) && env_value("OY_SHIM").is_none()
    }

    fn service_target_resolver() -> Result<Option<ServiceTargetResolver>> {
        let base_url = env_value("OPENAI_BASE_URL");
        let configured_shim = resolve_shim()?;
        let resolver = ServiceTargetResolver::from_resolver_fn(move |target: ServiceTarget| {
            let model_name = target.model.model_name.to_string();
            if let Some(mapped) =
                openai_compatible_target(&target.model, configured_shim.as_deref())
                    .map_err(|err| err.to_string())?
            {
                return Ok(mapped);
            }
            if let Some(url) = base_url.as_ref().filter(|_| configured_shim.is_none())
                && openai_env_applies_to_model(&target.model)
            {
                return Ok(ServiceTarget {
                    endpoint: Endpoint::from_owned(normalize_base_url(url) + "/"),
                    auth: target.auth,
                    model: ModelIden::new(openai_adapter_for_model(&model_name), model_name),
                });
            }
            Ok(target)
        });
        Ok(Some(resolver))
    }

    fn openai_compatible_target(
        model: &ModelIden,
        configured_shim: Option<&str>,
    ) -> Result<Option<ServiceTarget>> {
        let model_name = model.model_name.to_string();
        let (namespace, inline_model) = config::split_model_spec(&model_name);
        let inline_shim = namespace.filter(|shim| config::is_routing_shim(shim));
        let shim = inline_shim.or(configured_shim);
        let Some(shim) = shim else {
            return Ok(None);
        };
        if !config::is_routing_shim(shim) {
            bail!("invalid routing shim: {shim}");
        }
        let target_model = if inline_shim.is_some() {
            ModelIden::new(
                openai_adapter_for_model(inline_model),
                inline_model.to_string(),
            )
        } else {
            model.clone()
        };
        let config = shim_endpoint_config(shim)
            .ok_or_else(|| anyhow!("routing shim {shim} is not configured or lacks credentials"))?;
        Ok(Some(ServiceTarget {
            endpoint: Endpoint::from_owned(normalize_base_url(&config.base_url) + "/"),
            auth: AuthData::from_single(config.api_key),
            model: target_model,
        }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::Mutex;

        static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

        #[test]
        fn local_shim_endpoint_config_matches_python_defaults() {
            let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
            unsafe { std::env::remove_var("LOCAL_API_KEY") };
            unsafe { std::env::remove_var("OY_SHIM") };
            unsafe { std::env::set_var("OPENAI_API_KEY", "openai-token") };
            let config = shim_endpoint_config("local-8088").unwrap();
            assert_eq!(config.shim, "local-8088");
            assert_eq!(config.base_url, "http://127.0.0.1:8088/v1");
            assert_eq!(config.api_key, "oy-local");
            assert!(shim_endpoint_config("local-nope").is_none());
            unsafe { std::env::remove_var("OPENAI_API_KEY") };
        }

        #[test]
        fn openai_response_only_models_use_responses_adapter() {
            assert_eq!(openai_adapter_for_model("gpt-5.5"), AdapterKind::OpenAIResp);
            assert_eq!(
                openai_adapter_for_model("openai/gpt-5.5"),
                AdapterKind::OpenAIResp
            );
            assert_eq!(
                openai_adapter_for_model("gpt-4.1-mini"),
                AdapterKind::OpenAI
            );
        }

        #[test]
        fn model_listing_includes_static_hints_as_selectable_models() {
            let hints = vec!["gpt-4.1-mini".to_string()];
            let models = collect_all_models(&[], &hints);
            assert_eq!(models, vec!["gpt-4.1-mini".to_string()]);
        }

        #[test]
        fn genai_model_spec_is_identity() {
            assert_eq!(to_genai_model_spec("copilot::gpt-5.5"), "copilot::gpt-5.5");
            assert_eq!(to_genai_model_spec("gpt-5.4-mini"), "gpt-5.4-mini");
            assert_eq!(
                canonical_model_spec("  local-8080::qwen3.5  "),
                "local-8080::qwen3.5"
            );
        }

        #[test]
        fn reasoning_defaults_to_high_for_capable_models_and_allows_suffix_override() {
            let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
            unsafe { std::env::remove_var("OY_THINKING") };
            unsafe { std::env::remove_var("OY_REASONING_EFFORT") };
            assert_eq!(default_reasoning_effort("gpt-5.5"), Some("high"));
            assert_eq!(
                default_reasoning_effort("copilot::gpt-5.5-low"),
                Some("low")
            );
            assert_eq!(default_reasoning_effort("gpt-4.1-mini"), None);
        }

        #[test]
        fn reasoning_env_override_can_disable_or_adjust() {
            let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
            unsafe { std::env::set_var("OY_THINKING", "off") };
            assert_eq!(default_reasoning_effort("gpt-5.5"), Some("none"));
            unsafe { std::env::set_var("OY_THINKING", "medium") };
            assert_eq!(default_reasoning_effort("gpt-5.5"), Some("medium"));
            unsafe { std::env::remove_var("OY_THINKING") };
            unsafe { std::env::remove_var("OY_REASONING_EFFORT") };
        }

        #[test]
        fn extract_model_ids_handles_openai_shape() {
            let value = serde_json::json!({
                "data": [
                    {"id": "gpt-4.1-mini"},
                    {"id": "gpt-4.1"},
                    {"id": "gpt-4.1-mini"}
                ]
            });
            assert_eq!(
                extract_model_ids(&value),
                vec!["gpt-4.1".to_string(), "gpt-4.1-mini".to_string()]
            );
        }

        #[test]
        fn inline_routing_shim_overrides_configured_shim() {
            let target = ModelIden::new(AdapterKind::OpenAI, "local-8088::qwen3.5".to_string());
            let mapped = openai_compatible_target(&target, Some("openai"))
                .unwrap()
                .unwrap();
            assert_eq!(mapped.model.model_name, "qwen3.5");
            assert_eq!(mapped.endpoint.base_url(), "http://127.0.0.1:8088/v1/");
        }

        #[test]
        fn native_adapter_namespace_is_not_treated_as_routing_shim() {
            let target =
                ModelIden::new(AdapterKind::OpenAIResp, "openai_resp::gpt-5.5".to_string());
            assert!(openai_compatible_target(&target, None).unwrap().is_none());

            let mapped = openai_compatible_target(&target, Some("local-8088"))
                .unwrap()
                .unwrap();
            assert_eq!(mapped.model.model_name, "openai_resp::gpt-5.5");
            assert_eq!(mapped.model.adapter_kind, AdapterKind::OpenAIResp);
            assert_eq!(mapped.endpoint.base_url(), "http://127.0.0.1:8088/v1/");
        }

        #[test]
        fn configured_shim_still_routes_plain_model() {
            let target = ModelIden::new(AdapterKind::OpenAI, "qwen3.5".to_string());
            let mapped = openai_compatible_target(&target, Some("local-8088"))
                .unwrap()
                .unwrap();
            assert_eq!(mapped.model.model_name, "qwen3.5");
            assert_eq!(mapped.endpoint.base_url(), "http://127.0.0.1:8088/v1/");
        }

        #[test]
        fn openai_env_only_applies_to_openai_models_without_routing() {
            let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
            unsafe { std::env::remove_var("OY_SHIM") };
            assert!(openai_env_applies_to_model(&ModelIden::new(
                AdapterKind::OpenAI,
                "gpt-4.1-mini"
            )));
            assert!(openai_env_applies_to_model(&ModelIden::new(
                AdapterKind::OpenAIResp,
                "gpt-5.5"
            )));
            assert!(openai_env_applies_to_model(&ModelIden::new(
                AdapterKind::OpenAIResp,
                "openai_resp::gpt-5.5"
            )));
            assert!(!openai_env_applies_to_model(&ModelIden::new(
                AdapterKind::Gemini,
                "gemini-2.5-flash"
            )));
            assert!(!openai_env_applies_to_model(&ModelIden::new(
                AdapterKind::Anthropic,
                "claude-sonnet-4"
            )));
            assert!(!openai_env_applies_to_model(&ModelIden::new(
                AdapterKind::OpenAI,
                "openai::gpt-4.1-mini"
            )));
            unsafe { std::env::set_var("OY_SHIM", "openai") };
            assert!(!openai_env_applies_to_model(&ModelIden::new(
                AdapterKind::OpenAI,
                "gpt-4.1-mini"
            )));
            unsafe { std::env::remove_var("OY_SHIM") };
        }

        #[test]
        fn bedrock_mantle_requires_bedrock_specific_bearer_token() {
            let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
            unsafe { std::env::remove_var("BEDROCK_MANTLE_API_KEY") };
            unsafe { std::env::remove_var("BEDROCK_MANTLE_BASE_URL") };
            unsafe { std::env::remove_var("OPENAI_BASE_URL") };
            unsafe { std::env::set_var("OPENAI_API_KEY", "openai-token") };
            assert!(shim_endpoint_config(SHIM_BEDROCK_MANTLE).is_none());

            unsafe { std::env::set_var("AWS_BEARER_TOKEN_BEDROCK", "bedrock-token") };
            unsafe { std::env::set_var("OPENAI_BASE_URL", "https://openai.example/v1") };
            let config = shim_endpoint_config(SHIM_BEDROCK_MANTLE).unwrap();
            assert_eq!(config.api_key, "bedrock-token");
            assert_eq!(config.source, "AWS_BEARER_TOKEN_BEDROCK");
            assert_eq!(
                config.base_url,
                format!(
                    "https://bedrock-mantle.{}.api.aws/v1",
                    crate::bedrock::region()
                )
            );
            unsafe { std::env::remove_var("AWS_BEARER_TOKEN_BEDROCK") };
            unsafe { std::env::remove_var("OPENAI_API_KEY") };
            unsafe { std::env::remove_var("OPENAI_BASE_URL") };
        }

        #[test]
        fn opencode_reads_api_key_from_auth_json_shapes() {
            let value = serde_json::json!({
                "opencode": { "type": "api", "key": "zen-token" },
                "opencode-go": { "type": "wellknown", "token": "go-token" }
            });
            assert_eq!(
                opencode_auth_key_from_value(SHIM_OPENCODE, &value),
                Some("zen-token".to_string())
            );
            assert_eq!(
                opencode_auth_key_from_value(SHIM_OPENCODE_GO, &value),
                Some("go-token".to_string())
            );
        }

        #[test]
        fn opencode_env_key_wins_over_auth_json() {
            let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
            unsafe { std::env::set_var("OPENCODE_API_KEY", "env-token") };
            unsafe { std::env::set_var("OPENCODE_BASE_URL", "https://example.invalid/v1") };
            let config = shim_endpoint_config(SHIM_OPENCODE).unwrap();
            assert_eq!(config.api_key, "env-token");
            assert_eq!(config.source, "OPENCODE_API_KEY");
            assert_eq!(config.base_url, "https://example.invalid/v1");
            unsafe { std::env::remove_var("OPENCODE_API_KEY") };
            unsafe { std::env::remove_var("OPENCODE_BASE_URL") };
        }

        #[test]
        fn recommended_models_follow_detected_auth() {
            let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
            unsafe { std::env::remove_var("OY_SHIM") };
            unsafe { std::env::set_var("OPENAI_API_KEY", "openai-token") };
            let recommendations = recommended_models();
            assert!(recommendations.contains(&"gpt-4.1-mini".to_string()));
            unsafe { std::env::remove_var("OPENAI_API_KEY") };
        }

        #[test]
        fn builtin_hints_include_bedrock_variants() {
            let hints = list_builtin_model_hints();
            assert!(hints.iter().any(|item| item.starts_with("bedrock::")));
            assert!(
                hints
                    .iter()
                    .any(|item| item.starts_with("bedrock-mantle::"))
            );
            assert!(hints.iter().any(|item| item.starts_with("opencode::")));
            assert!(hints.iter().any(|item| item.starts_with("opencode-go::")));
        }
    }
}

// === session ===
pub(crate) mod session {
    use anyhow::{Context, Result, bail};
    use chrono::{DateTime, Utc};
    use genai::chat::{
        ChatMessage, ChatOptions, ChatRequest, ChatResponse, ToolCall, ToolResponse,
    };
    use genai::webc;
    use reqwest::StatusCode;
    use reqwest::header::RETRY_AFTER;
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::future::Future;
    use std::path::Path;
    use std::time::Duration;
    use tiktoken_rs::{bpe_for_model, cl100k_base};
    use tokio::time::sleep;

    use crate::config::{self, SessionFile};

    const DEFAULT_MAX_TOOL_ROUNDS: usize = 512;
    const CHAT_RATE_LIMIT_MAX_RETRIES: usize = 3;
    const CHAT_RATE_LIMIT_DEFAULT_DELAY: Duration = Duration::from_secs(2);
    const CHAT_RATE_LIMIT_MAX_DELAY: Duration = Duration::from_secs(60);
    use crate::model;
    use crate::tools::{TodoItem, ToolContext, ToolPolicy};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Transcript {
        #[serde(default)]
        pub summary: Option<String>,
        pub messages: Vec<StoredMessage>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "role")]
    pub enum StoredMessage {
        #[serde(rename = "user")]
        User { content: String },
        #[serde(rename = "summary")]
        Summary { content: String },
        #[serde(rename = "assistant")]
        Assistant { content: String },
        #[serde(rename = "assistant_tool_calls")]
        AssistantToolCalls { tool_calls: Vec<StoredToolCall> },
        #[serde(rename = "tool")]
        Tool { call_id: String, content: String },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct StoredToolCall {
        pub call_id: String,
        pub fn_name: String,
        pub fn_arguments: Value,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    pub struct TokenEstimate {
        pub messages: usize,
        pub system_tokens: usize,
        pub message_tokens: usize,
        pub total_tokens: usize,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    pub struct CompactionStats {
        pub before_tokens: usize,
        pub after_tokens: usize,
        pub removed_messages: usize,
        pub compacted_tools: usize,
        pub summarized: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    pub struct ContextStatus {
        pub estimate: TokenEstimate,
        pub limit_tokens: usize,
        pub input_budget_tokens: usize,
        pub trigger_tokens: usize,
        pub summary_present: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ContextBudgetExceeded {
        pub estimated_tokens: usize,
        pub input_budget_tokens: usize,
        pub limit_tokens: usize,
    }

    impl std::fmt::Display for ContextBudgetExceeded {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "context estimate {} exceeds input budget {}; use /compact, temporarily raise OY_CONTEXT_LIMIT, or force-truncate history",
                self.estimated_tokens, self.input_budget_tokens
            )
        }
    }

    impl std::error::Error for ContextBudgetExceeded {}

    fn model_tokenizer_name(model: &str) -> &str {
        model
            .rsplit_once("::")
            .map(|(_, name)| name)
            .unwrap_or(model)
    }

    pub(crate) fn count_tokens(model: &str, text: &str) -> usize {
        let model_name = model_tokenizer_name(model);
        if let Ok(bpe) = bpe_for_model(model_name) {
            return bpe.encode_with_special_tokens(text).len();
        }
        cl100k_base()
            .ok()
            .map(|bpe| bpe.encode_with_special_tokens(text).len())
            .unwrap_or_else(|| text.split_whitespace().count())
    }

    fn take_chars(text: &str, max_chars: usize) -> String {
        text.chars().take(max_chars).collect()
    }

    fn take_last_chars(text: &str, max_chars: usize) -> String {
        let mut chars = text.chars().rev().take(max_chars).collect::<Vec<_>>();
        chars.reverse();
        chars.into_iter().collect()
    }

    fn compact_text(text: &str, model: &str, max_tokens: usize, label: &str) -> String {
        if count_tokens(model, text) <= max_tokens {
            return text.to_string();
        }
        let target_chars = max_tokens.saturating_mul(3).max(512);
        let half = target_chars / 2;
        let head = take_chars(text, half);
        let tail = take_last_chars(text, half);
        format!(
            "[{label}] original ~{} tokens, {} bytes. Preserved head/tail.\n\n--- head ---\n{}\n\n--- tail ---\n{}",
            count_tokens(model, text),
            text.len(),
            head.trim_end(),
            tail.trim_start()
        )
    }

    fn message_label(message: &StoredMessage) -> &'static str {
        match message {
            StoredMessage::User { .. } => "user",
            StoredMessage::Summary { .. } => "summary",
            StoredMessage::Assistant { .. } => "assistant",
            StoredMessage::AssistantToolCalls { .. } => "assistant_tool_calls",
            StoredMessage::Tool { .. } => "tool",
        }
    }

    impl StoredMessage {
        fn content_text(&self) -> String {
            match self {
                StoredMessage::User { content }
                | StoredMessage::Summary { content }
                | StoredMessage::Assistant { content } => content.clone(),
                StoredMessage::AssistantToolCalls { tool_calls } => tool_calls
                    .iter()
                    .map(|call| format!("{} {}", call.fn_name, call.fn_arguments))
                    .collect::<Vec<_>>()
                    .join(
                        "
",
                    ),
                StoredMessage::Tool { content, .. } => content.clone(),
            }
        }
    }

    #[derive(Debug, Clone)]
    pub struct Session {
        pub root: std::path::PathBuf,
        pub model: String,
        pub system_prompt: String,
        pub interactive: bool,
        pub policy: ToolPolicy,
        pub mode: String,
        pub transcript: Transcript,
        pub todos: Vec<TodoItem>,
    }

    impl Transcript {
        pub fn new() -> Self {
            Self {
                summary: None,
                messages: Vec::new(),
            }
        }

        fn valid_compaction_keep_from(&self, requested: usize) -> usize {
            let mut keep_from = requested.min(self.messages.len());
            while matches!(
                self.messages.get(keep_from),
                Some(StoredMessage::Tool { .. })
            ) {
                keep_from += 1;
            }
            keep_from
        }

        pub fn undo_last_turn(&mut self) -> bool {
            for index in (0..self.messages.len()).rev() {
                if matches!(self.messages[index], StoredMessage::User { .. }) {
                    self.messages.truncate(index);
                    return true;
                }
            }
            false
        }

        pub fn force_truncate_oldest_turns(&mut self) -> usize {
            if self.messages.len() <= 1 {
                return 0;
            }
            let remove_count = (self.messages.len() / 4)
                .max(1)
                .min(self.messages.len() - 1);
            let keep_from = self.valid_truncation_keep_from(remove_count);
            if keep_from == 0 || keep_from >= self.messages.len() {
                return 0;
            }
            self.messages.drain(..keep_from);
            keep_from
        }

        fn valid_truncation_keep_from(&self, requested: usize) -> usize {
            let mut keep_from = self.valid_compaction_keep_from(requested);
            while keep_from < self.messages.len()
                && !matches!(self.messages[keep_from], StoredMessage::User { .. })
            {
                keep_from += 1;
            }
            keep_from
        }

        pub fn token_estimate(
            &self,
            model: &str,
            system_prompt: &str,
            todos: &[TodoItem],
        ) -> TokenEstimate {
            let count_text = |text: &str| count_tokens(model, text);
            let system_tokens = count_text(system_prompt) + if todos.is_empty() { 0 } else { 4 };
            let summary_tokens = self
                .summary
                .as_ref()
                .map(|summary| 4 + count_text(summary))
                .unwrap_or(0);
            let message_tokens = summary_tokens
                + self
                    .messages
                    .iter()
                    .map(|message| 4 + count_text(&message.content_text()))
                    .sum::<usize>();
            TokenEstimate {
                messages: self.message_count(),
                system_tokens,
                message_tokens,
                total_tokens: system_tokens + message_tokens,
            }
        }

        fn message_count(&self) -> usize {
            self.messages.len() + usize::from(self.summary.is_some())
        }

        pub fn compact_tool_outputs(&mut self, model: &str, max_tokens: usize) -> usize {
            let mut compacted = 0;
            for message in &mut self.messages {
                let StoredMessage::Tool { content, .. } = message else {
                    continue;
                };
                if count_tokens(model, content) <= max_tokens
                    || content.contains("[tool output compacted]")
                {
                    continue;
                }
                *content = compact_text(content, model, max_tokens, "tool output compacted");
                compacted += 1;
            }
            compacted
        }

        fn rebuild_with_summary(&mut self, summary: String, keep_from: usize) {
            let existing = self.summary.take();
            let mut merged = String::from("[compacted conversation summary]\n");
            if let Some(existing) = existing.filter(|s| !s.trim().is_empty()) {
                merged.push_str(existing.trim());
                merged.push_str("\n\n[latest compaction]\n");
            }
            merged.push_str(summary.trim());
            self.summary = Some(merged);
            self.messages = self.messages.split_off(keep_from.min(self.messages.len()));
        }

        pub fn deterministic_compact_old_turns(
            &mut self,
            model: &str,
            system_prompt: &str,
            todos: &[TodoItem],
            budget: usize,
            recent_messages: usize,
            summary_tokens: usize,
        ) -> Option<CompactionStats> {
            let before = self.token_estimate(model, system_prompt, todos);
            if before.total_tokens <= budget || self.messages.len() <= 1 {
                return None;
            }
            let protected = recent_messages.max(1).min(self.messages.len() - 1);
            let keep_from = self.valid_compaction_keep_from(self.messages.len() - protected);
            if keep_from == 0 {
                return None;
            }
            let removed = self.messages[..keep_from].to_vec();
            let summary = deterministic_summary(&removed, model, summary_tokens);
            let removed_messages = removed.len();
            self.rebuild_with_summary(summary, keep_from);
            let after = self.token_estimate(model, system_prompt, todos);
            Some(CompactionStats {
                before_tokens: before.total_tokens,
                after_tokens: after.total_tokens,
                removed_messages,
                compacted_tools: 0,
                summarized: true,
            })
        }

        pub fn to_chat_request(
            &self,
            system_prompt: &str,
            tool_context: &ToolContext,
        ) -> ChatRequest {
            let mut req = ChatRequest::default().with_system(system_prompt);
            let mut pending_tool_call_ids: Vec<String> = Vec::new();
            if let Some(summary) = self.summary.as_ref().filter(|s| !s.trim().is_empty()) {
                req = req.append_message(ChatMessage::user(format!(
                    "[Compacted earlier conversation]\n{}",
                    summary.trim()
                )));
            }
            for (index, msg) in self.messages.iter().enumerate() {
                match msg {
                    StoredMessage::User { content } => {
                        req = req.append_message(ChatMessage::user(content.clone()))
                    }
                    StoredMessage::Summary { content } => {
                        req = req.append_message(ChatMessage::user(content.clone()))
                    }
                    StoredMessage::Assistant { content } => {
                        req = req.append_message(ChatMessage::assistant(content.clone()))
                    }
                    StoredMessage::AssistantToolCalls { tool_calls } => {
                        let calls = tool_calls
                            .iter()
                            .filter(|call| {
                                has_following_tool_response(
                                    &self.messages[index + 1..],
                                    &call.call_id,
                                )
                            })
                            .map(|call| {
                                pending_tool_call_ids.push(call.call_id.clone());
                                ToolCall {
                                    call_id: call.call_id.clone(),
                                    fn_name: call.fn_name.clone(),
                                    fn_arguments: call.fn_arguments.clone(),
                                    thought_signatures: None,
                                }
                            })
                            .collect::<Vec<_>>();
                        if !calls.is_empty() {
                            req = req.append_message(ChatMessage::assistant(calls));
                        }
                    }
                    StoredMessage::Tool { call_id, content } => {
                        if let Some(position) =
                            pending_tool_call_ids.iter().position(|id| id == call_id)
                        {
                            pending_tool_call_ids.swap_remove(position);
                            req = req.append_message(ChatMessage::from(ToolResponse::new(
                                call_id.clone(),
                                content.clone(),
                            )));
                        }
                    }
                }
            }
            let mut prompt = system_prompt.to_string();
            if !tool_context.todos.is_empty() {
                let header = config::session_text_value("transcript", "todo_system")
                    .unwrap_or_else(|_| String::from("{todos}"));
                let todos = crate::tools::format_todos(&tool_context.todos);
                prompt.push_str("\n\n");
                prompt.push_str(header.replace("{todos}", todos.trim_end()).trim());
            }
            req.system = Some(prompt);
            req
        }
    }

    impl Session {
        pub fn new(
            root: std::path::PathBuf,
            model: String,
            interactive: bool,
            mode: String,
            policy: ToolPolicy,
        ) -> Self {
            let system_prompt = config::system_prompt(interactive, &mode);
            Self {
                root,
                model,
                system_prompt,
                interactive,
                policy,
                mode,
                transcript: Transcript::new(),
                todos: Vec::new(),
            }
        }

        pub fn tool_context(&self) -> ToolContext {
            ToolContext {
                root: self.root.clone(),
                interactive: self.interactive,
                policy: self.policy,
                todos: self.todos.clone(),
            }
        }

        fn chat_options(&self) -> Option<ChatOptions> {
            model::reasoning_effort_option(&self.model)
                .and_then(|effort| effort.parse().ok())
                .map(|effort| ChatOptions::default().with_reasoning_effort(effort))
        }

        fn wait_status(&self, model_spec: &str) -> String {
            let estimate =
                self.transcript
                    .token_estimate(model_spec, &self.system_prompt, &self.todos);
            let mut parts = vec![
                "oy".to_string(),
                display_model(model_spec).to_string(),
                token_count_text(estimate.total_tokens),
                format!("{} msg", estimate.messages),
            ];
            if let Some(effort) = model::default_reasoning_effort(model_spec) {
                parts.push(format!("think {effort}"));
            }
            if !self.todos.is_empty() {
                let active = self
                    .todos
                    .iter()
                    .filter(|item| item.status != "done")
                    .count();
                parts.push(format!("{active}/{} todo", self.todos.len()));
            }
            parts.join(" · ")
        }

        pub fn context_status(&self) -> ContextStatus {
            let model_spec = model::to_genai_model_spec(&self.model);
            let config = config::context_config();
            ContextStatus {
                estimate: self.transcript.token_estimate(
                    &model_spec,
                    &self.system_prompt,
                    &self.todos,
                ),
                limit_tokens: config.limit_tokens,
                input_budget_tokens: config.input_budget_tokens(),
                trigger_tokens: config.trigger_tokens(),
                summary_present: self.transcript.summary.is_some(),
            }
        }

        pub fn compact_deterministic(&mut self) -> Option<CompactionStats> {
            let config = config::context_config();
            let model_spec = model::to_genai_model_spec(&self.model);
            let before =
                self.transcript
                    .token_estimate(&model_spec, &self.system_prompt, &self.todos);
            let compacted_tools = self
                .transcript
                .compact_tool_outputs(&model_spec, config.tool_output_tokens);
            let mut stats = self.transcript.deterministic_compact_old_turns(
                &model_spec,
                &self.system_prompt,
                &self.todos,
                config.input_budget_tokens(),
                config.recent_messages,
                config.summary_tokens,
            );
            if compacted_tools > 0 {
                let after =
                    self.transcript
                        .token_estimate(&model_spec, &self.system_prompt, &self.todos);
                match stats.as_mut() {
                    Some(stats) => stats.compacted_tools = compacted_tools,
                    None => {
                        stats = Some(CompactionStats {
                            before_tokens: before.total_tokens,
                            after_tokens: after.total_tokens,
                            removed_messages: 0,
                            compacted_tools,
                            summarized: false,
                        });
                    }
                }
            }
            stats
        }

        pub async fn compact_llm(&mut self) -> Result<Option<CompactionStats>> {
            compact_llm_session(self, true).await
        }

        pub fn save(&self, name: Option<&str>) -> Result<std::path::PathBuf> {
            let payload = SessionFile {
                model: self.model.clone(),
                saved_at: Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
                workspace_root: Some(self.root.clone()),
                transcript: serde_json::to_value(&self.transcript)?,
                todos: self.todos.clone(),
            };
            config::save_session_file(name, &payload)
        }
    }

    pub fn load_saved(
        name: Option<&str>,
        interactive: bool,
        mode: String,
        policy: ToolPolicy,
    ) -> Result<Option<Session>> {
        let Some(path) = config::resolve_saved_session(name)? else {
            return Ok(None);
        };
        let saved = config::load_session_file(&path)?;
        let transcript: Transcript = serde_json::from_value(saved.transcript)
            .with_context(|| format!("invalid saved transcript in {}", path.display()))?;
        let root = config::oy_root()?;
        ensure_saved_workspace_matches(&path, saved.workspace_root.as_deref(), &root)?;
        let system_prompt = config::system_prompt(interactive, &mode);
        Ok(Some(Session {
            root,
            model: saved.model,
            system_prompt,
            interactive,
            policy,
            mode,
            transcript,
            todos: saved.todos,
        }))
    }

    fn ensure_saved_workspace_matches(
        session_path: &Path,
        saved_root: Option<&Path>,
        current_root: &Path,
    ) -> Result<()> {
        let Some(saved_root) = saved_root else {
            return Ok(());
        };
        if saved_root == current_root {
            return Ok(());
        }
        bail!(
            "saved session {} belongs to workspace {}; current workspace is {}",
            session_path.display(),
            saved_root.display(),
            current_root.display()
        )
    }

    fn deterministic_summary(messages: &[StoredMessage], model: &str, max_tokens: usize) -> String {
        let mut out = String::from(
            "This summary was produced deterministically to fit the context budget. Prefer exact recent messages that follow over this summary.\n\n",
        );
        let per_message = (max_tokens / messages.len().max(1)).clamp(128, 1024);
        for (idx, message) in messages.iter().enumerate() {
            let text = message.content_text();
            out.push_str(&format!(
                "## {} {} (~{} tokens)\n",
                idx + 1,
                message_label(message),
                count_tokens(model, &text)
            ));
            match message {
                StoredMessage::AssistantToolCalls { tool_calls } => {
                    for call in tool_calls {
                        out.push_str(&format!(
                            "- tool call `{}` args: {}\n",
                            call.fn_name, call.fn_arguments
                        ));
                    }
                }
                StoredMessage::Tool { call_id, .. } => {
                    out.push_str(&format!("call_id: `{call_id}`\n"));
                    out.push_str(&compact_text(
                        &text,
                        model,
                        per_message,
                        "old tool output summarized",
                    ));
                    out.push('\n');
                }
                _ => {
                    out.push_str(&compact_text(
                        &text,
                        model,
                        per_message,
                        "old message summarized",
                    ));
                    out.push('\n');
                }
            }
            out.push('\n');
        }
        compact_text(&out, model, max_tokens, "deterministic transcript summary")
    }

    fn transcript_for_summary(
        messages: &[StoredMessage],
        model: &str,
        max_tokens: usize,
    ) -> String {
        let mut out = String::new();
        let per_message = (max_tokens / messages.len().max(1)).clamp(256, 2048);
        for (idx, message) in messages.iter().enumerate() {
            let text = message.content_text();
            out.push_str(&format!(
                "\n<message index=\"{}\" role=\"{}\">\n{}\n</message>\n",
                idx + 1,
                message_label(message),
                compact_text(
                    &text,
                    model,
                    per_message,
                    "message pre-truncated for summarization"
                )
            ));
        }
        compact_text(
            &out,
            model,
            max_tokens,
            "transcript pre-truncated for summarization",
        )
    }

    fn has_following_tool_response(messages: &[StoredMessage], call_id: &str) -> bool {
        for message in messages {
            match message {
                StoredMessage::Tool { call_id: id, .. } if id == call_id => return true,
                StoredMessage::Tool { .. } => continue,
                _ => return false,
            }
        }
        false
    }

    fn compaction_prompt(
        existing_summary: Option<&str>,
        messages: &[StoredMessage],
        model: &str,
    ) -> String {
        let prior = existing_summary.unwrap_or("");
        let transcript = transcript_for_summary(messages, model, 48_000);
        format!(
            r#"You are compacting a coding-agent transcript so future requests stay under a context limit.

Preserve facts needed to continue work:
- user goals, constraints, preferences, and explicit instructions
- exact filenames, commands, APIs, errors, test results, and config/env names
- decisions made and rationale when important
- tool results that affect next actions
- changes already made
- active todos/current plan/open questions

Prefer preserving human input over assistant prose. Drop filler, repeated logs, and irrelevant verbose output. Do not invent facts.

Return concise markdown with sections:
## User intent
## Constraints
## Repo facts
## Changes made
## Commands/results
## Current plan
## Open issues

Existing summary, if any:
{prior}

Transcript to compact:
{transcript}
"#
        )
    }

    fn display_model(model_spec: &str) -> &str {
        model_spec
            .rsplit_once("::")
            .map(|(_, model)| model)
            .unwrap_or(model_spec)
    }

    fn token_count_text(count: usize) -> String {
        if count < 1000 {
            format!("{count} tok")
        } else {
            format!("{:.1}k tok", count as f64 / 1000.0)
        }
    }

    async fn ensure_context_budget(session: &mut Session, model_spec: &str) -> Result<()> {
        let config = config::context_config();
        let estimate =
            session
                .transcript
                .token_estimate(model_spec, &session.system_prompt, &session.todos);
        if estimate.total_tokens <= config.trigger_tokens() {
            return Ok(());
        }

        if let Some(stats) = session.compact_deterministic()
            && !crate::ui::is_quiet()
        {
            crate::ui::err_line(format_args!(
                "compacted context: {} -> {} tokens ({} old messages, {} tool outputs)",
                stats.before_tokens,
                stats.after_tokens,
                stats.removed_messages,
                stats.compacted_tools
            ));
        }

        let estimate =
            session
                .transcript
                .token_estimate(model_spec, &session.system_prompt, &session.todos);
        if estimate.total_tokens > config.input_budget_tokens() {
            return Err(ContextBudgetExceeded {
                estimated_tokens: estimate.total_tokens,
                input_budget_tokens: config.input_budget_tokens(),
                limit_tokens: config.limit_tokens,
            }
            .into());
        }
        Ok(())
    }

    async fn compact_llm_session(
        session: &mut Session,
        force: bool,
    ) -> Result<Option<CompactionStats>> {
        let client = model::build_client()?;
        let model_spec = model::to_genai_model_spec(&session.model);
        compact_llm_session_with_client(session, &client, &model_spec, force).await
    }

    async fn exec_chat(
        model_spec: &str,
        client: &genai::Client,
        req: ChatRequest,
        options: Option<&ChatOptions>,
    ) -> Result<ChatResponse> {
        let retry_label = display_model(model_spec).to_string();
        retry_rate_limited_chat(&retry_label, || {
            let req = req.clone();
            let options = options.cloned();
            async move {
                if crate::bedrock::is_bedrock_model(model_spec) {
                    crate::bedrock::exec_chat(model_spec, req, options.as_ref()).await
                } else {
                    Ok(client.exec_chat(model_spec, req, options.as_ref()).await?)
                }
            }
        })
        .await
    }

    async fn retry_rate_limited_chat<F, Fut>(label: &str, mut call: F) -> Result<ChatResponse>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<ChatResponse>>,
    {
        let mut attempt = 0usize;
        loop {
            match call().await {
                Ok(response) => return Ok(response),
                Err(err) => {
                    let Some(delay) = rate_limit_retry_delay(err.as_ref(), attempt) else {
                        return Err(err);
                    };
                    attempt += 1;
                    if !crate::ui::is_quiet() {
                        crate::ui::err_line(format_args!(
                            "oy · {label} · rate limited; retrying in {}s ({attempt}/{CHAT_RATE_LIMIT_MAX_RETRIES})",
                            delay.as_secs()
                        ));
                    }
                    sleep(delay).await;
                }
            }
        }
    }

    fn rate_limit_retry_delay(
        err: &(dyn std::error::Error + 'static),
        attempt: usize,
    ) -> Option<Duration> {
        if attempt >= CHAT_RATE_LIMIT_MAX_RETRIES {
            return None;
        }

        genai_rate_limit_delay(err)
            .or_else(|| bedrock_rate_limit_delay(err))
            .map(|delay| delay.clamp(Duration::from_secs(1), CHAT_RATE_LIMIT_MAX_DELAY))
    }

    fn genai_rate_limit_delay(err: &(dyn std::error::Error + 'static)) -> Option<Duration> {
        let err = err.downcast_ref::<genai::Error>()?;
        match err {
            genai::Error::WebModelCall { webc_error, .. }
            | genai::Error::WebAdapterCall { webc_error, .. } => webc_rate_limit_delay(webc_error),
            genai::Error::HttpError { status, .. } if *status == StatusCode::TOO_MANY_REQUESTS => {
                Some(CHAT_RATE_LIMIT_DEFAULT_DELAY)
            }
            _ => None,
        }
    }

    fn webc_rate_limit_delay(err: &webc::Error) -> Option<Duration> {
        let webc::Error::ResponseFailedStatus {
            status, headers, ..
        } = err
        else {
            return None;
        };
        if *status != StatusCode::TOO_MANY_REQUESTS {
            return None;
        }
        headers
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after)
            .or(Some(CHAT_RATE_LIMIT_DEFAULT_DELAY))
    }

    fn bedrock_rate_limit_delay(err: &(dyn std::error::Error + 'static)) -> Option<Duration> {
        let text = err.to_string().to_ascii_lowercase();
        if text.contains("throttling")
            || text.contains("too many requests")
            || text.contains("rate exceeded")
        {
            Some(CHAT_RATE_LIMIT_DEFAULT_DELAY)
        } else {
            None
        }
    }

    fn parse_retry_after(value: &str) -> Option<Duration> {
        if let Ok(seconds) = value.trim().parse::<u64>() {
            return Some(Duration::from_secs(seconds));
        }
        let retry_at = DateTime::parse_from_rfc2822(value).ok()?;
        let delay = retry_at
            .with_timezone(&Utc)
            .signed_duration_since(Utc::now());
        delay.to_std().ok().or(Some(Duration::from_secs(0)))
    }

    async fn compact_llm_session_with_client(
        session: &mut Session,
        client: &genai::Client,
        model_spec: &str,
        force: bool,
    ) -> Result<Option<CompactionStats>> {
        let config = config::context_config();
        let before =
            session
                .transcript
                .token_estimate(model_spec, &session.system_prompt, &session.todos);
        if !force && before.total_tokens <= config.input_budget_tokens() {
            return Ok(None);
        }
        if session.transcript.messages.len() <= 1 {
            return Ok(None);
        }

        let protected = config
            .recent_messages
            .max(1)
            .min(session.transcript.messages.len() - 1);
        let keep_from = session
            .transcript
            .valid_compaction_keep_from(session.transcript.messages.len() - protected);
        if keep_from == 0 {
            return Ok(None);
        }

        let removed = session.transcript.messages[..keep_from].to_vec();
        let prompt = compaction_prompt(session.transcript.summary.as_deref(), &removed, model_spec);
        let req = ChatRequest::default()
            .with_system(
                "You compact coding-agent transcripts. Return only the compacted markdown summary.",
            )
            .append_message(ChatMessage::user(prompt));
        let options = session.chat_options();
        let response = exec_chat(model_spec, client, req, options.as_ref()).await?;
        let mut summary = response.into_first_text().unwrap_or_default();
        if summary.trim().is_empty() {
            summary = deterministic_summary(&removed, model_spec, config.summary_tokens);
        } else if count_tokens(model_spec, &summary) > config.summary_tokens {
            summary = compact_text(
                &summary,
                model_spec,
                config.summary_tokens,
                "llm summary compacted",
            );
        }

        let removed_messages = removed.len();
        session.transcript.rebuild_with_summary(summary, keep_from);
        let after =
            session
                .transcript
                .token_estimate(model_spec, &session.system_prompt, &session.todos);
        Ok(Some(CompactionStats {
            before_tokens: before.total_tokens,
            after_tokens: after.total_tokens,
            removed_messages,
            compacted_tools: 0,
            summarized: true,
        }))
    }

    #[derive(Default)]
    struct RepeatedNoopTools {
        seen: BTreeSet<String>,
    }

    impl RepeatedNoopTools {
        fn record(&mut self, name: &str, args: &Value, result: &Value) -> Result<()> {
            if !is_noop_tool_result(name, result) {
                self.seen.clear();
                return Ok(());
            }
            let key = format!(
                "{}:{}",
                name,
                serde_json::to_string(args).unwrap_or_default()
            );
            if !self.seen.insert(key) {
                bail!(
                    "tool loop made no progress: repeated no-op {name}; inspect the latest tool output and choose a different action"
                )
            }
            Ok(())
        }
    }

    fn is_noop_tool_result(name: &str, result: &Value) -> bool {
        match name {
            "replace" => {
                result.get("replacement_count").and_then(Value::as_u64) == Some(0)
                    && result
                        .get("changed_file_count")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        == 0
                    && result
                        .get("errors")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty)
            }
            _ => false,
        }
    }

    pub async fn run_prompt(session: &mut Session, prompt: &str) -> Result<String> {
        run_prompt_with_policy(session, prompt, None).await
    }

    pub async fn run_prompt_read_only(session: &mut Session, prompt: &str) -> Result<String> {
        run_prompt_with_policy(session, prompt, Some(ToolPolicy::read_only())).await
    }

    pub async fn run_prompt_once_no_tools(
        model: &str,
        system_prompt: &str,
        prompt: &str,
    ) -> Result<String> {
        let client = model::build_client()?;
        let model_spec = model::to_genai_model_spec(model);
        let req = ChatRequest::default()
            .with_system(system_prompt)
            .append_message(ChatMessage::user(prompt.to_string()));
        if !crate::ui::is_quiet() {
            let tokens =
                count_tokens(&model_spec, system_prompt) + count_tokens(&model_spec, prompt);
            crate::ui::err_line(format_args!(
                "oy · {} · {} · no tools",
                display_model(&model_spec),
                token_count_text(tokens)
            ));
        }
        let options = model::reasoning_effort_option(model)
            .and_then(|effort| effort.parse().ok())
            .map(|effort| ChatOptions::default().with_reasoning_effort(effort));
        let response = exec_chat(&model_spec, &client, req, options.as_ref()).await?;
        Ok(response.into_first_text().unwrap_or_default())
    }

    async fn run_prompt_with_policy(
        session: &mut Session,
        prompt: &str,
        policy_override: Option<ToolPolicy>,
    ) -> Result<String> {
        let client = model::build_client()?;
        session.transcript.messages.push(StoredMessage::User {
            content: prompt.to_string(),
        });
        let mut repeated_noop_tools = RepeatedNoopTools::default();
        let tool_round_limit = config::max_tool_rounds(DEFAULT_MAX_TOOL_ROUNDS);
        let mut tool_round_count = 0usize;
        let mut tool_call_count = 0usize;

        loop {
            let mut tool_context = session.tool_context();
            if let Some(policy) = policy_override {
                tool_context.policy = policy;
            }
            let tool_specs = crate::tools::tool_specs(&tool_context);
            let model_spec = model::to_genai_model_spec(&session.model);
            ensure_context_budget(session, &model_spec).await?;
            let req = session
                .transcript
                .to_chat_request(&session.system_prompt, &tool_context)
                .with_tools(tool_specs.clone());
            if !crate::ui::is_quiet() {
                crate::ui::err_line(format_args!("{}", session.wait_status(&model_spec)));
            }
            let options = session.chat_options();
            let response = exec_chat(&model_spec, &client, req, options.as_ref()).await?;
            let tool_calls = response
                .tool_calls()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            if !tool_calls.is_empty() {
                let next_tool_round = tool_round_count + 1;
                if tool_round_limit.exceeded(next_tool_round) {
                    let limit = tool_round_limit.label();
                    bail!(
                        "tool loop exceeded {limit} tool rounds ({tool_call_count} tool calls completed); set OY_MAX_TOOL_ROUNDS=<number> or OY_MAX_TOOL_ROUNDS=unlimited for trusted long runs"
                    );
                }
                tool_round_count = next_tool_round;
                crate::ui::tool_batch(tool_round_count, tool_calls.len());
                session
                    .transcript
                    .messages
                    .push(StoredMessage::AssistantToolCalls {
                        tool_calls: tool_calls
                            .iter()
                            .map(|call| StoredToolCall {
                                call_id: call.call_id.clone(),
                                fn_name: call.fn_name.clone(),
                                fn_arguments: call.fn_arguments.clone(),
                            })
                            .collect(),
                    });

                for call in tool_calls {
                    tool_call_count += 1;
                    let mut ctx = session.tool_context();
                    if let Some(policy) = policy_override {
                        ctx.policy = policy;
                    }
                    let result = match crate::tools::invoke(
                        &mut ctx,
                        &call.fn_name,
                        call.fn_arguments.clone(),
                    )
                    .await
                    {
                        Ok(value) => value,
                        Err(err) => json!({"ok": false, "error": err.to_string()}),
                    };
                    session.todos = ctx.todos;
                    let content = crate::tools::encode_tool_output(&result);
                    repeated_noop_tools.record(&call.fn_name, &call.fn_arguments, &result)?;
                    session.transcript.messages.push(StoredMessage::Tool {
                        call_id: call.call_id.clone(),
                        content,
                    });
                }
                continue;
            }

            let answer = response.into_first_text().unwrap_or_default();
            session.transcript.messages.push(StoredMessage::Assistant {
                content: answer.clone(),
            });
            return Ok(answer);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use genai::ModelIden;
        use genai::adapter::AdapterKind;

        #[test]
        fn rate_limit_delay_respects_retry_after_seconds() {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(RETRY_AFTER, "7".parse().unwrap());
            let err = genai::Error::WebModelCall {
                model_iden: ModelIden::new(AdapterKind::OpenAI, "gpt-test"),
                webc_error: webc::Error::ResponseFailedStatus {
                    status: StatusCode::TOO_MANY_REQUESTS,
                    body: "rate limited".into(),
                    headers: Box::new(headers),
                },
            };

            assert_eq!(
                rate_limit_retry_delay(&err, 0),
                Some(Duration::from_secs(7))
            );
        }

        #[test]
        fn rate_limit_delay_clamps_retry_after_and_retry_count() {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(RETRY_AFTER, "120".parse().unwrap());
            let err = genai::Error::WebAdapterCall {
                adapter_kind: AdapterKind::OpenAI,
                webc_error: webc::Error::ResponseFailedStatus {
                    status: StatusCode::TOO_MANY_REQUESTS,
                    body: "rate limited".into(),
                    headers: Box::new(headers),
                },
            };

            assert_eq!(
                rate_limit_retry_delay(&err, 0),
                Some(CHAT_RATE_LIMIT_MAX_DELAY)
            );
            assert_eq!(
                rate_limit_retry_delay(&err, CHAT_RATE_LIMIT_MAX_RETRIES),
                None
            );
        }

        #[test]
        fn rate_limit_delay_ignores_non_429_status() {
            let err = genai::Error::WebModelCall {
                model_iden: ModelIden::new(AdapterKind::OpenAI, "gpt-test"),
                webc_error: webc::Error::ResponseFailedStatus {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    body: "server error".into(),
                    headers: Box::new(reqwest::header::HeaderMap::new()),
                },
            };

            assert_eq!(rate_limit_retry_delay(&err, 0), None);
        }

        #[test]
        fn saved_workspace_mismatch_fails_closed() {
            let dir = tempfile::tempdir().unwrap();
            let current = tempfile::tempdir().unwrap();
            let session_path = dir.path().join("session.json");
            let saved_root = dir.path().to_path_buf();

            let err =
                ensure_saved_workspace_matches(&session_path, Some(&saved_root), current.path())
                    .unwrap_err();

            assert!(err.to_string().contains("belongs to workspace"));
        }

        #[test]
        fn saved_workspace_allows_legacy_without_root() {
            let dir = tempfile::tempdir().unwrap();
            assert!(
                ensure_saved_workspace_matches(&dir.path().join("session.json"), None, dir.path())
                    .is_ok()
            );
        }

        #[test]
        fn repeated_noop_tools_rejects_repeated_zero_replace() {
            let mut guard = RepeatedNoopTools::default();
            let args = json!({"path": "src/main.rs", "pattern": "missing", "replacement": "x"});
            let result = json!({
                "changed_file_count": 0,
                "replacement_count": 0,
                "errors": []
            });

            guard.record("replace", &args, &result).unwrap();
            let err = guard.record("replace", &args, &result).unwrap_err();

            assert!(err.to_string().contains("repeated no-op replace"));
        }

        #[test]
        fn repeated_noop_tools_allows_retry_after_progress() {
            let mut guard = RepeatedNoopTools::default();
            let args = json!({"path": "src/main.rs", "pattern": "missing", "replacement": "x"});
            let noop = json!({
                "changed_file_count": 0,
                "replacement_count": 0,
                "errors": []
            });
            let progress = json!({
                "changed_file_count": 1,
                "replacement_count": 1,
                "errors": []
            });

            guard.record("replace", &args, &noop).unwrap();
            guard.record("replace", &args, &progress).unwrap();

            assert!(guard.record("replace", &args, &noop).is_ok());
        }

        #[test]
        fn undo_last_turn_removes_user_and_followups() {
            let mut tx = Transcript {
                summary: None,
                messages: vec![
                    StoredMessage::User {
                        content: "one".into(),
                    },
                    StoredMessage::Assistant {
                        content: "two".into(),
                    },
                    StoredMessage::User {
                        content: "three".into(),
                    },
                    StoredMessage::AssistantToolCalls {
                        tool_calls: Vec::new(),
                    },
                    StoredMessage::Tool {
                        call_id: "c".into(),
                        content: "tool".into(),
                    },
                ],
            };
            assert!(tx.undo_last_turn());
            assert_eq!(tx.messages.len(), 2);
            assert!(matches!(tx.messages[0], StoredMessage::User { .. }));
            assert!(matches!(tx.messages[1], StoredMessage::Assistant { .. }));
        }

        #[test]
        fn force_truncate_oldest_turns_removes_old_history_without_orphan_tool() {
            let mut tx = Transcript {
                summary: None,
                messages: vec![
                    StoredMessage::User {
                        content: "old user".into(),
                    },
                    StoredMessage::AssistantToolCalls {
                        tool_calls: vec![StoredToolCall {
                            call_id: "call-1".into(),
                            fn_name: "read".into(),
                            fn_arguments: json!({"path": "src/main.rs"}),
                        }],
                    },
                    StoredMessage::Tool {
                        call_id: "call-1".into(),
                        content: "tool result".into(),
                    },
                    StoredMessage::User {
                        content: "new user".into(),
                    },
                ],
            };

            assert_eq!(tx.force_truncate_oldest_turns(), 3);
            assert_eq!(tx.messages.len(), 1);
            assert!(matches!(tx.messages[0], StoredMessage::User { .. }));
        }

        #[test]
        fn token_estimate_counts_summary() {
            let mut tx = Transcript::new();
            tx.summary = Some("old user wanted tests".into());
            tx.messages.push(StoredMessage::User {
                content: "new prompt".into(),
            });
            let estimate = tx.token_estimate("gpt-4o", "system", &[]);
            assert_eq!(estimate.messages, 2);
            assert!(estimate.message_tokens > 2);
        }

        #[test]
        fn compact_tool_outputs_preserves_head_and_tail() {
            let mut tx = Transcript {
                summary: None,
                messages: vec![StoredMessage::Tool {
                    call_id: "c".into(),
                    content: format!("{} middle {}", "a".repeat(10_000), "z".repeat(10_000)),
                }],
            };
            assert_eq!(tx.compact_tool_outputs("gpt-4o", 256), 1);
            let StoredMessage::Tool { content, .. } = &tx.messages[0] else {
                panic!("expected tool message");
            };
            assert!(content.contains("tool output compacted"));
            assert!(content.contains("aaa"));
            assert!(content.contains("zzz"));
        }

        #[test]
        fn deterministic_compaction_keeps_recent_messages() {
            let mut tx = Transcript {
                summary: None,
                messages: vec![
                    StoredMessage::User {
                        content: "old user".into(),
                    },
                    StoredMessage::Assistant {
                        content: "old assistant".into(),
                    },
                    StoredMessage::User {
                        content: "recent user".into(),
                    },
                    StoredMessage::Assistant {
                        content: "recent assistant".into(),
                    },
                ],
            };
            let stats = tx
                .deterministic_compact_old_turns("gpt-4o", "system", &[], 1, 2, 1024)
                .unwrap();
            assert_eq!(stats.removed_messages, 2);
            assert!(tx.summary.as_deref().unwrap().contains("old user"));
            assert_eq!(tx.messages.len(), 2);
            assert!(matches!(tx.messages[0], StoredMessage::User { .. }));
        }

        #[test]
        fn compaction_does_not_leave_orphan_tool_response() {
            let mut tx = Transcript {
                summary: None,
                messages: vec![
                    StoredMessage::User {
                        content: "old user".into(),
                    },
                    StoredMessage::AssistantToolCalls {
                        tool_calls: vec![StoredToolCall {
                            call_id: "call-1".into(),
                            fn_name: "read".into(),
                            fn_arguments: json!({"path": "src/main.rs"}),
                        }],
                    },
                    StoredMessage::Tool {
                        call_id: "call-1".into(),
                        content: "tool result".into(),
                    },
                    StoredMessage::User {
                        content: "latest user".into(),
                    },
                ],
            };

            let stats = tx
                .deterministic_compact_old_turns("gpt-4o", "system", &[], 1, 2, 1024)
                .unwrap();

            assert_eq!(stats.removed_messages, 3);
            assert_eq!(tx.messages.len(), 1);
            assert!(matches!(tx.messages[0], StoredMessage::User { .. }));
        }

        #[test]
        fn chat_request_drops_orphan_tool_messages() {
            let tx = Transcript {
                summary: None,
                messages: vec![
                    StoredMessage::Tool {
                        call_id: "missing-call".into(),
                        content: "orphan".into(),
                    },
                    StoredMessage::AssistantToolCalls {
                        tool_calls: vec![StoredToolCall {
                            call_id: "no-result".into(),
                            fn_name: "read".into(),
                            fn_arguments: json!({"path": "src/main.rs"}),
                        }],
                    },
                    StoredMessage::User {
                        content: "continue".into(),
                    },
                ],
            };
            let ctx = ToolContext {
                root: std::path::PathBuf::new(),
                interactive: false,
                policy: ToolPolicy::read_only(),
                todos: Vec::new(),
            };

            let req = tx.to_chat_request("system", &ctx);

            assert_eq!(req.messages.len(), 1);
            assert!(
                req.messages[0]
                    .content
                    .first_text()
                    .unwrap()
                    .contains("continue")
            );
        }

        #[test]
        fn token_estimate_counts_system_and_messages() {
            let tx = Transcript {
                summary: None,
                messages: vec![
                    StoredMessage::User {
                        content: "hello world".into(),
                    },
                    StoredMessage::Assistant {
                        content: "hi".into(),
                    },
                ],
            };
            let estimate = tx.token_estimate("gpt-4o", "system", &[]);
            assert_eq!(estimate.messages, 2);
            assert!(estimate.system_tokens > 0);
            assert!(estimate.message_tokens > estimate.messages);
            assert_eq!(
                estimate.total_tokens,
                estimate.system_tokens + estimate.message_tokens
            );
        }
    }
}
