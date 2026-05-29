//! Session transcripts stored as [`Message`] sequences with token
//! estimation, context-budget tracking, and compaction signalling.
//!
//! [`Transcript`] is the persisted unit; [`ContextStatus`] and
//! [`ContextBudgetExceeded`] provide budget feedback to the session loop.

use serde::{Deserialize, Serialize};

use super::compaction::{compact_text, count_tokens, message_content_text};
use crate::config;
use crate::llm::{Message, MessageContent, ToolResultContent};
use crate::tools::{TodoItem, ToolContext};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    #[serde(default)]
    pub summary: Option<String>,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TokenEstimate {
    pub messages: usize,
    pub system_tokens: usize,
    pub message_tokens: usize,
    pub total_tokens: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CompactionStats {
    pub removed_messages: usize,
    pub compacted_tools: usize,
    pub summarized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ContextStatus {
    pub estimate: TokenEstimate,
    pub limit_tokens: usize,
    pub input_budget_tokens: usize,
    pub trigger_tokens: usize,
    pub summary_present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudgetExceeded {
    pub estimated_tokens: usize,
    pub input_budget_tokens: usize,
    pub limit_tokens: usize,
}

impl std::fmt::Display for ContextBudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "context estimate {} exceeds input budget {}; use /compact, temporarily raise OY_CONTEXT_LIMIT, or force-truncate history",
            self.estimated_tokens, self.input_budget_tokens
        )
    }
}

impl std::error::Error for ContextBudgetExceeded {}

impl Transcript {
    pub fn new() -> Self {
        Self {
            summary: None,
            messages: Vec::new(),
        }
    }

    pub fn oldest_turns_truncated(&self) -> Option<(Self, usize)> {
        if self.messages.len() <= 1 {
            return None;
        }
        let remove_count = (self.messages.len() / 4)
            .max(1)
            .min(self.messages.len() - 1);
        let keep_from = self.valid_keep_from(remove_count);
        if keep_from == 0 || keep_from >= self.messages.len() {
            return None;
        }
        Some((
            Self {
                summary: self.summary.clone(),
                messages: self.messages[keep_from..].to_vec(),
            },
            keep_from,
        ))
    }

    pub fn token_estimate(
        &self,
        model: &str,
        system_prompt: &str,
        todos: &[TodoItem],
    ) -> TokenEstimate {
        let count_text = |text: &str| count_tokens(model, text);
        let system_tokens = count_text(system_prompt) + if todos.is_empty() { 0 } else { 4 };
        let summary_tokens = self
            .summary
            .as_ref()
            .map(|summary| 4 + count_text(summary))
            .unwrap_or(0);
        let message_tokens = summary_tokens
            + self
                .messages
                .iter()
                .map(|message| 4 + count_text(&message_content_text(message)))
                .sum::<usize>();
        TokenEstimate {
            messages: self.messages.len() + usize::from(self.summary.is_some()),
            system_tokens,
            message_tokens,
            total_tokens: system_tokens + message_tokens,
        }
    }

    pub fn with_compacted_tool_outputs(&self, max_bytes: usize) -> (Self, usize) {
        let mut messages = self.messages.clone();
        let mut compacted = 0;
        for message in &mut messages {
            let Message::User { content } = message else {
                continue;
            };
            for item in content.iter_mut() {
                let MessageContent::ToolResult { content, .. } = item else {
                    continue;
                };
                for part in content.iter_mut() {
                    let ToolResultContent::Text { text } = part else {
                        continue;
                    };
                    if text.contains("[tool output compacted]") {
                        continue;
                    }
                    let original_len = text.len();
                    *text = compact_text(text, max_bytes, "tool output compacted");
                    if text.len() < original_len {
                        compacted += 1;
                    }
                }
            }
        }
        (
            Self {
                summary: self.summary.clone(),
                messages,
            },
            compacted,
        )
    }

    /// Aggressive compaction: compact ALL tool outputs with smaller budget.
    /// Used when we need maximum space savings before dropping old messages.
    pub fn with_all_tool_outputs_compacted(&self, max_bytes: usize) -> (Self, usize) {
        let mut messages = self.messages.clone();
        let mut compacted = 0;
        for message in &mut messages {
            let Message::User { content } = message else {
                continue;
            };
            for item in content.iter_mut() {
                let MessageContent::ToolResult { content, .. } = item else {
                    continue;
                };
                for part in content.iter_mut() {
                    let ToolResultContent::Text { text } = part else {
                        continue;
                    };
                    let original_len = text.len();
                    // Always re-compact, even if already marked
                    *text = compact_text(text, max_bytes, "tool output compacted");
                    if text.len() < original_len {
                        compacted += 1;
                    }
                }
            }
        }
        (
            Self {
                summary: self.summary.clone(),
                messages,
            },
            compacted,
        )
    }

    pub fn deterministically_compacted(
        &self,
        recent_messages: usize,
        summary_bytes: usize,
    ) -> Option<(Self, CompactionStats)> {
        if self.messages.len() <= 1 {
            return None;
        }
        let protected = recent_messages.max(1).min(self.messages.len() - 1);
        let keep_from = self.valid_keep_from(self.messages.len() - protected);
        if keep_from == 0 {
            return None;
        }
        let removed_messages = keep_from;
        Some((
            self.with_truncation_note(keep_from, summary_bytes),
            CompactionStats {
                removed_messages,
                compacted_tools: 0,
                summarized: true,
            },
        ))
    }

    pub fn to_messages(&self) -> Vec<Message> {
        let mut messages = Vec::new();
        if let Some(summary) = self.summary.as_ref().filter(|s| !s.trim().is_empty()) {
            messages.push(Message::user_text(format!(
                "[Compacted earlier conversation]\n{}",
                summary.trim()
            )));
        }
        messages.extend(self.messages.clone());
        messages
    }

    pub fn request_preamble(&self, system_prompt: &str, tool_context: &ToolContext) -> String {
        let mut prompt = system_prompt.to_string();
        if !tool_context.todos().is_empty() {
            let header = config::session_text_value("transcript", "todo_system")
                .unwrap_or_else(|_| String::from("{todos}"));
            let todos = crate::tools::format_todos(tool_context.todos());
            prompt.push_str("\n\n");
            prompt.push_str(header.replace("{todos}", todos.trim_end()).trim());
        }
        prompt
    }

    fn valid_keep_from(&self, requested: usize) -> usize {
        let mut keep_from = requested.min(self.messages.len());
        while keep_from < self.messages.len() && !is_user_prompt(&self.messages[keep_from]) {
            keep_from += 1;
        }
        keep_from
    }

    fn with_truncation_note(&self, keep_from: usize, summary_bytes: usize) -> Self {
        let note = format!(
            "[context truncated] Removed {keep_from} older messages; retained the most recent conversation window."
        );
        let merged = self
            .summary
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .map(|summary| format!("{}\n\n{}", summary.trim(), note))
            .unwrap_or(note);
        Self {
            summary: Some(compact_text(&merged, summary_bytes, "truncated summary")),
            messages: self.messages[keep_from.min(self.messages.len())..].to_vec(),
        }
    }
}

fn is_user_prompt(message: &Message) -> bool {
    let Message::User { content } = message else {
        return false;
    };
    content
        .iter()
        .any(|item| matches!(item, MessageContent::Text { text, .. } if !text.trim().is_empty()))
}
