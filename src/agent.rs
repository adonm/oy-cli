use anyhow::{Context, Result};
use chrono::Utc;
use genai::chat::{ChatMessage, ChatRequest, ToolCall, ToolResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tiktoken_rs::{bpe_for_model, cl100k_base};

use crate::config::{self, SessionFile};
use crate::model;
use crate::tools::{self, TodoItem, ToolContext};

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
            let mut todos = String::new();
            for item in &tool_context.todos {
                let icon = match item.status.as_str() {
                    "done" => "[x]",
                    "in_progress" => "[~]",
                    _ => "[ ]",
                };
                todos.push_str(&format!("{icon} {}: {}\n", item.id, item.task));
            }
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

pub async fn run_prompt(session: &mut Session, prompt: &str) -> Result<String> {
    let client = model::build_client()?;
    session.transcript.messages.push(StoredMessage::User {
        content: prompt.to_string(),
    });

    loop {
        let tool_context = session.tool_context();
        let req = session
            .transcript
            .to_chat_request(&session.system_prompt, &tool_context)
            .with_tools(tools::tool_specs(&tool_context));
        let model_spec = model::to_genai_model_spec(&session.model);
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
                    match tools::invoke(&mut ctx, &call.fn_name, call.fn_arguments.clone()).await {
                        Ok(value) => value,
                        Err(err) => json!({"ok": false, "error": err.to_string()}),
                    };
                session.todos = ctx.todos;
                let content = tools::encode_tool_output(&result);
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
}
