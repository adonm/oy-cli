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
