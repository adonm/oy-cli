use serde_json::{Value, json};

use super::Protocol;

#[path = "providers/route.rs"]
pub(crate) mod route;

pub(crate) const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub(crate) const GITHUB_COPILOT_BASE_URL: &str = "https://api.githubcopilot.com";
pub(crate) const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
pub(crate) const BEDROCK_DEFAULT_REGION: &str = "us-east-1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderFamily {
    OpenAi,
    OpenAiCompatible,
    OpenRouter,
    Xai,
    GitHubCopilot,
    AzureOpenAi,
    CloudflareAiGateway,
    CloudflareWorkersAi,
    Anthropic,
    GoogleGemini,
    AmazonBedrock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderMetadata {
    pub(crate) id: &'static str,
    pub(crate) family: ProviderFamily,
    pub(crate) default_base_url: Option<&'static str>,
    pub(crate) auth_env: &'static [&'static str],
    pub(crate) supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteProfile {
    pub(crate) model_id: String,
    pub(crate) base_url: String,
    pub(crate) protocol: Protocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenAiCompatibleProfile {
    pub(crate) provider: &'static str,
    pub(crate) base_url: &'static str,
}

pub(crate) const OPENAI_COMPATIBLE_PROFILES: &[OpenAiCompatibleProfile] = &[
    OpenAiCompatibleProfile {
        provider: "baseten",
        base_url: "https://inference.baseten.co/v1",
    },
    OpenAiCompatibleProfile {
        provider: "cerebras",
        base_url: "https://api.cerebras.ai/v1",
    },
    OpenAiCompatibleProfile {
        provider: "deepinfra",
        base_url: "https://api.deepinfra.com/v1/openai",
    },
    OpenAiCompatibleProfile {
        provider: "deepseek",
        base_url: "https://api.deepseek.com/v1",
    },
    OpenAiCompatibleProfile {
        provider: "fireworks",
        base_url: "https://api.fireworks.ai/inference/v1",
    },
    OpenAiCompatibleProfile {
        provider: "groq",
        base_url: "https://api.groq.com/openai/v1",
    },
    OpenAiCompatibleProfile {
        provider: "openrouter",
        base_url: "https://openrouter.ai/api/v1",
    },
    OpenAiCompatibleProfile {
        provider: "togetherai",
        base_url: "https://api.together.xyz/v1",
    },
    OpenAiCompatibleProfile {
        provider: "xai",
        base_url: "https://api.x.ai/v1",
    },
];

pub(crate) const PROVIDERS: &[ProviderMetadata] = &[
    ProviderMetadata {
        id: "openai",
        family: ProviderFamily::OpenAi,
        default_base_url: Some(OPENAI_BASE_URL),
        auth_env: &["OPENAI_API_KEY"],
        supported: true,
    },
    ProviderMetadata {
        id: "github-copilot",
        family: ProviderFamily::GitHubCopilot,
        default_base_url: Some(GITHUB_COPILOT_BASE_URL),
        auth_env: &[
            "GITHUB_COPILOT_API_KEY",
            "COPILOT_API_KEY",
            "OPENCODE_API_KEY",
        ],
        supported: true,
    },
    ProviderMetadata {
        id: "openrouter",
        family: ProviderFamily::OpenRouter,
        default_base_url: Some("https://openrouter.ai/api/v1"),
        auth_env: &["OPENROUTER_API_KEY", "OPENCODE_API_KEY"],
        supported: true,
    },
    ProviderMetadata {
        id: "xai",
        family: ProviderFamily::Xai,
        default_base_url: Some("https://api.x.ai/v1"),
        auth_env: &["XAI_API_KEY"],
        supported: true,
    },
    ProviderMetadata {
        id: "azure",
        family: ProviderFamily::AzureOpenAi,
        default_base_url: None,
        auth_env: &["AZURE_OPENAI_API_KEY"],
        supported: true,
    },
    ProviderMetadata {
        id: "cloudflare-ai-gateway",
        family: ProviderFamily::CloudflareAiGateway,
        default_base_url: None,
        auth_env: &["CLOUDFLARE_API_TOKEN", "CF_AIG_TOKEN"],
        supported: true,
    },
    ProviderMetadata {
        id: "cloudflare-workers-ai",
        family: ProviderFamily::CloudflareWorkersAi,
        default_base_url: None,
        auth_env: &["CLOUDFLARE_API_KEY", "CLOUDFLARE_WORKERS_AI_TOKEN"],
        supported: true,
    },
    ProviderMetadata {
        id: "anthropic",
        family: ProviderFamily::Anthropic,
        default_base_url: Some("https://api.anthropic.com/v1"),
        auth_env: &["ANTHROPIC_API_KEY"],
        supported: true,
    },
    ProviderMetadata {
        id: "google",
        family: ProviderFamily::GoogleGemini,
        default_base_url: Some(GEMINI_BASE_URL),
        auth_env: &[
            "GOOGLE_GENERATIVE_AI_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
        ],
        supported: true,
    },
    ProviderMetadata {
        id: "amazon-bedrock",
        family: ProviderFamily::AmazonBedrock,
        default_base_url: None,
        auth_env: &[
            "BEDROCK_API_KEY",
            "AWS_BEARER_TOKEN_BEDROCK",
            "AWS_ACCESS_KEY_ID",
        ],
        supported: true,
    },
];

pub(crate) fn provider_metadata(provider: &str) -> Option<ProviderMetadata> {
    PROVIDERS
        .iter()
        .copied()
        .find(|metadata| metadata.id == provider)
        .or_else(|| {
            openai_compatible_profile(provider).map(|profile| ProviderMetadata {
                id: profile.provider,
                family: ProviderFamily::OpenAiCompatible,
                default_base_url: Some(profile.base_url),
                auth_env: &[],
                supported: true,
            })
        })
}

pub(crate) fn openai_profile(model: &str, base_url: Option<String>) -> RouteProfile {
    RouteProfile {
        model_id: model.to_string(),
        base_url: base_url.unwrap_or_else(|| OPENAI_BASE_URL.to_string()),
        protocol: Protocol::OpenAiChat,
    }
}

pub(crate) fn xai_profile(model: &str, base_url: Option<String>) -> RouteProfile {
    let profile = openai_compatible_profile("xai").expect("xai profile exists");
    RouteProfile {
        model_id: model.to_string(),
        base_url: base_url.unwrap_or_else(|| profile.base_url.to_string()),
        protocol: Protocol::OpenAiResponses,
    }
}

pub(crate) fn openrouter_profile(model: &str, base_url: Option<String>) -> RouteProfile {
    let profile = openai_compatible_profile("openrouter").expect("openrouter profile exists");
    RouteProfile {
        model_id: model.to_string(),
        base_url: base_url.unwrap_or_else(|| profile.base_url.to_string()),
        protocol: Protocol::OpenAiChat,
    }
}

pub(crate) fn azure_profile(
    model: &str,
    base_url: String,
    use_completion_urls: bool,
) -> RouteProfile {
    RouteProfile {
        model_id: model.to_string(),
        base_url,
        protocol: if use_completion_urls {
            Protocol::OpenAiChat
        } else {
            Protocol::OpenAiResponses
        },
    }
}

pub(crate) fn azure_resource_base_url(resource_name: &str) -> String {
    format!(
        "https://{}.openai.azure.com/openai/v1",
        resource_name.trim()
    )
}

pub(crate) fn cloudflare_ai_gateway_base_url(account_id: &str, gateway_id: Option<&str>) -> String {
    format!(
        "https://gateway.ai.cloudflare.com/v1/{}/{}/compat",
        percent_encode(account_id),
        percent_encode(gateway_id.unwrap_or("default").trim())
    )
}

pub(crate) fn cloudflare_workers_ai_base_url(account_id: &str) -> String {
    format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/ai/v1",
        percent_encode(account_id)
    )
}

pub(crate) fn github_copilot_profile(model: &str, opencode_base_url: Option<&str>) -> RouteProfile {
    let base_url = opencode_base_url
        .unwrap_or(GITHUB_COPILOT_BASE_URL)
        .trim_end_matches("/v1")
        .trim_end_matches('/')
        .to_string();
    RouteProfile {
        model_id: model.to_string(),
        base_url,
        protocol: if github_copilot_should_use_responses_api(model) {
            Protocol::OpenAiResponses
        } else {
            Protocol::OpenAiChat
        },
    }
}

pub(crate) fn opencode_profile(provider: &str, model_id: &str, base_url: &str) -> RouteProfile {
    RouteProfile {
        model_id: model_id.to_string(),
        base_url: base_url.to_string(),
        protocol: if is_bedrock_provider(provider) {
            Protocol::BedrockConverse
        } else if provider == "anthropic" {
            Protocol::AnthropicMessages
        } else if provider == "google" {
            Protocol::Gemini
        } else if opencode_should_use_responses_api(provider, model_id) {
            Protocol::OpenAiResponses
        } else {
            Protocol::OpenAiChat
        },
    }
}

pub(crate) fn gemini_profile(model: &str, base_url: Option<String>) -> RouteProfile {
    RouteProfile {
        model_id: model.to_string(),
        base_url: base_url.unwrap_or_else(|| GEMINI_BASE_URL.to_string()),
        protocol: Protocol::Gemini,
    }
}

pub(crate) fn anthropic_profile(model: &str, base_url: Option<String>) -> RouteProfile {
    RouteProfile {
        model_id: model.to_string(),
        base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com/v1".to_string()),
        protocol: Protocol::AnthropicMessages,
    }
}

pub(crate) fn bedrock_profile(model: &str, region: &str, base_url: Option<String>) -> RouteProfile {
    RouteProfile {
        model_id: model.to_string(),
        base_url: base_url.unwrap_or_else(|| bedrock_base_url(region)),
        protocol: Protocol::BedrockConverse,
    }
}

pub(crate) fn bedrock_base_url(region: &str) -> String {
    format!("https://bedrock-runtime.{region}.amazonaws.com")
}

pub(crate) fn is_bedrock_provider(provider: &str) -> bool {
    matches!(provider, "bedrock" | "amazon-bedrock")
}

pub(crate) fn openai_compatible_profile(provider: &str) -> Option<OpenAiCompatibleProfile> {
    OPENAI_COMPATIBLE_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.provider == provider)
}

pub(crate) fn github_copilot_should_use_responses_api(model_id: &str) -> bool {
    let model = model_id.to_ascii_lowercase();
    let Some(rest) = model.strip_prefix("gpt-") else {
        // Local compatibility extension: Copilot's Gemini 3 route has been observed
        // returning Responses-style payloads even though OpenCode's helper only covers GPT.
        return model.starts_with("gemini-3");
    };
    let digits = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse::<u64>().is_ok_and(|major| major >= 5) && !model.starts_with("gpt-5-mini")
}

pub(crate) fn openrouter_body_options(input: Option<&Value>) -> Option<Value> {
    let object = input.and_then(Value::as_object)?;
    let mut body = serde_json::Map::new();
    match object.get("usage") {
        Some(Value::Bool(true)) => {
            body.insert("usage".to_string(), json!({"include": true}));
        }
        Some(Value::Object(usage)) => {
            body.insert("usage".to_string(), Value::Object(usage.clone()));
        }
        _ => {}
    }
    if let Some(Value::Object(reasoning)) = object.get("reasoning") {
        body.insert("reasoning".to_string(), Value::Object(reasoning.clone()));
    }
    if let Some(prompt_cache_key) = object.get("promptCacheKey").and_then(Value::as_str) {
        body.insert("prompt_cache_key".to_string(), json!(prompt_cache_key));
    }
    (!body.is_empty()).then_some(Value::Object(body))
}

pub(crate) fn opencode_should_use_responses_api(provider: &str, model_id: &str) -> bool {
    provider == "opencode" && model_id.to_ascii_lowercase().starts_with("gpt-5")
}

pub(crate) fn gpt5_default_provider_options(
    model_id: &str,
    protocol: Protocol,
) -> Option<serde_json::Value> {
    gpt5_default_provider_options_for_protocol(model_id, protocol, false)
}

pub(crate) fn openai_default_provider_options(
    model_id: &str,
    protocol: Protocol,
) -> Option<serde_json::Value> {
    let mut options = serde_json::json!({"store": false});
    if let Some(gpt5) = gpt5_default_provider_options_for_protocol(model_id, protocol, true) {
        merge_json_objects(&mut options, gpt5);
    }
    Some(options)
}

pub(crate) fn github_copilot_default_provider_options(
    model_id: &str,
    protocol: Protocol,
) -> Option<serde_json::Value> {
    let mut options = serde_json::json!({"store": false});
    if let Some(gpt5) = gpt5_default_provider_options_for_protocol(model_id, protocol, false) {
        merge_json_objects(&mut options, gpt5);
    }
    Some(options)
}

fn gpt5_default_provider_options_for_protocol(
    model_id: &str,
    protocol: Protocol,
    responses_text_verbosity: bool,
) -> Option<serde_json::Value> {
    let id = model_id.to_ascii_lowercase();
    if !id.contains("gpt-5") || id.contains("gpt-5-chat") || id.contains("gpt-5-pro") {
        return None;
    }
    if !protocol.uses_responses_api() {
        return Some(serde_json::json!({"reasoning_effort": "medium"}));
    }

    let mut options = serde_json::json!({"reasoning": {"effort": "medium", "summary": "auto"}});
    if responses_text_verbosity
        && id.contains("gpt-5.")
        && !id.contains("codex")
        && !id.contains("-chat")
    {
        merge_json_objects(
            &mut options,
            serde_json::json!({"text": {"verbosity": "low"}}),
        );
    }
    Some(options)
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

fn percent_encode(input: &str) -> String {
    url::form_urlencoded::byte_serialize(input.as_bytes()).collect()
}

#[cfg(test)]
#[path = "test/provider.rs"]
mod tests;
