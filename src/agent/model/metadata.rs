//! Best-effort model metadata cache.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::agent::opencode_models;

/// Information about the provider for a model spec.
#[derive(Debug, Clone)]
pub(crate) struct ProviderInfo {
    /// The provider id (e.g. "openai", "github-copilot")
    pub provider: String,
    /// The API endpoint URL, if determinable from OpenCode metadata
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CachedModelInfo {
    #[cfg(test)]
    pub model_spec: String,
    pub limits: Option<opencode_models::OpenCodeModelLimit>,
    pub provider_info: ProviderInfo,
}

static MODEL_INFO_CACHE: LazyLock<std::sync::RwLock<HashMap<String, CachedModelInfo>>> =
    LazyLock::new(|| std::sync::RwLock::new(HashMap::new()));

pub async fn cache_model_limits(model_spec: &str) -> Result<()> {
    let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(model_spec);
    let provider = parsed.provider_or_openai();
    let limits = opencode_models::lookup_limit(provider, parsed.base_model);

    let endpoint = crate::llm::providers::provider_metadata(provider)
        .and_then(|metadata| metadata.default_base_url.map(str::to_string));

    let key = canonical_cache_key(model_spec);
    let mut cache = MODEL_INFO_CACHE
        .write()
        .unwrap_or_else(|err| err.into_inner());
    cache.insert(
        key,
        CachedModelInfo {
            #[cfg(test)]
            model_spec: canonical_cache_key(model_spec),
            limits,
            provider_info: ProviderInfo {
                provider: provider.to_string(),
                endpoint,
            },
        },
    );

    Ok(())
}

/// Extract provider info from a model spec string.
pub(crate) fn provider_info(model_spec: &str) -> ProviderInfo {
    let lock = MODEL_INFO_CACHE
        .read()
        .unwrap_or_else(|err| err.into_inner());
    if let Some(info) = lock.get(&canonical_cache_key(model_spec)) {
        info.provider_info.clone()
    } else {
        let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(model_spec);
        ProviderInfo {
            provider: parsed.provider_or_openai().to_string(),
            endpoint: None,
        }
    }
}

/// Look up token limits for a model spec from OpenCode metadata.
/// Returns `None` when OpenCode isn't available or the model isn't found.
pub(crate) fn model_limits(model_spec: &str) -> Option<opencode_models::OpenCodeModelLimit> {
    MODEL_INFO_CACHE
        .read()
        .unwrap_or_else(|err| err.into_inner())
        .get(&canonical_cache_key(model_spec))
        .and_then(|info| info.limits)
}

pub(crate) fn canonical_cache_key(model_spec: &str) -> String {
    let parsed = crate::llm::route::resolve::ParsedModelSpec::parse(model_spec);
    format!("{}/{}", parsed.provider_or_openai(), parsed.base_model)
}

#[cfg(test)]
pub(crate) fn replace_cached_model_info(
    next: Option<CachedModelInfo>,
) -> HashMap<String, CachedModelInfo> {
    let mut cache = MODEL_INFO_CACHE
        .write()
        .unwrap_or_else(|err| err.into_inner());
    let mut next_cache = HashMap::new();
    if let Some(info) = next {
        next_cache.insert(info.model_spec.clone(), info);
    }
    std::mem::replace(&mut *cache, next_cache)
}

#[cfg(test)]
pub(crate) fn restore_cached_model_info(next: HashMap<String, CachedModelInfo>) {
    let mut cache = MODEL_INFO_CACHE
        .write()
        .unwrap_or_else(|err| err.into_inner());
    *cache = next;
}
