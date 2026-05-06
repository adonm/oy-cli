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
    #[serde(skip)]
    spec: String,
    #[serde(default)]
    id: String,
    #[serde(default, rename = "providerID")]
    provider_id: String,
    #[serde(default)]
    api: OpenCodeModelApi,
}

#[derive(Debug, Default, Clone, Deserialize)]
struct OpenCodeModelApi {
    id: Option<String>,
    url: Option<String>,
    npm: Option<String>,
}

impl OpenCodeModelListing {
    pub(crate) fn load() -> Result<Self> {
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
        parse_verbose(&String::from_utf8_lossy(&output.stdout))
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
            if !model.is_supported_by_rig() {
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

    pub(crate) fn is_openai_compatible_api(&self) -> bool {
        self.api.npm.as_deref().is_some_and(|api| {
            matches!(
                api,
                "@ai-sdk/openai" | "@ai-sdk/openai-compatible" | "@ai-sdk/github-copilot"
            )
        })
    }

    fn is_supported_by_rig(&self) -> bool {
        self.is_openai_compatible_api()
            || matches!(
                self.provider_id.as_str(),
                "bedrock" | "amazon-bedrock" | "vertexai"
            )
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
anthropic/claude-test
{
  "id": "claude-test",
  "providerID": "anthropic",
  "api": { "id": "claude-test", "url": "https://api.anthropic.com/v1", "npm": "@ai-sdk/anthropic" }
}
"#;
        let listing = parse_verbose(text).unwrap();
        let model = listing.find("github-copilot", "gpt-5.5").unwrap();
        assert_eq!(model.api_id(), "gpt-5.5");
        assert_eq!(model.api_url(), Some("https://api.githubcopilot.com"));
        let groups = listing.into_adapter_models();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].models(), &["github-copilot/gpt-5.5".to_string()]);
    }

    #[test]
    fn falls_back_to_spec_for_missing_ids() {
        let text = r#"bedrock/anthropic.test
{
  "api": { "id": "anthropic.test" }
}
"#;
        let listing = parse_verbose(text).unwrap();
        assert!(listing.find("bedrock", "anthropic.test").is_some());
    }
}
