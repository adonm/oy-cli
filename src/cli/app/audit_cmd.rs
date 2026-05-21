//! `oy audit` subcommand: argument parsing and delegation
//! to the deterministic no-tools audit pipeline.

use anyhow::Result;
use clap::ValueEnum;
use std::path::PathBuf;

use crate::audit;
use crate::config;
use crate::model;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(super) enum AuditFormat {
    Markdown,
    Sarif,
}

impl From<AuditFormat> for audit::AuditOutputFormat {
    fn from(format: AuditFormat) -> Self {
        match format {
            AuditFormat::Markdown => Self::Markdown,
            AuditFormat::Sarif => Self::Sarif,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct AuditArgs {
    pub(super) focus: Vec<String>,
    pub(super) out: PathBuf,
    pub(super) max_chunks: usize,
    pub(super) format: audit::AuditOutputFormat,
}

pub(super) async fn audit_command(args: AuditArgs) -> Result<i32> {
    let started = std::time::Instant::now();
    let focus = args.focus.join(" ");
    let root = config::oy_root()?;
    let model = model::resolve_model(None)?;
    if !crate::ui::is_quiet() {
        crate::ui::section("audit");
        crate::ui::kv("workspace", root.display());
        crate::ui::kv("model", &model);
        crate::ui::kv("mode", "no-tools");
        crate::ui::kv("format", args.format.name());
        crate::ui::kv("out", args.out.display());
        crate::ui::kv("max chunks", args.max_chunks);
        if !focus.trim().is_empty() {
            crate::ui::kv("focus", crate::ui::compact_preview(&focus, 100));
        }
    }
    let result = audit::run(audit::AuditOptions {
        root,
        model,
        focus,
        out: args.out,
        max_chunks: args.max_chunks,
        format: args.format,
    })
    .await?;
    if crate::ui::is_json() {
        let payload = serde_json::json!({
            "output": result.output_path,
            "files": result.file_count,
            "chunks": result.chunk_count,
            "format": args.format.name(),
            "elapsed_ms": started.elapsed().as_millis(),
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
    } else {
        crate::ui::success(format_args!(
            "wrote {} ({} files, {} chunks, {})",
            result.output_path.display(),
            result.file_count,
            result.chunk_count,
            crate::ui::format_duration(started.elapsed())
        ));
    }
    Ok(0)
}
