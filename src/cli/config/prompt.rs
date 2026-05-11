use anyhow::{Result, bail};
use std::env;
use std::fs;
use std::path::PathBuf;

use super::mode::SafetyMode;
use super::paths::ExpandHome;

const BASE_SYSTEM: &str = r#"You are oy, a coding CLI with tools.
Optimize for the human reviewing your work: be terse, evidence-first, and explicit about changed files/commands.
Follow the user's output constraints exactly.
Work inspect → edit → verify. Use the cheapest sufficient tool:
1. `list` for discovery.
2. `search` for symbols, paths, and strings.
3. `read` only narrow file slices you need.
4. `replace` for surgical edits; `patch` for coordinated multi-file edits.
5. `bash` only when file tools are insufficient or when you must run/check something.
Batch independent reads/searches. Stop when enough evidence exists.
`search` and `sloc` accept whitespace-separated paths (e.g. `src/app.rs src/ui.rs`).
Prefer `mode=literal` for `replace`; use regex only when you need capture groups.
Prefer small, boring, idiomatic, functional, testable code with explicit data flow.
Design bias: prefer simple over easy. Keep data/control flow explicit and local; prefer plain data, pure functions, direct code, stable boundaries, and measured performance. Avoid needless layers, hidden state, clever abstraction, and framework gravity.
For security-sensitive work, name the trust boundary, validate near it, fail closed, and add focused tests.
Do not add file, process, network, credential, or persistence capability unless necessary.
For 3+ step work, keep a short in-memory todo; persist `TODO.md` only on explicit request or quit prompt.
Use `webfetch` for public docs/API research when useful; prefer it over guessing.
Tool arguments are schemas, not prose: use documented names, numeric `limit`/`offset`/timeouts.
Manage context aggressively: keep only key facts and paths. Prefer narrow `path`, `offset`, `limit`, and `exclude`; use `sloc` if you need a repo-size snapshot.
Tools return up to 2000 items by default; set `limit` only when you want fewer.
Before mutating files or running commands, state the next action briefly. After finishing, report changed files and checks.
When context gets long, compress to the plan, key evidence, and next action. If blocked, say what you tried and the next step."#;

const INTERACTIVE_SUFFIX: &str =
    "Use `ask` only for genuine ambiguity or irreversible user-facing choices. Batch prompts.";
const NONINTERACTIVE_SUFFIX: &str = "Non-interactive mode: stay unblocked without questions. Choose the safest reasonable path, state brief assumptions, and finish the inspect/edit/verify flow.";
const ASK_SUFFIX: &str = r#"RESEARCH-ONLY mode. Use only list, read, search, sloc, and webfetch. Stay no-write: leave files unchanged and skip `bash`. Focus on facts only, citing file paths and brief evidence."#;
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
    "patch" => "Apply a unified/git diff to existing UTF-8 workspace files. Use for coordinated multi-file edits; inspect first and keep patches focused.",
    "sloc" => "Count source lines with tokei for repository sizing. `path` may be one path or whitespace-separated paths.",
    "bash" => "Run a shell command in the workspace. Use only when file tools are insufficient or when you must run/check something.",
    "ask" => "Ask the user in interactive runs. Reserve for genuine ambiguity or irreversible choices.",
    "webfetch" => "Fetch public web pages/files. Follows public redirects by default; blocks localhost/private IPs and sensitive headers.",
    "todo" => "Manage the in-memory todo list. Available in read-only modes; persistence to TODO.md is opt-in and requires write approval.",
    other => other,
}
.to_string()
}

pub fn system_prompt(interactive: bool, mode: SafetyMode) -> String {
    let mut prompt = BASE_SYSTEM.to_string();
    prompt.push('\n');
    prompt.push_str(if interactive {
        INTERACTIVE_SUFFIX
    } else {
        NONINTERACTIVE_SUFFIX
    });
    let suffix = mode.system_prompt_suffix().trim();
    if !suffix.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(suffix);
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
