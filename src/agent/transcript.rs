use genai::chat::{ChatMessage, ChatRequest, ToolCall, ToolResponse};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::compaction::{
    compact_text, count_tokens, deterministic_summary, has_following_tool_response,
    message_content_text, msg_reasoning_content,
};
use crate::config;
use crate::tools::{TodoItem, ToolContext};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    #[serde(default)]
    pub summary: Option<String>,
    pub messages: Vec<StoredMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum StoredMessage {
    #[serde(rename = "user")]
    User { content: String },
    #[serde(rename = "summary")]
    Summary { content: String },
    #[serde(rename = "assistant")]
    Assistant {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
    },
    #[serde(rename = "assistant_tool_calls")]
    AssistantToolCalls {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
        tool_calls: Vec<StoredToolCall>,
    },
    #[serde(rename = "tool")]
    Tool { call_id: String, content: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToolCall {
    pub call_id: String,
    pub fn_name: String,
    pub fn_arguments: Value,
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
    pub before_tokens: usize,
    pub after_tokens: usize,
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

    pub(super) fn valid_compaction_keep_from(&self, requested: usize) -> usize {
        let mut keep_from = requested.min(self.messages.len());
        while matches!(
            self.messages.get(keep_from),
            Some(StoredMessage::Tool { .. })
        ) {
            keep_from += 1;
        }
        keep_from
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

    pub fn force_truncate_oldest_turns(&mut self) -> usize {
        if self.messages.len() <= 1 {
            return 0;
        }
        let remove_count = (self.messages.len() / 4)
            .max(1)
            .min(self.messages.len() - 1);
        let keep_from = self.valid_truncation_keep_from(remove_count);
        if keep_from == 0 || keep_from >= self.messages.len() {
            return 0;
        }
        self.messages.drain(..keep_from);
        keep_from
    }

    fn valid_truncation_keep_from(&self, requested: usize) -> usize {
        let mut keep_from = self.valid_compaction_keep_from(requested);
        while keep_from < self.messages.len()
            && !matches!(self.messages[keep_from], StoredMessage::User { .. })
        {
            keep_from += 1;
        }
        keep_from
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
            messages: self.message_count(),
            system_tokens,
            message_tokens,
            total_tokens: system_tokens + message_tokens,
        }
    }

    fn message_count(&self) -> usize {
        self.messages.len() + usize::from(self.summary.is_some())
    }

    pub fn compact_tool_outputs(&mut self, model: &str, max_tokens: usize) -> usize {
        let mut compacted = 0;
        for message in &mut self.messages {
            let StoredMessage::Tool { content, .. } = message else {
                continue;
            };
            if count_tokens(model, content) <= max_tokens
                || content.contains("[tool output compacted]")
            {
                continue;
            }
            *content = compact_text(content, model, max_tokens, "tool output compacted");
            compacted += 1;
        }
        compacted
    }

    pub(super) fn rebuild_with_summary(&mut self, summary: String, keep_from: usize) {
        let existing = self.summary.take();
        let mut merged = String::from("[compacted conversation summary]\n");
        if let Some(existing) = existing.filter(|s| !s.trim().is_empty()) {
            merged.push_str(existing.trim());
            merged.push_str("\n\n[latest compaction]\n");
        }
        merged.push_str(summary.trim());
        self.summary = Some(merged);
        self.messages = self.messages.split_off(keep_from.min(self.messages.len()));
    }

    pub fn deterministic_compact_old_turns(
        &mut self,
        model: &str,
        system_prompt: &str,
        todos: &[TodoItem],
        budget: usize,
        recent_messages: usize,
        summary_tokens: usize,
    ) -> Option<CompactionStats> {
        let before = self.token_estimate(model, system_prompt, todos);
        if before.total_tokens <= budget || self.messages.len() <= 1 {
            return None;
        }
        let protected = recent_messages.max(1).min(self.messages.len() - 1);
        let keep_from = self.valid_compaction_keep_from(self.messages.len() - protected);
        if keep_from == 0 {
            return None;
        }
        let removed = self.messages[..keep_from].to_vec();
        let summary = deterministic_summary(&removed, model, summary_tokens);
        let removed_messages = removed.len();
        self.rebuild_with_summary(summary, keep_from);
        let after = self.token_estimate(model, system_prompt, todos);
        Some(CompactionStats {
            before_tokens: before.total_tokens,
            after_tokens: after.total_tokens,
            removed_messages,
            compacted_tools: 0,
            summarized: true,
        })
    }

    pub fn to_chat_request(&self, system_prompt: &str, tool_context: &ToolContext) -> ChatRequest {
        let mut req = ChatRequest::default().with_system(system_prompt);
        let mut pending_tool_call_ids: Vec<String> = Vec::new();
        if let Some(summary) = self.summary.as_ref().filter(|s| !s.trim().is_empty()) {
            req = req.append_message(ChatMessage::user(format!(
                "[Compacted earlier conversation]\n{}",
                summary.trim()
            )));
        }
        for (index, msg) in self.messages.iter().enumerate() {
            match msg {
                StoredMessage::User { content } => {
                    req = req.append_message(ChatMessage::user(content.clone()))
                }
                StoredMessage::Summary { content } => {
                    req = req.append_message(ChatMessage::user(content.clone()))
                }
                StoredMessage::Assistant { content, .. } => {
                    let reasoning_content = msg_reasoning_content(msg);
                    req = req.append_message(
                        ChatMessage::assistant(content.clone())
                            .with_reasoning_content(reasoning_content),
                    )
                }
                StoredMessage::AssistantToolCalls { tool_calls, .. } => {
                    let calls = tool_calls
                        .iter()
                        .filter(|call| {
                            has_following_tool_response(&self.messages[index + 1..], &call.call_id)
                        })
                        .map(|call| {
                            pending_tool_call_ids.push(call.call_id.clone());
                            ToolCall {
                                call_id: call.call_id.clone(),
                                fn_name: call.fn_name.clone(),
                                fn_arguments: call.fn_arguments.clone(),
                                thought_signatures: None,
                            }
                        })
                        .collect::<Vec<_>>();
                    if !calls.is_empty() {
                        let reasoning_content = msg_reasoning_content(msg);
                        req = req.append_message(
                            ChatMessage::assistant(calls).with_reasoning_content(reasoning_content),
                        );
                    }
                }
                StoredMessage::Tool { call_id, content } => {
                    if let Some(position) =
                        pending_tool_call_ids.iter().position(|id| id == call_id)
                    {
                        pending_tool_call_ids.swap_remove(position);
                        req = req.append_message(ChatMessage::from(ToolResponse::new(
                            call_id.clone(),
                            content.clone(),
                        )));
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolPolicy;
    use serde_json::json;

    #[test]
    fn undo_last_turn_removes_user_and_followups() {
        let mut tx = Transcript {
            summary: None,
            messages: vec![
                StoredMessage::User {
                    content: "one".into(),
                },
                StoredMessage::Assistant {
                    content: "two".into(),
                    reasoning_content: None,
                },
                StoredMessage::User {
                    content: "three".into(),
                },
                StoredMessage::AssistantToolCalls {
                    reasoning_content: None,
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
    fn force_truncate_oldest_turns_removes_old_history_without_orphan_tool() {
        let mut tx = Transcript {
            summary: None,
            messages: vec![
                StoredMessage::User {
                    content: "old user".into(),
                },
                StoredMessage::AssistantToolCalls {
                    reasoning_content: None,
                    tool_calls: vec![StoredToolCall {
                        call_id: "call-1".into(),
                        fn_name: "read".into(),
                        fn_arguments: json!({"path": "src/main.rs"}),
                    }],
                },
                StoredMessage::Tool {
                    call_id: "call-1".into(),
                    content: "tool result".into(),
                },
                StoredMessage::User {
                    content: "new user".into(),
                },
            ],
        };

        assert_eq!(tx.force_truncate_oldest_turns(), 3);
        assert_eq!(tx.messages.len(), 1);
        assert!(matches!(tx.messages[0], StoredMessage::User { .. }));
    }

    #[test]
    fn token_estimate_counts_summary() {
        let mut tx = Transcript::new();
        tx.summary = Some("old user wanted tests".into());
        tx.messages.push(StoredMessage::User {
            content: "new prompt".into(),
        });
        let estimate = tx.token_estimate("gpt-4o", "system", &[]);
        assert_eq!(estimate.messages, 2);
        assert!(estimate.message_tokens > 2);
    }

    #[test]
    fn compact_tool_outputs_preserves_head_and_tail() {
        let mut tx = Transcript {
            summary: None,
            messages: vec![StoredMessage::Tool {
                call_id: "c".into(),
                content: format!("{} middle {}", "a".repeat(10_000), "z".repeat(10_000)),
            }],
        };
        assert_eq!(tx.compact_tool_outputs("gpt-4o", 256), 1);
        let StoredMessage::Tool { content, .. } = &tx.messages[0] else {
            panic!("expected tool message");
        };
        assert!(content.contains("tool output compacted"));
        assert!(content.contains("aaa"));
        assert!(content.contains("zzz"));
    }

    #[test]
    fn deterministic_compaction_keeps_recent_messages() {
        let mut tx = Transcript {
            summary: None,
            messages: vec![
                StoredMessage::User {
                    content: "old user".into(),
                },
                StoredMessage::Assistant {
                    content: "old assistant".into(),
                    reasoning_content: None,
                },
                StoredMessage::User {
                    content: "recent user".into(),
                },
                StoredMessage::Assistant {
                    content: "recent assistant".into(),
                    reasoning_content: None,
                },
            ],
        };
        let stats = tx
            .deterministic_compact_old_turns("gpt-4o", "system", &[], 1, 2, 1024)
            .unwrap();
        assert_eq!(stats.removed_messages, 2);
        assert!(tx.summary.as_deref().unwrap().contains("old user"));
        assert_eq!(tx.messages.len(), 2);
        assert!(matches!(tx.messages[0], StoredMessage::User { .. }));
    }

    #[test]
    fn compaction_does_not_leave_orphan_tool_response() {
        let mut tx = Transcript {
            summary: None,
            messages: vec![
                StoredMessage::User {
                    content: "old user".into(),
                },
                StoredMessage::AssistantToolCalls {
                    reasoning_content: None,
                    tool_calls: vec![StoredToolCall {
                        call_id: "call-1".into(),
                        fn_name: "read".into(),
                        fn_arguments: json!({"path": "src/main.rs"}),
                    }],
                },
                StoredMessage::Tool {
                    call_id: "call-1".into(),
                    content: "tool result".into(),
                },
                StoredMessage::User {
                    content: "latest user".into(),
                },
            ],
        };

        let stats = tx
            .deterministic_compact_old_turns("gpt-4o", "system", &[], 1, 2, 1024)
            .unwrap();

        assert_eq!(stats.removed_messages, 3);
        assert_eq!(tx.messages.len(), 1);
        assert!(matches!(tx.messages[0], StoredMessage::User { .. }));
    }

    #[test]
    fn chat_request_round_trips_assistant_reasoning_content() {
        let tx = Transcript {
            summary: None,
            messages: vec![
                StoredMessage::User {
                    content: "hello".into(),
                },
                StoredMessage::Assistant {
                    content: "answer".into(),
                    reasoning_content: Some("internal reasoning".into()),
                },
            ],
        };
        let ctx = ToolContext {
            root: std::path::PathBuf::new(),
            interactive: false,
            policy: ToolPolicy::read_only(),
            todos: Vec::new(),
        };

        let req = tx.to_chat_request("system", &ctx);

        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[1].content.first_text(), Some("answer"));
        assert_eq!(
            req.messages[1]
                .content
                .joined_reasoning_content()
                .as_deref(),
            Some("internal reasoning")
        );
    }

    #[test]
    fn chat_request_round_trips_tool_call_reasoning_content() {
        let tx = Transcript {
            summary: None,
            messages: vec![
                StoredMessage::AssistantToolCalls {
                    reasoning_content: Some("tool reasoning".into()),
                    tool_calls: vec![StoredToolCall {
                        call_id: "call-1".into(),
                        fn_name: "read".into(),
                        fn_arguments: json!({"path": "src/main.rs"}),
                    }],
                },
                StoredMessage::Tool {
                    call_id: "call-1".into(),
                    content: "tool result".into(),
                },
            ],
        };
        let ctx = ToolContext {
            root: std::path::PathBuf::new(),
            interactive: false,
            policy: ToolPolicy::read_only(),
            todos: Vec::new(),
        };

        let req = tx.to_chat_request("system", &ctx);

        assert_eq!(req.messages.len(), 2);
        assert_eq!(
            req.messages[0]
                .content
                .joined_reasoning_content()
                .as_deref(),
            Some("tool reasoning")
        );
        assert_eq!(req.messages[0].content.tool_calls().len(), 1);
    }

    #[test]
    fn chat_request_drops_orphan_tool_messages() {
        let tx = Transcript {
            summary: None,
            messages: vec![
                StoredMessage::Tool {
                    call_id: "missing-call".into(),
                    content: "orphan".into(),
                },
                StoredMessage::AssistantToolCalls {
                    reasoning_content: None,
                    tool_calls: vec![StoredToolCall {
                        call_id: "no-result".into(),
                        fn_name: "read".into(),
                        fn_arguments: json!({"path": "src/main.rs"}),
                    }],
                },
                StoredMessage::User {
                    content: "continue".into(),
                },
            ],
        };
        let ctx = ToolContext {
            root: std::path::PathBuf::new(),
            interactive: false,
            policy: ToolPolicy::read_only(),
            todos: Vec::new(),
        };

        let req = tx.to_chat_request("system", &ctx);

        assert_eq!(req.messages.len(), 1);
        assert!(
            req.messages[0]
                .content
                .first_text()
                .unwrap()
                .contains("continue")
        );
    }

    #[test]
    fn token_estimate_counts_system_and_messages() {
        let tx = Transcript {
            summary: None,
            messages: vec![
                StoredMessage::User {
                    content: "hello world".into(),
                },
                StoredMessage::Assistant {
                    content: "hi".into(),
                    reasoning_content: None,
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
