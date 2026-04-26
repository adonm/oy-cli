use anyhow::{Context, Result, bail};
use chrono::Utc;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ReasoningEffort, ToolCall, ToolResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use tiktoken_rs::{bpe_for_model, cl100k_base};

use crate::config::{self, SessionFile};

const MAX_TOOL_ROUNDS: usize = 640;
use crate::model;
use crate::tools::{TodoItem, ToolContext, ToolPolicy};

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
    pub auto_compact: bool,
    pub summary_present: bool,
}

fn model_tokenizer_name(model: &str) -> &str {
    model
        .rsplit_once("::")
        .map(|(_, name)| name)
        .unwrap_or(model)
}

fn count_tokens(model: &str, text: &str) -> usize {
    let model_name = model_tokenizer_name(model);
    if let Ok(bpe) = bpe_for_model(model_name) {
        return bpe.encode_with_special_tokens(text).len();
    }
    cl100k_base()
        .ok()
        .map(|bpe| bpe.encode_with_special_tokens(text).len())
        .unwrap_or_else(|| text.split_whitespace().count())
}

fn take_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn take_last_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars().rev().take(max_chars).collect::<Vec<_>>();
    chars.reverse();
    chars.into_iter().collect()
}

fn compact_text(text: &str, model: &str, max_tokens: usize, label: &str) -> String {
    if count_tokens(model, text) <= max_tokens {
        return text.to_string();
    }
    let target_chars = max_tokens.saturating_mul(3).max(512);
    let half = target_chars / 2;
    let head = take_chars(text, half);
    let tail = take_last_chars(text, half);
    format!(
        "[{label}] original ~{} tokens, {} bytes. Preserved head/tail.\n\n--- head ---\n{}\n\n--- tail ---\n{}",
        count_tokens(model, text),
        text.len(),
        head.trim_end(),
        tail.trim_start()
    )
}

fn message_label(message: &StoredMessage) -> &'static str {
    match message {
        StoredMessage::User { .. } => "user",
        StoredMessage::Summary { .. } => "summary",
        StoredMessage::Assistant { .. } => "assistant",
        StoredMessage::AssistantToolCalls { .. } => "assistant_tool_calls",
        StoredMessage::Tool { .. } => "tool",
    }
}

impl StoredMessage {
    fn content_text(&self) -> String {
        match self {
            StoredMessage::User { content }
            | StoredMessage::Summary { content }
            | StoredMessage::Assistant { content } => content.clone(),
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
    pub policy: ToolPolicy,
    pub agent: String,
    pub transcript: Transcript,
    pub todos: Vec<TodoItem>,
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            summary: None,
            messages: Vec::new(),
        }
    }

    fn valid_compaction_keep_from(&self, requested: usize) -> usize {
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
                .map(|message| 4 + count_text(&message.content_text()))
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

    fn rebuild_with_summary(&mut self, summary: String, keep_from: usize) {
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
                StoredMessage::Assistant { content } => {
                    req = req.append_message(ChatMessage::assistant(content.clone()))
                }
                StoredMessage::AssistantToolCalls { tool_calls } => {
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
                        req = req.append_message(ChatMessage::assistant(calls));
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

impl Session {
    pub fn new(
        root: std::path::PathBuf,
        model: String,
        interactive: bool,
        agent: String,
        policy: ToolPolicy,
    ) -> Self {
        let system_prompt = session_system_prompt(&root, interactive, &agent);
        Self {
            root,
            model,
            system_prompt,
            interactive,
            policy,
            agent,
            transcript: Transcript::new(),
            todos: Vec::new(),
        }
    }

    pub fn tool_context(&self) -> ToolContext {
        ToolContext {
            root: self.root.clone(),
            interactive: self.interactive,
            policy: self.policy,
            todos: self.todos.clone(),
        }
    }

    fn chat_options(&self) -> Option<ChatOptions> {
        model::reasoning_effort_option(&self.model)
            .map(|effort| ChatOptions::default().with_reasoning_effort(reasoning_effort(effort)))
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
        if let Some(effort) = model::default_reasoning_effort(model_spec) {
            parts.push(format!("think {effort}"));
        }
        if !self.todos.is_empty() {
            let active = self
                .todos
                .iter()
                .filter(|item| item.status != "done")
                .count();
            parts.push(format!("{active}/{} todo", self.todos.len()));
        }
        parts.join(" · ")
    }

    pub fn context_status(&self) -> ContextStatus {
        let model_spec = model::to_genai_model_spec(&self.model);
        let config = config::context_config();
        ContextStatus {
            estimate: self
                .transcript
                .token_estimate(&model_spec, &self.system_prompt, &self.todos),
            limit_tokens: config.limit_tokens,
            input_budget_tokens: config.input_budget_tokens(),
            trigger_tokens: config.trigger_tokens(),
            auto_compact: config.auto_compact,
            summary_present: self.transcript.summary.is_some(),
        }
    }

    pub fn compact_deterministic(&mut self) -> Option<CompactionStats> {
        let config = config::context_config();
        let model_spec = model::to_genai_model_spec(&self.model);
        let before = self
            .transcript
            .token_estimate(&model_spec, &self.system_prompt, &self.todos);
        let compacted_tools = self
            .transcript
            .compact_tool_outputs(&model_spec, config.tool_output_tokens);
        let mut stats = self.transcript.deterministic_compact_old_turns(
            &model_spec,
            &self.system_prompt,
            &self.todos,
            config.input_budget_tokens(),
            config.recent_messages,
            config.summary_tokens,
        );
        if compacted_tools > 0 {
            let after =
                self.transcript
                    .token_estimate(&model_spec, &self.system_prompt, &self.todos);
            match stats.as_mut() {
                Some(stats) => stats.compacted_tools = compacted_tools,
                None => {
                    stats = Some(CompactionStats {
                        before_tokens: before.total_tokens,
                        after_tokens: after.total_tokens,
                        removed_messages: 0,
                        compacted_tools,
                        summarized: false,
                    });
                }
            }
        }
        stats
    }

    pub async fn compact_llm(&mut self) -> Result<Option<CompactionStats>> {
        compact_llm_session(self, true).await
    }

    pub fn save(&self, name: Option<&str>) -> Result<std::path::PathBuf> {
        let payload = SessionFile {
            model: self.model.clone(),
            agent: self.agent.clone(),
            saved_at: Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            transcript: serde_json::to_value(&self.transcript)?,
            todos: self.todos.clone(),
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
    let system_prompt = session_system_prompt(&root, interactive, &saved.agent);
    Ok(Some(Session {
        root,
        model: saved.model,
        system_prompt,
        interactive,
        policy: config::tool_policy(&saved.agent),
        agent: saved.agent,
        transcript,
        todos: saved.todos,
    }))
}

fn deterministic_summary(messages: &[StoredMessage], model: &str, max_tokens: usize) -> String {
    let mut out = String::from(
        "This summary was produced deterministically to fit the context budget. Prefer exact recent messages that follow over this summary.\n\n",
    );
    let per_message = (max_tokens / messages.len().max(1)).clamp(128, 1024);
    for (idx, message) in messages.iter().enumerate() {
        let text = message.content_text();
        out.push_str(&format!(
            "## {} {} (~{} tokens)\n",
            idx + 1,
            message_label(message),
            count_tokens(model, &text)
        ));
        match message {
            StoredMessage::AssistantToolCalls { tool_calls } => {
                for call in tool_calls {
                    out.push_str(&format!(
                        "- tool call `{}` args: {}\n",
                        call.fn_name, call.fn_arguments
                    ));
                }
            }
            StoredMessage::Tool { call_id, .. } => {
                out.push_str(&format!("call_id: `{call_id}`\n"));
                out.push_str(&compact_text(
                    &text,
                    model,
                    per_message,
                    "old tool output summarized",
                ));
                out.push('\n');
            }
            _ => {
                out.push_str(&compact_text(
                    &text,
                    model,
                    per_message,
                    "old message summarized",
                ));
                out.push('\n');
            }
        }
        out.push('\n');
    }
    compact_text(&out, model, max_tokens, "deterministic transcript summary")
}

fn transcript_for_summary(messages: &[StoredMessage], model: &str, max_tokens: usize) -> String {
    let mut out = String::new();
    let per_message = (max_tokens / messages.len().max(1)).clamp(256, 2048);
    for (idx, message) in messages.iter().enumerate() {
        let text = message.content_text();
        out.push_str(&format!(
            "\n<message index=\"{}\" role=\"{}\">\n{}\n</message>\n",
            idx + 1,
            message_label(message),
            compact_text(
                &text,
                model,
                per_message,
                "message pre-truncated for summarization"
            )
        ));
    }
    compact_text(
        &out,
        model,
        max_tokens,
        "transcript pre-truncated for summarization",
    )
}

fn has_following_tool_response(messages: &[StoredMessage], call_id: &str) -> bool {
    for message in messages {
        match message {
            StoredMessage::Tool { call_id: id, .. } if id == call_id => return true,
            StoredMessage::Tool { .. } => continue,
            _ => return false,
        }
    }
    false
}

fn compaction_prompt(
    existing_summary: Option<&str>,
    messages: &[StoredMessage],
    model: &str,
) -> String {
    let prior = existing_summary.unwrap_or("");
    let transcript = transcript_for_summary(messages, model, 48_000);
    format!(
        r#"You are compacting a coding-agent transcript so future requests stay under a context limit.

Preserve facts needed to continue work:
- user goals, constraints, preferences, and explicit instructions
- exact filenames, commands, APIs, errors, test results, and config/env names
- decisions made and rationale when important
- tool results that affect next actions
- changes already made
- active todos/current plan/open questions

Prefer preserving human input over assistant prose. Drop filler, repeated logs, and irrelevant verbose output. Do not invent facts.

Return concise markdown with sections:
## User intent
## Constraints
## Repo facts
## Changes made
## Commands/results
## Current plan
## Open issues

Existing summary, if any:
{prior}

Transcript to compact:
{transcript}
"#
    )
}

fn session_system_prompt(root: &Path, interactive: bool, agent: &str) -> String {
    let mut prompt = config::active_system_prompt(interactive, agent);
    if let Some(snapshot) = crate::tools::compact_workspace_snapshot(root) {
        prompt.push_str("\n\n");
        prompt.push_str(&snapshot);
    }
    prompt
}

fn model_suffix(model_spec: &str) -> &str {
    model_spec
        .rsplit_once("::")
        .map(|(_, model)| model)
        .unwrap_or(model_spec)
}

fn reasoning_effort(value: &str) -> ReasoningEffort {
    value
        .parse::<ReasoningEffort>()
        .unwrap_or(ReasoningEffort::High)
}

fn format_tokens(count: usize) -> String {
    if count < 1000 {
        format!("{count} tok")
    } else {
        format!("{:.1}k tok", count as f64 / 1000.0)
    }
}

async fn ensure_context_budget(
    session: &mut Session,
    client: &genai::Client,
    model_spec: &str,
) -> Result<()> {
    let config = config::context_config();
    let estimate =
        session
            .transcript
            .token_estimate(model_spec, &session.system_prompt, &session.todos);
    if estimate.total_tokens <= config.trigger_tokens() {
        return Ok(());
    }
    if !config.auto_compact {
        if estimate.total_tokens > config.input_budget_tokens() {
            bail!(
                "context estimate {} exceeds input budget {}; enable OY_AUTO_COMPACT or use /compact",
                estimate.total_tokens,
                config.input_budget_tokens()
            );
        }
        return Ok(());
    }

    if let Some(stats) = session.compact_deterministic() {
        if !crate::ui::is_quiet() {
            crate::ui::err_line(format_args!(
                "compacted context: {} -> {} tokens ({} old messages, {} tool outputs)",
                stats.before_tokens,
                stats.after_tokens,
                stats.removed_messages,
                stats.compacted_tools
            ));
        }
    }

    let estimate =
        session
            .transcript
            .token_estimate(model_spec, &session.system_prompt, &session.todos);
    if estimate.total_tokens <= config.input_budget_tokens() {
        return Ok(());
    }

    if let Some(stats) = compact_llm_session_with_client(session, client, model_spec, false).await?
    {
        if !crate::ui::is_quiet() {
            crate::ui::err_line(format_args!(
                "summarized context: {} -> {} tokens ({} old messages)",
                stats.before_tokens, stats.after_tokens, stats.removed_messages
            ));
        }
    }

    let estimate =
        session
            .transcript
            .token_estimate(model_spec, &session.system_prompt, &session.todos);
    if estimate.total_tokens > config.input_budget_tokens() {
        bail!(
            "context estimate {} still exceeds input budget {} after compaction; current prompt/system may be too large",
            estimate.total_tokens,
            config.input_budget_tokens()
        );
    }
    Ok(())
}

async fn compact_llm_session(
    session: &mut Session,
    force: bool,
) -> Result<Option<CompactionStats>> {
    let client = model::build_client()?;
    let model_spec = model::to_genai_model_spec(&session.model);
    compact_llm_session_with_client(session, &client, &model_spec, force).await
}

async fn compact_llm_session_with_client(
    session: &mut Session,
    client: &genai::Client,
    model_spec: &str,
    force: bool,
) -> Result<Option<CompactionStats>> {
    let config = config::context_config();
    let before =
        session
            .transcript
            .token_estimate(model_spec, &session.system_prompt, &session.todos);
    if !force && before.total_tokens <= config.input_budget_tokens() {
        return Ok(None);
    }
    if session.transcript.messages.len() <= 1 {
        return Ok(None);
    }

    let protected = config
        .recent_messages
        .max(1)
        .min(session.transcript.messages.len() - 1);
    let keep_from = session
        .transcript
        .valid_compaction_keep_from(session.transcript.messages.len() - protected);
    if keep_from == 0 {
        return Ok(None);
    }

    let removed = session.transcript.messages[..keep_from].to_vec();
    let prompt = compaction_prompt(session.transcript.summary.as_deref(), &removed, model_spec);
    let req = ChatRequest::default()
        .with_system(
            "You compact coding-agent transcripts. Return only the compacted markdown summary.",
        )
        .append_message(ChatMessage::user(prompt));
    let options = session.chat_options();
    let response = client.exec_chat(model_spec, req, options.as_ref()).await?;
    let mut summary = response.into_first_text().unwrap_or_default();
    if summary.trim().is_empty() {
        summary = deterministic_summary(&removed, model_spec, config.summary_tokens);
    } else if count_tokens(model_spec, &summary) > config.summary_tokens {
        summary = compact_text(
            &summary,
            model_spec,
            config.summary_tokens,
            "llm summary compacted",
        );
    }

    let removed_messages = removed.len();
    session.transcript.rebuild_with_summary(summary, keep_from);
    let after =
        session
            .transcript
            .token_estimate(model_spec, &session.system_prompt, &session.todos);
    Ok(Some(CompactionStats {
        before_tokens: before.total_tokens,
        after_tokens: after.total_tokens,
        removed_messages,
        compacted_tools: 0,
        summarized: true,
    }))
}

#[derive(Default)]
struct RepeatedNoopTools {
    seen: BTreeSet<String>,
}

impl RepeatedNoopTools {
    fn record(&mut self, name: &str, args: &Value, result: &Value) -> Result<()> {
        if !is_noop_tool_result(name, result) {
            self.seen.clear();
            return Ok(());
        }
        let key = format!(
            "{}:{}",
            name,
            serde_json::to_string(args).unwrap_or_default()
        );
        if !self.seen.insert(key) {
            bail!(
                "tool loop made no progress: repeated no-op {name}; inspect the latest tool output and choose a different action"
            )
        }
        Ok(())
    }
}

fn is_noop_tool_result(name: &str, result: &Value) -> bool {
    match name {
        "replace" => {
            result.get("replacement_count").and_then(Value::as_u64) == Some(0)
                && result
                    .get("changed_file_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    == 0
                && result
                    .get("errors")
                    .and_then(Value::as_array)
                    .is_none_or(Vec::is_empty)
        }
        _ => false,
    }
}

pub async fn run_prompt(session: &mut Session, prompt: &str) -> Result<String> {
    let client = model::build_client()?;
    session.transcript.messages.push(StoredMessage::User {
        content: prompt.to_string(),
    });
    let mut repeated_noop_tools = RepeatedNoopTools::default();

    for tool_round in 0..=MAX_TOOL_ROUNDS {
        if tool_round == MAX_TOOL_ROUNDS {
            bail!("tool loop exceeded {MAX_TOOL_ROUNDS} rounds; try a narrower prompt");
        }
        let tool_context = session.tool_context();
        let tool_specs = crate::tools::tool_specs(&tool_context);
        let model_spec = model::to_genai_model_spec(&session.model);
        ensure_context_budget(session, &client, &model_spec).await?;
        let req = session
            .transcript
            .to_chat_request(&session.system_prompt, &tool_context)
            .with_tools(tool_specs.clone());
        if !crate::ui::is_quiet() {
            crate::ui::err_line(format_args!("{}", session.wait_status(&model_spec)));
        }
        let options = session.chat_options();
        let response = client.exec_chat(&model_spec, req, options.as_ref()).await?;
        let tool_calls = response
            .tool_calls()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if !tool_calls.is_empty() {
            crate::ui::tool_batch(tool_round + 1, tool_calls.len());
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
                repeated_noop_tools.record(&call.fn_name, &call.fn_arguments, &result)?;
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
    bail!("tool loop exceeded {MAX_TOOL_ROUNDS} rounds; try a narrower prompt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_noop_tools_rejects_repeated_zero_replace() {
        let mut guard = RepeatedNoopTools::default();
        let args = json!({"path": "src/main.rs", "pattern": "missing", "replacement": "x"});
        let result = json!({
            "changed_file_count": 0,
            "replacement_count": 0,
            "errors": []
        });

        guard.record("replace", &args, &result).unwrap();
        let err = guard.record("replace", &args, &result).unwrap_err();

        assert!(err.to_string().contains("repeated no-op replace"));
    }

    #[test]
    fn repeated_noop_tools_allows_retry_after_progress() {
        let mut guard = RepeatedNoopTools::default();
        let args = json!({"path": "src/main.rs", "pattern": "missing", "replacement": "x"});
        let noop = json!({
            "changed_file_count": 0,
            "replacement_count": 0,
            "errors": []
        });
        let progress = json!({
            "changed_file_count": 1,
            "replacement_count": 1,
            "errors": []
        });

        guard.record("replace", &args, &noop).unwrap();
        guard.record("replace", &args, &progress).unwrap();

        assert!(guard.record("replace", &args, &noop).is_ok());
    }

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
                },
                StoredMessage::User {
                    content: "recent user".into(),
                },
                StoredMessage::Assistant {
                    content: "recent assistant".into(),
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
    fn chat_request_drops_orphan_tool_messages() {
        let tx = Transcript {
            summary: None,
            messages: vec![
                StoredMessage::Tool {
                    call_id: "missing-call".into(),
                    content: "orphan".into(),
                },
                StoredMessage::AssistantToolCalls {
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
