use anyhow::Result;
use dialoguer::{Input, Select, theme::ColorfulTheme};
use std::fmt::Display;

use reedline_repl_rs::reedline::{
    DefaultPrompt, DefaultPromptSegment, EditCommand, Emacs, FileBackedHistory, KeyCode,
    KeyModifiers, Reedline, ReedlineEvent, Signal, default_emacs_keybindings,
};
use std::path::PathBuf;

use crate::agent::{self, Session};
use crate::config;
use crate::model;

const HISTORY_SIZE: usize = 10_000;

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
    prompt_update_todo_on_quit(session).await?;
    Ok(0)
}

fn is_cursor_position_timeout(err: &impl Display) -> bool {
    let text = err.to_string();
    text.contains("cursor position") && text.contains("could not be read")
}

async fn prompt_update_todo_on_quit(session: &mut Session) -> Result<()> {
    if !crate::config::can_prompt() {
        return Ok(());
    }
    let prompt = "Update TODO.md with a concise summary of session actions?";
    let choices = ["yes".to_string(), "no".to_string()];
    if ask(prompt, Some(&choices))? != "yes" {
        return Ok(());
    }

    let summary = agent::run_prompt(
        session,
        "Update TODO.md with a concise summary of the session actions. If TODO.md already exists, read it first and merge its still-relevant items with the session summary before calling the todo tool with persist=true; do not blindly overwrite existing project todos. Keep the session summary to one done item unless active follow-up work remains.",
    )
    .await?;
    if !summary.is_empty() {
        crate::ui::markdown(&format!("{summary}\n"));
    }
    Ok(())
}

async fn handle_chat_line(session: &mut Session, line: &str) -> Result<bool> {
    if line.is_empty() {
        return Ok(true);
    }
    if let Some(command) = line.strip_prefix('/') {
        return handle_slash_command(session, command.trim()).await;
    }
    run_prompt_with_model_reselect(session, line).await?;
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
        "yolo" => yolo_command(session),
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
        "/help (/h, /?) -- show command help",
        "/tokens (/t) -- show approximate context tokens",
        "/compact [llm|deterministic] (/k) -- compact old transcript context",
        "/model [value] (/m) -- show or switch model",
        "/thinking [auto|off|low|medium|high] -- adjust reasoning effort",
        "/status (/s), /debug (/d) -- show session status",
        "/yolo -- approve all tools for this session",
        "/ask <question> -- research-only query",
        "/save [name] -- save session transcript",
        "/load [name] -- load a saved session",
        "/undo (/u) -- remove last prompt and follow-ups",
        "/clear (/c) -- clear conversation",
        "/quit (/q), /exit -- end session",
    ]
    .join("\n")
}

async fn ask_command(session: &mut Session, prompt: &str) -> Result<bool> {
    if prompt.is_empty() {
        anyhow::bail!("Usage: /ask <question>");
    }
    let answer = agent::run_prompt(session, &config::ask_system_prompt(prompt)).await?;
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
    crate::ui::kv("auto compact", status.auto_compact);
    crate::ui::kv("summary", status.summary_present);
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
        config::save_model_config(value)?;
        session.model = model::resolve_model(Some(value))?;
    }
    crate::ui::line(format_args!("model: {}", session.model));
    Ok(true)
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
    crate::ui::kv("agent", &session.agent);
    crate::ui::kv("interactive", session.interactive);
    crate::ui::kv(
        "files-write",
        format_args!("{:?}", session.policy.files_write),
    );
    crate::ui::kv("shell", format_args!("{:?}", session.policy.shell));
    crate::ui::kv("network", session.policy.network);
    crate::ui::kv("risk", policy_risk_label(session));
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
    crate::ui::kv("auto compact", status.auto_compact);
    crate::ui::kv("summary", status.summary_present);
    Ok(true)
}

fn policy_risk_label(session: &Session) -> &'static str {
    use crate::tools::Approval;
    if session.policy.read_only {
        "read-only"
    } else if session.policy.shell == Approval::Auto {
        "high: auto shell"
    } else if session.policy.files_write == Approval::Auto {
        "medium: auto edits"
    } else {
        "normal: asks before edits/shell"
    }
}

fn yolo_command(session: &mut Session) -> Result<bool> {
    session.policy.files_write = crate::tools::Approval::Auto;
    session.policy.shell = crate::tools::Approval::Auto;
    crate::ui::success("yolo enabled: auto-approving file edits and shell commands");
    crate::ui::warn("oy is not a sandbox; use only in trusted workspaces");
    Ok(true)
}

fn save_command(name: Option<&str>, session: &mut Session) -> Result<bool> {
    let path = session.save(name)?;
    crate::ui::success(format_args!("saved session {}", path.display()));
    Ok(true)
}

fn load_command(name: Option<&str>, session: &mut Session) -> Result<bool> {
    if let Some(new_session) = agent::load_saved(name, true)? {
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

async fn run_prompt_with_model_reselect(session: &mut Session, prompt: &str) -> Result<()> {
    loop {
        match agent::run_prompt(session, prompt).await {
            Ok(answer) => {
                if !answer.is_empty() {
                    crate::ui::markdown(&format!("{answer}\n"));
                }
                return Ok(());
            }
            Err(err) if config::can_prompt() => {
                crate::ui::err_line(format_args!("model call failed: {err:#}"));
                session.transcript.undo_last_turn();
                let Some(model) = choose_replacement_model(session).await? else {
                    return Err(err);
                };
                session.model = model;
                config::save_model_config(&session.model)?;
                crate::ui::err_line(format_args!("retrying with model: {}", session.model));
            }
            Err(err) => return Err(err),
        }
    }
}

async fn choose_replacement_model(session: &Session) -> Result<Option<String>> {
    let listing = model::inspect_models().await?;
    let items = replacement_model_choices(&session.model, listing.all_models, listing.hints);
    if items.is_empty() {
        return Ok(None);
    }
    choose_model(None, &items)
}

fn replacement_model_choices(
    current: &str,
    mut models: Vec<String>,
    hints: Vec<String>,
) -> Vec<String> {
    models.extend(hints);
    models.retain(|item| item != current);
    models.sort();
    models.dedup();
    models
}

pub fn choose_model(current: Option<&str>, items: &[String]) -> Result<Option<String>> {
    choose_model_with_initial_list(current, items, true)
}

pub fn choose_model_with_initial_list(
    current: Option<&str>,
    items: &[String],
    _print_initial_list: bool,
) -> Result<Option<String>> {
    if items.is_empty() || !config::can_prompt() {
        return Ok(None);
    }
    let theme = ColorfulTheme::default();
    let default = current.and_then(|value| items.iter().position(|item| item == value));
    let mut prompt = Select::with_theme(&theme)
        .with_prompt("Models")
        .items(items)
        .default(default.unwrap_or(0));
    if current.is_some() {
        prompt = prompt.with_prompt("Models (Esc keeps current)");
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
    std::fs::create_dir_all(&history)?;
    Ok(history.join(format!("{name}.txt")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_path_uses_named_history_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_path_in(dir.path().to_path_buf(), "chat").unwrap();
        assert!(path.ends_with("history/chat.txt"));
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
    fn replacement_model_choices_drop_current_and_dedup() {
        let choices = replacement_model_choices(
            "broken",
            vec!["broken".into(), "ok".into()],
            vec!["ok".into(), "other".into()],
        );
        assert_eq!(choices, vec!["ok".to_string(), "other".to_string()]);
    }
}
