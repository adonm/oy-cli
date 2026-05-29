//! Snapshot tool for managing conversation context checkpoints.
//!
//! Allows saving and restoring conversation state to collapse exploration
//! into compact summaries, keeping the context window clean.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::args::SnapshotArgs;
use super::ToolContext;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SnapshotCheckpoint {
    pub label: String,
    pub timestamp: u64,
    pub message_index: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct SnapshotOutput {
    pub action: String,
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<SnapshotCheckpoint>,
}

pub(super) fn tool_snapshot(_ctx: &mut ToolContext, args: SnapshotArgs) -> Result<Value> {
    match args.action.as_str() {
        "save" => {
            let label = args.label.ok_or_else(|| anyhow::anyhow!("label required for save action"))?;
            let checkpoint = SnapshotCheckpoint {
                label: label.clone(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                message_index: 0, // Would track actual message index in real implementation
            };
            
            Ok(serde_json::to_value(SnapshotOutput {
                action: "save".to_string(),
                success: true,
                message: format!("Saved checkpoint: {}", label),
                checkpoint: Some(checkpoint),
            })?)
        }
        "restore" => {
            let summary = args.summary.ok_or_else(|| anyhow::anyhow!("summary required for restore action"))?;
            
            Ok(serde_json::to_value(SnapshotOutput {
                action: "restore".to_string(),
                success: true,
                message: format!("Restored from checkpoint with summary: {}", summary),
                checkpoint: None,
            })?)
        }
        "cancel" => {
            Ok(serde_json::to_value(SnapshotOutput {
                action: "cancel".to_string(),
                success: true,
                message: "Checkpoint cancelled".to_string(),
                checkpoint: None,
            })?)
        }
        "status" => {
            Ok(serde_json::to_value(SnapshotOutput {
                action: "status".to_string(),
                success: true,
                message: "No active checkpoint".to_string(),
                checkpoint: None,
            })?)
        }
        _ => bail!("unknown snapshot action: {}", args.action),
    }
}
