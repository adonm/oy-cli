use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::config;

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
struct OpenAiCompatibleEndpoint {
    adapter: String,
    base_url: String,
    api_key: String,
    shim: Option<String>,
    source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ShimEndpointConfig {
    pub(super) shim: String,
    pub(super) base_url: String,
    pub(super) api_key: String,
    pub(super) source: String,
}

pub(super) const SHIM_OPENAI: &str = "openai";
pub(super) const SHIM_COPILOT: &str = "copilot";
pub(super) const SHIM_BEDROCK_MANTLE: &str = "bedrock-mantle";
pub(super) const SHIM_OPENCODE: &str = "opencode";
pub(super) const SHIM_OPENCODE_GO: &str = "opencode-go";
const SHIM_ORDER: &[&str] = &[
    SHIM_OPENAI,
    SHIM_COPILOT,
    SHIM_BEDROCK_MANTLE,
    SHIM_OPENCODE,
    SHIM_OPENCODE_GO,
];

pub(super) async fn inspect_openai_compatible_models() -> Vec<AdapterModels> {
    let mut out = Vec::new();
    for endpoint in openai_compatible_endpoints() {
        match fetch_openai_compatible_models(&endpoint).await {
            Ok(models) if !models.is_empty() => out.push(AdapterModels::Available {
                adapter: endpoint.adapter,
                source: endpoint.source,
                count: models.len(),
                models,
            }),
            Ok(_) => {}
            Err(err) => out.push(AdapterModels::Failed {
                adapter: endpoint.adapter,
                source: endpoint.source,
                error: err.to_string(),
            }),
        }
    }
    out
}

fn openai_compatible_endpoints() -> Vec<OpenAiCompatibleEndpoint> {
    let mut endpoints = Vec::new();
    let mut seen = BTreeSet::new();
    for shim in SHIM_ORDER
        .iter()
        .copied()
        .chain(extra_local_shims().iter().map(String::as_str))
    {
        if let Some(config) = shim_endpoint_config(shim) {
            push_endpoint(
                &mut endpoints,
                &mut seen,
                OpenAiCompatibleEndpoint {
                    adapter: config.shim.clone(),
                    source: format!("GET {}/models", normalize_base_url(&config.base_url)),
                    base_url: config.base_url,
                    api_key: config.api_key,
                    shim: Some(config.shim),
                },
            );
        }
    }
    endpoints
}

pub(super) fn shim_endpoint_config(shim: &str) -> Option<ShimEndpointConfig> {
    match shim {
        SHIM_OPENAI => env_value("OPENAI_API_KEY").map(|api_key| ShimEndpointConfig {
            shim: SHIM_OPENAI.to_string(),
            base_url: env_value("OPENAI_BASE_URL")
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            api_key,
            source: "OPENAI_API_KEY".to_string(),
        }),
        SHIM_COPILOT => github_token().map(|api_key| ShimEndpointConfig {
            shim: SHIM_COPILOT.to_string(),
            base_url: env_value("COPILOT_BASE_URL")
                .unwrap_or_else(|| "https://api.githubcopilot.com".to_string()),
            api_key,
            source: "GitHub token".to_string(),
        }),
        SHIM_BEDROCK_MANTLE => bearer_endpoint_config(
            SHIM_BEDROCK_MANTLE,
            || {
                env_value("BEDROCK_MANTLE_BASE_URL").unwrap_or_else(|| {
                    format!(
                        "https://bedrock-mantle.{}.api.aws/v1",
                        crate::bedrock::region()
                    )
                })
            },
            &[
                (
                    "BEDROCK_MANTLE_API_KEY",
                    env_value("BEDROCK_MANTLE_API_KEY"),
                ),
                (
                    "AWS_BEARER_TOKEN_BEDROCK",
                    env_value("AWS_BEARER_TOKEN_BEDROCK"),
                ),
            ],
        ),
        SHIM_OPENCODE => opencode_endpoint_config(SHIM_OPENCODE, "https://opencode.ai/zen/v1"),
        SHIM_OPENCODE_GO => {
            opencode_endpoint_config(SHIM_OPENCODE_GO, "https://opencode.ai/zen/go/v1")
        }
        value if value.starts_with("local-") => value
            .strip_prefix("local-")
            .and_then(|port| port.parse::<u16>().ok())
            .map(|_| ShimEndpointConfig {
                shim: value.to_string(),
                base_url: local_base_url(value),
                api_key: local_api_key(),
                source: local_api_key_source(),
            }),
        _ => None,
    }
}

fn bearer_endpoint_config(
    shim: &str,
    base_url: impl FnOnce() -> String,
    credentials: &[(&str, Option<String>)],
) -> Option<ShimEndpointConfig> {
    let (source, api_key) = credentials
        .iter()
        .find_map(|(source, value)| value.as_ref().map(|api_key| (*source, api_key.clone())))?;
    Some(ShimEndpointConfig {
        shim: shim.to_string(),
        base_url: base_url(),
        api_key,
        source: source.to_string(),
    })
}

fn opencode_endpoint_config(shim: &str, default_base_url: &str) -> Option<ShimEndpointConfig> {
    bearer_endpoint_config(
        shim,
        || opencode_base_url(shim, default_base_url),
        &[
            ("OPENCODE_API_KEY", env_value("OPENCODE_API_KEY")),
            ("opencode auth.json", opencode_auth_key(shim)),
        ],
    )
}

fn opencode_base_url(shim: &str, default_base_url: &str) -> String {
    let shim_env = format!("{}_BASE_URL", shim.to_ascii_uppercase().replace('-', "_"));
    env_value(&shim_env)
        .or_else(|| env_value("OPENCODE_BASE_URL"))
        .unwrap_or_else(|| default_base_url.to_string())
}

fn opencode_auth_key(shim: &str) -> Option<String> {
    opencode_auth_key_from_path(shim, opencode_auth_path())
}

fn opencode_auth_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("opencode")
        .join("auth.json")
}

fn opencode_auth_key_from_path(provider: &str, path: PathBuf) -> Option<String> {
    let value = fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
    opencode_auth_key_from_value(provider, &value)
}

fn opencode_auth_key_from_value(provider: &str, value: &Value) -> Option<String> {
    let provider_value = opencode_provider_candidates(provider)
        .into_iter()
        .find_map(|candidate| value.get(candidate))?;
    match provider_value.get("type").and_then(Value::as_str) {
        Some("api") => provider_value
            .get("key")
            .and_then(Value::as_str)
            .filter(|key| !key.trim().is_empty())
            .map(ToOwned::to_owned),
        Some("wellknown") => provider_value
            .get("token")
            .or_else(|| provider_value.get("key"))
            .and_then(Value::as_str)
            .filter(|key| !key.trim().is_empty())
            .map(ToOwned::to_owned),
        _ => None,
    }
}

fn opencode_provider_candidates(provider: &str) -> Vec<&'static str> {
    match provider {
        SHIM_OPENCODE => vec![SHIM_OPENCODE, SHIM_OPENCODE_GO],
        SHIM_OPENCODE_GO => vec![SHIM_OPENCODE_GO, SHIM_OPENCODE],
        _ => Vec::new(),
    }
}

fn extra_local_shims() -> Vec<String> {
    let mut items = BTreeSet::new();
    for value in [
        super::model::resolve_model(None).ok(),
        env_value("OY_MODEL"),
        super::model::resolve_shim().ok().flatten(),
    ]
    .into_iter()
    .flatten()
    {
        let (shim, _) = config::split_model_spec(&value);
        if let Some(shim) = shim.filter(|s| s.starts_with("local-")) {
            items.insert(shim.to_string());
        }
    }
    items.into_iter().collect()
}

fn push_endpoint(
    endpoints: &mut Vec<OpenAiCompatibleEndpoint>,
    seen: &mut BTreeSet<String>,
    endpoint: OpenAiCompatibleEndpoint,
) {
    let key = format!(
        "{}
{}
{}",
        endpoint.adapter,
        normalize_base_url(&endpoint.base_url),
        endpoint.shim.clone().unwrap_or_default()
    );
    if seen.insert(key) {
        endpoints.push(endpoint);
    }
}

fn local_base_url(shim: &str) -> String {
    if let Some(port) = shim.strip_prefix("local-") {
        let env_name = format!("OY_LOCAL_{}_URL", port);
        if let Some(url) = env_value(&env_name) {
            return url;
        }
        return format!("http://127.0.0.1:{port}/v1");
    }
    "http://127.0.0.1:8080/v1".to_string()
}

fn local_api_key() -> String {
    env_value("LOCAL_API_KEY").unwrap_or_else(|| "oy-local".to_string())
}

fn local_api_key_source() -> String {
    if env_value("LOCAL_API_KEY").is_some() {
        "LOCAL_API_KEY".to_string()
    } else {
        "default local auth".to_string()
    }
}

async fn fetch_openai_compatible_models(
    endpoint: &OpenAiCompatibleEndpoint,
) -> Result<Vec<String>> {
    let url = format!("{}/models", normalize_base_url(&endpoint.base_url));
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?
        .get(&url)
        .bearer_auth(&endpoint.api_key)
        .header("Accept", "application/json")
        .send()
        .await?;
    if !response.status().is_success() {
        bail!("GET {url} failed with HTTP {}", response.status());
    }
    let value = response.json::<Value>().await?;
    let models = extract_model_ids(&value)
        .into_iter()
        .map(|id| match endpoint.shim.as_deref() {
            Some(prefix) => format!("{prefix}::{id}"),
            None => id,
        })
        .collect::<Vec<_>>();
    Ok(models)
}

fn extract_model_ids(value: &Value) -> Vec<String> {
    let data = if let Some(items) = value.get("data").and_then(Value::as_array) {
        items.clone()
    } else if let Some(items) = value.as_array() {
        items.clone()
    } else {
        Vec::new()
    };
    let mut ids = data
        .into_iter()
        .filter_map(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    ids
}

pub(super) fn normalize_base_url(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_string()
}

pub(super) fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn github_token() -> Option<String> {
    env_value("COPILOT_GITHUB_TOKEN")
        .or_else(|| env_value("GH_TOKEN"))
        .or_else(|| env_value("GITHUB_TOKEN"))
        .or_else(gh_auth_token)
}

pub(super) fn github_token_auto_configured() -> bool {
    env_value("COPILOT_GITHUB_TOKEN").is_none()
        && env_value("GH_TOKEN").is_none()
        && env_value("GITHUB_TOKEN").is_none()
        && gh_auth_token().is_some()
}

fn gh_auth_token() -> Option<String> {
    let output = std::process::Command::new("gh")
        .arg("auth")
        .arg("token")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::super::model::ENV_TEST_LOCK;
    use super::*;

    #[test]
    fn local_shim_endpoint_config_uses_default_local_auth() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        unsafe { std::env::remove_var("LOCAL_API_KEY") };
        unsafe { std::env::remove_var("OY_SHIM") };
        unsafe { std::env::set_var("OPENAI_API_KEY", "openai-token") };
        let config = shim_endpoint_config("local-8088").unwrap();
        assert_eq!(config.shim, "local-8088");
        assert_eq!(config.base_url, "http://127.0.0.1:8088/v1");
        assert_eq!(config.api_key, "oy-local");
        assert_eq!(config.source, "default local auth");

        unsafe { std::env::set_var("LOCAL_API_KEY", "local-token") };
        let config = shim_endpoint_config("local-8088").unwrap();
        assert_eq!(config.api_key, "local-token");
        assert_eq!(config.source, "LOCAL_API_KEY");
        assert!(shim_endpoint_config("local-nope").is_none());
        unsafe { std::env::remove_var("LOCAL_API_KEY") };
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
    }

    #[test]
    fn extract_model_ids_handles_openai_shape() {
        let value = serde_json::json!({
            "data": [
                {"id": "gpt-4.1-mini"},
                {"id": "gpt-4.1"},
                {"id": "gpt-4.1-mini"}
            ]
        });
        assert_eq!(
            extract_model_ids(&value),
            vec!["gpt-4.1".to_string(), "gpt-4.1-mini".to_string()]
        );
    }

    #[test]
    fn bedrock_mantle_requires_bedrock_specific_bearer_token() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        unsafe { std::env::remove_var("BEDROCK_MANTLE_API_KEY") };
        unsafe { std::env::remove_var("BEDROCK_MANTLE_BASE_URL") };
        unsafe { std::env::remove_var("OPENAI_BASE_URL") };
        unsafe { std::env::set_var("OPENAI_API_KEY", "openai-token") };
        assert!(shim_endpoint_config(SHIM_BEDROCK_MANTLE).is_none());

        unsafe { std::env::set_var("AWS_BEARER_TOKEN_BEDROCK", "bedrock-token") };
        unsafe { std::env::set_var("OPENAI_BASE_URL", "https://openai.example/v1") };
        let config = shim_endpoint_config(SHIM_BEDROCK_MANTLE).unwrap();
        assert_eq!(config.api_key, "bedrock-token");
        assert_eq!(config.source, "AWS_BEARER_TOKEN_BEDROCK");
        assert_eq!(
            config.base_url,
            format!(
                "https://bedrock-mantle.{}.api.aws/v1",
                crate::bedrock::region()
            )
        );
        unsafe { std::env::remove_var("AWS_BEARER_TOKEN_BEDROCK") };
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
        unsafe { std::env::remove_var("OPENAI_BASE_URL") };
    }

    #[test]
    fn opencode_reads_api_key_from_auth_json_shapes() {
        let value = serde_json::json!({
            "opencode": { "type": "api", "key": "zen-token" },
            "opencode-go": { "type": "wellknown", "token": "go-token" }
        });
        assert_eq!(
            opencode_auth_key_from_value(SHIM_OPENCODE, &value),
            Some("zen-token".to_string())
        );
        assert_eq!(
            opencode_auth_key_from_value(SHIM_OPENCODE_GO, &value),
            Some("go-token".to_string())
        );

        let only_zen = serde_json::json!({
            "opencode": { "type": "api", "key": "shared-token" }
        });
        assert_eq!(
            opencode_auth_key_from_value(SHIM_OPENCODE_GO, &only_zen),
            Some("shared-token".to_string())
        );

        let only_go = serde_json::json!({
            "opencode-go": { "type": "wellknown", "token": "go-shared-token" }
        });
        assert_eq!(
            opencode_auth_key_from_value(SHIM_OPENCODE, &only_go),
            Some("go-shared-token".to_string())
        );
    }

    #[test]
    fn opencode_env_key_wins_over_auth_json() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        unsafe { std::env::set_var("OPENCODE_API_KEY", "env-token") };
        unsafe { std::env::set_var("OPENCODE_BASE_URL", "https://example.invalid/v1") };
        let config = shim_endpoint_config(SHIM_OPENCODE).unwrap();
        assert_eq!(config.api_key, "env-token");
        assert_eq!(config.source, "OPENCODE_API_KEY");
        assert_eq!(config.base_url, "https://example.invalid/v1");
        let go_config = shim_endpoint_config(SHIM_OPENCODE_GO).unwrap();
        assert_eq!(go_config.api_key, "env-token");
        assert_eq!(go_config.source, "OPENCODE_API_KEY");
        assert_eq!(go_config.base_url, "https://example.invalid/v1");
        unsafe { std::env::remove_var("OPENCODE_API_KEY") };
        unsafe { std::env::remove_var("OPENCODE_BASE_URL") };
    }
}
