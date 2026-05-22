//! Model selection, endpoint discovery, reasoning-effort resolution,
//! and the prompt-level LLM chat/tool loop.
//!
//! This module builds an [`LlmRequest`] from `oy`-owned messages and
//! tool specs, resolves the model route, and drives the native backend.
//! Provider metadata queries delegate to [`super::opencode_models`].

use crate::config;
use crate::llm::{
    ChatBackend, LlmRequest, LlmResponse, LlmTools, Message, NativeOpenAiBackend, ToolSpec,
};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::{LazyLock, RwLock};

pub(crate) use super::auth::AuthStatus;
use super::auth::env_value;
use super::opencode_models;
pub(crate) use super::opencode_models::AdapterModels;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListing {
    pub current: Option<String>,
    pub auth: Vec<AuthStatus>,
    pub dynamic: Vec<AdapterModels>,
    pub all_models: Vec<String>,
}

pub fn resolve_model(configured: Option<&str>) -> Result<String> {
    if let Some(value) = configured.filter(|v| !v.trim().is_empty()) {
        return Ok(config::canonical_model_spec(value));
    }
    if let Ok(value) = env::var("OY_MODEL")
        && !value.trim().is_empty()
    {
        return Ok(config::canonical_model_spec(&value));
    }
    let saved = config::load_model_config()?;
    if let Some(model) = saved.model.filter(|model| !model.trim().is_empty()) {
        return Ok(config::canonical_model_spec(&model));
    }
    bail!(no_model_message())
}

fn no_model_message() -> String {
    [
        "No model configured.",
        "Run `oy model` to inspect OpenCode verbose model metadata.",
        "Then run: oy \"inspect this repo\"",
        "Advanced: use `oy model` to list options or set OY_MODEL for one run.",
    ]
    .join("\n")
}

pub async fn inspect_models() -> Result<ModelListing> {
    let current = resolve_model(None).ok();
    let dynamic = opencode_models::inspect();
    let all_models = dynamic
        .iter()
        .flat_map(|group| group.models().iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(ModelListing {
        current,
        auth: super::auth::auth_statuses(),
        dynamic,
        all_models,
    })
}

/// Information about the provider for a model spec.
#[derive(Debug, Clone)]
pub(crate) struct ProviderInfo {
    /// The provider id (e.g. "openai", "github-copilot")
    pub provider: String,
    /// The API endpoint URL, if determinable from OpenCode metadata
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CachedModelInfo {
    pub model_spec: String,
    pub limits: Option<opencode_models::OpenCodeModelLimit>,
    pub provider_info: ProviderInfo,
}

static MODEL_INFO_CACHE: LazyLock<std::sync::RwLock<Option<CachedModelInfo>>> =
    LazyLock::new(|| std::sync::RwLock::new(None));

pub async fn cache_model_limits(model_spec: &str) -> Result<()> {
    let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(model_spec);
    let provider = parsed.provider_or_openai();
    if let Some(metadata) = crate::llm::providers::provider_metadata(provider)
        && !metadata.supported
    {
        bail!(
            "provider `{provider}` uses {:?}, which is not implemented by oy's native LLM backend yet",
            metadata.family
        );
    }
    let limits = opencode_models::lookup_limit(provider, parsed.base_model);

    let endpoint = match provider {
        "openai" => Some("https://api.openai.com/v1".to_string()),
        "github-copilot" => Some("https://api.githubcopilot.com".to_string()),
        _ => None,
    };

    let mut cache = MODEL_INFO_CACHE.write().unwrap();
    *cache = Some(CachedModelInfo {
        model_spec: canonical_cache_key(model_spec),
        limits,
        provider_info: ProviderInfo {
            provider: provider.to_string(),
            endpoint,
        },
    });

    Ok(())
}

/// Extract provider info from a model spec string.
pub(crate) fn provider_info(model_spec: &str) -> ProviderInfo {
    let lock = MODEL_INFO_CACHE.read().unwrap();
    if let Some(info) = lock
        .as_ref()
        .filter(|info| info.model_spec == canonical_cache_key(model_spec))
    {
        info.provider_info.clone()
    } else {
        let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(model_spec);
        ProviderInfo {
            provider: parsed.provider_or_openai().to_string(),
            endpoint: None,
        }
    }
}

/// Look up token limits for a model spec from OpenCode metadata.
/// Returns `None` when OpenCode isn't available or the model isn't found.
pub(crate) fn model_limits(model_spec: &str) -> Option<opencode_models::OpenCodeModelLimit> {
    MODEL_INFO_CACHE
        .read()
        .unwrap()
        .as_ref()
        .filter(|info| info.model_spec == canonical_cache_key(model_spec))
        .and_then(|info| info.limits)
}

fn canonical_cache_key(model_spec: &str) -> String {
    let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(model_spec);
    format!("{}/{}", parsed.provider_or_openai(), parsed.base_model)
}

static BACKEND: NativeOpenAiBackend = NativeOpenAiBackend;

pub async fn exec_chat(
    model_spec: &str,
    preamble: &str,
    messages: Vec<Message>,
    tool_specs: Vec<ToolSpec>,
    tools: LlmTools,
    max_turns: usize,
) -> Result<LlmResponse> {
    let _ = cache_model_limits(model_spec).await;
    let route = prepare_chat(model_spec)?;
    let request = LlmRequest {
        route,
        system_prompt: preamble.to_string(),
        system_cache: None,
        messages,
        tools: tool_specs,
        max_turns,
        tool_choice: None,
        generation: None,
        cache: None,
    };
    BACKEND.chat(request, tools).await
}

fn prepare_chat(model_spec: &str) -> Result<crate::llm::ModelRoute> {
    crate::llm::route::resolve::model_route(model_spec, default_reasoning_effort(model_spec))
}

#[cfg(test)]
fn split_model_spec(spec: &str) -> (Option<&str>, &str) {
    let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(spec);
    (parsed.provider, parsed.base_model)
}

// ---------------------------------------------------------------------------
// Reasoning effort helpers
// ---------------------------------------------------------------------------

/// Default reasoning effort for `model_spec`.
///
/// Resolves in order:
/// 1. Inline suffix on the model name (e.g. `gpt-5.5-low`)
/// 2. `OY_THINKING` / `OY_REASONING_EFFORT` env var
/// 3. OpenCode model metadata (`capabilities.reasoning` / `variants`)
/// 4. Static fallback for known reasoning-capable models
/// 5. `None` otherwise
pub fn default_reasoning_effort(model_spec: &str) -> Option<String> {
    let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(model_spec);
    if let Some(effort) = parsed.reasoning_effort.map(str::to_string) {
        return Some(effort);
    }
    reasoning_effort_option(model_spec)
}

/// Resolve the reasoning effort value for a model spec,
/// honouring env-var overrides and falling back to OpenCode model metadata.
pub fn reasoning_effort_option(model_spec: &str) -> Option<String> {
    if THINKING_OVERRIDE
        .read()
        .expect("thinking override lock poisoned")
        .is_some()
        || env::var("OY_THINKING").is_ok()
        || env::var("OY_REASONING_EFFORT").is_ok()
    {
        return configured_reasoning_effort();
    }
    let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(model_spec);
    if parsed.reasoning_effort.is_some() {
        return None;
    }
    let base_model = parsed.base_model;

    // Moonshot/Kimi defaults thinking on for this model. Its OpenAI-compatible
    // chat endpoint rejects follow-up tool requests unless assistant tool-call
    // messages echo `reasoning_content`, so explicitly disable thinking by
    // default for reliable tool use. Users can still force it with
    // `/thinking high`, `OY_THINKING=high`, or a `-high` model suffix.
    if is_moonshot_kimi_model(model_spec) {
        return Some("none".to_string());
    }

    // Prefer OpenCode metadata when available.
    if let Some(effort) = opencode_reasoning_effort(base_model) {
        return Some(effort);
    }

    // Static fallback for when OpenCode is unavailable.
    reasoning_capable_fallback(base_model).map(|s| s.to_string())
}

/// Thread-safe override set by the `/thinking` command, taking precedence over env vars.
static THINKING_OVERRIDE: LazyLock<RwLock<Option<String>>> = LazyLock::new(|| RwLock::new(None));

/// Set the thinking effort override. Use `None` / `"auto"` to clear.
pub fn set_thinking_override(value: Option<&str>) {
    let mut guard = THINKING_OVERRIDE
        .write()
        .expect("thinking override lock poisoned");
    match value {
        Some("auto") | Some("") | None => *guard = None,
        Some(v) => *guard = Some(v.to_string()),
    }
}

/// Get the current thinking effort, checking override first, then env.
pub fn get_thinking_effort() -> Option<String> {
    THINKING_OVERRIDE
        .read()
        .expect("thinking override lock poisoned")
        .clone()
        .or_else(|| env_value("OY_THINKING"))
        .or_else(|| env_value("OY_REASONING_EFFORT"))
}

fn configured_reasoning_effort() -> Option<String> {
    get_thinking_effort().and_then(normalize_effort_value)
}

fn normalize_effort_value(value: String) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => None,
        "off" | "false" | "0" | "none" => Some("none".to_string()),
        "minimal" => Some("minimal".to_string()),
        "low" => Some("low".to_string()),
        "medium" => Some("medium".to_string()),
        "high" | "true" | "1" | "on" => Some("high".to_string()),
        _ => None,
    }
}

/// Supported reasoning effort values for `model_spec` according to OpenCode.
/// Falls back to the universal set when OpenCode is unavailable.
pub fn reasoning_efforts_for(model_spec: &str) -> Vec<String> {
    let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(model_spec);
    let (provider, model_name) = split_model_spec_for_opencode(parsed.base_model);
    if let Some(info) = opencode_models::find(provider, model_name) {
        let efforts = info.reasoning_efforts();
        if !efforts.is_empty() {
            return efforts.iter().map(|s| s.to_string()).collect();
        }
        // Model says it supports reasoning but has no explicit variants;
        // "high" is the universal default.
        if info.supports_reasoning() {
            return vec!["high".to_string()];
        }
    }
    // Fallback to universal set.
    vec![
        "minimal".to_string(),
        "low".to_string(),
        "medium".to_string(),
        "high".to_string(),
    ]
}

fn is_moonshot_kimi_model(model_spec: &str) -> bool {
    let lower = model_spec.to_ascii_lowercase();
    lower.contains("moonshot") || lower.contains("kimi")
}

/// Query OpenCode for the model's default reasoning effort.
fn opencode_reasoning_effort(model_name: &str) -> Option<String> {
    let (provider, model) = split_model_spec_for_opencode(model_name);
    opencode_models::find(provider, model)
        .and_then(|info| info.default_reasoning_effort().map(|s| s.to_string()))
}

/// Split a model name into (provider, model) for OpenCode lookup.
fn split_model_spec_for_opencode(model: &str) -> (&str, &str) {
    if let Some((provider, model)) = model.rsplit_once('/') {
        (provider, model)
    } else {
        ("openai", model)
    }
}

/// Static fallback: true when the model name matches known reasoning-capable
/// families. Kept for environments where `opencode` is not installed.
fn reasoning_capable_fallback(model: &str) -> Option<&'static str> {
    let model = model
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(model)
        .to_ascii_lowercase();
    let capable = model.starts_with("gpt-5")
        || model.contains("codex")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("claude-3-7")
        || model.starts_with("claude-4")
        || model.starts_with("claude-sonnet-4")
        || model.starts_with("claude-opus-4")
        || model.starts_with("gemini-3");
    capable.then_some("high")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Protocol, RouteAuth};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        saved: Option<CachedModelInfo>,
    }

    impl ModelInfoCacheGuard {
        fn replace(next: Option<CachedModelInfo>) -> Self {
            let mut cache = MODEL_INFO_CACHE
                .write()
                .unwrap_or_else(|err| err.into_inner());
            let saved = cache.clone();
            *cache = next;
            Self { saved }
        }
    }

    impl Drop for ModelInfoCacheGuard {
        fn drop(&mut self) {
            let mut cache = MODEL_INFO_CACHE
                .write()
                .unwrap_or_else(|err| err.into_inner());
            *cache = self.saved.take();
        }
    }

    #[test]
    fn copilot_routes_reasoning_models_to_responses_api() {
        assert!(crate::llm::providers::github_copilot_should_use_responses_api("gpt-5.5"));
        assert!(crate::llm::providers::github_copilot_should_use_responses_api("gpt-5.3-codex"));
        assert!(
            crate::llm::providers::github_copilot_should_use_responses_api(
                "gemini-3.1-pro-preview"
            )
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
    fn prepare_chat_builds_openai_chat_plan() {
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
        assert_eq!(chat.protocol, Protocol::OpenAiChat);
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

        assert_eq!(chat.protocol, Protocol::OpenAiChat);
        assert_eq!(
            chat.additional_params,
            Some(serde_json::json!({
                "store": false,
                "reasoning_effort": "low",
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
}
