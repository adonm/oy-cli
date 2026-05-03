// === config ===
pub(crate) mod config {
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
        #[serde(default)]
        pub recent_models: Vec<String>,
    }

    const RECENT_MODEL_LIMIT: usize = 5;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SessionFile {
        pub model: String,
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
        "webfetch" => "Fetch public web pages/files. Follows public redirects by default; blocks localhost/private IPs and sensitive headers.",
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
        let data = fs::read_to_string(&path)
            .with_context(|| format!("failed reading {}", path.display()))?;
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

    fn updated_recent_models(previous: &[String], selected: &str) -> Vec<String> {
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
        let data = fs::read_to_string(path)
            .with_context(|| format!("failed reading {}", path.display()))?;
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
        use std::sync::Mutex;

        static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

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
            assert!(saved.recent_models.is_empty());

            let saved = saved_model_config_from_selection("openai_resp::gpt-5.5");
            assert_eq!(saved.model.as_deref(), Some("openai_resp::gpt-5.5"));
            assert_eq!(saved.shim.as_deref(), None);
            assert!(saved.recent_models.is_empty());
        }

        #[test]
        fn saved_model_config_defaults_legacy_recent_models() {
            let saved: SavedModelConfig =
                serde_json::from_str(r#"{"model":"gpt-test","shim":null}"#).unwrap();
            assert_eq!(saved.model.as_deref(), Some("gpt-test"));
            assert!(saved.recent_models.is_empty());
        }

        #[test]
        fn recent_models_are_deduped_most_recent_first_and_limited() {
            let previous = vec![
                "gpt-a".to_string(),
                "gpt-b".to_string(),
                "gpt-c".to_string(),
                "gpt-d".to_string(),
                "gpt-e".to_string(),
            ];
            assert_eq!(
                updated_recent_models(&previous, " gpt-c "),
                vec!["gpt-c", "gpt-a", "gpt-b", "gpt-d", "gpt-e"]
            );
            assert_eq!(
                updated_recent_models(&previous, "gpt-f"),
                vec!["gpt-f", "gpt-a", "gpt-b", "gpt-c", "gpt-d"]
            );
        }

        #[test]
        fn save_and_clear_model_config_persist_recent_models() {
            let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
            let dir = tempfile::tempdir().unwrap();
            let config = dir.path().join("config.json");
            unsafe { env::set_var("OY_CONFIG", &config) };

            save_model_config("gpt-a").unwrap();
            save_model_config("gpt-b").unwrap();
            save_model_config("gpt-a").unwrap();
            assert_eq!(recent_models().unwrap(), vec!["gpt-a", "gpt-b"]);

            clear_recent_models().unwrap();
            let saved = load_model_config().unwrap();
            assert_eq!(saved.model.as_deref(), Some("gpt-a"));
            assert!(saved.recent_models.is_empty());

            unsafe { env::remove_var("OY_CONFIG") };
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
        fn session_file_ignores_legacy_mode_and_defaults_missing_fields() {
            let raw = r#"{
            "model": "gpt-test",
            "agent": "default",
            "mode": "auto-approve",
            "saved_at": "2026-01-01T00:00:00",
            "transcript": {"messages": []}
        }"#;
            let file: SessionFile = serde_json::from_str(raw).unwrap();
            assert_eq!(file.model, "gpt-test");
            assert!(file.todos.is_empty());
            assert!(file.workspace_root.is_none());
        }

        #[test]
        fn session_file_save_omits_mode() {
            let file = SessionFile {
                model: "gpt-test".into(),
                saved_at: "2026-01-01T00:00:00".into(),
                workspace_root: None,
                transcript: serde_json::json!({"messages": []}),
                todos: Vec::new(),
            };
            let raw = serde_json::to_value(&file).unwrap();
            assert!(raw.get("mode").is_none());
            assert!(raw.get("agent").is_none());
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
}

// === ui ===
pub(crate) mod ui {
    use std::borrow::Cow;
    use std::fmt::{Display, Write as _};
    use std::io::IsTerminal as _;
    use std::sync::LazyLock;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::time::Duration;
    use syntect::easy::HighlightLines;
    use syntect::highlighting::{Theme, ThemeSet};
    use syntect::parsing::SyntaxSet;
    use syntect::util::as_24_bit_terminal_escaped;
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    /// Controls how much user-facing output `oy` writes while it runs.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum OutputMode {
        /// Suppress normal progress output.
        Quiet = 0,
        /// Show standard human-readable progress output.
        Normal = 1,
        /// Show fuller tool previews and diagnostic context.
        Verbose = 2,
        /// Prefer machine-readable JSON where a command supports it.
        Json = 3,
    }

    static OUTPUT_MODE: AtomicU8 = AtomicU8::new(OutputMode::Normal as u8);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ColorMode {
        Auto,
        Always,
        Never,
    }

    static COLOR_MODE: LazyLock<ColorMode> = LazyLock::new(color_mode_from_env);

    pub fn init_output_mode(mode: Option<OutputMode>) {
        let mode = mode
            .or_else(output_mode_from_env)
            .unwrap_or(OutputMode::Normal);
        set_output_mode(mode);
    }

    /// Sets the process-wide output mode used by CLI rendering helpers.
    pub fn set_output_mode(mode: OutputMode) {
        OUTPUT_MODE.store(mode as u8, Ordering::Relaxed);
    }

    pub fn output_mode() -> OutputMode {
        match OUTPUT_MODE.load(Ordering::Relaxed) {
            0 => OutputMode::Quiet,
            2 => OutputMode::Verbose,
            3 => OutputMode::Json,
            _ => OutputMode::Normal,
        }
    }

    pub fn is_quiet() -> bool {
        matches!(output_mode(), OutputMode::Quiet | OutputMode::Json)
    }

    pub fn is_json() -> bool {
        matches!(output_mode(), OutputMode::Json)
    }

    pub fn is_verbose() -> bool {
        matches!(output_mode(), OutputMode::Verbose)
    }

    fn output_mode_from_env() -> Option<OutputMode> {
        if truthy_env("OY_QUIET") {
            return Some(OutputMode::Quiet);
        }
        if truthy_env("OY_VERBOSE") {
            return Some(OutputMode::Verbose);
        }
        match std::env::var("OY_OUTPUT")
            .ok()?
            .to_ascii_lowercase()
            .as_str()
        {
            "quiet" => Some(OutputMode::Quiet),
            "verbose" => Some(OutputMode::Verbose),
            "json" => Some(OutputMode::Json),
            "normal" => Some(OutputMode::Normal),
            _ => None,
        }
    }

    fn truthy_env(name: &str) -> bool {
        matches!(
            std::env::var(name).ok().as_deref(),
            Some("1" | "true" | "yes" | "on")
        )
    }

    fn color_mode_from_env() -> ColorMode {
        color_mode_from_values(
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var("OY_COLOR").ok().as_deref(),
        )
    }

    fn color_mode_from_values(no_color: bool, oy_color: Option<&str>) -> ColorMode {
        if no_color {
            return ColorMode::Never;
        }
        match oy_color.map(str::to_ascii_lowercase).as_deref() {
            Some("always" | "1" | "true" | "yes" | "on") => ColorMode::Always,
            Some("never" | "0" | "false" | "no" | "off") => ColorMode::Never,
            _ => ColorMode::Auto,
        }
    }

    pub fn color_enabled() -> bool {
        color_enabled_for_stdout(std::io::stdout().is_terminal())
    }

    fn color_enabled_for_stdout(stdout_is_terminal: bool) -> bool {
        color_enabled_for_mode(*COLOR_MODE, stdout_is_terminal)
    }

    fn color_enabled_for_mode(mode: ColorMode, stdout_is_terminal: bool) -> bool {
        match mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => stdout_is_terminal,
        }
    }

    pub fn terminal_width() -> usize {
        terminal_size::terminal_size()
            .map(|(terminal_size::Width(width), _)| width as usize)
            .filter(|width| *width >= 40)
            .unwrap_or(100)
    }

    pub fn paint(code: &str, text: impl Display) -> String {
        if color_enabled() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn faint(text: impl Display) -> String {
        paint("2", text)
    }

    pub fn bold(text: impl Display) -> String {
        paint("1", text)
    }

    pub fn cyan(text: impl Display) -> String {
        paint("36", text)
    }

    pub fn green(text: impl Display) -> String {
        paint("32", text)
    }

    pub fn yellow(text: impl Display) -> String {
        paint("33", text)
    }

    pub fn red(text: impl Display) -> String {
        paint("31", text)
    }

    pub fn magenta(text: impl Display) -> String {
        paint("35", text)
    }

    pub fn status_text(ok: bool, text: impl Display) -> String {
        if ok { green(text) } else { red(text) }
    }

    pub fn bool_text(value: bool) -> String {
        status_text(value, value)
    }

    pub fn path(text: impl Display) -> String {
        paint("1;36", text)
    }

    pub fn out(text: &str) {
        print!("{text}");
    }

    pub fn err(text: &str) {
        eprint!("{text}");
    }

    pub fn line(text: impl Display) {
        out(&format!("{text}\n"));
    }

    pub fn err_line(text: impl Display) {
        err(&format!("{text}\n"));
    }

    pub fn markdown(text: &str) {
        out(&render_markdown(text));
    }

    fn render_markdown(text: &str) -> String {
        if !color_enabled() {
            return text.to_string();
        }
        let mut in_fence = false;
        let mut out = String::new();
        for line in text.lines() {
            let trimmed = line.trim_start();
            let rendered = if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
                faint(line)
            } else if in_fence {
                cyan(line)
            } else if trimmed.starts_with('#') {
                paint("1;35", line)
            } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                cyan(line)
            } else {
                line.to_string()
            };
            let _ = writeln!(out, "{rendered}");
        }
        if text.ends_with('\n') {
            out
        } else {
            out.trim_end_matches('\n').to_string()
        }
    }

    pub fn code(path: &str, text: &str, first_line: usize) -> String {
        numbered_block(path, &normalize_code_preview_text(text), first_line)
    }

    pub fn text_block(title: &str, text: &str) -> String {
        numbered_block(title, text, 1)
    }

    pub fn block_title(title: &str) -> String {
        path(format_args!("── {title}"))
    }

    #[cfg(test)]
    fn numbered_line(line_number: usize, width: usize, text: &str) -> String {
        numbered_line_with_max_width(line_number, width, text, usize::MAX)
    }

    fn numbered_line_with_max_width(
        line_number: usize,
        width: usize,
        text: &str,
        max_width: usize,
    ) -> String {
        let text = normalize_code_preview_text(text);
        let prefix = format!(
            "{} {} ",
            faint(format_args!("{line_number:>width$}")),
            faint("│")
        );
        let available = max_width
            .saturating_sub(ansi_stripped_width(&prefix))
            .max(1);
        format!("{prefix}{}", truncate_width(&text, available))
    }

    fn normalize_code_preview_text(text: &str) -> Cow<'_, str> {
        const TAB_WIDTH: usize = 4;
        if !text.contains('\t') {
            return Cow::Borrowed(text);
        }

        let mut out = String::with_capacity(text.len());
        let mut column = 0usize;
        for ch in text.chars() {
            match ch {
                '\t' => {
                    let spaces = TAB_WIDTH - (column % TAB_WIDTH);
                    out.extend(std::iter::repeat_n(' ', spaces));
                    column += spaces;
                }
                '\n' | '\r' => {
                    out.push(ch);
                    column = 0;
                }
                _ => {
                    out.push(ch);
                    column += UnicodeWidthChar::width(ch).unwrap_or(0);
                }
            }
        }
        Cow::Owned(out)
    }

    fn numbered_block(title: &str, text: &str, first_line: usize) -> String {
        let title = if title.is_empty() { "text" } else { title };
        let line_count = text.lines().count().max(1);
        let width = first_line
            .saturating_add(line_count.saturating_sub(1))
            .max(1)
            .to_string()
            .len();
        let max_width = terminal_width().saturating_sub(4).max(40);
        let code_width = max_width.saturating_sub(width + 3).max(1);
        let mut out = String::new();
        let _ = writeln!(out, "{}", truncate_width(&block_title(title), max_width));
        if text.is_empty() {
            let _ = writeln!(
                out,
                "{}",
                numbered_line_with_max_width(first_line, width, "", max_width)
            );
        } else {
            let display_text = text
                .lines()
                .map(|line| truncate_width(line, code_width))
                .collect::<Vec<_>>()
                .join("\n");
            let highlighted = highlighted_block(title, &display_text);
            let lines = highlighted.as_deref().unwrap_or(&display_text).lines();
            for (idx, line) in lines.enumerate() {
                let _ = writeln!(
                    out,
                    "{}",
                    numbered_line_with_max_width(first_line + idx, width, line, max_width)
                );
            }
        }
        out.trim_end().to_string()
    }

    static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
    static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

    fn highlighted_block(title: &str, text: &str) -> Option<String> {
        if !color_enabled() {
            return None;
        }
        let syntax = syntax_for_title(title)?;
        let theme = terminal_theme()?;
        let mut highlighter = HighlightLines::new(syntax, theme);
        let mut out = String::new();
        for line in text.lines() {
            let ranges = highlighter.highlight_line(line, &SYNTAX_SET).ok()?;
            let _ = writeln!(out, "{}", as_24_bit_terminal_escaped(&ranges, false));
        }
        Some(if text.ends_with('\n') {
            out
        } else {
            out.trim_end_matches('\n').to_string()
        })
    }

    fn syntax_for_title(title: &str) -> Option<&'static syntect::parsing::SyntaxReference> {
        let syntaxes = &*SYNTAX_SET;
        let name = title.rsplit('/').next().unwrap_or(title);
        if let Some(ext) = name.rsplit_once('.').map(|(_, ext)| ext) {
            syntaxes.find_syntax_by_extension(ext)
        } else {
            syntaxes.find_syntax_by_token(name)
        }
        .or_else(|| syntaxes.find_syntax_by_name(title))
    }

    fn terminal_theme() -> Option<&'static Theme> {
        THEME_SET
            .themes
            .get("base16-ocean.dark")
            .or_else(|| THEME_SET.themes.values().next())
    }

    pub fn diff(text: &str) -> String {
        if !color_enabled() {
            return text.to_string();
        }
        let mut out = String::new();
        for line in text.lines() {
            let rendered = if line.starts_with("+++") || line.starts_with("---") {
                bold(line)
            } else if line.starts_with("@@") {
                cyan(line)
            } else if line.starts_with('+') {
                green(line)
            } else if line.starts_with('-') {
                red(line)
            } else {
                line.to_string()
            };
            let _ = writeln!(out, "{rendered}");
        }
        if text.ends_with('\n') {
            out
        } else {
            out.trim_end_matches('\n').to_string()
        }
    }

    pub fn section(title: &str) {
        line(bold(title));
    }

    pub fn kv(key: &str, value: impl Display) {
        line(format_args!(
            "  {} {value}",
            faint(format_args!("{key:<11}"))
        ));
    }

    pub fn success(text: impl Display) {
        line(format_args!("{} {text}", green("✓")));
    }

    pub fn warn(text: impl Display) {
        line(format_args!("{} {text}", yellow("!")));
    }

    pub fn progress(
        label: &str,
        current: usize,
        total: usize,
        detail: impl Display,
        elapsed: Duration,
    ) {
        if is_quiet() {
            return;
        }
        line(progress_line(
            label,
            current,
            total,
            &detail.to_string(),
            elapsed,
        ));
    }

    fn progress_line(
        label: &str,
        current: usize,
        total: usize,
        detail: &str,
        elapsed: Duration,
    ) -> String {
        let total = total.max(1);
        let current = current.min(total);
        let head = format!(
            "  {} {current}/{total} {}",
            progress_bar(current, total, 18),
            cyan(label)
        );
        if detail.trim().is_empty() {
            format!("{head} · {}", faint(format_duration(elapsed)))
        } else {
            format!("{head} · {detail} · {}", faint(format_duration(elapsed)))
        }
    }

    fn progress_bar(current: usize, total: usize, width: usize) -> String {
        let width = width.max(1);
        let total = total.max(1);
        let current = current.min(total);
        let filled = current.saturating_mul(width) / total;
        format!(
            "[{}{}]",
            green("█".repeat(filled)),
            faint("░".repeat(width.saturating_sub(filled)))
        )
    }

    pub fn tool_batch(round: usize, count: usize) {
        if is_quiet() {
            return;
        }
        err_line(tool_batch_line(round, count));
    }

    pub fn tool_start(name: &str, detail: &str) {
        if is_quiet() {
            return;
        }
        err_line(tool_start_line(name, detail));
    }

    pub fn tool_result(name: &str, elapsed: Duration, preview: &str) {
        if is_quiet() {
            return;
        }
        let preview = preview.trim_end();
        let head = tool_result_head(name, elapsed);
        let Some((first, rest)) = preview.split_once('\n') else {
            if preview.is_empty() {
                err_line(head);
            } else {
                err_line(format_args!("{head} · {first}", first = preview));
            }
            return;
        };
        err_line(format_args!("{head} · {first}"));
        for line in rest.lines() {
            err_line(format_args!("    {line}"));
        }
    }

    pub fn tool_error(name: &str, elapsed: Duration, err: impl Display) {
        if is_quiet() {
            return;
        }
        err_line(format_args!(
            "  {} {name} {} · {err:#}",
            red("✗"),
            format_duration(elapsed)
        ));
    }

    pub fn format_duration(elapsed: Duration) -> String {
        if elapsed.as_millis() < 1000 {
            format!("{}ms", elapsed.as_millis())
        } else {
            format!("{:.1}s", elapsed.as_secs_f64())
        }
    }

    fn tool_batch_line(round: usize, count: usize) -> String {
        format!("{} tools r{round} ×{count}", magenta("↻"))
    }

    fn tool_start_line(name: &str, detail: &str) -> String {
        if detail.is_empty() {
            format!("  {} {name}", cyan("→"))
        } else {
            format!("  {} {name} · {detail}", cyan("→"))
        }
    }

    fn tool_result_head(name: &str, elapsed: Duration) -> String {
        format!("  {} {name} {}", green("✓"), format_duration(elapsed))
    }

    pub fn compact_spaces(value: &str) -> String {
        value.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    pub fn truncate_chars(text: &str, max: usize) -> String {
        truncate_width(text, max)
    }

    pub fn truncate_width(text: &str, max_width: usize) -> String {
        if ansi_stripped_width(text) <= max_width {
            return text.to_string();
        }
        truncate_plain_width(text, max_width)
    }

    fn truncate_plain_width(text: &str, max_width: usize) -> String {
        if UnicodeWidthStr::width(text) <= max_width {
            return text.to_string();
        }
        let ellipsis = "…";
        let limit = max_width.saturating_sub(UnicodeWidthStr::width(ellipsis));
        let mut out = String::new();
        let mut width = 0usize;
        for ch in text.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + ch_width > limit {
                break;
            }
            width += ch_width;
            out.push(ch);
        }
        out.push_str(ellipsis);
        out
    }

    fn ansi_stripped_width(text: &str) -> usize {
        let mut width = 0usize;
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' && chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            } else {
                width += UnicodeWidthChar::width(ch).unwrap_or(0);
            }
        }
        width
    }

    pub fn compact_preview(text: &str, max: usize) -> String {
        truncate_width(&compact_spaces(text), max)
    }

    pub fn clamp_lines(text: &str, max_lines: usize, max_cols: usize) -> String {
        let mut out = String::new();
        let lines = text.lines().collect::<Vec<_>>();
        for line in lines.iter().take(max_lines) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&truncate_width(line, max_cols));
        }
        if lines.len() > max_lines {
            let _ = write!(out, "\n… {} more lines", lines.len() - max_lines);
        }
        out
    }

    #[allow(dead_code)]
    pub fn wrap_line(text: &str, indent: &str) -> String {
        let width = terminal_width().saturating_sub(indent.width()).max(20);
        textwrap::wrap(text, width)
            .into_iter()
            .map(|line| format!("{indent}{line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn head_tail(text: &str, max_chars: usize) -> (String, bool) {
        if text.chars().count() <= max_chars {
            return (text.to_string(), false);
        }
        let head_len = max_chars / 2;
        let tail_len = max_chars.saturating_sub(head_len);
        let head = text.chars().take(head_len).collect::<String>();
        let tail = text
            .chars()
            .rev()
            .take(tail_len)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>();
        let hidden = text
            .chars()
            .count()
            .saturating_sub(head.chars().count() + tail.chars().count());
        (
            format!("{head}\n… [truncated {hidden} chars] …\n{tail}"),
            true,
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn color_mode_name(mode: ColorMode) -> &'static str {
            match mode {
                ColorMode::Auto => "auto",
                ColorMode::Always => "always",
                ColorMode::Never => "never",
            }
        }

        #[test]
        fn color_mode_env_parsing() {
            assert_eq!(color_mode_name(color_mode_from_values(false, None)), "auto");
            assert_eq!(
                color_mode_name(color_mode_from_values(false, Some("always"))),
                "always"
            );
            assert_eq!(
                color_mode_name(color_mode_from_values(false, Some("on"))),
                "always"
            );
            assert_eq!(
                color_mode_name(color_mode_from_values(false, Some("off"))),
                "never"
            );
            assert_eq!(
                color_mode_name(color_mode_from_values(true, Some("always"))),
                "never"
            );
        }

        #[test]
        fn color_auto_requires_terminal() {
            assert!(!color_enabled_for_mode(ColorMode::Auto, false));
            assert!(color_enabled_for_mode(ColorMode::Auto, true));
            assert!(color_enabled_for_mode(ColorMode::Always, false));
            assert!(!color_enabled_for_mode(ColorMode::Never, true));
        }

        #[test]
        fn elapsed_format_is_compact() {
            assert_eq!(format_duration(Duration::from_millis(42)), "42ms");
            assert_eq!(format_duration(Duration::from_millis(1250)), "1.2s");
        }

        #[test]
        fn progress_line_shows_bar_count_detail_and_elapsed() {
            set_output_mode(OutputMode::Normal);
            assert_eq!(progress_bar(2, 4, 8), "[████░░░░]");
            assert_eq!(
                progress_line("review", 2, 4, "chunk 3", Duration::from_millis(1250)),
                "  [█████████░░░░░░░░░] 2/4 review · chunk 3 · 1.2s"
            );
        }

        #[test]
        fn tool_progress_lines_are_dense() {
            set_output_mode(OutputMode::Normal);
            assert_eq!(tool_batch_line(2, 3), "↻ tools r2 ×3");
            assert_eq!(
                tool_start_line("read", "path=src/main.rs"),
                "  → read · path=src/main.rs"
            );
            assert_eq!(
                tool_result_head("read", Duration::from_millis(42)),
                "  ✓ read 42ms"
            );
        }

        #[test]
        fn numbered_line_expands_tabs_to_stable_columns() {
            set_output_mode(OutputMode::Normal);
            assert_eq!(numbered_line(7, 1, "\tlet x = 1;"), "7 │     let x = 1;");
            assert_eq!(numbered_line(8, 1, "ab\tcd"), "8 │ ab  cd");
            assert_eq!(
                code("demo.rs", "\tfn main() {}\n\t\tprintln!(\"hi\");", 1),
                "── demo.rs\n1 │     fn main() {}\n2 │         println!(\"hi\");"
            );
        }

        #[test]
        fn numbered_line_clamps_long_read_lines_to_preview_width() {
            set_output_mode(OutputMode::Normal);
            let line = numbered_line_with_max_width(
                394,
                3,
                r#"        .filter(|line| !line.starts_with(&format!("> {}", prompts::AUDIT_TRANSPARENCY_PREFIX)))"#,
                40,
            );
            assert!(UnicodeWidthStr::width(line.as_str()) <= 40, "{line}");
            assert!(line.starts_with("394 │ "));
            assert!(line.ends_with('…'));
            assert!(!line.contains('\n'));
        }

        #[test]
        fn code_preview_lines_fit_tool_result_indent_width() {
            set_output_mode(OutputMode::Normal);
            let preview = code(
                "src/audit.rs",
                r#"pub(crate) fn with_transparency_line(report: &str, snippet: &str) -> String {
        .filter(|line| !line.starts_with(&format!("> {}", prompts::AUDIT_TRANSPARENCY_PREFIX)))"#,
                390,
            );
            let max_width = terminal_width().saturating_sub(4).max(40);
            for line in preview.lines() {
                assert!(
                    UnicodeWidthStr::width(line) <= max_width,
                    "line exceeded {max_width}: {line}"
                );
            }
        }
    }
}

// === chat ===
pub(crate) mod chat {
    use anyhow::{Context as _, Result};
    use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
    use std::fmt::Display;

    use reedline_repl_rs::reedline::{
        DefaultPrompt, DefaultPromptSegment, EditCommand, Emacs, FileBackedHistory, KeyCode,
        KeyModifiers, Reedline, ReedlineEvent, Signal, default_emacs_keybindings,
    };
    use std::path::PathBuf;

    use crate::config;
    use crate::model;
    use crate::session::{self, Session};

    const HISTORY_SIZE: usize = 10_000;
    const MAX_CONTEXT_RECOVERY_ATTEMPTS: usize = 3;

    fn chat_line_editor(history_path: PathBuf) -> Result<Reedline> {
        let mut keybindings = default_emacs_keybindings();
        keybindings.add_binding(KeyModifiers::NONE, KeyCode::Enter, ReedlineEvent::Submit);
        let insert_newline = ReedlineEvent::Edit(vec![EditCommand::InsertNewline]);
        keybindings.add_binding(KeyModifiers::SHIFT, KeyCode::Enter, insert_newline.clone());
        keybindings.add_binding(KeyModifiers::ALT, KeyCode::Enter, insert_newline);

        Ok(Reedline::create()
            .with_history(Box::new(FileBackedHistory::with_file(
                HISTORY_SIZE,
                history_path,
            )?))
            .with_edit_mode(Box::new(Emacs::new(keybindings)))
            .use_bracketed_paste(true))
    }

    pub async fn run_chat(session: &mut Session) -> Result<i32> {
        crate::ui::section("oy chat");
        crate::ui::kv("keys", "Enter sends · Alt/Shift+Enter newline · /? help");
        let history_path = history_path("chat")?;
        let mut line_editor = chat_line_editor(history_path.clone())?;
        let prompt = DefaultPrompt::new(
            DefaultPromptSegment::Basic("oy".to_string()),
            DefaultPromptSegment::Empty,
        );

        loop {
            let signal = match line_editor.read_line(&prompt) {
                Ok(signal) => signal,
                Err(err) if is_cursor_position_timeout(&err) => {
                    crate::ui::warn("terminal cursor position timed out; resetting prompt");
                    line_editor = chat_line_editor(history_path.clone())?;
                    continue;
                }
                Err(err) => return Err(err.into()),
            };

            match signal {
                Signal::Success(line) => {
                    line_editor.sync_history()?;
                    if !handle_chat_line(session, line.trim()).await? {
                        break;
                    }
                }
                Signal::CtrlD => break,
                Signal::CtrlC => {
                    line_editor.sync_history()?;
                    break;
                }
            }
        }
        prompt_update_todo_on_quit(session);
        Ok(0)
    }

    fn is_cursor_position_timeout(err: &impl Display) -> bool {
        let text = err.to_string();
        text.contains("cursor position") && text.contains("could not be read")
    }

    fn prompt_update_todo_on_quit(session: &Session) {
        if crate::config::can_prompt() && !session.todos.is_empty() {
            let active = session
                .todos
                .iter()
                .filter(|item| item.status != "done")
                .count();
            crate::ui::line(format_args!(
                "todo summary: {active}/{} active in memory; use the todo tool with persist=true to write TODO.md",
                session.todos.len()
            ));
        }
    }

    async fn handle_chat_line(session: &mut Session, line: &str) -> Result<bool> {
        if line.is_empty() {
            return Ok(true);
        }
        if let Some(command) = line.strip_prefix('/') {
            return handle_slash_command(session, command.trim()).await;
        }
        run_prompt_with_context_recovery(session, line).await?;
        Ok(true)
    }

    async fn handle_slash_command(session: &mut Session, command: &str) -> Result<bool> {
        let mut parts = command.split_whitespace();
        let raw_name = parts.next().unwrap_or_default();
        let name = normalize_chat_command(raw_name);
        match name {
            "" => Ok(true),
            "help" => {
                crate::ui::markdown(&format!("{}\n", chat_help_text()));
                Ok(true)
            }
            "tokens" => tokens_command(session),
            "compact" => compact_command(parts.next(), session).await,
            "model" => model_command(parts.next(), session).await,
            "thinking" => thinking_command(parts.next()),
            "debug" | "status" => status_command(session),
            "ask" => {
                let prompt = parts.collect::<Vec<_>>().join(" ");
                ask_command(session, &prompt).await
            }
            "save" => save_command(parts.next(), session),
            "load" => load_command(parts.next(), session),
            "undo" => undo_command(session),
            "clear" => clear_command(session),
            "quit" | "exit" => Ok(false),
            other => {
                crate::ui::warn(format_args!("unknown command /{other}"));
                Ok(true)
            }
        }
    }

    fn normalize_chat_command(command: &str) -> &str {
        match command {
            "h" | "?" => "help",
            "t" => "tokens",
            "k" => "compact",
            "m" => "model",
            "d" => "debug",
            "s" => "status",
            "u" => "undo",
            "c" => "clear",
            "q" => "quit",
            other => other,
        }
    }

    pub(crate) fn chat_help_text() -> String {
        [
            "Enter sends; Alt/Shift+Enter inserts newline",
            "/help (/h, /?) -- show help",
            "/status (/s), /debug (/d) -- show model, mode, context, and todos",
            "/model [value] (/m) -- show or switch model",
            "/ask <question> -- research-only query",
            "/save [name], /load [name] -- save or load a session",
            "/undo (/u), /clear (/c) -- repair conversation state",
            "/quit (/q), /exit -- end session",
            "Advanced: /tokens, /compact [llm|deterministic], /thinking [auto|off|low|medium|high]",
        ]
        .join("\n")
    }

    async fn ask_command(session: &mut Session, prompt: &str) -> Result<bool> {
        if prompt.is_empty() {
            anyhow::bail!("Usage: /ask <question>");
        }
        let answer =
            session::run_prompt_read_only(session, &config::ask_system_prompt(prompt)).await?;
        if !answer.is_empty() {
            crate::ui::markdown(&format!("{answer}\n"));
        }
        Ok(true)
    }

    fn tokens_command(session: &Session) -> Result<bool> {
        let status = session.context_status();
        crate::ui::section("Context");
        crate::ui::kv("messages", status.estimate.messages);
        crate::ui::kv(
            "system",
            format_args!("~{} tokens", status.estimate.system_tokens),
        );
        crate::ui::kv(
            "messages",
            format_args!("~{} tokens", status.estimate.message_tokens),
        );
        crate::ui::kv(
            "total",
            format_args!("~{} tokens", status.estimate.total_tokens),
        );
        crate::ui::kv("limit", format_args!("{} tokens", status.limit_tokens));
        crate::ui::kv(
            "input budget",
            format_args!("{} tokens", status.input_budget_tokens),
        );
        crate::ui::kv("trigger", format_args!("{} tokens", status.trigger_tokens));
        crate::ui::kv("summary", crate::ui::bool_text(status.summary_present));
        Ok(true)
    }

    async fn compact_command(mode: Option<&str>, session: &mut Session) -> Result<bool> {
        let before = session.context_status().estimate.total_tokens;
        let stats = match mode.unwrap_or("llm") {
            "" | "llm" | "smart" => session.compact_llm().await?,
            "deterministic" | "det" | "fast" => session.compact_deterministic(),
            other => anyhow::bail!("compact mode must be llm or deterministic; got {other}"),
        };
        let after = session.context_status().estimate.total_tokens;
        crate::ui::section("Compaction");
        if let Some(stats) = stats {
            crate::ui::kv(
                "tokens",
                format_args!("{} -> {}", stats.before_tokens, stats.after_tokens),
            );
            crate::ui::kv("removed messages", stats.removed_messages);
            crate::ui::kv("tool outputs", stats.compacted_tools);
            crate::ui::kv("summarized", stats.summarized);
        } else {
            crate::ui::kv("tokens", format_args!("{before} -> {after}"));
            crate::ui::line("nothing to compact");
        }
        Ok(true)
    }

    async fn model_command(value: Option<&str>, session: &mut Session) -> Result<bool> {
        if let Some(value) = value {
            save_selected_model(value, session)?;
            crate::ui::line(format_args!("model: {}", session.model));
            return Ok(true);
        }

        match choose_recent_model(Some(&session.model), &config::recent_models()?)? {
            RecentModelChoice::Selected(model_spec) => {
                save_selected_model(&model_spec, session)?;
            }
            RecentModelChoice::Clear => {
                config::clear_recent_models()?;
                crate::ui::success("cleared recent model history");
            }
            RecentModelChoice::Inspect => {
                let listing = model::inspect_models().await?;
                print_chat_model_listing(&listing);
                if let Some(chosen) = choose_model_from_items(
                    listing.current.as_deref(),
                    &listing.all_models,
                    "Models",
                )? {
                    save_selected_model(&chosen, session)?;
                }
            }
            RecentModelChoice::Cancelled => {}
        }
        crate::ui::line(format_args!("model: {}", session.model));
        Ok(true)
    }

    fn save_selected_model(model_spec: &str, session: &mut Session) -> Result<()> {
        config::save_model_config(model_spec)?;
        session.model = model::resolve_model(Some(model_spec))?;
        Ok(())
    }

    fn print_chat_model_listing(listing: &model::ModelListing) {
        crate::ui::section("Models");
        crate::ui::kv("current", listing.current.as_deref().unwrap_or("<unset>"));
        crate::ui::kv("selectable", listing.all_models.len());
        if listing.all_models.is_empty() {
            crate::ui::warn("no models found from configured endpoints");
        }
    }

    fn thinking_command(value: Option<&str>) -> Result<bool> {
        if let Some(value) = value {
            match value {
                "" | "auto" => unsafe { std::env::remove_var("OY_THINKING") },
                "off" | "none" => unsafe { std::env::set_var("OY_THINKING", "none") },
                "minimal" | "low" | "medium" | "high" => unsafe {
                    std::env::set_var("OY_THINKING", value)
                },
                other => anyhow::bail!(
                    "thinking must be auto, off, minimal, low, medium, or high; got {other}"
                ),
            }
        }
        crate::ui::line(format_args!(
            "thinking: {}",
            std::env::var("OY_THINKING").unwrap_or_else(|_| "auto".to_string())
        ));
        Ok(true)
    }

    fn status_command(session: &Session) -> Result<bool> {
        crate::ui::section("Status");
        crate::ui::kv("workspace", session.root.display());
        crate::ui::kv("model", &session.model);
        crate::ui::kv("genai", model::to_genai_model_spec(&session.model));
        crate::ui::kv(
            "thinking",
            model::default_reasoning_effort(&session.model).unwrap_or("auto/off"),
        );
        crate::ui::kv("mode", &session.mode);
        crate::ui::kv("interactive", crate::ui::bool_text(session.interactive));
        crate::ui::kv(
            "files-write",
            format_args!("{:?}", session.policy.files_write),
        );
        crate::ui::kv("shell", format_args!("{:?}", session.policy.shell));
        crate::ui::kv("network", crate::ui::bool_text(session.policy.network));
        crate::ui::kv("risk", config::policy_risk_label(&session.policy));
        crate::ui::kv("messages", session.transcript.messages.len());
        crate::ui::kv("todos", session.todos.len());
        let status = session.context_status();
        crate::ui::kv(
            "context",
            format_args!(
                "~{} / {} tokens",
                status.estimate.total_tokens, status.input_budget_tokens
            ),
        );
        crate::ui::kv("summary", crate::ui::bool_text(status.summary_present));
        Ok(true)
    }

    fn save_command(name: Option<&str>, session: &mut Session) -> Result<bool> {
        let path = session.save(name)?;
        crate::ui::success(format_args!("saved session {}", path.display()));
        Ok(true)
    }

    fn load_command(name: Option<&str>, session: &mut Session) -> Result<bool> {
        if let Some(new_session) =
            session::load_saved(name, true, session.mode.clone(), session.policy)?
        {
            *session = new_session;
            crate::ui::success("loaded session");
        } else {
            crate::ui::warn("no saved sessions found");
        }
        Ok(true)
    }

    fn undo_command(session: &mut Session) -> Result<bool> {
        if session.transcript.undo_last_turn() {
            crate::ui::success("undid last turn");
        } else {
            crate::ui::warn("nothing to undo");
        }
        Ok(true)
    }

    fn clear_command(session: &mut Session) -> Result<bool> {
        session.transcript.messages.clear();
        crate::ui::success("conversation cleared");
        Ok(true)
    }

    async fn run_prompt_with_context_recovery(session: &mut Session, prompt: &str) -> Result<()> {
        let mut recovery_attempts = 0usize;
        loop {
            match session::run_prompt(session, prompt).await {
                Ok(answer) => {
                    if !answer.is_empty() {
                        crate::ui::markdown(&format!("{answer}\n"));
                    }
                    return Ok(());
                }
                Err(err) => {
                    let Some(budget_err) = err
                        .downcast_ref::<session::ContextBudgetExceeded>()
                        .copied()
                    else {
                        return Err(err);
                    };
                    recovery_attempts += 1;
                    crate::ui::err_line(format_args!("model call failed: {err:#}"));
                    session.transcript.undo_last_turn();
                    if recovery_attempts >= MAX_CONTEXT_RECOVERY_ATTEMPTS {
                        offer_save_after_context_failures(session)?;
                        return Ok(());
                    }
                    if !recover_context_budget(session, recovery_attempts, budget_err)? {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn recover_context_budget(
        session: &mut Session,
        attempt: usize,
        budget_err: session::ContextBudgetExceeded,
    ) -> Result<bool> {
        if config::can_prompt() {
            let raised_limit =
                config::context_config().input_budget_tokens() >= budget_err.estimated_tokens;
            let choices = vec![
                format!(
                    "Retry with current OY_CONTEXT_LIMIT={}{}",
                    config::context_config().limit_tokens,
                    if raised_limit {
                        " (now sufficient)"
                    } else {
                        ""
                    }
                ),
                "Force-truncate oldest history and retry".to_string(),
                "Save session and stop".to_string(),
                "Stop without saving".to_string(),
            ];
            let choice = ask("Context is over budget. Choose recovery", Some(&choices))?;
            if choice.starts_with("Retry with current OY_CONTEXT_LIMIT=") {
                return Ok(true);
            }
            match choice.as_str() {
                "Force-truncate oldest history and retry" => {}
                "Save session and stop" => {
                    let path = session.save(None)?;
                    crate::ui::success(format_args!("saved session {}", path.display()));
                    crate::ui::line(
                        "Try `/load` later, or switch models with `/model` after reloading.",
                    );
                    return Ok(false);
                }
                _ => return Ok(false),
            }
        }

        let before = session.context_status().estimate.total_tokens;
        let removed = session.transcript.force_truncate_oldest_turns();
        let after = session.context_status().estimate.total_tokens;
        if removed == 0 || after >= before {
            if attempt + 1 >= MAX_CONTEXT_RECOVERY_ATTEMPTS {
                offer_save_after_context_failures(session)?;
                return Ok(false);
            }
            anyhow::bail!(
                "context remains over budget and no more history can be truncated; save the session and try a different model later"
            );
        }
        crate::ui::warn(format_args!(
            "force-truncated {removed} old messages: {before} -> {after} tokens"
        ));
        Ok(true)
    }

    fn offer_save_after_context_failures(session: &Session) -> Result<()> {
        crate::ui::warn(format_args!(
            "context is still over budget after {MAX_CONTEXT_RECOVERY_ATTEMPTS} recovery attempts"
        ));
        if config::can_prompt()
            && Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt("Save this session so you can resume later?")
                .default(true)
                .interact()?
        {
            let path = session
                .save(None)
                .context("failed to save over-budget session")?;
            crate::ui::success(format_args!("saved session {}", path.display()));
        }
        crate::ui::line(
            "Try `/load` later, then raise OY_CONTEXT_LIMIT, use `/compact`, or switch models with `/model`.",
        );
        Ok(())
    }

    pub fn choose_model(current: Option<&str>, items: &[String]) -> Result<Option<String>> {
        choose_model_with_initial_list(current, items, true)
    }

    pub fn choose_recent_model(
        current: Option<&str>,
        recent: &[String],
    ) -> Result<RecentModelChoice> {
        if recent.len() < 2 || !config::can_prompt() {
            return Ok(RecentModelChoice::Inspect);
        }
        let items = recent_model_menu_items(recent);
        let default = current
            .and_then(|value| recent.iter().position(|item| item == value))
            .unwrap_or(0);
        let choice = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Recent models")
            .items(&items)
            .default(default)
            .interact_opt()?;
        Ok(match choice {
            Some(index) if index < recent.len() => {
                RecentModelChoice::Selected(recent[index].clone())
            }
            Some(index) if index == recent.len() => RecentModelChoice::Inspect,
            Some(_) => RecentModelChoice::Clear,
            None => RecentModelChoice::Cancelled,
        })
    }

    fn recent_model_menu_items(recent: &[String]) -> Vec<String> {
        recent
            .iter()
            .cloned()
            .chain([
                "Inspect all models…".to_string(),
                "Clear recent model history".to_string(),
            ])
            .collect()
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum RecentModelChoice {
        Selected(String),
        Inspect,
        Clear,
        Cancelled,
    }

    pub fn choose_model_with_initial_list(
        current: Option<&str>,
        items: &[String],
        _print_initial_list: bool,
    ) -> Result<Option<String>> {
        if items.is_empty() || !config::can_prompt() {
            return Ok(None);
        }
        choose_model_from_items(current, items, "Models")
    }

    pub fn choose_model_from_items(
        current: Option<&str>,
        items: &[String],
        label: &str,
    ) -> Result<Option<String>> {
        if items.is_empty() || !config::can_prompt() {
            return Ok(None);
        }
        let theme = ColorfulTheme::default();
        let default = current.and_then(|value| items.iter().position(|item| item == value));
        let mut prompt = Select::with_theme(&theme)
            .with_prompt(label)
            .items(items)
            .default(default.unwrap_or(0));
        if current.is_some() {
            prompt = prompt.with_prompt(format!("{label} (Esc keeps current)"));
        }
        Ok(prompt.interact_opt()?.map(|index| items[index].clone()))
    }

    pub fn ask(question: &str, choices: Option<&[String]>) -> Result<String> {
        if let Some(choices) = choices {
            if choices.is_empty() {
                return Ok(String::new());
            }
            let index = Select::with_theme(&ColorfulTheme::default())
                .with_prompt(question)
                .items(choices)
                .default(0)
                .interact_opt()?;
            return Ok(index
                .map(|index| choices[index].clone())
                .unwrap_or_default());
        }
        Ok(Input::<String>::with_theme(&ColorfulTheme::default())
            .with_prompt(question)
            .interact_text()?)
    }

    fn history_path(name: &str) -> Result<PathBuf> {
        history_path_in(config::config_dir_path(), name)
    }

    fn history_path_in(config_dir: PathBuf, name: &str) -> Result<PathBuf> {
        let history = config_dir.join("history");
        config::create_private_dir_all(&history)?;
        let path = history.join(format!("{name}.txt"));
        if !path.exists() {
            config::write_private_file(&path, b"")?;
        }
        Ok(path)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn history_path_uses_named_private_history_file() {
            let dir = tempfile::tempdir().unwrap();
            let path = history_path_in(dir.path().to_path_buf(), "chat").unwrap();
            assert!(path.ends_with("history/chat.txt"));
            assert!(path.exists());

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let history_dir_mode = std::fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
                assert_eq!(history_dir_mode, 0o700);
                assert_eq!(file_mode, 0o600);
            }
        }

        #[test]
        fn normalize_chat_command_maps_slash_aliases() {
            assert_eq!(normalize_chat_command("q"), "quit");
            assert_eq!(normalize_chat_command("tokens"), "tokens");
            assert_eq!(normalize_chat_command("k"), "compact");
            assert_eq!(normalize_chat_command("s"), "status");
        }

        #[test]
        fn chat_help_uses_slash_commands() {
            let help = chat_help_text();
            assert!(help.contains("/help"));
            assert!(help.contains("/quit"));
            assert!(help.contains("/compact"));
            assert!(help.contains("/status"));
        }

        #[test]
        fn recent_model_menu_appends_inspect_and_clear_actions() {
            let items = recent_model_menu_items(&["gpt-a".to_string(), "gpt-b".to_string()]);
            assert_eq!(
                items,
                vec![
                    "gpt-a",
                    "gpt-b",
                    "Inspect all models…",
                    "Clear recent model history"
                ]
            );
        }
    }
}

// === app ===
pub(crate) mod app {
    use anyhow::{Result, bail};
    use clap::{Args, Parser, Subcommand, ValueEnum};
    use std::io::IsTerminal as _;
    use std::path::{Path, PathBuf};

    use crate::audit;
    use crate::config;
    use crate::model;
    use crate::session::{self, Session};

    const MODEL_LIST_LIMIT: usize = 30;

    #[derive(Debug, Parser)]
    #[command(
        name = "oy",
        version,
        about = "Small local AI coding assistant for your shell.",
        after_help = "Examples:\n  oy doctor\n  oy model\n  oy \"inspect this repo and summarize risks\"\n  oy chat --mode plan\n  oy run --out plan.md \"write a migration plan\"\n\nSafety: file tools stay inside the workspace, but oy is not a sandbox. Use --mode plan or a container/VM for untrusted repos."
    )]
    struct Cli {
        #[arg(long, global = true, conflicts_with_all = ["verbose", "json"], help = "Suppress normal progress output")]
        quiet: bool,
        #[arg(long, global = true, conflicts_with_all = ["quiet", "json"], help = "Show fuller tool previews")]
        verbose: bool,
        #[arg(long, global = true, conflicts_with_all = ["quiet", "verbose"], help = "Print machine-readable JSON where supported")]
        json: bool,
        #[command(subcommand)]
        command: Option<Command>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
    enum AuditFormat {
        Markdown,
        Sarif,
    }

    impl From<AuditFormat> for audit::AuditOutputFormat {
        fn from(format: AuditFormat) -> Self {
            match format {
                AuditFormat::Markdown => Self::Markdown,
                AuditFormat::Sarif => Self::Sarif,
            }
        }
    }

    #[derive(Debug, Subcommand)]
    enum Command {
        /// Run one task in the current workspace; prompt can be args or stdin.
        Run(RunArgs),
        /// Start an interactive chat session with slash commands and history.
        Chat(ChatArgs),
        /// List, choose, and save model ids/routing shims.
        Model(ModelArgs),
        /// Check setup, auth, paths, and safety-relevant defaults.
        Doctor(DoctorArgs),
        /// Audit the current workspace and write findings.
        Audit {
            #[arg(
                long,
                value_enum,
                default_value_t = AuditFormat::Markdown,
                help = "Output format: markdown or sarif"
            )]
            format: AuditFormat,
            #[arg(
                long,
                value_name = "PATH",
                help = "Write findings to a workspace file (default: ISSUES.md or oy.sarif)"
            )]
            out: Option<PathBuf>,
            #[arg(
                long,
                value_name = "N",
                default_value_t = audit::DEFAULT_MAX_REVIEW_CHUNKS,
                help = "Maximum audit chunks to review before failing closed"
            )]
            max_chunks: usize,
            #[arg(value_name = "FOCUS", help = "Optional audit focus text")]
            focus: Vec<String>,
        },
    }

    #[derive(Debug, Args, Clone)]
    struct SharedModeArgs {
        #[arg(
            long,
            alias = "agent",
            default_value = "default",
            help = "Safety mode (default: balanced): plan, ask, edit, or auto"
        )]
        mode: String,
        #[arg(
            long = "continue-session",
            default_value_t = false,
            help = "Resume the most recent saved session"
        )]
        continue_session: bool,
        #[arg(
            long,
            default_value = "",
            value_name = "NAME_OR_NUMBER",
            help = "Resume a named or numbered saved session"
        )]
        resume: String,
    }

    #[derive(Debug, Args, Clone)]
    struct RunArgs {
        #[command(flatten)]
        shared: SharedModeArgs,
        #[arg(
            long,
            value_name = "PATH",
            help = "Write the final answer to a workspace file"
        )]
        out: Option<PathBuf>,
        #[arg(
            value_name = "PROMPT",
            help = "Task prompt; omitted means read stdin or start chat in a TTY"
        )]
        task: Vec<String>,
    }

    #[derive(Debug, Args, Clone)]
    struct ChatArgs {
        #[command(flatten)]
        shared: SharedModeArgs,
    }

    #[derive(Debug, Args, Clone)]
    struct ModelArgs {
        #[arg(
            value_name = "MODEL",
            help = "Model id or routing shim selection from `oy model`, e.g. copilot::<model-id>"
        )]
        model: Option<String>,
    }

    #[derive(Debug, Args, Clone)]
    struct DoctorArgs {
        #[arg(
            long,
            alias = "agent",
            default_value = "default",
            help = "Safety mode to inspect (default: balanced): plan, ask, edit, or auto"
        )]
        mode: String,
    }

    pub async fn run(argv: Vec<String>) -> Result<i32> {
        let normalized = normalize_args(argv);
        let mut cli = Cli::parse_from(std::iter::once("oy".to_string()).chain(normalized.clone()));
        restore_trailing_audit_options(&mut cli);
        crate::ui::init_output_mode(cli_output_mode(&cli));
        match cli.command.unwrap_or(Command::Run(RunArgs {
            shared: SharedModeArgs {
                mode: "default".to_string(),
                continue_session: false,
                resume: String::new(),
            },
            out: None,
            task: Vec::new(),
        })) {
            Command::Run(args) => run_command(args).await,
            Command::Chat(args) => chat_command(args).await,
            Command::Model(args) => model_command(args).await,
            Command::Doctor(args) => doctor_command(args).await,
            Command::Audit {
                format,
                out,
                max_chunks,
                focus,
            } => {
                audit_command(AuditArgs {
                    focus,
                    out: out.unwrap_or_else(|| audit::default_output_path(format.into())),
                    max_chunks,
                    format: format.into(),
                })
                .await
            }
        }
    }

    fn restore_trailing_audit_options(cli: &mut Cli) {
        let Some(Command::Audit {
            format: _,
            out: _,
            max_chunks,
            focus,
        }) = &mut cli.command
        else {
            return;
        };
        let mut filtered_focus = Vec::new();
        let mut i = 0usize;
        while i < focus.len() {
            match focus[i].as_str() {
                "--max-chunks" => {
                    if let Some(value) = focus.get(i + 1)
                        && let Ok(parsed) = value.parse::<usize>()
                    {
                        *max_chunks = parsed;
                        i += 2;
                        continue;
                    }
                }
                raw if raw.starts_with("--max-chunks=") => {
                    if let Some((_, value)) = raw.split_once('=')
                        && let Ok(parsed) = value.parse::<usize>()
                    {
                        *max_chunks = parsed;
                        i += 1;
                        continue;
                    }
                }
                _ => {}
            }
            filtered_focus.push(focus[i].clone());
            i += 1;
        }
        *focus = filtered_focus;
    }

    fn cli_output_mode(cli: &Cli) -> Option<crate::ui::OutputMode> {
        if cli.quiet {
            Some(crate::ui::OutputMode::Quiet)
        } else if cli.verbose {
            Some(crate::ui::OutputMode::Verbose)
        } else if cli.json {
            Some(crate::ui::OutputMode::Json)
        } else {
            None
        }
    }

    #[cfg(test)]
    fn parse_cli_for_test(args: &[&str]) -> Cli {
        let mut cli = Cli::parse_from(args);
        restore_trailing_audit_options(&mut cli);
        cli
    }

    #[cfg(test)]
    fn command_help_for_test(command: &str) -> String {
        let mut cmd = <Cli as clap::CommandFactory>::command();
        let Some(subcommand) = cmd.find_subcommand_mut(command) else {
            panic!("unknown command: {command}");
        };
        let mut help = Vec::new();
        subcommand.write_long_help(&mut help).expect("write help");
        String::from_utf8(help).expect("utf8 help")
    }

    fn normalize_args(mut args: Vec<String>) -> Vec<String> {
        if args.is_empty() {
            return if config::can_prompt() {
                vec!["--help".to_string()]
            } else {
                vec!["run".to_string()]
            };
        }
        if matches!(
            args.first().map(String::as_str),
            Some("--continue") | Some("-c")
        ) {
            return std::iter::once("run".to_string())
                .chain(std::iter::once("--continue-session".to_string()))
                .chain(args.drain(1..))
                .collect();
        }
        if args.first().map(String::as_str) == Some("--resume") {
            return std::iter::once("run".to_string()).chain(args).collect();
        }
        let commands = ["run", "chat", "model", "doctor", "audit", "-h", "--help"];
        if args
            .first()
            .is_some_and(|arg| !arg.starts_with('-') && !commands.contains(&arg.as_str()))
        {
            let mut out = vec!["run".to_string()];
            out.extend(args);
            return out;
        }
        args
    }

    async fn run_command(args: RunArgs) -> Result<i32> {
        let task = collect_task(&args.task)?;
        if task.trim().is_empty() {
            return chat_command(ChatArgs {
                shared: args.shared,
            })
            .await;
        }
        let mut session = load_or_new(
            false,
            &args.shared.mode,
            args.shared.continue_session,
            &args.shared.resume,
        )?;
        print_session_intro("run", &session, Some(&task));
        let answer = session::run_prompt(&mut session, &task).await?;
        if crate::ui::is_json() {
            print_run_json(&session, &answer)?;
        } else if let Some(path) = args.out {
            write_workspace_file(&session.root, &path, &answer)?;
            crate::ui::success(format_args!("wrote {}", path.display()));
        } else if !answer.is_empty() {
            crate::ui::markdown(&format!("{answer}\n"));
        }
        Ok(0)
    }

    fn print_run_json(session: &Session, answer: &str) -> Result<()> {
        let status = session.context_status();
        let payload = serde_json::json!({
            "answer": answer,
            "model": session.model,
            "mode": session.mode,
            "workspace": session.root,
            "tokens": status.estimate,
            "context": status,
            "messages": status.estimate.messages,
            "todos": session.todos,
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
        Ok(())
    }

    async fn chat_command(args: ChatArgs) -> Result<i32> {
        let mut session = load_or_new(
            true,
            &args.shared.mode,
            args.shared.continue_session,
            &args.shared.resume,
        )?;
        print_session_intro("chat", &session, None);
        crate::chat::run_chat(&mut session).await
    }

    async fn model_command(args: ModelArgs) -> Result<i32> {
        if let Some(model_spec) = args
            .model
            .as_deref()
            .filter(|value| is_exact_model_spec(value))
        {
            let normalized = model::canonical_model_spec(model_spec);
            config::save_model_config(&normalized)?;
            if crate::ui::is_json() {
                print_saved_model_json(&normalized)?;
            } else {
                print_saved_model(&normalized);
            }
            return Ok(0);
        }

        if args.model.is_none() && !crate::ui::is_json() {
            let current = model::resolve_model(None).ok();
            match crate::chat::choose_recent_model(current.as_deref(), &config::recent_models()?)? {
                crate::chat::RecentModelChoice::Selected(model_spec) => {
                    config::save_model_config(&model_spec)?;
                    print_saved_model(&model_spec);
                    return Ok(0);
                }
                crate::chat::RecentModelChoice::Clear => {
                    config::clear_recent_models()?;
                    crate::ui::success("cleared recent model history");
                    return Ok(0);
                }
                crate::chat::RecentModelChoice::Cancelled => return Ok(0),
                crate::chat::RecentModelChoice::Inspect => {}
            }
        }

        let listing = model::inspect_models().await?;
        if let Some(model_spec) = args.model {
            let normalized = resolve_model_choice(&listing, &model_spec)?;
            config::save_model_config(&normalized)?;
            if crate::ui::is_json() {
                print_model_json(&listing, Some(&normalized))?;
            } else {
                print_saved_model(&normalized);
            }
            return Ok(0);
        }
        if crate::ui::is_json() {
            print_model_json(&listing, None)?;
            return Ok(0);
        }
        print_model_listing(&listing);
        if config::can_prompt()
            && !listing.all_models.is_empty()
            && let Some(chosen) = crate::chat::choose_model_with_initial_list(
                listing.current.as_deref(),
                &listing.all_models,
                false,
            )?
        {
            config::save_model_config(&chosen)?;
            print_saved_model(&chosen);
        }
        Ok(0)
    }

    fn is_exact_model_spec(value: &str) -> bool {
        let value = value.trim();
        value.contains("::") || value.contains('/') || value.contains(':') || value.contains('.')
    }

    fn print_saved_model_json(saved: &str) -> Result<()> {
        let payload = serde_json::json!({ "saved": saved });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
        Ok(())
    }

    fn print_model_json(listing: &model::ModelListing, saved: Option<&str>) -> Result<()> {
        let payload = serde_json::json!({
            "current": listing.current,
            "current_shim": listing.current_shim,
            "saved": saved,
            "recent_models": config::recent_models()?,
            "auth": listing.auth,
            "dynamic": listing.dynamic,
            "all_models": listing.all_models,
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
        Ok(())
    }

    fn print_model_listing(listing: &model::ModelListing) {
        crate::ui::section("Models");
        crate::ui::kv(
            "current",
            current_model_text(
                listing.current.as_deref().unwrap_or("<unset>"),
                listing.current_shim.as_deref(),
            ),
        );
        crate::ui::kv("selectable", listing.all_models.len());
        if let Ok(recent) = config::recent_models()
            && !recent.is_empty()
        {
            crate::ui::line("");
            crate::ui::section("Recent models");
            for model in recent {
                let marker = if listing.current.as_deref() == Some(model.as_str()) {
                    "*"
                } else {
                    " "
                };
                crate::ui::line(format_args!("    {marker} {model}"));
            }
        }
        if !listing.auth.is_empty() {
            crate::ui::line("");
            crate::ui::section("Auth / shims");
            for item in &listing.auth {
                let env_var = item.env_var.as_deref().unwrap_or("-");
                let active = if listing.current_shim.as_deref() == Some(item.adapter.as_str()) {
                    " *"
                } else {
                    ""
                };
                crate::ui::line(format_args!(
                    "  {}{}  {} ({})",
                    item.adapter, active, env_var, item.source
                ));
                crate::ui::line(format_args!("    {}", item.detail));
            }
        }

        crate::ui::line("");
        crate::ui::section("Introspected endpoint models");
        if listing.dynamic.is_empty() {
            crate::ui::line("  none found from configured OpenAI-compatible endpoints");
        } else {
            for item in &listing.dynamic {
                if !item.ok {
                    crate::ui::line(format_args!(
                        "  {}  failed via {}",
                        item.adapter, item.source
                    ));
                    if let Some(error) = item.error.as_deref() {
                        crate::ui::line(format_args!(
                            "    {}",
                            crate::ui::truncate_chars(error, 140)
                        ));
                    }
                    continue;
                }
                crate::ui::line(format_args!(
                    "  {}  {} models via {}",
                    item.adapter, item.count, item.source
                ));
                for model_name in item.models.iter().take(MODEL_LIST_LIMIT) {
                    let marker = if listing.current.as_deref() == Some(model_name.as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    crate::ui::line(format_args!("    {marker} {model_name}"));
                }
                if item.models.len() > MODEL_LIST_LIMIT {
                    crate::ui::line(format_args!(
                        "    … {} more; use `oy model <filter>` or interactive selection",
                        item.models.len() - MODEL_LIST_LIMIT
                    ));
                }
            }
        }
    }

    fn current_model_text(model_spec: &str, shim: Option<&str>) -> String {
        match shim.filter(|value| !value.is_empty()) {
            Some(shim) => format!("{model_spec} (shim: {shim})"),
            None => model_spec.to_string(),
        }
    }

    fn print_saved_model(selection: &str) {
        let saved = config::saved_model_config_from_selection(selection);
        crate::ui::success(format_args!(
            "saved model {}",
            saved.model.as_deref().unwrap_or(selection)
        ));
        if let Some(shim) = saved.shim {
            crate::ui::kv("shim", shim);
        }
    }

    fn resolve_model_choice(listing: &model::ModelListing, query: &str) -> Result<String> {
        let normalized = model::canonical_model_spec(query);
        if listing.all_models.iter().any(|item| item == &normalized) {
            return Ok(normalized);
        }
        if !config::can_prompt() {
            bail!(
                "No exact model match for `{}`. Re-run in a TTY to choose interactively.",
                query
            );
        }
        let matches = listing
            .all_models
            .iter()
            .filter(|item| {
                item.to_ascii_lowercase()
                    .contains(&query.to_ascii_lowercase())
            })
            .cloned()
            .collect::<Vec<_>>();
        if matches.is_empty() {
            bail!("No matching model for `{}`", query);
        }
        crate::chat::choose_model(listing.current.as_deref(), &matches)
            .map(|value| value.unwrap_or(normalized))
    }

    async fn doctor_command(args: DoctorArgs) -> Result<i32> {
        let root = config::oy_root()?;
        let listing = model::inspect_models().await?;
        let mode = config::safety_mode(&args.mode)?;
        let policy = config::tool_policy(mode.name());
        let config_file = config::config_root();
        let config_dir = config::config_dir_path();
        let sessions_dir = config::sessions_dir().unwrap_or_else(|_| config_dir.join("sessions"));
        let history_dir = config_dir.join("history");
        let bash_ok = std::process::Command::new("bash")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);

        if crate::ui::is_json() {
            let payload = serde_json::json!({
                "workspace": root,
                "model": listing.current,
                "shim": listing.current_shim,
                "recent_models": config::recent_models()?,
                "auth": listing.auth,
                "mode": mode.name(),
                "policy": policy,
                "interactive": config::can_prompt(),
                "non_interactive": config::non_interactive(),
                "config_file": config_file,
                "config_dir": config_dir,
                "sessions_dir": sessions_dir,
                "history_dir": history_dir,
                "bash": bash_ok,
                "next_step": recommended_next_step(&listing),
            });
            crate::ui::line(serde_json::to_string_pretty(&payload)?);
            return Ok(0);
        }

        crate::ui::section("Doctor");
        crate::ui::kv("workspace", root.display());
        crate::ui::kv("model", listing.current.as_deref().unwrap_or("<unset>"));
        crate::ui::kv("shim", listing.current_shim.as_deref().unwrap_or("<none>"));
        if let Ok(recent) = config::recent_models() {
            crate::ui::kv("recent models", recent.len());
        }
        crate::ui::kv("mode", mode.name());
        crate::ui::kv("files-write", format_args!("{:?}", policy.files_write));
        crate::ui::kv("shell", format_args!("{:?}", policy.shell));
        crate::ui::kv("network", crate::ui::bool_text(policy.network));
        crate::ui::kv("risk", config::policy_risk_label(&policy));
        crate::ui::kv("interactive", crate::ui::bool_text(config::can_prompt()));
        crate::ui::kv(
            "bash",
            crate::ui::status_text(bash_ok, if bash_ok { "ok" } else { "missing" }),
        );
        crate::ui::line("");
        crate::ui::section("Local state");
        crate::ui::kv("config", config_file.display());
        crate::ui::kv("sessions", sessions_dir.display());
        crate::ui::kv("history", history_dir.display());
        crate::ui::line(
            "  Treat local state as sensitive: prompts, source snippets, tool output, and command output may be saved.",
        );
        crate::ui::line("");
        crate::ui::section("Auth / shims");
        if listing.auth.is_empty() {
            crate::ui::warn("no provider auth detected");
        } else {
            for item in &listing.auth {
                crate::ui::line(format_args!(
                    "  {}  {} ({})",
                    item.adapter,
                    item.env_var.as_deref().unwrap_or("-"),
                    item.source
                ));
                crate::ui::line(format_args!("    {}", item.detail));
            }
        }
        if listing.current.is_none() {
            crate::ui::line("");
            crate::ui::warn("no model configured");
            crate::ui::line(format_args!("  {}", recommended_next_step(&listing)));
        }
        crate::ui::line("");
        crate::ui::section("Recommended next steps");
        crate::ui::line(format_args!("  1. {}", recommended_next_step(&listing)));
        crate::ui::line("  2. For untrusted repos: `oy chat --mode plan`");
        crate::ui::line(format_args!(
            "  • Read-only container: {}",
            safe_container_command(&root, true)
        ));
        crate::ui::line("");
        crate::ui::section("Safety");
        crate::ui::line(
            "  oy is not a sandbox. Use `oy chat --mode plan` or a disposable container/VM for untrusted repos.",
        );
        crate::ui::line(
            "  Mount only needed credentials/env vars. Do not mount the host Docker socket into AI-assisted containers.",
        );
        Ok(0)
    }

    fn recommended_next_step(listing: &model::ModelListing) -> String {
        if listing.current.is_some() {
            return "Run `oy \"inspect this repo\"` or `oy chat`.".to_string();
        }
        if listing.all_models.is_empty() {
            return "Configure provider auth, then run `oy model` to inspect endpoint models."
                .to_string();
        }
        "Choose an introspected model with `oy model <name>`.".to_string()
    }

    fn safe_container_command(root: &Path, read_only: bool) -> String {
        let mode = if read_only { "ro" } else { "rw" };
        format!(
            "docker run --rm -it -v \"{}:/workspace:{mode}\" -w /workspace oy-image oy chat --mode plan",
            root.display()
        )
    }

    #[derive(Debug, Clone)]
    struct AuditArgs {
        focus: Vec<String>,
        out: PathBuf,
        max_chunks: usize,
        format: audit::AuditOutputFormat,
    }

    async fn audit_command(args: AuditArgs) -> Result<i32> {
        let started = std::time::Instant::now();
        let focus = args.focus.join(" ");
        let root = config::oy_root()?;
        let model = model::resolve_model(None)?;
        if !crate::ui::is_quiet() {
            crate::ui::section("audit");
            crate::ui::kv("workspace", root.display());
            crate::ui::kv("model", &model);
            crate::ui::kv("mode", "no-tools");
            crate::ui::kv("format", args.format.name());
            crate::ui::kv("out", args.out.display());
            crate::ui::kv("max chunks", args.max_chunks);
            if !focus.trim().is_empty() {
                crate::ui::kv("focus", crate::ui::compact_preview(&focus, 100));
            }
        }
        let result = audit::run(audit::AuditOptions {
            root,
            model,
            focus,
            out: args.out,
            max_chunks: args.max_chunks,
            format: args.format,
        })
        .await?;
        if crate::ui::is_json() {
            let payload = serde_json::json!({
                "output": result.output_path,
                "files": result.file_count,
                "chunks": result.chunk_count,
                "format": args.format.name(),
                "elapsed_ms": started.elapsed().as_millis(),
            });
            crate::ui::line(serde_json::to_string_pretty(&payload)?);
        } else {
            crate::ui::success(format_args!(
                "wrote {} ({} files, {} chunks, {})",
                result.output_path.display(),
                result.file_count,
                result.chunk_count,
                crate::ui::format_duration(started.elapsed())
            ));
        }
        Ok(0)
    }

    fn load_or_new(
        interactive: bool,
        mode_name: &str,
        continue_session: bool,
        resume: &str,
    ) -> Result<Session> {
        let mode = config::safety_mode(mode_name)?;
        let policy = config::tool_policy(mode.name());
        if continue_session || !resume.is_empty() {
            let name = if continue_session { None } else { Some(resume) };
            if let Some(session) =
                session::load_saved(name, interactive, mode.name().to_string(), policy)?
            {
                return Ok(session);
            }
        }
        let root = config::oy_root()?;
        let model = model::resolve_model(None)?;
        Ok(Session::new(
            root,
            model,
            interactive,
            mode.name().to_string(),
            policy,
        ))
    }

    fn collect_task(parts: &[String]) -> Result<String> {
        if !parts.is_empty() {
            return Ok(parts.join(" "));
        }
        if std::io::stdin().is_terminal() {
            return Ok(String::new());
        }
        let mut input = String::new();
        use std::io::Read as _;
        std::io::stdin().read_to_string(&mut input)?;
        Ok(input.trim().to_string())
    }

    fn print_session_intro(mode: &str, session: &Session, prompt: Option<&str>) {
        if crate::ui::is_quiet() {
            return;
        }
        crate::ui::section(mode);
        crate::ui::kv("workspace", session.root.display());
        crate::ui::kv("model", &session.model);
        crate::ui::kv("mode", &session.mode);
        crate::ui::kv("risk", config::policy_risk_label(&session.policy));
        if let Some(prompt) = prompt {
            crate::ui::kv("prompt", crate::ui::compact_preview(prompt, 100));
        }
    }

    fn write_workspace_file(root: &Path, requested: &Path, body: &str) -> Result<()> {
        let path = config::resolve_workspace_output_path(root, requested)?;
        let mut out = body.trim_end().to_string();
        out.push('\n');
        config::write_workspace_file(&path, out.as_bytes())
    }

    #[cfg(test)]
    mod audit_tests {
        use super::*;

        #[test]
        fn audit_accepts_max_chunks_flag() {
            let cli = parse_cli_for_test(&["oy", "audit", "--max-chunks", "240", "auth paths"]);
            let Some(Command::Audit {
                max_chunks, focus, ..
            }) = cli.command
            else {
                panic!("expected audit command");
            };
            assert_eq!(max_chunks, 240);
            assert_eq!(focus, vec!["auth paths"]);
        }

        #[test]
        fn help_documents_audit_options() {
            let help = command_help_for_test("audit");
            assert!(help.contains("--max-chunks <N>"));
            assert!(help.contains("--format <FORMAT>"));
        }

        #[test]
        fn audit_accepts_sarif_format() {
            let cli = parse_cli_for_test(&["oy", "audit", "--format", "sarif", "auth paths"]);
            let Some(Command::Audit { format, out, .. }) = cli.command else {
                panic!("expected audit command");
            };
            assert_eq!(format, AuditFormat::Sarif);
            assert_eq!(out, None);
        }

        #[test]
        fn exact_model_specs_are_endpoint_qualified_or_provider_ids() {
            assert!(is_exact_model_spec("copilot::gpt-4.1-mini"));
            assert!(is_exact_model_spec("openai/gpt-4.1-mini"));
            assert!(is_exact_model_spec(
                "bedrock::global.amazon.nova-2-lite-v1:0"
            ));
            assert!(!is_exact_model_spec("gpt"));
            assert!(!is_exact_model_spec("nova"));
        }
    }
}
