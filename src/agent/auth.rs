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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GitHubCopilotAuth {
    ApiKey(String),
    GitHubAccessToken(String),
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

pub(crate) fn github_copilot_auth() -> Option<GitHubCopilotAuth> {
    copilot_api_key()
        .map(GitHubCopilotAuth::ApiKey)
        .or_else(|| github_access_token().map(GitHubCopilotAuth::GitHubAccessToken))
}

fn copilot_api_key() -> Option<String> {
    first_nonempty([
        || provider_api_key("github-copilot"),
        || env_value("COPILOT_API_KEY"),
        || env_value("OPENCODE_API_KEY"),
        || opencode_auth_key_from_path("github-copilot", opencode_auth_path()),
    ])
}

fn github_access_token() -> Option<String> {
    first_nonempty([
        || env_value("COPILOT_GITHUB_ACCESS_TOKEN"),
        || env_value("COPILOT_GITHUB_TOKEN"),
        || env_value("GH_TOKEN"),
        || env_value("GITHUB_TOKEN"),
        gh_auth_token,
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
    let auth = github_copilot_auth();
    let auto = github_token_auto_configured();
    let api_key = copilot_api_key();
    let present = auth.is_some();
    let availability = match (present, auto) {
        (true, _) => AuthAvailability::Present,
        (false, true) => AuthAvailability::AutoConfigured,
        (false, false) => AuthAvailability::Missing,
    };
    let detail = if api_key.is_some() {
        "Copilot API token detected; direct Copilot auth available.".to_string()
    } else if auth.is_some() && auto {
        "GitHub token available from `gh auth token`.".to_string()
    } else if auth.is_some() {
        "GitHub token detected; copilot-compatible auth available.".to_string()
    } else {
        "No GitHub auth token detected.".to_string()
    };
    AuthStatus {
        adapter: "github-copilot".to_string(),
        env_var: Some(
            "GITHUB_COPILOT_API_KEY, COPILOT_API_KEY, COPILOT_GITHUB_ACCESS_TOKEN, COPILOT_GITHUB_TOKEN, GH_TOKEN, GITHUB_TOKEN, OpenCode auth.json".to_string(),
        ),
        availability,
        source: if api_key.is_some() {
            "copilot api token"
        } else if auto {
            "gh"
        } else if auth.is_some() {
            "env"
        } else {
            "missing"
        }
        .to_string(),
        detail,
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
            .or_else(|| provider_value.get("refresh"))
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

fn github_token_auto_configured() -> bool {
    copilot_api_key().is_none()
        && env_value("COPILOT_GITHUB_ACCESS_TOKEN").is_none()
        && env_value("COPILOT_GITHUB_TOKEN").is_none()
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
    use super::*;

    #[test]
    fn opencode_reads_generic_auth_json_shapes() {
        let value = serde_json::json!({
            "anthropic": { "type": "api", "key": "anthropic-token" },
            "github-copilot": { "type": "wellknown", "token": "copilot-token" },
            "github-copilot-oauth": { "type": "oauth", "refresh": "copilot-oauth-token", "access": "old-token" }
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
    }
}
