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
pub(crate) use super::opencode_models::AdapterModels;
use super::opencode_models::{self, OpenCodeModelListing};

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
        let model = legacy_saved_model_spec(&model, saved.shim.as_deref());
        return Ok(config::canonical_model_spec(&model));
    }
    bail!(no_model_message())
}

fn no_model_message() -> String {
    [
        "No model configured.",
        "Run `oy model` to inspect auth-backed model endpoints.",
        "Then run: oy \"inspect this repo\"",
        "Advanced: use `oy model` to list options or set OY_MODEL for one run.",
    ]
    .join("\n")
}

fn legacy_saved_model_spec(model: &str, shim: Option<&str>) -> String {
    let model = model.trim();
    let Some(shim) = shim.map(str::trim).filter(|shim| !shim.is_empty()) else {
        return model.to_string();
    };
    let (prefix, bare_model) = config::split_model_spec(model);
    if prefix == Some("openai_resp") {
        return format!("{shim}::{bare_model}");
    }
    if prefix.is_some() || model.contains('/') {
        return model.to_string();
    }
    format!("{shim}::{model}")
}

pub async fn inspect_models() -> Result<ModelListing> {
    let current = resolve_model(None).ok();
    let auth = auth_statuses()
        .into_iter()
        .filter(|item| item.availability.is_available())
        .collect::<Vec<_>>();
    let dynamic = inspect_opencode_models();
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

pub async fn exec_chat(
    model_spec: &str,
    preamble: &str,
    messages: Vec<Message>,
    tools: Vec<Box<dyn ToolDyn>>,
    max_turns: usize,
) -> Result<PromptResponse> {
    let route = resolve_chat_route(model_spec)?;
    let mut history = messages;
    let prompt = history.pop().unwrap_or_else(|| Message::user(""));
    execute_chat_route(route, preamble, history, prompt, tools, max_turns).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChatRoute {
    OpenAi {
        model: String,
        api_key: String,
        base_url: Option<String>,
    },
    OpenAiCompatible {
        model: String,
        api_key: String,
        base_url: String,
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
    Bedrock {
        model: String,
    },
    VertexAi {
        model: String,
    },
}

fn resolve_chat_route(model_spec: &str) -> Result<ChatRoute> {
    let (provider, model) = split_model_spec(model_spec.trim());
    let provider = provider.map(config::canonical_provider).unwrap_or("openai");
    match provider {
        "github-copilot" => {
            let model_info = opencode_model("github-copilot", model);
            let model_id = model_info
                .as_ref()
                .map(|model| model.api_id().to_string())
                .unwrap_or_else(|| model.to_string());
            let auth = github_copilot_auth().context("GitHub Copilot auth is not configured")?;
            if copilot_uses_responses_api(&model_id) {
                let GitHubCopilotAuth::ApiKey(api_key) = auth else {
                    bail!("GitHub Copilot model `{model}` requires a Copilot API token, but only a GitHub token is configured");
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
        "amazon-bedrock" => Ok(ChatRoute::Bedrock {
            model: opencode_api_id("bedrock", model),
        }),
        "vertexai" => Ok(ChatRoute::VertexAi {
            model: opencode_api_id("vertexai", model),
        }),
        "openai" => Ok(ChatRoute::OpenAi {
            model: model.to_string(),
            api_key: env_value("OPENAI_API_KEY").context("OpenAI auth is not configured")?,
            base_url: env_value("OPENAI_BASE_URL"),
        }),
        provider => {
            let model_info = opencode_model(provider, model)
                .ok_or_else(|| anyhow!("unknown OpenCode model `{provider}/{model}`"))?;
            if !model_info.is_openai_compatible_api() {
                bail!("OpenCode model `{provider}/{model}` is not OpenAI-compatible");
            }
            Ok(ChatRoute::OpenAiCompatible {
                model: model_info.api_id().to_string(),
                api_key: opencode_auth_key(provider).ok_or_else(|| {
                    anyhow!("OpenCode auth.json has no credentials for `{provider}`")
                })?,
                base_url: model_info
                    .api_url()
                    .ok_or_else(|| {
                        anyhow!("OpenCode model `{provider}/{model}` does not expose an API URL")
                    })?
                    .to_string(),
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
            let agent = client.agent(&model).preamble(preamble).tools(tools).build();
            agent
                .prompt(prompt)
                .with_history(history)
                .max_turns(max_turns)
                .extended_details()
                .await
                .map_err(Into::into)
        }
        ChatRoute::OpenAiCompatible {
            model,
            api_key,
            base_url,
        } => {
            let client = openai::Client::builder()
                .api_key(api_key)
                .base_url(base_url)
                .build()?
                .completions_api();
            let agent = client.agent(&model).preamble(preamble).tools(tools).build();
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
                GitHubCopilotAuth::ApiKey(api_key) => copilot::Client::builder().api_key(api_key).build()?,
                GitHubCopilotAuth::GitHubAccessToken(token) => copilot::Client::builder()
                    .github_access_token(token)
                    .build()?,
            };
            let agent = client.agent(&model).preamble(preamble).tools(tools).build();
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
            let agent = client.agent(&model).preamble(preamble).tools(tools).build();
            agent
                .prompt(prompt)
                .with_history(history)
                .max_turns(max_turns)
                .extended_details()
                .await
                .map_err(Into::into)
        }
        ChatRoute::Bedrock { model } => {
            let client = crate::bedrock::client().await?;
            let agent = client.agent(&model).preamble(preamble).tools(tools).build();
            agent
                .prompt(prompt)
                .with_history(history)
                .max_turns(max_turns)
                .extended_details()
                .await
                .map_err(Into::into)
        }
        ChatRoute::VertexAi { model } => {
            let client = rig_vertexai::Client::builder().build()?;
            let agent = client.agent(&model).preamble(preamble).tools(tools).build();
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

fn opencode_api_id(provider: &str, model: &str) -> String {
    opencode_model(provider, model)
        .map(|model| model.api_id().to_string())
        .unwrap_or_else(|| model.to_string())
}

fn copilot_uses_responses_api(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.contains("codex") || model.starts_with("gpt-5") || model.starts_with("gemini-3")
}

fn opencode_model(provider: &str, model: &str) -> Option<super::opencode_models::OpenCodeModel> {
    OpenCodeModelListing::load()
        .ok()?
        .find(provider, model)
        .cloned()
}

fn inspect_opencode_models() -> Vec<AdapterModels> {
    opencode_models::inspect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_listing_only_includes_introspected_models() {
        let models = collect_all_models(&[]);
        assert!(models.is_empty());
    }

    #[test]
    fn legacy_openai_resp_saved_model_uses_shim() {
        assert_eq!(
            legacy_saved_model_spec("openai_resp::gpt-5.5", Some("copilot")),
            "copilot::gpt-5.5"
        );
        assert_eq!(
            legacy_saved_model_spec("github-copilot/gpt-5.5", Some("copilot")),
            "github-copilot/gpt-5.5"
        );
    }

    #[test]
    fn copilot_routes_reasoning_models_to_responses_api() {
        assert!(copilot_uses_responses_api("gpt-5.5"));
        assert!(copilot_uses_responses_api("gpt-5.3-codex"));
        assert!(copilot_uses_responses_api("gemini-3.1-pro-preview"));
        assert!(!copilot_uses_responses_api("gpt-4.1"));
    }
}
