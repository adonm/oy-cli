use anyhow::Result;

use std::fmt::{Display, Write as _};
use std::io::IsTerminal as _;
use std::sync::LazyLock;

static COLOR: LazyLock<bool> = LazyLock::new(|| {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    match std::env::var("OY_COLOR").ok().as_deref() {
        Some("always") => true,
        Some("never") => false,
        _ => std::io::stdout().is_terminal(),
    }
});

fn paint(code: &str, text: impl Display) -> String {
    if *COLOR {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
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
    out(text);
}

pub fn section(title: &str) {
    line(paint("1", title));
}

pub fn kv(key: &str, value: impl Display) {
    line(format_args!(
        "  {} {value}",
        paint("2", format_args!("{key:<11}"))
    ));
}

pub fn success(text: impl Display) {
    line(format_args!("{} {text}", paint("32", "✓")));
}

pub fn warn(text: impl Display) {
    line(format_args!("{} {text}", paint("33", "!")));
}

pub fn tool_start(name: &str, detail: &str) {
    if detail.is_empty() {
        err_line(format_args!("{} {name}", paint("36", "→")));
    } else {
        err_line(format_args!("{} {name} {detail}", paint("36", "→")));
    }
}

pub fn tool_result(preview: &str) {
    let preview = preview.trim_end();
    if !preview.is_empty() {
        err_line(preview);
    }
}

pub fn tool_error(name: &str, err: impl Display) {
    err_line(format_args!("{} {name}: {err:#}", paint("31", "✗")));
}

pub fn compact_spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn truncate_chars(text: &str, max: usize) -> String {
    let limit = max.saturating_sub(3);
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= limit {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

pub fn compact_preview(text: &str, max: usize) -> String {
    truncate_chars(&compact_spaces(text), max)
}

pub fn clamp_lines(text: &str, max_lines: usize, max_cols: usize) -> String {
    let mut out = String::new();
    let mut total = 0usize;
    for (idx, line) in text.lines().enumerate() {
        total = idx + 1;
        if idx < max_lines {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&truncate_chars(line, max_cols));
        }
    }
    if total > max_lines {
        let _ = write!(out, "\n… {} more lines", total - max_lines);
    }
    out
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
        format!("{head}\n... [truncated {hidden} chars] ...\n{tail}"),
        true,
    )
}
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
    let mut line_editor = chat_line_editor(history_path)?;
    let prompt = DefaultPrompt::new(
        DefaultPromptSegment::Basic("oy".to_string()),
        DefaultPromptSegment::Empty,
    );

    loop {
        match line_editor.read_line(&prompt)? {
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

async fn prompt_update_todo_on_quit(session: &mut Session) -> Result<()> {
    if !crate::config::can_prompt() {
        return Ok(());
    }
    let prompt = "Update TODO.md with a concise summary of session actions?";
    let choices = ["yes".to_string(), "no".to_string()];
    if crate::ui::ask(prompt, Some(&choices))? != "yes" {
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
        "model" => model_command(parts.next(), session).await,
        "debug" => debug_command(session),
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
        "m" => "model",
        "d" => "debug",
        "u" => "undo",
        "c" => "clear",
        "q" => "quit",
        other => other,
    }
}

fn chat_help_text() -> String {
    [
        "Enter sends; Alt/Shift+Enter inserts newline",
        "/help (/h, /?) -- show command help",
        "/tokens (/t) -- show approximate context tokens",
        "/model [value] (/m) -- show or switch model",
        "/debug (/d) -- show session debug info",
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
    let estimate = session.transcript.token_estimate(
        &model::to_genai_model_spec(&session.model),
        &session.system_prompt,
        &session.todos,
    );
    crate::ui::section("Context");
    crate::ui::kv("messages", estimate.messages);
    crate::ui::kv("system", format_args!("~{} tokens", estimate.system_tokens));
    crate::ui::kv(
        "messages",
        format_args!("~{} tokens", estimate.message_tokens),
    );
    crate::ui::kv("total", format_args!("~{} tokens", estimate.total_tokens));
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

fn debug_command(session: &Session) -> Result<bool> {
    crate::ui::section("Session");
    crate::ui::kv("workspace", session.root.display());
    crate::ui::kv("model", &session.model);
    crate::ui::kv("genai", model::to_genai_model_spec(&session.model));
    crate::ui::kv("agent", &session.agent);
    crate::ui::kv("interactive", session.interactive);
    crate::ui::kv("auto-edits", session.policy.auto_approve_edits);
    crate::ui::kv("auto-bash", session.policy.auto_approve_bash);
    crate::ui::kv("messages", session.transcript.messages.len());
    crate::ui::kv("todos", session.todos.len());
    Ok(true)
}

fn yolo_command(session: &mut Session) -> Result<bool> {
    session.policy.auto_approve_edits = true;
    session.policy.auto_approve_bash = true;
    crate::ui::success("yolo enabled");
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
    choose_items("Models", current, items, true)
}

pub fn choose_model_with_initial_list(
    current: Option<&str>,
    items: &[String],
    print_initial_list: bool,
) -> Result<Option<String>> {
    choose_items("Models", current, items, print_initial_list)
}

pub fn ask(question: &str, choices: Option<&[String]>) -> Result<String> {
    crate::ui::line(question);
    let Some(choices) = choices else {
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        return Ok(input.trim().to_string());
    };
    Ok(choose_items("Choices", None, choices, true)?.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Selection {
    Current,
    Selected(String),
    Ambiguous(Vec<String>),
    Invalid,
}

fn choose_items(
    heading: &str,
    current: Option<&str>,
    items: &[String],
    print_initial_list: bool,
) -> Result<Option<String>> {
    if items.is_empty() {
        return Ok(None);
    }
    if print_initial_list {
        print_numbered_items(heading, items, "", current, 30);
    }
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim();
    match select_item(input, items) {
        Selection::Current => Ok(current.map(ToOwned::to_owned)),
        Selection::Selected(value) => Ok(Some(value)),
        Selection::Ambiguous(matches) => {
            print_numbered_items(heading, &matches, input, current, 30);
            Ok(None)
        }
        Selection::Invalid => {
            if !input.is_empty() {
                crate::ui::warn(format_args!("no {heading} match `{input}`"));
            }
            Ok(None)
        }
    }
}

fn select_item(input: &str, items: &[String]) -> Selection {
    if input.trim().is_empty() {
        return Selection::Current;
    }
    if let Ok(index) = input.parse::<usize>() {
        return items
            .get(index.saturating_sub(1))
            .cloned()
            .map(Selection::Selected)
            .unwrap_or(Selection::Invalid);
    }
    if let Some(item) = items.iter().find(|item| item.as_str() == input) {
        return Selection::Selected(item.clone());
    }
    let matches = filter_items(items, input);
    match matches.len() {
        0 => Selection::Invalid,
        1 => Selection::Selected(matches[0].clone()),
        _ => Selection::Ambiguous(matches),
    }
}

fn filter_items(items: &[String], query: &str) -> Vec<String> {
    let needle = query.to_ascii_lowercase();
    items
        .iter()
        .filter(|item| item.to_ascii_lowercase().contains(&needle))
        .cloned()
        .collect()
}

fn history_path(name: &str) -> Result<PathBuf> {
    history_path_in(config::config_dir_path(), name)
}

fn history_path_in(config_dir: PathBuf, name: &str) -> Result<PathBuf> {
    let history = config_dir.join("history");
    std::fs::create_dir_all(&history)?;
    Ok(history.join(format!("{name}.txt")))
}

fn print_numbered_items(
    heading: &str,
    items: &[String],
    query: &str,
    current: Option<&str>,
    limit: usize,
) {
    let title = if query.is_empty() {
        heading.to_string()
    } else {
        format!("{heading} matching `{query}`")
    };
    let width = items.len().min(limit).max(1).to_string().len();
    crate::ui::section(&format!("{title} ({})", items.len()));
    for (idx, item) in items.iter().take(limit).enumerate() {
        let marker = if current == Some(item.as_str()) {
            "*"
        } else {
            " "
        };
        crate::ui::line(format_args!(
            "{marker} {:>width$}. {item}",
            idx + 1,
            width = width
        ));
    }
    if items.len() > limit {
        crate::ui::line(format_args!(
            "  ... {} more; type more text to filter",
            items.len() - limit
        ));
    }
    if !query.is_empty() {
        crate::ui::line("  Enter a number from this filtered list to select.");
    }
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
    }

    #[test]
    fn chat_help_uses_slash_commands() {
        let help = chat_help_text();
        assert!(help.contains("/help"));
        assert!(help.contains("/quit"));
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

    #[test]
    fn select_item_supports_index_exact_filter_and_ambiguity() {
        let items = vec![
            "alpha".to_string(),
            "beta".to_string(),
            "alphabet".to_string(),
        ];
        assert_eq!(
            select_item("2", &items),
            Selection::Selected("beta".to_string())
        );
        assert_eq!(
            select_item("alpha", &items),
            Selection::Selected("alpha".to_string())
        );
        assert!(matches!(
            select_item("alp", &items),
            Selection::Ambiguous(_)
        ));
        assert_eq!(select_item("", &items), Selection::Current);
        assert_eq!(select_item("9", &items), Selection::Invalid);
    }

    #[test]
    fn filter_items_is_case_insensitive() {
        let items = vec!["gpt-4o".to_string(), "Claude Sonnet".to_string()];
        assert_eq!(
            filter_items(&items, "sonnet"),
            vec!["Claude Sonnet".to_string()]
        );
    }
}
