use anyhow::{Context, Result};
use chrono::Utc;
use genai::chat::{ChatMessage, ChatRequest, ToolCall, ToolResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::Path;
use tiktoken_rs::{bpe_for_model, cl100k_base};

use crate::config::{self, SessionFile};
use crate::model;
use crate::tools::{TodoItem, ToolContext};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub messages: Vec<StoredMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum StoredMessage {
    #[serde(rename = "user")]
    User { content: String },
    #[serde(rename = "assistant")]
    Assistant { content: String },
    #[serde(rename = "assistant_tool_calls")]
    AssistantToolCalls { tool_calls: Vec<StoredToolCall> },
    #[serde(rename = "tool")]
    Tool { call_id: String, content: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToolCall {
    pub call_id: String,
    pub fn_name: String,
    pub fn_arguments: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenEstimate {
    pub messages: usize,
    pub system_tokens: usize,
    pub message_tokens: usize,
    pub total_tokens: usize,
}

impl StoredMessage {
    fn content_text(&self) -> String {
        match self {
            StoredMessage::User { content } | StoredMessage::Assistant { content } => {
                content.clone()
            }
            StoredMessage::AssistantToolCalls { tool_calls } => tool_calls
                .iter()
                .map(|call| format!("{} {}", call.fn_name, call.fn_arguments))
                .collect::<Vec<_>>()
                .join(
                    "
",
                ),
            StoredMessage::Tool { content, .. } => content.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub root: std::path::PathBuf,
    pub model: String,
    pub system_prompt: String,
    pub interactive: bool,
    pub yolo: bool,
    pub agent: String,
    pub transcript: Transcript,
    pub todos: Vec<TodoItem>,
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn undo_last_turn(&mut self) -> bool {
        for index in (0..self.messages.len()).rev() {
            if matches!(self.messages[index], StoredMessage::User { .. }) {
                self.messages.truncate(index);
                return true;
            }
        }
        false
    }

    pub fn token_estimate(
        &self,
        model: &str,
        system_prompt: &str,
        todos: &[TodoItem],
    ) -> TokenEstimate {
        let model_name = model
            .rsplit_once("::")
            .map(|(_, name)| name)
            .unwrap_or(model);
        let bpe = bpe_for_model(model_name).ok();
        let fallback_bpe = if bpe.is_none() {
            cl100k_base().ok()
        } else {
            None
        };
        let count_text = |text: &str| -> usize {
            if let Some(bpe) = bpe {
                return bpe.encode_with_special_tokens(text).len();
            }
            fallback_bpe
                .as_ref()
                .map(|bpe| bpe.encode_with_special_tokens(text).len())
                .unwrap_or_else(|| text.split_whitespace().count())
        };
        let system_tokens = count_text(system_prompt) + if todos.is_empty() { 0 } else { 4 };
        let message_tokens = self
            .messages
            .iter()
            .map(|message| 4 + count_text(&message.content_text()))
            .sum::<usize>();
        TokenEstimate {
            messages: self.messages.len(),
            system_tokens,
            message_tokens,
            total_tokens: system_tokens + message_tokens,
        }
    }

    pub fn to_chat_request(&self, system_prompt: &str, tool_context: &ToolContext) -> ChatRequest {
        let mut req = ChatRequest::default().with_system(system_prompt);
        for msg in &self.messages {
            match msg {
                StoredMessage::User { content } => {
                    req = req.append_message(ChatMessage::user(content.clone()))
                }
                StoredMessage::Assistant { content } => {
                    req = req.append_message(ChatMessage::assistant(content.clone()))
                }
                StoredMessage::AssistantToolCalls { tool_calls } => {
                    let calls = tool_calls
                        .iter()
                        .map(|call| ToolCall {
                            call_id: call.call_id.clone(),
                            fn_name: call.fn_name.clone(),
                            fn_arguments: call.fn_arguments.clone(),
                            thought_signatures: None,
                        })
                        .collect::<Vec<_>>();
                    req = req.append_message(ChatMessage::assistant(calls));
                }
                StoredMessage::Tool { call_id, content } => {
                    req = req.append_message(ChatMessage::from(ToolResponse::new(
                        call_id.clone(),
                        content.clone(),
                    )));
                }
            }
        }
        let mut prompt = system_prompt.to_string();
        if !tool_context.todos.is_empty() {
            let header = config::session_text_value("transcript", "todo_system")
                .unwrap_or_else(|_| String::from("{todos}"));
            let todos = crate::tools::format_todos(&tool_context.todos);
            prompt.push_str("\n\n");
            prompt.push_str(header.replace("{todos}", todos.trim_end()).trim());
        }
        req.system = Some(prompt);
        req
    }
}

impl Session {
    pub fn new(
        root: std::path::PathBuf,
        model: String,
        interactive: bool,
        agent: String,
        yolo: bool,
    ) -> Self {
        Self {
            root,
            model,
            system_prompt: config::active_system_prompt(interactive, &agent),
            interactive,
            yolo,
            agent,
            transcript: Transcript::new(),
            todos: Vec::new(),
        }
    }

    pub fn tool_context(&self) -> ToolContext {
        ToolContext {
            root: self.root.clone(),
            interactive: self.interactive,
            yolo: self.yolo,
            agent: self.agent.clone(),
            todos: self.todos.clone(),
        }
    }

    fn wait_status(&self, model_spec: &str) -> String {
        let estimate = self
            .transcript
            .token_estimate(model_spec, &self.system_prompt, &self.todos);
        let mut parts = vec![
            "oy".to_string(),
            model_suffix(model_spec).to_string(),
            format!("{}", format_tokens(estimate.total_tokens)),
            format!("{} msg", estimate.messages),
        ];
        if !self.todos.is_empty() {
            let active = self
                .todos
                .iter()
                .filter(|item| item.status != "done")
                .count();
            parts.push(format!("{active}/{} todo", self.todos.len()));
        }
        append_starship_suffix(parts.join(" · "), &self.root)
    }

    pub fn save(&self, name: Option<&str>) -> Result<std::path::PathBuf> {
        let payload = SessionFile {
            model: self.model.clone(),
            agent: self.agent.clone(),
            saved_at: Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            transcript: serde_json::to_value(&self.transcript)?,
        };
        config::save_session_file(name, &payload)
    }
}

pub fn load_saved(name: Option<&str>, interactive: bool) -> Result<Option<Session>> {
    let Some(path) = config::resolve_saved_session(name)? else {
        return Ok(None);
    };
    let saved = config::load_session_file(&path)?;
    let transcript: Transcript = serde_json::from_value(saved.transcript)
        .with_context(|| format!("invalid saved transcript in {}", path.display()))?;
    let root = config::oy_root()?;
    Ok(Some(Session {
        root,
        model: saved.model,
        system_prompt: config::active_system_prompt(interactive, &saved.agent),
        interactive,
        yolo: config::yolo_enabled() || saved.agent == "auto-approve",
        agent: saved.agent,
        transcript,
        todos: Vec::new(),
    }))
}

fn model_suffix(model_spec: &str) -> &str {
    model_spec
        .rsplit_once("::")
        .map(|(_, model)| model)
        .unwrap_or(model_spec)
}

fn format_tokens(count: usize) -> String {
    if count < 1000 {
        format!("{count} tok")
    } else {
        format!("{:.1}k tok", count as f64 / 1000.0)
    }
}

fn append_starship_suffix(status: String, root: &Path) -> String {
    append_starship_suffix_with_line(status, starship_line(root).as_deref())
}

fn append_starship_suffix_with_line(status: String, line: Option<&str>) -> String {
    let Some(line) = line else {
        return status;
    };
    format!("{status} {}", dim(line))
}

fn starship_line(root: &Path) -> Option<String> {
    let output = starship_command(root).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            let stripped = strip_ansi(line);
            let line = stripped.trim();
            (!line.is_empty()).then(|| line.to_string())
        })
}

fn starship_command(root: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("starship");
    command
        .arg("prompt")
        .arg("--cmd-duration=0")
        .arg("--status=0")
        .current_dir(root)
        .env("STARSHIP_SHELL", "fish");
    if let Some(config) = starship_config_with_git_metrics() {
        command.env("STARSHIP_CONFIG", config);
    }
    command
}

fn starship_config_with_git_metrics() -> Option<OsString> {
    let existing = std::env::var_os("STARSHIP_CONFIG");
    let source = existing
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .or_else(default_starship_config);
    let source = source.as_deref();
    if starship_git_metrics_configured(source) {
        return existing;
    }
    let mut config = source.unwrap_or_default().trim_end().to_string();
    if !config.is_empty() {
        config.push_str("\n\n");
    }
    config.push_str("[git_metrics]\ndisabled = false\n");
    let path = std::env::temp_dir().join(format!("oy-starship-{}.toml", std::process::id()));
    std::fs::write(&path, config).ok()?;
    Some(path.into_os_string())
}

fn default_starship_config() -> Option<String> {
    let path = dirs::config_dir()?.join("starship.toml");
    std::fs::read_to_string(path).ok()
}

fn starship_git_metrics_configured(config: Option<&str>) -> bool {
    let Some(config) = config else {
        return false;
    };
    let mut in_git_metrics = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_git_metrics = line.trim_matches(['[', ']'].as_ref()).trim() == "git_metrics";
            continue;
        }
        if in_git_metrics && line.starts_with("disabled") && line.contains('=') {
            return true;
        }
    }
    false
}

fn dim(text: &str) -> String {
    format!("\x1b[2m{text}\x1b[0m")
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'[') {
            chars.next();
            continue;
        }
        if ch == '\\' && chars.peek() == Some(&']') {
            chars.next();
            continue;
        }
        if ch == '\\' && chars.peek() == Some(&'\\') {
            chars.next();
            out.push('\\');
            continue;
        }
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for ch in chars.by_ref() {
                if ('@'..='~').contains(&ch) {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

pub async fn run_prompt(session: &mut Session, prompt: &str) -> Result<String> {
    let client = model::build_client()?;
    session.transcript.messages.push(StoredMessage::User {
        content: prompt.to_string(),
    });

    loop {
        let tool_context = session.tool_context();
        let tool_specs = crate::tools::tool_specs(&tool_context);
        let req = session
            .transcript
            .to_chat_request(&session.system_prompt, &tool_context)
            .with_tools(tool_specs.clone());
        let model_spec = model::to_genai_model_spec(&session.model);
        crate::highlight::stderr(&format!("{}\n", session.wait_status(&model_spec)));
        let response = client.exec_chat(&model_spec, req, None).await?;
        let tool_calls = response
            .tool_calls()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if !tool_calls.is_empty() {
            session
                .transcript
                .messages
                .push(StoredMessage::AssistantToolCalls {
                    tool_calls: tool_calls
                        .iter()
                        .map(|call| StoredToolCall {
                            call_id: call.call_id.clone(),
                            fn_name: call.fn_name.clone(),
                            fn_arguments: call.fn_arguments.clone(),
                        })
                        .collect(),
                });

            for call in tool_calls {
                let mut ctx = session.tool_context();
                let result =
                    match crate::tools::invoke(&mut ctx, &call.fn_name, call.fn_arguments.clone())
                        .await
                    {
                        Ok(value) => value,
                        Err(err) => json!({"ok": false, "error": err.to_string()}),
                    };
                session.todos = ctx.todos;
                let content = crate::tools::encode_tool_output(&result);
                session.transcript.messages.push(StoredMessage::Tool {
                    call_id: call.call_id.clone(),
                    content,
                });
            }
            continue;
        }

        let answer = response.into_first_text().unwrap_or_default();
        session.transcript.messages.push(StoredMessage::Assistant {
            content: answer.clone(),
        });
        return Ok(answer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_last_turn_removes_user_and_followups() {
        let mut tx = Transcript {
            messages: vec![
                StoredMessage::User {
                    content: "one".into(),
                },
                StoredMessage::Assistant {
                    content: "two".into(),
                },
                StoredMessage::User {
                    content: "three".into(),
                },
                StoredMessage::AssistantToolCalls {
                    tool_calls: Vec::new(),
                },
                StoredMessage::Tool {
                    call_id: "c".into(),
                    content: "tool".into(),
                },
            ],
        };
        assert!(tx.undo_last_turn());
        assert_eq!(tx.messages.len(), 2);
        assert!(matches!(tx.messages[0], StoredMessage::User { .. }));
        assert!(matches!(tx.messages[1], StoredMessage::Assistant { .. }));
    }

    #[test]
    fn token_estimate_counts_system_and_messages() {
        let tx = Transcript {
            messages: vec![
                StoredMessage::User {
                    content: "hello world".into(),
                },
                StoredMessage::Assistant {
                    content: "hi".into(),
                },
            ],
        };
        let estimate = tx.token_estimate("gpt-4o", "system", &[]);
        assert_eq!(estimate.messages, 2);
        assert!(estimate.system_tokens > 0);
        assert!(estimate.message_tokens > estimate.messages);
        assert_eq!(
            estimate.total_tokens,
            estimate.system_tokens + estimate.message_tokens
        );
    }

    #[test]
    fn strip_ansi_removes_escape_sequences() {
        assert_eq!(strip_ansi("\x1b[32mmain\x1b[0m λ"), "main λ");
    }

    #[test]
    fn append_starship_suffix_dims_suffix() {
        assert_eq!(
            append_starship_suffix_with_line("oy · model · 1 tok ctx".to_string(), Some("main")),
            "oy · model · 1 tok ctx \x1b[2mmain\x1b[0m"
        );
    }

    #[test]
    fn starship_git_metrics_configured_detects_explicit_disabled() {
        assert!(starship_git_metrics_configured(Some(
            "[git_metrics]
disabled = true
"
        )));
        assert!(!starship_git_metrics_configured(Some("[git_branch]\n")));
    }

    #[test]
    fn strip_ansi_removes_bash_prompt_markers() {
        assert_eq!(strip_ansi("\\[\x1b[32m\\]main\\[\x1b[0m\\]"), "main");
    }
}
