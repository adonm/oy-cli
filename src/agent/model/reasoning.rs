//! Reasoning-effort selection and overrides.

use std::env;
use std::sync::{LazyLock, RwLock};

use super::super::auth::env_value;
use super::super::opencode_models;

/// Default reasoning effort for `model_spec`.
///
/// Resolves in order:
/// 1. Inline suffix on the model name (e.g. `gpt-5.5-low`)
/// 2. `OY_THINKING` / `OY_REASONING_EFFORT` env var
/// 3. OpenCode model metadata (`capabilities.reasoning` / `variants`)
/// 4. Static fallback for known reasoning-capable models
/// 5. `None` otherwise
pub fn default_reasoning_effort(model_spec: &str) -> Option<String> {
    let parsed = crate::llm::ParsedModelSpec::parse(model_spec);
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
    let parsed = crate::llm::ParsedModelSpec::parse(model_spec);
    if parsed.reasoning_effort.is_some() {
        return None;
    }
    let base_model = parsed.base_model;
    let provider = parsed.provider_or_openai();

    // Moonshot/Kimi defaults thinking on for this model. Its OpenAI-compatible
    // chat endpoint rejects follow-up tool requests unless assistant tool-call
    // messages echo `reasoning_content`, so explicitly disable thinking by
    // default for reliable tool use. Users can still force it with
    // `/thinking high`, `OY_THINKING=high`, or a `-high` model suffix.
    if is_moonshot_kimi_model(model_spec) {
        return Some("none".to_string());
    }

    // Prefer OpenCode metadata when available.
    if let Some(effort) = opencode_reasoning_effort(provider, base_model) {
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
    let parsed = crate::llm::ParsedModelSpec::parse(model_spec);
    let provider = parsed.provider_or_openai();
    let model_name = parsed.base_model;
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
fn opencode_reasoning_effort(provider: &str, model_name: &str) -> Option<String> {
    opencode_models::find(provider, model_name)
        .and_then(|info| info.default_reasoning_effort().map(|s| s.to_string()))
}

/// Static fallback: true when the model name matches known reasoning-capable
/// families. Kept for environments where `opencode` is not installed.
pub(crate) fn reasoning_capable_fallback(model: &str) -> Option<&'static str> {
    let model = model.to_ascii_lowercase();
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
