//! OpenCode JSON/JSONC parsing and oy-owned config stripping.
//!
//! Older oy releases registered the `@oy-cli/opencode` plugin and oy-named
//! commands in OpenCode config files. Setup now installs plain skill files
//! and strips those legacy entries, preserving unrelated config.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::fs;
use std::path::Path;

const LEGACY_PLUGIN_PACKAGE: &str = "@oy-cli/opencode";

/// Strip oy-owned entries from an OpenCode config, returning `None` when the
/// file is unchanged so unmodified configs are never rewritten.
pub(super) fn strip_owned_config(path: &Path) -> Result<Option<String>> {
    let mut root = read_config(path)?;
    if !config_has_oy_entries(&root) {
        return Ok(None);
    }
    let object = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} must contain a JSON object", path.display()))?;
    remove_oy_config_entries(object)?;
    format_json(&root).map(Some)
}

#[cfg(test)]
pub(super) fn update_config(path: &Path) -> Result<()> {
    let Some(body) = strip_owned_config(path)? else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, body)?;
    Ok(())
}

fn read_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
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

pub(super) fn remove_oy_config_entries(object: &mut Map<String, Value>) -> Result<()> {
    remove_legacy_plugins(object)?;
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

fn is_legacy_plugin_value(value: &Value) -> bool {
    value.as_str().is_some_and(|spec| {
        spec == LEGACY_PLUGIN_PACKAGE
            || spec
                .strip_prefix(LEGACY_PLUGIN_PACKAGE)
                .is_some_and(|suffix| suffix.starts_with('@') && suffix.len() > 1)
    }) || value
        .get("package")
        .and_then(Value::as_str)
        .is_some_and(|spec| {
            spec == LEGACY_PLUGIN_PACKAGE
                || spec
                    .strip_prefix(LEGACY_PLUGIN_PACKAGE)
                    .is_some_and(|suffix| suffix.starts_with('@') && suffix.len() > 1)
        })
}

fn remove_legacy_plugins(object: &mut Map<String, Value>) -> Result<()> {
    let Some(plugins) = object.get_mut("plugins") else {
        return Ok(());
    };
    let Some(plugins) = plugins.as_array_mut() else {
        bail!("native OpenCode `plugins` must be an array");
    };
    plugins.retain(|plugin| !is_legacy_plugin_value(plugin));
    if plugins.is_empty() {
        object.remove("plugins");
    }
    Ok(())
}

pub(super) fn config_has_oy_entries(config: &Value) -> bool {
    config
            .get("plugins")
            .and_then(Value::as_array)
            .is_some_and(|plugins| plugins.iter().any(is_legacy_plugin_value))
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
