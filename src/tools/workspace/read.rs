use anyhow::{Result, bail};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::super::ToolContext;
use super::super::args::{ReadArgs, ReadMultipleFilesArgs};
use super::MAX_WORKSPACE_FILE_BYTES;
use super::output::{ReadOutput, ReadMultipleFilesOutput};
use super::paths::{rel_path, resolve_read_path};

fn compute_checksum(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

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
    let lines: Vec<&str> = text.lines().collect();
    let line_count = lines.len();

    let (start, stop) = if let Some(tail) = args.tail_lines {
        let tail_start = line_count.saturating_sub(tail);
        (tail_start, line_count)
    } else {
        let start = args.offset.saturating_sub(1);
        let stop = start + args.limit.max(1);
        (start, stop.min(line_count))
    };

    let shown: Vec<&str> = lines[start..stop].to_vec();
    let truncated = stop < line_count;
    let checksum = Some(compute_checksum(&text));

    Ok(serde_json::to_value(ReadOutput {
        path: display_path,
        offset: start + 1,
        limit: stop - start,
        text: shown.join("\n"),
        line_count,
        truncated,
        checksum,
    })?)
}

pub(crate) fn tool_read_multiple_files(
    ctx: &ToolContext,
    args: ReadMultipleFilesArgs,
) -> Result<Value> {
    if args.files.len() > 20 {
        bail!("read_multiple_files supports at most 20 files per call");
    }

    let mut results = Vec::new();

    for file_req in args.files {
        let path = resolve_read_path(ctx, &file_req.path)?;
        if path.is_dir() {
            bail!("read path is a directory: {}", file_req.path);
        }
        let Some(item) = read_text_file(ctx.root(), &path)? else {
            bail!("read path is not utf-8 text: {}", file_req.path);
        };

        let lines: Vec<&str> = item.text.lines().collect();
        let line_count = lines.len();

        let (start, stop) = if let Some(tail) = file_req.tail_lines {
            let tail_start = line_count.saturating_sub(tail);
            (tail_start, line_count)
        } else {
            let start = file_req.offset.saturating_sub(1);
            let stop = start + file_req.limit.max(1);
            (start, stop.min(line_count))
        };

        let shown: Vec<&str> = lines[start..stop].to_vec();
        let truncated = stop < line_count;
        let checksum = Some(compute_checksum(&item.text));

        results.push(ReadOutput {
            path: item.display_path,
            offset: start + 1,
            limit: stop - start,
            text: shown.join("\n"),
            line_count,
            truncated,
            checksum,
        });
    }

    Ok(serde_json::to_value(ReadMultipleFilesOutput { files: results })?)
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

/// Public wrapper for reading file content, used by outline and other tools
pub(crate) fn read_file_content(_root: &Path, path: &Path) -> Result<String> {
    if fs::metadata(path)?.len() > MAX_WORKSPACE_FILE_BYTES {
        bail!(
            "file exceeds workspace read cap of {} bytes",
            MAX_WORKSPACE_FILE_BYTES
        );
    }
    let raw = fs::read(path)?;
    crate::decode_utf8(raw).map_err(|_| anyhow::anyhow!("file is not valid UTF-8"))
}
