use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolCall {
    pub(crate) id: String,
    pub(crate) call_id: String,
    pub(crate) name: String,
    pub(crate) arguments: String,
}

impl ToolCall {
    pub(crate) fn from_raw_input(
        id: String,
        name: String,
        input: &str,
        route: &str,
    ) -> Result<Self> {
        let arguments = if input.is_empty() { "{}" } else { input };
        serde_json::from_str::<Value>(arguments).with_context(|| {
            format!("Invalid JSON input for {route} tool call {name}: {arguments}")
        })?;
        Ok(Self {
            call_id: id.clone(),
            id,
            name,
            arguments: arguments.to_string(),
        })
    }

    pub(crate) fn arguments_value(&self) -> Result<Value> {
        serde_json::from_str(&self.arguments)
            .with_context(|| format!("tool `{}` supplied invalid JSON arguments", self.name))
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Usage {
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) non_cached_input_tokens: Option<u64>,
    pub(crate) cache_read_input_tokens: Option<u64>,
    pub(crate) cache_write_input_tokens: Option<u64>,
    pub(crate) reasoning_tokens: Option<u64>,
    pub(crate) total_tokens: Option<u64>,
    pub(crate) provider_metadata: Option<Value>,
}

impl Usage {
    pub(crate) fn from_openai_chat(usage: &Value) -> Self {
        let input = u64_at(usage, "/prompt_tokens");
        let output = u64_at(usage, "/completion_tokens");
        let cached = u64_at(usage, "/prompt_tokens_details/cached_tokens");
        let reasoning = u64_at(usage, "/completion_tokens_details/reasoning_tokens");
        Self {
            input_tokens: input,
            output_tokens: output,
            non_cached_input_tokens: subtract_tokens(input, cached),
            cache_read_input_tokens: cached,
            cache_write_input_tokens: None,
            reasoning_tokens: reasoning,
            total_tokens: total_tokens(input, output, u64_at(usage, "/total_tokens")),
            provider_metadata: Some(serde_json::json!({"openai": usage.clone()})),
        }
    }

    pub(crate) fn from_openai_responses(usage: &Value) -> Self {
        let input = u64_at(usage, "/input_tokens");
        let output = u64_at(usage, "/output_tokens");
        let cached = u64_at(usage, "/input_tokens_details/cached_tokens");
        let reasoning = u64_at(usage, "/output_tokens_details/reasoning_tokens");
        Self {
            input_tokens: input,
            output_tokens: output,
            non_cached_input_tokens: subtract_tokens(input, cached),
            cache_read_input_tokens: cached,
            cache_write_input_tokens: None,
            reasoning_tokens: reasoning,
            total_tokens: total_tokens(input, output, u64_at(usage, "/total_tokens")),
            provider_metadata: Some(serde_json::json!({"openai": usage.clone()})),
        }
    }

    pub(crate) fn from_bedrock(usage: &Value) -> Self {
        let input = u64_at(usage, "/inputTokens");
        let output = u64_at(usage, "/outputTokens");
        let cache_read = u64_at(usage, "/cacheReadInputTokens");
        let cache_write = u64_at(usage, "/cacheWriteInputTokens");
        Self {
            input_tokens: input,
            output_tokens: output,
            non_cached_input_tokens: subtract_tokens(input, sum_tokens(cache_read, cache_write)),
            cache_read_input_tokens: cache_read,
            cache_write_input_tokens: cache_write,
            reasoning_tokens: None,
            total_tokens: total_tokens(input, output, u64_at(usage, "/totalTokens")),
            provider_metadata: Some(serde_json::json!({"bedrock": usage.clone()})),
        }
    }

    pub(crate) fn from_anthropic(usage: &Value) -> Option<Self> {
        let non_cached = u64_at(usage, "/input_tokens");
        let output = u64_at(usage, "/output_tokens");
        let cache_read = u64_at(usage, "/cache_read_input_tokens");
        let cache_write = u64_at(usage, "/cache_creation_input_tokens");
        let input = sum_tokens(non_cached, sum_tokens(cache_read, cache_write));
        if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() {
            return None;
        }
        Some(Self {
            input_tokens: input,
            output_tokens: output,
            non_cached_input_tokens: non_cached,
            cache_read_input_tokens: cache_read,
            cache_write_input_tokens: cache_write,
            reasoning_tokens: None,
            total_tokens: total_tokens(input, output, None),
            provider_metadata: Some(serde_json::json!({"anthropic": usage.clone()})),
        })
    }

    pub(crate) fn from_gemini(usage: &Value) -> Self {
        let input = u64_at(usage, "/promptTokenCount");
        let cached = u64_at(usage, "/cachedContentTokenCount");
        let visible_output = u64_at(usage, "/candidatesTokenCount");
        let reasoning = u64_at(usage, "/thoughtsTokenCount");
        let output = sum_tokens(visible_output, reasoning);
        Self {
            input_tokens: input,
            output_tokens: visible_output.map(|_| output.unwrap_or(0)),
            non_cached_input_tokens: subtract_tokens(input, cached),
            cache_read_input_tokens: cached,
            cache_write_input_tokens: None,
            reasoning_tokens: reasoning,
            total_tokens: total_tokens(
                input,
                visible_output.map(|_| output.unwrap_or(0)),
                u64_at(usage, "/totalTokenCount"),
            ),
            provider_metadata: Some(serde_json::json!({"google": usage.clone()})),
        }
    }

    pub(crate) fn merge_prefer_defined(self, fallback: Self) -> Self {
        let input = self.input_tokens.or(fallback.input_tokens);
        let output = self.output_tokens.or(fallback.output_tokens);
        Self {
            input_tokens: input,
            output_tokens: output,
            non_cached_input_tokens: self
                .non_cached_input_tokens
                .or(fallback.non_cached_input_tokens),
            cache_read_input_tokens: self
                .cache_read_input_tokens
                .or(fallback.cache_read_input_tokens),
            cache_write_input_tokens: self
                .cache_write_input_tokens
                .or(fallback.cache_write_input_tokens),
            reasoning_tokens: self.reasoning_tokens.or(fallback.reasoning_tokens),
            total_tokens: self
                .total_tokens
                .or_else(|| total_tokens(input, output, fallback.total_tokens)),
            provider_metadata: merge_provider_metadata(
                fallback.provider_metadata,
                self.provider_metadata,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FinishReason {
    Stop,
    Length,
    ContentFilter,
    ToolCalls,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LlmEvent {
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolInputStart {
        id: String,
        name: String,
    },
    ToolInputDelta {
        text: String,
    },
    ToolInputEnd {
        id: String,
        name: String,
    },
    ToolCall {
        call: ToolCall,
        provider_executed: bool,
    },
    ToolResult {
        call_id: String,
        name: String,
        output: Value,
        provider_executed: bool,
    },
    ProviderError {
        message: String,
        retryable: bool,
    },
    StepFinish {
        reason: FinishReason,
        usage: Option<Usage>,
    },
}

#[derive(Debug, Default, Clone)]
pub(crate) struct StepAccumulator {
    pub(crate) text: String,
    pub(crate) reasoning_content: Option<Value>,
    pub(crate) tool_calls: Vec<ToolCall>,
    pub(crate) finish_reason: Option<FinishReason>,
    pub(crate) usage: Option<Usage>,
}

impl StepAccumulator {
    pub(crate) fn push(&mut self, event: LlmEvent) -> Result<()> {
        match event {
            LlmEvent::TextDelta { text } => {
                self.text.push_str(&text);
            }
            LlmEvent::ReasoningDelta { text } => {
                if text.is_empty() {
                    return Ok(());
                }
                match self.reasoning_content.as_ref().and_then(Value::as_str) {
                    Some(existing) => {
                        let mut combined = existing.to_string();
                        combined.push_str(&text);
                        self.reasoning_content = Some(Value::String(combined));
                    }
                    None => self.reasoning_content = Some(Value::String(text)),
                }
            }
            LlmEvent::ToolCall {
                call,
                provider_executed,
            } => {
                if !provider_executed {
                    self.tool_calls.push(call);
                }
            }
            LlmEvent::ProviderError { message, .. } => bail!(message),
            LlmEvent::StepFinish { reason, usage } => {
                self.finish_reason = Some(reason);
                self.usage = usage;
            }
            LlmEvent::ToolInputStart { .. }
            | LlmEvent::ToolInputDelta { .. }
            | LlmEvent::ToolInputEnd { .. }
            | LlmEvent::ToolResult { .. } => {}
        }
        Ok(())
    }
}

fn merge_provider_metadata(left: Option<Value>, right: Option<Value>) -> Option<Value> {
    match (left, right) {
        (Some(Value::Object(mut left)), Some(Value::Object(right))) => {
            left.extend(right);
            Some(Value::Object(left))
        }
        (_, Some(right)) => Some(right),
        (Some(left), None) => Some(left),
        (None, None) => None,
    }
}

fn subtract_tokens(total: Option<u64>, subset: Option<u64>) -> Option<u64> {
    match (total, subset) {
        (Some(total), Some(subset)) => Some(total.saturating_sub(subset)),
        _ => None,
    }
}

fn total_tokens(input: Option<u64>, output: Option<u64>, total: Option<u64>) -> Option<u64> {
    total.or_else(|| Some(input? + output?))
}

fn sum_tokens(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left + right),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn u64_at(value: &Value, pointer: &str) -> Option<u64> {
    value.pointer(pointer).and_then(Value::as_u64)
}
