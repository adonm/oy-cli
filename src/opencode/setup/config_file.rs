//! OpenCode JSON/JSONC parsing and oy-owned config transformations.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::Path;

const OPENCODE_PLUGIN_PACKAGE: &str = "@oy-cli/opencode";

pub(super) fn remove_owned_config(path: &Path) -> Result<String> {
    let mut root = read_config(path)?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} must contain a JSON object", path.display()))?;
    remove_oy_config_entries(object)?;
    format_json(&root)
}

pub(super) fn config_body(path: &Path) -> Result<String> {
    let mut root = read_config(path)?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} must contain a JSON object", path.display()))?;
    remove_oy_config_entries(object)?;
    merge_plugin(object)?;
    format_json(&root)
}

fn read_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!(
            "refusing to read symlinked OpenCode config {}",
            path.display()
        );
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    parse_opencode_config(&text).with_context(|| {
        format!(
            "{} must be valid opencode JSON/JSONC for oy setup to update it",
            path.display()
        )
    })
}

#[cfg(test)]
pub(super) fn update_config(path: &Path) -> Result<()> {
    let body = config_body(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, body)?;
    Ok(())
}

pub(super) fn remove_oy_config_entries(object: &mut Map<String, Value>) -> Result<()> {
    remove_owned_plugins(object)?;
    for key in ["command", "commands"] {
        let remove = object
            .get_mut(key)
            .and_then(Value::as_object_mut)
            .is_some_and(|entries| {
                entries.retain(|name, _| !is_oy_name(name));
                entries.is_empty()
            });
        if remove {
            object.remove(key);
        }
    }
    if let Some(mcp) = object.get_mut("mcp").and_then(Value::as_object_mut) {
        mcp.remove("oy");
        if let Some(servers) = mcp.get_mut("servers").and_then(Value::as_object_mut) {
            servers.remove("oy");
            if servers.is_empty() {
                mcp.remove("servers");
            }
        }
        if mcp.is_empty() {
            object.remove("mcp");
        }
    }
    Ok(())
}

fn is_oy_name(name: &str) -> bool {
    name == "oy" || name.starts_with("oy-")
}

pub(super) fn opencode_plugin_spec() -> String {
    format!("{OPENCODE_PLUGIN_PACKAGE}@{}", env!("CARGO_PKG_VERSION"))
}

fn is_oy_plugin_spec(value: &str) -> bool {
    value == OPENCODE_PLUGIN_PACKAGE
        || value
            .strip_prefix(OPENCODE_PLUGIN_PACKAGE)
            .is_some_and(|suffix| suffix.starts_with('@') && suffix.len() > 1)
}

fn is_oy_plugin_value(value: &Value) -> bool {
    value.as_str().is_some_and(is_oy_plugin_spec)
        || value
            .get("package")
            .and_then(Value::as_str)
            .is_some_and(is_oy_plugin_spec)
}

fn remove_owned_plugins(object: &mut Map<String, Value>) -> Result<()> {
    let Some(plugins) = object.get_mut("plugins") else {
        return Ok(());
    };
    let Some(plugins) = plugins.as_array_mut() else {
        bail!("native OpenCode `plugins` must be an array");
    };
    plugins.retain(|plugin| !is_oy_plugin_value(plugin));
    if plugins.is_empty() {
        object.remove("plugins");
    }
    Ok(())
}

fn merge_plugin(object: &mut Map<String, Value>) -> Result<()> {
    remove_owned_plugins(object)?;
    let plugins = object
        .entry("plugins")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(plugins) = plugins.as_array_mut() else {
        bail!("native OpenCode `plugins` must be an array");
    };
    plugins.push(Value::String(opencode_plugin_spec()));
    Ok(())
}

pub(super) fn config_has_oy_entries(config: &Value) -> bool {
    config
        .get("plugins")
        .and_then(Value::as_array)
        .is_some_and(|plugins| plugins.iter().any(is_oy_plugin_value))
        || ["command", "commands"].iter().any(|key| {
            config
                .get(*key)
                .and_then(Value::as_object)
                .is_some_and(|entries| entries.keys().any(|name| is_oy_name(name)))
        })
        || config
            .get("mcp")
            .and_then(Value::as_object)
            .is_some_and(|mcp| {
                mcp.contains_key("oy")
                    || mcp
                        .get("servers")
                        .and_then(Value::as_object)
                        .is_some_and(|servers| servers.contains_key("oy"))
            })
}

pub(super) fn config_has_all_oy_entries(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(config) = parse_opencode_config(&text) else {
        return false;
    };
    config
        .get("plugins")
        .and_then(Value::as_array)
        .is_some_and(|plugins| {
            plugins
                .iter()
                .any(|plugin| plugin.as_str() == Some(opencode_plugin_spec().as_str()))
        })
}

pub(super) fn format_json(value: &Value) -> Result<String> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    Ok(text)
}

pub(super) fn parse_opencode_config(text: &str) -> Result<Value> {
    Ok(serde_json::from_str::<Value>(text)
        .or_else(|_| serde_json::from_str::<Value>(&strip_jsonc(text)))?)
}

fn strip_jsonc(text: &str) -> String {
    let mut without_comments = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            without_comments.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            without_comments.push(ch);
            continue;
        }
        if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            without_comments.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for next in chars.by_ref() {
                        if previous == '*' && next == '/' {
                            break;
                        }
                        if next == '\n' {
                            without_comments.push('\n');
                        }
                        previous = next;
                    }
                }
                _ => without_comments.push(ch),
            }
            continue;
        }
        without_comments.push(ch);
    }

    remove_trailing_commas(&without_comments)
}

fn remove_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars = text.chars().collect::<Vec<_>>();
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in chars.iter().copied().enumerate() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == ',' {
            let next = chars[idx + 1..]
                .iter()
                .copied()
                .find(|next| !next.is_whitespace());
            if matches!(next, Some('}' | ']')) {
                continue;
            }
        }
        out.push(ch);
    }
    out
}
