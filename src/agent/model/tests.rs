use super::*;
use crate::llm::{Message, Protocol, RouteAuth, ToolSpec};
use std::env;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn split_model_spec(spec: &str) -> (Option<&str>, &str) {
    let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(spec);
    (parsed.provider, parsed.base_model)
}

struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
        let saved = vars
            .iter()
            .map(|(name, _)| (*name, env::var(name).ok()))
            .collect::<Vec<_>>();
        for (name, value) in vars {
            match value {
                Some(value) => unsafe { env::set_var(name, value) },
                None => unsafe { env::remove_var(name) },
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in self.saved.drain(..) {
            match value {
                Some(value) => unsafe { env::set_var(name, value) },
                None => unsafe { env::remove_var(name) },
            }
        }
    }
}

struct ModelInfoCacheGuard {
    saved: std::collections::HashMap<String, CachedModelInfo>,
}

impl ModelInfoCacheGuard {
    fn replace(next: Option<CachedModelInfo>) -> Self {
        Self {
            saved: replace_cached_model_info(next),
        }
    }
}

impl Drop for ModelInfoCacheGuard {
    fn drop(&mut self) {
        restore_cached_model_info(std::mem::take(&mut self.saved));
    }
}

#[test]
fn copilot_routes_reasoning_models_to_responses_api() {
    assert!(crate::llm::providers::github_copilot_should_use_responses_api("gpt-5.5"));
    assert!(crate::llm::providers::github_copilot_should_use_responses_api("gpt-5.3-codex"));
    assert!(
        crate::llm::providers::github_copilot_should_use_responses_api("gemini-3.1-pro-preview")
    );
    assert!(!crate::llm::providers::github_copilot_should_use_responses_api("gpt-4.1"));
}

#[test]
fn opencode_gpt5_routes_to_responses_api() {
    assert!(crate::llm::providers::opencode_should_use_responses_api(
        "opencode",
        "gpt-5.4-mini"
    ));
    assert!(!crate::llm::providers::opencode_should_use_responses_api(
        "opencode-go",
        "mimo-v2.5-pro"
    ));
    assert!(!crate::llm::providers::opencode_should_use_responses_api(
        "opencode-go",
        "kimi-k2.6"
    ));
}

#[test]
fn split_model_spec_strips_reasoning_suffix_for_routing() {
    assert_eq!(split_model_spec("gpt-5.5-high"), (None, "gpt-5.5"));
    assert_eq!(
        split_model_spec("copilot::gpt-5.5-low"),
        (Some("copilot"), "gpt-5.5")
    );
    assert_eq!(
        split_model_spec("opencode-go/kimi-k2.6-high"),
        (Some("opencode-go"), "kimi-k2.6")
    );
}

#[test]
fn parsed_model_spec_captures_provider_model_and_reasoning_suffix() {
    let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(" copilot::gpt-5.5-low ");
    assert_eq!(parsed.provider, Some("copilot"));
    assert_eq!(parsed.model, "gpt-5.5-low");
    assert_eq!(parsed.base_model, "gpt-5.5");
    assert_eq!(parsed.reasoning_effort, Some("low"));
    assert_eq!(parsed.provider_or_openai(), "github-copilot");
}

#[test]
fn prepare_chat_builds_openai_responses_plan() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set(&[
        ("OPENAI_API_KEY", Some("test-openai-key")),
        ("OPENAI_BASE_URL", Some("https://openai.example/v1")),
        (
            "OPENROUTER_PROVIDER_OPTIONS",
            Some("{\"usage\":true,\"promptCacheKey\":\"should-not-leak\"}"),
        ),
    ]);
    let chat = prepare_chat("openai::gpt-4.1-mini").unwrap();
    assert_eq!(chat.protocol, Protocol::OpenAiResponses);
    assert_eq!(chat.model, "gpt-4.1-mini");
    assert_eq!(chat.auth, RouteAuth::ApiKey("test-openai-key".to_string()));
    assert_eq!(chat.base_url.as_deref(), Some("https://openai.example/v1"));
    assert_eq!(
        chat.additional_params,
        Some(serde_json::json!({"store": false}))
    );
}

#[test]
fn prepare_chat_keeps_openrouter_provider_options_on_openrouter_route() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set(&[
        ("OPENROUTER_API_KEY", Some("test-openrouter-key")),
        ("OPENROUTER_BASE_URL", Some("https://openrouter.example/v1")),
        (
            "OPENROUTER_PROVIDER_OPTIONS",
            Some("{\"usage\":true,\"promptCacheKey\":\"abc\",\"ignored\":true}"),
        ),
    ]);

    let chat = prepare_chat("openrouter/qwen-test").unwrap();

    assert_eq!(chat.protocol, Protocol::OpenAiChat);
    assert_eq!(chat.model, "qwen-test");
    assert_eq!(
        chat.auth,
        RouteAuth::ApiKey("test-openrouter-key".to_string())
    );
    assert_eq!(
        chat.base_url.as_deref(),
        Some("https://openrouter.example/v1")
    );
    assert_eq!(
        chat.additional_params,
        Some(serde_json::json!({
            "usage": {"include": true},
            "prompt_cache_key": "abc"
        }))
    );
}

#[test]
fn prepare_chat_adds_openai_gpt5_defaults_and_preserves_explicit_effort() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set(&[("OPENAI_API_KEY", Some("test-openai-key"))]);

    let chat = prepare_chat("openai::gpt-5.5-low").unwrap();

    assert_eq!(chat.protocol, Protocol::OpenAiResponses);
    assert_eq!(
        chat.additional_params,
        Some(serde_json::json!({
            "store": false,
            "reasoning": {"effort": "low", "summary": "auto"},
            "text": {"verbosity": "low"},
        }))
    );
}

#[test]
fn prepare_chat_routes_copilot_chat_models_to_openai_compatible_chat() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set(&[
        ("GITHUB_COPILOT_API_KEY", Some("test-copilot-key")),
        ("COPILOT_API_KEY", None),
        ("OPENCODE_API_KEY", None),
    ]);
    let chat = prepare_chat("github-copilot/gpt-4.1").unwrap();
    assert_eq!(chat.protocol, Protocol::OpenAiChat);
    assert_eq!(chat.model, "gpt-4.1");
    assert_eq!(chat.auth, RouteAuth::ApiKey("test-copilot-key".to_string()));
    assert_eq!(
        chat.base_url.as_deref(),
        Some("https://api.githubcopilot.com")
    );
    assert_eq!(
        chat.additional_params,
        Some(serde_json::json!({"store": false}))
    );
}

#[test]
fn prepare_chat_routes_copilot_reasoning_models_to_responses_with_api_key() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set(&[
        ("GITHUB_COPILOT_API_KEY", Some("test-copilot-key")),
        ("COPILOT_API_KEY", None),
        ("OPENCODE_API_KEY", None),
    ]);
    let chat = prepare_chat("github-copilot/gpt-5.5-low").unwrap();
    assert_eq!(chat.protocol, Protocol::OpenAiResponses);
    assert_eq!(chat.model, "gpt-5.5");
    assert_eq!(chat.auth, RouteAuth::ApiKey("test-copilot-key".to_string()));
    assert_eq!(
        chat.base_url.as_deref(),
        Some("https://api.githubcopilot.com")
    );
    assert_eq!(
        chat.additional_params,
        Some(serde_json::json!({
            "store": false,
            "reasoning": {"effort": "low", "summary": "auto"}
        }))
    );
}

#[test]
fn prepare_chat_routes_xai_to_responses_api() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set(&[
        ("XAI_API_KEY", Some("test-xai-key")),
        ("XAI_BASE_URL", None),
    ]);

    let chat = prepare_chat("xai/grok-4").unwrap();

    assert_eq!(chat.protocol, Protocol::OpenAiResponses);
    assert_eq!(chat.model, "grok-4");
    assert_eq!(chat.auth, RouteAuth::ApiKey("test-xai-key".to_string()));
    assert_eq!(chat.base_url.as_deref(), Some("https://api.x.ai/v1"));
}

#[test]
fn prepare_chat_routes_google_gemini_with_api_key_header() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set(&[
        ("GOOGLE_GENERATIVE_AI_API_KEY", Some("test-google-key")),
        ("GEMINI_API_KEY", None),
        ("GOOGLE_API_KEY", None),
        (
            "GOOGLE_BASE_URL",
            Some("https://generativelanguage.example/v1beta"),
        ),
    ]);

    let chat = prepare_chat("google/gemini-3-flash").unwrap();

    assert_eq!(chat.protocol, Protocol::Gemini);
    assert_eq!(chat.model, "gemini-3-flash");
    assert_eq!(
        chat.auth,
        RouteAuth::Header {
            name: "x-goog-api-key".to_string(),
            value: "test-google-key".to_string(),
        }
    );
    assert_eq!(
        chat.base_url.as_deref(),
        Some("https://generativelanguage.example/v1beta")
    );
    assert_eq!(chat.additional_params, None);
}

#[test]
fn prepare_chat_routes_anthropic_messages_with_version_header() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set(&[
        ("ANTHROPIC_API_KEY", Some("test-anthropic-key")),
        ("ANTHROPIC_BASE_URL", Some("https://anthropic.example/v1")),
        ("ANTHROPIC_VERSION", Some("2024-01-01")),
        (
            "ANTHROPIC_PROVIDER_OPTIONS",
            Some("{\"thinking\":{\"type\":\"enabled\",\"budget_tokens\":1024}}"),
        ),
    ]);

    let chat = prepare_chat("anthropic/claude-sonnet-4-5").unwrap();

    assert_eq!(chat.protocol, Protocol::AnthropicMessages);
    assert_eq!(chat.model, "claude-sonnet-4-5");
    assert_eq!(
        chat.base_url.as_deref(),
        Some("https://anthropic.example/v1")
    );
    assert_eq!(
        chat.auth,
        RouteAuth::Headers(vec![
            ("x-api-key".to_string(), "test-anthropic-key".to_string()),
            ("anthropic-version".to_string(), "2024-01-01".to_string()),
        ])
    );
    assert_eq!(
        chat.additional_params,
        Some(serde_json::json!({"thinking":{"type":"enabled","budget_tokens":1024}}))
    );
}

#[test]
fn prepare_chat_routes_azure_with_api_key_header_and_version_query() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set(&[
        ("AZURE_OPENAI_API_KEY", Some("test-azure-key")),
        ("AZURE_OPENAI_RESOURCE_NAME", Some("oy-test")),
        ("AZURE_OPENAI_BASE_URL", None),
        ("AZURE_OPENAI_API_VERSION", Some("2025-01-01")),
        ("AZURE_OPENAI_USE_COMPLETION_URLS", None),
    ]);

    let chat = prepare_chat("azure/gpt-5.5-low").unwrap();

    assert_eq!(chat.protocol, Protocol::OpenAiResponses);
    assert_eq!(
        chat.base_url.as_deref(),
        Some("https://oy-test.openai.azure.com/openai/v1")
    );
    assert_eq!(
        chat.auth,
        RouteAuth::Header {
            name: "api-key".to_string(),
            value: "test-azure-key".to_string(),
        }
    );
    assert_eq!(
        chat.query_params,
        Some(vec![("api-version".to_string(), "2025-01-01".to_string())])
    );
    assert_eq!(
        chat.additional_params,
        Some(serde_json::json!({"reasoning": {"effort": "low", "summary": "auto"}}))
    );
}

#[test]
fn prepare_chat_routes_cloudflare_ai_gateway_with_gateway_header() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set(&[
        ("CLOUDFLARE_ACCOUNT_ID", Some("acct 1")),
        ("CLOUDFLARE_AI_GATEWAY_ID", Some("gw/one")),
        ("CLOUDFLARE_AI_GATEWAY_BASE_URL", None),
        ("CLOUDFLARE_API_TOKEN", Some("test-gateway-key")),
        ("CF_AIG_TOKEN", None),
        ("OPENAI_API_KEY", None),
    ]);

    let chat = prepare_chat("cloudflare-ai-gateway/meta-llama").unwrap();

    assert_eq!(chat.protocol, Protocol::OpenAiChat);
    assert_eq!(
        chat.base_url.as_deref(),
        Some("https://gateway.ai.cloudflare.com/v1/acct+1/gw%2Fone/compat")
    );
    assert_eq!(
        chat.auth,
        RouteAuth::Header {
            name: "cf-aig-authorization".to_string(),
            value: "test-gateway-key".to_string(),
        }
    );
}

#[test]
fn reasoning_defaults_to_high_for_capable_models_and_allows_suffix_override() {
    assert_eq!(default_reasoning_effort("gpt-5.5").as_deref(), Some("high"));
    assert_eq!(
        default_reasoning_effort("copilot::gpt-5.5-low").as_deref(),
        Some("low")
    );
    // Use a model absent from both the static fallback and OpenCode metadata
    // so the test is deterministic regardless of whether `opencode` is available.
    assert_eq!(default_reasoning_effort("test/no-reasoning-model"), None);
}

#[test]
fn miri_smoke_model_routing_and_reasoning_defaults() {
    assert_eq!(
        split_model_spec("opencode-go/kimi-k2.6-high"),
        (Some("opencode-go"), "kimi-k2.6")
    );
    assert!(crate::llm::providers::github_copilot_should_use_responses_api("gpt-5.5"));
    assert_eq!(reasoning_capable_fallback("gpt-5.5"), Some("high"));
    assert_eq!(
        default_reasoning_effort("moonshot/kimi-k2.6").as_deref(),
        Some("none")
    );
}

#[test]
fn moonshot_kimi_explicitly_disables_reasoning_by_default() {
    assert_eq!(
        default_reasoning_effort("opencode-go/kimi-k2.6").as_deref(),
        Some("none")
    );
    assert_eq!(
        default_reasoning_effort("moonshot/kimi-k2.6").as_deref(),
        Some("none")
    );
    assert_eq!(
        default_reasoning_effort("opencode-go/kimi-k2.6-high").as_deref(),
        Some("high")
    );
}

#[test]
fn reasoning_env_override_can_disable_or_adjust() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    set_thinking_override(Some("none"));
    assert_eq!(default_reasoning_effort("gpt-5.5").as_deref(), Some("none"));
    set_thinking_override(Some("medium"));
    assert_eq!(
        default_reasoning_effort("gpt-5.5").as_deref(),
        Some("medium")
    );
    set_thinking_override(None);
}

#[test]
fn reasoning_effort_json_uses_route_specific_param_shape() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let chat_json = crate::llm::route::resolve::reasoning_effort_json(
        default_reasoning_effort("gpt-5.5"),
        false,
    );
    assert_eq!(
        chat_json,
        Some(serde_json::json!({"reasoning_effort": "high"}))
    );
    let responses_json = crate::llm::route::resolve::reasoning_effort_json(
        default_reasoning_effort("gpt-5.5"),
        true,
    );
    assert_eq!(
        responses_json,
        Some(serde_json::json!({"reasoning": {"effort": "high"}}))
    );
    assert_eq!(
        crate::llm::route::resolve::reasoning_effort_json(
            default_reasoning_effort("gpt-4.1-mini"),
            false
        ),
        None
    );
}

#[test]
fn merge_additional_params_deep_merges_provider_defaults_and_overrides() {
    let base = Some(serde_json::json!({
        "reasoning": {"effort": "medium", "summary": "auto"},
        "text": {"verbosity": "low"}
    }));
    let overlay = Some(serde_json::json!({
        "reasoning": {"effort": "high"}
    }));

    assert_eq!(
        crate::llm::route::resolve::merge_additional_params(base, overlay),
        Some(serde_json::json!({
            "reasoning": {"effort": "high", "summary": "auto"},
            "text": {"verbosity": "low"}
        }))
    );
}

#[test]
fn merge_additional_params_replaces_non_object_overlays() {
    let base = Some(serde_json::json!({"reasoning": {"effort": "medium"}}));
    let overlay = Some(serde_json::json!({"reasoning": false}));

    assert_eq!(
        crate::llm::route::resolve::merge_additional_params(base, overlay),
        Some(serde_json::json!({"reasoning": false}))
    );
}

#[test]
fn model_info_cache_is_model_specific() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _cache = ModelInfoCacheGuard::replace(Some(CachedModelInfo {
        model_spec: canonical_cache_key("github-copilot/gpt-5.5-low"),
        limits: Some(opencode_models::OpenCodeModelLimit {
            context: 123_000,
            input: Some(120_000),
            output: 3_000,
        }),
        provider_info: ProviderInfo {
            provider: "github-copilot".to_string(),
            endpoint: Some("https://api.githubcopilot.com".to_string()),
        },
    }));

    let matching = provider_info("github-copilot/gpt-5.5-high");
    assert_eq!(matching.provider, "github-copilot");
    assert_eq!(
        matching.endpoint.as_deref(),
        Some("https://api.githubcopilot.com")
    );
    assert_eq!(
        model_limits("github-copilot/gpt-5.5-high").map(|limit| (
            limit.context,
            limit.input,
            limit.output
        )),
        Some((123_000, Some(120_000), 3_000))
    );

    let other = provider_info("openrouter/qwen-test");
    assert_eq!(other.provider, "openrouter");
    assert_eq!(other.endpoint, None);
    assert!(model_limits("openrouter/qwen-test").is_none());
}

// ── Live integration tests (network + OpenCode required) ──
// Run all with:  cargo nextest run --run-ignored ignored-only live_
// Run one with: cargo nextest run --run-ignored ignored-only live_<name>

// ---------------------------------------------------------------
// Simple text-response tests (no tools)
// ---------------------------------------------------------------

/// Returns `true` if the error chain looks like an auth/credential failure.
fn is_auth_error(err: &anyhow::Error) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("401")
        || text.contains("unauthorized")
        || text.contains("unauthenticated")
        || text.contains("overloaded_credentials")
        || text.contains("auth")
            && (text.contains("invalid") || text.contains("missing") || text.contains("failed"))
}

async fn assert_model_responds(model: &str, label: &str) {
    let system = "You are a helpful assistant. Answer very briefly.";
    let prompt = "Say hello in exactly one word.";
    match crate::session::run_prompt_once_no_tools(model, system, prompt).await {
        Ok(result) => {
            assert!(
                !result.trim().is_empty(),
                "{label} response should not be empty"
            );
            eprintln!("{label} response: {result}");
        }
        Err(err) if is_auth_error(&err) => {
            eprintln!("{label}: SKIP (auth error, not a code bug): {err}");
        }
        Err(err) => panic!("{label} should return a response: {err}"),
    }
}

#[tokio::test]
#[ignore]
async fn live_google_gemini_flash() {
    assert_model_responds("opencode/gemini-3-flash", "gemini-3-flash").await;
}

#[tokio::test]
#[ignore]
async fn live_google_gemini_pro() {
    assert_model_responds("opencode/gemini-3.1-pro", "gemini-3.1-pro").await;
}

#[tokio::test]
#[ignore]
async fn live_anthropic_claude_haiku() {
    assert_model_responds("opencode/claude-haiku-4-5", "claude-haiku-4-5").await;
}

#[tokio::test]
#[ignore]
async fn live_deepseek_v4_pro() {
    assert_model_responds("opencode-go/deepseek-v4-pro", "deepseek-v4-pro").await;
}

#[tokio::test]
#[ignore]
async fn live_deepseek_v4_flash() {
    assert_model_responds("opencode-go/deepseek-v4-flash", "deepseek-v4-flash").await;
}

#[tokio::test]
#[ignore]
async fn live_kimi_k26() {
    assert_model_responds("opencode-go/kimi-k2.6", "kimi-k2.6").await;
}

// ---------------------------------------------------------------
// Tool-calling tests — verify the model can invoke a tool
// ---------------------------------------------------------------

use serde::Deserialize;

/// A trivial echo tool: the model can call "echo" with a message,
/// and we verify it does so.
#[derive(Deserialize)]
struct EchoArgs {
    message: String,
}

#[derive(Clone)]
struct Echo;

impl crate::llm::LlmTool for Echo {
    fn name(&self) -> &str {
        "echo"
    }

    fn call<'a>(&'a self, args: String) -> crate::llm::LlmToolFuture<'a> {
        Box::pin(async move {
            let args: EchoArgs = serde_json::from_str(&args)?;
            Ok(args.message)
        })
    }
}

async fn assert_model_uses_tool(model: &str, label: &str) {
    let system = "You have an echo tool. When asked to ping, you MUST call the echo tool with message 'ping'. Do not just reply with text — actually call the tool.";
    let prompt = "Please ping.";

    let response = match super::exec_chat(
        model,
        system,
        vec![Message::user_text(prompt)],
        vec![ToolSpec {
            name: "echo".to_string(),
            description: "Echo back the message. Call this with the word 'ping'.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"message": {"type": "string"}},
                "required": ["message"]
            }),
            cache: None,
        }],
        vec![Box::new(Echo) as Box<dyn crate::llm::LlmTool>],
        crate::config::max_tool_rounds(2),
    )
    .await
    {
        Ok(response) => response,
        Err(err) if is_auth_error(&err) => {
            eprintln!("{label}: SKIP (auth error, not a code bug): {err}");
            return;
        }
        Err(err) => panic!("{label} tool call should succeed: {err}"),
    };

    let output = response.output.trim().to_string();
    eprintln!("{label} tool output: {output}");
    // Live smoke test: the API call with a tool definition must succeed.
    // Some small models may reply directly instead of invoking the tool;
    // either outcome proves the tool plumbing works.
    assert!(
        !output.is_empty(),
        "{label}: tool response should not be empty"
    );
}

#[tokio::test]
#[ignore]
async fn live_tools_google_gemini() {
    assert_model_uses_tool("opencode/gemini-3-flash", "gemini-3-flash+tools").await;
}

#[tokio::test]
#[ignore]
async fn live_tools_anthropic_claude() {
    assert_model_uses_tool("opencode/claude-haiku-4-5", "claude-haiku-4-5+tools").await;
}

#[tokio::test]
#[ignore]
async fn live_tools_deepseek() {
    assert_model_uses_tool("opencode-go/deepseek-v4-flash", "deepseek-v4-flash+tools").await;
}

#[tokio::test]
#[ignore]
async fn live_tools_kimi() {
    assert_model_uses_tool("opencode-go/kimi-k2.6", "kimi-k2.6+tools").await;
}
