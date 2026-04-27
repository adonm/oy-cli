use crate::tools::{Approval, ToolPolicy};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use dirs::config_dir;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedModelConfig {
    pub model: Option<String>,
    pub shim: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFile {
    pub model: String,
    #[serde(default, alias = "agent")]
    pub mode: String,
    pub saved_at: String,
    #[serde(default)]
    pub workspace_root: Option<PathBuf>,
    pub transcript: serde_json::Value,
    #[serde(default)]
    pub todos: Vec<crate::tools::TodoItem>,
}

#[derive(Debug, Clone, Copy)]
pub struct ContextConfig {
    pub limit_tokens: usize,
    pub output_reserve_tokens: usize,
    pub safety_reserve_tokens: usize,
    pub trigger_ratio: f64,
    pub recent_messages: usize,
    pub tool_output_tokens: usize,
    pub summary_tokens: usize,
}

impl ContextConfig {
    pub fn input_budget_tokens(self) -> usize {
        self.limit_tokens
            .saturating_sub(self.output_reserve_tokens)
            .saturating_sub(self.safety_reserve_tokens)
            .max(1)
    }

    pub fn trigger_tokens(self) -> usize {
        ((self.input_budget_tokens() as f64) * self.trigger_ratio) as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyMode {
    Default,
    Plan,
    AutoEdits,
    AutoAll,
}

impl SafetyMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "" | "default" | "ask" => Ok(Self::Default),
            "plan" | "read-only" | "readonly" | "read" => Ok(Self::Plan),
            "accept-edits" | "edit" | "edits" | "auto-edits" | "write" => Ok(Self::AutoEdits),
            "auto-approve" | "auto" | "yolo" => Ok(Self::AutoAll),
            other => bail!("Unknown mode `{other}`. Available: plan, ask, edit, auto"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
            Self::AutoEdits => "accept-edits",
            Self::AutoAll => "auto-approve",
        }
    }

    fn system_prompt_suffix(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Plan => PLAN_SYSTEM,
            Self::AutoEdits => ACCEPT_EDITS_SYSTEM,
            Self::AutoAll => AUTO_APPROVE_SYSTEM,
        }
    }

    fn policy(self) -> ToolPolicy {
        match self {
            Self::Plan => ToolPolicy::read_only(),
            Self::Default => ToolPolicy {
                read_only: false,
                files_write: Approval::Ask,
                shell: Approval::Ask,
                network: true,
            },
            Self::AutoEdits => ToolPolicy {
                read_only: false,
                files_write: Approval::Auto,
                shell: Approval::Ask,
                network: true,
            },
            Self::AutoAll => ToolPolicy {
                read_only: false,
                files_write: Approval::Auto,
                shell: Approval::Auto,
                network: true,
            },
        }
    }
}

const DEFAULT_CONFIG_DIR_NAME: &str = "oy-rust";

const BASE_SYSTEM: &str = r#"You are oy, a coding CLI with tools.
Optimize for the human reviewing your work: be terse, evidence-first, and explicit about changed files/commands.
Follow the user's output constraints exactly.
Work inspect → edit → verify. Use the cheapest sufficient tool:
1. `list` for discovery.
2. `search` for symbols, paths, and strings.
3. `read` only narrow file slices you need.
4. `replace` for surgical edits.
5. `bash` only when file tools are insufficient or when you must run/check something.
Batch independent reads/searches. Stop when enough evidence exists.
Prefer small, boring, idiomatic, functional, testable code with explicit data flow.
For security-sensitive work, name the trust boundary, validate near it, fail closed, and add focused tests.
Do not add file, process, network, credential, or persistence capability unless necessary.
For 3+ step work, keep a short in-memory todo; persist `TODO.md` only on explicit request or quit prompt.
Use `webfetch` for public docs/API research when useful; prefer it over guessing.
Tool arguments are schemas, not prose: use documented names, numeric `limit`/`offset`/timeouts, and `mode=literal` for exact search/replace when regex metacharacters are not intended.
Manage context aggressively: keep only key facts and paths. Prefer narrow `path`, `offset`, `limit`, and `exclude`; use `sloc` if you need a repo-size snapshot.
Before mutating files or running commands, state the next action briefly. After finishing, report changed files and checks.
When context gets long, compress to the plan, key evidence, and next action. If blocked, say what you tried and the next step."#;

const INTERACTIVE_SUFFIX: &str =
    "Use `ask` only for genuine ambiguity or irreversible user-facing choices. Batch prompts.";
const NONINTERACTIVE_SUFFIX: &str = "Non-interactive mode: stay unblocked without questions. Choose the safest reasonable path, state brief assumptions, and finish the inspect/edit/verify flow.";
const ASK_SUFFIX: &str = r#"RESEARCH-ONLY mode. Use only list, read, search, sloc, and webfetch. Stay no-write: leave files unchanged and skip `bash`. Focus on facts only, citing file paths and brief evidence."#;
const PLAN_SYSTEM: &str = r#"PLAN mode. Stay read-only. Use only list, read, search, sloc, todo for in-memory planning, ask when interactive, and webfetch when available. Keep files unchanged, skip shell commands, and describe changes as proposed rather than applied."#;
const ACCEPT_EDITS_SYSTEM: &str = r#"ACCEPT-EDITS mode. File edits may run without asking. Keep edits small and targeted, inspect before changing, and reach for `bash` only when genuinely necessary."#;
const AUTO_APPROVE_SYSTEM: &str = r#"AUTO-APPROVE mode. Tools may run without asking. Still avoid destructive commands, broad rewrites, credential exposure, persistence changes, and network/file/process expansion unless clearly needed. Treat shell and replacement tools as strict side effects: inspect first, then run the smallest command/edit."#;
const TODO_SYSTEM: &str = r#"Current in-memory todo:
{todos}"#;

pub fn session_text_value(section: &str, key: &str) -> Result<String> {
    let value = match (section, key) {
        ("system", "base") => BASE_SYSTEM,
        ("system", "interactive_suffix") => INTERACTIVE_SUFFIX,
        ("system", "noninteractive_suffix") => NONINTERACTIVE_SUFFIX,
        ("system", "ask_suffix") => ASK_SUFFIX,
        ("transcript", "todo_system") => TODO_SYSTEM,
        _ => bail!("missing session text key: {section}.{key}"),
    };
    Ok(value.to_string())
}

pub fn tool_description(name: &str) -> String {
    match name {
        "list" => "List workspace paths. Use first for discovery. `path` is a workspace-relative glob and defaults to `*`. Returns items, count, and truncation state.",
        "read" => "Read one UTF-8 text file. Prefer narrow `offset`/`limit` slices over full-file reads.",
        "search" => "Search workspace text with ripgrep-style Rust regex. Use `mode=literal` for exact strings.",
        "replace" => "Replace workspace text with Rust regex captures, or exact text with `mode=literal`. Inspect/search before changing.",
        "sloc" => "Count source lines with tokei for repository sizing. `path` may be one path or whitespace-separated paths.",
        "bash" => "Run a shell command in the workspace. Use only when file tools are insufficient or when you must run/check something.",
        "ask" => "Ask the user in interactive runs. Reserve for genuine ambiguity or irreversible choices.",
        "webfetch" => "Fetch public web pages/files. Blocks localhost/private IPs and sensitive headers.",
        "todo" => "Manage the in-memory todo list. Available in read-only modes; persistence to TODO.md is opt-in and requires write approval.",
        other => other,
    }
    .to_string()
}

pub fn safety_mode(mode: &str) -> Result<SafetyMode> {
    SafetyMode::parse(mode)
}

pub fn tool_policy(mode: &str) -> ToolPolicy {
    let mode = SafetyMode::parse(mode).unwrap_or(SafetyMode::Default);
    mode.policy()
}

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
    create_private_dir_all(&dir)?;
    Ok(dir)
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
        create_private_dir_all(parent)?;
    }
    let payload = saved_model_config_from_selection(model_spec);
    let text = serde_json::to_string_pretty(&payload)?;
    write_private_file(&path, text.as_bytes())?;
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

pub fn policy_risk_label(policy: &ToolPolicy) -> &'static str {
    if policy.read_only {
        "read-only: no file edits or shell"
    } else if policy.shell == Approval::Auto {
        "high: auto shell"
    } else if policy.files_write == Approval::Auto {
        "medium: auto edits"
    } else {
        "normal: asks before edits/shell"
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

pub fn non_interactive() -> bool {
    env_flag("OY_NON_INTERACTIVE", false)
}

pub fn can_prompt() -> bool {
    std::io::stdin().is_terminal() && !non_interactive()
}

pub fn context_config() -> ContextConfig {
    let limit_tokens = parse_usize_env("OY_CONTEXT_LIMIT", 128_000).max(1_000);
    let output_reserve_tokens = parse_usize_env("OY_CONTEXT_OUTPUT_RESERVE", 12_000);
    let safety_reserve_tokens = parse_usize_env("OY_CONTEXT_SAFETY_RESERVE", 4_000);
    ContextConfig {
        limit_tokens,
        output_reserve_tokens,
        safety_reserve_tokens,
        trigger_ratio: parse_f64_env("OY_COMPACT_TRIGGER", 0.80).clamp(0.10, 1.0),
        recent_messages: parse_usize_env("OY_COMPACT_RECENT_MESSAGES", 16).max(1),
        tool_output_tokens: parse_usize_env("OY_COMPACT_TOOL_OUTPUT_TOKENS", 4_000).max(256),
        summary_tokens: parse_usize_env("OY_COMPACT_SUMMARY_TOKENS", 8_000).max(512),
    }
}

pub fn system_prompt(interactive: bool, mode: &str) -> String {
    let mut prompt = BASE_SYSTEM.to_string();
    prompt.push('\n');
    prompt.push_str(if interactive {
        INTERACTIVE_SUFFIX
    } else {
        NONINTERACTIVE_SUFFIX
    });
    if let Ok(mode) = safety_mode(mode) {
        let suffix = mode.system_prompt_suffix().trim();
        if !suffix.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(suffix);
        }
    }
    if let Ok(raw) = env::var("OY_SYSTEM_FILE") {
        let path = PathBuf::from(&raw)
            .expand_home()
            .unwrap_or_else(|_| PathBuf::from(raw));
        if path.is_file()
            && let Ok(extra) = fs::read_to_string(path)
            && !extra.trim().is_empty()
        {
            prompt.push_str("\n\n");
            prompt.push_str(extra.trim());
        }
    }
    prompt
}

pub fn ask_system_prompt(prompt: &str) -> String {
    format!("{}\n\n{}", prompt.trim_end(), ASK_SUFFIX)
}

pub fn max_bash_cmd_bytes() -> usize {
    env::var("OY_MAX_BASH_CMD_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16 * 1024)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRoundLimit {
    Limited(usize),
    Unlimited,
}

impl ToolRoundLimit {
    pub fn exceeded(self, completed_rounds: usize) -> bool {
        matches!(self, Self::Limited(max) if completed_rounds > max)
    }

    pub fn label(self) -> String {
        match self {
            Self::Limited(max) => max.to_string(),
            Self::Unlimited => "unlimited".to_string(),
        }
    }
}

pub fn max_tool_rounds(default: usize) -> ToolRoundLimit {
    parse_tool_round_limit(env::var("OY_MAX_TOOL_ROUNDS").ok().as_deref(), default)
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

fn parse_usize_env(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn parse_tool_round_limit(value: Option<&str>, default: usize) -> ToolRoundLimit {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return ToolRoundLimit::Limited(default.max(1));
    };
    if matches!(
        value.to_ascii_lowercase().as_str(),
        "unlimited" | "none" | "off"
    ) {
        return ToolRoundLimit::Unlimited;
    }
    match value.parse::<usize>() {
        Ok(0) => ToolRoundLimit::Unlimited,
        Ok(max) => ToolRoundLimit::Limited(max),
        Err(_) => ToolRoundLimit::Limited(default.max(1)),
    }
}

fn parse_f64_env(name: &str, default: f64) -> f64 {
    env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}

pub fn write_workspace_file(path: &Path, bytes: &[u8]) -> Result<()> {
    reject_symlink_destination(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        let mode = fs::metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o600);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(mode)
            .open(path)
            .with_context(|| format!("failed writing {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed writing {}", path.display()))?;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(mode);
        file.set_permissions(perms)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes).with_context(|| format!("failed writing {}", path.display()))
    }
}

pub fn resolve_workspace_output_path(root: &Path, requested: &Path) -> Result<PathBuf> {
    if requested.is_absolute()
        || requested
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!(
            "output path must stay inside workspace: {}",
            requested.display()
        );
    }
    let root = root
        .canonicalize()
        .context("failed to resolve workspace root")?;
    let path = root.join(requested);
    let parent = path.parent().unwrap_or(&root);
    if parent.exists() {
        let resolved_parent = parent
            .canonicalize()
            .with_context(|| format!("failed resolving {}", parent.display()))?;
        if !resolved_parent.starts_with(&root) {
            bail!("output path escapes workspace: {}", requested.display());
        }
    }
    reject_symlink_destination(&path)?;
    Ok(path)
}

pub fn reject_symlink_destination(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("refusing to write symlink: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed checking {}", path.display())),
    }
}

pub fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        if let Some(parent) = path.parent() {
            create_private_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("failed writing {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed writing {}", path.display()))?;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(0o600);
        file.set_permissions(perms)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes).with_context(|| format!("failed writing {}", path.display()))
    }
}

pub fn create_private_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
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
    fn mode_policy_and_risk_labels_are_centralized() {
        let plan = tool_policy("plan");
        assert_eq!(safety_mode("ask").unwrap().name(), "default");
        assert_eq!(safety_mode("read_only").unwrap().name(), "plan");
        assert_eq!(safety_mode("edit").unwrap().name(), "accept-edits");
        assert_eq!(safety_mode("yolo").unwrap().name(), "auto-approve");
        assert!(plan.read_only);
        assert_eq!(
            policy_risk_label(&plan),
            "read-only: no file edits or shell"
        );
        assert_eq!(
            policy_risk_label(&tool_policy("accept-edits")),
            "medium: auto edits"
        );
        assert_eq!(
            policy_risk_label(&tool_policy("auto-approve")),
            "high: auto shell"
        );
    }

    #[test]
    fn output_paths_stay_in_workspace() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_workspace_output_path(dir.path(), Path::new("notes/out.md")).is_ok());
        assert!(resolve_workspace_output_path(dir.path(), Path::new("../out.md")).is_err());
        assert!(resolve_workspace_output_path(dir.path(), Path::new("/tmp/out.md")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn output_paths_reject_symlink_destinations() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.md");
        fs::write(&target, "safe").unwrap();
        symlink(&target, dir.path().join("link.md")).unwrap();
        let err = resolve_workspace_output_path(dir.path(), Path::new("link.md")).unwrap_err();
        assert!(err.to_string().contains("refusing to write symlink"));
    }

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

    #[test]
    fn session_file_loads_legacy_without_todos() {
        let raw = r#"{
            "model": "gpt-test",
            "agent": "default",
            "saved_at": "2026-01-01T00:00:00",
            "transcript": {"messages": []}
        }"#;
        let file: SessionFile = serde_json::from_str(raw).unwrap();
        assert!(file.todos.is_empty());
        assert!(file.workspace_root.is_none());
    }

    #[test]
    fn tool_round_limit_supports_high_and_unlimited_values() {
        assert_eq!(
            parse_tool_round_limit(None, 512),
            ToolRoundLimit::Limited(512)
        );
        assert_eq!(
            parse_tool_round_limit(Some("2048"), 512),
            ToolRoundLimit::Limited(2048)
        );
        assert_eq!(
            parse_tool_round_limit(Some("0"), 512),
            ToolRoundLimit::Unlimited
        );
        assert_eq!(
            parse_tool_round_limit(Some("unlimited"), 512),
            ToolRoundLimit::Unlimited
        );
        assert_eq!(
            parse_tool_round_limit(Some("bad"), 512),
            ToolRoundLimit::Limited(512)
        );
        assert!(ToolRoundLimit::Limited(2).exceeded(3));
        assert!(!ToolRoundLimit::Unlimited.exceeded(usize::MAX));
    }
}
