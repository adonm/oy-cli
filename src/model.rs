use crate::config;
use anyhow::{Result, bail};
use genai::adapter::AdapterKind;
use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct AuthStatus {
    pub adapter: String,
    pub env_var: Option<String>,
    pub present: bool,
    pub source: String,
    pub detail: String,
    pub auto_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelListing {
    pub current: Option<String>,
    pub current_shim: Option<String>,
    pub auth: Vec<AuthStatus>,
    pub dynamic: Vec<AdapterModels>,
    pub hints: Vec<String>,
    pub all_models: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdapterModels {
    pub adapter: String,
    pub ok: bool,
    pub source: String,
    pub count: usize,
    pub models: Vec<String>,
    pub error: Option<String>,
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
struct ShimEndpointConfig {
    shim: String,
    base_url: String,
    api_key: String,
    source: String,
}

const SHIM_OPENAI: &str = "openai";
const SHIM_CODEX: &str = "codex";
const SHIM_MANTLE: &str = "bedrock-mantle";
const SHIM_COPILOT: &str = "copilot";
const SHIM_OPENCODE: &str = "opencode";
const SHIM_ORDER: &[&str] = &[
    "local-8080",
    "local-11434",
    SHIM_OPENAI,
    SHIM_CODEX,
    SHIM_MANTLE,
    SHIM_COPILOT,
    SHIM_OPENCODE,
];
const OPENCODE_ZEN_URL: &str = "https://opencode.ai/zen/v1";

pub fn resolve_model(configured: Option<&str>) -> Result<String> {
    if let Some(value) = configured.filter(|v| !v.trim().is_empty()) {
        return Ok(canonical_model_spec(value));
    }
    if let Ok(value) = env::var("OY_MODEL") {
        if !value.trim().is_empty() {
            return Ok(canonical_model_spec(&value));
        }
    }
    if let Some(model) = config::load_model_config()?.model {
        return Ok(canonical_model_spec(&model));
    }
    bail!("No model configured. Set OY_MODEL or run `oy model <model>` to persist one.")
}

pub fn resolve_shim() -> Result<Option<String>> {
    if let Ok(value) = env::var("OY_SHIM") {
        if !value.trim().is_empty() {
            return Ok(Some(value));
        }
    }
    Ok(config::load_model_config()?.shim)
}

pub fn list_builtin_model_hints() -> Vec<String> {
    vec![
        "openai_resp::gpt-5.5".to_string(),
        "gpt-5.4-mini".to_string(),
        "gpt-4.1-mini".to_string(),
        "gemini-2.0-flash".to_string(),
        "claude-3-7-sonnet-latest".to_string(),
        "copilot::gpt-4.1-mini".to_string(),
        "local-8080::qwen3.5".to_string(),
        "local-11434::qwen3.5".to_string(),
    ]
}

pub async fn inspect_models() -> Result<ModelListing> {
    let current = resolve_model(None).ok();
    let current_shim = resolve_shim().ok().flatten();
    let dynamic = inspect_openai_compatible_models().await;
    let hints = list_builtin_model_hints();
    let all_models = collect_all_models(&dynamic, &hints);
    Ok(ModelListing {
        current,
        current_shim,
        auth: auth_statuses()
            .into_iter()
            .filter(|item| item.present || item.auto_configured)
            .collect(),
        dynamic,
        hints,
        all_models,
    })
}

fn collect_all_models(dynamic: &[AdapterModels], hints: &[String]) -> Vec<String> {
    let mut items = dynamic
        .iter()
        .flat_map(|group| group.models.iter().cloned())
        .chain(hints.iter().cloned())
        .collect::<Vec<_>>();
    items.sort();
    items.dedup();
    items
}

pub fn canonical_model_spec(spec: &str) -> String {
    spec.trim().to_string()
}

pub fn to_genai_model_spec(spec: &str) -> String {
    canonical_model_spec(spec)
}

pub fn auth_statuses() -> Vec<AuthStatus> {
    let mut items = Vec::new();
    if let Some(status) = bearer_shim_status(SHIM_OPENAI, Some("OPENAI_API_KEY")) {
        items.push(status);
    }
    if let Some(status) = local_auth_status() {
        items.push(status);
    }
    if let Some(status) = bearer_shim_status(SHIM_CODEX, Some("~/.codex/auth.json")) {
        items.push(status);
    }
    items.push(bedrock_status());
    items.push(github_status());
    if let Some(status) = bearer_shim_status(SHIM_OPENCODE, Some("OPENCODE_API_KEY, opencode auth"))
    {
        items.push(status);
    }
    items
        .into_iter()
        .filter(|item| item.present || item.auto_configured)
        .collect()
}

fn bearer_shim_status(shim: &str, env_var: Option<&str>) -> Option<AuthStatus> {
    let config = shim_endpoint_config(shim)?;
    Some(AuthStatus {
        adapter: shim.to_string(),
        env_var: env_var.map(ToOwned::to_owned),
        present: true,
        source: config.source,
        detail: format!("using {}", normalize_base_url(&config.base_url)),
        auto_configured: false,
    })
}

fn bedrock_status() -> AuthStatus {
    let region = env_value("AWS_REGION").or_else(|| env_value("AWS_DEFAULT_REGION"));
    let present = region.is_some();
    AuthStatus {
        adapter: SHIM_MANTLE.to_string(),
        env_var: Some("AWS_REGION, AWS_DEFAULT_REGION".to_string()),
        present,
        source: if present { "env" } else { "missing" }.to_string(),
        detail: region
            .map(|region| format!("AWS region configured ({region}); SigV4 routing is not yet implemented in Rust/genai path"))
            .unwrap_or_else(|| "No AWS region detected for bedrock-mantle.".to_string()),
        auto_configured: false,
    }
}

fn local_auth_status() -> Option<AuthStatus> {
    let local = env_value("LOCAL_API_KEY");
    let openai = env_value("OPENAI_API_KEY");
    let (env_var, detail) = if local.is_some() {
        (
            Some("LOCAL_API_KEY".to_string()),
            "LOCAL_API_KEY detected for OpenAI-compatible local endpoints.".to_string(),
        )
    } else if openai.is_some() {
        (
            Some("OPENAI_API_KEY".to_string()),
            "OPENAI_API_KEY will also be used for local OpenAI-compatible endpoints.".to_string(),
        )
    } else {
        return None;
    };
    Some(AuthStatus {
        adapter: "local-openai-compatible".to_string(),
        env_var,
        present: true,
        source: "env".to_string(),
        detail,
        auto_configured: false,
    })
}

async fn inspect_openai_compatible_models() -> Vec<AdapterModels> {
    let mut out = Vec::new();
    for endpoint in openai_compatible_endpoints() {
        if let Ok(models) = fetch_openai_compatible_models(&endpoint).await {
            if !models.is_empty() {
                out.push(AdapterModels {
                    adapter: endpoint.adapter,
                    ok: true,
                    source: endpoint.source,
                    count: models.len(),
                    models,
                    error: None,
                });
            }
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

fn shim_endpoint_config(shim: &str) -> Option<ShimEndpointConfig> {
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
        SHIM_CODEX => codex_openai_api_key().map(|api_key| ShimEndpointConfig {
            shim: SHIM_CODEX.to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key,
            source: "~/.codex/auth.json OPENAI_API_KEY".to_string(),
        }),
        SHIM_OPENCODE => opencode_api_key().map(|api_key| ShimEndpointConfig {
            shim: SHIM_OPENCODE.to_string(),
            base_url: OPENCODE_ZEN_URL.to_string(),
            api_key,
            source: "OPENCODE_API_KEY or opencode auth".to_string(),
        }),
        value if value.starts_with("local-") => value
            .strip_prefix("local-")
            .and_then(|port| port.parse::<u16>().ok())
            .map(|_| ShimEndpointConfig {
                shim: value.to_string(),
                base_url: local_base_url(value),
                api_key: local_api_key(),
                source: "local OpenAI-compatible endpoint".to_string(),
            }),
        SHIM_MANTLE => None,
        _ => None,
    }
}

fn extra_local_shims() -> Vec<String> {
    let mut items = BTreeSet::new();
    for source in [
        resolve_model(None).ok(),
        env_value("OY_MODEL"),
        resolve_shim().ok().flatten(),
    ] {
        if let Some(value) = source {
            let (shim, _) = config::split_model_spec(&value);
            if let Some(shim) = shim.filter(|s| s.starts_with("local-")) {
                items.insert(shim.to_string());
            }
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
    env_value("LOCAL_API_KEY")
        .or_else(|| env_value("OPENAI_API_KEY"))
        .unwrap_or_else(|| "oy-local".to_string())
}

fn codex_openai_api_key() -> Option<String> {
    json_file_value(
        dirs::home_dir()?.join(".codex/auth.json"),
        &["OPENAI_API_KEY"],
    )
}

fn opencode_api_key() -> Option<String> {
    env_value("OPENCODE_API_KEY").or_else(|| {
        json_file_nested_value(
            dirs::home_dir()?.join(".local/share/opencode/auth.json"),
            &["opencode"],
            &["key"],
        )
    })
}

fn json_file_value(path: PathBuf, keys: &[&str]) -> Option<String> {
    let value = serde_json::from_str::<Value>(&std::fs::read_to_string(path).ok()?).ok()?;
    keys.iter()
        .find_map(|key| value.get(*key)?.as_str().map(ToOwned::to_owned))
}

fn json_file_nested_value(path: PathBuf, parents: &[&str], keys: &[&str]) -> Option<String> {
    let value = serde_json::from_str::<Value>(&std::fs::read_to_string(path).ok()?).ok()?;
    for parent in parents {
        if let Some(item) = value.get(*parent) {
            for key in keys {
                if let Some(value) = item
                    .get(*key)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
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

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_string()
}

fn github_status() -> AuthStatus {
    let copilot = env_value("COPILOT_GITHUB_TOKEN");
    let gh = env_value("GH_TOKEN");
    let github = env_value("GITHUB_TOKEN");
    let auto = github_token_auto_configured();
    let present = copilot.is_some() || gh.is_some() || github.is_some() || auto;
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
        present,
        source: if auto {
            "gh"
        } else if copilot.is_some() || gh.is_some() || github.is_some() {
            "env"
        } else {
            "missing"
        }
        .to_string(),
        detail,
        auto_configured: auto,
    }
}
fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn github_token() -> Option<String> {
    env_value("COPILOT_GITHUB_TOKEN")
        .or_else(|| env_value("GH_TOKEN"))
        .or_else(|| env_value("GITHUB_TOKEN"))
        .or_else(gh_auth_token)
}

fn github_token_auto_configured() -> bool {
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

pub fn build_client() -> Result<Client> {
    let mut builder = Client::builder();
    if let Some(resolver) = service_target_resolver()? {
        builder = builder.with_service_target_resolver(resolver);
    }
    if let Some(auth) = auth_resolver()? {
        builder = builder.with_auth_resolver(auth);
    }
    Ok(builder.build())
}

fn auth_resolver() -> Result<Option<AuthResolver>> {
    let base_url = env::var("OPENAI_BASE_URL").ok();
    let api_key = env::var("OPENAI_API_KEY").ok();
    if base_url.is_none() && api_key.is_none() {
        return Ok(None);
    }
    let resolver = AuthResolver::from_resolver_fn(move |_model: ModelIden| {
        let data = api_key
            .as_ref()
            .map(|key| Some(AuthData::from_single(key.clone())))
            .unwrap_or(None);
        Ok(data)
    });
    Ok(Some(resolver))
}

fn openai_adapter_for_model(model: &str) -> AdapterKind {
    if is_openai_responses_model(model) {
        AdapterKind::OpenAIResp
    } else {
        AdapterKind::OpenAI
    }
}

fn is_openai_responses_model(model: &str) -> bool {
    let model = model
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(model);
    model.starts_with("gpt-5.5")
        || (model.starts_with("gpt") && (model.contains("codex") || model.contains("pro")))
}

fn service_target_resolver() -> Result<Option<ServiceTargetResolver>> {
    let base_url = env::var("OPENAI_BASE_URL").ok();
    let configured_shim = resolve_shim()?;
    let resolver = ServiceTargetResolver::from_resolver_fn(move |target: ServiceTarget| {
        let model_name = target.model.model_name.to_string();
        if let Some(mapped) = openai_compatible_target(&target.model, configured_shim.as_deref())
            .map_err(|err| err.to_string())?
        {
            return Ok(mapped);
        }
        if let Some(url) = base_url.as_ref() {
            return Ok(ServiceTarget {
                endpoint: Endpoint::from_owned(url.trim_end_matches('/').to_string() + "/"),
                auth: target.auth,
                model: ModelIden::new(openai_adapter_for_model(&model_name), model_name),
            });
        }
        Ok(target)
    });
    Ok(Some(resolver))
}

fn openai_compatible_target(
    model: &ModelIden,
    configured_shim: Option<&str>,
) -> Result<Option<ServiceTarget>> {
    let model_name = model.model_name.to_string();
    let (inline_shim, inline_model) = config::split_model_spec(&model_name);
    let shim = configured_shim.or(inline_shim);
    let Some(shim) = shim.filter(|shim| config::is_routing_shim(shim)) else {
        return Ok(None);
    };
    let target_model = if inline_shim.is_some() {
        ModelIden::new(
            openai_adapter_for_model(inline_model),
            inline_model.to_string(),
        )
    } else {
        model.clone()
    };
    if let Some(config) = shim_endpoint_config(shim) {
        return Ok(Some(ServiceTarget {
            endpoint: Endpoint::from_owned(normalize_base_url(&config.base_url) + "/"),
            auth: AuthData::from_single(config.api_key),
            model: target_model,
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_shim_endpoint_config_matches_python_defaults() {
        let config = shim_endpoint_config("local-8088").unwrap();
        assert_eq!(config.shim, "local-8088");
        assert_eq!(config.base_url, "http://127.0.0.1:8088/v1");
        assert_eq!(config.api_key, "oy-local");
        assert!(shim_endpoint_config("local-nope").is_none());
    }

    #[test]
    fn json_file_helpers_read_python_auth_shapes() {
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("codex.json");
        std::fs::write(&codex, r#"{"OPENAI_API_KEY":"codex-key"}"#).unwrap();
        assert_eq!(
            json_file_value(codex, &["OPENAI_API_KEY"]).as_deref(),
            Some("codex-key")
        );

        let opencode = dir.path().join("opencode.json");
        std::fs::write(&opencode, r#"{"opencode":{"key":"zen-key"}}"#).unwrap();
        assert_eq!(
            json_file_nested_value(opencode, &["opencode"], &["key"]).as_deref(),
            Some("zen-key")
        );
    }

    #[test]
    fn openai_response_only_models_use_responses_adapter() {
        assert_eq!(openai_adapter_for_model("gpt-5.5"), AdapterKind::OpenAIResp);
        assert_eq!(
            openai_adapter_for_model("openai/gpt-5.5"),
            AdapterKind::OpenAIResp
        );
        assert_eq!(
            openai_adapter_for_model("gpt-4.1-mini"),
            AdapterKind::OpenAI
        );
    }

    #[test]
    fn model_listing_includes_static_hints_as_selectable_models() {
        let hints = vec!["gpt-4.1-mini".to_string()];
        let models = collect_all_models(&[], &hints);
        assert_eq!(models, vec!["gpt-4.1-mini".to_string()]);
    }

    #[test]
    fn genai_model_spec_is_identity() {
        assert_eq!(to_genai_model_spec("copilot::gpt-5.5"), "copilot::gpt-5.5");
        assert_eq!(to_genai_model_spec("gpt-5.4-mini"), "gpt-5.4-mini");
        assert_eq!(
            canonical_model_spec("  local-8080::qwen3.5  "),
            "local-8080::qwen3.5"
        );
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
}
