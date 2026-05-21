use super::*;

#[test]
fn copilot_response_api_rule_matches_opencode_gpt_rule_and_local_gemini_quirk() {
    assert!(github_copilot_should_use_responses_api("gpt-5.5"));
    assert!(github_copilot_should_use_responses_api("gpt-5.3-codex"));
    assert!(!github_copilot_should_use_responses_api("gpt-5-mini"));
    assert!(!github_copilot_should_use_responses_api("gpt-4.1"));
    assert!(github_copilot_should_use_responses_api(
        "gemini-3.1-pro-preview"
    ));
}

#[test]
fn openai_compatible_profiles_include_opencode_set() {
    assert_eq!(
        openai_compatible_profile("deepseek").unwrap().base_url,
        "https://api.deepseek.com/v1"
    );
    assert_eq!(
        openai_compatible_profile("openrouter").unwrap().base_url,
        "https://openrouter.ai/api/v1"
    );
    assert!(openai_compatible_profile("unknown").is_none());
}

#[test]
fn provider_registry_matches_opencode_provider_surface() {
    let ids = PROVIDERS
        .iter()
        .map(|provider| provider.id)
        .collect::<Vec<_>>();

    assert!(ids.contains(&"openai"));
    assert!(ids.contains(&"github-copilot"));
    assert!(ids.contains(&"openrouter"));
    assert!(ids.contains(&"xai"));
    assert!(ids.contains(&"azure"));
    assert!(ids.contains(&"cloudflare-ai-gateway"));
    assert!(ids.contains(&"cloudflare-workers-ai"));
    assert!(ids.contains(&"anthropic"));
    assert!(ids.contains(&"google"));
    assert!(ids.contains(&"amazon-bedrock"));
    assert!(provider_metadata("anthropic").unwrap().supported);
    assert_eq!(
        provider_metadata("deepseek").unwrap().family,
        ProviderFamily::OpenAiCompatible
    );
}

#[test]
fn provider_base_url_helpers_match_opencode_helpers() {
    assert_eq!(
        azure_resource_base_url("my-resource"),
        "https://my-resource.openai.azure.com/openai/v1"
    );
    assert_eq!(
        cloudflare_ai_gateway_base_url("acct 1", Some("gw/one")),
        "https://gateway.ai.cloudflare.com/v1/acct+1/gw%2Fone/compat"
    );
    assert_eq!(
        cloudflare_workers_ai_base_url("acct/1"),
        "https://api.cloudflare.com/client/v4/accounts/acct%2F1/ai/v1"
    );
}

#[test]
fn openrouter_body_options_match_opencode_projection() {
    assert_eq!(
        openrouter_body_options(Some(&serde_json::json!({
            "usage": true,
            "reasoning": {"effort": "high"},
            "promptCacheKey": "abc",
            "ignored": true
        }))),
        Some(serde_json::json!({
            "usage": {"include": true},
            "reasoning": {"effort": "high"},
            "prompt_cache_key": "abc"
        }))
    );
}

#[test]
fn opencode_gpt5_uses_responses_api() {
    assert!(opencode_should_use_responses_api(
        "opencode",
        "gpt-5.4-mini"
    ));
    assert!(!opencode_should_use_responses_api(
        "opencode-go",
        "kimi-k2.6"
    ));
}

#[test]
fn openai_defaults_match_opencode_store_and_gpt5_options() {
    assert_eq!(
        openai_default_provider_options("gpt-4.1", Protocol::OpenAiChat),
        Some(serde_json::json!({"store": false}))
    );
    assert_eq!(
        openai_default_provider_options("gpt-5.5", Protocol::OpenAiChat),
        Some(serde_json::json!({
            "store": false,
            "reasoning_effort": "medium"
        }))
    );
    assert_eq!(
        openai_default_provider_options("gpt-5.5", Protocol::OpenAiResponses),
        Some(serde_json::json!({
            "store": false,
            "reasoning": {"effort": "medium", "summary": "auto"},
            "text": {"verbosity": "low"}
        }))
    );
    assert_eq!(
        openai_default_provider_options("gpt-5.3-codex", Protocol::OpenAiResponses),
        Some(serde_json::json!({
            "store": false,
            "reasoning": {"effort": "medium", "summary": "auto"}
        }))
    );
    assert_eq!(
        openai_default_provider_options("gpt-5-chat", Protocol::OpenAiResponses),
        Some(serde_json::json!({"store": false}))
    );
}
