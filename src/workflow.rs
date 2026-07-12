//! Typed workflow context shared across OpenCode workflow orchestration.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{
    fs,
    hash::{Hash as _, Hasher as _},
    io::Write as _,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowKind {
    Audit,
    Review,
    Enhance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WorkflowScope {
    Workspace { path: String },
    GitDiff { target: String, oid: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowContext {
    pub schema_version: u16,
    pub run_id: String,
    pub kind: WorkflowKind,
    pub workspace: PathBuf,
    pub scope: WorkflowScope,
    pub focus: Vec<String>,
    pub output: PathBuf,
    pub format: String,
    pub max_chunks: usize,
    pub model: Option<String>,
    pub session_id: Option<String>,
    /// Accepted only so recovery can read leases created by oy 0.12 safety modes.
    #[serde(default, rename = "mode", skip_serializing)]
    pub legacy_mode: Option<String>,
    pub output_before: Option<u64>,
}

impl WorkflowContext {
    pub(crate) fn encode(&self) -> Result<String> {
        serde_json::to_string(self).context("failed encoding workflow context")
    }

    pub(crate) fn validate(&self, root: &Path) -> Result<()> {
        if self.schema_version != 1 {
            bail!("unsupported workflow context schema");
        }
        if self.max_chunks == 0 {
            bail!("max_chunks must be greater than zero");
        }
        if self.workspace != root {
            bail!("workflow context workspace does not match OY_ROOT");
        }
        Ok(())
    }
}

pub(crate) fn retained(root: &Path) -> Result<Option<WorkflowContext>> {
    let raw = match fs::read_to_string(context_path(root)) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed reading workflow recovery context"),
    };
    let context: WorkflowContext =
        serde_json::from_str(&raw).context("invalid retained workflow context")?;
    context.validate(root)?;
    Ok(Some(context))
}

pub(crate) fn output_digest(root: &Path, output: &Path) -> Result<Option<u64>> {
    let path = root.join(output);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        bail!("workflow output is not a regular file: {}", path.display());
    }
    let bytes = fs::read(path)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(Some(hasher.finish()))
}

pub(crate) struct WorkflowLease {
    path: PathBuf,
    keep_on_drop: bool,
}

impl WorkflowLease {
    pub(crate) fn acquire(context: &WorkflowContext) -> Result<Self> {
        let path = context_path(&context.workspace);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        if path.exists() {
            let existing: WorkflowContext = serde_json::from_str(&fs::read_to_string(&path)?)?;
            if existing.run_id == context.run_id {
                return Ok(Self {
                    path,
                    keep_on_drop: true,
                });
            }
            bail!(
                "an incomplete oy workflow already exists for this workspace; review or remove {}",
                path.display()
            );
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
        let mut file = options.open(&path)?;
        file.write_all(context.encode()?.as_bytes())?;
        file.sync_all()?;
        Ok(Self {
            path,
            keep_on_drop: true,
        })
    }

    pub(crate) fn complete(mut self) {
        self.keep_on_drop = false;
        let _ = fs::remove_file(&self.path);
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorkflowLease {
    fn drop(&mut self) {
        if !self.keep_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn context_path(root: &Path) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("oy/workflows")
        .join(format!("{:016x}.json", hasher.finish()))
}

pub(crate) fn new_run_id() -> Result<String> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("failed generating workflow run id: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(crate) fn resolve_scope(root: &Path, focus: &[String]) -> Result<(WorkflowScope, Vec<String>)> {
    if focus.len() == 1 {
        let requested = Path::new(&focus[0]);
        if !requested.is_absolute()
            && !requested
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            let candidate = root.join(requested);
            if candidate.exists() {
                let resolved = candidate.canonicalize()?;
                if !resolved.starts_with(root) {
                    bail!("workflow path escaped workspace");
                }
                let relative = resolved.strip_prefix(root)?.to_string_lossy().into_owned();
                return Ok((WorkflowScope::Workspace { path: relative }, Vec::new()));
            }
        }
    }
    Ok((
        WorkflowScope::Workspace {
            path: ".".to_string(),
        },
        focus.to_vec(),
    ))
}

pub(crate) fn resolve_diff_scope(root: &Path, target: &str) -> Result<WorkflowScope> {
    let expression = format!("{target}^{{commit}}");
    let output = Command::new("git")
        .args(["rev-parse", "--verify", expression.as_str()])
        .current_dir(root)
        .output()
        .context("failed resolving review target")?;
    if !output.status.success() {
        bail!("invalid review target: {target}");
    }
    let oid = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(WorkflowScope::GitDiff {
        target: target.to_string(),
        oid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_focus_becomes_bound_scope() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("scope.rs"), "fn main() {}\n").unwrap();
        let (scope, focus) = resolve_scope(root.path(), &["scope.rs".to_string()]).unwrap();
        assert_eq!(
            scope,
            WorkflowScope::Workspace {
                path: "scope.rs".to_string()
            }
        );
        assert!(focus.is_empty());
    }

    #[test]
    fn workflow_lease_is_visible_to_context_reader() {
        let root = tempfile::tempdir().unwrap();
        let context = WorkflowContext {
            schema_version: 1,
            run_id: "a".repeat(48),
            kind: WorkflowKind::Audit,
            workspace: root.path().to_path_buf(),
            scope: WorkflowScope::Workspace {
                path: ".".to_string(),
            },
            focus: Vec::new(),
            output: PathBuf::from("ISSUES.md"),
            format: "markdown".to_string(),
            max_chunks: 5,
            model: None,
            session_id: None,
            legacy_mode: None,
            output_before: None,
        };
        let lease = WorkflowLease::acquire(&context).unwrap();
        assert_eq!(retained(root.path()).unwrap(), Some(context));
        let recovered = retained(root.path()).unwrap().unwrap();
        let resumed_lease = WorkflowLease::acquire(&recovered).unwrap();
        drop(resumed_lease);
        lease.complete();
        assert!(retained(root.path()).unwrap().is_none());
    }

    #[test]
    fn legacy_mode_is_accepted_but_not_reencoded() {
        let root = tempfile::tempdir().unwrap();
        let context: WorkflowContext = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "run_id": "b".repeat(48),
            "kind": "enhance",
            "workspace": root.path(),
            "scope": { "type": "workspace", "path": "." },
            "focus": [],
            "output": "REVIEW.md",
            "format": "auto-approve",
            "max_chunks": 5,
            "model": null,
            "session_id": null,
            "mode": "auto-approve",
            "output_before": null
        }))
        .unwrap();

        assert_eq!(context.legacy_mode.as_deref(), Some("auto-approve"));
        assert!(!context.encode().unwrap().contains("\"mode\""));
    }
}
