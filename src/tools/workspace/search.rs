use anyhow::{Context, Result};
use fff_search::{
    AiGrepConfig, GrepMode, GrepSearchOptions, QueryParser, has_regex_metacharacters,
};
use globset::GlobSet;
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use super::super::ToolContext;
use super::super::args::{ExcludeArg, SearchArgs, SearchMode};
use super::MAX_WORKSPACE_FILE_BYTES;
use super::discovery::{build_exclude_set, fff_picker};
use super::output::{SearchHit, SearchOutput, ToolErrorItem};
use super::paths::{rel_path, resolve_existing_paths};

const MAX_SEARCH_MATCHES: usize = 10_000;

fn search_mode(
    pattern: &str,
    mode: SearchMode,
) -> Result<(GrepMode, &'static str, Option<String>)> {
    match mode {
        SearchMode::Regex => {
            Regex::new(pattern).with_context(|| format!("invalid regex: {pattern}"))?;
            Ok((GrepMode::Regex, "regex", None))
        }
        SearchMode::Literal => Ok((GrepMode::PlainText, "literal", None)),
        SearchMode::Auto if !has_regex_metacharacters(pattern) => {
            Ok((GrepMode::PlainText, "literal", None))
        }
        SearchMode::Auto => match Regex::new(pattern) {
            Ok(_) => Ok((GrepMode::Regex, "regex", None)),
            Err(err) => Ok((
                GrepMode::PlainText,
                "literal",
                Some(format!(
                    "pattern looked like regex but was invalid; searched literally: {err}"
                )),
            )),
        },
    }
}

pub(crate) fn tool_search(ctx: &ToolContext, args: SearchArgs) -> Result<Value> {
    let (grep_mode, mode, warning) = search_mode(&args.pattern, args.mode)?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let targets = resolve_existing_paths(ctx, &args.path)?;
    let shown = args.limit.max(1);
    let cap = shown.min(MAX_SEARCH_MATCHES);
    let mut matches = Vec::new();
    let mut errors = Vec::new();
    let mut truncated = false;
    let target_count = targets.len();
    for (target_idx, target) in targets.iter().enumerate() {
        match fff_search_target(
            ctx.root(),
            target,
            &args.pattern,
            grep_mode,
            &exclude,
            cap.saturating_sub(matches.len()),
        ) {
            Ok(SearchTargetMatches {
                matches: mut found,
                truncated: target_truncated,
            }) => {
                matches.append(&mut found);
                if target_truncated || (matches.len() >= cap && target_idx + 1 < target_count) {
                    truncated = true;
                    break;
                }
            }
            Err(err) => {
                errors.push(ToolErrorItem {
                    path: rel_path(ctx.root(), target),
                    message: err.to_string(),
                });
            }
        }
    }
    let read_path = best_read_path(&matches);
    let file_count = count_match_files(&matches);
    Ok(serde_json::to_value(SearchOutput {
        pattern: args.pattern,
        mode,
        warning,
        read_path,
        file_count,
        path: args.path,
        match_count: matches.len(),
        matches,
        truncated,
        exclude: args.exclude.as_ref().map(ExcludeArg::patterns),
        errors: (!errors.is_empty()).then_some(errors),
    })?)
}
struct SearchTargetMatches {
    matches: Vec<SearchHit>,
    truncated: bool,
}

fn grep_options(mode: GrepMode, limit: usize) -> GrepSearchOptions {
    GrepSearchOptions {
        max_file_size: MAX_WORKSPACE_FILE_BYTES,
        max_matches_per_file: 0,
        page_limit: limit,
        mode,
        ..GrepSearchOptions::default()
    }
}

fn fff_search_target(
    root: &Path,
    target: &Path,
    pattern: &str,
    mode: GrepMode,
    exclude: &GlobSet,
    limit: usize,
) -> Result<SearchTargetMatches> {
    if limit == 0 {
        return Ok(SearchTargetMatches {
            matches: Vec::new(),
            truncated: true,
        });
    }

    if target.is_file() {
        return search_exact_file(root, target, pattern, mode, exclude, limit);
    }

    let base = target;
    let picker = fff_picker(base)?;
    let parser = QueryParser::new(AiGrepConfig);
    let query = parser.parse(pattern);
    let result = picker.grep(&query, &grep_options(mode, limit));

    let mut matches = Vec::new();
    let mut truncated = result.next_file_offset > 0;
    for item in result.matches {
        let file = result.files[item.file_index];
        let display = display_path_from_base(root, base, file.relative_path(&picker).as_str());
        if exclude.is_match(display.as_str()) {
            continue;
        }
        if matches.len() >= limit {
            truncated = true;
            break;
        }
        matches.push(SearchHit {
            path: display,
            line_number: item.line_number as usize,
            column: item.col + 1,
            text: crate::ui::truncate_chars(item.line_content.trim_end_matches(['\r', '\n']), 1000),
        });
    }

    Ok(SearchTargetMatches { matches, truncated })
}

fn search_exact_file(
    root: &Path,
    target: &Path,
    pattern: &str,
    mode: GrepMode,
    exclude: &GlobSet,
    limit: usize,
) -> Result<SearchTargetMatches> {
    let display = rel_path(root, target);
    if exclude.is_match(display.as_str()) || fs::metadata(target)?.len() > MAX_WORKSPACE_FILE_BYTES
    {
        return Ok(SearchTargetMatches {
            matches: Vec::new(),
            truncated: false,
        });
    }

    let raw = fs::read(target)?;
    let text = match crate::decode_utf8(raw) {
        Ok(text) => text,
        Err(crate::TextDecodeError::Binary) => {
            return Ok(SearchTargetMatches {
                matches: Vec::new(),
                truncated: false,
            });
        }
        Err(crate::TextDecodeError::NonUtf8) => anyhow::bail!("cannot decode utf-8"),
    };
    let regex = match mode {
        GrepMode::Regex => {
            Regex::new(pattern).with_context(|| format!("invalid regex: {pattern}"))?
        }
        GrepMode::PlainText => Regex::new(&regex::escape(pattern))?,
        GrepMode::Fuzzy => anyhow::bail!("fuzzy grep mode is not supported for workspace search"),
    };

    let mut matches = Vec::new();
    let mut truncated = false;
    for (line_idx, line) in text.lines().enumerate() {
        for item in regex.find_iter(line) {
            if matches.len() >= limit {
                truncated = true;
                return Ok(SearchTargetMatches { matches, truncated });
            }
            matches.push(SearchHit {
                path: display.clone(),
                line_number: line_idx + 1,
                column: item.start() + 1,
                text: crate::ui::truncate_chars(line.trim_end_matches(['\r', '\n']), 1000),
            });
        }
    }

    Ok(SearchTargetMatches { matches, truncated })
}

fn display_path_from_base(root: &Path, base: &Path, rel_to_base: &str) -> String {
    let rel_to_base = rel_to_base.replace('\\', "/");
    let base_rel = rel_path(root, base);
    if base_rel.is_empty() {
        rel_to_base
    } else if rel_to_base.is_empty() {
        base_rel
    } else {
        format!("{}/{rel_to_base}", base_rel.trim_end_matches('/'))
    }
}

fn best_read_path(matches: &[SearchHit]) -> Option<String> {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    let mut first = std::collections::BTreeMap::<&str, usize>::new();
    for (idx, hit) in matches.iter().enumerate() {
        *counts.entry(hit.path.as_str()).or_insert(0) += 1;
        first.entry(hit.path.as_str()).or_insert(idx);
    }
    counts
        .into_iter()
        .max_by(|(path_a, count_a), (path_b, count_b)| {
            let first_a = first.get(path_a).copied().unwrap_or(usize::MAX);
            let first_b = first.get(path_b).copied().unwrap_or(usize::MAX);
            count_a
                .cmp(count_b)
                .then_with(|| first_b.cmp(&first_a))
                .then_with(|| path_b.cmp(path_a))
        })
        .map(|(path, _)| path.to_string())
}

fn count_match_files(matches: &[SearchHit]) -> usize {
    matches
        .iter()
        .map(|hit| hit.path.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}
