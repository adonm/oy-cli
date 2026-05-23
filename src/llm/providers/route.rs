//! Provider-specific [`ModelRoute`] builders.
//!
//! OpenCode keeps provider facades under its LLM package. These builders are
//! the oy equivalent: provider/model/auth/base-url decisions live next to the
//! LLM provider metadata, not in the agent facade.

use anyhow::{Context, Result, anyhow, bail};

use crate::agent::auth::{env_value, github_copilot_api_key, opencode_auth_key};
use crate::agent::opencode_models;
use crate::llm::{AwsCredentials, ModelRoute, Protocol, RouteAuth};

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
        if !info.is_openai_compatible_api() && !info.is_bedrock_api() && !info.is_gemini_api() {
            bail!("OpenCode model `{provider}/{model}` is not supported by the native LLM backend");
        }
        let model_id = info.api_id().to_string();
        let base_url = info
            .api_url()
            .map(ToOwned::to_owned)
            .or_else(|| {
                crate::llm::providers::openai_compatible_profile(provider)
                    .map(|profile| profile.base_url.to_string())
            })
            .or_else(|| {
                crate::llm::providers::is_bedrock_provider(provider).then(|| {
                    crate::llm::providers::bedrock_base_url(
                        crate::llm::providers::BEDROCK_DEFAULT_REGION,
                    )
                })
            })
            .ok_or_else(|| {
                anyhow!("OpenCode model `{provider}/{model}` does not expose an API URL")
            })?;
        let profile = if info.is_gemini_api() {
            crate::llm::providers::gemini_profile(&model_id, Some(base_url.clone()))
        } else {
            crate::llm::providers::opencode_profile(provider, &model_id, &base_url)
        };
        Ok(Self {
            model_id: profile.model_id,
            base_url: profile.base_url,
            protocol: profile.protocol,
        })
    }
}

pub(crate) fn prepare_openai_chat(model: &str) -> Result<ModelRoute> {
    let profile = crate::llm::providers::openai_profile(model, env_value("OPENAI_BASE_URL"));
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth: RouteAuth::ApiKey(
            env_value("OPENAI_API_KEY").context("OpenAI auth is not configured")?,
        ),
        base_url: Some(profile.base_url),
        query_params: None,
        additional_params: None,
    })
}

fn env_json(name: &str) -> Option<serde_json::Value> {
    env_value(name).and_then(|value| serde_json::from_str(&value).ok())
}

pub(crate) fn prepare_xai_chat(model: &str) -> Result<ModelRoute> {
    let profile = crate::llm::providers::xai_profile(model, env_value("XAI_BASE_URL"));
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth: RouteAuth::ApiKey(
            env_value("XAI_API_KEY").context("xAI auth is not configured; set XAI_API_KEY")?,
        ),
        base_url: Some(profile.base_url),
        query_params: None,
        additional_params: None,
    })
}

pub(crate) fn prepare_openrouter_chat(model: &str) -> Result<ModelRoute> {
    let profile =
        crate::llm::providers::openrouter_profile(model, env_value("OPENROUTER_BASE_URL"));
    let provider_options = env_json("OPENROUTER_PROVIDER_OPTIONS")
        .and_then(|value| crate::llm::providers::openrouter_body_options(Some(&value)));
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth: RouteAuth::ApiKey(
            env_value("OPENROUTER_API_KEY")
                .or_else(|| env_value("OPENCODE_API_KEY"))
                .context("OpenRouter auth is not configured; set OPENROUTER_API_KEY")?,
        ),
        base_url: Some(profile.base_url),
        query_params: None,
        additional_params: provider_options,
    })
}

pub(crate) fn prepare_google_chat(model: &str) -> Result<ModelRoute> {
    let model_id = opencode_models::find("google", model)
        .as_ref()
        .map(|info| info.api_id().to_string())
        .unwrap_or_else(|| model.to_string());
    let profile = crate::llm::providers::gemini_profile(
        &model_id,
        env_value("GOOGLE_BASE_URL").or_else(|| env_value("GEMINI_BASE_URL")),
    );
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth: RouteAuth::Header {
            name: "x-goog-api-key".to_string(),
            value: env_value("GOOGLE_GENERATIVE_AI_API_KEY")
                .or_else(|| env_value("GEMINI_API_KEY"))
                .or_else(|| env_value("GOOGLE_API_KEY"))
                .context("Google Gemini auth is not configured; set GOOGLE_GENERATIVE_AI_API_KEY or GEMINI_API_KEY")?,
        },
        base_url: Some(profile.base_url),
        query_params: None,
        additional_params: None,
    })
}

pub(crate) fn prepare_anthropic_chat(model: &str) -> Result<ModelRoute> {
    let model_id = opencode_models::find("anthropic", model)
        .as_ref()
        .map(|info| info.api_id().to_string())
        .unwrap_or_else(|| model.to_string());
    let profile =
        crate::llm::providers::anthropic_profile(&model_id, env_value("ANTHROPIC_BASE_URL"));
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth: RouteAuth::Headers(vec![
            (
                "x-api-key".to_string(),
                env_value("ANTHROPIC_API_KEY")
                    .context("Anthropic auth is not configured; set ANTHROPIC_API_KEY")?,
            ),
            (
                "anthropic-version".to_string(),
                env_value("ANTHROPIC_VERSION").unwrap_or_else(|| "2023-06-01".to_string()),
            ),
        ]),
        base_url: Some(profile.base_url),
        query_params: None,
        additional_params: env_json("ANTHROPIC_PROVIDER_OPTIONS"),
    })
}

pub(crate) fn prepare_azure_chat(model: &str) -> Result<ModelRoute> {
    let base_url = env_value("AZURE_OPENAI_BASE_URL")
        .or_else(|| env_value("AZURE_BASE_URL"))
        .or_else(|| {
            env_value("AZURE_OPENAI_RESOURCE_NAME")
                .map(|name| crate::llm::providers::azure_resource_base_url(&name))
        })
        .context("Azure OpenAI requires AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME")?;
    let use_completion_urls = env_value("AZURE_OPENAI_USE_COMPLETION_URLS")
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "on" | "yes"));
    let profile = crate::llm::providers::azure_profile(model, base_url, use_completion_urls);
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth: RouteAuth::Header {
            name: "api-key".to_string(),
            value: env_value("AZURE_OPENAI_API_KEY")
                .context("Azure OpenAI auth is not configured; set AZURE_OPENAI_API_KEY")?,
        },
        base_url: Some(profile.base_url),
        query_params: Some(vec![(
            "api-version".to_string(),
            env_value("AZURE_OPENAI_API_VERSION").unwrap_or_else(|| "v1".to_string()),
        )]),
        additional_params: None,
    })
}

pub(crate) fn prepare_cloudflare_ai_gateway_chat(model: &str) -> Result<ModelRoute> {
    let base_url = env_value("CLOUDFLARE_AI_GATEWAY_BASE_URL").or_else(|| {
        env_value("CLOUDFLARE_ACCOUNT_ID").map(|account_id| {
            crate::llm::providers::cloudflare_ai_gateway_base_url(
                &account_id,
                env_value("CLOUDFLARE_AI_GATEWAY_ID").as_deref(),
            )
        })
    }).context("Cloudflare AI Gateway requires CLOUDFLARE_AI_GATEWAY_BASE_URL or CLOUDFLARE_ACCOUNT_ID")?;
    let gateway_key = env_value("CLOUDFLARE_API_TOKEN").or_else(|| env_value("CF_AIG_TOKEN"));
    let api_key = env_value("OPENAI_API_KEY");
    let auth = match (gateway_key, api_key) {
        (Some(gateway_key), Some(api_key)) => RouteAuth::Composite(vec![
            RouteAuth::Header {
                name: "cf-aig-authorization".to_string(),
                value: gateway_key,
            },
            RouteAuth::ApiKey(api_key),
        ]),
        (Some(gateway_key), None) => RouteAuth::Header {
            name: "cf-aig-authorization".to_string(),
            value: gateway_key,
        },
        (None, Some(api_key)) => RouteAuth::ApiKey(api_key),
        (None, None) => bail!(
            "Cloudflare AI Gateway auth is not configured; set CLOUDFLARE_API_TOKEN or CF_AIG_TOKEN"
        ),
    };
    Ok(ModelRoute {
        protocol: Protocol::OpenAiChat,
        model: model.to_string(),
        auth,
        base_url: Some(base_url),
        query_params: None,
        additional_params: None,
    })
}

pub(crate) fn prepare_cloudflare_workers_ai_chat(model: &str) -> Result<ModelRoute> {
    let base_url = env_value("CLOUDFLARE_WORKERS_AI_BASE_URL").or_else(|| {
        env_value("CLOUDFLARE_ACCOUNT_ID")
            .map(|account_id| crate::llm::providers::cloudflare_workers_ai_base_url(&account_id))
    }).context("Cloudflare Workers AI requires CLOUDFLARE_WORKERS_AI_BASE_URL or CLOUDFLARE_ACCOUNT_ID")?;
    Ok(ModelRoute {
        protocol: Protocol::OpenAiChat,
        model: model.to_string(),
        auth: RouteAuth::ApiKey(
            env_value("CLOUDFLARE_API_KEY")
                .or_else(|| env_value("CLOUDFLARE_WORKERS_AI_TOKEN"))
                .context("Cloudflare Workers AI auth is not configured; set CLOUDFLARE_API_KEY or CLOUDFLARE_WORKERS_AI_TOKEN")?,
        ),
        base_url: Some(base_url),
        query_params: None,
        additional_params: None,
    })
}

pub(crate) fn prepare_github_copilot_chat(model: &str) -> Result<ModelRoute> {
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
    let route_profile = crate::llm::providers::github_copilot_profile(
        &model_id,
        profile.as_ref().map(|profile| profile.base_url.as_str()),
    );
    Ok(ModelRoute {
        protocol: route_profile.protocol,
        model: route_profile.model_id,
        auth: RouteAuth::ApiKey(api_key),
        base_url: Some(route_profile.base_url),
        query_params: None,
        additional_params: None,
    })
}

pub(crate) fn prepare_bedrock_chat(provider: &str, model: &str) -> Result<ModelRoute> {
    let model_id = opencode_models::find(provider, model)
        .as_ref()
        .map(|info| info.api_id().to_string())
        .unwrap_or_else(|| model.to_string());
    let region = env_value("AWS_REGION")
        .or_else(|| env_value("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|| crate::llm::providers::BEDROCK_DEFAULT_REGION.to_string());
    let profile = crate::llm::providers::bedrock_profile(
        &model_id,
        &region,
        env_value("BEDROCK_BASE_URL").or_else(|| env_value("AWS_BEDROCK_BASE_URL")),
    );
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth: bedrock_auth(&region)?,
        base_url: Some(profile.base_url),
        query_params: None,
        additional_params: None,
    })
}

fn bedrock_auth(region: &str) -> Result<RouteAuth> {
    if let Some(api_key) =
        env_value("BEDROCK_API_KEY").or_else(|| env_value("AWS_BEARER_TOKEN_BEDROCK"))
    {
        return Ok(RouteAuth::ApiKey(api_key));
    }
    let access_key_id = env_value("AWS_ACCESS_KEY_ID").context(
        "Bedrock auth is not configured; set BEDROCK_API_KEY, AWS_BEARER_TOKEN_BEDROCK, or AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY",
    )?;
    let secret_access_key = env_value("AWS_SECRET_ACCESS_KEY")
        .context("Bedrock SigV4 auth requires AWS_SECRET_ACCESS_KEY")?;
    Ok(RouteAuth::AwsSigV4(AwsCredentials {
        region: region.to_string(),
        access_key_id,
        secret_access_key,
        session_token: env_value("AWS_SESSION_TOKEN"),
    }))
}

pub(crate) fn prepare_opencode_compatible_chat(provider: &str, model: &str) -> Result<ModelRoute> {
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
        query_params: None,
        additional_params: None,
    })
}
