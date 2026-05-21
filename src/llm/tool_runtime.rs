use anyhow::{Result, bail};
use std::collections::HashMap;

use super::schema::ToolCall;
use super::{LlmTool, LlmTools};

const TOOL_ONLY_CHURN_LIMIT: usize = 64;

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

    match tool.call(call.arguments.clone()).await {
        Ok(output) => ToolCallOutcome::success(output),
        Err(err) => ToolCallOutcome::failure(tool_failure_output(&call.name, &err)),
    }
}

fn tool_failure_output(name: &str, err: &anyhow::Error) -> String {
    tool_error_output(
        &format!("tool `{name}` failed: {err}"),
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
mod tests {
    use super::*;
    use anyhow::anyhow;

    struct FailingTool;

    struct EchoTool;

    impl LlmTool for FailingTool {
        fn name(&self) -> &str {
            "fail"
        }

        fn call<'a>(&'a self, _args: String) -> crate::llm::LlmToolFuture<'a> {
            Box::pin(async move { Err(anyhow!("boom")) })
        }
    }

    impl LlmTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn call<'a>(&'a self, args: String) -> crate::llm::LlmToolFuture<'a> {
            Box::pin(async move { Ok(args) })
        }
    }

    fn call(name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            call_id: "call-1".to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    #[tokio::test]
    async fn tool_call_failure_is_returned_to_model_as_tool_output() {
        let tools: ToolMap = HashMap::from([(
            "fail".to_string(),
            Box::new(FailingTool) as Box<dyn LlmTool>,
        )]);

        let output = call_tool(&tools, &call("fail", "{}")).await.output;

        assert!(output.contains("TOOL_ERROR: tool `fail` failed: boom"));
        assert!(output.contains("RECOVERY:"));
        assert!(output.contains("Do not retry the same tool call unchanged"));
    }

    #[tokio::test]
    async fn repeated_identical_failed_tool_call_is_not_reinvoked() {
        let tools: ToolMap = HashMap::from([(
            "fail".to_string(),
            Box::new(FailingTool) as Box<dyn LlmTool>,
        )]);
        let call = call("fail", "{\"path\":\"missing\"}");
        let mut state = ToolLoopState::default();

        let first = execute_tool_call(&tools, &mut state, &call).await;
        let second = execute_tool_call(&tools, &mut state, &call).await;

        assert!(first.output.contains("tool `fail` failed: boom"));
        assert!(
            second
                .output
                .contains("repeated identical failed tool call `fail` after 1 failure(s)")
        );
        assert!(second.output.contains("RECOVERY:"));
    }

    #[tokio::test]
    async fn unknown_tool_failure_lists_enabled_tools() {
        let tools: ToolMap =
            HashMap::from([("echo".to_string(), Box::new(EchoTool) as Box<dyn LlmTool>)]);

        let outcome = call_tool(&tools, &call("missing", "{}")).await;

        assert!(outcome.failed);
        assert!(
            outcome
                .output
                .contains("TOOL_ERROR: model requested unknown tool `missing`")
        );
        assert!(outcome.output.contains("Enabled tools: echo."));
    }

    #[test]
    fn tool_only_churn_guard_fails_before_default_round_budget() {
        let call = call("read", "{}");
        let mut state = ToolLoopState::default();

        for _ in 0..TOOL_ONLY_CHURN_LIMIT {
            state
                .note_assistant_turn("", std::slice::from_ref(&call))
                .unwrap();
        }
        let err = state
            .note_assistant_turn("", std::slice::from_ref(&call))
            .unwrap_err();

        assert!(err.to_string().contains("no text progress"));
    }
}
