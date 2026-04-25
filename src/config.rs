use anyhow::{Context, Result, bail};
use chrono::Utc;
use dirs::config_dir;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedModelConfig {
    pub model: Option<String>,
    pub shim: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFile {
    pub model: String,
    pub agent: String,
    pub saved_at: String,
    pub transcript: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct AgentProfile {
    pub name: String,
    pub system_prompt_suffix: String,
    pub tool_mode: ToolMode,
    pub auto_approve_edits: bool,
    pub auto_approve_bash: bool,
    pub yolo: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMode {
    Normal,
    ReadOnly,
}

#[derive(Debug, Clone, Deserialize)]
struct SessionText {
    #[serde(default)]
    system: BTreeMap<String, String>,
    #[serde(default)]
    agents: BTreeMap<String, AgentText>,
    #[serde(default)]
    transcript: BTreeMap<String, String>,
    #[serde(default)]
    audit: BTreeMap<String, String>,
    #[serde(default)]
    audit_logic: BTreeMap<String, String>,
    #[serde(default)]
    tools: BTreeMap<String, ToolText>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct AgentText {
    #[serde(default)]
    system: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ToolText {
    #[serde(default)]
    description: String,
}

static SESSION_TEXT: OnceLock<SessionText> = OnceLock::new();
const DEFAULT_CONFIG_DIR_NAME: &str = "oy-rust";

pub fn config_root() -> PathBuf {
    if let Ok(raw) = env::var("OY_CONFIG") {
        return PathBuf::from(&raw)
            .expand_home()
            .unwrap_or_else(|_| PathBuf::from(raw));
    }
    config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join(DEFAULT_CONFIG_DIR_NAME)
        .join("config.json")
}

pub fn oy_root() -> Result<PathBuf> {
    let raw_root = env::var("OY_ROOT").unwrap_or_else(|_| ".".to_string());
    let path = PathBuf::from(&raw_root)
        .expand_home()
        .unwrap_or_else(|_| PathBuf::from(raw_root))
        .canonicalize()
        .context("failed to resolve workspace root")?;
    if !path.is_dir() {
        bail!("Workspace root is not a directory: {}", path.display());
    }
    Ok(path)
}

pub fn config_dir_path() -> PathBuf {
    config_root()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(format!(".config/{DEFAULT_CONFIG_DIR_NAME}")))
}

pub fn sessions_dir() -> Result<PathBuf> {
    let dir = config_dir_path().join("sessions");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}

fn session_text() -> &'static SessionText {
    SESSION_TEXT.get_or_init(|| load_session_text().expect("session_text.toml must load"))
}

fn load_session_text() -> Result<SessionText> {
    const RAW: &str = include_str!("../assets/session_text.toml");
    toml::from_str(RAW).context("failed parsing embedded session_text.toml")
}

pub fn session_text_value(section: &str, key: &str) -> Result<String> {
    let value = match section {
        "system" => session_text().system.get(key),
        "transcript" => session_text().transcript.get(key),
        "audit" => session_text().audit.get(key),
        "audit_logic" => session_text().audit_logic.get(key),
        _ => None,
    };
    value
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing session text key: {section}.{key}"))
}

pub fn session_text_format(section: &str, key: &str, values: &[(&str, String)]) -> Result<String> {
    let mut text = session_text_value(section, key)?;
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    Ok(text)
}

pub fn tool_description(name: &str) -> String {
    session_text()
        .tools
        .get(name)
        .map(|tool| tool.description.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| name.to_string())
}

pub fn list_agent_profiles() -> Vec<String> {
    let mut items = session_text()
        .agents
        .keys()
        .map(|name| name.replace('_', "-"))
        .collect::<Vec<_>>();
    if items.is_empty() {
        items.push("default".to_string());
    }
    items.sort();
    items
}

pub fn normalize_agent_profile(agent: &str) -> Result<String> {
    let value = if agent.trim().is_empty() {
        "default".to_string()
    } else {
        agent.trim().to_ascii_lowercase().replace('_', "-")
    };
    let available = list_agent_profiles();
    if available.iter().any(|item| item == &value) {
        return Ok(value);
    }
    bail!(
        "Unknown agent profile `{}`. Available: {}",
        value,
        available.join(", ")
    )
}

pub fn agent_profile(agent: &str) -> Result<AgentProfile> {
    let name = normalize_agent_profile(agent)?;
    let raw = session_text()
        .agents
        .get(&name.replace('-', "_"))
        .cloned()
        .unwrap_or_default();
    let mut profile = AgentProfile {
        name: name.clone(),
        system_prompt_suffix: raw.system.trim().to_string(),
        tool_mode: ToolMode::Normal,
        auto_approve_edits: false,
        auto_approve_bash: false,
        yolo: false,
    };
    match name.as_str() {
        "plan" => profile.tool_mode = ToolMode::ReadOnly,
        "accept-edits" => profile.auto_approve_edits = true,
        "auto-approve" => {
            profile.yolo = true;
            profile.auto_approve_edits = true;
            profile.auto_approve_bash = true;
        }
        _ => {}
    }
    Ok(profile)
}

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
        fs::create_dir_all(parent)?;
    }
    let payload = saved_model_config_from_selection(model_spec);
    let text = serde_json::to_string_pretty(&payload)?;
    fs::write(&path, text).with_context(|| format!("failed writing {}", path.display()))?;
    Ok(())
}

pub fn saved_model_config_from_selection(model_spec: &str) -> SavedModelConfig {
    let model_spec = model_spec.trim();
    let (prefix, model) = split_model_spec(model_spec);
    if let Some(shim) = prefix.filter(|shim| is_routing_shim(shim)) {
        return SavedModelConfig {
            model: Some(genai_model_for_shim(shim, model)),
            shim: Some(shim.to_string()),
        };
    }
    SavedModelConfig {
        model: Some(model_spec.to_string()),
        shim: None,
    }
}

fn genai_model_for_shim(shim: &str, model: &str) -> String {
    if is_copilot_shim(shim) && is_openai_responses_model(model) {
        format!("openai_resp::{model}")
    } else {
        model.to_string()
    }
}

fn is_openai_responses_model(model: &str) -> bool {
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
        "openai" | "codex" | "bedrock-mantle" | "copilot" | "opencode"
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

pub fn yolo_enabled() -> bool {
    env_flag("OY_YOLO", false)
}

pub fn non_interactive() -> bool {
    env_flag("OY_NON_INTERACTIVE", false)
}

pub fn can_prompt() -> bool {
    std::io::stdin().is_terminal() && !non_interactive()
}

pub fn active_system_prompt(interactive: bool, agent: &str) -> String {
    let mut prompt = session_text_value("system", "base").unwrap_or_default();
    let suffix_key = if interactive {
        "interactive_suffix"
    } else {
        "noninteractive_suffix"
    };
    if let Ok(suffix) = session_text_value("system", suffix_key) {
        if !suffix.trim().is_empty() {
            prompt.push('\n');
            prompt.push_str(suffix.trim());
        }
    }
    if let Ok(profile) = agent_profile(agent) {
        if !profile.system_prompt_suffix.trim().is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(profile.system_prompt_suffix.trim());
        }
    }
    if let Ok(raw) = env::var("OY_SYSTEM_FILE") {
        let path = PathBuf::from(&raw)
            .expand_home()
            .unwrap_or_else(|_| PathBuf::from(raw));
        if path.is_file() {
            if let Ok(extra) = fs::read_to_string(path) {
                if !extra.trim().is_empty() {
                    prompt.push_str("\n\n");
                    prompt.push_str(extra.trim());
                }
            }
        }
    }
    prompt
}

pub fn ask_system_prompt(prompt: &str) -> String {
    let suffix = session_text_value("system", "ask_suffix").unwrap_or_default();
    if suffix.trim().is_empty() {
        prompt.to_string()
    } else {
        format!("{}\n\n{}", prompt.trim_end(), suffix.trim())
    }
}

pub fn ralph_limit_seconds() -> u64 {
    parse_duration_env("OY_RALPH_LIMIT", 3 * 3600)
}

pub fn max_bash_cmd_bytes() -> usize {
    env::var("OY_MAX_BASH_CMD_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16 * 1024)
}

pub fn save_session_file(name: Option<&str>, file: &SessionFile) -> Result<PathBuf> {
    let sessions = sessions_dir()?;
    let stem = name
        .filter(|s| !s.trim().is_empty())
        .map(sanitize_session_name)
        .unwrap_or_else(|| Utc::now().format("%Y%m%d-%H%M%S").to_string());
    let path = sessions.join(format!("{stem}.json"));
    let body = serde_json::to_string_pretty(file)?;
    fs::write(&path, body).with_context(|| format!("failed writing {}", path.display()))?;
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
    if let Ok(index) = name.parse::<usize>() {
        if index >= 1 && index <= sessions.len() {
            return Ok(Some(sessions[index - 1].clone()));
        }
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
    Ok(
        serde_json::from_str(&data)
            .with_context(|| format!("failed parsing {}", path.display()))?,
    )
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

fn parse_duration_env(name: &str, default: u64) -> u64 {
    let Some(value) = env::var(name).ok() else {
        return default;
    };
    parse_duration_seconds(&value).unwrap_or(default)
}

pub fn parse_duration_seconds(value: &str) -> Result<u64> {
    let value = value.trim();
    if let Some(num) = value.strip_suffix('h') {
        return Ok(num.trim().parse::<u64>()? * 3600);
    }
    if let Some(num) = value.strip_suffix('m') {
        return Ok(num.trim().parse::<u64>()? * 60);
    }
    if let Some(num) = value.strip_suffix('s') {
        return Ok(num.trim().parse::<u64>()?);
    }
    Ok(value.parse::<u64>()?)
}

fn env_flag(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

trait ExpandHome {
    fn expand_home(self) -> Result<PathBuf>;
}

impl ExpandHome for PathBuf {
    fn expand_home(self) -> Result<PathBuf> {
        let text = self.to_string_lossy();
        if text == "~" || text.starts_with("~/") {
            let home = dirs::home_dir().context("home directory not found")?;
            let suffix = text
                .strip_prefix('~')
                .unwrap_or_default()
                .trim_start_matches('/');
            return Ok(if suffix.is_empty() {
                home
            } else {
                home.join(suffix)
            });
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_dir_name_is_rust_specific() {
        assert_eq!(DEFAULT_CONFIG_DIR_NAME, "oy-rust");
    }

    #[test]
    fn saved_model_config_keeps_exact_genai_model_and_infers_routing_shim() {
        let saved = saved_model_config_from_selection("copilot::gpt-5.5");
        assert_eq!(saved.model.as_deref(), Some("openai_resp::gpt-5.5"));
        assert_eq!(saved.shim.as_deref(), Some("copilot"));

        let saved = saved_model_config_from_selection("openai_resp::gpt-5.5");
        assert_eq!(saved.model.as_deref(), Some("openai_resp::gpt-5.5"));
        assert_eq!(saved.shim.as_deref(), None);
    }

    #[test]
    fn split_model_spec_supports_double_colon() {
        assert_eq!(
            split_model_spec("copilot::gpt-4.1-mini"),
            (Some("copilot"), "gpt-4.1-mini")
        );
    }

    #[test]
    fn split_model_spec_leaves_plain_models_untouched() {
        assert_eq!(split_model_spec("gpt-5.4-mini"), (None, "gpt-5.4-mini"));
    }

    #[test]
    fn session_text_loads_base_prompt() {
        assert!(
            session_text_value("system", "base")
                .unwrap()
                .contains("You are oy")
        );
    }
}
