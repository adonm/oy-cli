use anyhow::{Result, bail};
use serde_json::Value;
use std::fs;
use std::path::Path;

use super::super::ToolContext;
use super::super::args::ReadArgs;
use super::MAX_WORKSPACE_FILE_BYTES;
use super::output::ReadOutput;
use super::paths::{rel_path, resolve_read_path};

pub(crate) fn tool_read(ctx: &ToolContext, args: ReadArgs) -> Result<Value> {
    let path = resolve_read_path(ctx, &args.path)?;
    if path.is_dir() {
        bail!("read path is a directory: {}", args.path);
    }
    let Some(item) = read_text_file(ctx.root(), &path)? else {
        bail!("read path is not utf-8 text: {}", args.path);
    };
    let display_path = item.display_path;
    let text = item.text;
    let mut shown = Vec::new();
    let start = args.offset.saturating_sub(1);
    let stop = start + args.limit.max(1);
    let mut line_count = 0usize;
    for (idx, line) in text.lines().enumerate() {
        line_count = idx + 1;
        if idx < start {
            continue;
        }
        if idx < stop {
            shown.push(line.to_string());
        }
    }
    let truncated = line_count > stop;
    Ok(serde_json::to_value(ReadOutput {
        path: display_path,
        offset: args.offset,
        limit: args.limit,
        text: shown.join("\n"),
        line_count,
        truncated,
    })?)
}

struct SearchText {
    display_path: String,
    text: String,
}

fn read_text_file(root: &Path, path: &Path) -> Result<Option<SearchText>> {
    let rel = rel_path(root, path);
    if fs::metadata(path)?.len() > MAX_WORKSPACE_FILE_BYTES {
        bail!(
            "file exceeds workspace read cap of {} bytes: {rel}",
            MAX_WORKSPACE_FILE_BYTES
        );
    }
    let raw = fs::read(path)?;
    Ok(crate::decode_utf8(raw).ok().map(|text| SearchText {
        display_path: rel,
        text,
    }))
}
