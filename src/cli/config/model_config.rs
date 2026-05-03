use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

use super::paths::{config_root, create_private_dir_all, write_private_file};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedModelConfig {
    pub model: Option<String>,
    pub shim: Option<String>,
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
    payload.recent_models = updated_recent_models(&previous.recent_models, model_spec);
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
    if config.model.is_none() && config.shim.is_none() && config.recent_models.is_empty() {
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
    let canonical = crate::model::canonical_model_spec(selected);
    let mut recent = Vec::with_capacity(RECENT_MODEL_LIMIT);
    recent.push(canonical.clone());
    recent.extend(
        previous
            .iter()
            .map(|item| crate::model::canonical_model_spec(item))
            .filter(|item| !item.is_empty() && item != &canonical),
    );
    recent.truncate(RECENT_MODEL_LIMIT);
    recent
}

pub fn saved_model_config_from_selection(model_spec: &str) -> SavedModelConfig {
    let model_spec = model_spec.trim();
    let (prefix, model) = split_model_spec(model_spec);
    if let Some(shim) = prefix.filter(|shim| is_routing_shim(shim)) {
        return SavedModelConfig {
            model: Some(genai_model_for_shim(shim, model)),
            shim: Some(shim.to_string()),
            recent_models: Vec::new(),
        };
    }
    SavedModelConfig {
        model: Some(model_spec.to_string()),
        shim: None,
        recent_models: Vec::new(),
    }
}

fn genai_model_for_shim(shim: &str, model: &str) -> String {
    if is_copilot_shim(shim) && is_openai_responses_model(model) {
        format!("openai_resp::{model}")
    } else {
        model.to_string()
    }
}

pub fn is_openai_responses_model(model: &str) -> bool {
    let (_, model) = split_model_spec(model);
    let model = model
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(model);
    model.starts_with("gpt-5.5")
        || (model.starts_with("gpt") && (model.contains("codex") || model.contains("pro")))
}

pub fn is_routing_shim(shim: &str) -> bool {
    matches!(
        shim,
        "openai" | "copilot" | "bedrock-mantle" | "opencode" | "opencode-go"
    ) || shim
        .strip_prefix("local-")
        .is_some_and(|port| port.parse::<u16>().is_ok())
}

fn is_copilot_shim(shim: &str) -> bool {
    shim == "copilot"
}

pub fn split_model_spec(spec: &str) -> (Option<&str>, &str) {
    if let Some(index) = spec.find("::") {
        let (left, right) = spec.split_at(index);
        return (Some(left), &right[2..]);
    }
    (None, spec)
}
