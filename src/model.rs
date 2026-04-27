use crate::config;
use anyhow::{Result, anyhow, bail};
use genai::adapter::AdapterKind;
use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
use std::fs;
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
    pub recommended: Vec<String>,
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
const SHIM_COPILOT: &str = "copilot";
const SHIM_BEDROCK_MANTLE: &str = "bedrock-mantle";
const SHIM_OPENCODE: &str = "opencode";
const SHIM_OPENCODE_GO: &str = "opencode-go";
const SHIM_ORDER: &[&str] = &[
    "local-8080",
    "local-11434",
    SHIM_OPENAI,
    SHIM_COPILOT,
    SHIM_BEDROCK_MANTLE,
    SHIM_OPENCODE,
    SHIM_OPENCODE_GO,
];

pub fn resolve_model(configured: Option<&str>) -> Result<String> {
    if let Some(value) = configured.filter(|v| !v.trim().is_empty()) {
        return Ok(canonical_model_spec(value));
    }
    if let Ok(value) = env::var("OY_MODEL")
        && !value.trim().is_empty()
    {
        return Ok(canonical_model_spec(&value));
    }
    if let Some(model) = config::load_model_config()?.model {
        return Ok(canonical_model_spec(&model));
    }
    bail!(no_model_message())
}

fn no_model_message() -> String {
    let mut lines = vec!["No model configured.".to_string()];
    if let Some(choice) = recommended_models().first() {
        lines.push(format!("Detected provider auth. Try: oy model {choice}"));
    } else {
        lines.push("No provider auth detected. Run `oy doctor` for setup help.".to_string());
    }
    lines.push("Then run: oy \"inspect this repo\"".to_string());
    lines.push("Advanced: use `oy model` to list options or set OY_MODEL for one run.".to_string());
    lines.join("\n")
}

pub fn resolve_shim() -> Result<Option<String>> {
    if let Ok(value) = env::var("OY_SHIM")
        && !value.trim().is_empty()
    {
        return Ok(Some(value));
    }
    Ok(config::load_model_config()?.shim)
}

pub fn recommended_models() -> Vec<String> {
    let mut out = Vec::new();
    let auth = auth_statuses();
    if auth.iter().any(|item| item.adapter == SHIM_OPENAI) {
        out.push("gpt-4.1-mini".to_string());
    }
    if auth.iter().any(|item| item.adapter == "github") {
        out.push("copilot::gpt-4.1-mini".to_string());
    }
    if auth.iter().any(|item| item.adapter == "bedrock") {
        out.push("bedrock::global.amazon.nova-2-lite-v1:0".to_string());
    }
    if auth.iter().any(|item| item.adapter == SHIM_BEDROCK_MANTLE) {
        out.push("bedrock-mantle::moonshotai.kimi-k2.5".to_string());
    }
    if auth.iter().any(|item| item.adapter == SHIM_OPENCODE) {
        out.push("opencode::gpt-5.1-codex-max".to_string());
    }
    if auth.iter().any(|item| item.adapter == SHIM_OPENCODE_GO) {
        out.push("opencode-go::kimi-k2.5".to_string());
    }
    if auth
        .iter()
        .any(|item| item.adapter == "local-openai-compatible")
    {
        out.push("local-8080::qwen3.5".to_string());
    }
    out.sort();
    out.dedup();
    out
}

pub fn list_builtin_model_hints() -> Vec<String> {
    vec![
        "openai_resp::gpt-5.5".to_string(),
        "gpt-5.4-mini".to_string(),
        "gpt-4.1-mini".to_string(),
        "copilot::gpt-4.1-mini".to_string(),
        "bedrock::global.amazon.nova-2-lite-v1:0".to_string(),
        "bedrock::au.anthropic.claude-sonnet-4-5-20250929-v1:0".to_string(),
        "bedrock::au.anthropic.claude-haiku-4-5-20251001-v1:0".to_string(),
        "bedrock::global.anthropic.claude-sonnet-4-5-20250929-v1:0".to_string(),
        "bedrock::openai.gpt-oss-120b-1:0".to_string(),
        "bedrock-mantle::moonshotai.kimi-k2.5".to_string(),
        "bedrock-mantle::moonshot.kimi-k2-thinking".to_string(),
        "bedrock-mantle::openai.gpt-oss-120b".to_string(),
        "opencode::gpt-5.1-codex-max".to_string(),
        "opencode::kimi-k2.5".to_string(),
        "opencode::gpt-5-nano".to_string(),
        "opencode-go::kimi-k2.5".to_string(),
        "local-8080::qwen3.5".to_string(),
        "local-11434::qwen3.5".to_string(),
    ]
}

pub async fn inspect_models() -> Result<ModelListing> {
    let current = resolve_model(None).ok();
    let current_shim = resolve_shim().ok().flatten();
    let auth = auth_statuses()
        .into_iter()
        .filter(|item| item.present || item.auto_configured)
        .collect::<Vec<_>>();
    let recommended = recommended_models();
    let dynamic = inspect_openai_compatible_models().await;
    let hints = list_builtin_model_hints();
    let all_models = collect_all_models(&dynamic, &hints);
    Ok(ModelListing {
        current,
        current_shim,
        auth,
        recommended,
        dynamic,
        hints,
        all_models,
    })
}

fn collect_all_models(dynamic: &[AdapterModels], hints: &[String]) -> Vec<String> {
    let mut items = dynamic
        .iter()
        .filter(|group| group.ok)
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

pub fn default_reasoning_effort(model_spec: &str) -> Option<&'static str> {
    let (_, model) = config::split_model_spec(model_spec);
    let (inline_effort, _) = split_reasoning_effort_suffix(model);
    inline_effort.or_else(|| reasoning_effort_option(model_spec))
}

pub fn reasoning_effort_option(model_spec: &str) -> Option<&'static str> {
    if env::var("OY_THINKING").is_ok() || env::var("OY_REASONING_EFFORT").is_ok() {
        return configured_reasoning_effort();
    }
    let (_, model) = config::split_model_spec(model_spec);
    let (inline_effort, base_model) = split_reasoning_effort_suffix(model);
    if inline_effort.is_some() {
        return None;
    }
    reasoning_capable_model(base_model).then_some("high")
}

fn configured_reasoning_effort() -> Option<&'static str> {
    env_value("OY_THINKING")
        .or_else(|| env_value("OY_REASONING_EFFORT"))
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => None,
            "off" | "false" | "0" | "none" => Some("none"),
            "minimal" => Some("minimal"),
            "low" => Some("low"),
            "medium" => Some("medium"),
            "high" | "true" | "1" | "on" => Some("high"),
            _ => None,
        })
}

fn split_reasoning_effort_suffix(model: &str) -> (Option<&'static str>, &str) {
    if let Some((base, suffix)) = model.rsplit_once('-') {
        let effort = match suffix.to_ascii_lowercase().as_str() {
            "none" => Some("none"),
            "minimal" => Some("minimal"),
            "low" => Some("low"),
            "medium" => Some("medium"),
            "high" => Some("high"),
            _ => None,
        };
        if let Some(effort) = effort {
            return (Some(effort), base);
        }
    }
    (None, model)
}

fn reasoning_capable_model(model: &str) -> bool {
    let model = model
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(model)
        .to_ascii_lowercase();
    model.starts_with("gpt-5")
        || model.contains("codex")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("claude-3-7")
        || model.starts_with("claude-4")
        || model.starts_with("claude-sonnet-4")
        || model.starts_with("claude-opus-4")
        || model.starts_with("gemini-3")
}

pub fn auth_statuses() -> Vec<AuthStatus> {
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
        match fetch_openai_compatible_models(&endpoint).await {
            Ok(models) if !models.is_empty() => out.push(AdapterModels {
                adapter: endpoint.adapter,
                ok: true,
                source: endpoint.source,
                count: models.len(),
                models,
                error: None,
            }),
            Ok(_) => {}
            Err(err) => out.push(AdapterModels {
                adapter: endpoint.adapter,
                ok: false,
                source: endpoint.source,
                count: 0,
                models: Vec::new(),
                error: Some(err.to_string()),
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
        SHIM_BEDROCK_MANTLE => bearer_endpoint_config(
            SHIM_BEDROCK_MANTLE,
            || {
                env_value("BEDROCK_MANTLE_BASE_URL")
                    .or_else(|| env_value("OPENAI_BASE_URL"))
                    .unwrap_or_else(|| {
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
                ("OPENAI_API_KEY", env_value("OPENAI_API_KEY")),
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
                source: "local OpenAI-compatible endpoint".to_string(),
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
    let provider = if shim == SHIM_OPENCODE_GO {
        SHIM_OPENCODE_GO
    } else {
        SHIM_OPENCODE
    };
    opencode_auth_key_from_path(provider, opencode_auth_path())
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
    let provider_value = value.get(provider).or_else(|| {
        provider
            .strip_suffix('/')
            .and_then(|trimmed| value.get(trimmed))
    })?;
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

fn extra_local_shims() -> Vec<String> {
    let mut items = BTreeSet::new();
    for value in [
        resolve_model(None).ok(),
        env_value("OY_MODEL"),
        resolve_shim().ok().flatten(),
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
    env_value("LOCAL_API_KEY")
        .or_else(|| env_value("OPENAI_API_KEY"))
        .unwrap_or_else(|| "oy-local".to_string())
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

fn bedrock_status() -> AuthStatus {
    let status = crate::bedrock::auth_status();
    AuthStatus {
        adapter: "bedrock".to_string(),
        env_var: Some("AWS_ACCESS_KEY_ID, AWS_PROFILE".to_string()),
        present: status.present,
        source: status.source,
        detail: status.detail,
        auto_configured: status.auto_configured,
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
    let Some(api_key) = env_value("OPENAI_API_KEY") else {
        return Ok(None);
    };
    let resolver = AuthResolver::from_resolver_fn(move |model: ModelIden| {
        let model_name = model.model_name.to_string();
        let (inline_shim, _) = config::split_model_spec(&model_name);
        if inline_shim.is_some_and(config::is_routing_shim) || env_value("OY_SHIM").is_some() {
            return Ok(None);
        }
        Ok(Some(AuthData::from_single(api_key.clone())))
    });
    Ok(Some(resolver))
}

fn openai_adapter_for_model(model: &str) -> AdapterKind {
    if config::is_openai_responses_model(model) {
        AdapterKind::OpenAIResp
    } else {
        AdapterKind::OpenAI
    }
}

fn service_target_resolver() -> Result<Option<ServiceTargetResolver>> {
    let base_url = env_value("OPENAI_BASE_URL");
    let configured_shim = resolve_shim()?;
    let resolver = ServiceTargetResolver::from_resolver_fn(move |target: ServiceTarget| {
        let model_name = target.model.model_name.to_string();
        if let Some(mapped) = openai_compatible_target(&target.model, configured_shim.as_deref())
            .map_err(|err| err.to_string())?
        {
            return Ok(mapped);
        }
        if let Some(url) = base_url.as_ref().filter(|_| configured_shim.is_none()) {
            let (namespace, _) = config::split_model_spec(&model_name);
            if namespace.is_none_or(|shim| !config::is_routing_shim(shim)) {
                return Ok(ServiceTarget {
                    endpoint: Endpoint::from_owned(normalize_base_url(url) + "/"),
                    auth: target.auth,
                    model: ModelIden::new(openai_adapter_for_model(&model_name), model_name),
                });
            }
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
    let (namespace, inline_model) = config::split_model_spec(&model_name);
    let inline_shim = namespace.filter(|shim| config::is_routing_shim(shim));
    let shim = inline_shim.or(configured_shim);
    let Some(shim) = shim else {
        return Ok(None);
    };
    if !config::is_routing_shim(shim) {
        bail!("invalid routing shim: {shim}");
    }
    let target_model = if inline_shim.is_some() {
        ModelIden::new(
            openai_adapter_for_model(inline_model),
            inline_model.to_string(),
        )
    } else {
        model.clone()
    };
    let config = shim_endpoint_config(shim)
        .ok_or_else(|| anyhow!("routing shim {shim} is not configured or lacks credentials"))?;
    Ok(Some(ServiceTarget {
        endpoint: Endpoint::from_owned(normalize_base_url(&config.base_url) + "/"),
        auth: AuthData::from_single(config.api_key),
        model: target_model,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn local_shim_endpoint_config_matches_python_defaults() {
        let config = shim_endpoint_config("local-8088").unwrap();
        assert_eq!(config.shim, "local-8088");
        assert_eq!(config.base_url, "http://127.0.0.1:8088/v1");
        assert_eq!(config.api_key, "oy-local");
        assert!(shim_endpoint_config("local-nope").is_none());
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
    fn reasoning_defaults_to_high_for_capable_models_and_allows_suffix_override() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        unsafe { std::env::remove_var("OY_THINKING") };
        unsafe { std::env::remove_var("OY_REASONING_EFFORT") };
        assert_eq!(default_reasoning_effort("gpt-5.5"), Some("high"));
        assert_eq!(
            default_reasoning_effort("copilot::gpt-5.5-low"),
            Some("low")
        );
        assert_eq!(default_reasoning_effort("gpt-4.1-mini"), None);
    }

    #[test]
    fn reasoning_env_override_can_disable_or_adjust() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        unsafe { std::env::set_var("OY_THINKING", "off") };
        assert_eq!(default_reasoning_effort("gpt-5.5"), Some("none"));
        unsafe { std::env::set_var("OY_THINKING", "medium") };
        assert_eq!(default_reasoning_effort("gpt-5.5"), Some("medium"));
        unsafe { std::env::remove_var("OY_THINKING") };
        unsafe { std::env::remove_var("OY_REASONING_EFFORT") };
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
    fn inline_routing_shim_overrides_configured_shim() {
        let target = ModelIden::new(AdapterKind::OpenAI, "local-8088::qwen3.5".to_string());
        let mapped = openai_compatible_target(&target, Some("openai"))
            .unwrap()
            .unwrap();
        assert_eq!(mapped.model.model_name, "qwen3.5");
        assert_eq!(mapped.endpoint.base_url(), "http://127.0.0.1:8088/v1/");
    }

    #[test]
    fn native_adapter_namespace_is_not_treated_as_routing_shim() {
        let target = ModelIden::new(AdapterKind::OpenAIResp, "openai_resp::gpt-5.5".to_string());
        assert!(openai_compatible_target(&target, None).unwrap().is_none());

        let mapped = openai_compatible_target(&target, Some("local-8088"))
            .unwrap()
            .unwrap();
        assert_eq!(mapped.model.model_name, "openai_resp::gpt-5.5");
        assert_eq!(mapped.model.adapter_kind, AdapterKind::OpenAIResp);
        assert_eq!(mapped.endpoint.base_url(), "http://127.0.0.1:8088/v1/");
    }

    #[test]
    fn configured_shim_still_routes_plain_model() {
        let target = ModelIden::new(AdapterKind::OpenAI, "qwen3.5".to_string());
        let mapped = openai_compatible_target(&target, Some("local-8088"))
            .unwrap()
            .unwrap();
        assert_eq!(mapped.model.model_name, "qwen3.5");
        assert_eq!(mapped.endpoint.base_url(), "http://127.0.0.1:8088/v1/");
    }

    #[test]
    fn bedrock_mantle_uses_bedrock_bearer_token_before_openai_key() {
        unsafe { std::env::remove_var("BEDROCK_MANTLE_API_KEY") };
        unsafe { std::env::remove_var("BEDROCK_MANTLE_BASE_URL") };
        unsafe { std::env::set_var("AWS_BEARER_TOKEN_BEDROCK", "bedrock-token") };
        unsafe { std::env::set_var("OPENAI_API_KEY", "openai-token") };
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
        unsafe { std::env::remove_var("OPENCODE_API_KEY") };
        unsafe { std::env::remove_var("OPENCODE_BASE_URL") };
    }

    #[test]
    fn recommended_models_follow_detected_auth() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        unsafe { std::env::set_var("OPENAI_API_KEY", "openai-token") };
        let recommendations = recommended_models();
        assert!(recommendations.contains(&"gpt-4.1-mini".to_string()));
        assert!(recommendations.contains(&"local-8080::qwen3.5".to_string()));
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
    }

    #[test]
    fn builtin_hints_include_bedrock_variants() {
        let hints = list_builtin_model_hints();
        assert!(hints.iter().any(|item| item.starts_with("bedrock::")));
        assert!(
            hints
                .iter()
                .any(|item| item.starts_with("bedrock-mantle::"))
        );
        assert!(hints.iter().any(|item| item.starts_with("opencode::")));
        assert!(hints.iter().any(|item| item.starts_with("opencode-go::")));
    }
}
