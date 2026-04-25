use anyhow::Result;
use reedline_repl_rs::reedline::{
    DefaultPrompt, DefaultPromptSegment, DefaultValidator, EditCommand, Emacs, FileBackedHistory,
    KeyCode, KeyModifiers, Reedline, ReedlineEvent, Signal, default_emacs_keybindings,
};
use std::path::PathBuf;

use crate::agent::{self, Session};
use crate::config;
use crate::model;

const HISTORY_SIZE: usize = 10_000;

fn chat_line_editor(history_path: PathBuf) -> Result<Reedline> {
    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Enter,
        ReedlineEvent::SubmitOrNewline,
    );
    let insert_newline = ReedlineEvent::Edit(vec![EditCommand::InsertNewline]);
    keybindings.add_binding(KeyModifiers::SHIFT, KeyCode::Enter, insert_newline.clone());
    keybindings.add_binding(KeyModifiers::ALT, KeyCode::Enter, insert_newline);
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('j'),
        ReedlineEvent::Submit,
    );

    Ok(Reedline::create()
        .with_history(Box::new(FileBackedHistory::with_file(
            HISTORY_SIZE,
            history_path,
        )?))
        .with_edit_mode(Box::new(Emacs::new(keybindings)))
        .with_validator(Box::new(DefaultValidator))
        .use_bracketed_paste(true))
}

pub async fn run_chat(session: &mut Session) -> Result<i32> {
    println!(
        "oy chat — Enter sends; Alt/Shift+Enter inserts newline; Ctrl+J force-sends; /help for commands"
    );
    let history_path = history_path("chat")?;
    let mut line_editor = chat_line_editor(history_path)?;
    let prompt = DefaultPrompt::new(
        DefaultPromptSegment::Basic("oy".to_string()),
        DefaultPromptSegment::Empty,
    );

    loop {
        match line_editor.read_line(&prompt)? {
            Signal::Success(line) => {
                if !handle_chat_line(session, line.trim()).await? {
                    break;
                }
            }
            Signal::CtrlD => break,
            Signal::CtrlC => {}
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
        crate::highlight::stdout(&format!("{summary}\n"));
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
            crate::highlight::stdout(&format!("{}\n", chat_help_text()));
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
            println!("Unknown command: /{other}");
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
        "Enter sends complete input; Alt/Shift+Enter inserts newline; Ctrl+J force-sends",
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
        crate::highlight::stdout(&format!("{answer}\n"));
    }
    Ok(true)
}

fn tokens_command(session: &Session) -> Result<bool> {
    let estimate = session.transcript.token_estimate(
        &model::to_genai_model_spec(&session.model),
        &session.system_prompt,
        &session.todos,
    );
    println!("messages: {}", estimate.messages);
    println!("system tokens: ~{}", estimate.system_tokens);
    println!("message tokens: ~{}", estimate.message_tokens);
    println!("total tokens: ~{}", estimate.total_tokens);
    Ok(true)
}

async fn model_command(value: Option<&str>, session: &mut Session) -> Result<bool> {
    if let Some(value) = value {
        config::save_model_config(value)?;
        session.model = model::resolve_model(Some(value))?;
    }
    crate::highlight::stdout(&format!("model: {}\n", session.model));
    Ok(true)
}

fn debug_command(session: &Session) -> Result<bool> {
    println!("workspace: {}", session.root.display());
    crate::highlight::stdout(&format!("model: {}\n", session.model));
    println!(
        "genai-model: {}",
        model::to_genai_model_spec(&session.model)
    );
    println!("agent: {}", session.agent);
    println!("interactive: {}", session.interactive);
    println!("yolo: {}", session.yolo);
    println!("messages: {}", session.transcript.messages.len());
    println!("todos: {}", session.todos.len());
    Ok(true)
}

fn yolo_command(session: &mut Session) -> Result<bool> {
    session.yolo = true;
    println!("yolo enabled");
    Ok(true)
}

fn save_command(name: Option<&str>, session: &mut Session) -> Result<bool> {
    let path = session.save(name)?;
    println!("saved session: {}", path.display());
    Ok(true)
}

fn load_command(name: Option<&str>, session: &mut Session) -> Result<bool> {
    if let Some(new_session) = agent::load_saved(name, true)? {
        *session = new_session;
        println!("loaded session");
    } else {
        println!("No saved sessions found.");
    }
    Ok(true)
}

fn undo_command(session: &mut Session) -> Result<bool> {
    if session.transcript.undo_last_turn() {
        println!("undid last turn");
    } else {
        println!("nothing to undo");
    }
    Ok(true)
}

fn clear_command(session: &mut Session) -> Result<bool> {
    session.transcript.messages.clear();
    println!("conversation cleared");
    Ok(true)
}

async fn run_prompt_with_model_reselect(session: &mut Session, prompt: &str) -> Result<()> {
    loop {
        match agent::run_prompt(session, prompt).await {
            Ok(answer) => {
                if !answer.is_empty() {
                    crate::highlight::stdout(&format!("{answer}\n"));
                }
                return Ok(());
            }
            Err(err) if config::can_prompt() => {
                crate::highlight::stderr(&format!("model call failed: {err:#}\n"));
                session.transcript.undo_last_turn();
                let Some(model) = choose_replacement_model(session).await? else {
                    return Err(err);
                };
                session.model = model;
                config::save_model_config(&session.model)?;
                crate::highlight::stderr(&format!("retrying with model: {}\n", session.model));
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
    print_initial_list: bool,
) -> Result<Option<String>> {
    if items.is_empty() {
        return Ok(None);
    }
    if print_initial_list {
        print_model_choices(current, items, "");
    }
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    match select_item(input.trim(), items) {
        Selection::Current => Ok(current.map(ToOwned::to_owned)),
        Selection::Selected(value) => Ok(Some(value)),
        Selection::Ambiguous(matches) => {
            print_model_choices(current, &matches, input.trim());
            Ok(None)
        }
        Selection::Invalid => Ok(None),
    }
}

pub fn ask(question: &str, choices: Option<&[String]>) -> Result<String> {
    if let Some(choices) = choices {
        print_choices(choices);
    }
    println!("{question}");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if let Some(choices) = choices {
        return match select_item(input, choices) {
            Selection::Selected(value) => Ok(value),
            Selection::Current => Ok(String::new()),
            Selection::Ambiguous(matches) => {
                print_choices(&matches);
                Ok(String::new())
            }
            Selection::Invalid => Ok(String::new()),
        };
    }
    Ok(input.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Selection {
    Current,
    Selected(String),
    Ambiguous(Vec<String>),
    Invalid,
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

fn print_model_choices(current: Option<&str>, items: &[String], query: &str) {
    if items.is_empty() {
        println!("No model matches for `{query}`.");
        return;
    }
    let heading = if query.is_empty() {
        "Models".to_string()
    } else {
        format!("Models matching `{query}`")
    };
    println!("{heading}:");
    for (idx, item) in items.iter().take(30).enumerate() {
        let marker = if current == Some(item.as_str()) {
            "*"
        } else {
            " "
        };
        println!("{marker} {:>2}. {item}", idx + 1);
    }
    if items.len() > 30 {
        println!("... {} more; keep filtering", items.len() - 30);
    }
}

fn print_choices(choices: &[String]) {
    for (idx, choice) in choices.iter().enumerate() {
        println!("{:>2}. {choice}", idx + 1);
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
