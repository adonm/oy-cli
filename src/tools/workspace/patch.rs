use anyhow::{Context, Result, anyhow, bail};
use diffy::apply;
use diffy::patch_set::{FileOperation, FilePatch, ParseOptions, PatchSet};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use crate::config;

use super::super::args::PatchArgs;
use super::super::{Approval, ToolContext, require_mutation_approval};
use super::MAX_WORKSPACE_FILE_BYTES;
use super::diff::unified_diff;
use super::output::{PatchChangedFileOutput, PatchOutput};
use super::paths::{rel_path, resolve_existing_path};

struct PatchPlan {
    path: PathBuf,
    display_path: String,
    updated: String,
    diff: String,
}

struct ApplyPatchFile {
    path: String,
    hunks: Vec<ApplyPatchHunk>,
}

struct ApplyPatchHunk {
    anchor: Option<String>,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
}

pub(crate) fn tool_patch(ctx: &ToolContext, mut args: PatchArgs) -> Result<Value> {
    // diffy parses incorrectly when the patch doesn't end with a newline:
    // Insert lines lack trailing \n (silent corruption) and context lines
    // fail to match (apply error).
    if !args.patch.ends_with('\n') {
        args.patch.push('\n');
    }
    let (patch_count, plans) = plan_patch(ctx, &args)?;
    let approval_preview = if ctx.policy().approval("patch") == Approval::Ask && ctx.interactive() {
        Some(combined_patch_diff(&plans))
    } else {
        None
    };
    require_mutation_approval(ctx, "patch", approval_preview.as_deref())?;

    let writes = plans
        .iter()
        .map(|plan| config::WorkspaceWrite::new(&plan.path, plan.updated.as_bytes()))
        .collect::<Vec<_>>();
    config::write_workspace_batch(&writes)?;

    let shown = args.limit.max(1);
    let changed_file_count = plans.len();
    let diff = combined_patch_diff(&plans);
    Ok(serde_json::to_value(PatchOutput {
        patch_count,
        changed_file_count,
        changed_files: plans
            .into_iter()
            .take(shown)
            .map(|plan| PatchChangedFileOutput {
                path: plan.display_path,
                diff: plan.diff,
            })
            .collect(),
        diff,
        truncated: changed_file_count > shown,
    })?)
}

fn parse_patch_set(text: &str) -> Result<Vec<FilePatch<'_, str>>> {
    let git =
        PatchSet::parse(text, ParseOptions::gitdiff()).collect::<std::result::Result<Vec<_>, _>>();
    match git {
        Ok(patches) if !patches.is_empty() => Ok(patches),
        _ => PatchSet::parse(text, ParseOptions::unidiff())
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("invalid patch"),
    }
}

fn plan_patch(ctx: &ToolContext, args: &PatchArgs) -> Result<(usize, Vec<PatchPlan>)> {
    if is_apply_patch_format(args.patch.as_str()) {
        return plan_apply_patch(ctx, args.patch.as_str());
    }
    let patches = parse_patch_set(args.patch.as_str())?;
    if patches.is_empty() {
        bail!("patch did not contain any file changes");
    }

    let patch_count = patches.len();
    let mut seen = BTreeSet::new();
    let mut plans = Vec::new();
    for file_patch in patches {
        let (patch_path, path) = resolve_patch_target(ctx, &file_patch, args.strip)?;
        let updated = build_patch_plan(ctx, &path, &patch_path, |text| {
            let text_patch = file_patch
                .patch()
                .as_text()
                .ok_or_else(|| anyhow!("binary patches are not supported: {patch_path}"))?;
            apply(text, text_patch).map_err(|err| {
                anyhow!(
                    "failed applying patch for {patch_path}: {err}; re-read the file and regenerate the hunk with current context"
                )
            })
        })?;
        let Some(plan) = updated else { continue };
        if !seen.insert(plan.display_path.clone()) {
            bail!(
                "patch contains multiple changes for the same file: {}",
                plan.display_path
            );
        }
        plans.push(plan);
    }
    Ok((patch_count, plans))
}

fn is_apply_patch_format(text: &str) -> bool {
    text.trim_start().starts_with("*** Begin Patch")
}

fn plan_apply_patch(ctx: &ToolContext, text: &str) -> Result<(usize, Vec<PatchPlan>)> {
    let files = parse_apply_patch(text)?;
    let patch_count = files.len();
    let mut seen = BTreeSet::new();
    let mut plans = Vec::new();

    for file in files {
        let path = resolve_existing_path(ctx, &file.path)?;
        let patch_path = file.path.clone();
        let updated = build_patch_plan(ctx, &path, &patch_path, |text| {
            apply_context_hunks(text, &file)
        })?;
        let Some(plan) = updated else { continue };
        if !seen.insert(plan.display_path.clone()) {
            bail!(
                "patch contains multiple changes for the same file: {}",
                plan.display_path
            );
        }
        plans.push(plan);
    }

    Ok((patch_count, plans))
}

/// Shared per-file pipeline for the two patch formats.
///
/// `apply` is the only step that differs between unified diff and
/// `*** Begin Patch`: it receives the decoded file text and must return
/// the new text. The helper owns the directory/symlink/size/read/decode
/// guards, the skip-if-unchanged short-circuit, the display-path dedup
/// check, and the diff computation.
///
/// Returns `Ok(None)` when the patched text is identical to the input
/// (no work to write), and `Ok(Some(plan))` otherwise.
fn build_patch_plan(
    ctx: &ToolContext,
    path: &std::path::Path,
    patch_path: &str,
    apply: impl FnOnce(&str) -> Result<String>,
) -> Result<Option<PatchPlan>> {
    if path.is_dir() {
        bail!("cannot patch directory: {patch_path}");
    }
    if ctx.root().join(patch_path).is_symlink() || path.is_symlink() {
        bail!("cannot patch symlink: {patch_path}");
    }
    if fs::metadata(path)?.len() > MAX_WORKSPACE_FILE_BYTES {
        bail!("cannot patch file over workspace read cap: {patch_path}");
    }

    let raw = fs::read(path)?;
    let text = match crate::decode_utf8(raw) {
        Ok(text) => text,
        Err(crate::TextDecodeError::Binary) => bail!("cannot patch binary file: {patch_path}"),
        Err(crate::TextDecodeError::NonUtf8) => bail!("cannot decode utf-8: {patch_path}"),
    };
    let updated = apply(&text)?;
    if updated == text {
        return Ok(None);
    }

    let display_path = rel_path(ctx.root(), path);
    let diff = unified_diff(&display_path, &text, &updated);
    Ok(Some(PatchPlan {
        path: path.to_path_buf(),
        display_path,
        updated,
        diff,
    }))
}

fn parse_apply_patch(text: &str) -> Result<Vec<ApplyPatchFile>> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut index = 0;
    while lines.get(index).is_some_and(|line| line.trim().is_empty()) {
        index += 1;
    }
    if lines.get(index).map(|line| line.trim()) != Some("*** Begin Patch") {
        bail!("invalid patch");
    }
    index += 1;

    let mut files = Vec::new();
    let mut saw_end = false;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        if trimmed == "*** End Patch" {
            saw_end = true;
            index += 1;
            break;
        }
        if trimmed.is_empty() {
            index += 1;
            continue;
        }
        if line.starts_with("*** Add File:") {
            bail!("file creation patches are not supported");
        }
        if line.starts_with("*** Delete File:") {
            bail!("file deletion patches are not supported");
        }
        let Some(path) = line.strip_prefix("*** Update File:") else {
            bail!("invalid patch");
        };
        let path = path.trim();
        if path.is_empty() {
            bail!("patch path is empty");
        }
        index += 1;

        let mut hunks = Vec::new();
        while index < lines.len() {
            let line = lines[index];
            if line.trim() == "*** End Patch" || line.starts_with("*** Update File:") {
                break;
            }
            if line.starts_with("*** Add File:") {
                bail!("file creation patches are not supported");
            }
            if line.starts_with("*** Delete File:") {
                bail!("file deletion patches are not supported");
            }
            if line.trim().is_empty() {
                index += 1;
                continue;
            }
            let Some(anchor) = line.strip_prefix("@@") else {
                bail!("invalid patch");
            };
            index += 1;
            let anchor = anchor
                .trim()
                .trim_matches('@')
                .trim()
                .strip_prefix(' ')
                .unwrap_or_else(|| anchor.trim().trim_matches('@').trim())
                .trim()
                .to_string();
            let anchor = (!anchor.is_empty()).then_some(anchor);
            let mut old_lines = Vec::new();
            let mut new_lines = Vec::new();

            while index < lines.len() {
                let line = lines[index];
                if line.starts_with("@@")
                    || line.trim() == "*** End Patch"
                    || line.starts_with("*** Update File:")
                    || line.starts_with("*** Add File:")
                    || line.starts_with("*** Delete File:")
                {
                    break;
                }
                if line == r"\ No newline at end of file" {
                    index += 1;
                    continue;
                }
                let Some(prefix) = line.chars().next() else {
                    bail!("invalid patch");
                };
                let content = format!("{}\n", &line[prefix.len_utf8()..]);
                match prefix {
                    ' ' => {
                        old_lines.push(content.clone());
                        new_lines.push(content);
                    }
                    '-' => old_lines.push(content),
                    '+' => new_lines.push(content),
                    _ => bail!("invalid patch"),
                }
                index += 1;
            }
            if old_lines.is_empty() && new_lines.is_empty() {
                bail!("invalid patch");
            }
            hunks.push(ApplyPatchHunk {
                anchor,
                old_lines,
                new_lines,
            });
        }
        if hunks.is_empty() {
            bail!("patch did not contain any file changes");
        }
        files.push(ApplyPatchFile {
            path: path.to_string(),
            hunks,
        });
    }

    if !saw_end {
        bail!("invalid patch");
    }
    if lines[index..].iter().any(|line| !line.trim().is_empty()) {
        bail!("invalid patch");
    }
    if files.is_empty() {
        bail!("patch did not contain any file changes");
    }
    Ok(files)
}

fn apply_context_hunks(text: &str, file: &ApplyPatchFile) -> Result<String> {
    let mut lines = split_preserving_newlines(text);
    let mut cursor = 0;
    for (idx, hunk) in file.hunks.iter().enumerate() {
        let start = if hunk.old_lines.is_empty() {
            if let Some(ref anchor) = hunk.anchor {
                find_anchor_line(&lines, anchor, cursor)
                    .or_else(|| find_anchor_line(&lines, anchor, 0))
                    .ok_or_else(|| {
                        anyhow!(
                            "failed applying patch for {}: context hunk #{} anchor '{}' not found; re-read the file and regenerate the hunk with current context",
                            file.path,
                            idx + 1,
                            anchor
                        )
                    })?
            } else {
                cursor
            }
        } else {
            let anchor_start = hunk
                .anchor
                .as_ref()
                .and_then(|anchor| find_anchor_line(&lines, anchor, cursor))
                .unwrap_or(cursor);
            find_line_sequence(&lines, &hunk.old_lines, anchor_start)
                .or_else(|| find_line_sequence(&lines, &hunk.old_lines, cursor))
                .or_else(|| find_line_sequence(&lines, &hunk.old_lines, 0))
                .ok_or_else(|| {
                    anyhow!(
                        "failed applying patch for {}: context hunk #{} did not match; re-read the file and regenerate the hunk with current context",
                        file.path,
                        idx + 1
                    )
                })?
        };
        lines.splice(start..start + hunk.old_lines.len(), hunk.new_lines.clone());
        cursor = start + hunk.new_lines.len();
    }
    Ok(lines.concat())
}

fn split_preserving_newlines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_string).collect()
}

fn find_anchor_line(lines: &[String], anchor: &str, start: usize) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .skip(start.min(lines.len()))
        .find(|(_, line)| line.trim_end_matches(['\r', '\n']).contains(anchor))
        .map(|(idx, _)| idx)
}

fn find_line_sequence(lines: &[String], needle: &[String], start: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > lines.len() {
        return None;
    }
    lines
        .windows(needle.len())
        .enumerate()
        .skip(start.min(lines.len()))
        .find(|(_, window)| *window == needle)
        .map(|(idx, _)| idx)
}

fn resolve_patch_target(
    ctx: &ToolContext,
    file_patch: &FilePatch<'_, str>,
    strip: usize,
) -> Result<(String, PathBuf)> {
    let mut errors = Vec::new();
    for candidate_strip in patch_strip_candidates(strip) {
        let operation = file_patch.operation().strip_prefix(candidate_strip);
        let patch_path = match patch_path_from_operation(&operation) {
            Ok(path) => path,
            Err(err) => {
                errors.push(format!("strip {candidate_strip}: {err}"));
                continue;
            }
        };
        match resolve_existing_path(ctx, &patch_path) {
            Ok(path) => return Ok((patch_path, path)),
            Err(err) => errors.push(format!("strip {candidate_strip}: {err}")),
        }
    }

    if errors.len() == 1 {
        return Err(anyhow!(errors.remove(0)));
    }
    bail!(
        "could not resolve patch path after trying strip values {}: {}",
        patch_strip_candidates(strip)
            .into_iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        errors.join("; ")
    );
}

fn patch_strip_candidates(strip: usize) -> Vec<usize> {
    if strip == 1 { vec![1, 0] } else { vec![strip] }
}

fn patch_path_from_operation(operation: &FileOperation<'_, str>) -> Result<String> {
    match operation {
        FileOperation::Modify { original, modified } if original == modified => {
            let path = original.as_ref();
            if path.trim().is_empty() {
                bail!("patch path is empty");
            }
            Ok(path.to_string())
        }
        FileOperation::Modify { original, modified } => bail!(
            "rename-style modify patches are not supported: {} -> {}",
            original.as_ref(),
            modified.as_ref()
        ),
        FileOperation::Create(path) => {
            bail!("file creation patches are not supported: {}", path.as_ref())
        }
        FileOperation::Delete(path) => {
            bail!("file deletion patches are not supported: {}", path.as_ref())
        }
        FileOperation::Rename { from, to } => bail!(
            "file rename patches are not supported: {} -> {}",
            from.as_ref(),
            to.as_ref()
        ),
        FileOperation::Copy { from, to } => bail!(
            "file copy patches are not supported: {} -> {}",
            from.as_ref(),
            to.as_ref()
        ),
    }
}

fn combined_patch_diff(files: &[PatchPlan]) -> String {
    let text = files
        .iter()
        .map(|item| item.diff.as_str())
        .filter(|diff| !diff.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    crate::ui::head_tail(&text, 12000).0
}
