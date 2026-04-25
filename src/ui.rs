use anyhow::Result;
use reedline::{
    ColumnarMenu, DefaultCompleter, DefaultPrompt, DefaultPromptSegment, FileBackedHistory,
    Reedline, ReedlineMenu, Signal,
};
use std::path::PathBuf;

use crate::agent::{self, Session, StoredMessage};
use crate::config;
use crate::model;

const HISTORY_SIZE: usize = 10_000;

pub async fn run_chat(session: &mut Session) -> Result<i32> {
    print_chat_intro(session);
    let mut line_editor = reedline("chat", chat_command_completions())?;
    let prompt = prompt("oy");
    loop {
        match line_editor.read_line(&prompt)? {
            Signal::Success(input) => {
                let input = input.trim();
                if input.is_empty() {
                    continue;
                }
                if let Some(limit) = parse_history_command(input) {
                    print_transcript(session, limit);
                    continue;
                }
                if input.starts_with('/') {
                    if !crate::cli::handle_chat_command(session, input).await? {
                        return Ok(0);
                    }
                    continue;
                }
                run_prompt_with_model_reselect(session, input).await?;
            }
            Signal::CtrlD | Signal::CtrlC => return Ok(0),
            _ => continue,
        }
    }
}

async fn run_prompt_with_model_reselect(session: &mut Session, prompt: &str) -> Result<()> {
    loop {
        match agent::run_prompt(session, prompt).await {
            Ok(answer) => {
                if !answer.is_empty() {
                    println!("{answer}");
                }
                return Ok(());
            }
            Err(err) if config::can_prompt() => {
                eprintln!("model call failed: {err:#}");
                session.transcript.undo_last_turn();
                let Some(model) = choose_replacement_model(session).await? else {
                    return Err(err);
                };
                session.model = model;
                config::save_model_config(&session.model)?;
                eprintln!("retrying with model: {}", session.model);
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
    if items.is_empty() {
        return Ok(None);
    }
    print_model_choices(current, items, "");
    let mut active_items = items.to_vec();
    let mut line_editor = reedline("model", items.to_vec())?;
    let prompt = prompt("model/filter");
    loop {
        match line_editor.read_line(&prompt)? {
            Signal::Success(input) => {
                let input = input.trim();
                match select_item(input, &active_items) {
                    Selection::Current => return Ok(current.map(ToOwned::to_owned)),
                    Selection::Selected(value) => return Ok(Some(value)),
                    Selection::Ambiguous(matches) => {
                        print_model_choices(current, &matches, input);
                        active_items = matches;
                    }
                    Selection::Invalid => {
                        let matches = filter_items(items, input);
                        if matches.is_empty() {
                            println!("No model matches `{input}`.");
                        } else if matches.len() == 1 {
                            return Ok(Some(matches[0].clone()));
                        } else {
                            print_model_choices(current, &matches, input);
                            active_items = matches;
                        }
                    }
                }
            }
            Signal::CtrlD | Signal::CtrlC => return Ok(None),
            _ => continue,
        }
    }
}

pub fn ask(question: &str, choices: Option<&[String]>) -> Result<String> {
    if let Some(choices) = choices {
        print_choices(choices);
    }
    let mut line_editor = reedline("ask", choices.unwrap_or(&[]).to_vec())?;
    let prompt = prompt(question);
    loop {
        match line_editor.read_line(&prompt)? {
            Signal::Success(input) => {
                let input = input.trim();
                if let Some(choices) = choices {
                    match select_item(input, choices) {
                        Selection::Selected(value) => return Ok(value),
                        Selection::Current => continue,
                        Selection::Ambiguous(matches) => print_choices(&matches),
                        Selection::Invalid => {
                            println!("Enter a number 1-{} or an exact choice.", choices.len())
                        }
                    }
                    continue;
                }
                return Ok(input.to_string());
            }
            Signal::CtrlD | Signal::CtrlC => return Ok(String::new()),
            _ => continue,
        }
    }
}

fn reedline(name: &str, completions: Vec<String>) -> Result<Reedline> {
    let history = FileBackedHistory::with_file(HISTORY_SIZE, history_path(name)?)?;
    let completer = Box::new(DefaultCompleter::new_with_wordlen(completions, 1));
    Ok(Reedline::create()
        .with_history(Box::new(history))
        .with_completer(completer)
        .with_menu(ReedlineMenu::EngineCompleter(Box::new(
            ColumnarMenu::default(),
        )))
        .with_quick_completions(true)
        .with_partial_completions(true))
}

fn prompt(left: &str) -> DefaultPrompt {
    DefaultPrompt::new(
        DefaultPromptSegment::Basic(left.to_string()),
        DefaultPromptSegment::Empty,
    )
}

fn history_path(name: &str) -> Result<PathBuf> {
    history_path_in(config::config_dir_path(), name)
}

fn history_path_in(config_dir: PathBuf, name: &str) -> Result<PathBuf> {
    let history = config_dir.join("history");
    std::fs::create_dir_all(&history)?;
    Ok(history.join(format!("{name}.txt")))
}

fn chat_command_completions() -> Vec<String> {
    let mut completions = crate::cli::CHAT_COMMANDS
        .iter()
        .map(|item| command_name(item.command).to_string())
        .chain(
            crate::cli::CHAT_COMMAND_ALIASES
                .iter()
                .map(|(alias, _)| alias.to_string()),
        )
        .collect::<Vec<_>>();
    completions.sort();
    completions.dedup();
    completions
}

fn command_name(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or(command)
}

fn parse_history_command(input: &str) -> Option<Option<usize>> {
    let mut parts = input.split_whitespace();
    if parts.next()? != "/history" {
        return None;
    }
    let limit = parts.next().and_then(|value| value.parse::<usize>().ok());
    Some(limit)
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

fn print_chat_intro(session: &Session) {
    println!(
        "oy chat — model={} agent={}  (/help, /history, Ctrl-D quits)",
        model::to_genai_model_spec(&session.model),
        session.agent
    );
}

fn print_transcript(session: &Session, limit: Option<usize>) {
    if session.transcript.messages.is_empty() {
        println!("No messages yet.");
        return;
    }
    let start = limit
        .map(|limit| session.transcript.messages.len().saturating_sub(limit))
        .unwrap_or(0);
    for message in session.transcript.messages.iter().skip(start) {
        match message {
            StoredMessage::User { content } => print_prefixed("you", content),
            StoredMessage::Assistant { content } => print_prefixed("oy", content),
            StoredMessage::AssistantToolCalls { tool_calls } => {
                for call in tool_calls {
                    println!(
                        "tool> {} {}",
                        call.fn_name,
                        preview_line(&call.fn_arguments.to_string())
                    );
                }
            }
            StoredMessage::Tool { content, .. } => {
                print_prefixed("tool", &preview_block(content, 24))
            }
        }
        println!();
    }
}

fn print_prefixed(prefix: &str, content: &str) {
    let mut lines = content.lines();
    if let Some(first) = lines.next() {
        println!("{prefix}> {first}");
    }
    for line in lines {
        println!("    {line}");
    }
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

fn preview_line(text: &str) -> String {
    let out = text.lines().next().unwrap_or_default().trim();
    if out.is_empty() {
        "done".to_string()
    } else {
        out.chars().take(120).collect()
    }
}

fn preview_block(text: &str, max_lines: usize) -> String {
    let mut lines = text.lines().take(max_lines).collect::<Vec<_>>();
    if text.lines().count() > max_lines {
        lines.push("...");
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_command_completions_include_history_command_name() {
        let completions = chat_command_completions();
        assert!(completions.contains(&"/history".to_string()));
        assert!(completions.contains(&"/model".to_string()));
        assert!(completions.contains(&"/q".to_string()));
    }

    #[test]
    fn parse_history_command_accepts_optional_limit() {
        assert_eq!(parse_history_command("/history"), Some(None));
        assert_eq!(parse_history_command("/history 5"), Some(Some(5)));
        assert_eq!(parse_history_command("/help"), None);
    }

    #[test]
    fn history_path_uses_named_history_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_path_in(dir.path().to_path_buf(), "chat").unwrap();
        assert!(path.ends_with("history/chat.txt"));
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

    #[test]
    fn preview_block_truncates_long_tool_output() {
        let out = preview_block("a\nb\nc", 2);
        assert_eq!(out, "a\nb\n...");
    }

    #[test]
    fn preview_line_has_default_for_empty_text() {
        assert_eq!(preview_line("\n"), "done");
    }
}
