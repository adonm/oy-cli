//! Session orchestration: context management, tool loop driving,
//! compaction triggers, and saved-session persistence.
//!
//! [`Session`] is the main state machine — it manages transcript
//! messages, runs the chat/tool loop, and signals compaction when
//! the context budget is exceeded.

use anyhow::Result;
use chrono::Utc;

use super::compaction::count_tokens;
pub use super::transcript::{CompactionStats, ContextBudgetExceeded, ContextStatus, Transcript};
use crate::config::{self, SafetyMode, SessionFile};

mod storage;
pub use storage::load_saved;

use crate::llm::Message;
use crate::model;
use crate::tools::{TodoItem, TodoStatus, ToolContext, ToolPolicy};
use std::sync::Arc;
use tokio::sync::Mutex;

const DEFAULT_MAX_TOOL_ROUNDS: usize = 512;

fn display_model(model_spec: &str) -> &str {
    model_spec
        .rsplit_once("::")
        .map(|(_, model)| model)
        .unwrap_or(model_spec)
}

fn token_count_text(count: usize) -> String {
    if count < 1000 {
        format!("{count} tok")
    } else {
        format!("{:.1}k tok", count as f64 / 1000.0)
    }
}

fn tokens_to_compaction_bytes(tokens: usize) -> usize {
    tokens.saturating_mul(4).max(512)
}

fn context_config_for_model(model_spec: &str) -> config::ContextConfig {
    let limits = crate::agent::model::model_limits(model_spec);
    let input_limit = limits.map(|l| l.input.unwrap_or(l.context));
    let output_limit = limits.and_then(|l| if l.output > 0 { Some(l.output) } else { None });
    config::context_config_for_model(input_limit, output_limit)
}

#[derive(Debug, Clone)]
pub struct Session {
    pub root: std::path::PathBuf,
    pub model: String,
    pub system_prompt: String,
    pub interactive: bool,
    pub mode: SafetyMode,
    pub transcript: Transcript,
    pub todos: Vec<TodoItem>,
}

impl Session {
    pub fn new(
        root: std::path::PathBuf,
        model: String,
        interactive: bool,
        mode: SafetyMode,
    ) -> Self {
        let system_prompt = config::system_prompt(interactive, mode);
        Self {
            root,
            model,
            system_prompt,
            interactive,
            mode,
            transcript: Transcript::new(),
            todos: Vec::new(),
        }
    }

    pub fn policy(&self) -> ToolPolicy {
        self.mode.policy()
    }

    pub fn restarted(&self) -> Self {
        Self::new(
            self.root.clone(),
            self.model.clone(),
            self.interactive,
            self.mode,
        )
    }

    pub fn tool_context(&self) -> ToolContext {
        ToolContext::new(
            self.root.clone(),
            self.interactive,
            self.policy(),
            self.todos.clone(),
        )
    }

    fn wait_status(&self, model_spec: &str) -> String {
        let estimate = self
            .transcript
            .token_estimate(model_spec, &self.system_prompt, &self.todos);
        let mut parts = vec![
            "oy".to_string(),
            display_model(model_spec).to_string(),
            token_count_text(estimate.total_tokens),
            format!("{} msg", estimate.messages),
        ];
        if !self.todos.is_empty() {
            let active = self
                .todos
                .iter()
                .filter(|item| item.status != TodoStatus::Done)
                .count();
            parts.push(format!("{active}/{} todo", self.todos.len()));
        }
        parts.join(" · ")
    }

    pub fn context_status(&self) -> ContextStatus {
        let model_spec = self.model.trim().to_string();
        let config = context_config_for_model(&model_spec);
        ContextStatus {
            estimate: self
                .transcript
                .token_estimate(&model_spec, &self.system_prompt, &self.todos),
            limit_tokens: config.limit_tokens,
            input_budget_tokens: config.input_budget_tokens(),
            trigger_tokens: config.trigger_tokens(),
            summary_present: self.transcript.summary.is_some(),
        }
    }

    pub fn compact_deterministic(&mut self) -> Option<CompactionStats> {
        let config = context_config_for_model(&self.model);
        let (base, mut stats) = self
            .transcript
            .deterministically_compacted(
                config.recent_messages,
                tokens_to_compaction_bytes(config.summary_tokens),
            )
            .map_or_else(
                || (self.transcript.clone(), None),
                |(transcript, stats)| (transcript, Some(stats)),
            );
        let (transcript, compacted_tools) =
            base.with_compacted_tool_outputs(tokens_to_compaction_bytes(config.tool_output_tokens));
        if compacted_tools > 0 {
            match stats.as_mut() {
                Some(stats) => stats.compacted_tools = compacted_tools,
                None => {
                    stats = Some(CompactionStats {
                        removed_messages: 0,
                        compacted_tools,
                        summarized: false,
                    });
                }
            }
        }
        if stats.is_some() {
            self.transcript = transcript;
        }
        stats
    }

    pub fn save(&self, name: Option<&str>) -> Result<std::path::PathBuf> {
        let payload = SessionFile {
            model: self.model.clone(),
            saved_at: Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            workspace_root: self.root.clone(),
            mode: Some(self.mode),
            transcript: serde_json::to_value(&self.transcript)?,
            todos: self.todos.clone(),
        };
        config::save_session_file(name, &payload)
    }
}

async fn ensure_context_budget(session: &mut Session, model_spec: &str) -> Result<()> {
    let config = context_config_for_model(model_spec);
    let estimate =
        session
            .transcript
            .token_estimate(model_spec, &session.system_prompt, &session.todos);
    if estimate.total_tokens <= config.trigger_tokens() {
        return Ok(());
    }

    if let Some(stats) = session.compact_deterministic()
        && !crate::ui::is_quiet()
    {
        crate::ui::err_line(format_args!(
            "compacted context: {} old messages, {} tool outputs",
            stats.removed_messages, stats.compacted_tools
        ));
    }

    let estimate =
        session
            .transcript
            .token_estimate(model_spec, &session.system_prompt, &session.todos);
    if estimate.total_tokens > config.input_budget_tokens() {
        return Err(ContextBudgetExceeded {
            estimated_tokens: estimate.total_tokens,
            input_budget_tokens: config.input_budget_tokens(),
            limit_tokens: config.limit_tokens,
        }
        .into());
    }
    Ok(())
}

pub async fn run_prompt(session: &mut Session, prompt: &str) -> Result<String> {
    run_prompt_with_policy(session, prompt, None).await
}

pub async fn run_prompt_read_only(session: &mut Session, prompt: &str) -> Result<String> {
    run_prompt_with_policy(session, prompt, Some(ToolPolicy::read_only())).await
}

pub async fn run_prompt_once_no_tools(
    model: &str,
    system_prompt: &str,
    prompt: &str,
) -> Result<String> {
    let model_spec = model.trim().to_string();
    let _title = crate::ui::title_scope(format_args!(
        "oy · {} · no tools",
        display_model(&model_spec)
    ));
    if !crate::ui::is_quiet() {
        let tokens = count_tokens(&model_spec, system_prompt) + count_tokens(&model_spec, prompt);
        crate::ui::err_line(format_args!(
            "oy · {} · {} · no tools",
            display_model(&model_spec),
            token_count_text(tokens)
        ));
    }
    let response = model::exec_chat(
        &model_spec,
        system_prompt,
        vec![Message::user_text(prompt)],
        Vec::new(),
        Vec::new(),
        config::max_tool_rounds(DEFAULT_MAX_TOOL_ROUNDS),
    )
    .await?;
    Ok(response.output)
}

async fn run_prompt_with_policy(
    session: &mut Session,
    prompt: &str,
    policy_override: Option<ToolPolicy>,
) -> Result<String> {
    session.transcript.messages.push(Message::user_text(prompt));
    let max_tool_rounds = config::max_tool_rounds(DEFAULT_MAX_TOOL_ROUNDS);

    let mut tool_context = session.tool_context();
    if let Some(policy) = policy_override {
        tool_context.env.policy = policy;
    }
    let tool_context = Arc::new(Mutex::new(tool_context));
    let model_spec = session.model.trim().to_string();
    ensure_context_budget(session, &model_spec).await?;
    let preamble = {
        let ctx = tool_context.lock().await;
        session
            .transcript
            .request_preamble(&session.system_prompt, &ctx)
    };
    let messages = session.transcript.to_messages();
    let tool_specs = {
        let ctx = tool_context.lock().await;
        crate::tools::tool_specs(&ctx)
    };
    let llm_tools = crate::tools::llm_tools(tool_context.clone()).await;
    crate::ui::title_progress(session.wait_status(&model_spec));
    if !crate::ui::is_quiet() {
        crate::ui::err_line(format_args!("{}", session.wait_status(&model_spec)));
    }
    let response = model::exec_chat(
        &model_spec,
        &preamble,
        messages,
        tool_specs,
        llm_tools,
        max_tool_rounds,
    )
    .await?;
    session.todos = tool_context.lock().await.todos().to_vec();
    if let Some(messages) = response.messages {
        session.transcript.messages.extend(messages);
    } else {
        session
            .transcript
            .messages
            .push(Message::assistant_text(response.output.clone()));
    }
    Ok(response.output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Approval, ToolPolicy};

    #[test]
    fn session_policy_is_derived_from_mode() {
        let session = Session::new(
            std::path::PathBuf::from("."),
            "model".into(),
            false,
            SafetyMode::Plan,
        );

        assert_eq!(session.policy(), ToolPolicy::read_only());

        let restarted = session.restarted();
        assert_eq!(restarted.mode, SafetyMode::Plan);
        assert_eq!(restarted.policy(), ToolPolicy::read_only());
    }

    #[test]
    fn tool_context_uses_derived_session_policy() {
        let session = Session::new(
            std::path::PathBuf::from("."),
            "model".into(),
            false,
            SafetyMode::AutoEdits,
        );

        assert_eq!(
            session.tool_context().policy().files_write(),
            Approval::Auto
        );
        assert_eq!(session.tool_context().policy().shell, Approval::Ask);
    }
}
