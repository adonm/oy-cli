//! Model-spec to [`ModelRoute`] resolution.
//!
//! This mirrors OpenCode's split: agent/app code chooses a model spec, while
//! the LLM layer maps provider/model/auth/options into a runnable route.

use anyhow::Result;

use crate::config;
use crate::llm::{ModelRoute, Protocol};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParsedModelSpec<'a> {
    pub(crate) provider: Option<&'a str>,
    pub(crate) model: &'a str,
    pub(crate) base_model: &'a str,
    pub(crate) reasoning_effort: Option<&'static str>,
}

impl<'a> ParsedModelSpec<'a> {
    pub(crate) fn parse(spec: &'a str) -> Self {
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

    pub(crate) fn provider_or_openai(self) -> &'a str {
        self.provider
            .map(config::canonical_provider)
            .unwrap_or("openai")
    }
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

pub(crate) fn model_route(
    model_spec: &str,
    reasoning_effort: Option<String>,
) -> Result<ModelRoute> {
    let parsed = ParsedModelSpec::parse(model_spec);
    let provider = parsed.provider_or_openai();
    let mut route = match provider {
        "github-copilot" => {
            crate::llm::providers::route::prepare_github_copilot_chat(parsed.base_model)?
        }
        "openai" => crate::llm::providers::route::prepare_openai_chat(parsed.base_model)?,
        "xai" => crate::llm::providers::route::prepare_xai_chat(parsed.base_model)?,
        "openrouter" => crate::llm::providers::route::prepare_openrouter_chat(parsed.base_model)?,
        "anthropic" => crate::llm::providers::route::prepare_anthropic_chat(parsed.base_model)?,
        "azure" => crate::llm::providers::route::prepare_azure_chat(parsed.base_model)?,
        "cloudflare-ai-gateway" => {
            crate::llm::providers::route::prepare_cloudflare_ai_gateway_chat(parsed.base_model)?
        }
        "cloudflare-workers-ai" => {
            crate::llm::providers::route::prepare_cloudflare_workers_ai_chat(parsed.base_model)?
        }
        "bedrock" | "amazon-bedrock" => {
            crate::llm::providers::route::prepare_bedrock_chat(provider, parsed.base_model)?
        }
        provider => crate::llm::providers::route::prepare_opencode_compatible_chat(
            provider,
            parsed.base_model,
        )?,
    };
    let provider_defaults = match provider {
        "openai" => {
            crate::llm::providers::openai_default_provider_options(&route.model, route.protocol)
        }
        "github-copilot" => crate::llm::providers::github_copilot_default_provider_options(
            &route.model,
            route.protocol,
        ),
        "openrouter" | "anthropic" => None,
        _ if matches!(
            route.protocol,
            Protocol::AnthropicMessages | Protocol::BedrockConverse
        ) =>
        {
            None
        }
        _ => crate::llm::providers::gpt5_default_provider_options(&route.model, route.protocol),
    };
    let reasoning_overlay = if route.protocol == Protocol::AnthropicMessages {
        None
    } else {
        reasoning_effort_json(reasoning_effort, route.protocol.uses_responses_api())
    };
    let route_params = route.additional_params.take();
    route.additional_params = merge_additional_params(
        merge_additional_params(provider_defaults, route_params),
        reasoning_overlay,
    );
    Ok(route)
}

pub(crate) fn reasoning_effort_json(
    effort: Option<String>,
    responses_api: bool,
) -> Option<serde_json::Value> {
    let effort = effort?;
    if responses_api {
        Some(serde_json::json!({"reasoning": {"effort": effort}}))
    } else {
        Some(serde_json::json!({"reasoning_effort": effort}))
    }
}

pub(crate) fn merge_additional_params(
    base: Option<serde_json::Value>,
    overlay: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    match (base, overlay) {
        (None, None) => None,
        (Some(value), None) | (None, Some(value)) => Some(value),
        (Some(mut base), Some(overlay)) => {
            merge_json_objects(&mut base, overlay);
            Some(base)
        }
    }
}

fn merge_json_objects(base: &mut serde_json::Value, overlay: serde_json::Value) {
    let Some(base_object) = base.as_object_mut() else {
        *base = overlay;
        return;
    };
    let serde_json::Value::Object(overlay) = overlay else {
        *base = overlay;
        return;
    };
    for (key, value) in overlay {
        match (base_object.get_mut(&key), value) {
            (Some(existing), serde_json::Value::Object(next)) if existing.is_object() => {
                merge_json_objects(existing, serde_json::Value::Object(next));
            }
            (_, value) => {
                base_object.insert(key, value);
            }
        }
    }
}
