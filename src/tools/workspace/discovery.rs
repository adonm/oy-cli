use anyhow::{Context, Result};
use fff_search::{
    FFFMode, FilePicker, FilePickerOptions, FuzzySearchOptions, PaginationArgs, QueryParser,
};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::fs;
use std::path::{Path, PathBuf};

use super::super::args::ExcludeArg;
use super::paths::{display_path, rel_path};

const MAX_SEARCH_MATCHES: usize = 10_000;

pub(super) fn glob_has_meta(pattern: &str) -> bool {
    pattern.chars().any(|c| matches!(c, '*' | '?' | '[' | '{'))
}

pub(super) fn list_dir_children(root: &Path, dir: &Path, exclude: &GlobSet) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let rel = rel_path(root, &path);
        if exclude.is_match(rel.as_str()) {
            continue;
        }
        out.push(display_path(root, &path));
    }
    out.sort();
    Ok(out)
}

pub(super) fn fff_picker(base: &Path) -> Result<FilePicker> {
    let mut picker = FilePicker::new(FilePickerOptions {
        base_path: base.to_string_lossy().to_string(),
        mode: FFFMode::Ai,
        watch: false,
        ..FilePickerOptions::default()
    })?;
    picker.collect_files()?;
    Ok(picker)
}

pub(super) fn fff_fuzzy_workspace_paths(
    root: &Path,
    query: &str,
    exclude: &GlobSet,
) -> Result<(Vec<String>, usize)> {
    fff_fuzzy_workspace_paths_with_limit(root, query, exclude, MAX_SEARCH_MATCHES)
}

pub(super) fn fff_fuzzy_workspace_paths_with_limit(
    root: &Path,
    query: &str,
    exclude: &GlobSet,
    limit: usize,
) -> Result<(Vec<String>, usize)> {
    let picker = fff_picker(root)?;
    let parser = QueryParser::default();
    let query = parser.parse(query);
    let results = picker.fuzzy_search(
        &query,
        None,
        FuzzySearchOptions {
            project_path: Some(root),
            pagination: PaginationArgs { offset: 0, limit },
            ..FuzzySearchOptions::default()
        },
    );

    let mut items = Vec::new();
    for item in results.items {
        let path = item.relative_path(&picker).replace('\\', "/");
        if !exclude.is_match(path.as_str()) {
            items.push(path);
        }
    }
    let count = items.len();
    Ok((items, count))
}

pub(super) fn fff_indexed_files(
    root: &Path,
    target: &Path,
    exclude: &GlobSet,
) -> Result<Vec<PathBuf>> {
    if target.is_file() {
        let rel = rel_path(root, target);
        return Ok((!exclude.is_match(rel.as_str()))
            .then(|| target.to_path_buf())
            .into_iter()
            .collect());
    }

    let picker = fff_picker(target)?;
    let mut files = Vec::new();
    for item in picker.get_files() {
        let rel_to_target = item.relative_path(&picker).replace('\\', "/");
        let path = target.join(&rel_to_target);
        let rel_to_root = rel_path(root, &path);
        if !exclude.is_match(rel_to_root.as_str()) {
            files.push(path);
        }
    }
    Ok(files)
}

pub(super) fn build_exclude_set(exclude: Option<&ExcludeArg>) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    if let Some(exclude) = exclude {
        for pattern in exclude.patterns() {
            builder.add(
                Glob::new(&pattern).with_context(|| format!("invalid exclude glob: {pattern}"))?,
            );
            if pattern.ends_with('/') {
                let children = format!("{pattern}**");
                builder.add(
                    Glob::new(&children)
                        .with_context(|| format!("invalid exclude glob: {children}"))?,
                );
            }
        }
    }
    Ok(builder.build()?)
}
