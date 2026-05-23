//! Interactive chat shell: REPL, slash commands, model switching,
//! history, and approval prompts.

use anyhow::{Context as _, Result, bail};
use dialoguer::{Confirm, theme::ColorfulTheme};
use std::fmt::Display;

use reedline_repl_rs::reedline::{
    DefaultPrompt, DefaultPromptSegment, EditCommand, Emacs, FileBackedHistory, KeyCode,
    KeyModifiers, Reedline, ReedlineEvent, Signal, default_emacs_keybindings,
};
use std::path::PathBuf;

use crate::config;
use crate::session::{self, Session};
use crate::tools::TodoStatus;

mod commands;
mod history;

use commands::handle_slash_command;
pub(crate) use commands::{
    RecentModelChoice, ask, choose_model, choose_model_with_initial_list, choose_recent_model,
};
use history::history_path;

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
    crate::ui::kv(
        "keys",
        "Enter sends · Alt/Shift+Enter newline · Ctrl-C interrupts active turn/quits prompt · /? help",
    );
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
    if config::can_prompt() && !session.todos.is_empty() {
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
        crate::ui::title_progress(format_args!(
            "oy chat · /{}",
            crate::ui::compact_preview(command.trim(), 48)
        ));
        return handle_slash_command(session, command.trim()).await;
    }
    crate::ui::title_progress(format_args!(
        "oy chat · {}",
        crate::ui::compact_preview(line, 56)
    ));
    run_prompt_with_context_recovery(session, line).await?;
    crate::ui::title_progress(format_args!("oy chat · {}", session.mode.name()));
    Ok(true)
}

#[derive(Debug, Clone, Copy)]
struct ChatTurnInterrupted;

impl std::fmt::Display for ChatTurnInterrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "chat turn interrupted")
    }
}

impl std::error::Error for ChatTurnInterrupted {}

async fn run_prompt_interruptible(session: &mut Session, prompt: &str) -> Result<String> {
    tokio::select! {
        result = session::run_prompt(session, prompt) => result,
        signal_result = tokio::signal::ctrl_c() => {
            signal_result.context("failed to listen for Ctrl-C")?;
            bail!(ChatTurnInterrupted);
        }
    }
}

async fn run_prompt_with_context_recovery(session: &mut Session, prompt: &str) -> Result<()> {
    let mut recovery_attempts = 0usize;
    loop {
        let err = match run_prompt_interruptible(session, prompt).await {
            Ok(answer) => return complete_chat_turn(answer),
            Err(err) if err.is::<ChatTurnInterrupted>() => {
                crate::ui::warn("interrupted current turn; still in chat");
                return Ok(());
            }
            Err(err) => err,
        };

        if let Some(budget_err) = err
            .downcast_ref::<session::ContextBudgetExceeded>()
            .copied()
        {
            recovery_attempts += 1;
            crate::ui::err_line(format_args!("model call failed: {err}"));
            if recovery_attempts >= MAX_CONTEXT_RECOVERY_ATTEMPTS {
                offer_save_after_context_failures(session)?;
                return Ok(());
            }
            if !recover_context_budget(session, recovery_attempts, budget_err)? {
                return Ok(());
            }
            continue;
        }

        if !prompt_retry_after_llm_error(&err)? {
            return Ok(());
        }
    }
}

fn complete_chat_turn(answer: String) -> Result<()> {
    if !answer.is_empty() {
        crate::ui::markdown(&format!("{answer}\n"));
    }
    Ok(())
}

fn prompt_retry_after_llm_error(err: &anyhow::Error) -> Result<bool> {
    crate::ui::err_line(format_args!("model call failed: {err}"));

    if !config::can_prompt() {
        return Err(anyhow::anyhow!(err.to_string()));
    }

    let choices = vec![
        "Retry".to_string(),
        "Return to chat".to_string(),
        "Exit".to_string(),
    ];
    let choice = ask("LLM call failed. What do you want to do?", Some(&choices))?;

    match choice.as_str() {
        "Retry" => Ok(true),
        "Return to chat" => Ok(false),
        _ => Err(anyhow::anyhow!(err.to_string())),
    }
}

fn recover_context_budget(
    session: &mut Session,
    attempt: usize,
    budget_err: session::ContextBudgetExceeded,
) -> Result<bool> {
    if config::can_prompt() {
        let limits = crate::model::model_limits(&session.model);
        let input_limit = limits.map(|l| l.input.unwrap_or(l.context));
        let output_limit = limits.and_then(|l| if l.output > 0 { Some(l.output) } else { None });
        let ctx_cfg = config::context_config_for_model(input_limit, output_limit);
        let raised_limit = ctx_cfg.input_budget_tokens() >= budget_err.estimated_tokens;
        let choices = vec![
            format!(
                "Retry with current OY_CONTEXT_LIMIT={}{}",
                ctx_cfg.limit_tokens,
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
    let Some((transcript, removed)) = session.transcript.oldest_turns_truncated() else {
        if attempt + 1 >= MAX_CONTEXT_RECOVERY_ATTEMPTS {
            offer_save_after_context_failures(session)?;
            return Ok(false);
        }
        anyhow::bail!(
            "context remains over budget and no more history can be truncated; save the session and try a different model later"
        );
    };
    let old_transcript = std::mem::replace(&mut session.transcript, transcript);
    let after = session.context_status().estimate.total_tokens;
    if after >= before {
        session.transcript = old_transcript;
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
