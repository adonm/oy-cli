use super::{
    CacheHint, CachePolicy, CachePolicyObject, LlmRequest, Message, MessageCachePolicy,
    MessageContent, Protocol, ToolSpec,
};

const DEFAULT_POLICY: CachePolicyObject = CachePolicyObject {
    tools: true,
    system: true,
    messages: Some(MessageCachePolicy::LatestUserMessage),
    ttl_seconds: None,
};

const NO_POLICY: CachePolicyObject = CachePolicyObject {
    tools: false,
    system: false,
    messages: None,
    ttl_seconds: None,
};

pub(crate) const INLINE_BREAKPOINT_CAP: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Breakpoints {
    pub(crate) remaining: usize,
    pub(crate) dropped: usize,
}

impl Breakpoints {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            remaining: cap,
            dropped: 0,
        }
    }

    pub(crate) fn take(&mut self) -> bool {
        if self.remaining == 0 {
            self.dropped += 1;
            return false;
        }
        self.remaining -= 1;
        true
    }
}

pub(crate) fn apply(request: LlmRequest) -> LlmRequest {
    if !respects_inline_hints(request.route.protocol) {
        return request;
    }

    let policy = resolve(request.cache.as_ref());
    if !policy.tools && !policy.system && policy.messages.is_none() {
        return request;
    }

    let hint = make_hint(policy.ttl_seconds);
    let mut system_cache = request.system_cache;
    if policy.system && !request.system_prompt.trim().is_empty() && system_cache.is_none() {
        system_cache = Some(hint);
    }

    let tools = if policy.tools {
        mark_last_tool(request.tools, hint)
    } else {
        request.tools
    };
    let messages = mark_system_and_messages(request.messages, policy, hint);

    LlmRequest {
        tools,
        messages,
        system_cache,
        ..request
    }
}

pub(crate) fn respects_inline_hints(protocol: Protocol) -> bool {
    match protocol {
        Protocol::OpenAiChat | Protocol::OpenAiResponses => false,
        Protocol::AnthropicMessages | Protocol::BedrockConverse => true,
    }
}

pub(crate) fn ttl_bucket(ttl_seconds: Option<u64>) -> Option<&'static str> {
    ttl_seconds.filter(|seconds| *seconds >= 3600).map(|_| "1h")
}

pub(crate) fn cache_point_allowed(
    breakpoints: &mut Breakpoints,
    cache: Option<&CacheHint>,
) -> bool {
    if cache.is_none() {
        return false;
    }
    breakpoints.take()
}

fn resolve(policy: Option<&CachePolicy>) -> CachePolicyObject {
    match policy {
        None | Some(CachePolicy::Auto) => DEFAULT_POLICY,
        Some(CachePolicy::None) => NO_POLICY,
        Some(CachePolicy::Object(policy)) => policy.clone(),
    }
}

fn make_hint(ttl_seconds: Option<u64>) -> CacheHint {
    CacheHint::Ephemeral { ttl_seconds }
}

fn mark_last_tool(mut tools: Vec<ToolSpec>, hint: CacheHint) -> Vec<ToolSpec> {
    if let Some(tool) = tools.last_mut()
        && tool.cache.is_none()
    {
        tool.cache = Some(hint);
    }
    tools
}

fn mark_system_and_messages(
    mut messages: Vec<Message>,
    policy: CachePolicyObject,
    hint: CacheHint,
) -> Vec<Message> {
    if policy.system {
        mark_last_system(&mut messages, hint);
    }
    if let Some(strategy) = policy.messages {
        mark_messages(&mut messages, strategy, hint);
    }
    messages
}

fn mark_last_system(messages: &mut [Message], hint: CacheHint) {
    let Some(message) = messages
        .iter_mut()
        .rfind(|message| matches!(message, Message::System { .. }))
    else {
        return;
    };
    let Message::System { cache, .. } = message else {
        return;
    };
    if cache.is_none() {
        *cache = Some(hint);
    }
}

fn mark_messages(messages: &mut [Message], strategy: MessageCachePolicy, hint: CacheHint) {
    match strategy {
        MessageCachePolicy::LatestUserMessage => mark_latest_role(messages, Role::User, hint),
        MessageCachePolicy::LatestAssistant => mark_latest_role(messages, Role::Assistant, hint),
        MessageCachePolicy::Tail { count } => {
            let start = messages.len().saturating_sub(count);
            for message in messages.iter_mut().skip(start) {
                mark_message(message, hint);
            }
        }
    }
}

fn mark_latest_role(messages: &mut [Message], role: Role, hint: CacheHint) {
    let Some(message) = messages.iter_mut().rfind(|message| role.matches(message)) else {
        return;
    };
    mark_message(message, hint);
}

fn mark_message(message: &mut Message, hint: CacheHint) {
    let content = match message {
        Message::User { content } | Message::Assistant { content, .. } => content,
        Message::System { cache, .. } => {
            if cache.is_none() {
                *cache = Some(hint);
            }
            return;
        }
    };
    if content.is_empty() {
        return;
    }
    let mark_at = content
        .iter()
        .rposition(|item| matches!(item, MessageContent::Text { .. }))
        .unwrap_or(content.len() - 1);
    set_content_cache(&mut content[mark_at], hint);
}

fn set_content_cache(content: &mut MessageContent, hint: CacheHint) {
    match content {
        MessageContent::Text { cache, .. } | MessageContent::Opaque { cache, .. } => {
            if cache.is_none() {
                *cache = Some(hint);
            }
        }
        MessageContent::ToolResult { cache, .. } => {
            if cache.is_none() {
                *cache = Some(hint);
            }
        }
        MessageContent::ToolCall { .. } | MessageContent::Reasoning { .. } => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
}

impl Role {
    fn matches(self, message: &Message) -> bool {
        matches!(
            (self, message),
            (Self::User, Message::User { .. }) | (Self::Assistant, Message::Assistant { .. })
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ModelRoute, RouteAuth, ToolSpec};
    use serde_json::json;

    #[test]
    fn openai_family_skips_inline_cache_hints_like_opencode() {
        let request = request_with_protocol(Protocol::OpenAiChat, None);

        let actual = apply(request);

        assert_eq!(actual.tools[0].cache, None);
        assert_eq!(actual.system_cache, None);
        assert_eq!(message_system_cache(&actual.messages[0]), None);
        assert_eq!(text_cache(&actual.messages[1]), None);
        assert!(!respects_inline_hints(Protocol::OpenAiChat));
        assert!(!respects_inline_hints(Protocol::OpenAiResponses));
    }

    #[test]
    fn auto_policy_marks_tools_system_and_latest_user_message() {
        let mut request = request_with_protocol(Protocol::AnthropicMessages, None);
        request.system_prompt = "root system".to_string();

        let actual = apply(request);

        let expected = Some(CacheHint::Ephemeral { ttl_seconds: None });
        assert_eq!(actual.tools[0].cache, expected);
        assert_eq!(actual.system_cache, expected);
        assert_eq!(message_system_cache(&actual.messages[0]), expected);
        assert_eq!(text_cache(&actual.messages[1]), None);
        assert_eq!(text_cache(&actual.messages[3]), expected);
    }

    #[test]
    fn none_policy_preserves_manual_cache_hints_only() {
        let mut request = request_with_protocol(Protocol::BedrockConverse, Some(CachePolicy::None));
        request.tools[0].cache = Some(CacheHint::Persistent {
            ttl_seconds: Some(7200),
        });

        let actual = apply(request);

        assert_eq!(
            actual.tools[0].cache,
            Some(CacheHint::Persistent {
                ttl_seconds: Some(7200)
            })
        );
        assert_eq!(actual.system_cache, None);
        assert_eq!(message_system_cache(&actual.messages[0]), None);
        assert_eq!(text_cache(&actual.messages[3]), None);
    }

    #[test]
    fn object_policy_marks_requested_tail_with_ttl() {
        let request = request_with_protocol(
            Protocol::BedrockConverse,
            Some(CachePolicy::Object(CachePolicyObject {
                messages: Some(MessageCachePolicy::Tail { count: 2 }),
                ttl_seconds: Some(3600),
                ..CachePolicyObject::default()
            })),
        );

        let actual = apply(request);
        let expected = Some(CacheHint::Ephemeral {
            ttl_seconds: Some(3600),
        });

        assert_eq!(actual.tools[0].cache, None);
        assert_eq!(actual.system_cache, None);
        assert_eq!(message_system_cache(&actual.messages[0]), None);
        assert_eq!(text_cache(&actual.messages[2]), expected);
        assert_eq!(text_cache(&actual.messages[3]), expected);
    }

    #[test]
    fn latest_user_policy_marks_tool_result_only_message() {
        let mut request = request_with_protocol(Protocol::AnthropicMessages, None);
        request.messages.push(Message::User {
            content: vec![MessageContent::ToolResult {
                id: "result".to_string(),
                call_id: Some("call".to_string()),
                content: vec![],
                cache: None,
            }],
        });

        let actual = apply(request);

        assert_eq!(
            tool_result_cache(actual.messages.last().unwrap()),
            Some(CacheHint::Ephemeral { ttl_seconds: None })
        );
    }

    #[test]
    fn breakpoint_helpers_track_cap_and_ttl_bucket() {
        let mut breakpoints = Breakpoints::new(INLINE_BREAKPOINT_CAP);
        let hint = CacheHint::Ephemeral { ttl_seconds: None };

        for _ in 0..INLINE_BREAKPOINT_CAP {
            assert!(cache_point_allowed(&mut breakpoints, Some(&hint)));
        }
        assert!(!cache_point_allowed(&mut breakpoints, Some(&hint)));
        assert!(!cache_point_allowed(&mut breakpoints, None));
        assert_eq!(breakpoints.dropped, 1);
        assert_eq!(ttl_bucket(None), None);
        assert_eq!(ttl_bucket(Some(3599)), None);
        assert_eq!(ttl_bucket(Some(3600)), Some("1h"));
    }

    fn request_with_protocol(protocol: Protocol, cache: Option<CachePolicy>) -> LlmRequest {
        LlmRequest {
            route: ModelRoute {
                protocol,
                model: "test".to_string(),
                base_url: Some("https://example.test".to_string()),
                auth: RouteAuth::ApiKey("test".to_string()),
                query_params: None,
                additional_params: None,
            },
            system_prompt: String::new(),
            system_cache: None,
            messages: vec![
                Message::System {
                    content: "system".to_string(),
                    cache: None,
                },
                Message::user_text("first"),
                Message::assistant_text("assistant"),
                Message::user_text("latest"),
            ],
            tools: vec![ToolSpec {
                name: "tool".to_string(),
                description: "tool".to_string(),
                parameters: json!({"type": "object"}),
                cache: None,
            }],
            max_turns: 1,
            tool_choice: None,
            generation: None,
            cache,
        }
    }

    fn message_system_cache(message: &Message) -> Option<CacheHint> {
        let Message::System { cache, .. } = message else {
            panic!("expected system message")
        };
        *cache
    }

    fn text_cache(message: &Message) -> Option<CacheHint> {
        let (Message::User { content } | Message::Assistant { content, .. }) = message else {
            panic!("expected content message")
        };
        let MessageContent::Text { cache, .. } = &content[0] else {
            panic!("expected text content")
        };
        *cache
    }

    fn tool_result_cache(message: &Message) -> Option<CacheHint> {
        let Message::User { content } = message else {
            panic!("expected user message")
        };
        let MessageContent::ToolResult { cache, .. } = &content[0] else {
            panic!("expected tool result content")
        };
        *cache
    }
}
