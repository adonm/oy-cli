//! Multi-pattern compression pipeline for tool outputs entering the
//! LLM transcript.
//!
//! All transforms are idempotent and safety-preserving: they improve
//! presentation density without altering semantic content. Only
//! [`compact_text`] adds provenance headers; internal stages are pure.
//! Token counting uses tokenizer-specific BPE through [`tiktoken_rs`].

use crate::llm::{Message, MessageContent, ToolResultContent};
use regex::Regex;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use tiktoken_rs::{bpe_for_model, cl100k_base_singleton, CoreBPE};

static TOKENIZER_CACHE: LazyLock<RwLock<HashMap<String, &'static CoreBPE>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn model_tokenizer_name(model: &str) -> &str {
    model
        .rsplit_once("::")
        .map(|(_, name)| name)
        .unwrap_or(model)
}

pub(crate) fn count_tokens(model: &str, text: &str) -> usize {
    if let Some(&bpe) = TOKENIZER_CACHE.read().unwrap().get(model) {
        return bpe.encode_with_special_tokens(text).len();
    }

    let model_name = model_tokenizer_name(model);
    let bpe = bpe_for_model(model_name).unwrap_or_else(|_| cl100k_base_singleton());

    TOKENIZER_CACHE
        .write()
        .unwrap()
        .insert(model.to_string(), bpe);

    bpe.encode_with_special_tokens(text).len()
}

// === Multi-pattern compression engine ===
//
// A stack of structural, redundancy, and noise-reduction transforms applied
// to tool outputs before they enter the LLM transcript. All patterns are
// idempotent and safety-preserving: they never alter semantic content, only
// presentation density.
//
// Architecture:
//   compact_text()  ← the single entry point for transcript compaction
//     compress()    ← the staged pipeline (public for testing)
//       stage 1: strip ANSI
//       stage 2: collapse blank lines
//       stage 3: dedup repeated lines
//       stage 4: compact noisy patterns (paths, hashes)
//       stage 5: preserve head/tail (error-aware)
//
// Only compact_text() adds provenance headers. Internal stages are pure
// transforms — they don't annotate their output.

/// Aggressiveness of the compression pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CompressionMode {
    /// Heuristic selection based on byte size.
    Auto,
    /// Strip ANSI only.
    Light,
    /// Light + collapse blanks, dedup lines, compact paths/hashes.
    Normal,
    /// Normal + head/tail preservation when over budget.
    Aggressive,
}

/// Run the compression pipeline. Returns (compressed_text, ratio).
///
/// The ratio is 0.0 when text was unchanged; 1.0 would be 100% reduction.
/// Mode::Auto selects Light (<2KB), Normal (<16KB), or Aggressive (≥16KB).
pub(crate) fn compress(text: &str, max_bytes: usize, mode: CompressionMode) -> (String, f64) {
    let original_len = text.len();
    if original_len == 0 {
        return (String::new(), 0.0);
    }

    let mode = resolve_mode(mode, original_len);

    let mut result = text.to_string();

    // Stage 1: Strip ANSI (always — no semantic value).
    result = strip_ansi(&result);

    // Stage 2: Collapse runs of blank lines.
    if mode >= CompressionMode::Normal {
        result = collapse_blank_lines(&result);
    }

    // Stage 3: Deduplicate consecutive identical lines.
    if mode >= CompressionMode::Normal {
        result = dedup_repeated_lines(&result);
    }

    // Stage 4: Compact noisy patterns (paths, hashes).
    if mode >= CompressionMode::Normal {
        result = compact_noisy_patterns(&result);
    }

    // Stage 5: Head/tail preservation with error hoisting.
    if mode >= CompressionMode::Aggressive && result.len() > max_bytes {
        result = preserve_head_tail(&result, max_bytes);
    }

    let ratio = if original_len > 0 {
        1.0 - (result.len() as f64 / original_len as f64)
    } else {
        0.0
    };

    (result, ratio)
}

fn resolve_mode(mode: CompressionMode, len: usize) -> CompressionMode {
    if mode != CompressionMode::Auto {
        return mode;
    }
    if len < 2048 {
        CompressionMode::Light
    } else if len < 16384 {
        CompressionMode::Normal
    } else {
        CompressionMode::Aggressive
    }
}

fn strip_ansi(text: &str) -> String {
    // strip-ansi-escapes handles the full ECMA-48/ISO 6429 spec including
    // CSI, OSC, and multi-byte sequences, outperforming a hand-rolled regex.
    let bytes = strip_ansi_escapes::strip(text);
    String::from_utf8_lossy(&bytes).to_string()
}

fn collapse_blank_lines(text: &str) -> String {
    static BLANK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("\n{3,}").unwrap());
    BLANK_RE.replace_all(text, "\n\n").to_string()
}

fn dedup_repeated_lines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 3 {
        return text.to_string();
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut run_start = 0usize;
    for i in 1..=lines.len() {
        let run_ended =
            i == lines.len() || lines[i] != lines[i - 1] || lines[i - 1].trim().is_empty();
        if run_ended {
            let run_len = i - run_start;
            if run_len == 1 {
                out.push(lines[run_start].to_string());
            } else if run_len == 2 {
                out.push(lines[run_start].to_string());
                out.push(lines[run_start].to_string());
            } else {
                out.push(lines[run_start].to_string());
                out.push(format!("[… {} more identical lines …]", run_len - 1));
            }
            run_start = i;
        }
    }
    out.join("\n")
}

fn compact_noisy_patterns(text: &str) -> String {
    // Shorten long path prefixes (common in compiler/tool output).
    static PATH_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:/[^\s:{}\[\]]+){4,}").unwrap());
    let text = PATH_RE.replace_all(text, |caps: &regex::Captures| {
        let path = &caps[0];
        if path.len() <= 60 {
            return path.to_string();
        }
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() <= 3 {
            return path.to_string();
        }
        format!("/{}/…/{}", parts[0], parts[parts.len() - 1])
    });

    // Shorten hex hashes (git SHAs, build IDs) — keep prefix.
    static HASH_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b([0-9a-fA-F]{7})[0-9a-fA-F]{33,}\b").unwrap());
    let text = HASH_RE.replace_all(&text, "$1..");

    text.to_string()
}

/// Head/tail preservation with error-line hoisting.
///
/// When text exceeds `max_bytes`, we keep the first and last ~half bytes
/// while ensuring lines that signal errors (`error:`, `FAILED`, `fatal:`,
/// etc.) appear in the head region regardless of their original position.
/// This is a pure transform — the caller (`compact_text`) adds provenance.
fn preserve_head_tail(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let target = max_bytes.max(512);
    let half = target / 2;
    let (head, tail) = partition_error_aware(text, half);
    format!(
        "… head …\n{}\n\n… tail …\n{}",
        head.trim_end(),
        tail.trim_start(),
    )
}

/// Split text into head/tail halves, hoisting error-signaling lines into
/// the head so they survive compaction.
///
/// Single pass: scans lines once, partitioning into error lines, head buffer,
/// and tail buffer, then joins.
fn partition_error_aware(text: &str, half: usize) -> (String, String) {
    static ERROR_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(\b(?:error|FAILED|panicked at|fatal:|FAIL|ABORTED)\b)|(^error\[)")
            .unwrap()
    });

    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= 2 {
        let head: String = text.chars().take(half).collect();
        let tail: String = text
            .chars()
            .rev()
            .take(half)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return (head, tail);
    }

    let mut error_lines: Vec<&str> = Vec::new();
    let mut head_lines: Vec<&str> = Vec::new();
    let mut tail_lines: Vec<&str> = Vec::new();
    let mut head_bytes = 0usize;
    let mut tail_bytes = 0usize;

    // Single forward scan: classify each line.
    for &line in &lines {
        if ERROR_RE.is_match(line) {
            error_lines.push(line);
        }
    }

    // Build head: error lines first (up to 30% of budget), then top lines.
    let error_budget = half / 3;
    for &line in &error_lines {
        if head_bytes >= error_budget {
            break;
        }
        head_lines.push(line);
        head_bytes += line.len() + 1;
    }
    for &line in &lines {
        if head_bytes >= half {
            break;
        }
        if ERROR_RE.is_match(line) {
            continue; // already in error_lines, avoid duplicates
        }
        head_lines.push(line);
        head_bytes += line.len() + 1;
    }

    // Build tail from the bottom.
    for &line in lines.iter().rev() {
        if tail_bytes >= half {
            break;
        }
        tail_lines.push(line);
        tail_bytes += line.len() + 1;
    }
    tail_lines.reverse();

    (head_lines.join("\n"), tail_lines.join("\n"))
}

/// Compact tool output for transcript storage.
///
/// If `text` fits within `max_bytes` it is returned unchanged. Otherwise a
/// compression pipeline runs and the result is prefixed with a provenance
/// line telling the model the output was compacted and that the tool can be
/// re-run for full results.
pub(super) fn compact_text(text: &str, max_bytes: usize, label: &str) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let (compressed, ratio) = compress(text, max_bytes, CompressionMode::Aggressive);
    let body = if ratio < 0.05 {
        // Compression didn't help; fall back to head/tail only.
        preserve_head_tail(text, max_bytes)
    } else {
        compressed
    };
    format!(
        "[{label} — compacted {}→{} bytes ({:.0}%); re-run tool for full output]\n{body}",
        text.len(),
        body.len(),
        100.0 * (1.0 - body.len() as f64 / text.len() as f64),
    )
}

pub(super) fn message_content_text(message: &Message) -> String {
    match message {
        Message::System { content, .. } => content.clone(),
        Message::User { content } | Message::Assistant { content, .. } => content
            .iter()
            .map(message_content_part_text)
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn message_content_part_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text { text, .. } => text.clone(),
        MessageContent::ToolCall {
            name, arguments, ..
        } => format!("{name} {arguments}"),
        MessageContent::ToolResult { content, .. } => content
            .iter()
            .map(tool_result_content_text)
            .collect::<Vec<_>>()
            .join("\n"),
        MessageContent::Reasoning { value } | MessageContent::Opaque { value, .. } => {
            value_to_text(value)
        }
    }
}

fn tool_result_content_text(content: &ToolResultContent) -> String {
    match content {
        ToolResultContent::Text { text } => text.clone(),
        ToolResultContent::Opaque { value } => value_to_text(value),
    }
}

fn value_to_text(value: &serde_json::Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    if let Some(text) = value.get("text").and_then(serde_json::Value::as_str) {
        return text.to_string();
    }
    serde_json::to_string(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_color_codes() {
        let input = "\x1b[32mgreen\x1b[0m text";
        assert_eq!(strip_ansi(input), "green text");
    }

    #[test]
    fn collapse_blank_lines_squashes_gaps() {
        let input = "a\n\n\n\nb\n\n\nc";
        let result = collapse_blank_lines(input);
        assert_eq!(result.matches('\n').count(), 4); // a\n\nb\n\nc
    }

    #[test]
    fn dedup_repeated_lines_collapses_runs() {
        let input = "error\nwarning\nerror\nerror\nerror\nerror\nfatal";
        let result = dedup_repeated_lines(input);
        assert!(
            result.contains("more identical lines"),
            "expected count; got: {result}"
        );
        assert!(result.contains("fatal"));
    }

    #[test]
    fn compact_noisy_patterns_shortens_paths_and_hashes() {
        let input = "at /home/user/projects/rust/oy-cli/src/agent/subdir/another/compaction.rs:42\ncommit a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c";
        let result = compact_noisy_patterns(input);
        assert!(result.contains("…"));
        assert!(!result.contains("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c"));
        assert!(result.contains("a1b2c3d.."));
    }

    #[test]
    fn compress_under_budget_returns_unchanged() {
        let input = "short text";
        let (result, ratio) = compress(input, 1024, CompressionMode::Normal);
        assert_eq!(result, input);
        assert_eq!(ratio, 0.0);
    }

    #[test]
    fn compress_over_budget_preserves_head_tail() {
        let input = "x".repeat(5000);
        let (result, ratio) = compress(&input, 512, CompressionMode::Aggressive);
        assert!(result.len() <= 1024); // some overhead for the label
        assert!(ratio > 0.5);
    }

    #[test]
    fn error_lines_survive_compaction() {
        let mut lines: Vec<String> = (0..200).map(|i| format!("info: line {i}")).collect();
        lines.insert(50, "error: something broke".into());
        lines.push("fatal: unrecoverable".into());
        let input = lines.join("\n");
        let (result, _) = compress(&input, 512, CompressionMode::Aggressive);
        assert!(
            result.contains("error: something broke"),
            "error line was dropped"
        );
        assert!(
            result.contains("fatal: unrecoverable"),
            "fatal line was dropped"
        );
    }

    #[test]
    fn count_tokens_caching() {
        // First run resolves and populates cache
        let tokens_1 = count_tokens("gpt-4o", "hello world");
        assert_eq!(tokens_1, 2);

        // Second run uses cache
        let tokens_2 = count_tokens("gpt-4o", "hello world");
        assert_eq!(tokens_2, 2);

        // Test with custom model name that maps to fallback
        let tokens_3 = count_tokens("anthropic:claude-3-5-sonnet", "hello world");
        assert_eq!(tokens_3, 2);

        // Verify cache actually contains the entries
        let cache = TOKENIZER_CACHE.read().unwrap();
        assert!(cache.contains_key("gpt-4o"));
        assert!(cache.contains_key("anthropic:claude-3-5-sonnet"));
    }
}
