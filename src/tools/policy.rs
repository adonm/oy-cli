//! Minimal safety-mode policy types retained for wrapper compatibility.

use serde::{Deserialize, Serialize};

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
pub struct ToolPolicy {
    pub files: FileAccess,
    pub shell: Approval,
}

impl ToolPolicy {
    pub fn read_only() -> Self {
        Self {
            files: FileAccess::ReadOnly,
            shell: Approval::Deny,
        }
    }

    pub fn with_write(files_write: Approval, shell: Approval) -> Self {
        Self {
            files: FileAccess::Write(files_write),
            shell,
        }
    }

    pub fn files_write(self) -> Approval {
        match self.files {
            FileAccess::ReadOnly => Approval::Deny,
            FileAccess::Write(approval) => approval,
        }
    }
}
