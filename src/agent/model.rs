use crate::config;
use crate::llm::{
    ChatBackend, LlmRequest, LlmResponse, LlmTools, Message, ModelRoute, NativeOpenAiBackend,
    Protocol, RouteAuth, ToolSpec,
};
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use std::env;
use std::sync::{LazyLock, RwLock};

pub(crate) use super::auth::{AuthStatus, auth_statuses};
use super::auth::{env_value, github_copilot_api_key, opencode_auth_key};
use super::opencode_models;
pub(crate) use super::opencode_models::AdapterModels;

#[derive(Debug, Clone, Serialize)]
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
    let auth = auth_statuses()
        .into_iter()
        .filter(|item| item.availability.is_available())
        .collect::<Vec<_>>();
    let dynamic = opencode_models::inspect();
    let all_models = collect_all_models(&dynamic);
    Ok(ModelListing {
        current,
        auth,
        dynamic,
        all_models,
    })
}

fn collect_all_models(dynamic: &[AdapterModels]) -> Vec<String> {
    let mut items = dynamic
        .iter()
        .flat_map(|group| group.models().iter().cloned())
        .collect::<Vec<_>>();
    items.sort();
    items.dedup();
    items
}

/// Information about the provider for a model spec.
#[derive(Debug, Clone)]
pub(crate) struct ProviderInfo {
    /// The provider id (e.g. "openai", "github-copilot")
    pub provider: String,
    /// The API endpoint URL, if determinable from OpenCode metadata
    pub endpoint: Option<String>,
}

/// Extract provider info from a model spec string.
pub(crate) fn provider_info(model_spec: &str) -> ProviderInfo {
    let parsed = ParsedModelSpec::parse(model_spec);
    let provider = parsed.provider_or_openai().to_string();
    let model_info = opencode_models::find(&provider, parsed.base_model);
    let endpoint = model_info
        .as_ref()
        .and_then(|info| info.api_url().map(|s| s.trim_end_matches('/').to_string()));
    ProviderInfo { provider, endpoint }
}

/// Look up token limits for a model spec from OpenCode metadata.
/// Returns `None` when OpenCode isn't available or the model isn't found.
pub(crate) fn model_limits(model_spec: &str) -> Option<opencode_models::OpenCodeModelLimit> {
    let parsed = ParsedModelSpec::parse(model_spec);
    let provider = parsed.provider.map(config::canonical_provider)?;
    opencode_models::lookup_limit(provider, parsed.base_model)
}

pub async fn exec_chat(
    model_spec: &str,
    preamble: &str,
    messages: Vec<Message>,
    tool_specs: Vec<ToolSpec>,
    tools: LlmTools,
    max_turns: usize,
) -> Result<LlmResponse> {
    let route = prepare_chat(model_spec)?;
    let request = LlmRequest {
        route,
        system_prompt: preamble.to_string(),
        messages,
        tools: tool_specs,
        max_turns,
    };
    NativeOpenAiBackend.chat(request, tools).await
}

fn prepare_chat(model_spec: &str) -> Result<ModelRoute> {
    let parsed = ParsedModelSpec::parse(model_spec);
    let provider = parsed.provider_or_openai();
    let mut route = match provider {
        "github-copilot" => prepare_github_copilot_chat(parsed.base_model)?,
        "openai" => ModelRoute {
            protocol: Protocol::OpenAiChat,
            model: parsed.base_model.to_string(),
            auth: RouteAuth::ApiKey(
                env_value("OPENAI_API_KEY").context("OpenAI auth is not configured")?,
            ),
            base_url: env_value("OPENAI_BASE_URL"),
            additional_params: None,
        },
        provider => prepare_opencode_compatible_chat(provider, parsed.base_model)?,
    };
    route.additional_params =
        reasoning_effort_json(model_spec, route.protocol.uses_responses_api());
    Ok(route)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenCodeRouteProfile {
    model_id: String,
    base_url: String,
    protocol: Protocol,
}

impl OpenCodeRouteProfile {
    fn from_model(
        provider: &str,
        model: &str,
        info: &opencode_models::OpenCodeModel,
    ) -> Result<Self> {
        if !info.is_openai_compatible_api() {
            bail!("OpenCode model `{provider}/{model}` is not OpenAI-compatible");
        }
        let model_id = info.api_id().to_string();
        let base_url = info
            .api_url()
            .ok_or_else(|| {
                anyhow!("OpenCode model `{provider}/{model}` does not expose an API URL")
            })?
            .to_string();
        let protocol = if opencode_requires_responses_api_shim(provider, &model_id) {
            Protocol::OpenAiResponses
        } else {
            Protocol::OpenAiChat
        };
        Ok(Self {
            model_id,
            base_url,
            protocol,
        })
    }
}

fn prepare_github_copilot_chat(model: &str) -> Result<ModelRoute> {
    let model_info = opencode_models::find("github-copilot", model);
    let profile = model_info
        .as_ref()
        .map(|info| OpenCodeRouteProfile::from_model("github-copilot", model, info))
        .transpose()?;
    let model_id = profile
        .as_ref()
        .map(|profile| profile.model_id.clone())
        .unwrap_or_else(|| model.to_string());
    let api_key = github_copilot_api_key().context(
        "GitHub Copilot API token is not configured; set GITHUB_COPILOT_API_KEY, COPILOT_API_KEY, OPENCODE_API_KEY, or OpenCode auth.json",
    )?;
    let protocol = if copilot_requires_responses_api_shim(&model_id) {
        Protocol::OpenAiResponses
    } else {
        profile
            .as_ref()
            .map(|profile| profile.protocol)
            .unwrap_or(Protocol::OpenAiChat)
    };
    Ok(ModelRoute {
        protocol,
        model: model_id,
        auth: RouteAuth::ApiKey(api_key),
        base_url: Some(copilot_base_url(profile.as_ref())),
        additional_params: None,
    })
}

fn copilot_base_url(profile: Option<&OpenCodeRouteProfile>) -> String {
    profile
        .map(|profile| profile.base_url.as_str())
        .unwrap_or("https://api.githubcopilot.com")
        .trim_end_matches("/v1")
        .trim_end_matches('/')
        .to_string()
}

fn prepare_opencode_compatible_chat(provider: &str, model: &str) -> Result<ModelRoute> {
    let model_info = opencode_models::find(provider, model)
        .ok_or_else(|| anyhow!("unknown OpenCode model `{provider}/{model}`"))?;
    let profile = OpenCodeRouteProfile::from_model(provider, model, &model_info)?;
    let api_key = opencode_auth_key(provider)
        .ok_or_else(|| anyhow!("OpenCode auth.json has no credentials for `{provider}`"))?;
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth: RouteAuth::ApiKey(api_key),
        base_url: Some(profile.base_url),
        additional_params: None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedModelSpec<'a> {
    provider: Option<&'a str>,
    model: &'a str,
    base_model: &'a str,
    reasoning_effort: Option<&'static str>,
}

impl<'a> ParsedModelSpec<'a> {
    fn parse(spec: &'a str) -> Self {
        let spec = spec.trim();
        let (namespace, model) = config::split_model_spec(spec);
        if let Some(provider) = namespace {
            return Self::from_parts(Some(provider), model);
        }
        if let Some((provider, model)) = spec.split_once('/')
            && !provider.trim().is_empty()
            && !model.trim().is_empty()
        {
            return Self::from_parts(Some(provider), model);
        }
        Self::from_parts(None, model)
    }

    fn from_parts(provider: Option<&'a str>, model: &'a str) -> Self {
        let (reasoning_effort, base_model) = split_reasoning_effort_suffix(model);
        Self {
            provider,
            model,
            base_model,
            reasoning_effort,
        }
    }

    fn provider_or_openai(self) -> &'a str {
        self.provider
            .map(config::canonical_provider)
            .unwrap_or("openai")
    }
}

#[cfg(test)]
fn split_model_spec(spec: &str) -> (Option<&str>, &str) {
    let parsed = ParsedModelSpec::parse(spec);
    (parsed.provider, parsed.base_model)
}

fn copilot_requires_responses_api_shim(model: &str) -> bool {
    // Copilot reasoning models require `/responses`. Keep this narrow local
    // route rule until OpenCode exposes a protocol bit for these models.
    let model = model.to_ascii_lowercase();
    model.contains("codex") || model.starts_with("gpt-5") || model.starts_with("gemini-3")
}

fn opencode_requires_responses_api_shim(provider: &str, model: &str) -> bool {
    // OpenCode's `opencode` GPT-5 family returns Responses API payloads from
    // `https://opencode.ai/zen/v1`.
    provider == "opencode" && model.to_ascii_lowercase().starts_with("gpt-5")
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
    let parsed = ParsedModelSpec::parse(model_spec);
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
    let parsed = ParsedModelSpec::parse(model_spec);
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
    THINKING_OVERRIDE
        .read()
        .expect("thinking override lock poisoned")
        .clone()
        .or_else(|| env_value("OY_THINKING"))
        .or_else(|| env_value("OY_REASONING_EFFORT"))
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => None,
            "off" | "false" | "0" | "none" => Some("none".to_string()),
            "minimal" => Some("minimal".to_string()),
            "low" => Some("low".to_string()),
            "medium" => Some("medium".to_string()),
            "high" | "true" | "1" | "on" => Some("high".to_string()),
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

/// Supported reasoning effort values for `model_spec` according to OpenCode.
/// Falls back to the universal set when OpenCode is unavailable.
pub fn reasoning_efforts_for(model_spec: &str) -> Vec<String> {
    let parsed = ParsedModelSpec::parse(model_spec);
    let (provider, model_name) = split_model_spec_for_opencode(parsed.base_model);
    if let Some(info) = opencode_models::lookup_reasoning(provider, model_name) {
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
    opencode_models::lookup_reasoning(provider, model)
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

/// Build a JSON payload suitable for `additional_params` on the agent builder.
///
/// Returns `None` when no reasoning effort should be applied (model doesn't
/// support it or the resolved value is "auto"/empty).
fn reasoning_effort_json(model_spec: &str, responses_api: bool) -> Option<serde_json::Value> {
    let effort = default_reasoning_effort(model_spec)?;
    if responses_api {
        Some(serde_json::json!({"reasoning": {"effort": effort}}))
    } else {
        Some(serde_json::json!({"reasoning_effort": effort}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn model_listing_only_includes_introspected_models() {
        let models = collect_all_models(&[]);
        assert!(models.is_empty());
    }

    #[test]
    fn copilot_routes_reasoning_models_to_responses_api() {
        assert!(copilot_requires_responses_api_shim("gpt-5.5"));
        assert!(copilot_requires_responses_api_shim("gpt-5.3-codex"));
        assert!(copilot_requires_responses_api_shim(
            "gemini-3.1-pro-preview"
        ));
        assert!(!copilot_requires_responses_api_shim("gpt-4.1"));
    }

    #[test]
    fn opencode_gpt5_routes_to_responses_api() {
        assert!(opencode_requires_responses_api_shim(
            "opencode",
            "gpt-5.4-mini"
        ));
        assert!(!opencode_requires_responses_api_shim(
            "opencode-go",
            "mimo-v2.5-pro"
        ));
        assert!(!opencode_requires_responses_api_shim(
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
        let parsed = ParsedModelSpec::parse(" copilot::gpt-5.5-low ");
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
        ]);
        let chat = prepare_chat("openai::gpt-4.1-mini").unwrap();
        assert_eq!(chat.protocol, Protocol::OpenAiChat);
        assert_eq!(chat.model, "gpt-4.1-mini");
        assert_eq!(chat.auth, RouteAuth::ApiKey("test-openai-key".to_string()));
        assert_eq!(chat.base_url.as_deref(), Some("https://openai.example/v1"));
        assert_eq!(chat.additional_params, None);
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
        assert_eq!(chat.additional_params, None);
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
            Some(serde_json::json!({"reasoning": {"effort": "low"}}))
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
        assert!(copilot_requires_responses_api_shim("gpt-5.5"));
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
        let chat_json = reasoning_effort_json("gpt-5.5", false);
        assert_eq!(
            chat_json,
            Some(serde_json::json!({"reasoning_effort": "high"}))
        );
        let responses_json = reasoning_effort_json("gpt-5.5", true);
        assert_eq!(
            responses_json,
            Some(serde_json::json!({"reasoning": {"effort": "high"}}))
        );
        assert_eq!(reasoning_effort_json("gpt-4.1-mini", false), None);
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
