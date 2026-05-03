use crate::tools::{Approval, FileAccess, ToolPolicy};
use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize};

const PLAN_SYSTEM: &str = r#"PLAN mode. Stay read-only. Use only list, read, search, sloc, todo for in-memory planning, ask when interactive, and webfetch when available. Keep files unchanged, skip shell commands, and describe changes as proposed rather than applied."#;
const ACCEPT_EDITS_SYSTEM: &str = r#"ACCEPT-EDITS mode. File edits may run without asking. Keep edits small and targeted, inspect before changing, and reach for `bash` only when genuinely necessary."#;
const AUTO_APPROVE_SYSTEM: &str = r#"AUTO-APPROVE mode. Tools may run without asking. Still avoid destructive commands, broad rewrites, credential exposure, persistence changes, and network/file/process expansion unless clearly needed. Treat shell and replacement tools as strict side effects: inspect first, then run the smallest command/edit."#;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum SafetyMode {
    #[default]
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "accept-edits")]
    AutoEdits,
    #[serde(rename = "auto-approve")]
    AutoAll,
}

impl SafetyMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "" | "default" | "ask" => Ok(Self::Default),
            "plan" | "read-only" | "readonly" | "read" => Ok(Self::Plan),
            "accept-edits" | "edit" | "edits" | "auto-edits" | "write" => Ok(Self::AutoEdits),
            "auto-approve" | "auto" | "yolo" => Ok(Self::AutoAll),
            other => bail!("Unknown mode `{other}`. Available: plan, ask, edit, auto"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
            Self::AutoEdits => "accept-edits",
            Self::AutoAll => "auto-approve",
        }
    }

    pub(super) fn system_prompt_suffix(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Plan => PLAN_SYSTEM,
            Self::AutoEdits => ACCEPT_EDITS_SYSTEM,
            Self::AutoAll => AUTO_APPROVE_SYSTEM,
        }
    }

    pub fn policy(self) -> ToolPolicy {
        match self {
            Self::Plan => ToolPolicy::read_only(),
            Self::Default => ToolPolicy::with_write(Approval::Ask, Approval::Ask),
            Self::AutoEdits => ToolPolicy::with_write(Approval::Auto, Approval::Ask),
            Self::AutoAll => ToolPolicy::with_write(Approval::Auto, Approval::Auto),
        }
    }
}

impl std::str::FromStr for SafetyMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

impl<'de> Deserialize<'de> for SafetyMode {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

pub fn tool_policy(mode: SafetyMode) -> ToolPolicy {
    mode.policy()
}

pub fn policy_risk_label(policy: &ToolPolicy) -> &'static str {
    match (policy.files, policy.shell) {
        (FileAccess::ReadOnly, _) => "read-only: no file edits or shell",
        (_, Approval::Auto) => "high: auto shell",
        (FileAccess::Write(Approval::Auto), _) => "medium: auto edits",
        _ => "normal: asks before edits/shell",
    }
}
