//! `oy run` and `oy chat` subcommands: one-shot task execution
//! and interactive chat session startup.

use anyhow::Result;
use clap::Args;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use crate::config;
use crate::model;
use crate::session::{self, Session};

#[derive(Debug, Args, Clone)]
pub(super) struct SharedModeArgs {
    #[arg(
        long,
        alias = "agent",
        default_value = "default",
        help = "Safety mode (default: balanced): plan, ask, edit, or auto"
    )]
    pub(super) mode: config::SafetyMode,
    #[arg(
        long = "continue-session",
        default_value_t = false,
        help = "Resume the most recent saved session"
    )]
    pub(super) continue_session: bool,
    #[arg(
        long,
        default_value = "",
        value_name = "NAME_OR_NUMBER",
        help = "Resume a named or numbered saved session"
    )]
    pub(super) resume: String,
}

#[derive(Debug, Args, Clone)]
pub(super) struct RunArgs {
    #[command(flatten)]
    pub(super) shared: SharedModeArgs,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write the final answer to a workspace file"
    )]
    pub(super) out: Option<PathBuf>,
    #[arg(
        value_name = "PROMPT",
        help = "Task prompt; omitted means read stdin or start chat in a TTY"
    )]
    pub(super) task: Vec<String>,
}

#[derive(Debug, Args, Clone)]
pub(super) struct ChatArgs {
    #[command(flatten)]
    pub(super) shared: SharedModeArgs,
}

pub(super) async fn run_command(args: RunArgs) -> Result<i32> {
    let task = collect_task(&args.task)?;
    if task.trim().is_empty() {
        return chat_command(ChatArgs {
            shared: args.shared,
        })
        .await;
    }
    let mut session = load_or_new(
        false,
        args.shared.mode,
        args.shared.continue_session,
        &args.shared.resume,
    )?;
    let _ = crate::agent::model::cache_model_limits(&session.model).await;
    let _title = crate::ui::title_scope(format_args!(
        "oy run · {} · {}",
        session.mode.name(),
        crate::ui::compact_preview(&task, 48)
    ));
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

pub(super) async fn chat_command(args: ChatArgs) -> Result<i32> {
    let mut session = load_or_new(
        true,
        args.shared.mode,
        args.shared.continue_session,
        &args.shared.resume,
    )?;
    let _ = crate::agent::model::cache_model_limits(&session.model).await;
    let _title = crate::ui::title_scope(format_args!("oy chat · {}", session.mode.name()));
    print_session_intro("chat", &session, None);
    crate::chat::run_chat(&mut session).await
}

fn load_or_new(
    interactive: bool,
    mode: config::SafetyMode,
    continue_session: bool,
    resume: &str,
) -> Result<Session> {
    if continue_session || !resume.is_empty() {
        let name = if continue_session { None } else { Some(resume) };
        if let Some(session) = session::load_saved(name, interactive, mode)? {
            return Ok(session);
        }
    }
    let root = config::oy_root()?;
    let model = model::resolve_model(None)?;
    Ok(Session::new(root, model, interactive, mode))
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
    crate::ui::kv("mode", session.mode.name());
    crate::ui::kv("risk", config::policy_risk_label(&session.policy()));
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
