use crate::config;
use anyhow::{Result, anyhow, bail};
use genai::adapter::AdapterKind;
use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use serde::Serialize;
use std::env;

pub(crate) use super::auth::{AuthStatus, auth_statuses};
pub(crate) use super::endpoints::AdapterModels;
use super::endpoints::{
    env_value, inspect_openai_compatible_models, normalize_base_url, shim_endpoint_config,
};

#[derive(Debug, Clone, Serialize)]
pub struct ModelListing {
    pub current: Option<String>,
    pub current_shim: Option<String>,
    pub auth: Vec<AuthStatus>,
    pub dynamic: Vec<AdapterModels>,
    pub all_models: Vec<String>,
}

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
    lines.push("Run `oy model` to inspect auth-backed model endpoints.".to_string());
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

pub async fn inspect_models() -> Result<ModelListing> {
    let current = resolve_model(None).ok();
    let current_shim = resolve_shim().ok().flatten();
    let auth = auth_statuses()
        .into_iter()
        .filter(|item| item.availability.is_available())
        .collect::<Vec<_>>();
    let dynamic = inspect_openai_compatible_models().await;
    let all_models = collect_all_models(&dynamic);
    Ok(ModelListing {
        current,
        current_shim,
        auth,
        dynamic,
        all_models,
    })
}

fn collect_all_models(dynamic: &[AdapterModels]) -> Vec<String> {
    let mut items = dynamic
        .iter()
        .flat_map(|group| group.models().iter().cloned())
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
        if openai_env_applies_to_model(&model) {
            Ok(Some(AuthData::from_single(api_key.clone())))
        } else {
            Ok(None)
        }
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

fn is_openai_adapter(kind: AdapterKind) -> bool {
    matches!(kind, AdapterKind::OpenAI | AdapterKind::OpenAIResp)
}

fn openai_env_applies_to_model(model: &ModelIden) -> bool {
    is_openai_adapter(model.adapter_kind) && openai_env_applies_to_model_name(&model.model_name)
}

fn openai_env_applies_to_model_name(model_name: &str) -> bool {
    let (namespace, _) = config::split_model_spec(model_name);
    matches!(namespace, None | Some("openai_resp")) && env_value("OY_SHIM").is_none()
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
        if let Some(url) = base_url.as_ref().filter(|_| configured_shim.is_none())
            && openai_env_applies_to_model(&target.model)
        {
            return Ok(ServiceTarget {
                endpoint: Endpoint::from_owned(normalize_base_url(url) + "/"),
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
pub(crate) static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
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
    fn model_listing_only_includes_introspected_models() {
        let models = collect_all_models(&[]);
        assert!(models.is_empty());
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
    fn inline_routing_shim_overrides_configured_shim() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        unsafe { std::env::set_var("LOCAL_API_KEY", "local-token") };
        let target = ModelIden::new(AdapterKind::OpenAI, "local-8088::qwen3.5".to_string());
        let mapped = openai_compatible_target(&target, Some("openai"))
            .unwrap()
            .unwrap();
        assert_eq!(mapped.model.model_name, "qwen3.5");
        assert_eq!(mapped.endpoint.base_url(), "http://127.0.0.1:8088/v1/");
        unsafe { std::env::remove_var("LOCAL_API_KEY") };
    }

    #[test]
    fn native_adapter_namespace_is_not_treated_as_routing_shim() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        unsafe { std::env::set_var("LOCAL_API_KEY", "local-token") };
        let target = ModelIden::new(AdapterKind::OpenAIResp, "openai_resp::gpt-5.5".to_string());
        assert!(openai_compatible_target(&target, None).unwrap().is_none());

        let mapped = openai_compatible_target(&target, Some("local-8088"))
            .unwrap()
            .unwrap();
        assert_eq!(mapped.model.model_name, "openai_resp::gpt-5.5");
        assert_eq!(mapped.model.adapter_kind, AdapterKind::OpenAIResp);
        assert_eq!(mapped.endpoint.base_url(), "http://127.0.0.1:8088/v1/");
        unsafe { std::env::remove_var("LOCAL_API_KEY") };
    }

    #[test]
    fn configured_shim_still_routes_plain_model() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        unsafe { std::env::set_var("LOCAL_API_KEY", "local-token") };
        let target = ModelIden::new(AdapterKind::OpenAI, "qwen3.5".to_string());
        let mapped = openai_compatible_target(&target, Some("local-8088"))
            .unwrap()
            .unwrap();
        assert_eq!(mapped.model.model_name, "qwen3.5");
        assert_eq!(mapped.endpoint.base_url(), "http://127.0.0.1:8088/v1/");
        unsafe { std::env::remove_var("LOCAL_API_KEY") };
    }

    #[test]
    fn openai_env_only_applies_to_openai_models_without_routing() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        unsafe { std::env::remove_var("OY_SHIM") };
        assert!(openai_env_applies_to_model(&ModelIden::new(
            AdapterKind::OpenAI,
            "gpt-4.1-mini"
        )));
        assert!(openai_env_applies_to_model(&ModelIden::new(
            AdapterKind::OpenAIResp,
            "gpt-5.5"
        )));
        assert!(openai_env_applies_to_model(&ModelIden::new(
            AdapterKind::OpenAIResp,
            "openai_resp::gpt-5.5"
        )));
        assert!(!openai_env_applies_to_model(&ModelIden::new(
            AdapterKind::Gemini,
            "gemini-2.5-flash"
        )));
        assert!(!openai_env_applies_to_model(&ModelIden::new(
            AdapterKind::Anthropic,
            "claude-sonnet-4"
        )));
        assert!(!openai_env_applies_to_model(&ModelIden::new(
            AdapterKind::OpenAI,
            "openai::gpt-4.1-mini"
        )));
        unsafe { std::env::set_var("OY_SHIM", "openai") };
        assert!(!openai_env_applies_to_model(&ModelIden::new(
            AdapterKind::OpenAI,
            "gpt-4.1-mini"
        )));
        unsafe { std::env::remove_var("OY_SHIM") };
    }
}
