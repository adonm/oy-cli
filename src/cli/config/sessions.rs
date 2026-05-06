use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use super::mode::SafetyMode;
use super::paths::{sessions_dir, write_private_file};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFile {
    pub model: String,
    pub saved_at: String,
    pub workspace_root: PathBuf,
    #[serde(default)]
    pub mode: Option<SafetyMode>,
    pub transcript: serde_json::Value,
    #[serde(default)]
    pub todos: Vec<crate::tools::TodoItem>,
}

pub fn save_session_file(name: Option<&str>, file: &SessionFile) -> Result<PathBuf> {
    let sessions = sessions_dir()?;
    let stem = name
        .filter(|s| !s.trim().is_empty())
        .map(sanitize_session_name)
        .unwrap_or_else(|| Utc::now().format("%Y%m%d-%H%M%S").to_string());
    let path = sessions.join(format!("{stem}.json"));
    let body = serde_json::to_string_pretty(file)?;
    write_private_file(&path, body.as_bytes())?;
    Ok(path)
}

pub fn list_saved_sessions() -> Result<Vec<PathBuf>> {
    let dir = sessions_dir()?;
    let mut items = fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    items.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    items.reverse();
    Ok(items)
}

pub fn resolve_saved_session(name: Option<&str>) -> Result<Option<PathBuf>> {
    let sessions = list_saved_sessions()?;
    if sessions.is_empty() {
        return Ok(None);
    }
    let Some(name) = name else {
        return Ok(sessions.first().cloned());
    };
    if let Ok(index) = name.parse::<usize>()
        && index >= 1
        && index <= sessions.len()
    {
        return Ok(Some(sessions[index - 1].clone()));
    }
    if let Some(exact) = sessions
        .iter()
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(name))
    {
        return Ok(Some(exact.clone()));
    }
    Ok(sessions
        .iter()
        .find(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.contains(name))
        })
        .cloned())
}

pub fn load_session_file(path: &Path) -> Result<SessionFile> {
    let data =
        fs::read_to_string(path).with_context(|| format!("failed reading {}", path.display()))?;
    serde_json::from_str(&data).with_context(|| format!("failed parsing {}", path.display()))
}

pub fn sanitize_session_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}
