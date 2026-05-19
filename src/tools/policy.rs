//! Tool approval policy and mutation approval boundary.
//!
//! Safety modes reduce to this matrix: read tools are exposed elsewhere, while
//! file writes, todo persistence, and shell commands are allowed, asked, or denied here.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::ToolContext;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Approval {
    #[default]
    Deny,
    Ask,
    Auto,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileAccess {
    #[default]
    ReadOnly,
    Write(Approval),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkAccess {
    Disabled,
    #[default]
    Enabled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPolicy {
    pub files: FileAccess,
    pub shell: Approval,
    pub network: NetworkAccess,
}

impl ToolPolicy {
    pub fn read_only() -> Self {
        Self {
            files: FileAccess::ReadOnly,
            shell: Approval::Deny,
            network: NetworkAccess::Enabled,
        }
    }

    pub fn with_write(files_write: Approval, shell: Approval) -> Self {
        Self {
            files: FileAccess::Write(files_write),
            shell,
            network: NetworkAccess::Enabled,
        }
    }

    pub fn files_write(self) -> Approval {
        match self.files {
            FileAccess::ReadOnly => Approval::Deny,
            FileAccess::Write(approval) => approval,
        }
    }

    pub fn approval(self, tool: &str) -> Approval {
        match tool {
            "todo" => Approval::Auto,
            "replace" | "patch" | "todo_persist" => self.files_write(),
            "bash" => self.shell,
            _ => Approval::Deny,
        }
    }
}

pub(crate) fn require_mutation_approval(
    ctx: &ToolContext,
    tool: &str,
    preview: Option<&str>,
) -> Result<()> {
    match ctx.policy.approval(tool) {
        Approval::Auto => Ok(()),
        Approval::Deny => bail!("tool denied by policy: {tool}"),
        Approval::Ask if !ctx.interactive => bail!(
            "tool denied by policy: {tool} requires interactive approval or an auto-approve mode"
        ),
        Approval::Ask => approve_tool(tool, preview),
    }
}

fn approval_display_name(tool: &str) -> &str {
    match tool {
        "todo_persist" => "todo",
        other => other,
    }
}

fn approve_tool(tool: &str, preview: Option<&str>) -> Result<()> {
    let display_tool = approval_display_name(tool);
    if let Some(preview) = preview.filter(|s| !s.trim().is_empty()) {
        crate::ui::err_line(crate::ui::diff(preview).trim_end());
    }
    crate::ui::section("Approval required");
    crate::ui::kv("tool", display_tool);
    crate::ui::kv("default", "deny");
    if tool == "bash" {
        crate::ui::warn(
            "shell commands run with your user permissions; credential-like env vars are removed",
        );
    }
    let choices = ["no".to_string(), "yes".to_string()];
    if crate::chat::ask(&format!("Approve {display_tool}?"), Some(&choices))? == "yes" {
        Ok(())
    } else {
        bail!("tool denied by user")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SafetyMode;

    #[test]
    fn safety_modes_map_to_expected_tool_policies() {
        let cases = [
            (SafetyMode::Plan, ToolPolicy::read_only()),
            (
                SafetyMode::Default,
                ToolPolicy::with_write(Approval::Ask, Approval::Ask),
            ),
            (
                SafetyMode::AutoEdits,
                ToolPolicy::with_write(Approval::Auto, Approval::Ask),
            ),
            (
                SafetyMode::AutoAll,
                ToolPolicy::with_write(Approval::Auto, Approval::Auto),
            ),
        ];

        for (mode, policy) in cases {
            assert_eq!(mode.policy(), policy, "{}", mode.name());
        }
    }

    #[test]
    fn approval_matrix_is_explicit_for_mutating_tools() {
        let cases = [
            (ToolPolicy::read_only(), Approval::Deny, Approval::Deny),
            (
                ToolPolicy::with_write(Approval::Ask, Approval::Ask),
                Approval::Ask,
                Approval::Ask,
            ),
            (
                ToolPolicy::with_write(Approval::Auto, Approval::Ask),
                Approval::Auto,
                Approval::Ask,
            ),
            (
                ToolPolicy::with_write(Approval::Auto, Approval::Auto),
                Approval::Auto,
                Approval::Auto,
            ),
        ];

        for (policy, file_approval, shell_approval) in cases {
            assert_eq!(policy.approval("replace"), file_approval);
            assert_eq!(policy.approval("patch"), file_approval);
            assert_eq!(policy.approval("todo_persist"), file_approval);
            assert_eq!(policy.approval("bash"), shell_approval);
            assert_eq!(policy.approval("todo"), Approval::Auto);
            assert_eq!(policy.approval("unknown"), Approval::Deny);
        }
    }
}
