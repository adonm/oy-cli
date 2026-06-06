//! Safety modes: [`SafetyMode`] enum and conversion to [`ToolPolicy`]
//! with ask/edit/plan/auto variants.

use crate::tools::{Approval, ToolPolicy};
use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize};

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
