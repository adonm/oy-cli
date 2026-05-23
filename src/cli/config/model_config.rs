//! Saved model config: [`SavedModelConfig`] serialisation,
//! model-spec splitting/canonicalisation, and recent-model tracking.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

use super::paths::{config_root, create_private_dir_all, write_private_file};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedModelConfig {
    pub model: Option<String>,
    #[serde(default)]
    pub recent_models: Vec<String>,
}

const RECENT_MODEL_LIMIT: usize = 5;

pub fn load_model_config() -> Result<SavedModelConfig> {
    let path = config_root();
    if !path.exists() {
        return Ok(SavedModelConfig::default());
    }
    let data =
        fs::read_to_string(&path).with_context(|| format!("failed reading {}", path.display()))?;
    let parsed = serde_json::from_str::<SavedModelConfig>(&data)
        .with_context(|| format!("failed parsing {}", path.display()))?;
    Ok(parsed)
}

pub fn save_model_config(model_spec: &str) -> Result<()> {
    let path = config_root();
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    let previous = load_model_config()?;
    let mut payload = saved_model_config_from_selection(model_spec);
    let selected = payload
        .model
        .as_deref()
        .unwrap_or_else(|| model_spec.trim());
    payload.recent_models = updated_recent_models(&previous.recent_models, selected);
    let text = serde_json::to_string_pretty(&payload)?;
    write_private_file(&path, text.as_bytes())?;
    Ok(())
}

pub fn recent_models() -> Result<Vec<String>> {
    Ok(load_model_config()?.recent_models)
}

pub fn clear_recent_models() -> Result<()> {
    let path = config_root();
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    let mut config = load_model_config()?;
    if config.model.is_none() && config.recent_models.is_empty() {
        if path.exists() {
            let text = serde_json::to_string_pretty(&config)?;
            write_private_file(&path, text.as_bytes())?;
        }
        return Ok(());
    }
    config.recent_models.clear();
    let text = serde_json::to_string_pretty(&config)?;
    write_private_file(&path, text.as_bytes())?;
    Ok(())
}

pub(super) fn updated_recent_models(previous: &[String], selected: &str) -> Vec<String> {
    let selected = selected.trim();
    if selected.is_empty() {
        return previous.iter().take(RECENT_MODEL_LIMIT).cloned().collect();
    }
    let canonical = selected.trim().to_string();
    let mut recent = Vec::with_capacity(RECENT_MODEL_LIMIT);
    recent.push(canonical.clone());
    recent.extend(
        previous
            .iter()
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty() && item != &canonical),
    );
    recent.truncate(RECENT_MODEL_LIMIT);
    recent
}

pub fn saved_model_config_from_selection(model_spec: &str) -> SavedModelConfig {
    SavedModelConfig {
        model: Some(canonical_model_spec(model_spec)),
        recent_models: Vec::new(),
    }
}

pub fn canonical_model_spec(model_spec: &str) -> String {
    let model_spec = model_spec.trim();
    let (prefix, model) = split_model_spec(model_spec);
    match prefix {
        Some(provider) => format!("{}/{model}", canonical_provider(provider)),
        None => model_spec.to_string(),
    }
}

pub fn canonical_provider(provider: &str) -> &str {
    crate::llm::providers::canonical_provider_id(provider.trim())
}

pub fn split_model_spec(spec: &str) -> (Option<&str>, &str) {
    if let Some(index) = spec.find("::") {
        let (left, right) = spec.split_at(index);
        return (Some(left), &right[2..]);
    }
    (None, spec)
}
