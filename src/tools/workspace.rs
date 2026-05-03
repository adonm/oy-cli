use anyhow::{Context, Result, anyhow, bail};
use glob::glob;
use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_regex::RegexMatcher;
use grep_searcher::SearcherBuilder;
use grep_searcher::sinks::UTF8;
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::{Value, json};
use similar::{ChangeTag, TextDiff};
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};
use tokei::{Config as TokeiConfig, Languages as TokeiLanguages, Sort as TokeiSort};

use crate::config;

use super::args::{
    ExcludeArg, ListArgs, ReadArgs, ReplaceArgs, ReplaceMode, SearchArgs, SearchMode, SlocArgs,
};
use super::{Approval, PREVIEW_ITEMS, ToolContext, require_mutation_approval};

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
            .filter(|path| !exclude.is_match(rel_path(&ctx.root, path).as_str()))
            .map(|path| display_path(&ctx.root, &path))
            .collect::<Vec<_>>();
        out.sort();
        out.dedup();
        out
    };
    Ok(json!({
        "path": args.path,
        "items": items.iter().take(shown_limit).cloned().collect::<Vec<_>>(),
        "count": items.len(),
        "truncated": items.len() > shown_limit,
        "exclude": args.exclude.as_ref().map(ExcludeArg::patterns)
    }))
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
    Ok(json!({
        "path": display_path,
        "offset": args.offset,
        "limit": args.limit,
        "text": shown.join("\n"),
        "line_count": line_count,
        "truncated": truncated
    }))
}

fn search_matchers(
    pattern: &str,
    mode: SearchMode,
) -> Result<(RegexMatcher, Regex, &'static str, Value)> {
    match mode {
        SearchMode::Regex => Ok((
            RegexMatcher::new_line_matcher(pattern)
                .with_context(|| format!("invalid regex: {pattern}"))?,
            Regex::new(pattern).with_context(|| format!("invalid regex: {pattern}"))?,
            "regex",
            Value::Null,
        )),
        SearchMode::Literal => {
            let escaped = regex::escape(pattern);
            Ok((
                RegexMatcher::new_line_matcher(&escaped)?,
                Regex::new(&escaped)?,
                "literal",
                Value::Null,
            ))
        }
        SearchMode::Auto => match Regex::new(pattern) {
            Ok(regex) => Ok((
                RegexMatcher::new_line_matcher(pattern)
                    .with_context(|| format!("invalid regex: {pattern}"))?,
                regex,
                "regex",
                Value::Null,
            )),
            Err(err) => {
                let escaped = regex::escape(pattern);
                Ok((
                    RegexMatcher::new_line_matcher(&escaped)?,
                    Regex::new(&escaped)?,
                    "literal",
                    json!(format!(
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
    let mut matches = Vec::new();
    let mut errors = Vec::new();
    for target in &targets {
        for path in walk_files(&ctx.root, target, &exclude)? {
            match search_file(&ctx.root, &path, &matcher, &column_regex) {
                Ok(mut found) => matches.append(&mut found),
                Err(err) => errors
                    .push(json!({"path": rel_path(&ctx.root, &path), "message": err.to_string()})),
            }
        }
    }
    let shown = args.limit.max(1);
    Ok(json!({
        "pattern": args.pattern,
        "mode": mode,
        "warning": warning,
        "path": args.path,
        "match_count": matches.len(),
        "matches": matches.iter().take(shown).cloned().collect::<Vec<_>>(),
        "truncated": matches.len() > shown,
        "exclude": args.exclude.as_ref().map(ExcludeArg::patterns),
        "errors": if errors.is_empty() { Value::Null } else { Value::Array(errors) }
    }))
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
                changed_files.push(json!({
                    "path": rel_path(&ctx.root, &path),
                    "replacements": count,
                    "diff": diff
                }));
                replacement_count += count;
            }
            Ok(ReplaceOutcome::Unchanged) => {}
            Ok(ReplaceOutcome::Skipped(reason)) => {
                skipped.push(json!({"path": rel_path(&ctx.root, &path), "reason": reason}))
            }
            Err(err) => {
                errors.push(json!({"path": rel_path(&ctx.root, &path), "message": err.to_string()}))
            }
        }
    }
    let shown = args.limit.max(1);
    Ok(json!({
        "pattern": args.pattern,
        "replacement": args.replacement,
        "mode": mode,
        "path": args.path,
        "changed_file_count": changed_files.len(),
        "replacement_count": replacement_count,
        "changed_files": changed_files.iter().take(shown).cloned().collect::<Vec<_>>(),
        "diff": combined_diff(&changed_files),
        "truncated": changed_files.len() > shown,
        "exclude": args.exclude.as_ref().map(ExcludeArg::patterns),
        "skipped": skipped,
        "errors": errors
    }))
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

    Ok(json!({
        "path": args.path,
        "format": "tokei-json",
        "output": output,
        "exclude": if exclude.is_empty() { Value::Null } else { serde_json::to_value(exclude)? }
    }))
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
    out: &mut Vec<Value>,
) {
    out.push(json!({
        "path": display_path,
        "line_number": line_number,
        "column": column,
        "text": crate::ui::truncate_chars(line.trim_end_matches(['\r', '\n']), 1000)
    }));
}

fn search_text_grep(
    display_path: &str,
    text: &str,
    matcher: &RegexMatcher,
    column_regex: &Regex,
    out: &mut Vec<Value>,
) -> Result<()> {
    let mut searcher = SearcherBuilder::new().line_number(true).build();
    let mut sink = UTF8(|line_number, line: &str| {
        let column = column_regex.find(line).map(|m| m.start() + 1).unwrap_or(1);
        push_match(display_path, line_number as usize, line, column, out);
        Ok(true)
    });
    searcher.search_reader(matcher, text.as_bytes(), &mut sink)?;
    Ok(())
}

pub(super) fn search_file(
    root: &Path,
    path: &Path,
    matcher: &RegexMatcher,
    column_regex: &Regex,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    if let Some(item) = read_text_file(root, path)? {
        search_text_grep(
            &item.display_path,
            &item.text,
            matcher,
            column_regex,
            &mut out,
        )?;
    }
    Ok(out)
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
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    let _ = writeln!(out, "--- {path}");
    let _ = writeln!(out, "+++ {path}");
    for group in diff.grouped_ops(3) {
        for op in group {
            for change in diff.iter_changes(&op) {
                let sign = match change.tag() {
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                    ChangeTag::Equal => ' ',
                };
                let _ = write!(out, "{sign}{change}");
            }
        }
    }
    crate::ui::head_tail(&out, 12000).0
}

fn combined_diff(files: &[Value]) -> String {
    let text = files
        .iter()
        .filter_map(|item| item.get("diff").and_then(Value::as_str))
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
        changed.push(json!({
            "path": rel_path(&ctx.root, &path),
            "replacements": regex.find_iter(&text).count(),
            "diff": unified_diff(&rel_path(&ctx.root, &path), &text, &updated)
        }));
        if changed.len() >= args.limit.clamp(1, PREVIEW_ITEMS) {
            break;
        }
    }
    Ok(combined_diff(&changed))
}
