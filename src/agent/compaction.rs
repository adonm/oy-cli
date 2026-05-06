use rig::completion::message::{AssistantContent, Message, ToolResultContent, UserContent};
use tiktoken_rs::{bpe_for_model, cl100k_base};

fn model_tokenizer_name(model: &str) -> &str {
    model
        .rsplit_once("::")
        .map(|(_, name)| name)
        .unwrap_or(model)
}

pub(crate) fn count_tokens(model: &str, text: &str) -> usize {
    let model_name = model_tokenizer_name(model);
    if let Ok(bpe) = bpe_for_model(model_name) {
        return bpe.encode_with_special_tokens(text).len();
    }
    cl100k_base()
        .ok()
        .map(|bpe| bpe.encode_with_special_tokens(text).len())
        .unwrap_or_else(|| text.split_whitespace().count())
}

fn take_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn take_last_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars().rev().take(max_chars).collect::<Vec<_>>();
    chars.reverse();
    chars.into_iter().collect()
}

pub(super) fn compact_text(text: &str, model: &str, max_tokens: usize, label: &str) -> String {
    let tokens = count_tokens(model, text);
    if tokens <= max_tokens {
        return text.to_string();
    }
    let target_chars = max_tokens.saturating_mul(3).max(512);
    let half = target_chars / 2;
    let head = take_chars(text, half);
    let tail = take_last_chars(text, half);
    format!(
        "[{label}] original ~{tokens} tokens, {} bytes. Preserved head/tail.\n\n--- head ---\n{}\n\n--- tail ---\n{}",
        text.len(),
        head.trim_end(),
        tail.trim_start()
    )
}

pub(super) fn message_content_text(message: &Message) -> String {
    match message {
        Message::System { content } => content.clone(),
        Message::User { content } => content
            .iter()
            .map(user_content_text)
            .collect::<Vec<_>>()
            .join("\n"),
        Message::Assistant { content, .. } => content
            .iter()
            .map(assistant_content_text)
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn user_content_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.text.clone(),
        UserContent::ToolResult(result) => result
            .content
            .iter()
            .map(tool_result_content_text)
            .collect::<Vec<_>>()
            .join("\n"),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn assistant_content_text(content: &AssistantContent) -> String {
    match content {
        AssistantContent::Text(text) => text.text.clone(),
        AssistantContent::ToolCall(call) => {
            format!("{} {}", call.function.name, call.function.arguments)
        }
        AssistantContent::Reasoning(reasoning) => reasoning.display_text(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn tool_result_content_text(content: &ToolResultContent) -> String {
    match content {
        ToolResultContent::Text(text) => text.text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}
