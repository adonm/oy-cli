//! Prompt-level chat execution handoff.

use anyhow::Result;

use crate::llm::{
    ChatBackend, LlmRequest, LlmResponse, LlmTools, Message, NativeOpenAiBackend, ToolSpec,
};

use super::metadata::cache_model_limits;
use super::reasoning::default_reasoning_effort;

static BACKEND: NativeOpenAiBackend = NativeOpenAiBackend;

pub async fn exec_chat(
    model_spec: &str,
    preamble: &str,
    messages: Vec<Message>,
    tool_specs: Vec<ToolSpec>,
    tools: LlmTools,
    max_turns: usize,
) -> Result<LlmResponse> {
    let _ = cache_model_limits(model_spec).await;
    let route = prepare_chat(model_spec)?;
    let request = LlmRequest {
        route,
        system_prompt: preamble.to_string(),
        system_cache: None,
        messages,
        tools: tool_specs,
        max_turns,
        tool_choice: None,
        generation: None,
        cache: None,
    };
    BACKEND.chat(request, tools).await
}

pub(crate) fn prepare_chat(model_spec: &str) -> Result<crate::llm::ModelRoute> {
    crate::llm::route::resolve::model_route(model_spec, default_reasoning_effort(model_spec))
}
