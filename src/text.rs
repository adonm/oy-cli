use std::fmt::Write as _;

pub fn compact_spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn truncate_chars(text: &str, max: usize) -> String {
    let limit = max.saturating_sub(3);
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= limit {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

pub fn compact_preview(text: &str, max: usize) -> String {
    truncate_chars(&compact_spaces(text), max)
}

pub fn clamp_lines(text: &str, max_lines: usize, max_cols: usize) -> String {
    let mut out = String::new();
    let mut total = 0usize;
    for (idx, line) in text.lines().enumerate() {
        total = idx + 1;
        if idx < max_lines {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&truncate_chars(line, max_cols));
        }
    }
    if total > max_lines {
        let _ = write!(out, "\n… {} more lines", total - max_lines);
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
        format!("{head}\n... [truncated {hidden} chars] ...\n{tail}"),
        true,
    )
}
