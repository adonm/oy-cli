//! Preview functions for network tools: webfetch and repo_clone.

use serde_json::Value;

use super::common::*;

pub(crate) fn summary_webfetch(args: &Value) -> String {
    compact_kvs(args, &[("return_format", 16), ("url", 100)])
}

pub(crate) fn preview_webfetch(value: &Value) -> String {
    let status = value
        .get("status_code")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let url = value_str(value, "url");
    let text = value.get("content").and_then(Value::as_str).unwrap_or("");
    let links = value
        .get("links")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let summary = format!(
        "HTTP {status} · scrape · {} line{} · {links} link{} · {url}",
        count_lines(text),
        plural(count_lines(text)),
        plural(links)
    );
    with_verbose(summary, || {
        let mut out = String::new();
        if !text.is_empty() {
            append_preview_lines(&mut out, text, "content");
        }
        if value_bool(value, "truncated") {
            out.push_str("\n  … response body truncated for model context");
        }
        out.trim_start().to_string()
    })
}

pub(crate) fn summary_repo_clone(args: &Value) -> String {
    compact_kvs(args, &[("repository", 80), ("branch", 30)])
}

pub(crate) fn preview_repo_clone(value: &Value) -> String {
    let repo = value_str(value, "repository");
    let status = value_str(value, "status");
    let path = value_str(value, "local_path");
    let branch = value.get("branch").and_then(Value::as_str).unwrap_or("");
    let head = value.get("head").and_then(Value::as_str).unwrap_or("");
    let mut out = format!("repo_clone · {status} · {repo}\n  path: {path}");
    if !branch.is_empty() {
        out.push_str(&format!("\n  branch: {branch}"));
    }
    if !head.is_empty() {
        let short = if head.len() > 8 { &head[..8] } else { head };
        out.push_str(&format!("\n  head: {short}"));
    }
    out
}
