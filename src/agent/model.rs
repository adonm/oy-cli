//! Model selection facade.
//!
//! Model routing, metadata caching, execution, and reasoning-effort policy live
//! in focused submodules so this facade only exposes the stable agent API.

use crate::config;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::env;

mod exec;
mod metadata;
mod reasoning;

pub(crate) use exec::exec_chat;
pub(crate) use metadata::{cache_model_limits, model_limits, provider_info};
pub(crate) use reasoning::{
    default_reasoning_effort, get_thinking_effort, reasoning_efforts_for, set_thinking_override,
};

#[cfg(test)]
pub(crate) use exec::prepare_chat;
#[cfg(test)]
pub(crate) use metadata::{
    CachedModelInfo, ProviderInfo, canonical_cache_key, replace_cached_model_info,
    restore_cached_model_info,
};
#[cfg(test)]
pub(crate) use reasoning::reasoning_capable_fallback;

pub(crate) use super::auth::AuthStatus;
use super::opencode_models;
pub(crate) use super::opencode_models::AdapterModels;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListing {
    pub current: Option<String>,
    pub auth: Vec<AuthStatus>,
    pub dynamic: Vec<AdapterModels>,
    pub all_models: Vec<String>,
}

pub fn resolve_model(configured: Option<&str>) -> Result<String> {
    if let Some(value) = configured.filter(|v| !v.trim().is_empty()) {
        return Ok(config::canonical_model_spec(value));
    }
    if let Ok(value) = env::var("OY_MODEL")
        && !value.trim().is_empty()
    {
        return Ok(config::canonical_model_spec(&value));
    }
    let saved = config::load_model_config()?;
    if let Some(model) = saved.model.filter(|model| !model.trim().is_empty()) {
        return Ok(config::canonical_model_spec(&model));
    }
    bail!(no_model_message())
}

fn no_model_message() -> String {
    [
        "No model configured.",
        "Run `oy model` to inspect OpenCode verbose model metadata.",
        "Then run: oy \"inspect this repo\"",
        "Advanced: use `oy model` to list options or set OY_MODEL for one run.",
    ]
    .join("\n")
}

pub async fn inspect_models() -> Result<ModelListing> {
    let current = resolve_model(None).ok();
    let dynamic = opencode_models::inspect();
    let all_models = dynamic
        .iter()
        .flat_map(|group| group.models().iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(ModelListing {
        current,
        auth: super::auth::auth_statuses(),
        dynamic,
        all_models,
    })
}

#[cfg(test)]
#[path = "model/tests.rs"]
mod tests;
