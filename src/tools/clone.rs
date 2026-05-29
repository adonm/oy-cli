//! Repository cloning tool for fetching external codebases into a managed cache.

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;

use super::args::RepoCloneArgs;
use super::ToolContext;

#[derive(Debug, Serialize)]
pub(super) struct RepoCloneOutput {
    pub repository: String,
    pub local_path: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
}

pub(super) async fn tool_repo_clone(_ctx: &ToolContext, args: RepoCloneArgs) -> Result<Value> {
    let reference = parse_repository_reference(&args.repository)?;
    
    if let Some(ref branch) = args.branch {
        validate_branch(branch)?;
    }

    let local_path = cache_path_for_reference(&reference);
    
    let (status, head) = if local_path.exists() && local_path.join(".git").exists() {
        // Already cloned, optionally refresh
        if args.refresh == Some(true) {
            fetch_latest(&local_path, args.branch.as_deref()).await?;
            ("refreshed".to_string(), get_head(&local_path).await.ok())
        } else {
            ("cached".to_string(), get_head(&local_path).await.ok())
        }
    } else {
        // Fresh clone
        clone_repository(&reference.remote, &local_path, args.branch.as_deref()).await?;
        ("cloned".to_string(), get_head(&local_path).await.ok())
    };

    let output = RepoCloneOutput {
        repository: reference.label,
        local_path: local_path.to_string_lossy().to_string(),
        status,
        branch: args.branch,
        head,
    };

    Ok(serde_json::to_value(output)?)
}

#[derive(Debug)]
struct RepositoryReference {
    remote: String,
    label: String,
    host: String,
    segments: Vec<String>,
}

fn parse_repository_reference(input: &str) -> Result<RepositoryReference> {
    let cleaned = input
        .trim()
        .replace("git+", "")
        .replace('#', "")
        .trim_end_matches('/')
        .to_string();
    
    if cleaned.is_empty() {
        bail!("repository reference cannot be empty");
    }

    // GitHub shorthand: owner/repo
    if !cleaned.contains("://") && !cleaned.contains(':') {
        let parts: Vec<&str> = cleaned.split('/').collect();
        if parts.len() == 2 {
            return Ok(RepositoryReference {
                remote: format!("https://github.com/{}.git", cleaned),
                label: cleaned.clone(),
                host: "github.com".to_string(),
                segments: parts.into_iter().map(String::from).collect(),
            });
        }
    }

    // Full git URL
    if cleaned.starts_with("http://") || cleaned.starts_with("https://") || cleaned.starts_with("git@") {
        let url = url::Url::parse(&cleaned).context("invalid repository URL")?;
        let host = url.host_str().unwrap_or("unknown").to_string();
        let path = url.path().trim_start_matches('/').replace(".git", "");
        let segments: Vec<String> = path.split('/').map(String::from).collect();
        let label = if host == "github.com" && segments.len() == 2 {
            segments.join("/")
        } else {
            format!("{}/{}", host, segments.join("/"))
        };
        
        return Ok(RepositoryReference {
            remote: cleaned,
            label,
            host,
            segments,
        });
    }

    bail!("invalid repository format: {}", input);
}

fn validate_branch(branch: &str) -> Result<()> {
    if !branch.chars().all(|c| c.is_alphanumeric() || matches!(c, '/' | '_' | '.' | '-')) 
        || branch.starts_with('-') 
        || branch.contains("..") 
    {
        bail!(
            "branch must contain only alphanumeric characters, /, _, ., and -, and cannot start with - or contain .."
        );
    }
    Ok(())
}

fn cache_path_for_reference(reference: &RepositoryReference) -> PathBuf {
    let cache_dir = super::workspace::repos_cache_root();

    let mut path = cache_dir.join(&reference.host);
    for segment in &reference.segments {
        path = path.join(segment);
    }
    path
}

async fn clone_repository(remote: &str, local_path: &PathBuf, branch: Option<&str>) -> Result<()> {
    if let Some(parent) = local_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("failed to create cache directory")?;
    }

    let mut cmd = Command::new("git");
    cmd.arg("clone")
        .arg("--depth")
        .arg("1")
        .arg(remote)
        .arg(local_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(branch) = branch {
        cmd.arg("--branch").arg(branch);
    }

    let output = cmd
        .output()
        .await
        .context("failed to execute git clone")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git clone failed: {}", stderr);
    }

    Ok(())
}

async fn fetch_latest(local_path: &PathBuf, branch: Option<&str>) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.current_dir(local_path)
        .arg("fetch")
        .arg("--depth")
        .arg("1")
        .arg("origin")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(branch) = branch {
        cmd.arg(branch);
    }

    let output = cmd
        .output()
        .await
        .context("failed to execute git fetch")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git fetch failed: {}", stderr);
    }

    Ok(())
}

async fn get_head(local_path: &PathBuf) -> Result<String> {
    let output = Command::new("git")
        .current_dir(local_path)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .await
        .context("failed to execute git rev-parse")?;

    if !output.status.success() {
        bail!("git rev-parse failed");
    }

    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(head)
}
