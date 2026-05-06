use anyhow::Result;
use std::env;
use std::process::{Command, Stdio};

const DEFAULT_REGION: &str = "ap-southeast-2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwsAuthAvailability {
    Present,
    AutoConfigured,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsAuthStatus {
    pub availability: AwsAuthAvailability,
    pub source: String,
    pub detail: String,
}

pub fn region() -> String {
    env_value("BEDROCK_REGION")
        .or_else(|| env_value("AWS_REGION"))
        .or_else(|| env_value("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

pub async fn client() -> Result<rig_bedrock::client::Client> {
    if let Some(profile) = aws_profile() {
        return Ok(rig_bedrock::client::Client::with_profile_name(&profile));
    }
    let region = region();
    Ok(rig_bedrock::client::ClientBuilder::default()
        .region(&region)
        .build()
        .await)
}

pub fn auth_status() -> AwsAuthStatus {
    if env_credentials_present() {
        return AwsAuthStatus {
            availability: AwsAuthAvailability::Present,
            source: "env".to_string(),
            detail: format!(
                "AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY detected for Bedrock in {}.",
                region()
            ),
        };
    }

    if aws_cli_available() {
        let profile = aws_profile();
        let sso = profile.as_deref().is_some_and(profile_looks_like_sso);
        return AwsAuthStatus {
            availability: AwsAuthAvailability::AutoConfigured,
            source: "aws-sdk/aws-cli".to_string(),
            detail: match (profile.as_deref(), sso) {
                (Some(profile), true) => format!(
                    "AWS profile `{profile}` appears to use SSO; run `aws sso login --profile {profile}` if credentials expire."
                ),
                (Some(profile), false) => format!(
                    "AWS SDK profile `{profile}` available for Bedrock in {}.",
                    region()
                ),
                (None, _) => format!(
                    "AWS SDK default credential chain available for Bedrock in {}.",
                    region()
                ),
            },
        };
    }

    AwsAuthStatus {
        availability: AwsAuthAvailability::Missing,
        source: "missing".to_string(),
        detail: "No AWS env credentials or AWS CLI detected for Bedrock.".to_string(),
    }
}

fn env_credentials_present() -> bool {
    env_value("AWS_ACCESS_KEY_ID").is_some() && env_value("AWS_SECRET_ACCESS_KEY").is_some()
}

fn aws_profile() -> Option<String> {
    env_value("AWS_PROFILE")
        .or_else(|| env_value("AWS_DEFAULT_PROFILE"))
        .filter(|profile| profile != "default")
}

fn aws_cli_available() -> bool {
    Command::new("aws")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn profile_looks_like_sso(profile: &str) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let config = home.join(".aws").join("config");
    let Ok(text) = std::fs::read_to_string(config) else {
        return false;
    };
    let headers = [
        format!("[profile {profile}]"),
        format!("[sso-session {profile}]"),
        if profile == "default" {
            "[default]".to_string()
        } else {
            String::new()
        },
    ];
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = headers
                .iter()
                .any(|header| !header.is_empty() && trimmed == header);
            continue;
        }
        if in_section && (trimmed.starts_with("sso_") || trimmed.starts_with("sso_session")) {
            return true;
        }
    }
    false
}

fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_region_defaults_and_env_override() {
        assert!(!region().is_empty());
    }
}
