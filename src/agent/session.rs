use anyhow::{Result, bail};
use chrono::Utc;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest};
use serde_json::json;

use super::chat::{display_model, exec_chat, token_count_text};
use super::compaction::{compact_text, compaction_prompt, count_tokens, deterministic_summary};
pub use super::transcript::{
    CompactionStats, ContextBudgetExceeded, ContextStatus, StoredMessage, StoredToolCall,
    Transcript,
};
use crate::config::{self, SafetyMode, SessionFile};

mod noop;
mod storage;
pub use storage::load_saved;

use crate::model;
use crate::tools::{TodoItem, TodoStatus, ToolContext, ToolPolicy};
use noop::RepeatedNoopTools;

const DEFAULT_MAX_TOOL_ROUNDS: usize = 512;

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

    fn chat_options(&self) -> Option<ChatOptions> {
        model::reasoning_effort_option(&self.model)
            .and_then(|effort| effort.parse().ok())
            .map(|effort| ChatOptions::default().with_reasoning_effort(effort))
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
        if let Some(effort) = model::default_reasoning_effort(model_spec) {
            parts.push(format!("think {effort}"));
        }
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
        let model_spec = model::to_genai_model_spec(&self.model);
        let config = config::context_config();
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
            saved_at: Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            workspace_root: Some(self.root.clone()),
            mode: Some(self.mode),
            transcript: serde_json::to_value(&self.transcript)?,
            todos: self.todos.clone(),
        };
        config::save_session_file(name, &payload)
    }
}

async fn ensure_context_budget(session: &mut Session, model_spec: &str) -> Result<()> {
    let config = config::context_config();
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
            "compacted context: {} -> {} tokens ({} old messages, {} tool outputs)",
            stats.before_tokens, stats.after_tokens, stats.removed_messages, stats.compacted_tools
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
    let response = exec_chat(model_spec, client, req, options.as_ref()).await?;
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
    let client = model::build_client()?;
    let model_spec = model::to_genai_model_spec(model);
    let req = ChatRequest::default()
        .with_system(system_prompt)
        .append_message(ChatMessage::user(prompt.to_string()));
    if !crate::ui::is_quiet() {
        let tokens = count_tokens(&model_spec, system_prompt) + count_tokens(&model_spec, prompt);
        crate::ui::err_line(format_args!(
            "oy · {} · {} · no tools",
            display_model(&model_spec),
            token_count_text(tokens)
        ));
    }
    let options = model::reasoning_effort_option(model)
        .and_then(|effort| effort.parse().ok())
        .map(|effort| ChatOptions::default().with_reasoning_effort(effort));
    let response = exec_chat(&model_spec, &client, req, options.as_ref()).await?;
    Ok(response.into_first_text().unwrap_or_default())
}

async fn run_prompt_with_policy(
    session: &mut Session,
    prompt: &str,
    policy_override: Option<ToolPolicy>,
) -> Result<String> {
    let client = model::build_client()?;
    session.transcript.messages.push(StoredMessage::User {
        content: prompt.to_string(),
    });
    let mut repeated_noop_tools = RepeatedNoopTools::default();
    let tool_round_limit = config::max_tool_rounds(DEFAULT_MAX_TOOL_ROUNDS);
    let mut tool_round_count = 0usize;
    let mut tool_call_count = 0usize;

    loop {
        let mut tool_context = session.tool_context();
        if let Some(policy) = policy_override {
            tool_context.policy = policy;
        }
        let tool_specs = crate::tools::tool_specs(&tool_context);
        let model_spec = model::to_genai_model_spec(&session.model);
        ensure_context_budget(session, &model_spec).await?;
        let req = session
            .transcript
            .to_chat_request(&session.system_prompt, &tool_context)
            .with_tools(tool_specs.clone());
        if !crate::ui::is_quiet() {
            crate::ui::err_line(format_args!("{}", session.wait_status(&model_spec)));
        }
        let options = session.chat_options();
        let response = exec_chat(&model_spec, &client, req, options.as_ref()).await?;
        let tool_calls = response
            .tool_calls()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if !tool_calls.is_empty() {
            let next_tool_round = tool_round_count + 1;
            if tool_round_limit.exceeded(next_tool_round) {
                let limit = tool_round_limit.label();
                bail!(
                    "tool loop exceeded {limit} tool rounds ({tool_call_count} tool calls completed); set OY_MAX_TOOL_ROUNDS=<number> or OY_MAX_TOOL_ROUNDS=unlimited for trusted long runs"
                );
            }
            tool_round_count = next_tool_round;
            crate::ui::tool_batch(tool_round_count, tool_calls.len());
            session
                .transcript
                .messages
                .push(StoredMessage::AssistantToolCalls {
                    reasoning_content: response.reasoning_content.clone(),
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
                tool_call_count += 1;
                let mut ctx = session.tool_context();
                if let Some(policy) = policy_override {
                    ctx.policy = policy;
                }
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

        let reasoning_content = response.reasoning_content.clone();
        let answer = response.into_first_text().unwrap_or_default();
        session.transcript.messages.push(StoredMessage::Assistant {
            content: answer.clone(),
            reasoning_content,
        });
        return Ok(answer);
    }
}
