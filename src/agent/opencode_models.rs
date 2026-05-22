//! OpenCode model metadata queries: token limits, reasoning capability,
//! and supported effort levels.
//!
//! This is the only source of OpenCode verbose model metadata; do not
//! add local provider/model registries. See [`AdapterModels`] for the
//! parsed listing shape and the free functions for cached lookups.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub(crate) const MODELS_SOURCE: &str = "opencode models --verbose";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum AdapterModels {
    Available {
        adapter: String,
        source: String,
        count: usize,
        models: Vec<String>,
    },
    Failed {
        adapter: String,
        source: String,
        error: String,
    },
}

impl AdapterModels {
    pub fn models(&self) -> &[String] {
        match self {
            Self::Available { models, .. } => models,
            Self::Failed { .. } => &[],
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OpenCodeModelListing {
    models: Vec<OpenCodeModel>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenCodeModel {
    #[serde(default)]
    spec: String,
    #[serde(default)]
    id: String,
    #[serde(default, rename = "providerID")]
    provider_id: String,
    #[serde(default)]
    api: OpenCodeModelApi,
    #[serde(default)]
    capabilities: OpenCodeCapabilities,
    #[serde(default)]
    variants: std::collections::HashMap<String, OpenCodeVariant>,
    #[serde(default)]
    limit: OpenCodeModelLimit,
}

#[derive(Debug, Default, Clone, Deserialize)]
struct OpenCodeModelApi {
    id: Option<String>,
    url: Option<String>,
    npm: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenCodeCapabilities {
    #[serde(default)]
    pub reasoning: bool,
}

/// Token limits reported by OpenCode for a model.
#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenCodeModelLimit {
    /// Total context window in tokens.
    #[serde(default)]
    pub context: usize,
    /// Max input tokens (may be less than context).
    #[serde(default)]
    pub input: Option<usize>,
    /// Max output tokens.
    #[serde(default)]
    pub output: usize,
}

// Value type is unused; we only inspect variant keys.
type OpenCodeVariant = serde_json::Value;

use std::sync::{LazyLock, RwLock};

static MODELS_CACHE: LazyLock<RwLock<Option<OpenCodeModelListing>>> =
    LazyLock::new(|| RwLock::new(None));

pub(crate) fn populate_cache(listing: OpenCodeModelListing) {
    let mut cache = MODELS_CACHE.write().unwrap();
    *cache = Some(listing);
}

impl OpenCodeModelListing {
    pub(crate) fn load() -> Result<Self> {
        if let Some(cached) = MODELS_CACHE.read().unwrap().as_ref() {
            return Ok(cached.clone());
        }

        let output = std::process::Command::new("opencode")
            .arg("models")
            .arg("--verbose")
            .output()
            .context("failed to run `opencode models --verbose`")?;
        if !output.status.success() {
            bail!(
                "`opencode models --verbose` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let listing = parse_verbose(&String::from_utf8_lossy(&output.stdout))?;
        populate_cache(listing.clone());
        Ok(listing)
    }

    pub(crate) fn find(&self, provider: &str, model: &str) -> Option<&OpenCodeModel> {
        self.models
            .iter()
            .find(|item| item.provider_id == provider && item.id == model)
            .or_else(|| {
                let spec = format!("{provider}/{model}");
                self.models.iter().find(|item| item.spec == spec)
            })
    }

    pub(crate) fn into_adapter_models(self) -> Vec<AdapterModels> {
        let mut groups = std::collections::BTreeMap::<String, Vec<String>>::new();
        for model in self.models {
            if !model.is_supported_by_native_openai() {
                continue;
            }
            groups
                .entry(model.provider_id)
                .or_default()
                .push(model.spec);
        }
        groups
            .into_iter()
            .map(|(adapter, mut models)| {
                models.sort();
                models.dedup();
                AdapterModels::Available {
                    adapter,
                    source: MODELS_SOURCE.to_string(),
                    count: models.len(),
                    models,
                }
            })
            .collect()
    }
}

impl OpenCodeModel {
    pub(crate) fn api_id(&self) -> &str {
        self.api.id.as_deref().unwrap_or(&self.id)
    }

    pub(crate) fn api_url(&self) -> Option<&str> {
        self.api
            .url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
    }

    /// Whether OpenCode reports this model as reasoning-capable.
    pub(crate) fn supports_reasoning(&self) -> bool {
        self.capabilities.reasoning
    }

    /// Supported reasoning effort levels, derived from the model's variants.
    /// Returns an empty slice when the model has no reasoning variants
    /// (capabilities.reasoning may still be true).
    pub(crate) fn reasoning_efforts(&self) -> Vec<&str> {
        let mut efforts: Vec<&str> = self
            .variants
            .keys()
            .map(|s| s.as_str())
            .filter(|k| {
                // Only include keys that map to known reasoning effort values
                matches!(*k, "none" | "minimal" | "low" | "medium" | "high")
            })
            .collect();
        efforts.sort();
        efforts
    }

    /// Default reasoning effort from OpenCode metadata.
    /// Returns `Some("high")` when the model is reasoning-capable and
    /// "high" is a supported variant, otherwise the first available variant,
    /// otherwise `None`.
    pub(crate) fn default_reasoning_effort(&self) -> Option<&str> {
        if !self.supports_reasoning() {
            return None;
        }
        let efforts = self.reasoning_efforts();
        if efforts.is_empty() {
            // Model says it supports reasoning but has no explicit variants;
            // "high" is the safe default.
            Some("high")
        } else if efforts.contains(&"high") {
            Some("high")
        } else {
            efforts.first().copied()
        }
    }

    pub(crate) fn is_openai_compatible_api(&self) -> bool {
        self.api.npm.as_deref().is_some_and(|api| {
            matches!(
                api,
                "@ai-sdk/openai"
                    | "@ai-sdk/openai-compatible"
                    | "@ai-sdk/github-copilot"
                    | "@ai-sdk/anthropic"
            )
        })
    }

    pub(crate) fn is_bedrock_api(&self) -> bool {
        self.api.npm.as_deref() == Some("@ai-sdk/amazon-bedrock")
            || matches!(self.provider_id.as_str(), "bedrock" | "amazon-bedrock")
    }

    fn is_supported_by_native_openai(&self) -> bool {
        if self.provider_id == "vertexai" {
            return false;
        }
        self.is_openai_compatible_api() || self.is_bedrock_api()
    }
}

pub(crate) fn parse_verbose(text: &str) -> Result<OpenCodeModelListing> {
    let mut lines = text.lines().peekable();
    let mut models = Vec::new();
    while let Some(line) = lines.next() {
        let spec = line.trim();
        if spec.is_empty() || spec.starts_with('{') || !spec.contains('/') {
            continue;
        }
        let mut json_lines = Vec::new();
        let mut depth = 0isize;
        while let Some(line) = lines.peek().copied() {
            if json_lines.is_empty() && !line.trim_start().starts_with('{') {
                break;
            }
            let line = lines.next().unwrap_or_default();
            depth += line.matches('{').count() as isize;
            depth -= line.matches('}').count() as isize;
            json_lines.push(line);
            if depth == 0 && !json_lines.is_empty() {
                break;
            }
        }
        if json_lines.is_empty() {
            continue;
        }
        let mut model: OpenCodeModel = serde_json::from_str(&json_lines.join("\n"))
            .with_context(|| format!("failed parsing OpenCode model metadata for {spec}"))?;
        model.spec = spec.to_string();
        if model.id.trim().is_empty() {
            model.id = spec
                .rsplit_once('/')
                .map(|(_, id)| id)
                .unwrap_or(spec)
                .to_string();
        }
        if model.provider_id.trim().is_empty() {
            model.provider_id = spec
                .split_once('/')
                .map(|(provider, _)| provider)
                .unwrap_or("")
                .to_string();
        }
        models.push(model);
    }
    Ok(OpenCodeModelListing { models })
}

pub(crate) fn inspect() -> Vec<AdapterModels> {
    match OpenCodeModelListing::load() {
        Ok(listing) => listing.into_adapter_models(),
        Err(err) => vec![AdapterModels::Failed {
            adapter: "opencode".to_string(),
            source: MODELS_SOURCE.to_string(),
            error: err.to_string(),
        }],
    }
}

pub(crate) fn find(provider: &str, model: &str) -> Option<OpenCodeModel> {
    OpenCodeModelListing::load()
        .ok()?
        .find(provider, model)
        .cloned()
}

/// Look up token limits for a model from OpenCode.
/// Returns `None` when the listing can't be loaded or the model isn't found,
/// or when the model reports no context limit.
pub(crate) fn lookup_limit(provider: &str, model: &str) -> Option<OpenCodeModelLimit> {
    let limit = OpenCodeModelListing::load()
        .ok()?
        .find(provider, model)?
        .limit;
    if limit.context == 0 {
        None
    } else {
        Some(limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_opencode_verbose_models() {
        let text = r#"github-copilot/gpt-5.5
{
  "id": "gpt-5.5",
  "providerID": "github-copilot",
  "api": { "id": "gpt-5.5", "url": "https://api.githubcopilot.com", "npm": "@ai-sdk/github-copilot" }
}
opencode/claude-test
{
  "id": "claude-test",
  "providerID": "opencode",
  "api": { "id": "claude-test", "url": "https://opencode.ai/zen/v1", "npm": "@ai-sdk/anthropic" }
}
"#;
        let listing = parse_verbose(text).unwrap();
        let model = listing.find("github-copilot", "gpt-5.5").unwrap();
        assert_eq!(model.api_id(), "gpt-5.5");
        assert_eq!(model.api_url(), Some("https://api.githubcopilot.com"));
        let groups = listing.into_adapter_models();
        // Both models are OpenAI-compatible (github-copilot + opencode proxying anthropic)
        assert_eq!(groups.len(), 2);
        // groups are sorted by adapter name
        assert_eq!(groups[0].models(), &["github-copilot/gpt-5.5".to_string()]);
        assert_eq!(groups[1].models(), &["opencode/claude-test".to_string()]);
    }

    #[test]
    fn falls_back_to_spec_for_missing_ids() {
        let text = r#"anthropic/anthropic.test
{
  "api": { "id": "anthropic.test" }
}
"#;
        let listing = parse_verbose(text).unwrap();
        assert!(listing.find("anthropic", "anthropic.test").is_some());
    }

    #[test]
    fn filters_google_models_until_native_protocol_is_supported() {
        let text = r#"google/gemini-3-flash
{
  "id": "gemini-3-flash",
  "providerID": "google",
  "api": { "id": "gemini-3-flash", "npm": "@ai-sdk/google" }
}
github-copilot/gpt-5.5
{
  "id": "gpt-5.5",
  "providerID": "github-copilot",
  "api": { "id": "gpt-5.5", "npm": "@ai-sdk/github-copilot" }
}
"#;
        let listing = parse_verbose(text).unwrap();
        let groups = listing.into_adapter_models();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].models(), &["github-copilot/gpt-5.5".to_string()]);
    }

    #[test]
    fn includes_bedrock_but_filters_vertexai_models() {
        let text = r#"bedrock/anthropic.claude-sonnet-4
{
  "id": "anthropic.claude-sonnet-4",
  "providerID": "bedrock",
  "api": { "id": "anthropic.claude-sonnet-4", "npm": "@ai-sdk/amazon-bedrock" }
}
amazon-bedrock/anthropic.claude-opus-4
{
  "id": "anthropic.claude-opus-4",
  "providerID": "amazon-bedrock",
  "api": { "id": "anthropic.claude-opus-4", "npm": "@ai-sdk/amazon-bedrock" }
}
vertexai/gemini-3.1-pro
{
  "id": "gemini-3.1-pro",
  "providerID": "vertexai",
  "api": { "id": "gemini-3.1-pro", "npm": "@ai-sdk/vertexai" }
}
github-copilot/gpt-5.5
{
  "id": "gpt-5.5",
  "providerID": "github-copilot",
  "api": { "id": "gpt-5.5", "npm": "@ai-sdk/github-copilot" }
}
"#;
        let listing = parse_verbose(text).unwrap();
        let groups = listing.into_adapter_models();
        assert_eq!(groups.len(), 3);
        assert_eq!(
            groups[0].models(),
            &["amazon-bedrock/anthropic.claude-opus-4".to_string()]
        );
        assert_eq!(
            groups[1].models(),
            &["bedrock/anthropic.claude-sonnet-4".to_string()]
        );
        assert_eq!(groups[2].models(), &["github-copilot/gpt-5.5".to_string()]);
    }
}
