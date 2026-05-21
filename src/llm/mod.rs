//! `oy`-owned LLM request/response, message, tool-spec, model route,
//! backend seam, and native OpenAI-compatible transport.
//!
//! This module defines the data plane between agent logic and the
//! provider wire protocol. [`LlmRequest`], [`Message`], [`ToolSpec`],
//! and [`ModelRoute`] are the stable shapes; [`ChatBackend`] is the
//! narrow backend trait. The default transport lives in [`openai`].

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

mod cache_policy;
mod openai;
mod protocols;
pub(crate) mod providers;
mod route;
mod schema;
mod tool_runtime;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_cache: Option<CacheHint>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_turns: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<GenerationOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CachePolicy>,
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
    AnthropicMessages,
    BedrockConverse,
}

impl Protocol {
    pub(crate) fn uses_responses_api(self) -> bool {
        matches!(self, Self::OpenAiResponses)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CacheHint {
    Ephemeral {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_seconds: Option<u64>,
    },
    Persistent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_seconds: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CachePolicy {
    Auto,
    None,
    Object(CachePolicyObject),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MessageCachePolicy {
    LatestUserMessage,
    LatestAssistant,
    Tail { count: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) struct CachePolicyObject {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tools: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub system: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<MessageCachePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToolChoice {
    Auto,
    None,
    Required,
    Tool { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerationOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum RouteAuth {
    ApiKey(String),
    Header { name: String, value: String },
    Headers(Vec<(String, String)>),
    Composite(Vec<RouteAuth>),
    AwsSigV4(AwsCredentials),
}

impl std::fmt::Debug for RouteAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey(_) => f.write_str("ApiKey(<redacted>)"),
            Self::Header { name, .. } => f
                .debug_struct("Header")
                .field("name", name)
                .field("value", &"<redacted>")
                .finish(),
            Self::Headers(headers) => f
                .debug_tuple("Headers")
                .field(&format_args!("{} headers", headers.len()))
                .finish(),
            Self::Composite(auths) => f
                .debug_tuple("Composite")
                .field(&format_args!("{} auth layers", auths.len()))
                .finish(),
            Self::AwsSigV4(credentials) => f
                .debug_struct("AwsSigV4")
                .field("region", &credentials.region)
                .field("access_key_id", &"<redacted>")
                .field("secret_access_key", &"<redacted>")
                .field(
                    "session_token",
                    &credentials.session_token.as_ref().map(|_| "<redacted>"),
                )
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct AwsCredentials {
    pub(crate) region: String,
    pub(crate) access_key_id: String,
    pub(crate) secret_access_key: String,
    pub(crate) session_token: Option<String>,
}

impl std::fmt::Debug for AwsCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsCredentials")
            .field("region", &self.region)
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
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
    pub query_params: Option<Vec<(String, String)>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub(crate) enum Message {
    System {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache: Option<CacheHint>,
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
            content: vec![MessageContent::Text {
                text: text.into(),
                cache: None,
            }],
        }
    }

    pub(crate) fn assistant_text(text: impl Into<String>) -> Self {
        Self::Assistant {
            id: None,
            content: vec![MessageContent::Text {
                text: text.into(),
                cache: None,
            }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum MessageContent {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache: Option<CacheHint>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache: Option<CacheHint>,
    },
    Reasoning {
        value: Value,
    },
    Opaque {
        value: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache: Option<CacheHint>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheHint>,
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
            cache: None,
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
