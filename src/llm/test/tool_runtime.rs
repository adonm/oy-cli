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
        signature: None,
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
