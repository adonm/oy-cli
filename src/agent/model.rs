use crate::config;
use anyhow::{Context, Result, anyhow, bail};
use rig::agent::PromptResponse;
use rig::client::CompletionClient;
use rig::completion::{Message, Prompt};
use rig::providers::{copilot, openai};
use rig::tool::ToolDyn;
use serde::Serialize;
use std::env;

pub(crate) use super::auth::{AuthStatus, auth_statuses};
use super::auth::{GitHubCopilotAuth, env_value, github_copilot_auth, opencode_auth_key};
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

/// Information about the rig provider for a model spec.
#[derive(Debug, Clone)]
pub(crate) struct ProviderInfo {
    /// The rig provider module name (e.g. "openai", "github-copilot")
    pub provider: String,
    /// The API endpoint URL, if determinable from OpenCode metadata
    pub endpoint: Option<String>,
}

/// Extract provider info from a model spec string.
pub(crate) fn provider_info(model_spec: &str) -> ProviderInfo {
    let (provider, model) = split_model_spec(model_spec.trim());
    let provider = provider
        .map(config::canonical_provider)
        .unwrap_or("openai")
        .to_string();
    let model = model.to_string();
    let model_info = opencode_models::find(&provider, &model);
    let endpoint = model_info
        .as_ref()
        .and_then(|info| info.api_url().map(|s| s.trim_end_matches('/').to_string()));
    ProviderInfo {
        provider,
        endpoint,
    }
}

/// Look up token limits for a model spec from OpenCode metadata.
/// Returns `None` when OpenCode isn't available or the model isn't found.
pub(crate) fn model_limits(model_spec: &str) -> Option<opencode_models::OpenCodeModelLimit> {
    let (provider, model) = split_model_spec(model_spec.trim());
    let provider = provider.map(config::canonical_provider)?;
    opencode_models::lookup_limit(provider, model)
}

pub async fn exec_chat(
    model_spec: &str,
    preamble: &str,
    messages: Vec<Message>,
    tools: Vec<Box<dyn ToolDyn>>,
    max_turns: usize,
) -> Result<PromptResponse> {
    let reasoning_effort = reasoning_effort_json(model_spec);
    let route = resolve_chat_route(model_spec)?;
    let mut history = messages;
    let prompt = history.pop().unwrap_or_else(|| Message::user(""));
    execute_chat_route(
        route,
        preamble,
        history,
        prompt,
        tools,
        max_turns,
        reasoning_effort,
    )
    .await
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChatRoute {
    OpenAi {
        model: String,
        api_key: String,
        base_url: Option<String>,
    },
    GitHubCopilot {
        model: String,
        auth: GitHubCopilotAuth,
    },
    GitHubCopilotResponses {
        model: String,
        api_key: String,
        base_url: String,
    },
}

fn resolve_chat_route(model_spec: &str) -> Result<ChatRoute> {
    let (provider, model) = split_model_spec(model_spec.trim());
    let provider = provider.map(config::canonical_provider).unwrap_or("openai");
    match provider {
        "github-copilot" => {
            let model_info = opencode_models::find("github-copilot", model);
            let model_id = model_info
                .as_ref()
                .map(|model| model.api_id().to_string())
                .unwrap_or_else(|| model.to_string());
            let auth = github_copilot_auth().context("GitHub Copilot auth is not configured")?;
            if copilot_requires_responses_api_shim(&model_id) {
                let GitHubCopilotAuth::ApiKey(api_key) = auth else {
                    bail!(
                        "GitHub Copilot model `{model}` requires a Copilot API token, but only a GitHub token is configured"
                    );
                };
                Ok(ChatRoute::GitHubCopilotResponses {
                    model: model_id,
                    api_key,
                    base_url: model_info
                        .as_ref()
                        .and_then(|model| model.api_url())
                        .unwrap_or("https://api.githubcopilot.com")
                        .trim_end_matches("/v1")
                        .trim_end_matches('/')
                        .to_string(),
                })
            } else {
                Ok(ChatRoute::GitHubCopilot {
                    model: model_id,
                    auth,
                })
            }
        }
        "openai" => Ok(ChatRoute::OpenAi {
            model: model.to_string(),
            api_key: env_value("OPENAI_API_KEY").context("OpenAI auth is not configured")?,
            base_url: env_value("OPENAI_BASE_URL"),
        }),
        provider => {
            let model_info = opencode_models::find(provider, model)
                .ok_or_else(|| anyhow!("unknown OpenCode model `{provider}/{model}`"))?;
            if !model_info.is_openai_compatible_api() {
                bail!("OpenCode model `{provider}/{model}` is not OpenAI-compatible");
            }
            Ok(ChatRoute::OpenAi {
                model: model_info.api_id().to_string(),
                api_key: opencode_auth_key(provider).ok_or_else(|| {
                    anyhow!("OpenCode auth.json has no credentials for `{provider}`")
                })?,
                base_url: Some(
                    model_info
                        .api_url()
                        .ok_or_else(|| {
                            anyhow!(
                                "OpenCode model `{provider}/{model}` does not expose an API URL"
                            )
                        })?
                        .to_string(),
                ),
            })
        }
    }
}

async fn execute_chat_route(
    route: ChatRoute,
    preamble: &str,
    history: Vec<Message>,
    prompt: Message,
    tools: Vec<Box<dyn ToolDyn>>,
    max_turns: usize,
    reasoning_effort: Option<serde_json::Value>,
) -> Result<PromptResponse> {
    match route {
        ChatRoute::OpenAi {
            model,
            api_key,
            base_url,
        } => {
            let mut builder = openai::Client::builder().api_key(api_key);
            if let Some(base_url) = base_url {
                builder = builder.base_url(base_url);
            }
            let client = builder.build()?.completions_api();
            let mut agent_builder = client.agent(&model).preamble(preamble).tools(tools);
            if let Some(ref params) = reasoning_effort {
                agent_builder = agent_builder.additional_params(params.clone());
            }
            let agent = agent_builder.build();
            agent
                .prompt(prompt)
                .with_history(history)
                .max_turns(max_turns)
                .extended_details()
                .await
                .map_err(Into::into)
        }
        ChatRoute::GitHubCopilot { model, auth } => {
            let client = match auth {
                GitHubCopilotAuth::ApiKey(api_key) => {
                    copilot::Client::builder().api_key(api_key).build()?
                }
                GitHubCopilotAuth::GitHubAccessToken(token) => copilot::Client::builder()
                    .github_access_token(token)
                    .build()?,
            };
            let mut agent_builder = client.agent(&model).preamble(preamble).tools(tools);
            if let Some(ref params) = reasoning_effort {
                agent_builder = agent_builder.additional_params(params.clone());
            }
            let agent = agent_builder.build();
            agent
                .prompt(prompt)
                .with_history(history)
                .max_turns(max_turns)
                .extended_details()
                .await
                .map_err(Into::into)
        }
        ChatRoute::GitHubCopilotResponses {
            model,
            api_key,
            base_url,
        } => {
            let client = openai::Client::builder()
                .api_key(api_key)
                .base_url(base_url)
                .build()?;
            let mut agent_builder = client.agent(&model).preamble(preamble).tools(tools);
            if let Some(ref params) = reasoning_effort {
                agent_builder = agent_builder.additional_params(params.clone());
            }
            let agent = agent_builder.build();
            agent
                .prompt(prompt)
                .with_history(history)
                .max_turns(max_turns)
                .extended_details()
                .await
                .map_err(Into::into)
        }
    }
}

fn split_model_spec(spec: &str) -> (Option<&str>, &str) {
    let (namespace, model) = config::split_model_spec(spec);
    if namespace.is_some() {
        return (namespace, model);
    }
    if let Some((provider, model)) = spec.split_once('/')
        && !provider.trim().is_empty()
        && !model.trim().is_empty()
    {
        return (Some(provider), model);
    }
    (None, spec)
}

fn copilot_requires_responses_api_shim(model: &str) -> bool {
    // Rig 0.36 routes only `*codex*` Copilot models to `/responses`.
    // Newer Copilot reasoning models also require `/responses`, so keep
    // this local compatibility shim until Rig exposes metadata-based routing
    // or expands its own Copilot route rule.
    let model = model.to_ascii_lowercase();
    model.contains("codex") || model.starts_with("gpt-5") || model.starts_with("gemini-3")
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
    let (_, model) = config::split_model_spec(model_spec);
    let (inline_effort, _) = split_reasoning_effort_suffix(model);
    if let Some(effort) = inline_effort.map(|s| s.to_string()) {
        return Some(effort);
    }
    reasoning_effort_option(model_spec)
}

/// Resolve the reasoning effort value for a model spec,
/// honouring env-var overrides and falling back to OpenCode model metadata.
pub fn reasoning_effort_option(model_spec: &str) -> Option<String> {
    if env::var("OY_THINKING").is_ok() || env::var("OY_REASONING_EFFORT").is_ok() {
        return configured_reasoning_effort();
    }
    let (_, model) = config::split_model_spec(model_spec);
    let (inline_effort, base_model) = split_reasoning_effort_suffix(model);
    if inline_effort.is_some() {
        return None;
    }

    // Prefer OpenCode metadata when available.
    if let Some(effort) = opencode_reasoning_effort(base_model) {
        return Some(effort);
    }

    // Static fallback for when OpenCode is unavailable.
    reasoning_capable_fallback(base_model).map(|s| s.to_string())
}

fn configured_reasoning_effort() -> Option<String> {
    env_value("OY_THINKING")
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
    let (_, model) = config::split_model_spec(model_spec);
    let (_, base_model) = split_reasoning_effort_suffix(model);
    let (provider, model_name) = split_model_spec_for_opencode(base_model);
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
fn reasoning_effort_json(model_spec: &str) -> Option<serde_json::Value> {
    let effort = default_reasoning_effort(model_spec)?;
    Some(serde_json::json!({"reasoning_effort": effort}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
    fn reasoning_defaults_to_high_for_capable_models_and_allows_suffix_override() {
        assert_eq!(default_reasoning_effort("gpt-5.5").as_deref(), Some("high"));
        assert_eq!(
            default_reasoning_effort("copilot::gpt-5.5-low").as_deref(),
            Some("low")
        );
        assert_eq!(default_reasoning_effort("gpt-4.1-mini"), None);
    }

    #[test]
    fn reasoning_env_override_can_disable_or_adjust() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        unsafe { std::env::set_var("OY_THINKING", "none") };
        assert_eq!(default_reasoning_effort("gpt-5.5").as_deref(), Some("none"));
        unsafe { std::env::set_var("OY_THINKING", "medium") };
        assert_eq!(
            default_reasoning_effort("gpt-5.5").as_deref(),
            Some("medium")
        );
        unsafe { std::env::remove_var("OY_THINKING") };
    }

    #[test]
    fn reasoning_effort_json_uses_flat_param() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let json = reasoning_effort_json("gpt-5.5");
        assert_eq!(json, Some(serde_json::json!({"reasoning_effort": "high"})));
        assert_eq!(reasoning_effort_json("gpt-4.1-mini"), None);
    }
}
