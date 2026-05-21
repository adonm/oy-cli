//! Slash-command handlers and interactive prompts: /help, /model,
//! /compact, /thinking, /safety, /save, /audit, /exit, and `ask`.

use anyhow::Result;
use dialoguer::{Input, Select, theme::ColorfulTheme};

use crate::config;
use crate::model;
use crate::session::{self, Session};
use crate::tools::NetworkAccess;

pub(super) async fn handle_slash_command(session: &mut Session, command: &str) -> Result<bool> {
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
        "thinking" => thinking_command(parts.next(), session),
        "model" => model_command(parts.next(), session).await,
        "debug" | "status" => status_command(session),
        "ask" => {
            let prompt = parts.collect::<Vec<_>>().join(" ");
            ask_command(session, &prompt).await
        }
        "save" => save_command(parts.next(), session),
        "load" => load_command(parts.next(), session),
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
        "c" => "clear",
        "q" => "quit",
        other => other,
    }
}

pub fn chat_help_text() -> String {
    [
        "Enter sends; Alt/Shift+Enter inserts newline; Ctrl-C interrupts an active turn or exits at the prompt",
        "/help (/h, /?) -- show help",
        "/status (/s), /debug (/d) -- show model, mode, context, and todos",
        "/model [value] (/m) -- show or switch model",
        "/ask <question> -- research-only query",
        "/save [name], /load [name] -- save or load a session",
        "/clear (/c) -- restart this chat session",
        "/quit (/q), /exit -- end session",
        "Advanced: /tokens, /compact [llm|deterministic], /thinking [auto|off|effort]",
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

fn thinking_command(value: Option<&str>, session: &Session) -> Result<bool> {
    let supported = model::reasoning_efforts_for(&session.model);
    if let Some(value) = value {
        match value {
            "" | "auto" => model::set_thinking_override(None),
            "off" | "none" => model::set_thinking_override(Some("none")),
            val if supported.iter().any(|s| s == val) => {
                model::set_thinking_override(Some(val));
            }
            other => anyhow::bail!(
                "thinking must be auto, off, or one of: {}; got {other}",
                supported.join(", ")
            ),
        }
    }
    let current = model::get_thinking_effort()
        .or_else(|| model::default_reasoning_effort(&session.model))
        .unwrap_or_else(|| "auto".to_string());
    crate::ui::line(format_args!("thinking: {current}"));
    if !supported.is_empty() {
        crate::ui::line(format_args!("  model supports: {}", supported.join(", ")));
    } else {
        crate::ui::line("  model does not advertise reasoning support");
    }
    Ok(true)
}

async fn compact_command(mode: Option<&str>, session: &mut Session) -> Result<bool> {
    let stats = match mode.unwrap_or("deterministic") {
        "" | "deterministic" | "det" | "fast" | "llm" | "smart" => session.compact_deterministic(),
        other => anyhow::bail!("compact mode must be deterministic; got {other}"),
    };
    crate::ui::section("Compaction");
    if let Some(stats) = stats {
        crate::ui::kv("removed messages", stats.removed_messages);
        crate::ui::kv("tool outputs", stats.compacted_tools);
        crate::ui::kv("summarized", stats.summarized);
    } else {
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

fn status_command(session: &Session) -> Result<bool> {
    let info = model::provider_info(&session.model);
    crate::ui::section("Status");
    crate::ui::kv("workspace", session.root.display());
    crate::ui::kv("model", &session.model);
    crate::ui::kv("provider", &info.provider);
    if let Some(ref endpoint) = info.endpoint {
        crate::ui::kv("endpoint", endpoint);
    }
    crate::ui::kv(
        "thinking",
        model::default_reasoning_effort(&session.model).unwrap_or_else(|| "auto/off".to_string()),
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

fn clear_command(session: &mut Session) -> Result<bool> {
    *session = session.restarted();
    crate::ui::success("restarted chat session");
    Ok(true)
}

pub(crate) fn choose_model(current: Option<&str>, items: &[String]) -> Result<Option<String>> {
    choose_model_with_initial_list(current, items, true)
}

pub(crate) fn choose_recent_model(
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
pub(crate) enum RecentModelChoice {
    Selected(String),
    Inspect,
    Clear,
    Cancelled,
}

pub(crate) fn choose_model_with_initial_list(
    current: Option<&str>,
    items: &[String],
    _print_initial_list: bool,
) -> Result<Option<String>> {
    if items.is_empty() || !config::can_prompt() {
        return Ok(None);
    }
    choose_model_from_items(current, items, "Models")
}

fn choose_model_from_items(
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn chat_help_snapshot() {
        insta::assert_snapshot!(chat_help_text());
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
