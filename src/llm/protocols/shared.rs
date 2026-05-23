use anyhow::{Result, bail};
use serde_json::Value;

use crate::llm::schema::ToolCall;
use crate::llm::{MessageContent, ToolResultContent};

#[derive(Debug, Clone)]
pub(crate) struct AssistantContent {
    pub(crate) text: String,
    pub(crate) reasoning_content: Option<Value>,
    pub(crate) tool_calls: Vec<ToolCall>,
}

pub(crate) fn assistant_parts(content: Vec<MessageContent>) -> Result<AssistantContent> {
    let mut text = Vec::new();
    let mut reasoning_content = None;
    let mut tool_calls = Vec::new();
    for item in content {
        match item {
            MessageContent::Text { text: value, .. } => text.push(value),
            MessageContent::ToolCall {
                id,
                call_id,
                name,
                arguments,
                signature,
                ..
            } => {
                let arguments = serde_json::to_string(&arguments)?;
                tool_calls.push(ToolCall {
                    call_id: call_id.unwrap_or_else(|| id.clone()),
                    id,
                    name,
                    arguments,
                    signature,
                });
            }
            MessageContent::Reasoning { value } => {
                reasoning_content.get_or_insert(value);
            }
            MessageContent::Opaque { value, .. } => text.push(serde_json::to_string(&value)?),
            MessageContent::ToolResult { .. } => {
                bail!("assistant message cannot contain a tool result")
            }
        }
    }
    Ok(AssistantContent {
        text: text.join("\n"),
        reasoning_content,
        tool_calls,
    })
}

pub(crate) fn tool_result_text(content: Vec<ToolResultContent>) -> Result<String> {
    content
        .into_iter()
        .map(|item| match item {
            ToolResultContent::Text { text } => Ok(text),
            ToolResultContent::Opaque { value } => {
                serde_json::to_string(&value).map_err(Into::into)
            }
        })
        .collect::<Result<Vec<_>>>()
        .map(|items| items.join("\n"))
}
