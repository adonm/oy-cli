//! Minimal stdio MCP server exposing deterministic oy primitives.

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};

use crate::audit::input;
use crate::{audit, config, tools, ui};

const DEFAULT_MODEL_FOR_COUNTING: &str = "cl100k_base";
pub(crate) const DEFAULT_TARGET_TOKENS: usize = 64_000;
const MAX_EXISTING_REPORT_BYTES: u64 = 1024 * 1024;

static CLEAN_REPO_INPUT_CACHE: LazyLock<Mutex<HashMap<String, Vec<input::AuditFile>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) async fn serve_stdio() -> Result<i32> {
    ui::set_output_mode(ui::OutputMode::Quiet);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line.context("failed reading MCP stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<Value>(&line) {
            Ok(request) => request,
            Err(err) => {
                write_response(
                    &mut stdout,
                    jsonrpc_error(Value::Null, -32700, err.to_string()),
                )?;
                continue;
            }
        };
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let response = match handle_request(method, params).await {
            Ok(response) => response.into_json(id),
            Err(err) => jsonrpc_error(id, -32603, err.to_string()),
        };
        write_response(&mut stdout, response)?;
    }
    Ok(0)
}

async fn handle_request(method: &str, params: Value) -> Result<JsonRpcResponse> {
    match method {
        "initialize" => Ok(JsonRpcResponse::Result(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "oy", "version": env!("CARGO_PKG_VERSION") }
        }))),
        "ping" => Ok(JsonRpcResponse::Result(json!({}))),
        "tools/list" => Ok(JsonRpcResponse::Result(
            json!({ "tools": tool_definitions() }),
        )),
        "tools/call" => handle_tool_call(params).await.map(JsonRpcResponse::Result),
        other => Ok(JsonRpcResponse::Error {
            code: -32601,
            message: format!("unknown MCP method: {other}"),
        }),
    }
}

enum JsonRpcResponse {
    Result(Value),
    Error { code: i32, message: String },
}

impl JsonRpcResponse {
    fn into_json(self, id: Value) -> Value {
        match self {
            Self::Result(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Self::Error { code, message } => jsonrpc_error(id, code, message),
        }
    }
}

async fn handle_tool_call(params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tools/call missing tool name"))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = match name {
        "repo_manifest" => repo_manifest(args)?,
        "repo_chunks" => repo_chunks(args)?,
        "git_diff_input" => git_diff_input(args)?,
        "existing_report" => existing_report(args)?,
        "outline" => builtin_tool("outline", args).await?,
        "sloc" => builtin_tool("sloc", args).await?,
        "sighthound" => builtin_tool("sighthound", args).await?,
        "render_audit_report" => render_audit_report(args)?,
        "render_review_report" => render_review_report(args)?,
        other => bail!("unknown oy MCP tool: {other}"),
    };
    Ok(json!({
        "content": [{ "type": "text", "text": result_text(result)? }],
        "isError": false
    }))
}

async fn builtin_tool(name: &str, args: Value) -> Result<Value> {
    tools::invoke_read_only_deterministic(workspace_root()?, name, args).await
}

fn result_text(value: Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value),
        other => serde_json::to_string_pretty(&other).context("failed encoding tool result"),
    }
}

fn write_response(stdout: &mut std::io::Stdout, response: Value) -> Result<()> {
    serde_json::to_writer(&mut *stdout, &response).context("failed writing MCP response")?;
    stdout
        .write_all(b"\n")
        .context("failed writing MCP newline")?;
    stdout.flush().context("failed flushing MCP response")
}

fn jsonrpc_error(id: Value, code: i32, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() }
    })
}

fn tool_definitions() -> Vec<Value> {
    let mut tools = vec![
        tool_def(
            "repo_manifest",
            "Build a deterministic, gitignore-aware repository manifest for audit/review planning.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative file or directory to inspect", "default": "." },
                    "model": { "type": "string", "description": "Model label retained for workflow metadata; token estimates use oy's approximate byte heuristic" },
                    "security_index": { "type": "boolean", "default": true },
                    "security_index_limit": { "type": "integer", "default": 120 }
                }
            }),
        ),
        tool_def(
            "repo_chunks",
            "Prepare deterministic repository chunks for a workspace-relative file or directory. Omit chunk to list summaries; pass a 1-based chunk number to get that chunk's text.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "default": "." },
                    "model": { "type": "string", "description": "Model label retained for workflow metadata; token estimates use oy's approximate byte heuristic" },
                    "target_tokens": { "type": "integer", "default": DEFAULT_TARGET_TOKENS, "description": "Target maximum tokens per chunk; increase above the largest in-scope file token count when deterministic input would otherwise fail closed" },
                    "chunk": { "type": "integer", "description": "1-based chunk number to return with full text" }
                }
            }),
        ),
        tool_def(
            "git_diff_input",
            "Prepare deterministic review input from git diff against a target branch/commit/ref.",
            json!({
                "type": "object",
                "required": ["target"],
                "properties": {
                    "target": { "type": "string" },
                    "model": { "type": "string", "description": "Model label retained for workflow metadata; token estimates use oy's approximate byte heuristic" },
                    "target_tokens": { "type": "integer", "default": DEFAULT_TARGET_TOKENS, "description": "Target maximum tokens per chunk; increase above the largest diff item token count when deterministic input would otherwise fail closed" },
                    "chunk": { "type": "integer", "description": "1-based chunk number to return with full diff text" }
                }
            }),
        ),
        tool_def(
            "existing_report",
            "Read an existing generated ISSUES.md or REVIEW.md report so a new audit/review can carry forward still-current findings and supersede stale ones.",
            json!({
                "type": "object",
                "required": ["kind"],
                "properties": {
                    "kind": { "type": "string", "enum": ["audit", "review"] },
                    "out": { "type": "string", "description": "Workspace report path to read; defaults to ISSUES.md for audit and REVIEW.md for review" }
                }
            }),
        ),
    ];

    if let Some(definition) = sloc_tool_definition() {
        tools.push(definition);
    }
    if let Some(definition) = outline_tool_definition() {
        tools.push(definition);
    }
    if let Some(definition) = sighthound_tool_definition() {
        tools.push(definition);
    }

    tools.extend([
        tool_def(
            "render_audit_report",
            "Render and write a deterministic audit report from agent-produced markdown/structured findings.",
            render_report_schema("ISSUES.md", true),
        ),
        tool_def(
            "render_review_report",
            "Render and write a deterministic review report from agent-produced markdown/structured findings.",
            render_report_schema("REVIEW.md", false),
        ),
    ]);

    tools
}

fn outline_tool_definition() -> Option<Value> {
    tools::has_external_outline_tool().then(|| {
        tool_def(
            "outline",
            "Extract structural definitions from a source file using Universal Ctags when it is installed on PATH.",
            json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" }
                }
            }),
        )
    })
}

fn sloc_tool_definition() -> Option<Value> {
    tools::has_external_sloc_counter().then(|| {
        tool_def(
            "sloc",
            "Count source lines by language using tokei when it is installed on PATH.",
            json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" },
                    "exclude": {
                        "oneOf": [
                            { "type": "string" },
                            { "type": "array", "items": { "type": "string" } }
                        ]
                    }
                }
            }),
        )
    })
}

fn sighthound_tool_definition() -> Option<Value> {
    tools::has_external_security_scanner().then(|| {
        tool_def(
            "sighthound",
            "Scan a workspace directory for source vulnerabilities with Sighthound's embedded rules. Uses independent gitignore-aware discovery that may include supported source excluded by oy's collector. Runs single-threaded with fixed arguments and bounded output.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "default": ".", "description": "Workspace-relative directory to scan" },
                    "analysis": { "type": "string", "enum": ["all", "simple", "taint"], "default": "all" },
                    "language": {
                        "type": "string",
                        "enum": ["python", "javascript", "typescript", "tsx", "java", "php", "csharp", "go", "ruby", "html", "django"],
                        "description": "Optional explicit scan language; omit to auto-detect supported languages"
                    },
                    "include_test_fixtures": { "type": "boolean", "default": false },
                    "max_findings": { "type": "integer", "minimum": 1, "maximum": 200, "default": 100, "description": "Maximum findings returned after stable sorting; output also has a byte budget" }
                }
            }),
        )
    })
}

fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": input_schema })
}

fn render_report_schema(default_out: &str, sarif: bool) -> Value {
    let format = if sarif {
        json!({ "type": "string", "enum": ["markdown", "sarif"], "default": "markdown" })
    } else {
        json!({ "type": "string", "enum": ["markdown"], "default": "markdown" })
    };
    json!({
        "type": "object",
        "properties": {
            "report": { "type": "string", "description": "Markdown report body; renderer owns transparency and oy-findings block" },
            "findings": { "description": "Structured findings array, [] for no findings, or object with findings/oy-findings" },
            "out": { "type": "string", "default": default_out },
            "format": format,
            "model": { "type": "string", "description": "Model used for the audit/review, included in the transparency line" },
            "target": { "type": "string", "description": "Review target branch/commit/ref, included in review transparency" },
            "focus": { "type": "string", "description": "Focus text included in the transparency line" },
            "max_chunks": { "type": "integer", "description": "Max chunk limit included in the transparency line" }
        }
    })
}

#[derive(Debug, Deserialize)]
struct RepoInputArgs {
    #[serde(default = "default_path")]
    path: String,
    #[serde(default = "default_model")]
    model: String,
    #[serde(default = "default_true")]
    security_index: bool,
    #[serde(default = "default_security_index_limit")]
    security_index_limit: usize,
}

#[derive(Debug, Deserialize)]
struct ChunkArgs {
    #[serde(default = "default_path")]
    path: String,
    #[serde(default = "default_model")]
    model: String,
    #[serde(default = "default_target_tokens")]
    target_tokens: usize,
    #[serde(default)]
    chunk: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct DiffArgs {
    target: String,
    #[serde(default = "default_model")]
    model: String,
    #[serde(default = "default_target_tokens")]
    target_tokens: usize,
    #[serde(default)]
    chunk: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ExistingReportArgs {
    kind: String,
    #[serde(default)]
    out: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct RenderReportArgs {
    #[serde(default)]
    report: Option<String>,
    #[serde(default)]
    findings: Option<Value>,
    #[serde(default)]
    out: Option<PathBuf>,
    #[serde(default = "default_markdown")]
    format: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    focus: Option<String>,
    #[serde(default)]
    max_chunks: Option<usize>,
}

fn repo_manifest(args: Value) -> Result<Value> {
    let args: RepoInputArgs = parse_args(args)?;
    let root = workspace_root()?;
    let files = collect_workspace_input_files(&root, &args.path, &args.model)?;
    if files.is_empty() {
        bail!("no reviewable text files found");
    }
    let manifest = input::build_manifest(&files);
    let security_index = args
        .security_index
        .then(|| input::build_security_index(&files, args.security_index_limit));
    Ok(json!({
        "path": args.path,
        "manifest": manifest,
        "security_index": security_index,
        "files": file_summaries(&files),
    }))
}

fn repo_chunks(args: Value) -> Result<Value> {
    let args: ChunkArgs = parse_args(args)?;
    let root = workspace_root()?;
    let files = collect_workspace_input_files(&root, &args.path, &args.model)?;
    if files.is_empty() {
        bail!("no reviewable text files found");
    }
    let manifest = input::build_manifest(&files);
    let chunks = input::chunk_files(files, args.target_tokens.max(1));
    input::ensure_chunks_fit_prompt(&chunks, args.target_tokens.max(1))?;
    chunks_response("workspace", manifest, chunks, args.chunk)
}

fn git_diff_input(args: Value) -> Result<Value> {
    let args: DiffArgs = parse_args(args)?;
    validate_target_ref(&args.target)?;
    let root = workspace_root()?;
    let _ = git_output(&root, &["rev-parse", "--show-toplevel"])
        .context("git_diff_input requires a git workspace")?;
    let diff = git_output(
        &root,
        &[
            "diff",
            "--no-ext-diff",
            "--find-renames",
            "--find-copies",
            "--unified=80",
            &args.target,
            "--",
        ],
    )
    .with_context(|| format!("failed to collect git diff against {}", args.target))?;
    if diff.trim().is_empty() {
        bail!("no git diff found against target {}", args.target);
    }
    let stats = input::parse_numstat(&git_output(
        &root,
        &["diff", "--numstat", &args.target, "--"],
    )?);
    let files = input::collect_diff_files(&diff, &args.model);
    if files.is_empty() {
        bail!(
            "no reviewable text diff found against target {}",
            args.target
        );
    }
    let manifest = input::build_diff_manifest(&args.target, &files, &stats);
    let chunks = input::chunk_files(files, args.target_tokens.max(1));
    input::ensure_chunks_fit_prompt(&chunks, args.target_tokens.max(1))?;
    chunks_response(
        &format!("git diff against {}", args.target),
        manifest,
        chunks,
        args.chunk,
    )
}

fn existing_report(args: Value) -> Result<Value> {
    let args: ExistingReportArgs = parse_args(args)?;
    existing_report_at(&workspace_root()?, args)
}

fn existing_report_at(root: &Path, args: ExistingReportArgs) -> Result<Value> {
    let (kind, default_out) = match args.kind.as_str() {
        "audit" => (
            "audit",
            audit::default_output_path(audit::AuditOutputFormat::Markdown),
        ),
        "review" => ("review", crate::review::default_output_path()),
        other => bail!("unsupported existing report kind: {other}"),
    };
    let out = args.out.unwrap_or(default_out);
    let output_path = config::resolve_workspace_output_path(root, &out)?;
    if !output_path.exists() {
        return Ok(json!({
            "kind": kind,
            "path": out,
            "exists": false,
            "report": "",
            "findings": 0,
        }));
    }
    let meta = fs::metadata(&output_path)
        .with_context(|| format!("failed to stat existing report: {}", output_path.display()))?;
    if meta.len() > MAX_EXISTING_REPORT_BYTES {
        bail!(
            "existing {kind} report is too large to include deterministically ({} bytes > {}): {}",
            meta.len(),
            MAX_EXISTING_REPORT_BYTES,
            output_path.display()
        );
    }
    let report = fs::read_to_string(&output_path)
        .with_context(|| format!("failed to read existing report: {}", output_path.display()))?;
    let findings = audit::report::findings_from_report(&report).len();
    Ok(json!({
        "kind": kind,
        "path": out,
        "exists": true,
        "report": report,
        "findings": findings,
    }))
}

fn chunks_response(
    source: &str,
    manifest: String,
    chunks: Vec<input::AuditChunk>,
    requested: Option<usize>,
) -> Result<Value> {
    if let Some(number) = requested {
        let idx = number
            .checked_sub(1)
            .ok_or_else(|| anyhow!("chunk numbers are 1-based"))?;
        let chunk = chunks.get(idx).ok_or_else(|| {
            anyhow!(
                "chunk {number} not found; available chunks: {}",
                chunks.len()
            )
        })?;
        return Ok(json!({
            "source": source,
            "manifest": manifest,
            "chunk": number,
            "chunk_count": chunks.len(),
            "tokens": chunk.tokens,
            "files": file_summaries(&chunk.files),
            "text": input::chunk_text(chunk),
        }));
    }
    Ok(json!({
        "source": source,
        "manifest": manifest,
        "chunk_count": chunks.len(),
        "chunks": chunks.iter().enumerate().map(|(idx, chunk)| json!({
            "chunk": idx + 1,
            "tokens": chunk.tokens,
            "files": file_summaries(&chunk.files),
        })).collect::<Vec<_>>()
    }))
}

fn render_audit_report(args: Value) -> Result<Value> {
    let args: RenderReportArgs = parse_args(args)?;
    let format = format_arg(&args.format)?;
    let out = args
        .out
        .unwrap_or_else(|| audit::default_output_path(format));
    let root = workspace_root()?;
    let output_path = config::resolve_workspace_output_path(&root, &out)?;
    let findings_payload = findings_payload(args.findings.as_ref(), "audit")?;
    let report = report_body(args.report, "# Audit Issues", findings_payload.as_deref())?;
    let report = with_machine_readable_findings(report, findings_payload.as_deref(), "audit");
    let report = audit::report::with_audit_transparency_line(
        &report,
        &audit::report::audit_transparency_snippet(
            args.model.as_deref(),
            args.focus.as_deref(),
            &out,
            args.max_chunks,
            format,
        ),
    );
    let finding_count = audit::report::findings_from_report(&report).len();
    let output = match args.format.as_str() {
        "markdown" => audit::report::with_succinct_findings_summary(&report),
        "sarif" => audit::report::render_sarif(&report)?,
        other => bail!("unsupported audit report format: {other}"),
    };
    config::write_workspace_file(&output_path, output.as_bytes())?;
    Ok(json!({
        "output": output_path,
        "format": args.format,
        "findings": finding_count,
    }))
}

fn render_review_report(args: Value) -> Result<Value> {
    let args: RenderReportArgs = parse_args(args)?;
    let root = workspace_root()?;
    render_review_report_at(&root, args)
}

fn render_review_report_at(root: &Path, mut args: RenderReportArgs) -> Result<Value> {
    if args.format != "markdown" {
        bail!("review reports support markdown only");
    }
    let out = args
        .out
        .take()
        .unwrap_or_else(crate::review::default_output_path);
    let output_path = config::resolve_workspace_output_path(root, &out)?;
    let findings_payload = findings_payload(args.findings.as_ref(), "review")?;
    let report = report_body(
        args.report,
        "# Code Quality Review",
        findings_payload.as_deref(),
    )?;
    let report = with_machine_readable_findings(report, findings_payload.as_deref(), "review");
    let report = audit::report::with_review_transparency_line(
        &report,
        &audit::report::review_transparency_snippet(
            args.model.as_deref(),
            args.target.as_deref(),
            args.focus.as_deref(),
            &out,
            args.max_chunks,
        ),
    );
    let output = report;
    config::write_workspace_file(&output_path, output.as_bytes())?;
    Ok(json!({
        "output": output_path,
        "format": "markdown",
        "findings": audit::report::findings_from_report(&output).len(),
    }))
}

fn report_body(
    report: Option<String>,
    title: &str,
    findings_payload: Option<&str>,
) -> Result<String> {
    if let Some(report) = report.filter(|report| !report.trim().is_empty()) {
        return Ok(report);
    }
    let payload = findings_payload.ok_or_else(|| anyhow!("report or findings is required"))?;
    Ok(format!(
        "{title}\n\n## Machine-readable findings\n\n```json oy-findings\n{payload}\n```\n"
    ))
}

fn findings_payload(findings: Option<&Value>, fallback_source: &str) -> Result<Option<String>> {
    findings
        .map(|findings| {
            audit::report::normalized_findings_payload(findings, fallback_source).ok_or_else(|| {
                anyhow!(
                    "findings must be a JSON array or an object with a findings/oy-findings array"
                )
            })
        })
        .transpose()
}

fn with_machine_readable_findings(
    report: String,
    findings_payload: Option<&str>,
    source: &str,
) -> String {
    if let Some(payload) = findings_payload {
        audit::report::with_structured_findings_payload(&report, payload)
    } else {
        audit::report::with_structured_findings_block(&report, source)
    }
}

fn file_summaries(files: &[input::AuditFile]) -> Vec<Value> {
    files
        .iter()
        .map(|file| {
            json!({
                "path": file.path,
                "language": file.language,
                "bytes": file.bytes,
                "tokens": file.tokens,
            })
        })
        .collect()
}

fn workspace_root() -> Result<PathBuf> {
    config::oy_root()
}

fn collect_workspace_input_files(
    root: &Path,
    path: &str,
    model: &str,
) -> Result<Vec<input::AuditFile>> {
    if let Some(key) = clean_repo_cache_key(root, path, model) {
        if let Some(files) = CLEAN_REPO_INPUT_CACHE.lock().unwrap().get(&key).cloned() {
            return Ok(files);
        }
        let files = collect_workspace_input_files_uncached(root, path, model)?;
        CLEAN_REPO_INPUT_CACHE
            .lock()
            .unwrap()
            .insert(key, files.clone());
        return Ok(files);
    }
    collect_workspace_input_files_uncached(root, path, model)
}

fn collect_workspace_input_files_uncached(
    root: &Path,
    path: &str,
    model: &str,
) -> Result<Vec<input::AuditFile>> {
    let resolved = resolve_workspace_path(root, path)?;
    if resolved.is_dir() {
        let prefix = resolved
            .strip_prefix(
                root.canonicalize()
                    .context("failed to resolve workspace root")?,
            )
            .context("path escapes workspace")?
            .to_string_lossy()
            .replace('\\', "/");
        let prefix = prefix.trim_matches('/').to_string();
        let prefix_with_sep = format!("{prefix}/");
        return Ok(input::collect_files(root, None, model)?
            .into_iter()
            .filter(|file| {
                prefix.is_empty()
                    || file.path == prefix
                    || file.path.starts_with(prefix_with_sep.as_str())
            })
            .collect());
    }
    if !resolved.is_file() {
        bail!("path is not a file or directory: {path}");
    }
    Ok(input::collect_file(root, &resolved, model)?
        .into_iter()
        .collect())
}

fn clean_repo_cache_key(root: &Path, path: &str, model: &str) -> Option<String> {
    let head = git_output(root, &["rev-parse", "HEAD"]).ok()?;
    let status = git_output(
        root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )
    .ok()?;
    if !status.trim().is_empty() {
        return None;
    }
    Some(format!(
        "{}\0{}\0{}\0{}",
        root.canonicalize().ok()?.display(),
        head.trim(),
        path,
        model
    ))
}

fn resolve_workspace_path(root: &Path, path: &str) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .context("failed to resolve workspace root")?;
    let raw = Path::new(path);
    if raw
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("path must stay inside workspace: {path}");
    }
    if !raw.is_absolute()
        && raw
            .components()
            .any(|component| matches!(component, Component::Prefix(_)))
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
    if target.trim().is_empty() {
        bail!("target cannot be empty");
    }
    if target.starts_with('-') {
        bail!("target must be a branch/commit/ref, not an option-like value");
    }
    if target.contains('\0') || target.contains('\n') || target.contains('\r') {
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim().lines().next().unwrap_or("unknown git error")
        );
    }
    String::from_utf8(output.stdout).context("git output was not UTF-8")
}

fn parse_args<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T> {
    serde_json::from_value(value).context("invalid tool arguments")
}

fn format_arg(format: &str) -> Result<audit::AuditOutputFormat> {
    match format {
        "markdown" => Ok(audit::AuditOutputFormat::Markdown),
        "sarif" => Ok(audit::AuditOutputFormat::Sarif),
        other => bail!("unsupported report format: {other}"),
    }
}

fn default_path() -> String {
    ".".to_string()
}

fn default_model() -> String {
    DEFAULT_MODEL_FOR_COUNTING.to_string()
}

fn default_true() -> bool {
    true
}

fn default_security_index_limit() -> usize {
    120
}

fn default_target_tokens() -> usize {
    DEFAULT_TARGET_TOKENS
}

fn default_markdown() -> String {
    "markdown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENTED_TOOL_NAMES: &[&str] = &[
        "repo_manifest",
        "repo_chunks",
        "git_diff_input",
        "existing_report",
        "sloc",
        "outline",
        "sighthound",
        "render_audit_report",
        "render_review_report",
    ];

    #[tokio::test]
    async fn initialize_advertises_protocol_and_server_version() {
        let response = handle_request("initialize", Value::Null)
            .await
            .unwrap()
            .into_json(json!(1));

        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(response["result"]["serverInfo"]["name"], "oy");
        assert_eq!(
            response["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert!(response["result"]["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn tools_list_matches_inventory_and_reference() {
        let response = handle_request("tools/list", Value::Null)
            .await
            .unwrap()
            .into_json(json!(2));
        let names = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        let mut expected = vec![
            "repo_manifest",
            "repo_chunks",
            "git_diff_input",
            "existing_report",
        ];
        if crate::tools::has_external_sloc_counter() {
            expected.push("sloc");
        }
        if crate::tools::has_external_outline_tool() {
            expected.push("outline");
        }
        if crate::tools::has_external_security_scanner() {
            expected.push("sighthound");
        }
        expected.extend(["render_audit_report", "render_review_report"]);

        assert_eq!(names, expected);
        let reference = include_str!("../docs/reference.md");
        for name in DOCUMENTED_TOOL_NAMES {
            assert!(
                reference.contains(&format!("`{name}`")),
                "docs/reference.md is missing the `{name}` MCP tool"
            );
        }
    }

    #[tokio::test]
    async fn unknown_method_returns_top_level_jsonrpc_error() {
        let response = handle_request("missing/method", Value::Null)
            .await
            .unwrap()
            .into_json(json!(7));

        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 7);
        assert!(response.get("result").is_none());
        assert_eq!(response["error"]["code"], -32601);
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("unknown MCP method: missing/method")
        );
    }

    #[test]
    fn sloc_tool_is_listed_only_when_tokei_is_available() {
        let tools = tool_definitions();
        let has_sloc = tools.iter().any(|tool| tool["name"] == "sloc");

        assert_eq!(has_sloc, crate::tools::has_external_sloc_counter());
    }

    #[test]
    fn outline_tool_is_listed_only_when_ctags_is_available() {
        let tools = tool_definitions();
        let has_outline = tools.iter().any(|tool| tool["name"] == "outline");

        assert_eq!(has_outline, crate::tools::has_external_outline_tool());
    }

    #[test]
    fn sighthound_tool_is_listed_only_when_scanner_is_available() {
        let tools = tool_definitions();
        let has_sighthound = tools.iter().any(|tool| tool["name"] == "sighthound");

        assert_eq!(
            has_sighthound,
            crate::tools::has_external_security_scanner()
        );
    }

    #[test]
    fn existing_report_tool_is_listed() {
        let tools = tool_definitions();

        assert!(tools.iter().any(|tool| tool["name"] == "existing_report"));
    }

    #[test]
    fn mcp_workspace_path_accepts_absolute_path_inside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("src");
        std::fs::create_dir(&nested).unwrap();

        let resolved = resolve_workspace_path(dir.path(), nested.to_str().unwrap()).unwrap();

        assert_eq!(resolved, nested.canonicalize().unwrap());
    }

    #[test]
    fn mcp_workspace_path_rejects_absolute_path_outside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        let err = resolve_workspace_path(dir.path(), outside.path().to_str().unwrap()).unwrap_err();

        assert!(err.to_string().contains("path escapes workspace"));
    }

    #[test]
    fn repo_input_accepts_file_path_and_preserves_workspace_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("src");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("lib.rs"), "fn main() {}\n").unwrap();

        let files = collect_workspace_input_files(dir.path(), "src/lib.rs", "cl100k_base").unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/lib.rs");
    }

    #[test]
    fn repo_input_accepts_directory_path_and_preserves_workspace_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("src");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("lib.rs"), "fn lib() {}\n").unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();

        let files = collect_workspace_input_files(dir.path(), "src", "cl100k_base").unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/lib.rs");
    }

    #[test]
    fn existing_report_reads_default_audit_report() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ISSUES.md"),
            "# Audit Issues\n\n### High: stale but structured\n\nEvidence: src/lib.rs:1\n",
        )
        .unwrap();

        let report = existing_report_at(
            dir.path(),
            ExistingReportArgs {
                kind: "audit".to_string(),
                out: None,
            },
        )
        .unwrap();

        assert_eq!(report["exists"], true);
        assert_eq!(report["findings"], 1);
        assert!(
            report["report"]
                .as_str()
                .unwrap()
                .contains("stale but structured")
        );
    }

    #[test]
    fn existing_report_returns_absent_default_review_report() {
        let dir = tempfile::tempdir().unwrap();

        let report = existing_report_at(
            dir.path(),
            ExistingReportArgs {
                kind: "review".to_string(),
                out: None,
            },
        )
        .unwrap();

        assert_eq!(report["exists"], false);
        assert_eq!(report["path"], "REVIEW.md");
        assert_eq!(report["report"], "");
    }

    #[test]
    fn render_review_report_adds_empty_findings_block_for_no_findings() {
        let dir = tempfile::tempdir().unwrap();

        let args = parse_args(json!({
            "report": "# Code Quality Review\n\n## Verdict\n\nNo major structural concerns.\n",
            "out": "REVIEW.md",
            "model": "openai/gpt-5.5"
        }))
        .unwrap();
        let result = render_review_report_at(dir.path(), args).unwrap();

        let report = std::fs::read_to_string(dir.path().join("REVIEW.md")).unwrap();
        assert_eq!(result["findings"], 0);
        assert!(report.contains("OY_MODEL=openai/gpt-5.5 oy review"));
        assert!(report.contains("```json oy-findings\n[]\n```"));
    }

    #[test]
    fn render_review_report_uses_supplied_findings_with_markdown_report() {
        let dir = tempfile::tempdir().unwrap();

        let args = parse_args(json!({
            "report": "# Code Quality Review\n\n## Verdict\n\nNeeds work.\n\n## Machine-readable findings\n\n```json oy-findings\nnot json\n```\n",
            "findings": [{
                "severity": "Medium",
                "title": "diff keeps duplicate source of truth",
                "locations": [{ "path": "src/lib.rs", "line": 7 }],
                "evidence": "src/lib.rs:7 duplicates the option list",
                "body": "Use one canonical option list."
            }],
            "out": "REVIEW.md"
        }))
        .unwrap();
        let result = render_review_report_at(dir.path(), args).unwrap();

        let report = std::fs::read_to_string(dir.path().join("REVIEW.md")).unwrap();
        assert_eq!(result["findings"], 1);
        assert_eq!(report.matches("```json oy-findings").count(), 1);
        assert!(!report.contains("not json"));
        assert!(report.contains("src/lib.rs:7 duplicates the option list"));
        assert!(report.contains("Use one canonical option list."));
    }
}
