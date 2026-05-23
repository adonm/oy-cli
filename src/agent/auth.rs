//! Environment, OpenCode, and GitHub Copilot API-token credential
//! lookup for model providers.
//!
//! This module is the single source of provider auth probing. Callers
//! check availability with [`auth_statuses`] or the narrower helpers
//! without duplicating credential paths.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthAvailability {
    Present,
    AutoConfigured,
    Missing,
}

impl AuthAvailability {
    pub fn is_available(self) -> bool {
        matches!(self, Self::Present | Self::AutoConfigured)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatus {
    pub adapter: String,
    pub env_var: Option<String>,
    pub availability: AuthAvailability,
    pub source: String,
    pub detail: String,
}

pub(crate) fn auth_statuses() -> Vec<AuthStatus> {
    let mut items = Vec::new();
    if let Some(status) = openai_status() {
        items.push(status);
    }
    for provider in opencode_auth_providers() {
        if let Some(status) = opencode_status(&provider) {
            items.push(status);
        }
    }
    items.push(github_status());
    items
        .into_iter()
        .filter(|item| item.availability.is_available())
        .collect()
}

pub(crate) fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn first_nonempty<const N: usize>(candidates: [fn() -> Option<String>; N]) -> Option<String> {
    candidates.into_iter().find_map(|candidate| candidate())
}

pub(crate) fn opencode_auth_key(provider: &str) -> Option<String> {
    provider_api_key(provider)
        .or_else(|| env_value("OPENCODE_API_KEY"))
        .or_else(|| opencode_auth_key_from_path(provider, opencode_auth_path()))
}

pub(crate) fn github_copilot_api_key() -> Option<String> {
    copilot_api_key()
}

fn copilot_api_key() -> Option<String> {
    first_nonempty([
        || provider_api_key("github-copilot"),
        || env_value("COPILOT_API_KEY"),
        || env_value("OPENCODE_API_KEY"),
        || opencode_auth_key_from_path("github-copilot", opencode_auth_path()),
    ])
}

fn opencode_status(provider: &str) -> Option<AuthStatus> {
    let _key = opencode_auth_key(provider)?;
    Some(AuthStatus {
        adapter: provider.to_string(),
        env_var: Some("OPENCODE_API_KEY or OpenCode auth.json".to_string()),
        availability: AuthAvailability::Present,
        source: if env_value("OPENCODE_API_KEY").is_some() {
            "env"
        } else {
            "opencode auth.json"
        }
        .to_string(),
        detail: format!("OpenCode credentials detected for `{provider}`."),
    })
}

fn openai_status() -> Option<AuthStatus> {
    let _key = env_value("OPENAI_API_KEY")?;
    Some(AuthStatus {
        adapter: "openai".to_string(),
        env_var: Some("OPENAI_API_KEY".to_string()),
        availability: AuthAvailability::Present,
        source: "env".to_string(),
        detail: format!(
            "using {}",
            env_value("OPENAI_BASE_URL")
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
                .trim_end_matches('/')
        ),
    })
}

fn github_status() -> AuthStatus {
    let api_key = copilot_api_key();
    AuthStatus {
        adapter: "github-copilot".to_string(),
        env_var: Some(
            "GITHUB_COPILOT_API_KEY, COPILOT_API_KEY, OPENCODE_API_KEY, OpenCode auth.json"
                .to_string(),
        ),
        availability: if api_key.is_some() {
            AuthAvailability::Present
        } else {
            AuthAvailability::Missing
        },
        source: if api_key.is_some() {
            "copilot api token"
        } else {
            "missing"
        }
        .to_string(),
        detail: if api_key.is_some() {
            "Copilot API token detected; native OpenAI-compatible auth available.".to_string()
        } else {
            "No Copilot API token detected.".to_string()
        },
    }
}

fn opencode_auth_providers() -> Vec<String> {
    let value = fs::read_to_string(opencode_auth_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let Some(Value::Object(map)) = value else {
        return Vec::new();
    };
    let root = Value::Object(map.clone());
    let mut providers = map
        .keys()
        .filter(|provider| opencode_auth_key_from_value(provider, &root).is_some())
        .cloned()
        .collect::<Vec<_>>();
    providers.sort();
    providers.dedup();
    providers
}

fn provider_api_key(provider: &str) -> Option<String> {
    let env_name = format!(
        "{}_API_KEY",
        provider.to_ascii_uppercase().replace(['-', '.'], "_")
    );
    env_value(&env_name)
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
    let provider_value = value
        .get(provider)
        .or_else(|| provider_alias(provider).and_then(|alias| value.get(alias)))?;
    match provider_value.get("type").and_then(Value::as_str) {
        Some("api") => provider_value
            .get("key")
            .and_then(Value::as_str)
            .filter(|key| !key.trim().is_empty())
            .map(ToOwned::to_owned),
        Some("wellknown") => provider_value
            .get("token")
            .or_else(|| provider_value.get("key"))
            .or_else(|| provider_value.get("access"))
            .and_then(Value::as_str)
            .filter(|key| !key.trim().is_empty())
            .map(ToOwned::to_owned),
        Some("oauth") => provider_value
            .get("access")
            .and_then(Value::as_str)
            .filter(|key| !key.trim().is_empty())
            .map(ToOwned::to_owned),
        _ => None,
    }
}

fn provider_alias(provider: &str) -> Option<&'static str> {
    match provider {
        "copilot" => Some("github-copilot"),
        "opencode-go" => Some("opencode"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_reads_generic_auth_json_shapes() {
        let value = serde_json::json!({
            "anthropic": { "type": "api", "key": "anthropic-token" },
            "github-copilot": { "type": "wellknown", "token": "copilot-token" },
            "github-copilot-oauth": { "type": "oauth", "refresh": "copilot-refresh-token", "access": "old-token" },
            "refresh-only-oauth": { "type": "oauth", "refresh": "copilot-refresh-token" }
        });
        assert_eq!(
            opencode_auth_key_from_value("anthropic", &value),
            Some("anthropic-token".to_string())
        );
        assert_eq!(
            opencode_auth_key_from_value("copilot", &value),
            Some("copilot-token".to_string())
        );
        assert_eq!(
            opencode_auth_key_from_value("github-copilot-oauth", &value),
            Some("old-token".to_string())
        );
        assert_eq!(
            opencode_auth_key_from_value("refresh-only-oauth", &value),
            None
        );
    }
}
