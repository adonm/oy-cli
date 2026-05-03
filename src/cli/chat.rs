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
use crate::tools::{NetworkAccess, TodoStatus};

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
            .filter(|item| item.status != TodoStatus::Done)
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
    let answer = session::run_prompt_read_only(session, &config::ask_system_prompt(prompt)).await?;
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
            if let Some(chosen) =
                choose_model_from_items(listing.current.as_deref(), &listing.all_models, "Models")?
            {
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
    crate::ui::kv("mode", session.mode.name());
    crate::ui::kv("interactive", crate::ui::bool_text(session.interactive));
    crate::ui::kv(
        "files-write",
        format_args!("{:?}", session.policy.files_write()),
    );
    crate::ui::kv("shell", format_args!("{:?}", session.policy.shell));
    crate::ui::kv(
        "network",
        crate::ui::bool_text(session.policy.network == NetworkAccess::Enabled),
    );
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
    if let Some(new_session) = session::load_saved(name, true, session.mode, session.policy)? {
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

pub fn choose_recent_model(current: Option<&str>, recent: &[String]) -> Result<RecentModelChoice> {
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
        Some(index) if index < recent.len() => RecentModelChoice::Selected(recent[index].clone()),
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
