use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

mod openai;

pub(crate) use openai::NativeOpenAiBackend;

pub(crate) type ChatFuture<'a> = Pin<Box<dyn Future<Output = Result<LlmResponse>> + 'a>>;
pub(crate) type LlmToolFuture<'a> = Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;

pub(crate) trait LlmTool: Send + Sync {
    fn name(&self) -> &str;
    fn call<'a>(&'a self, args: String) -> LlmToolFuture<'a>;
}

pub(crate) type LlmTools = Vec<Box<dyn LlmTool>>;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct LlmRequest {
    pub route: ModelRoute,
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_turns: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LlmResponse {
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<Message>>,
}

pub(crate) trait ChatBackend {
    type Tools;
    fn chat<'a>(&'a self, request: LlmRequest, tools: Self::Tools) -> ChatFuture<'a>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Protocol {
    OpenAiChat,
    OpenAiResponses,
}

impl Protocol {
    pub(crate) fn uses_responses_api(self) -> bool {
        matches!(self, Self::OpenAiResponses)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum RouteAuth {
    ApiKey(String),
}

impl std::fmt::Debug for RouteAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey(_) => f.write_str("ApiKey(<redacted>)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ModelRoute {
    pub protocol: Protocol,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing)]
    pub auth: RouteAuth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub(crate) enum Message {
    System {
        content: String,
    },
    User {
        content: Vec<MessageContent>,
    },
    Assistant {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        content: Vec<MessageContent>,
    },
}

impl Message {
    pub(crate) fn user_text(text: impl Into<String>) -> Self {
        Self::User {
            content: vec![MessageContent::Text { text: text.into() }],
        }
    }

    pub(crate) fn assistant_text(text: impl Into<String>) -> Self {
        Self::Assistant {
            id: None,
            content: vec![MessageContent::Text { text: text.into() }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum MessageContent {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        name: String,
        arguments: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_params: Option<Value>,
    },
    ToolResult {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        content: Vec<ToolResultContent>,
    },
    Reasoning {
        value: Value,
    },
    Opaque {
        value: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToolResultContent {
    Text { text: String },
    Opaque { value: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_spec_serializes_without_backend_details() {
        let spec = ToolSpec {
            name: "read".to_string(),
            description: "Read one file".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
                "additionalProperties": false
            }),
        };

        let actual = serde_json::to_string_pretty(&spec).unwrap();
        let expected = r#"{
  "name": "read",
  "description": "Read one file",
  "parameters": {
    "type": "object",
    "properties": {
      "path": {
        "type": "string"
      }
    },
    "required": [
      "path"
    ],
    "additionalProperties": false
  }
}"#;
        assert_eq!(actual, expected);
    }
}
