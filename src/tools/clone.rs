//! Repository cloning tool for fetching external codebases into a managed cache.
//!
//! Process boundary: `git clone`/`fetch`/`rev-parse` are wrapped in
//! `tokio::time::timeout` and `repo_clone` is registered as an
//! `external_side_effect` tool so the side-effect retry guard in
//! `tools::invoke_inner` covers it. The git process inherits the
//! shell's credential-env filter only by virtue of running from a
//! shell that already removed them; the clone tool does not run a
//! child shell.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::timeout;

use super::ToolContext;
use super::args::RepoCloneArgs;

const CLONE_TIMEOUT: Duration = Duration::from_secs(300);
const FETCH_TIMEOUT: Duration = Duration::from_secs(300);
const REV_PARSE_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Peel an optional `git+` prefix and a `#fragment` (treated as a
/// sub-path/ref annotation, not a URL fragment). The fragment is
/// preserved on the label but stripped from the remote used to
/// invoke `git`.
fn split_git_uri(input: &str) -> (String, Option<String>) {
    let trimmed = input.trim();
    let (no_git_plus, _) = trimmed
        .strip_prefix("git+")
        .map_or((trimmed, ()), |s| (s, ()));
    let (url_part, fragment) = match no_git_plus.split_once('#') {
        Some((url, frag)) => (url, Some(frag.to_string())),
        None => (no_git_plus, None),
    };
    (url_part.trim_end_matches('/').to_string(), fragment)
}

/// Parse an scp-style `git@host:owner/repo[.git]` reference. Returns
/// `None` if the input does not match the scp shape; the caller is
/// expected to fall through to a URL parse for other shapes.
fn parse_scp_reference(input: &str) -> Option<RepositoryReference> {
    let (user_host, path) = input.split_once(':')?;
    if path.is_empty() || path.contains("://") {
        return None;
    }
    let (user, host) = user_host.split_once('@')?;
    if user.is_empty() || host.is_empty() {
        return None;
    }
    if host.contains('@') || host.contains('/') {
        return None;
    }
    let segments: Vec<String> = path
        .trim_start_matches('/')
        .replace(".git", "")
        .split('/')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    if segments.is_empty() {
        return None;
    }
    let label = if host == "github.com" && segments.len() == 2 {
        segments.join("/")
    } else {
        format!("{}/{}", host, segments.join("/"))
    };
    Some(RepositoryReference {
        remote: input.to_string(),
        label,
        host: host.to_string(),
        segments,
    })
}

fn parse_repository_reference(input: &str) -> Result<RepositoryReference> {
    let (cleaned, _fragment) = split_git_uri(input);

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

    // scp-style: git@host:owner/repo[.git]
    if cleaned.contains('@')
        && cleaned.contains(':')
        && let Some(reference) = parse_scp_reference(&cleaned)
    {
        return Ok(reference);
    }

    // Full http(s):// or git+ssh:// URL
    if cleaned.starts_with("http://") || cleaned.starts_with("https://") {
        let url = url::Url::parse(&cleaned).context("invalid repository URL")?;
        let host = url.host_str().unwrap_or("unknown").to_string();
        let path = url.path().trim_start_matches('/').replace(".git", "");
        let segments: Vec<String> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
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
    if !branch
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '/' | '_' | '.' | '-'))
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
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(branch) = branch {
        cmd.arg("--branch").arg(branch);
    }

    let output = match timeout(CLONE_TIMEOUT, cmd.output()).await {
        Ok(result) => result.context("failed to execute git clone")?,
        Err(_) => bail!("git clone timed out after {}s", CLONE_TIMEOUT.as_secs()),
    };

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
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(branch) = branch {
        cmd.arg(branch);
    }

    let output = match timeout(FETCH_TIMEOUT, cmd.output()).await {
        Ok(result) => result.context("failed to execute git fetch")?,
        Err(_) => bail!("git fetch timed out after {}s", FETCH_TIMEOUT.as_secs()),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git fetch failed: {}", stderr);
    }

    Ok(())
}

async fn get_head(local_path: &PathBuf) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(local_path)
        .arg("rev-parse")
        .arg("HEAD")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = match timeout(REV_PARSE_TIMEOUT, cmd.output()).await {
        Ok(result) => result.context("failed to execute git rev-parse")?,
        Err(_) => bail!(
            "git rev-parse timed out after {}s",
            REV_PARSE_TIMEOUT.as_secs()
        ),
    };

    if !output.status.success() {
        bail!("git rev-parse failed");
    }

    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_git_uri_strips_fragment_and_prefix() {
        assert_eq!(
            split_git_uri("https://github.com/foo/bar.git#v1.2"),
            (
                "https://github.com/foo/bar.git".to_string(),
                Some("v1.2".to_string())
            )
        );
        assert_eq!(
            split_git_uri("git+https://github.com/foo/bar.git"),
            ("https://github.com/foo/bar.git".to_string(), None)
        );
        assert_eq!(
            split_git_uri("  https://github.com/foo/bar/  "),
            ("https://github.com/foo/bar".to_string(), None)
        );
    }

    #[test]
    fn parse_scp_reference_recognises_ssh_shapes() {
        let reference = parse_scp_reference("git@github.com:foo/bar.git").unwrap();
        assert_eq!(reference.host, "github.com");
        assert_eq!(reference.segments, vec!["foo", "bar"]);
        assert_eq!(reference.label, "foo/bar");
        assert_eq!(reference.remote, "git@github.com:foo/bar.git");
    }

    #[test]
    fn parse_scp_reference_rejects_non_scp_shapes() {
        assert!(parse_scp_reference("https://github.com/foo/bar").is_none());
        assert!(parse_scp_reference("github.com:foo/bar").is_none());
        assert!(parse_scp_reference("git@github.com").is_none());
    }

    #[test]
    fn parse_repository_reference_handles_url_fragments() {
        let reference = parse_repository_reference("https://github.com/foo/bar.git#v1.2").unwrap();
        assert_eq!(reference.remote, "https://github.com/foo/bar.git");
        assert_eq!(reference.host, "github.com");
        assert_eq!(reference.segments, vec!["foo", "bar"]);
    }

    #[test]
    fn parse_repository_reference_handles_scp_urls() {
        let reference = parse_repository_reference("git@github.com:foo/bar.git").unwrap();
        assert_eq!(reference.remote, "git@github.com:foo/bar.git");
        assert_eq!(reference.host, "github.com");
        assert_eq!(reference.segments, vec!["foo", "bar"]);
    }

    #[test]
    fn parse_repository_reference_rejects_empty_and_garbage() {
        assert!(parse_repository_reference("").is_err());
        assert!(parse_repository_reference("#fragment-only").is_err());
        assert!(parse_repository_reference("git@").is_err());
    }
}
