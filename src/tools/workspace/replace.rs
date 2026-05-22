use anyhow::{Context, Result, bail};
use globset::GlobSet;
use regex::Regex;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config;

use super::super::args::{ExcludeArg, ReplaceArgs, ReplaceMode};
use super::super::{Approval, PREVIEW_ITEMS, ToolContext, require_mutation_approval};
use super::MAX_WORKSPACE_FILE_BYTES;
use super::diff::{combined_diff, unified_diff};
use super::discovery::{build_exclude_set, fff_indexed_files};
use super::output::{ChangedFileOutput, ReplaceOutput, SkippedFileOutput, ToolErrorItem};
use super::paths::{rel_path, resolve_existing_path};

struct ReplacePlan {
    path: PathBuf,
    display_path: String,
    updated: String,
    replacements: usize,
    diff: String,
}

struct ReplacePlanOutput {
    plans: Vec<ReplacePlan>,
    skipped: Vec<SkippedFileOutput>,
    errors: Vec<ToolErrorItem>,
    replacement_count: usize,
}

enum ReplacePlanItem {
    Changed(ReplacePlan),
    Unchanged,
    Skipped(SkippedFileOutput),
}

fn replace_matcher_and_replacement(args: &ReplaceArgs) -> Result<(Regex, String, &'static str)> {
    match args.mode {
        ReplaceMode::Regex => Ok((
            Regex::new(&args.pattern)
                .with_context(|| format!("invalid regex: {}", args.pattern))?,
            args.replacement.clone(),
            "regex",
        )),
        ReplaceMode::Literal => Ok((
            Regex::new(&regex::escape(&args.pattern))?,
            args.replacement.replace('$', "$$"),
            "literal",
        )),
    }
}

pub(crate) fn tool_replace(ctx: &ToolContext, args: ReplaceArgs) -> Result<Value> {
    let (regex, replacement, mode) = replace_matcher_and_replacement(&args)?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let target = resolve_existing_path(ctx, &args.path)?;
    let mut preview_plan = None;
    let approval_preview = if ctx.policy().approval("replace") == Approval::Ask && ctx.interactive()
    {
        let plan = plan_replace(ctx, &target, &exclude, &regex, &replacement)?;
        let preview = preview_replace_plan(&plan, args.limit);
        preview_plan = Some(plan);
        Some(preview)
    } else {
        None
    };
    require_mutation_approval(ctx, "replace", approval_preview.as_deref())?;

    let plan = match preview_plan {
        Some(plan) => plan,
        None => plan_replace(ctx, &target, &exclude, &regex, &replacement)?,
    };
    let writes = plan
        .plans
        .iter()
        .map(|item| config::WorkspaceWrite::new(&item.path, item.updated.as_bytes()))
        .collect::<Vec<_>>();
    config::write_workspace_batch(&writes)?;

    let shown = args.limit.max(1);
    let changed_file_count = plan.plans.len();
    let changed_files = plan
        .plans
        .iter()
        .map(|item| ChangedFileOutput {
            path: item.display_path.clone(),
            replacements: item.replacements,
            diff: item.diff.clone(),
        })
        .collect::<Vec<_>>();
    let diff = combined_diff(&changed_files);
    Ok(serde_json::to_value(ReplaceOutput {
        pattern: args.pattern,
        replacement: args.replacement,
        mode,
        path: args.path,
        changed_file_count,
        replacement_count: plan.replacement_count,
        changed_files: changed_files.into_iter().take(shown).collect(),
        diff,
        truncated: changed_file_count > shown,
        exclude: args.exclude.as_ref().map(ExcludeArg::patterns),
        skipped: plan.skipped,
        errors: plan.errors,
    })?)
}

fn plan_replace(
    ctx: &ToolContext,
    target: &Path,
    exclude: &GlobSet,
    regex: &Regex,
    replacement: &str,
) -> Result<ReplacePlanOutput> {
    let mut plans = Vec::new();
    let mut skipped = Vec::new();
    let mut errors = Vec::new();
    let mut replacement_count = 0usize;
    for path in fff_indexed_files(ctx.root(), target, exclude)? {
        match plan_replace_file(ctx.root(), &path, regex, replacement) {
            Ok(ReplacePlanItem::Changed(plan)) => {
                replacement_count += plan.replacements;
                plans.push(plan);
            }
            Ok(ReplacePlanItem::Unchanged) => {}
            Ok(ReplacePlanItem::Skipped(item)) => skipped.push(item),
            Err(err) => errors.push(ToolErrorItem {
                path: rel_path(ctx.root(), &path),
                message: err.to_string(),
            }),
        }
    }
    Ok(ReplacePlanOutput {
        plans,
        skipped,
        errors,
        replacement_count,
    })
}

fn plan_replace_file(
    root: &Path,
    path: &Path,
    regex: &Regex,
    replacement: &str,
) -> Result<ReplacePlanItem> {
    let display_path = rel_path(root, path);
    if path.is_symlink() {
        return Ok(ReplacePlanItem::Skipped(SkippedFileOutput {
            path: display_path,
            reason: "symlink",
        }));
    }
    if fs::metadata(path)?.len() > MAX_WORKSPACE_FILE_BYTES {
        return Ok(ReplacePlanItem::Skipped(SkippedFileOutput {
            path: display_path,
            reason: "file exceeds workspace read cap",
        }));
    }
    let raw = fs::read(path)?;
    let text = match crate::decode_utf8(raw) {
        Ok(text) => text,
        Err(crate::TextDecodeError::Binary) => {
            return Ok(ReplacePlanItem::Skipped(SkippedFileOutput {
                path: display_path,
                reason: "binary file",
            }));
        }
        Err(crate::TextDecodeError::NonUtf8) => bail!("cannot decode utf-8"),
    };
    let replacements = regex.find_iter(&text).count();
    if replacements == 0 {
        return Ok(ReplacePlanItem::Unchanged);
    }
    let updated = regex.replace_all(&text, replacement).into_owned();
    let diff = unified_diff(&display_path, &text, &updated);
    Ok(ReplacePlanItem::Changed(ReplacePlan {
        path: path.to_path_buf(),
        display_path,
        updated,
        replacements,
        diff,
    }))
}

fn preview_replace_plan(plan: &ReplacePlanOutput, limit: usize) -> String {
    let changed = plan
        .plans
        .iter()
        .take(limit.clamp(1, PREVIEW_ITEMS))
        .map(|item| ChangedFileOutput {
            path: item.display_path.clone(),
            replacements: item.replacements,
            diff: item.diff.clone(),
        })
        .collect::<Vec<_>>();
    combined_diff(&changed)
}
