use anyhow::{Result, bail};
use futures_util::FutureExt;
use std::collections::HashMap;

use super::schema::ToolCall;
use super::{LlmTool, LlmTools};

const TOOL_ONLY_CHURN_LIMIT: usize = 256;

pub(crate) type ToolMap = HashMap<String, Box<dyn LlmTool>>;

pub(crate) fn tools_by_name(tools: LlmTools) -> ToolMap {
    tools
        .into_iter()
        .map(|tool| (tool.name().to_string(), tool))
        .collect()
}

#[derive(Debug, Default)]
pub(crate) struct ToolLoopState {
    failed_calls: HashMap<ToolCallFingerprint, usize>,
    tool_only_turns: usize,
}

impl ToolLoopState {
    pub(crate) fn note_assistant_turn(
        &mut self,
        text: &str,
        tool_calls: &[ToolCall],
    ) -> Result<()> {
        if !tool_calls.is_empty() && text.trim().is_empty() {
            self.tool_only_turns += 1;
        } else {
            self.tool_only_turns = 0;
        }
        if self.tool_only_turns > TOOL_ONLY_CHURN_LIMIT {
            bail!(
                "native OpenAI tool loop made no text progress for {TOOL_ONLY_CHURN_LIMIT} consecutive tool-only rounds"
            );
        }
        Ok(())
    }

    fn previous_failures(&self, call: &ToolCall) -> Option<usize> {
        self.failed_calls
            .get(&ToolCallFingerprint::from(call))
            .copied()
    }

    fn note_tool_result(&mut self, call: &ToolCall, failed: bool) {
        let fingerprint = ToolCallFingerprint::from(call);
        if failed {
            *self.failed_calls.entry(fingerprint).or_insert(0) += 1;
        } else {
            self.failed_calls.remove(&fingerprint);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ToolCallFingerprint {
    name: String,
    arguments: String,
}

impl From<&ToolCall> for ToolCallFingerprint {
    fn from(call: &ToolCall) -> Self {
        Self {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolCallOutcome {
    pub(crate) output: String,
    pub(crate) failed: bool,
}

impl ToolCallOutcome {
    fn success(output: String) -> Self {
        Self {
            output,
            failed: false,
        }
    }

    fn failure(output: String) -> Self {
        Self {
            output,
            failed: true,
        }
    }
}

pub(crate) async fn execute_tool_call(
    tools: &ToolMap,
    state: &mut ToolLoopState,
    call: &ToolCall,
) -> ToolCallOutcome {
    let outcome = if let Some(previous_failures) = state.previous_failures(call) {
        ToolCallOutcome::failure(repeated_failed_tool_call_output(call, previous_failures))
    } else {
        call_tool(tools, call).await
    };
    state.note_tool_result(call, outcome.failed);
    outcome
}

pub(crate) async fn call_tool(tools: &ToolMap, call: &ToolCall) -> ToolCallOutcome {
    let Some(tool) = tools.get(&call.name) else {
        return ToolCallOutcome::failure(unknown_tool_output(&call.name, tools));
    };

    let future = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tool.call(call.arguments.clone())
    })) {
        Ok(future) => future,
        Err(payload) => return ToolCallOutcome::failure(tool_panic_output(&call.name, payload)),
    };

    match std::panic::AssertUnwindSafe(future).catch_unwind().await {
        Ok(Ok(output)) => ToolCallOutcome::success(output),
        Ok(Err(err)) => ToolCallOutcome::failure(tool_failure_output(&call.name, &err)),
        Err(payload) => ToolCallOutcome::failure(tool_panic_output(&call.name, payload)),
    }
}

fn tool_failure_output(name: &str, err: &anyhow::Error) -> String {
    tool_error_output(
        &format!("tool `{name}` failed: {err}"),
        "Do not retry the same tool call unchanged. Fix the arguments, choose another tool, or report this blocker.",
    )
}

fn tool_panic_output(name: &str, payload: Box<dyn std::any::Any + Send>) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("panic payload unavailable");
    tool_error_output(
        &format!("tool `{name}` panicked: {message}"),
        "Do not retry the same tool call unchanged. Fix the arguments, choose another tool, or report this blocker.",
    )
}

fn unknown_tool_output(name: &str, tools: &ToolMap) -> String {
    tool_error_output(
        &format!("model requested unknown tool `{name}`"),
        &format!(
            "Use one of the enabled tools and documented argument schemas. {}",
            enabled_tools_hint(tools)
        ),
    )
}

fn repeated_failed_tool_call_output(call: &ToolCall, previous_failures: usize) -> String {
    tool_error_output(
        &format!(
            "repeated identical failed tool call `{}` after {previous_failures} failure(s)",
            call.name
        ),
        "Do not retry the same tool call unchanged. Change the arguments, choose another tool, or explain the blocker to the user.",
    )
}

fn tool_error_output(summary: &str, recovery: &str) -> String {
    format!("TOOL_ERROR: {summary}\nRECOVERY: {recovery}")
}

fn enabled_tools_hint(tools: &ToolMap) -> String {
    if tools.is_empty() {
        return "No tools are currently enabled.".to_string();
    }
    let mut names = tools.keys().map(String::as_str).collect::<Vec<_>>();
    names.sort_unstable();
    format!("Enabled tools: {}.", names.join(", "))
}

#[cfg(test)]
#[path = "test/tool_runtime.rs"]
mod tests;
