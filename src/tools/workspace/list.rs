use anyhow::Result;
use glob::glob;
use serde_json::Value;
use std::path::Path;

use super::super::ToolContext;
use super::super::args::{ExcludeArg, ListArgs};
use super::discovery::{
    build_exclude_set, fff_fuzzy_workspace_paths, glob_has_meta, list_dir_children,
};
use super::output::ListOutput;
use super::paths::{
    display_path, reject_out_of_workspace_path, resolve_existing_path, safe_list_item,
};

pub(crate) fn tool_list(ctx: &ToolContext, args: ListArgs) -> Result<Value> {
    reject_out_of_workspace_path(ctx.root(), &args.path, None)?;
    let exclude = build_exclude_set(args.exclude.as_ref())?;
    let shown_limit = args.limit.max(1);
    let (items, count) = if args.path == "." || args.path == "./" || args.path == "*" {
        let items = list_dir_children(ctx.root(), ctx.root(), &exclude)?;
        let count = items.len();
        (items, count)
    } else if !glob_has_meta(&args.path) {
        match resolve_existing_path(ctx, &args.path) {
            Ok(path) if path.is_dir() => {
                let items = list_dir_children(ctx.root(), &path, &exclude)?;
                let count = items.len();
                (items, count)
            }
            Ok(path) => (vec![display_path(ctx.root(), &path)], 1),
            Err(_) => fff_fuzzy_workspace_paths(ctx.root(), &args.path, &exclude)?,
        }
    } else {
        let pattern = if Path::new(&args.path).is_absolute() {
            args.path.clone()
        } else {
            ctx.root().join(&args.path).to_string_lossy().to_string()
        };
        let mut out = glob(&pattern)?
            .filter_map(|entry| entry.ok())
            .filter_map(|path| safe_list_item(ctx.root(), &path))
            .filter(|item| !exclude.is_match(item.as_str()))
            .collect::<Vec<_>>();
        out.sort();
        out.dedup();
        let count = out.len();
        (out, count)
    };
    Ok(serde_json::to_value(ListOutput {
        path: args.path,
        items: items.iter().take(shown_limit).cloned().collect(),
        count,
        truncated: count > shown_limit,
        exclude: args.exclude.as_ref().map(ExcludeArg::patterns),
    })?)
}
