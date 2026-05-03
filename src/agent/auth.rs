use serde::{Deserialize, Serialize};

use super::endpoints::{
    SHIM_OPENAI, env_value, github_token_auto_configured, normalize_base_url, shim_endpoint_config,
};

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
    if let Some(status) = bearer_shim_status(SHIM_OPENAI, Some("OPENAI_API_KEY")) {
        items.push(status);
    }
    if let Some(status) = local_auth_status() {
        items.push(status);
    }
    items.push(github_status());
    items.push(bedrock_status());
    items
        .into_iter()
        .filter(|item| item.availability.is_available())
        .collect()
}

fn bearer_shim_status(shim: &str, env_var: Option<&str>) -> Option<AuthStatus> {
    let config = shim_endpoint_config(shim)?;
    Some(AuthStatus {
        adapter: shim.to_string(),
        env_var: env_var.map(ToOwned::to_owned),
        availability: AuthAvailability::Present,
        source: config.source,
        detail: format!("using {}", normalize_base_url(&config.base_url)),
    })
}

fn local_auth_status() -> Option<AuthStatus> {
    let _local = env_value("LOCAL_API_KEY")?;
    Some(AuthStatus {
        adapter: "local-openai-compatible".to_string(),
        env_var: Some("LOCAL_API_KEY".to_string()),
        availability: AuthAvailability::Present,
        source: "env".to_string(),
        detail: "LOCAL_API_KEY detected for OpenAI-compatible local endpoints.".to_string(),
    })
}

fn github_status() -> AuthStatus {
    let copilot = env_value("COPILOT_GITHUB_TOKEN");
    let gh = env_value("GH_TOKEN");
    let github = env_value("GITHUB_TOKEN");
    let auto = github_token_auto_configured();
    let env_present = copilot.is_some() || gh.is_some() || github.is_some();
    let availability = match (env_present, auto) {
        (true, _) => AuthAvailability::Present,
        (false, true) => AuthAvailability::AutoConfigured,
        (false, false) => AuthAvailability::Missing,
    };
    let detail = match (copilot.as_deref(), gh.as_deref(), github.as_deref(), auto) {
        (Some(_), _, _, _) => {
            "COPILOT_GITHUB_TOKEN detected; copilot-compatible auth available.".to_string()
        }
        (None, Some(_), _, _) => {
            "GH_TOKEN detected; copilot-compatible auth available.".to_string()
        }
        (None, None, Some(_), _) => {
            "GITHUB_TOKEN detected; copilot-compatible auth available.".to_string()
        }
        (None, None, None, true) => "GitHub token available from `gh auth token`.".to_string(),
        (None, None, None, false) => "No GitHub auth token detected.".to_string(),
    };
    AuthStatus {
        adapter: "github".to_string(),
        env_var: Some("COPILOT_GITHUB_TOKEN, GH_TOKEN, GITHUB_TOKEN".to_string()),
        availability,
        source: if auto {
            "gh"
        } else if copilot.is_some() || gh.is_some() || github.is_some() {
            "env"
        } else {
            "missing"
        }
        .to_string(),
        detail,
    }
}

fn bedrock_status() -> AuthStatus {
    let status = crate::bedrock::auth_status();
    AuthStatus {
        adapter: "bedrock".to_string(),
        env_var: Some("AWS_ACCESS_KEY_ID, AWS_PROFILE".to_string()),
        availability: match status.availability {
            crate::bedrock::AwsAuthAvailability::Present => AuthAvailability::Present,
            crate::bedrock::AwsAuthAvailability::AutoConfigured => AuthAvailability::AutoConfigured,
            crate::bedrock::AwsAuthAvailability::Missing => AuthAvailability::Missing,
        },
        source: status.source,
        detail: status.detail,
    }
}
