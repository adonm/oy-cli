//! `oy review` subcommand: strict no-tools maintainability review for a
//! git target diff or the whole workspace.

use anyhow::Result;
use clap::Args;
use std::path::PathBuf;

use crate::config;
use crate::model;
use crate::review;

#[derive(Debug, Args, Clone)]
pub(super) struct ReviewArgs {
    #[arg(
        long,
        value_name = "PATH",
        help = "Write review findings to a workspace file (default: REVIEW.md)"
    )]
    pub(super) out: Option<PathBuf>,
    #[arg(
        long,
        value_name = "N",
        default_value_t = review::DEFAULT_MAX_REVIEW_CHUNKS,
        help = "Maximum review chunks before failing closed"
    )]
    pub(super) max_chunks: usize,
    #[arg(
        long,
        value_name = "TEXT",
        help = "Optional review focus text; can be repeated"
    )]
    pub(super) focus: Vec<String>,
    #[arg(
        value_name = "TARGET",
        help = "Optional branch/commit/ref to diff current workspace against; omitted reviews the whole workspace"
    )]
    pub(super) target: Option<String>,
}

pub(super) fn default_output_path() -> PathBuf {
    review::default_output_path()
}

pub(super) async fn review_command(args: ReviewArgs) -> Result<i32> {
    let started = std::time::Instant::now();
    let root = config::oy_root()?;
    let model = model::resolve_model(None)?;
    let focus = args.focus.join(" ");
    let out = args.out.unwrap_or_else(review::default_output_path);
    if !crate::ui::is_quiet() {
        crate::ui::section("review");
        crate::ui::kv("workspace", root.display());
        crate::ui::kv("model", &model);
        crate::ui::kv("mode", "no-tools");
        crate::ui::kv("out", out.display());
        crate::ui::kv("max chunks", args.max_chunks);
        crate::ui::kv(
            "target",
            args.target.as_deref().unwrap_or("whole workspace"),
        );
        if !focus.trim().is_empty() {
            crate::ui::kv("focus", crate::ui::compact_preview(&focus, 100));
        }
    }
    let result = review::run(review::ReviewOptions {
        root,
        model,
        target: args.target,
        focus,
        out,
        max_chunks: args.max_chunks,
    })
    .await?;
    if crate::ui::is_json() {
        let payload = serde_json::json!({
            "output": result.output_path,
            "items": result.item_count,
            "chunks": result.chunk_count,
            "source": result.source,
            "elapsed_ms": started.elapsed().as_millis(),
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
    } else {
        crate::ui::success(format_args!(
            "wrote {} ({} items, {} chunks, {})",
            result.output_path.display(),
            result.item_count,
            result.chunk_count,
            crate::ui::format_duration(started.elapsed())
        ));
    }
    Ok(0)
}
