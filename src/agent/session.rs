use anyhow::Result;
use chrono::Utc;

use super::compaction::count_tokens;
pub use super::transcript::{CompactionStats, ContextBudgetExceeded, ContextStatus, Transcript};
use crate::config::{self, SafetyMode, SessionFile};

mod storage;
pub use storage::load_saved;

use crate::model;
use crate::tools::{TodoItem, TodoStatus, ToolContext, ToolPolicy};
use std::sync::{Arc, Mutex};

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

#[derive(Debug, Clone)]
pub struct Session {
    pub root: std::path::PathBuf,
    pub model: String,
    pub system_prompt: String,
    pub interactive: bool,
    pub policy: ToolPolicy,
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
        policy: ToolPolicy,
    ) -> Self {
        let system_prompt = config::system_prompt(interactive, mode);
        Self {
            root,
            model,
            system_prompt,
            interactive,
            policy,
            mode,
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
        let limits = crate::agent::model::model_limits(&model_spec);
        let input_limit = limits.map(|l| l.input.unwrap_or(l.context));
        let output_limit = limits.and_then(|l| if l.output > 0 { Some(l.output) } else { None });
        let config = config::context_config_for_model(input_limit, output_limit);
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
        let limits = crate::agent::model::model_limits(&self.model);
        let input_limit = limits.map(|l| l.input.unwrap_or(l.context));
        let output_limit = limits.and_then(|l| if l.output > 0 { Some(l.output) } else { None });
        let config = config::context_config_for_model(input_limit, output_limit);
        let mut stats = self.transcript.deterministic_compact_old_turns(
            config.recent_messages,
            tokens_to_compaction_bytes(config.summary_tokens),
        );
        let compacted_tools = self
            .transcript
            .compact_tool_outputs(tokens_to_compaction_bytes(config.tool_output_tokens));
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
    let limits = crate::agent::model::model_limits(model_spec);
    let input_limit = limits.map(|l| l.input.unwrap_or(l.context));
    let output_limit = limits.and_then(|l| if l.output > 0 { Some(l.output) } else { None });
    let config = config::context_config_for_model(input_limit, output_limit);
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
        vec![rig::completion::Message::user(prompt.to_string())],
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
    session
        .transcript
        .messages
        .push(rig::completion::Message::user(prompt.to_string()));
    let max_tool_rounds = config::max_tool_rounds(DEFAULT_MAX_TOOL_ROUNDS);

    let mut tool_context = session.tool_context();
    if let Some(policy) = policy_override {
        tool_context.policy = policy;
    }
    let tool_context = Arc::new(Mutex::new(tool_context));
    let model_spec = session.model.trim().to_string();
    ensure_context_budget(session, &model_spec).await?;
    let preamble = session.transcript.request_preamble(
        &session.system_prompt,
        &tool_context.lock().expect("tool context mutex poisoned"),
    );
    let turn_start = session.transcript.messages.len().saturating_sub(1);
    let messages = session.transcript.to_messages();
    if !crate::ui::is_quiet() {
        crate::ui::err_line(format_args!("{}", session.wait_status(&model_spec)));
    }
    let response = model::exec_chat(
        &model_spec,
        &preamble,
        messages,
        crate::tools::rig_tools(tool_context.clone()),
        max_tool_rounds,
    )
    .await?;
    session.todos = tool_context
        .lock()
        .expect("tool context mutex poisoned")
        .todos
        .clone();
    if let Some(messages) = response.messages {
        session
            .transcript
            .replace_turn_from_rig(turn_start, messages);
    } else {
        session
            .transcript
            .messages
            .push(rig::completion::Message::assistant(response.output.clone()));
    }
    Ok(response.output)
}
