use crate::config;
use anyhow::{Context, Result, bail};
use genai::adapter::AdapterKind;
use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use isahc::{AsyncReadResponseExt, Request, config::Configurable};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
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
    model_prefix: Option<String>,
    source: String,
}

pub fn resolve_model(configured: Option<&str>) -> Result<String> {
    if let Some(value) = configured.filter(|v| !v.trim().is_empty()) {
        return Ok(canonical_model_spec(value));
    }
    if let Ok(value) = env::var("OY_MODEL") {
        if value.contains("::") || config::split_model_spec(&value).0.is_some() {
            return Ok(canonical_model_spec(&value));
        }
        if let Some(shim) = resolve_shim(None)? {
            return Ok(canonical_model_spec(&config::join_model_spec(
                &shim, &value,
            )));
        }
        return Ok(canonical_model_spec(&value));
    }
    let saved = config::load_model_config()?;
    if let (Some(shim), Some(model)) = (saved.shim.as_deref(), saved.model.as_deref()) {
        return Ok(canonical_model_spec(&config::join_model_spec(shim, model)));
    }
    if let Some(model) = saved.model {
        return Ok(canonical_model_spec(&model));
    }
    bail!("No model configured. Set OY_MODEL or run `oy model <model>` to persist one.")
}

pub fn resolve_shim(configured: Option<&str>) -> Result<Option<String>> {
    if let Some(value) = configured.filter(|v| !v.trim().is_empty()) {
        return Ok(Some(value.to_string()));
    }
    if let Ok(value) = env::var("OY_SHIM") {
        if !value.trim().is_empty() {
            return Ok(Some(value));
        }
    }
    Ok(config::load_model_config()?.shim)
}

pub fn list_builtin_model_hints() -> Vec<String> {
    vec![
        "gpt-5.4-mini".to_string(),
        "gpt-4.1-mini".to_string(),
        "gemini-2.0-flash".to_string(),
        "claude-3-7-sonnet-latest".to_string(),
        "github_copilot::openai/gpt-4.1-mini".to_string(),
        "local-8080::qwen3.5".to_string(),
        "local-11434::qwen3.5".to_string(),
    ]
}

pub async fn inspect_models() -> Result<ModelListing> {
    auto_configure_auth()?;
    let current = resolve_model(None).ok();
    let dynamic = inspect_openai_compatible_models().await;
    let all_models = collect_all_models(&dynamic);
    Ok(ModelListing {
        current,
        auth: auth_statuses()
            .into_iter()
            .filter(|item| item.present || item.auto_configured)
            .collect(),
        dynamic,
        hints: list_builtin_model_hints(),
        all_models,
    })
}

fn collect_all_models(dynamic: &[AdapterModels]) -> Vec<String> {
    let mut items = dynamic
        .iter()
        .flat_map(|group| group.models.iter().cloned())
        .collect::<Vec<_>>();
    items.sort();
    items.dedup();
    items
}

pub fn canonical_model_spec(spec: &str) -> String {
    to_genai_model_spec(spec)
}

pub fn to_genai_model_spec(spec: &str) -> String {
    let (shim, model) = config::split_model_spec(spec);
    match shim {
        None => spec.to_string(),
        Some("openai") => model.to_string(),
        Some("copilot") | Some("github-copilot") | Some("github_copilot") => {
            format!("github_copilot::{model}")
        }
        Some(other) => format!("{other}::{model}"),
    }
}

pub fn auth_statuses() -> Vec<AuthStatus> {
    let mut items = Vec::new();
    if env_value("OPENAI_API_KEY").is_some() {
        items.push(AuthStatus {
            adapter: if env_value("OPENAI_BASE_URL").is_some() {
                "openai-compatible".to_string()
            } else {
                "openai".to_string()
            },
            env_var: Some("OPENAI_API_KEY".to_string()),
            present: true,
            source: "env".to_string(),
            detail: if let Some(url) = env_value("OPENAI_BASE_URL") {
                format!(
                    "using OPENAI_BASE_URL={} for OpenAI-compatible /models introspection",
                    url
                )
            } else {
                "using default OpenAI /v1 endpoint for /models introspection".to_string()
            },
            auto_configured: false,
        });
    }
    if let Some(status) = local_auth_status() {
        items.push(status);
    }
    items.push(github_status());
    items
        .into_iter()
        .filter(|item| item.present || item.auto_configured)
        .collect()
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

    if let Some(key) = env_value("OPENAI_API_KEY") {
        let base =
            env_value("OPENAI_BASE_URL").unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        push_endpoint(
            &mut endpoints,
            &mut seen,
            OpenAiCompatibleEndpoint {
                adapter: if env_value("OPENAI_BASE_URL").is_some() {
                    "openai-compatible".to_string()
                } else {
                    "openai".to_string()
                },
                source: format!("GET {}/models", normalize_base_url(&base)),
                base_url: base,
                api_key: key,
                model_prefix: None,
            },
        );
    }

    if let Some(key) = github_token() {
        let base = env_value("COPILOT_BASE_URL")
            .unwrap_or_else(|| "https://api.githubcopilot.com".to_string());
        push_endpoint(
            &mut endpoints,
            &mut seen,
            OpenAiCompatibleEndpoint {
                adapter: "github_copilot".to_string(),
                source: format!("GET {}/models", normalize_base_url(&base)),
                base_url: base,
                api_key: key,
                model_prefix: Some("github_copilot".to_string()),
            },
        );
    }

    for shim in local_model_shims() {
        let base = local_base_url(&shim);
        let key = local_api_key();
        push_endpoint(
            &mut endpoints,
            &mut seen,
            OpenAiCompatibleEndpoint {
                adapter: shim.clone(),
                source: format!("GET {}/models", normalize_base_url(&base)),
                base_url: base,
                api_key: key.clone(),
                model_prefix: Some(shim),
            },
        );
    }

    endpoints
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
        endpoint.model_prefix.clone().unwrap_or_default()
    );
    if seen.insert(key) {
        endpoints.push(endpoint);
    }
}

fn local_model_shims() -> Vec<String> {
    let mut items = BTreeSet::new();
    items.insert("local-8080".to_string());
    items.insert("local-11434".to_string());
    for source in [
        resolve_model(None).ok(),
        env_value("OY_MODEL"),
        config::load_model_config().ok().and_then(|c| c.shim),
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

async fn fetch_openai_compatible_models(
    endpoint: &OpenAiCompatibleEndpoint,
) -> Result<Vec<String>> {
    let url = format!("{}/models", normalize_base_url(&endpoint.base_url));
    let request = Request::get(&url)
        .header("Authorization", format!("Bearer {}", endpoint.api_key))
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(15))
        .body(())?;
    let mut response = isahc::send_async(request).await?;
    if !response.status().is_success() {
        bail!("GET {url} failed with HTTP {}", response.status());
    }
    let value = response.json::<Value>().await?;
    let models = extract_model_ids(&value)
        .into_iter()
        .map(|id| match endpoint.model_prefix.as_deref() {
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
    let github = env_value("GITHUB_TOKEN");
    let gh = env_value("GH_TOKEN");
    let auto = env_value("OY_AUTO_GITHUB_TOKEN");
    let present = github.is_some() || gh.is_some();
    let detail = match (github.as_deref(), gh.as_deref(), auto.as_deref()) {
        (Some(_), _, Some(_)) => "GITHUB_TOKEN auto-populated from `gh auth token`.".to_string(),
        (Some(_), Some(_), None) => {
            "GITHUB_TOKEN and GH_TOKEN detected; github_copilot-compatible auth available."
                .to_string()
        }
        (Some(_), None, None) => {
            "GITHUB_TOKEN detected; github_copilot-compatible auth available.".to_string()
        }
        (None, Some(_), _) => "GH_TOKEN detected; GitHub auth available to tooling.".to_string(),
        (None, None, _) => "No GitHub auth token detected.".to_string(),
    };
    AuthStatus {
        adapter: "github".to_string(),
        env_var: Some("GITHUB_TOKEN, GH_TOKEN".to_string()),
        present,
        source: if auto.is_some() {
            "gh"
        } else if github.is_some() {
            "env"
        } else if gh.is_some() {
            "env-alias"
        } else {
            "missing"
        }
        .to_string(),
        detail,
        auto_configured: auto.is_some(),
    }
}

fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn github_token() -> Option<String> {
    env_value("GITHUB_TOKEN").or_else(|| env_value("GH_TOKEN"))
}

pub fn auto_configure_auth() -> Result<()> {
    ensure_github_token_from_gh()?;
    Ok(())
}

fn ensure_github_token_from_gh() -> Result<()> {
    if env_value("GITHUB_TOKEN").is_some() {
        return Ok(());
    }
    let output = std::process::Command::new("gh")
        .arg("auth")
        .arg("token")
        .output();
    let Ok(output) = output else {
        return Ok(());
    };
    if !output.status.success() {
        return Ok(());
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Ok(());
    }
    unsafe {
        env::set_var("GITHUB_TOKEN", token);
        env::set_var("OY_AUTO_GITHUB_TOKEN", "1");
    }
    Ok(())
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
    auto_configure_auth()?;
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

fn service_target_resolver() -> Result<Option<ServiceTargetResolver>> {
    let base_url = env::var("OPENAI_BASE_URL").ok();
    let resolver = ServiceTargetResolver::from_resolver_fn(move |target: ServiceTarget| {
        let model_name = target.model.model_name.to_string();
        if let Some(mapped) =
            openai_compatible_target(&model_name).map_err(|err| err.to_string())?
        {
            return Ok(mapped);
        }
        if let Some(url) = base_url.as_ref() {
            return Ok(ServiceTarget {
                endpoint: Endpoint::from_owned(url.trim_end_matches('/').to_string() + "/"),
                auth: target.auth,
                model: ModelIden::new(AdapterKind::OpenAI, model_name),
            });
        }
        Ok(target)
    });
    Ok(Some(resolver))
}

fn openai_compatible_target(model: &str) -> Result<Option<ServiceTarget>> {
    let (shim, model_name) = config::split_model_spec(model);
    let Some(shim) = shim else {
        return Ok(None);
    };
    if shim == "github_copilot" || shim == "copilot" || shim == "github-copilot" {
        let Some(token) = github_token() else {
            return Ok(None);
        };
        let base = env_value("COPILOT_BASE_URL")
            .unwrap_or_else(|| "https://api.githubcopilot.com".to_string());
        return Ok(Some(ServiceTarget {
            endpoint: Endpoint::from_owned(normalize_base_url(&base) + "/"),
            auth: AuthData::from_single(token),
            model: ModelIden::new(AdapterKind::OpenAI, model_name.to_string()),
        }));
    }
    match shim {
        "local-8080" => 8080,
        "local-11434" => 11434,
        other if other.starts_with("local-") => other
            .trim_start_matches("local-")
            .parse::<u16>()
            .context("invalid local model shim port")?,
        _ => return Ok(None),
    };
    let base = local_base_url(shim);
    let key = local_api_key();
    Ok(Some(ServiceTarget {
        endpoint: Endpoint::from_owned(normalize_base_url(&base) + "/"),
        auth: AuthData::from_single(key),
        model: ModelIden::new(AdapterKind::OpenAI, model_name.to_string()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genai_model_spec_maps_copilot() {
        assert_eq!(
            to_genai_model_spec("copilot:openai/gpt-4.1-mini"),
            "github_copilot::openai/gpt-4.1-mini"
        );
    }

    #[test]
    fn genai_model_spec_maps_local() {
        assert_eq!(
            to_genai_model_spec("local-8080:qwen3.5"),
            "local-8080::qwen3.5"
        );
    }

    #[test]
    fn genai_model_spec_leaves_plain_models() {
        assert_eq!(to_genai_model_spec("gpt-5.4-mini"), "gpt-5.4-mini");
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
