//! Text shaping helpers: space compaction, char/width truncation,
//! line clamping, and head/tail splitting.

use std::fmt::Write as _;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn compact_spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn truncate_chars(text: &str, max: usize) -> String {
    truncate_width(text, max)
}

pub fn truncate_width(text: &str, max_width: usize) -> String {
    if ansi_stripped_width(text) <= max_width {
        return text.to_string();
    }
    truncate_ansi_width(text, max_width)
}

fn truncate_ansi_width(text: &str, max_width: usize) -> String {
    let ellipsis = "…";
    let limit = max_width.saturating_sub(UnicodeWidthStr::width(ellipsis));
    let mut out = String::new();
    let mut width = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            out.push(ch);
            out.push(chars.next().expect("peeked CSI introducer"));
            for next in chars.by_ref() {
                out.push(next);
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
            continue;
        }

        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > limit {
            break;
        }
        width += ch_width;
        out.push(ch);
    }
    out.push_str(ellipsis);
    if text.contains("\u{1b}[") {
        out.push_str("\u{1b}[0m");
    }
    out
}

pub(super) fn ansi_stripped_width(text: &str) -> usize {
    let mut width = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else {
            width += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    width
}

pub fn compact_preview(text: &str, max: usize) -> String {
    truncate_width(&compact_spaces(text), max)
}

pub fn clamp_lines(text: &str, max_lines: usize, max_cols: usize) -> String {
    let mut out = String::new();
    let lines = text.lines().collect::<Vec<_>>();
    for line in lines.iter().take(max_lines) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&truncate_width(line, max_cols));
    }
    if lines.len() > max_lines {
        let _ = write!(out, "\n… {} more lines", lines.len() - max_lines);
    }
    out
}

pub fn head_tail(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let head_len = max_chars / 2;
    let tail_len = max_chars.saturating_sub(head_len);
    let head = text.chars().take(head_len).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let hidden = text
        .chars()
        .count()
        .saturating_sub(head.chars().count() + tail.chars().count());
    (
        format!("{head}\n… [truncated {hidden} chars] …\n{tail}"),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_width_preserves_ansi_prefix_and_resets() {
        let text = "\u{1b}[31mabcdef\u{1b}[0m";
        let truncated = truncate_width(text, 4);
        assert_eq!(ansi_stripped_width(&truncated), 4);
        assert!(truncated.starts_with("\u{1b}[31mabc…"), "{truncated:?}");
        assert!(truncated.ends_with("\u{1b}[0m"), "{truncated:?}");
    }

    #[test]
    fn truncate_width_counts_wide_chars_after_ansi_codes() {
        let text = "\u{1b}[32m你好world\u{1b}[0m";
        let truncated = truncate_width(text, 5);
        assert!(ansi_stripped_width(&truncated) <= 5, "{truncated:?}");
        assert!(truncated.ends_with("\u{1b}[0m"), "{truncated:?}");
    }
}
