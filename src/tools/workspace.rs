//! Workspace filesystem tools and path trust boundary.
//!
//! Listing, reading, searching, line counting, replacement, and patching all
//! validate paths against the configured workspace before touching the host.

use anyhow::{Context, Result, anyhow, bail};
use diffy::patch_set::{FileOperation, FilePatch, ParseOptions, PatchSet};
use diffy::{apply, create_patch};
use glob::glob;
use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;
use grep_searcher::sinks::UTF8;
use ignore::WalkBuilder;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use tokei::{Config as TokeiConfig, Languages as TokeiLanguages, Sort as TokeiSort};

use crate::config;

use super::args::{
    ExcludeArg, ListArgs, PatchArgs, ReadArgs, ReplaceArgs, ReplaceMode, SearchArgs, SearchMode,
    SlocArgs,
};
use super::{Approval, PREVIEW_ITEMS, ToolContext, require_mutation_approval};

pub(super) const MAX_WORKSPACE_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SEARCH_MATCHES: usize = 10_000;

#[derive(Debug, Serialize)]
pub(super) struct ListOutput {
    pub path: String,
    pub items: Vec<String>,
    pub count: usize,
    pub truncated: bool,
    pub exclude: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub(super) struct ReadOutput {
    pub path: String,
    pub offset: usize,
    pub limit: usize,
    pub text: String,
    pub line_count: usize,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct SearchHit {
    pub path: String,
    pub line_number: usize,
    pub column: usize,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub(super) struct ToolErrorItem {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub(super) struct SearchOutput {
    pub pattern: String,
    pub mode: &'static str,
    pub warning: Option<String>,
    pub path: String,
    pub match_count: usize,
    pub matches: Vec<SearchHit>,
    pub truncated: bool,
    pub exclude: Option<Vec<String>>,
    pub errors: Option<Vec<ToolErrorItem>>,
}

#[derive(Debug, Serialize)]
pub(super) struct ChangedFileOutput {
    pub path: String,
    pub replacements: usize,
    pub diff: String,
}

#[derive(Debug, Serialize)]
pub(super) struct SkippedFileOutput {
    pub path: String,
    pub reason: &'static str,
}

#[derive(Debug, Serialize)]
pub(super) struct ReplaceOutput {
    pub pattern: String,
    pub replacement: String,
    pub mode: &'static str,
    pub path: String,
    pub changed_file_count: usize,
    pub replacement_count: usize,
    pub changed_files: Vec<ChangedFileOutput>,
    pub diff: String,
    pub truncated: bool,
    pub exclude: Option<Vec<String>>,
    pub skipped: Vec<SkippedFileOutput>,
    pub errors: Vec<ToolErrorItem>,
}

#[derive(Debug, Serialize)]
pub(super) struct PatchChangedFileOutput {
    pub path: String,
    pub diff: String,
}

#[derive(Debug, Serialize)]
pub(super) struct PatchOutput {
    pub patch_count: usize,
    pub changed_file_count: usize,
    pub changed_files: Vec<PatchChangedFileOutput>,
    pub diff: String,
    pub truncated: bool,
}

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

#[derive(Debug, Serialize)]
pub(super) struct SlocOutput {
    pub path: String,
    pub format: &'static str,
    pub output: Value,
    pub exclude: Option<Vec<String>>,
}

// === Workspace tool implementations ===
pub(super) fn tool_list(ctx: &ToolContext, args: ListArgs) -> Result<Value> {
    reject_out_of_workspace_path(&ctx.root, &args.path, None)?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let shown_limit = args.limit.max(1);
    let items = if args.path == "." || args.path == "./" {
        let mut out = Vec::new();
        for entry in fs::read_dir(&ctx.root)? {
            let path = entry?.path();
            let rel = rel_path(&ctx.root, &path);
            if exclude.is_match(rel.as_str()) {
                continue;
            }
            out.push(display_path(&ctx.root, &path));
        }
        out.sort();
        out
    } else {
        let pattern = if Path::new(&args.path).is_absolute() {
            args.path.clone()
        } else {
            ctx.root.join(&args.path).to_string_lossy().to_string()
        };
        let mut out = glob(&pattern)?
            .filter_map(|entry| entry.ok())
            .filter_map(|path| safe_list_item(&ctx.root, &path))
            .filter(|item| !exclude.is_match(item.as_str()))
            .collect::<Vec<_>>();
        out.sort();
        out.dedup();
        out
    };
    Ok(serde_json::to_value(ListOutput {
        path: args.path,
        items: items.iter().take(shown_limit).cloned().collect(),
        count: items.len(),
        truncated: items.len() > shown_limit,
        exclude: args.exclude.as_ref().map(ExcludeArg::patterns),
    })?)
}

pub(super) fn tool_read(ctx: &ToolContext, args: ReadArgs) -> Result<Value> {
    let path = resolve_existing_path(ctx, &args.path)?;
    if path.is_dir() {
        bail!("read path is a directory: {}", args.path);
    }
    let Some(item) = read_text_file(&ctx.root, &path)? else {
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

fn search_matchers(
    pattern: &str,
    mode: SearchMode,
) -> Result<(RegexMatcher, Regex, &'static str, Option<String>)> {
    match mode {
        SearchMode::Regex => Ok((
            RegexMatcher::new_line_matcher(pattern)
                .with_context(|| format!("invalid regex: {pattern}"))?,
            Regex::new(pattern).with_context(|| format!("invalid regex: {pattern}"))?,
            "regex",
            None,
        )),
        SearchMode::Literal => {
            let escaped = regex::escape(pattern);
            Ok((
                RegexMatcher::new_line_matcher(&escaped)?,
                Regex::new(&escaped)?,
                "literal",
                None,
            ))
        }
        SearchMode::Auto => match Regex::new(pattern) {
            Ok(regex) => Ok((
                RegexMatcher::new_line_matcher(pattern)
                    .with_context(|| format!("invalid regex: {pattern}"))?,
                regex,
                "regex",
                None,
            )),
            Err(err) => {
                let escaped = regex::escape(pattern);
                Ok((
                    RegexMatcher::new_line_matcher(&escaped)?,
                    Regex::new(&escaped)?,
                    "literal",
                    Some(format!(
                        "pattern was not valid regex; searched literally: {err}"
                    )),
                ))
            }
        },
    }
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

pub(super) fn tool_search(ctx: &ToolContext, args: SearchArgs) -> Result<Value> {
    let (matcher, column_regex, mode, warning) = search_matchers(&args.pattern, args.mode)?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let targets = resolve_existing_paths(ctx, &args.path)?;
    let shown = args.limit.max(1);
    let cap = shown.min(MAX_SEARCH_MATCHES);
    let mut matches = Vec::new();
    let mut errors = Vec::new();
    let mut truncated = false;
    for target in &targets {
        for path in walk_files(&ctx.root, target, &exclude)? {
            match search_file_limited(
                &ctx.root,
                &path,
                &matcher,
                &column_regex,
                cap.saturating_sub(matches.len()),
            ) {
                Ok(SearchFileMatches {
                    matches: mut found,
                    truncated: file_truncated,
                }) => {
                    matches.append(&mut found);
                    if file_truncated || matches.len() >= cap {
                        truncated = true;
                        break;
                    }
                }
                Err(err) => errors.push(ToolErrorItem {
                    path: rel_path(&ctx.root, &path),
                    message: err.to_string(),
                }),
            }
        }
        if truncated {
            break;
        }
    }
    Ok(serde_json::to_value(SearchOutput {
        pattern: args.pattern,
        mode,
        warning,
        path: args.path,
        match_count: matches.len(),
        matches,
        truncated,
        exclude: args.exclude.as_ref().map(ExcludeArg::patterns),
        errors: (!errors.is_empty()).then_some(errors),
    })?)
}
pub(super) fn tool_replace(ctx: &ToolContext, args: ReplaceArgs) -> Result<Value> {
    let (regex, replacement, mode) = replace_matcher_and_replacement(&args)?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let target = resolve_existing_path(ctx, &args.path)?;
    let approval_preview = if ctx.policy.approval("replace") == Approval::Ask && ctx.interactive {
        preview_replace_plan(ctx, &args, &regex, &replacement, &target, &exclude).ok()
    } else {
        None
    };
    require_mutation_approval(ctx, "replace", approval_preview.as_deref())?;
    let mut changed_files = Vec::new();
    let mut skipped = Vec::new();
    let mut errors = Vec::new();
    let mut replacement_count = 0usize;
    for path in walk_files(&ctx.root, &target, &exclude)? {
        match replace_file(&path, &regex, &replacement) {
            Ok(ReplaceOutcome::Changed { count, diff }) => {
                changed_files.push(ChangedFileOutput {
                    path: rel_path(&ctx.root, &path),
                    replacements: count,
                    diff,
                });
                replacement_count += count;
            }
            Ok(ReplaceOutcome::Unchanged) => {}
            Ok(ReplaceOutcome::Skipped(reason)) => skipped.push(SkippedFileOutput {
                path: rel_path(&ctx.root, &path),
                reason,
            }),
            Err(err) => errors.push(ToolErrorItem {
                path: rel_path(&ctx.root, &path),
                message: err.to_string(),
            }),
        }
    }
    let shown = args.limit.max(1);
    let changed_file_count = changed_files.len();
    let diff = combined_diff(&changed_files);
    Ok(serde_json::to_value(ReplaceOutput {
        pattern: args.pattern,
        replacement: args.replacement,
        mode,
        path: args.path,
        changed_file_count,
        replacement_count,
        changed_files: changed_files.into_iter().take(shown).collect(),
        diff,
        truncated: changed_file_count > shown,
        exclude: args.exclude.as_ref().map(ExcludeArg::patterns),
        skipped,
        errors,
    })?)
}

pub(super) fn tool_patch(ctx: &ToolContext, mut args: PatchArgs) -> Result<Value> {
    // diffy parses incorrectly when the patch doesn't end with a newline:
    // Insert lines lack trailing \n (silent corruption) and context lines
    // fail to match (apply error).
    if !args.patch.ends_with('\n') {
        args.patch.push('\n');
    }
    let (patch_count, plans) = plan_patch(ctx, &args)?;
    let approval_preview = if ctx.policy.approval("patch") == Approval::Ask && ctx.interactive {
        Some(combined_patch_diff(&plans))
    } else {
        None
    };
    require_mutation_approval(ctx, "patch", approval_preview.as_deref())?;

    for plan in &plans {
        config::write_workspace_file(&plan.path, plan.updated.as_bytes())?;
    }

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

pub(super) fn tool_sloc(ctx: &ToolContext, args: SlocArgs) -> Result<Value> {
    let targets = resolve_existing_paths(ctx, &args.path)?;
    let exclude = args
        .exclude
        .as_ref()
        .map(ExcludeArg::patterns)
        .unwrap_or_default();
    let targets = targets
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let target_refs = targets.iter().map(String::as_str).collect::<Vec<_>>();
    let excluded = exclude.iter().map(String::as_str).collect::<Vec<_>>();

    let config = TokeiConfig {
        hidden: Some(false),
        no_ignore: Some(false),
        no_ignore_parent: Some(false),
        no_ignore_dot: Some(false),
        no_ignore_vcs: Some(false),
        ..TokeiConfig::default()
    };
    let mut languages = TokeiLanguages::new();
    languages.get_statistics(&target_refs, &excluded, &config);
    sort_tokei_reports(&mut languages);

    let mut output = serde_json::to_value(&languages)?;
    if let Value::Object(ref mut map) = output {
        map.insert(
            "Total".to_string(),
            serde_json::to_value(languages.total())?,
        );
    }

    Ok(serde_json::to_value(SlocOutput {
        path: args.path,
        format: "tokei-json",
        output,
        exclude: (!exclude.is_empty()).then_some(exclude),
    })?)
}

fn sort_tokei_reports(languages: &mut TokeiLanguages) {
    for language in languages.values_mut() {
        language.sort_by(TokeiSort::Code);
    }
}

// === Workspace filesystem boundary ===
fn reject_out_of_workspace_path(root: &Path, path: &str, resolved: Option<&Path>) -> Result<()> {
    let raw = Path::new(path);
    if raw.is_absolute() {
        bail!("path outside workspace is not allowed: {path} (absolute path)");
    }
    if raw.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("path outside workspace is not allowed: {path} (parent-directory path)");
    }
    if let Some(resolved) = resolved.filter(|resolved| !within_root(root, resolved)) {
        bail!(
            "path outside workspace is not allowed: {path} -> {}",
            resolved.display()
        );
    }
    Ok(())
}

fn resolve_existing_path(ctx: &ToolContext, path: &str) -> Result<PathBuf> {
    reject_out_of_workspace_path(&ctx.root, path, None)?;
    let joined = ctx.root.join(path);
    let resolved = joined
        .canonicalize()
        .with_context(|| format!("path does not exist: {path}"))?;
    reject_out_of_workspace_path(&ctx.root, path, Some(&resolved))?;
    Ok(resolved)
}

fn resolve_existing_paths(ctx: &ToolContext, path: &str) -> Result<Vec<PathBuf>> {
    match resolve_existing_path(ctx, path) {
        Ok(path) => Ok(vec![path]),
        Err(full_path_error) => {
            let parts = path.split_whitespace().collect::<Vec<_>>();
            if parts.len() <= 1 {
                return Err(full_path_error);
            }
            let mut out = Vec::new();
            for part in parts {
                out.push(resolve_existing_path(ctx, part)?);
            }
            out.sort();
            out.dedup();
            Ok(out)
        }
    }
}

fn build_exclude_set(exclude: Option<&ExcludeArg>) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    if let Some(exclude) = exclude {
        for pattern in exclude.patterns() {
            builder.add(
                Glob::new(&pattern).with_context(|| format!("invalid exclude glob: {pattern}"))?,
            );
        }
    }
    Ok(builder.build()?)
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn display_path(root: &Path, path: &Path) -> String {
    let mut value = rel_path(root, path);
    if path.is_dir() && !value.ends_with('/') {
        value.push('/');
    }
    value
}

fn safe_list_item(root: &Path, path: &Path) -> Option<String> {
    let resolved = path.canonicalize().ok()?;
    if !within_root(root, &resolved) {
        return None;
    }
    Some(display_path(root, path))
}

fn within_root(root: &Path, path: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn walk_files(root: &Path, target: &Path, exclude: &GlobSet) -> Result<Vec<PathBuf>> {
    if target.is_file() {
        return Ok(vec![target.to_path_buf()]);
    }
    let mut out = Vec::new();
    let mut builder = WalkBuilder::new(target);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true);
    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => return Err(anyhow!(err)),
        };
        let path = entry.path();
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            let rel = rel_path(root, path);
            if exclude.is_match(rel.as_str()) {
                continue;
            }
            out.push(path.to_path_buf());
        }
    }
    Ok(out)
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

fn push_match(
    display_path: &str,
    line_number: usize,
    line: &str,
    column: usize,
    out: &mut Vec<SearchHit>,
) {
    out.push(SearchHit {
        path: display_path.to_string(),
        line_number,
        column,
        text: crate::ui::truncate_chars(line.trim_end_matches(['\r', '\n']), 1000),
    });
}

fn search_text_grep(
    display_path: &str,
    text: &str,
    matcher: &RegexMatcher,
    column_regex: &Regex,
    limit: usize,
    out: &mut Vec<SearchHit>,
) -> Result<bool> {
    if limit == 0 {
        return Ok(true);
    }
    let mut truncated = false;
    let mut searcher = SearcherBuilder::new().line_number(true).build();
    let mut sink = UTF8(|line_number, line: &str| {
        if out.len() >= limit {
            truncated = true;
            return Ok(false);
        }
        let column = column_regex.find(line).map(|m| m.start() + 1).unwrap_or(1);
        push_match(display_path, line_number as usize, line, column, out);
        if out.len() >= limit {
            truncated = true;
            return Ok(false);
        }
        Ok(true)
    });
    searcher.search_reader(matcher, text.as_bytes(), &mut sink)?;
    Ok(truncated)
}

pub(super) struct SearchFileMatches {
    pub(super) matches: Vec<SearchHit>,
    pub(super) truncated: bool,
}

pub(super) fn search_file_limited(
    root: &Path,
    path: &Path,
    matcher: &RegexMatcher,
    column_regex: &Regex,
    limit: usize,
) -> Result<SearchFileMatches> {
    let mut matches = Vec::new();
    let mut truncated = false;
    if let Some(item) = read_text_file(root, path)? {
        truncated = search_text_grep(
            &item.display_path,
            &item.text,
            matcher,
            column_regex,
            limit,
            &mut matches,
        )?;
    }
    Ok(SearchFileMatches { matches, truncated })
}

#[cfg(test)]
pub(super) fn search_file(
    root: &Path,
    path: &Path,
    matcher: &RegexMatcher,
    column_regex: &Regex,
) -> Result<Vec<SearchHit>> {
    Ok(search_file_limited(root, path, matcher, column_regex, usize::MAX)?.matches)
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
        if path.is_dir() {
            bail!("cannot patch directory: {patch_path}");
        }
        if ctx.root.join(&patch_path).is_symlink() || path.is_symlink() {
            bail!("cannot patch symlink: {patch_path}");
        }
        if fs::metadata(&path)?.len() > MAX_WORKSPACE_FILE_BYTES {
            bail!("cannot patch file over workspace read cap: {patch_path}");
        }

        let raw = fs::read(&path)?;
        let text = match crate::decode_utf8(raw) {
            Ok(text) => text,
            Err(crate::TextDecodeError::Binary) => bail!("cannot patch binary file: {patch_path}"),
            Err(crate::TextDecodeError::NonUtf8) => bail!("cannot decode utf-8: {patch_path}"),
        };
        let text_patch = file_patch
            .patch()
            .as_text()
            .ok_or_else(|| anyhow!("binary patches are not supported: {patch_path}"))?;
        let updated = match apply(&text, text_patch) {
            Ok(updated) => updated,
            Err(err) => bail!(
                "failed applying patch for {patch_path}: {err}; re-read the file and regenerate the hunk with current context"
            ),
        };
        if updated == text {
            continue;
        }

        let display_path = rel_path(&ctx.root, &path);
        if !seen.insert(display_path.clone()) {
            bail!("patch contains multiple changes for the same file: {display_path}");
        }
        let diff = unified_diff(&display_path, &text, &updated);
        plans.push(PatchPlan {
            path,
            display_path,
            updated,
            diff,
        });
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
        if path.is_dir() {
            bail!("cannot patch directory: {}", file.path);
        }
        if ctx.root.join(&file.path).is_symlink() || path.is_symlink() {
            bail!("cannot patch symlink: {}", file.path);
        }
        if fs::metadata(&path)?.len() > MAX_WORKSPACE_FILE_BYTES {
            bail!("cannot patch file over workspace read cap: {}", file.path);
        }

        let raw = fs::read(&path)?;
        let text = match crate::decode_utf8(raw) {
            Ok(text) => text,
            Err(crate::TextDecodeError::Binary) => bail!("cannot patch binary file: {}", file.path),
            Err(crate::TextDecodeError::NonUtf8) => bail!("cannot decode utf-8: {}", file.path),
        };
        let updated = apply_context_hunks(&text, &file)?;
        if updated == text {
            continue;
        }

        let display_path = rel_path(&ctx.root, &path);
        if !seen.insert(display_path.clone()) {
            bail!("patch contains multiple changes for the same file: {display_path}");
        }
        let diff = unified_diff(&display_path, &text, &updated);
        plans.push(PatchPlan {
            path,
            display_path,
            updated,
            diff,
        });
    }

    Ok((patch_count, plans))
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
        let anchor_start = hunk
            .anchor
            .as_ref()
            .and_then(|anchor| find_anchor_line(&lines, anchor, cursor))
            .unwrap_or(cursor);
        let start = find_line_sequence(&lines, &hunk.old_lines, anchor_start)
            .or_else(|| find_line_sequence(&lines, &hunk.old_lines, cursor))
            .or_else(|| find_line_sequence(&lines, &hunk.old_lines, 0))
            .ok_or_else(|| {
                anyhow!(
                    "failed applying patch for {}: context hunk #{} did not match; re-read the file and regenerate the hunk with current context",
                    file.path,
                    idx + 1
                )
            })?;
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

enum ReplaceOutcome {
    Changed { count: usize, diff: String },
    Unchanged,
    Skipped(&'static str),
}

fn replace_file(path: &Path, regex: &Regex, replacement: &str) -> Result<ReplaceOutcome> {
    if path.is_symlink() {
        return Ok(ReplaceOutcome::Skipped("symlink"));
    }
    if fs::metadata(path)?.len() > MAX_WORKSPACE_FILE_BYTES {
        return Ok(ReplaceOutcome::Skipped("file exceeds workspace read cap"));
    }
    let raw = fs::read(path)?;
    let text = match crate::decode_utf8(raw) {
        Ok(text) => text,
        Err(crate::TextDecodeError::Binary) => {
            return Ok(ReplaceOutcome::Skipped("binary file"));
        }
        Err(crate::TextDecodeError::NonUtf8) => bail!("cannot decode utf-8"),
    };
    let count = regex.find_iter(&text).count();
    if count == 0 {
        return Ok(ReplaceOutcome::Unchanged);
    }
    let updated = regex.replace_all(&text, replacement).into_owned();
    let diff = unified_diff(&path.to_string_lossy(), &text, &updated);
    config::write_workspace_file(path, updated.as_bytes())?;
    Ok(ReplaceOutcome::Changed { count, diff })
}

fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let diff = create_patch(old, new).to_string();
    let diff = diff
        .strip_prefix("--- original\n+++ modified\n")
        .map(|body| format!("--- {path}\n+++ {path}\n{body}"))
        .unwrap_or(diff);
    crate::ui::head_tail(&diff, 12000).0
}

fn combined_diff(files: &[ChangedFileOutput]) -> String {
    let text = files
        .iter()
        .map(|item| item.diff.as_str())
        .filter(|diff| !diff.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    crate::ui::head_tail(&text, 12000).0
}

fn preview_replace_plan(
    ctx: &ToolContext,
    args: &ReplaceArgs,
    regex: &Regex,
    replacement: &str,
    target: &Path,
    exclude: &GlobSet,
) -> Result<String> {
    let mut changed = Vec::new();
    for path in walk_files(&ctx.root, target, exclude)? {
        if path.is_symlink() {
            continue;
        }
        if fs::metadata(&path)
            .ok()
            .is_some_and(|meta| meta.len() > MAX_WORKSPACE_FILE_BYTES)
        {
            continue;
        }
        let raw = match fs::read(&path) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let Ok(text) = crate::decode_utf8(raw) else {
            continue;
        };
        if !regex.is_match(&text) {
            continue;
        }
        let updated = regex.replace_all(&text, replacement).into_owned();
        let display_path = rel_path(&ctx.root, &path);
        changed.push(ChangedFileOutput {
            replacements: regex.find_iter(&text).count(),
            diff: unified_diff(&display_path, &text, &updated),
            path: display_path,
        });
        if changed.len() >= args.limit.clamp(1, PREVIEW_ITEMS) {
            break;
        }
    }
    Ok(combined_diff(&changed))
}
