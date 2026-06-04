//! Provider-specific [`ModelRoute`] builders.
//!
//! OpenCode keeps provider facades under its LLM package. These builders are
//! the oy equivalent: provider/model/auth/base-url decisions live next to the
//! LLM provider metadata, not in the agent facade.

use anyhow::{Context, Result, anyhow, bail};

use crate::agent::auth::{env_value, github_copilot_api_key, opencode_auth_key};
use crate::agent::opencode_models;
use crate::llm::providers::{ProviderFamily, ProviderMetadata};
use crate::llm::{AwsCredentials, ModelRoute, Protocol, RouteAuth};

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenCodeRouteProfile {
    model_id: String,
    base_url: String,
    protocol: Protocol,
    default_output_tokens: Option<u64>,
}

impl OpenCodeRouteProfile {
    fn from_model(
        provider: &str,
        model: &str,
        info: &opencode_models::OpenCodeModel,
    ) -> Result<Self> {
        if !info.is_openai_compatible_api()
            && !info.is_anthropic_api()
            && !info.is_bedrock_api()
            && !info.is_gemini_api()
        {
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
        } else if info.is_anthropic_api() {
            crate::llm::providers::anthropic_profile(
                &model_id,
                Some(strip_endpoint_suffix(&base_url, "messages")),
            )
        } else {
            crate::llm::providers::opencode_profile(provider, &model_id, &base_url)
        };
        Ok(Self {
            model_id: profile.model_id,
            base_url: profile.base_url,
            protocol: profile.protocol,
            default_output_tokens: (info.limits().output > 0)
                .then_some(info.limits().output as u64),
        })
    }
}

fn strip_endpoint_suffix(base_url: &str, suffix: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    trimmed
        .strip_suffix(&format!("/{suffix}"))
        .unwrap_or(trimmed)
        .to_string()
}

pub(crate) fn prepare_chat(
    provider: &str,
    metadata: Option<ProviderMetadata>,
    model: &str,
) -> Result<ModelRoute> {
    match metadata.map(|metadata| metadata.family) {
        Some(ProviderFamily::GitHubCopilot) => prepare_github_copilot_chat(model),
        Some(ProviderFamily::OpenAi) => prepare_openai_chat(model),
        Some(ProviderFamily::Xai) => prepare_xai_chat(model),
        Some(ProviderFamily::OpenRouter) => prepare_openrouter_chat(model),
        Some(ProviderFamily::Anthropic) => prepare_anthropic_chat(model),
        Some(ProviderFamily::GoogleGemini) => prepare_google_chat(model),
        Some(ProviderFamily::AzureOpenAi) => prepare_azure_chat(model),
        Some(ProviderFamily::CloudflareAiGateway) => prepare_cloudflare_ai_gateway_chat(model),
        Some(ProviderFamily::CloudflareWorkersAi) => prepare_cloudflare_workers_ai_chat(model),
        Some(ProviderFamily::AmazonBedrock) => prepare_bedrock_chat(provider, model),
        Some(ProviderFamily::OpenAiCompatible) | None => {
            prepare_opencode_compatible_chat(provider, model)
        }
    }
}

fn env_json(name: &str) -> Option<serde_json::Value> {
    env_value(name).and_then(|value| serde_json::from_str(&value).ok())
}

fn provider_auth_value(provider: &str) -> Option<String> {
    crate::llm::providers::provider_metadata(provider)
        .and_then(|metadata| metadata.first_auth_value(env_value))
}

/// Build a `ModelRoute` for an OpenAI-shaped provider (OpenAI, xAI,
/// OpenRouter, ...) that uses a single bearer/API key auth header.
///
/// Each caller supplies the per-provider [`RouteProfile`], the
/// `auth_provider` id used to look up the credential in provider
/// metadata, a context message for the missing-auth error, and the
/// already-resolved `additional_params` body options.
fn build_api_key_route(
    profile: crate::llm::providers::RouteProfile,
    auth_provider: &str,
    auth_missing_msg: &'static str,
    additional_params: Option<serde_json::Value>,
) -> Result<ModelRoute> {
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth: RouteAuth::ApiKey(provider_auth_value(auth_provider).context(auth_missing_msg)?),
        base_url: Some(profile.base_url),
        query_params: None,
        additional_params,
        default_output_tokens: None,
    })
}

pub(crate) fn prepare_openai_chat(model: &str) -> Result<ModelRoute> {
    let profile = crate::llm::providers::openai_profile(model, env_value("OPENAI_BASE_URL"));
    let additional_params = crate::llm::providers::openai_body_options(
        env_json("OPENAI_PROVIDER_OPTIONS").as_ref(),
        profile.protocol,
    );
    build_api_key_route(
        profile,
        "openai",
        "OpenAI auth is not configured",
        additional_params,
    )
}

pub(crate) fn prepare_xai_chat(model: &str) -> Result<ModelRoute> {
    let profile = crate::llm::providers::xai_profile(model, env_value("XAI_BASE_URL"));
    let additional_params = crate::llm::providers::openai_body_options(
        env_json("XAI_PROVIDER_OPTIONS").as_ref(),
        profile.protocol,
    );
    build_api_key_route(
        profile,
        "xai",
        "xAI auth is not configured; set XAI_API_KEY",
        additional_params,
    )
}

pub(crate) fn prepare_openrouter_chat(model: &str) -> Result<ModelRoute> {
    let profile =
        crate::llm::providers::openrouter_profile(model, env_value("OPENROUTER_BASE_URL"));
    let additional_params = env_json("OPENROUTER_PROVIDER_OPTIONS")
        .and_then(|value| crate::llm::providers::openrouter_body_options(Some(&value)));
    build_api_key_route(
        profile,
        "openrouter",
        "OpenRouter auth is not configured; set OPENROUTER_API_KEY",
        additional_params,
    )
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
    let provider_options = env_json("GEMINI_PROVIDER_OPTIONS")
        .or_else(|| env_json("GOOGLE_PROVIDER_OPTIONS"))
        .and_then(|value| crate::llm::providers::gemini_body_options(Some(&value)));
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth: RouteAuth::Header {
            name: "x-goog-api-key".to_string(),
            value: provider_auth_value("google")
                .context("Google Gemini auth is not configured; set GOOGLE_GENERATIVE_AI_API_KEY or GEMINI_API_KEY")?,
        },
        base_url: Some(profile.base_url),
        query_params: None,
        additional_params: provider_options,
        default_output_tokens: None,
    })
}

pub(crate) fn prepare_anthropic_chat(model: &str) -> Result<ModelRoute> {
    let model_id = opencode_models::find("anthropic", model)
        .as_ref()
        .map(|info| info.api_id().to_string())
        .unwrap_or_else(|| model.to_string());
    let profile =
        crate::llm::providers::anthropic_profile(&model_id, env_value("ANTHROPIC_BASE_URL"));
    let provider_options = crate::llm::providers::anthropic_body_options(
        env_json("ANTHROPIC_PROVIDER_OPTIONS").as_ref(),
    );
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth: RouteAuth::Headers(vec![
            (
                "x-api-key".to_string(),
                provider_auth_value("anthropic")
                    .context("Anthropic auth is not configured; set ANTHROPIC_API_KEY")?,
            ),
            (
                "anthropic-version".to_string(),
                env_value("ANTHROPIC_VERSION").unwrap_or_else(|| "2023-06-01".to_string()),
            ),
        ]),
        base_url: Some(profile.base_url),
        query_params: None,
        additional_params: provider_options,
        default_output_tokens: None,
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
            value: provider_auth_value("azure")
                .context("Azure OpenAI auth is not configured; set AZURE_OPENAI_API_KEY")?,
        },
        base_url: Some(profile.base_url),
        query_params: Some(vec![(
            "api-version".to_string(),
            env_value("AZURE_OPENAI_API_VERSION").unwrap_or_else(|| "v1".to_string()),
        )]),
        additional_params: None,
        default_output_tokens: None,
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
    let gateway_key = provider_auth_value("cloudflare-ai-gateway");
    let api_key =
        env_value("CLOUDFLARE_AI_GATEWAY_PROVIDER_API_KEY").or_else(|| env_value("OPENAI_API_KEY"));
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
        default_output_tokens: None,
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
            provider_auth_value("cloudflare-workers-ai")
                .context("Cloudflare Workers AI auth is not configured; set CLOUDFLARE_API_KEY or CLOUDFLARE_WORKERS_AI_TOKEN")?,
        ),
        base_url: Some(base_url),
        query_params: None,
        additional_params: None,
        default_output_tokens: None,
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
        default_output_tokens: profile.and_then(|profile| profile.default_output_tokens),
    })
}

pub(crate) fn prepare_bedrock_chat(provider: &str, model: &str) -> Result<ModelRoute> {
    let model_info = opencode_models::find(provider, model);
    let model_id = model_info
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
        default_output_tokens: model_info
            .as_ref()
            .and_then(|info| (info.limits().output > 0).then_some(info.limits().output as u64)),
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
    let auth = match profile.protocol {
        Protocol::Gemini => RouteAuth::Composite(vec![
            RouteAuth::Header {
                name: "x-goog-api-key".to_string(),
                value: api_key,
            },
            opencode_client_headers(),
        ]),
        Protocol::AnthropicMessages => RouteAuth::Composite(vec![
            RouteAuth::Headers(vec![
                ("x-api-key".to_string(), api_key),
                ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ]),
            opencode_client_headers(),
        ]),
        _ => RouteAuth::Composite(vec![RouteAuth::ApiKey(api_key), opencode_client_headers()]),
    };
    Ok(ModelRoute {
        protocol: profile.protocol,
        model: profile.model_id,
        auth,
        base_url: Some(profile.base_url),
        query_params: None,
        additional_params: None,
        default_output_tokens: profile.default_output_tokens,
    })
}

fn opencode_client_headers() -> RouteAuth {
    RouteAuth::Headers(vec![
        ("x-opencode-client".to_string(), "oy".to_string()),
        ("user-agent".to_string(), "oy".to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each builder reads its own env-var (OPENAI_API_KEY / XAI_API_KEY /
    // OPENROUTER_API_KEY) through `provider_auth_value`; the test sets
    // stubs so the helper can be exercised without real credentials.
    fn stub_auth_env() -> [(&'static str, &'static str); 3] {
        [
            ("OPENAI_API_KEY", "test-openai-key"),
            ("XAI_API_KEY", "test-xai-key"),
            ("OPENROUTER_API_KEY", "test-openrouter-key"),
        ]
    }

    #[test]
    fn build_api_key_route_copies_profile_into_route_with_api_key_auth() {
        for (name, value) in stub_auth_env() {
            // SAFETY: tests in this module own the env mutation; they run
            // serially within the same process.
            unsafe { std::env::set_var(name, value) };
        }
        let profile = crate::llm::providers::openai_profile("gpt-4.1", None);
        let route = build_api_key_route(
            profile.clone(),
            "openai",
            "OpenAI auth is not configured",
            None,
        )
        .unwrap();
        assert_eq!(route.protocol, profile.protocol);
        assert_eq!(route.model, profile.model_id);
        assert_eq!(route.base_url.as_deref(), Some(profile.base_url.as_str()));
        assert!(route.query_params.is_none());
        assert!(route.additional_params.is_none());
        assert!(route.default_output_tokens.is_none());
        match route.auth {
            RouteAuth::ApiKey(value) => assert_eq!(value, "test-openai-key"),
            other => panic!("expected RouteAuth::ApiKey, got {other:?}"),
        }
    }

    #[test]
    fn build_api_key_route_preserves_protocol_for_each_provider() {
        for (name, value) in stub_auth_env() {
            // SAFETY: see `build_api_key_route_copies_profile_into_route_with_api_key_auth`.
            unsafe { std::env::set_var(name, value) };
        }
        // The point of this test is to lock the contract that the
        // helper copies the profile's protocol verbatim rather than
        // substituting its own. The three OpenAI-shaped profiles
        // currently resolve to Responses (OpenAI/xAI) and Chat
        // (OpenRouter); we assert that the route matches the profile
        // for each one.
        let cases = [
            (
                "openai",
                crate::llm::providers::openai_profile("gpt-4.1", None),
            ),
            ("xai", crate::llm::providers::xai_profile("grok-2", None)),
            (
                "openrouter",
                crate::llm::providers::openrouter_profile("openai/gpt-4.1", None),
            ),
        ];
        for (provider, profile) in cases {
            let route = build_api_key_route(profile.clone(), provider, "x", None).unwrap();
            assert_eq!(
                route.protocol, profile.protocol,
                "{provider} route protocol must match profile"
            );
        }
    }

    #[test]
    fn build_api_key_route_surfaces_missing_auth_with_caller_message() {
        // Use a known-missing provider id so the test does not depend on
        // (and cannot leak into) the process env. The caller's
        // missing-auth message must be forwarded verbatim, proving the
        // helper does not bake its own context.
        let profile = crate::llm::providers::openai_profile("gpt-4.1", None);
        let err = build_api_key_route(
            profile,
            "definitely-no-such-provider",
            "caller-supplied message must surface",
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("caller-supplied message must surface")
        );
    }
}
