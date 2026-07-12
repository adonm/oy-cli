//! File-backed deterministic evidence preparation and report finalization.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::audit::input::{self, AuditChunk, AuditFile};
use crate::{audit, config};

pub(crate) const DEFAULT_TARGET_TOKENS: usize = 64_000;
const SCHEMA_VERSION: u16 = 1;
const MAX_EXISTING_REPORT_BYTES: u64 = 1024 * 1024;
const MAX_CANDIDATE_REPORT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CANDIDATE_FINDINGS_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Audit,
    Review,
}

#[derive(Debug, Clone)]
pub(crate) struct PrepareRequest {
    pub kind: Kind,
    pub path: String,
    pub target: Option<String>,
    pub output: PathBuf,
    pub format: String,
    pub focus: Vec<String>,
    pub max_chunks: usize,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Evidence {
    pub source: String,
    pub manifest: String,
    pub chunks: Vec<AuditChunk>,
    pub target: Option<String>,
    pub target_oid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunState {
    schema_version: u16,
    run_id: String,
    kind: Kind,
    workspace: PathBuf,
    path: String,
    target: Option<String>,
    target_oid: Option<String>,
    output: PathBuf,
    format: String,
    focus: Vec<String>,
    max_chunks: usize,
    model: Option<String>,
    generated_on: String,
    evidence_digest: String,
    candidate_report: PathBuf,
    candidate_findings: PathBuf,
    output_before: Option<String>,
    artifacts: Vec<ArtifactDigest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArtifactDigest {
    path: PathBuf,
    sha256: String,
    bytes: usize,
    lines: usize,
}

pub(crate) fn prepare(root: &Path, request: PrepareRequest) -> Result<Value> {
    if request.max_chunks == 0 {
        bail!("max_chunks must be greater than zero");
    }
    validate_format(request.kind, &request.format)?;
    let output_path = config::resolve_workspace_output_path(root, &request.output)?;
    let evidence = collect(root, &request)?;
    if evidence.chunks.is_empty() {
        bail!("no reviewable text evidence found");
    }
    if evidence.chunks.len() > request.max_chunks {
        bail!(
            "chunk_limit_exceeded: {} chunks exceeds max_chunks {}",
            evidence.chunks.len(),
            request.max_chunks
        );
    }

    let run_id = crate::workflow::new_run_id()?;
    let artifact_dir = PathBuf::from(format!(".oy/runs/{run_id}"));
    let candidate_report = artifact_dir.join("candidate/report.md");
    let candidate_findings = artifact_dir.join("candidate/findings.json");
    let candidate_report_path = config::resolve_workspace_output_path(root, &candidate_report)?;
    fs::create_dir_all(
        candidate_report_path
            .parent()
            .expect("candidate report path has parent"),
    )?;
    let manifest_path = artifact_dir.join("manifest.md");
    let mut artifacts = vec![write_artifact(
        root,
        &manifest_path,
        evidence.manifest.as_bytes(),
    )?];

    let mut chunk_artifacts = Vec::with_capacity(evidence.chunks.len());
    for (index, chunk) in evidence.chunks.iter().enumerate() {
        let path = artifact_dir
            .join("chunks")
            .join(format!("{:04}.txt", index + 1));
        let text = input::chunk_text(chunk);
        let artifact = write_artifact(root, &path, text.as_bytes())?;
        chunk_artifacts.push(artifact.clone());
        artifacts.push(artifact);
    }

    let previous_report = copy_previous_report(root, &request.output, &artifact_dir)?;
    if let Some(previous) = &previous_report {
        artifacts.push(previous.clone());
    }
    let evidence_digest = evidence_digest(&evidence.chunks);
    let source_paths = evidence
        .chunks
        .iter()
        .flat_map(|chunk| chunk.files.iter().map(|file| file.path.as_str()))
        .collect::<BTreeSet<_>>();
    let evidence_bytes = evidence
        .chunks
        .iter()
        .flat_map(|chunk| &chunk.files)
        .map(|file| file.text.len())
        .sum::<usize>();
    let evidence_lines = evidence
        .chunks
        .iter()
        .flat_map(|chunk| &chunk.files)
        .map(|file| file.text.lines().count())
        .sum::<usize>();
    let index_path = artifact_dir.join("index.json");
    let index = json!({
        "schema_version": SCHEMA_VERSION,
        "run_id": run_id,
        "kind": request.kind,
        "source": evidence.source,
        "scope": {
            "path": request.path,
            "target": evidence.target,
            "target_oid": evidence.target_oid,
        },
        "output": request.output,
        "format": request.format,
        "focus": request.focus,
        "max_chunks": request.max_chunks,
        "model": request.model,
        "evidence_digest": evidence_digest,
        "coverage": {
            "files": source_paths.len(),
            "chunks": chunk_artifacts.len(),
            "bytes": evidence_bytes,
            "lines": evidence_lines,
            "estimated_tokens": evidence.chunks.iter().map(|chunk| chunk.tokens).sum::<usize>(),
        },
        "exclusions": [
            "gitignored, hidden, dependency, build, and oy run-state paths",
            "lockfiles, likely-secret files, binary/non-UTF-8/empty files",
            "unreadable files and files larger than 512 KiB",
            "the bound report output",
        ],
        "manifest": manifest_path,
        "previous_report": previous_report.as_ref().map(|artifact| &artifact.path),
        "candidate_report": candidate_report,
        "candidate_findings": candidate_findings,
        "chunk_count": chunk_artifacts.len(),
        "chunks": chunk_artifacts.iter().enumerate().map(|(index, artifact)| json!({
            "number": index + 1,
            "path": artifact.path,
            "sha256": artifact.sha256,
            "bytes": artifact.bytes,
            "lines": artifact.lines,
        })).collect::<Vec<_>>(),
    });
    artifacts.push(write_artifact(
        root,
        &index_path,
        serde_json::to_string_pretty(&index)?.as_bytes(),
    )?);

    let output_before = previous_report
        .as_ref()
        .map(|artifact| artifact.sha256.clone());
    let output_after_snapshot = if output_path.exists() {
        Some(digest_bytes(&fs::read(&output_path)?))
    } else {
        None
    };
    if output_after_snapshot != output_before {
        bail!("output_changed: bound report changed during preparation");
    }

    let state = RunState {
        schema_version: SCHEMA_VERSION,
        run_id: run_id.clone(),
        kind: request.kind,
        workspace: root.canonicalize()?,
        path: request.path,
        target: evidence.target,
        target_oid: evidence.target_oid,
        output: request.output,
        format: request.format,
        focus: request.focus,
        max_chunks: request.max_chunks,
        model: request.model,
        generated_on: audit::report::utc_date_string(),
        evidence_digest: evidence_digest.clone(),
        candidate_report: candidate_report.clone(),
        candidate_findings: candidate_findings.clone(),
        output_before,
        artifacts,
    };
    write_state(&state)?;

    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "run_id": run_id,
        "kind": request.kind,
        "index": index_path,
        "candidate_report": candidate_report,
        "candidate_findings": candidate_findings,
        "output": state.output,
        "chunk_count": chunk_artifacts.len(),
        "evidence_digest": evidence_digest,
        "next": format!("Read the index and every chunk, write {} and {}, then run `oy {} finalize --run {}`", state.candidate_report.display(), state.candidate_findings.display(), match state.kind { Kind::Audit => "audit", Kind::Review => "review" }, state.run_id),
    }))
}

pub(crate) fn finalize(root: &Path, run_id: &str) -> Result<Value> {
    validate_run_id(run_id)?;
    let state = read_state(run_id)?;
    let canonical_root = root.canonicalize()?;
    if state.workspace != canonical_root {
        bail!("workflow workspace does not match the active workspace");
    }
    if state.schema_version != SCHEMA_VERSION {
        bail!("unsupported artifact workflow schema");
    }

    let request = PrepareRequest {
        kind: state.kind,
        path: state.path.clone(),
        target: state.target_oid.clone().or_else(|| state.target.clone()),
        output: state.output.clone(),
        format: state.format.clone(),
        focus: state.focus.clone(),
        max_chunks: state.max_chunks,
        model: state.model.clone(),
    };
    let current = collect(root, &request)?;
    if evidence_digest(&current.chunks) != state.evidence_digest {
        bail!("input_changed: repository evidence changed after preparation");
    }
    for artifact in &state.artifacts {
        let path = config::resolve_workspace_output_path(root, &artifact.path)?;
        let bytes = fs::read(&path)
            .with_context(|| format!("failed reading evidence artifact: {}", path.display()))?;
        if digest_bytes(&bytes) != artifact.sha256 {
            bail!(
                "artifact_changed: {} no longer matches prepared evidence",
                artifact.path.display()
            );
        }
    }

    let output_path = config::resolve_workspace_output_path(root, &state.output)?;
    let output_now = if output_path.exists() {
        Some(digest_bytes(&fs::read(&output_path)?))
    } else {
        None
    };
    if output_now != state.output_before {
        bail!("output_changed: bound report changed after preparation");
    }

    let candidate = read_candidate(
        root,
        &state.candidate_report,
        MAX_CANDIDATE_REPORT_BYTES,
        "report",
    )?;
    if candidate.trim().is_empty() {
        bail!(
            "candidate report is empty: {}",
            state.candidate_report.display()
        );
    }
    let findings_text = read_candidate(
        root,
        &state.candidate_findings,
        MAX_CANDIDATE_FINDINGS_BYTES,
        "findings",
    )?;
    let findings_value: Value =
        serde_json::from_str(&findings_text).context("candidate findings are not valid JSON")?;
    let source = match state.kind {
        Kind::Audit => "audit",
        Kind::Review => "review",
    };
    let findings = audit::report::normalized_findings_payload_strict(&findings_value, source)?;
    let mut report = audit::report::with_structured_findings_payload(&candidate, &findings);
    report = match state.kind {
        Kind::Audit => audit::report::with_audit_transparency_line(
            &report,
            &audit::report::audit_transparency_snippet_at(
                state.model.as_deref(),
                (!state.focus.is_empty())
                    .then(|| state.focus.join(" "))
                    .as_deref(),
                &state.output,
                Some(state.max_chunks),
                parse_audit_format(&state.format)?,
                &state.generated_on,
            ),
        ),
        Kind::Review => audit::report::with_review_transparency_line(
            &report,
            &audit::report::review_transparency_snippet_at(
                state.model.as_deref(),
                state.target.as_deref(),
                (!state.focus.is_empty())
                    .then(|| state.focus.join(" "))
                    .as_deref(),
                &state.output,
                Some(state.max_chunks),
                &state.generated_on,
            ),
        ),
    };
    report = add_run_provenance(&report, &state);
    let finding_count = audit::report::findings_from_report(&report).len();
    let output = match (state.kind, state.format.as_str()) {
        (Kind::Audit, "sarif") => audit::report::render_sarif(&report)?,
        (Kind::Audit, "markdown") => audit::report::with_succinct_findings_summary(&report),
        (Kind::Review, "markdown") => report,
        _ => bail!("unsupported report format"),
    };
    config::write_workspace_file(&output_path, output.as_bytes())?;
    fs::remove_file(state_path(run_id)?).context("failed completing artifact workflow state")?;

    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "run_id": run_id,
        "output": state.output,
        "format": state.format,
        "findings": finding_count,
        "evidence_digest": state.evidence_digest,
    }))
}

pub(crate) fn collect(root: &Path, request: &PrepareRequest) -> Result<Evidence> {
    match request.kind {
        Kind::Audit => {
            if request.target.is_some() {
                bail!("audit preparation does not accept a git target");
            }
            repository_excluding(
                root,
                &request.path,
                request.model.as_deref().unwrap_or(""),
                Some(&request.output),
            )
        }
        Kind::Review => {
            if let Some(target) = request.target.as_deref() {
                diff_excluding(
                    root,
                    target,
                    request.model.as_deref().unwrap_or(""),
                    Some(&request.output),
                )
            } else {
                repository_excluding(
                    root,
                    &request.path,
                    request.model.as_deref().unwrap_or(""),
                    Some(&request.output),
                )
            }
        }
    }
}

pub(crate) fn repository(root: &Path, path: &str, model: &str) -> Result<Evidence> {
    repository_excluding(root, path, model, None)
}

fn repository_excluding(
    root: &Path,
    path: &str,
    model: &str,
    output: Option<&Path>,
) -> Result<Evidence> {
    let resolved = resolve_workspace_path(root, path)?;
    let output = output
        .map(|path| config::resolve_workspace_output_path(root, path))
        .transpose()?;
    let files = if resolved.is_dir() {
        let canonical_root = root.canonicalize()?;
        let prefix = resolved
            .strip_prefix(&canonical_root)?
            .to_string_lossy()
            .trim_matches('/')
            .to_string();
        let prefix_with_sep = format!("{prefix}/");
        input::collect_files(root, output.as_deref(), model)?
            .into_iter()
            .filter(|file| {
                prefix.is_empty()
                    || file.path == prefix
                    || file.path.starts_with(prefix_with_sep.as_str())
            })
            .collect::<Vec<_>>()
    } else if resolved.is_file() {
        if output.as_ref().is_some_and(|output| output == &resolved) {
            Vec::new()
        } else {
            input::collect_file(root, &resolved, model)?
                .into_iter()
                .collect()
        }
    } else {
        bail!("path is not a file or directory: {path}");
    };
    evidence_from_files(format!("workspace path {path}"), files, None, None)
}

pub(crate) fn diff(root: &Path, target: &str, model: &str) -> Result<Evidence> {
    diff_excluding(root, target, model, None)
}

fn diff_excluding(
    root: &Path,
    target: &str,
    model: &str,
    output: Option<&Path>,
) -> Result<Evidence> {
    validate_target_ref(target)?;
    let oid = git_output(
        root,
        &["rev-parse", "--verify", &format!("{target}^{{commit}}")],
    )?
    .trim()
    .to_string();
    let diff = git_output(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--find-renames",
            "--find-copies",
            "--unified=80",
            &oid,
            "--",
        ],
    )?;
    if diff.trim().is_empty() {
        bail!("no git diff found against target {target}");
    }
    let stats = input::parse_numstat(&git_output(root, &["diff", "--numstat", &oid, "--"])?);
    let excluded = output.map(|path| path.to_string_lossy().into_owned());
    let files = input::collect_diff_files(&diff, model)
        .into_iter()
        .filter(|file| excluded.as_ref() != Some(&file.path))
        .collect::<Vec<_>>();
    let manifest = input::build_diff_manifest(target, &files, &stats);
    let chunks = input::chunk_files(files, DEFAULT_TARGET_TOKENS);
    input::ensure_chunks_fit_prompt(&chunks, DEFAULT_TARGET_TOKENS)?;
    Ok(Evidence {
        source: format!("git diff against {target}"),
        manifest,
        chunks,
        target: Some(target.to_string()),
        target_oid: Some(oid),
    })
}

fn evidence_from_files(
    source: String,
    files: Vec<AuditFile>,
    target: Option<String>,
    target_oid: Option<String>,
) -> Result<Evidence> {
    if files.is_empty() {
        bail!("no reviewable text files found");
    }
    let manifest = input::build_manifest(&files);
    let chunks = input::chunk_files(files, DEFAULT_TARGET_TOKENS);
    input::ensure_chunks_fit_prompt(&chunks, DEFAULT_TARGET_TOKENS)?;
    Ok(Evidence {
        source,
        manifest,
        chunks,
        target,
        target_oid,
    })
}

pub(crate) fn evidence_digest(chunks: &[AuditChunk]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"oy-evidence-v1\0");
    for chunk in chunks {
        for file in &chunk.files {
            hash.update((file.path.len() as u64).to_le_bytes());
            hash.update(file.path.as_bytes());
            hash.update((file.text.len() as u64).to_le_bytes());
            hash.update(file.text.as_bytes());
        }
    }
    format!("sha256:{:x}", hash.finalize())
}

fn write_artifact(root: &Path, relative: &Path, bytes: &[u8]) -> Result<ArtifactDigest> {
    let path = config::resolve_workspace_output_path(root, relative)?;
    config::write_workspace_file(&path, bytes)?;
    Ok(ArtifactDigest {
        path: relative.to_path_buf(),
        sha256: digest_bytes(bytes),
        bytes: bytes.len(),
        lines: bytes.iter().filter(|byte| **byte == b'\n').count()
            + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n")),
    })
}

fn copy_previous_report(
    root: &Path,
    output: &Path,
    artifact_dir: &Path,
) -> Result<Option<ArtifactDigest>> {
    let output_path = config::resolve_workspace_output_path(root, output)?;
    if !output_path.exists() {
        return Ok(None);
    }
    let metadata = fs::metadata(&output_path)?;
    if metadata.len() > MAX_EXISTING_REPORT_BYTES {
        bail!("existing report exceeds the 1 MiB carry-forward limit");
    }
    let previous = artifact_dir.join("previous-report.md");
    Ok(Some(write_artifact(
        root,
        &previous,
        &fs::read(output_path)?,
    )?))
}

fn read_candidate(root: &Path, relative: &Path, limit: u64, label: &str) -> Result<String> {
    let path = config::resolve_workspace_output_path(root, relative)?;
    let metadata = fs::metadata(&path).with_context(|| {
        format!(
            "candidate {label} is missing; write it to {} before finalizing",
            relative.display()
        )
    })?;
    if !metadata.is_file() {
        bail!(
            "candidate {label} is not a regular file: {}",
            relative.display()
        );
    }
    if metadata.len() > limit {
        bail!(
            "candidate {label} exceeds the {} byte limit: {}",
            limit,
            relative.display()
        );
    }
    fs::read_to_string(&path).with_context(|| {
        format!(
            "candidate {label} is not valid UTF-8: {}",
            relative.display()
        )
    })
}

fn write_state(state: &RunState) -> Result<()> {
    let path = state_path(&state.run_id)?;
    let parent = path.parent().expect("state path has parent");
    fs::create_dir_all(parent)?;
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temp, state)?;
    temp.as_file().sync_all()?;
    temp.persist(&path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed writing workflow state: {}", path.display()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn read_state(run_id: &str) -> Result<RunState> {
    let path = state_path(run_id)?;
    serde_json::from_slice(
        &fs::read(&path)
            .with_context(|| format!("unknown or completed artifact workflow: {run_id}"))?,
    )
    .context("invalid artifact workflow state")
}

fn state_path(run_id: &str) -> Result<PathBuf> {
    validate_run_id(run_id)?;
    let base = dirs::state_dir().ok_or_else(|| anyhow!("user state directory is unavailable"))?;
    Ok(base.join("oy/prepared-runs").join(format!("{run_id}.json")))
}

fn validate_run_id(run_id: &str) -> Result<()> {
    if run_id.len() != 48 || !run_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid workflow run id");
    }
    Ok(())
}

fn resolve_workspace_path(root: &Path, path: &str) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let raw = Path::new(path);
    if raw
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("path must stay inside workspace: {path}");
    }
    let candidate = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };
    let resolved = candidate
        .canonicalize()
        .with_context(|| format!("path does not exist: {path}"))?;
    if !resolved.starts_with(&root) {
        bail!("path escapes workspace: {path}");
    }
    Ok(resolved)
}

fn validate_target_ref(target: &str) -> Result<()> {
    if target.trim().is_empty() || target.starts_with('-') {
        bail!("target must be a non-option branch, commit, or ref");
    }
    if target.contains(['\0', '\n', '\r']) {
        bail!("target contains invalid control characters");
    }
    Ok(())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .lines()
                .next()
                .unwrap_or("unknown git error")
        );
    }
    String::from_utf8(output.stdout).context("git output was not UTF-8")
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn validate_format(kind: Kind, format: &str) -> Result<()> {
    match (kind, format) {
        (Kind::Audit, "markdown" | "sarif") | (Kind::Review, "markdown") => Ok(()),
        (Kind::Review, _) => bail!("review reports support markdown only"),
        _ => bail!("unsupported audit report format: {format}"),
    }
}

fn parse_audit_format(format: &str) -> Result<audit::AuditOutputFormat> {
    match format {
        "markdown" => Ok(audit::AuditOutputFormat::Markdown),
        "sarif" => Ok(audit::AuditOutputFormat::Sarif),
        _ => Err(anyhow!("unsupported audit report format: {format}")),
    }
}

fn add_run_provenance(report: &str, state: &RunState) -> String {
    let line = format!(
        "> oy evidence: schema `{}` · digest `{}` · artifacts `{}`",
        state.schema_version,
        state.evidence_digest,
        state.artifacts.len()
    );
    let mut lines = report.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let index = lines
        .iter()
        .position(|existing| existing.starts_with("> Generated with"))
        .map_or(1.min(lines.len()), |index| index + 1);
    lines.insert(index, line);
    let mut output = lines.join("\n");
    output.push('\n');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_stable_and_content_sensitive() {
        let file = |text: &str| AuditFile {
            path: "src/lib.rs".to_string(),
            language: "Rust",
            bytes: text.len() as u64,
            tokens: 1,
            text: text.to_string(),
            slice: None,
        };
        let first = vec![AuditChunk {
            files: vec![file("one")],
            tokens: 1,
        }];
        let second = vec![AuditChunk {
            files: vec![file("two")],
            tokens: 1,
        }];
        assert_eq!(evidence_digest(&first), evidence_digest(&first));
        assert_ne!(evidence_digest(&first), evidence_digest(&second));
    }

    #[test]
    fn invalid_run_ids_are_rejected() {
        assert!(validate_run_id("../state").is_err());
        assert!(validate_run_id(&"a".repeat(48)).is_ok());
    }

    #[test]
    fn prepare_and_finalize_file_backed_audit() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::write(
            root.path().join("src/lib.rs"),
            "pub fn answer() -> u8 { 42 }\n",
        )
        .unwrap();
        let prepared = prepare(
            root.path(),
            PrepareRequest {
                kind: Kind::Audit,
                path: ".".to_string(),
                target: None,
                output: PathBuf::from("ISSUES.md"),
                format: "markdown".to_string(),
                focus: Vec::new(),
                max_chunks: 10,
                model: None,
            },
        )
        .unwrap();
        let run_id = prepared["run_id"].as_str().unwrap();
        let candidate = root
            .path()
            .join(prepared["candidate_report"].as_str().unwrap());
        fs::write(
            candidate,
            "# Audit Issues\n\n## Findings summary\n\nNo concrete findings.\n\n## Detailed findings\n",
        )
        .unwrap();
        fs::write(
            root.path()
                .join(prepared["candidate_findings"].as_str().unwrap()),
            "[]\n",
        )
        .unwrap();

        let result = finalize(root.path(), run_id).unwrap();
        assert_eq!(result["findings"], 0);
        let report = fs::read_to_string(root.path().join("ISSUES.md")).unwrap();
        assert!(report.contains("oy evidence: schema"));
        assert!(report.contains("```json oy-findings\n[]"));
    }

    #[test]
    fn finalize_rejects_changed_repository_evidence() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("lib.rs"), "fn before() {}\n").unwrap();
        let prepared = prepare(
            root.path(),
            PrepareRequest {
                kind: Kind::Review,
                path: ".".to_string(),
                target: None,
                output: PathBuf::from("REVIEW.md"),
                format: "markdown".to_string(),
                focus: Vec::new(),
                max_chunks: 10,
                model: None,
            },
        )
        .unwrap();
        fs::write(root.path().join("lib.rs"), "fn after() {}\n").unwrap();

        let error = finalize(root.path(), prepared["run_id"].as_str().unwrap()).unwrap_err();
        assert!(error.to_string().contains("input_changed"));
        let _ = fs::remove_file(state_path(prepared["run_id"].as_str().unwrap()).unwrap());
    }

    #[test]
    fn finalize_rejects_tampered_index_artifact() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("lib.rs"), "fn stable() {}\n").unwrap();
        let prepared = prepare(
            root.path(),
            PrepareRequest {
                kind: Kind::Audit,
                path: ".".to_string(),
                target: None,
                output: PathBuf::from("ISSUES.md"),
                format: "markdown".to_string(),
                focus: Vec::new(),
                max_chunks: 10,
                model: None,
            },
        )
        .unwrap();
        let run_id = prepared["run_id"].as_str().unwrap();
        fs::write(
            root.path().join(prepared["index"].as_str().unwrap()),
            "{}\n",
        )
        .unwrap();

        let error = finalize(root.path(), run_id).unwrap_err();

        assert!(error.to_string().contains("artifact_changed"));
        let _ = fs::remove_file(state_path(run_id).unwrap());
    }
}
