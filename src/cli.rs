use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use ignore::WalkBuilder;
use serde::Serialize;
use std::io::{ErrorKind, IsTerminal as _, Read as _, Write as _};
use std::path::{Path, PathBuf};
use tokei::{Config as TokeiConfig, LanguageType};

use crate::agent::{self, Session};
use crate::config;
use crate::model;

const MODEL_LIST_LIMIT: usize = 30;
const DEFAULT_AUDIT_CHUNK_LINES: usize = 6000;
const MIN_AUDIT_CHUNK_LINES: usize = 500;
const DEFAULT_AUDIT_CHUNK_BYTES: usize = 768 * 1024;
const MIN_AUDIT_CHUNK_BYTES: usize = 64 * 1024;
const DEFAULT_AUDIT_FILE_BYTES: usize = 256 * 1024;
const MIN_AUDIT_FILE_BYTES: usize = 16 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "oy",
    version,
    about = "Small local AI coding assistant for your shell.",
    after_help = "Examples:\n  oy \"inspect this repo and summarize risks\"\n  oy chat --agent plan\n  oy run --out plan.md \"write a migration plan\"\n  oy model copilot::gpt-4.1-mini\n\nSafety: oy is not a sandbox. Use --agent plan or a container/VM for untrusted repos."
)]
struct Cli {
    #[arg(long, global = true, conflicts_with_all = ["verbose", "json"], help = "Suppress normal progress output")]
    quiet: bool,
    #[arg(long, global = true, conflicts_with_all = ["quiet", "json"], help = "Show fuller tool previews")]
    verbose: bool,
    #[arg(long, global = true, conflicts_with_all = ["quiet", "verbose"], help = "Print machine-readable JSON where supported")]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run one task in the current workspace; prompt can be args or stdin.
    Run(RunArgs),
    /// Start an interactive chat session with slash commands and history.
    Chat(ChatArgs),
    /// Re-run a maintenance prompt until the configured deadline.
    Ralph(RalphArgs),
    /// List, choose, and save model ids/routing shims.
    Model(ModelArgs),
    /// Check setup, auth, paths, and safety-relevant defaults.
    Doctor(DoctorArgs),
    /// Multi-pass repository audit to ISSUES.md.
    Audit(AuditArgs),
}

#[derive(Debug, Args, Clone)]
struct SharedAgentArgs {
    #[arg(
        long,
        default_value = "default",
        help = "Agent profile: default, plan, accept-edits, or auto-approve"
    )]
    agent: String,
    #[arg(
        long = "continue-session",
        default_value_t = false,
        help = "Resume the most recent saved session"
    )]
    continue_session: bool,
    #[arg(
        long,
        default_value = "",
        value_name = "NAME_OR_NUMBER",
        help = "Resume a named or numbered saved session"
    )]
    resume: String,
}

#[derive(Debug, Args, Clone)]
struct RunArgs {
    #[command(flatten)]
    shared: SharedAgentArgs,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write the final answer to a workspace file"
    )]
    out: Option<PathBuf>,
    #[arg(
        value_name = "PROMPT",
        help = "Task prompt; omitted means read stdin or start chat in a TTY"
    )]
    task: Vec<String>,
}

#[derive(Debug, Args, Clone)]
struct ChatArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Approve file edits and shell commands for this session; high risk"
    )]
    yolo: bool,
    #[command(flatten)]
    shared: SharedAgentArgs,
}

#[derive(Debug, Args, Clone)]
struct RalphArgs {
    #[arg(long, default_value = "default", help = "Agent profile to use")]
    agent: String,
    #[arg(value_name = "PROMPT", help = "Maintenance prompt to repeat")]
    task: Vec<String>,
}

#[derive(Debug, Args, Clone)]
struct ModelArgs {
    #[arg(
        value_name = "MODEL",
        help = "Model id or routing shim selection, e.g. copilot::gpt-4.1-mini"
    )]
    model: Option<String>,
}

#[derive(Debug, Args, Clone)]
struct DoctorArgs {
    #[arg(long, default_value = "default", help = "Agent profile to inspect")]
    agent: String,
}

#[derive(Debug, Args, Clone)]
struct AuditArgs {
    #[arg(value_name = "FOCUS", help = "Optional audit focus text")]
    focus: Vec<String>,
    #[arg(long, default_value_t = DEFAULT_AUDIT_CHUNK_LINES, help = "Approximate maximum source lines per audit chunk")]
    chunk_lines: usize,
    #[arg(long, default_value_t = DEFAULT_AUDIT_CHUNK_BYTES, help = "Approximate maximum source bytes per audit chunk")]
    chunk_bytes: usize,
    #[arg(long, default_value_t = DEFAULT_AUDIT_FILE_BYTES, help = "Maximum source bytes read per audit file")]
    file_bytes: usize,
    #[arg(
        long,
        default_value_t = false,
        help = "Include standards context where useful"
    )]
    standards: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Focus on executable/runtime logic; omit docs/comments"
    )]
    logic_only: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Plan chunks and print/write the manifest without model calls or ISSUES.md changes"
    )]
    dry_run: bool,
}

pub async fn run(argv: Vec<String>) -> Result<i32> {
    let normalized = normalize_args(argv);
    let cli = Cli::parse_from(std::iter::once("oy".to_string()).chain(normalized.clone()));
    crate::ui::init_output_mode(cli_output_mode(&cli));
    match cli.command.unwrap_or(Command::Run(RunArgs {
        shared: SharedAgentArgs {
            agent: "default".to_string(),
            continue_session: false,
            resume: String::new(),
        },
        out: None,
        task: Vec::new(),
    })) {
        Command::Run(args) => run_command(args).await,
        Command::Chat(args) => chat_command(args).await,
        Command::Ralph(args) => ralph_command(args).await,
        Command::Model(args) => model_command(args).await,
        Command::Doctor(args) => doctor_command(args).await,
        Command::Audit(args) => audit_command(args).await,
    }
}

fn cli_output_mode(cli: &Cli) -> Option<crate::ui::OutputMode> {
    if cli.quiet {
        Some(crate::ui::OutputMode::Quiet)
    } else if cli.verbose {
        Some(crate::ui::OutputMode::Verbose)
    } else if cli.json {
        Some(crate::ui::OutputMode::Json)
    } else {
        None
    }
}

fn normalize_args(mut args: Vec<String>) -> Vec<String> {
    if args.is_empty() {
        return if config::can_prompt() {
            vec!["--help".to_string()]
        } else {
            vec!["run".to_string()]
        };
    }
    if matches!(
        args.first().map(String::as_str),
        Some("--continue") | Some("-c")
    ) {
        return std::iter::once("run".to_string())
            .chain(std::iter::once("--continue-session".to_string()))
            .chain(args.drain(1..))
            .collect();
    }
    if args.first().map(String::as_str) == Some("--resume") {
        return std::iter::once("run".to_string()).chain(args).collect();
    }
    let commands = [
        "run", "chat", "ralph", "model", "doctor", "audit", "-h", "--help",
    ];
    if args
        .first()
        .is_some_and(|arg| !arg.starts_with('-') && !commands.contains(&arg.as_str()))
    {
        let mut out = vec!["run".to_string()];
        out.extend(args);
        return out;
    }
    args
}

async fn run_command(args: RunArgs) -> Result<i32> {
    let task = collect_task(&args.task)?;
    if task.trim().is_empty() {
        return chat_command(ChatArgs {
            yolo: false,
            shared: args.shared,
        })
        .await;
    }
    let mut session = load_or_new(
        false,
        &args.shared.agent,
        args.shared.continue_session,
        &args.shared.resume,
    )?;
    print_session_intro("run", &session, Some(&task));
    let answer = agent::run_prompt(&mut session, &task).await?;
    if crate::ui::is_json() {
        print_run_json(&session, &answer)?;
    } else if let Some(path) = args.out {
        write_workspace_file(&session.root, &path, &answer)?;
        crate::ui::success(format_args!("wrote {}", path.display()));
    } else if !answer.is_empty() {
        crate::ui::markdown(&format!("{answer}\n"));
    }
    Ok(0)
}

fn print_run_json(session: &Session, answer: &str) -> Result<()> {
    let status = session.context_status();
    let payload = serde_json::json!({
        "answer": answer,
        "model": session.model,
        "agent": session.agent,
        "workspace": session.root,
        "tokens": status.estimate,
        "context": status,
        "messages": status.estimate.messages,
        "todos": session.todos,
    });
    crate::ui::line(serde_json::to_string_pretty(&payload)?);
    Ok(())
}

async fn chat_command(args: ChatArgs) -> Result<i32> {
    let mut session = load_or_new(
        true,
        &args.shared.agent,
        args.shared.continue_session,
        &args.shared.resume,
    )?;
    if args.yolo {
        session.policy.files_write = crate::tools::Approval::Auto;
        session.policy.shell = crate::tools::Approval::Auto;
    }
    print_session_intro("chat", &session, None);
    crate::chat::run_chat(&mut session).await
}

async fn ralph_command(args: RalphArgs) -> Result<i32> {
    let task = collect_task(&args.task)?;
    if task.trim().is_empty() {
        bail!("Usage: `oy ralph <prompt>` — or pipe prompt text on stdin.");
    }
    let mut session = load_or_new(false, &args.agent, false, "")?;
    session.policy.files_write = crate::tools::Approval::Auto;
    session.policy.shell = crate::tools::Approval::Auto;
    print_session_intro("ralph", &session, Some(&task));
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(config::ralph_limit_seconds());
    let mut exit_code = 0;
    let mut run_number = 0usize;
    while std::time::Instant::now() < deadline {
        run_number += 1;
        if !crate::ui::is_quiet() {
            crate::ui::err_line(format_args!("ralph run {run_number}"));
        }
        if let Err(err) = agent::run_prompt(&mut session, &task).await {
            crate::ui::err_line(format_args!("ralph error: {err:#}"));
            exit_code = 1;
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(60).min(deadline - now)).await;
    }
    Ok(exit_code)
}

async fn model_command(args: ModelArgs) -> Result<i32> {
    let listing = model::inspect_models().await?;
    if let Some(model_spec) = args.model {
        let normalized = resolve_model_choice(&listing, &model_spec)?;
        config::save_model_config(&normalized)?;
        if crate::ui::is_json() {
            print_model_json(&listing, Some(&normalized))?;
        } else {
            print_saved_model(&normalized);
        }
        return Ok(0);
    }
    if crate::ui::is_json() {
        print_model_json(&listing, None)?;
        return Ok(0);
    }
    print_model_listing(&listing);
    if config::can_prompt() && !listing.all_models.is_empty() {
        if let Some(chosen) = crate::chat::choose_model_with_initial_list(
            listing.current.as_deref(),
            &listing.all_models,
            false,
        )? {
            config::save_model_config(&chosen)?;
            print_saved_model(&chosen);
        }
    }
    Ok(0)
}

fn print_model_json(listing: &model::ModelListing, saved: Option<&str>) -> Result<()> {
    let payload = serde_json::json!({
        "current": listing.current,
        "current_shim": listing.current_shim,
        "saved": saved,
        "auth": listing.auth,
        "dynamic": listing.dynamic,
        "hints": listing.hints,
        "all_models": listing.all_models,
    });
    crate::ui::line(serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn print_model_listing(listing: &model::ModelListing) {
    crate::ui::section("Models");
    crate::ui::kv(
        "current",
        current_model_text(
            listing.current.as_deref().unwrap_or("<unset>"),
            listing.current_shim.as_deref(),
        ),
    );
    crate::ui::kv("selectable", listing.all_models.len());

    if !listing.auth.is_empty() {
        crate::ui::line("");
        crate::ui::section("Auth / shims");
        for item in &listing.auth {
            let env_var = item.env_var.as_deref().unwrap_or("-");
            let active = if listing.current_shim.as_deref() == Some(item.adapter.as_str()) {
                " *"
            } else {
                ""
            };
            crate::ui::line(format_args!(
                "  {}{}  {} ({})",
                item.adapter, active, env_var, item.source
            ));
            crate::ui::line(format_args!("    {}", item.detail));
        }
    }

    crate::ui::line("");
    crate::ui::section("Introspected endpoint models");
    if listing.dynamic.is_empty() {
        crate::ui::line("  none found from configured OpenAI-compatible endpoints");
    } else {
        for item in &listing.dynamic {
            if !item.ok {
                crate::ui::line(format_args!(
                    "  {}  failed via {}",
                    item.adapter, item.source
                ));
                if let Some(error) = item.error.as_deref() {
                    crate::ui::line(format_args!(
                        "    {}",
                        crate::ui::truncate_chars(error, 140)
                    ));
                }
                continue;
            }
            crate::ui::line(format_args!(
                "  {}  {} models via {}",
                item.adapter, item.count, item.source
            ));
            for model_name in item.models.iter().take(MODEL_LIST_LIMIT) {
                let marker = if listing.current.as_deref() == Some(model_name.as_str()) {
                    "*"
                } else {
                    " "
                };
                crate::ui::line(format_args!("    {marker} {model_name}"));
            }
            if item.models.len() > MODEL_LIST_LIMIT {
                crate::ui::line(format_args!(
                    "    … {} more; use `oy model <filter>` or interactive selection",
                    item.models.len() - MODEL_LIST_LIMIT
                ));
            }
        }
    }

    let hinted = listing
        .hints
        .iter()
        .filter(|hint| {
            !listing
                .dynamic
                .iter()
                .any(|group| group.models.iter().any(|model| model == *hint))
        })
        .collect::<Vec<_>>();
    if !hinted.is_empty() {
        crate::ui::line("");
        crate::ui::section("Built-in selectable hints");
        for hint in hinted.iter().take(MODEL_LIST_LIMIT) {
            crate::ui::line(format_args!("  {hint}"));
        }
        if hinted.len() > MODEL_LIST_LIMIT {
            crate::ui::line(format_args!(
                "  … {} more hints",
                hinted.len() - MODEL_LIST_LIMIT
            ));
        }
    }
}

fn current_model_text(model_spec: &str, shim: Option<&str>) -> String {
    match shim.filter(|value| !value.is_empty()) {
        Some(shim) => format!("{model_spec} (shim: {shim})"),
        None => model_spec.to_string(),
    }
}

fn print_saved_model(selection: &str) {
    let saved = config::saved_model_config_from_selection(selection);
    crate::ui::success(format_args!(
        "saved model {}",
        saved.model.as_deref().unwrap_or(selection)
    ));
    if let Some(shim) = saved.shim {
        crate::ui::kv("shim", shim);
    }
}

fn resolve_model_choice(listing: &model::ModelListing, query: &str) -> Result<String> {
    let normalized = model::canonical_model_spec(query);
    if listing.all_models.iter().any(|item| item == &normalized) {
        return Ok(normalized);
    }
    if !config::can_prompt() {
        bail!(
            "No exact model match for `{}`. Re-run in a TTY to choose interactively.",
            query
        );
    }
    let matches = listing
        .all_models
        .iter()
        .filter(|item| {
            item.to_ascii_lowercase()
                .contains(&query.to_ascii_lowercase())
        })
        .cloned()
        .collect::<Vec<_>>();
    if matches.is_empty() {
        bail!("No matching model for `{}`", query);
    }
    crate::chat::choose_model(listing.current.as_deref(), &matches)
        .map(|value| value.unwrap_or(normalized))
}

async fn doctor_command(args: DoctorArgs) -> Result<i32> {
    let root = config::oy_root()?;
    let listing = model::inspect_models().await?;
    let profile = config::agent_profile(&args.agent)?;
    let policy = config::tool_policy(&profile.name);
    let config_file = config::config_root();
    let config_dir = config::config_dir_path();
    let sessions_dir = config::sessions_dir().unwrap_or_else(|_| config_dir.join("sessions"));
    let history_dir = config_dir.join("history");
    let bash_ok = std::process::Command::new("bash")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);

    if crate::ui::is_json() {
        let payload = serde_json::json!({
            "workspace": root,
            "model": listing.current,
            "shim": listing.current_shim,
            "auth": listing.auth,
            "agent": profile.name,
            "policy": policy,
            "interactive": config::can_prompt(),
            "non_interactive": config::non_interactive(),
            "config_file": config_file,
            "config_dir": config_dir,
            "sessions_dir": sessions_dir,
            "history_dir": history_dir,
            "bash": bash_ok,
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
        return Ok(0);
    }

    crate::ui::section("Doctor");
    crate::ui::kv("workspace", root.display());
    crate::ui::kv("model", listing.current.as_deref().unwrap_or("<unset>"));
    crate::ui::kv("shim", listing.current_shim.as_deref().unwrap_or("<none>"));
    crate::ui::kv("agent", &profile.name);
    crate::ui::kv("files-write", format_args!("{:?}", policy.files_write));
    crate::ui::kv("shell", format_args!("{:?}", policy.shell));
    crate::ui::kv("network", policy.network);
    crate::ui::kv("risk", policy_risk_label(&policy));
    crate::ui::kv("interactive", config::can_prompt());
    crate::ui::kv("bash", if bash_ok { "ok" } else { "missing" });
    crate::ui::line("");
    crate::ui::section("Local state");
    crate::ui::kv("config", config_file.display());
    crate::ui::kv("sessions", sessions_dir.display());
    crate::ui::kv("history", history_dir.display());
    crate::ui::line(
        "  Treat local state as sensitive: prompts, source snippets, tool output, and command output may be saved.",
    );
    crate::ui::line("");
    crate::ui::section("Auth / shims");
    if listing.auth.is_empty() {
        crate::ui::warn("no provider auth detected");
    } else {
        for item in &listing.auth {
            crate::ui::line(format_args!(
                "  {}  {} ({})",
                item.adapter,
                item.env_var.as_deref().unwrap_or("-"),
                item.source
            ));
            crate::ui::line(format_args!("    {}", item.detail));
        }
    }
    if listing.current.is_none() {
        crate::ui::line("");
        crate::ui::warn("no model configured");
        crate::ui::line("  Try: oy model copilot::gpt-4.1-mini");
        crate::ui::line("  Or:  OPENAI_API_KEY=... oy model gpt-4.1-mini");
        crate::ui::line("  Or:  oy model local-8080::qwen3.5");
    }
    crate::ui::line("");
    crate::ui::section("Recommended next steps");
    if listing.current.is_none() {
        crate::ui::line(
            "  1. Configure a model with `oy model copilot::gpt-4.1-mini` or a local shim.",
        );
    }
    crate::ui::line("  • For untrusted repos: `oy chat --agent plan`");
    crate::ui::line(format_args!(
        "  • Read-only container: {}",
        safe_container_command(&root, true)
    ));
    crate::ui::line("");
    crate::ui::section("Safety");
    crate::ui::line(
        "  oy is not a sandbox. Use `oy chat --agent plan` or a disposable container/VM for untrusted repos.",
    );
    crate::ui::line(
        "  Mount only needed credentials/env vars. Do not mount the host Docker socket into AI-assisted containers.",
    );
    crate::ui::line(
        "  Ralph is intentionally unattended and auto-approves edits and shell commands; use it only in trusted workspaces.",
    );
    Ok(0)
}

fn policy_risk_label(policy: &crate::tools::ToolPolicy) -> &'static str {
    use crate::tools::Approval;
    if policy.read_only {
        "read-only"
    } else if policy.shell == Approval::Auto {
        "high: auto shell"
    } else if policy.files_write == Approval::Auto {
        "medium: auto edits"
    } else {
        "normal: asks before edits/shell"
    }
}

fn safe_container_command(root: &Path, read_only: bool) -> String {
    let mode = if read_only { "ro" } else { "rw" };
    format!(
        "docker run --rm -it -v \"{}:/workspace:{mode}\" -w /workspace oy-image oy chat --agent plan",
        root.display()
    )
}

async fn audit_command(args: AuditArgs) -> Result<i32> {
    let root = config::oy_root()?;
    let focus = args.focus.join(" ");
    let mode = if args.logic_only {
        AuditMode::LogicOnly
    } else {
        AuditMode::Full
    };

    let chunk_lines = args.chunk_lines.max(MIN_AUDIT_CHUNK_LINES);
    let file_bytes = args.file_bytes.max(MIN_AUDIT_FILE_BYTES);
    let chunk_bytes = args.chunk_bytes.max(MIN_AUDIT_CHUNK_BYTES).max(file_bytes);
    let sloc = crate::tools::compact_workspace_snapshot(&root).unwrap_or_default();
    let docs = if mode == AuditMode::LogicOnly {
        String::new()
    } else {
        audit_docs(&root)?
    };
    let files = workspace_audit_files(&root, mode, file_bytes)?;
    let chunks = audit_chunks(files, chunk_lines, chunk_bytes);
    validate_audit_coverage(&chunks)?;
    let manifest = audit_manifest(mode, &chunks, chunk_lines, chunk_bytes, file_bytes);
    let manifest_rel = Path::new("ISSUES.audit-manifest.json");
    let manifest_path = resolve_workspace_output_path(&root, manifest_rel)?;
    write_workspace_file(
        &root,
        manifest_rel,
        &serde_json::to_string_pretty(&manifest)?,
    )?;

    if args.dry_run {
        if crate::ui::is_json() {
            let payload = serde_json::json!({
                "dry_run": true,
                "workspace": root,
                "focus": focus,
                "manifest_path": manifest_path,
                "manifest": manifest,
            });
            crate::ui::line(serde_json::to_string_pretty(&payload)?);
        } else if !crate::ui::is_quiet() {
            crate::ui::section("audit dry-run");
            crate::ui::kv("workspace", root.display());
            print_audit_plan(mode, &manifest, chunk_bytes, file_bytes, None);
            crate::ui::success("wrote ISSUES.audit-manifest.json");
            crate::ui::line("No model calls made and ISSUES.md left unchanged.");
        }
        return Ok(0);
    }

    let model = model::resolve_model(None)?;
    let session = Session::new(
        root.clone(),
        model,
        false,
        "plan".to_string(),
        config::tool_policy("plan"),
    );
    print_session_intro(
        "audit",
        &session,
        (!focus.is_empty()).then_some(focus.as_str()),
    );
    let draft_path = resolve_workspace_output_path(&root, Path::new("ISSUES.draft.md"))?;
    if !crate::ui::is_quiet() {
        print_audit_plan(
            mode,
            &manifest,
            chunk_bytes,
            file_bytes,
            Some("ISSUES.draft.md"),
        );
    }
    write_workspace_file(
        &root,
        Path::new("ISSUES.draft.md"),
        &format!(
            "# Audit draft\n\n{sloc}\n\n{} files in {} chunks planned.\nManifest: ISSUES.audit-manifest.json\n\n",
            manifest.file_count,
            chunks.len()
        ),
    )?;

    for (idx, chunk) in chunks.iter().enumerate() {
        crate::ui::section(&format!("audit chunk {}/{}", idx + 1, chunks.len()));
        let prompt =
            build_audit_chunk_prompt(&session, &focus, &sloc, &docs, chunk, args.standards, mode)?;
        let mut chunk_session = Session::new(
            root.clone(),
            session.model.clone(),
            false,
            "plan".to_string(),
            config::tool_policy("plan"),
        );
        let findings = agent::run_prompt(&mut chunk_session, &prompt).await?;
        append_audit_section(
            &draft_path,
            &format!("Chunk {}: {}", idx + 1, chunk.label),
            &findings,
        )?;
    }

    crate::ui::section("audit final reduction");
    let draft = std::fs::read_to_string(&draft_path).unwrap_or_default();
    let final_prompt = build_audit_final_prompt(&sloc, &docs, &draft, mode)?;
    let mut final_session = Session::new(
        root.clone(),
        session.model.clone(),
        false,
        "plan".to_string(),
        config::tool_policy("plan"),
    );
    let final_report = agent::run_prompt(&mut final_session, &final_prompt).await?;
    let final_report = format!(
        "{}\n\n{}",
        audit_transparency_line(
            &session,
            mode,
            chunk_lines,
            chunk_bytes,
            file_bytes,
            args.standards,
            &manifest,
        ),
        final_report.trim_start()
    );
    write_workspace_file(&root, Path::new("ISSUES.md"), &final_report)?;
    cleanup_audit_temp_files(&[&draft_path, &manifest_path])?;
    crate::ui::success("wrote ISSUES.md");
    Ok(0)
}

fn print_audit_plan(
    mode: AuditMode,
    manifest: &AuditManifest,
    chunk_bytes: usize,
    file_bytes: usize,
    draft: Option<&str>,
) {
    crate::ui::kv("mode", mode.label());
    crate::ui::kv("files", manifest.file_count);
    crate::ui::kv("chunks", manifest.chunk_count);
    crate::ui::kv("chunk-bytes", chunk_bytes);
    crate::ui::kv("file-bytes", file_bytes);
    if let Some(draft) = draft {
        crate::ui::kv("draft", draft);
    }
    crate::ui::kv("manifest", "ISSUES.audit-manifest.json");
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum AuditMode {
    Full,
    LogicOnly,
}

impl AuditMode {
    fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::LogicOnly => "logic-only",
        }
    }
}

#[derive(Debug, Clone)]
struct AuditFile {
    path: String,
    language: String,
    lines: usize,
    bytes: usize,
    original_bytes: usize,
    truncated: bool,
    text: String,
}

#[derive(Debug, Clone)]
struct AuditChunk {
    label: String,
    files: Vec<AuditFile>,
    lines: usize,
    bytes: usize,
}

#[derive(Debug, Serialize)]
struct AuditManifest {
    mode: AuditMode,
    file_count: usize,
    chunk_count: usize,
    total_lines: usize,
    total_bytes: usize,
    truncated_files: usize,
    max_chunk_lines: usize,
    max_chunk_bytes: usize,
    max_file_bytes: usize,
    chunks: Vec<AuditManifestChunk>,
}

#[derive(Debug, Serialize)]
struct AuditManifestChunk {
    index: usize,
    label: String,
    file_count: usize,
    lines: usize,
    bytes: usize,
    files: Vec<AuditManifestFile>,
}

#[derive(Debug, Serialize)]
struct AuditManifestFile {
    path: String,
    language: String,
    lines: usize,
    bytes: usize,
    original_bytes: usize,
    truncated: bool,
}

fn audit_chunks(files: Vec<AuditFile>, max_lines: usize, max_bytes: usize) -> Vec<AuditChunk> {
    let mut chunks = Vec::new();
    let mut current = AuditChunk {
        label: String::new(),
        files: Vec::new(),
        lines: 0,
        bytes: 0,
    };
    for file in files {
        let would_exceed_lines = current.lines + file.lines > max_lines;
        let would_exceed_bytes = current.bytes + file.bytes > max_bytes;
        if !current.files.is_empty() && (would_exceed_lines || would_exceed_bytes) {
            current.label = chunk_label(&current.files);
            chunks.push(current);
            current = AuditChunk {
                label: String::new(),
                files: Vec::new(),
                lines: 0,
                bytes: 0,
            };
        }
        current.lines += file.lines;
        current.bytes += file.bytes;
        current.files.push(file);
    }
    if !current.files.is_empty() {
        current.label = chunk_label(&current.files);
        chunks.push(current);
    }
    chunks
}

fn chunk_label(files: &[AuditFile]) -> String {
    match (files.first(), files.last()) {
        (Some(first), Some(last)) if first.path != last.path => {
            format!("{}..{}", first.path, last.path)
        }
        (Some(first), _) => first.path.clone(),
        _ => "workspace".to_string(),
    }
}

fn workspace_audit_files(
    root: &Path,
    mode: AuditMode,
    max_file_bytes: usize,
) -> Result<Vec<AuditFile>> {
    let mut files = Vec::new();
    let tokei_config = TokeiConfig::default();
    for entry in WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .build()
    {
        let entry = entry.map_err(|err| anyhow::anyhow!(err))?;
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if skip_audit_file(&rel) {
            continue;
        }
        let Some(language) = LanguageType::from_path(path, &tokei_config) else {
            continue;
        };
        if mode == AuditMode::LogicOnly && !logic_language(language) {
            continue;
        }
        let meta = std::fs::metadata(path).with_context(|| format!("failed stating {rel}"))?;
        let original_bytes = meta.len().try_into().unwrap_or(usize::MAX);
        let (raw, truncated) = read_audit_file_prefix(path, max_file_bytes)
            .with_context(|| format!("failed reading {rel}"))?;
        if raw.contains(&0) {
            continue;
        }
        let mut text = String::from_utf8(raw).with_context(|| format!("not utf-8 text: {rel}"))?;
        if mode == AuditMode::LogicOnly {
            text = strip_comments_for_logic(&rel, &text);
            if text.trim().is_empty() {
                continue;
            }
        }
        files.push(AuditFile {
            path: rel,
            language: language.name().to_string(),
            lines: text.lines().count().max(1),
            bytes: text.len(),
            original_bytes,
            truncated,
            text,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn read_audit_file_prefix(path: &Path, max_bytes: usize) -> Result<(Vec<u8>, bool)> {
    let mut file = std::fs::File::open(path)?;
    let mut raw = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut limited = (&mut file).take((max_bytes + 1) as u64);
    limited.read_to_end(&mut raw)?;
    let truncated = raw.len() > max_bytes;
    if truncated {
        raw.truncate(max_bytes);
        while std::str::from_utf8(&raw).is_err() && raw.pop().is_some() {}
        raw.extend_from_slice(
            b"

[oy audit: file truncated at byte budget]
",
        );
    }
    Ok((raw, truncated))
}

fn logic_language(language: LanguageType) -> bool {
    !matches!(
        language,
        LanguageType::Markdown
            | LanguageType::Org
            | LanguageType::Text
            | LanguageType::ReStructuredText
    )
}

fn skip_audit_file(path: &str) -> bool {
    path == "ISSUES.md"
        || path == "ISSUES.draft.md"
        || path == "ISSUES.audit-manifest.json"
        || path.starts_with(".git/")
        || path.starts_with("target/")
        || path.starts_with(".tmp/")
        || path.ends_with("Cargo.lock")
}

fn audit_docs(root: &Path) -> Result<String> {
    let mut out = String::new();
    for name in [
        "README.md",
        "SECURITY.md",
        "CONTRIBUTING.md",
        "CHANGELOG.md",
        "Cargo.toml",
        "assets/session_text.toml",
    ] {
        let path = root.join(name);
        if path.is_file() {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            out.push_str(&format!(
                "\n## {name}\n{}\n",
                crate::ui::truncate_chars(&text, 8000)
            ));
        }
    }
    Ok(out)
}

fn validate_audit_coverage(chunks: &[AuditChunk]) -> Result<()> {
    let files = chunks.iter().map(|chunk| chunk.files.len()).sum::<usize>();
    if files == 0 {
        bail!("audit coverage failed: no code files selected");
    }
    if chunks.iter().any(|chunk| chunk.files.is_empty()) {
        bail!("audit coverage failed: empty chunk planned");
    }
    Ok(())
}

fn audit_manifest(
    mode: AuditMode,
    chunks: &[AuditChunk],
    max_chunk_lines: usize,
    max_chunk_bytes: usize,
    max_file_bytes: usize,
) -> AuditManifest {
    AuditManifest {
        mode,
        file_count: chunks.iter().map(|chunk| chunk.files.len()).sum(),
        chunk_count: chunks.len(),
        total_lines: chunks.iter().map(|chunk| chunk.lines).sum(),
        total_bytes: chunks.iter().map(|chunk| chunk.bytes).sum(),
        truncated_files: chunks
            .iter()
            .flat_map(|chunk| &chunk.files)
            .filter(|file| file.truncated)
            .count(),
        max_chunk_lines,
        max_chunk_bytes,
        max_file_bytes,
        chunks: chunks
            .iter()
            .enumerate()
            .map(|(index, chunk)| AuditManifestChunk {
                index: index + 1,
                label: chunk.label.clone(),
                file_count: chunk.files.len(),
                lines: chunk.lines,
                bytes: chunk.bytes,
                files: chunk
                    .files
                    .iter()
                    .map(|file| AuditManifestFile {
                        path: file.path.clone(),
                        language: file.language.clone(),
                        lines: file.lines,
                        bytes: file.bytes,
                        original_bytes: file.original_bytes,
                        truncated: file.truncated,
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn build_audit_chunk_prompt(
    session: &Session,
    focus: &str,
    sloc: &str,
    docs: &str,
    chunk: &AuditChunk,
    standards: bool,
    mode: AuditMode,
) -> Result<String> {
    let mut parts = vec![
        config::session_text_value("system", "audit")?,
        config::session_text_value("audit", "default_user_prompt")?,
        config::session_text_value("audit", "inspect_suffix")?,
        config::session_text_format(
            "audit",
            "model_suffix",
            &[("model", model::to_genai_model_spec(&session.model))],
        )?,
        config::session_text_format("audit", "chunk_hint", &[("chunk", chunk.label.clone())])?,
        format!("Workspace SLOC/context:\n{sloc}"),
        format_audit_docs(docs),
        format_audit_chunk_content(chunk, mode),
    ];
    if standards {
        parts.push(config::session_text_value("audit", "standards_context")?);
    }
    parts.push(config::session_text_value("audit", "return_suffix")?);
    if mode == AuditMode::LogicOnly {
        parts.push(config::session_text_value("system", "audit_logic_suffix")?);
        parts.push("Logic-only mode: docs and comments were intentionally omitted from pinned source content. Ground findings in executable/runtime logic only.".to_string());
    }
    if !focus.trim().is_empty() {
        parts.push(config::session_text_format(
            "audit",
            "focus_hint",
            &[("focus", focus.to_string())],
        )?);
    }
    Ok(parts.join("\n\n"))
}

fn format_audit_docs(docs: &str) -> String {
    if docs.trim().is_empty() {
        "Docs/session context: <omitted>".to_string()
    } else {
        format!("Docs/session context:\n{docs}")
    }
}

fn format_audit_chunk_content(chunk: &AuditChunk, mode: AuditMode) -> String {
    let mut out = format!(
        "Pinned source content ({} files, {} lines, {} bytes, mode={}):",
        chunk.files.len(),
        chunk.lines,
        chunk.bytes,
        mode.label()
    );
    for file in &chunk.files {
        let truncated = if file.truncated {
            format!(", truncated from {} bytes", file.original_bytes)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "\n\n## File: {} ({}, {} lines, {} bytes{})\n```{}\n{}\n```",
            file.path,
            file.language,
            file.lines,
            file.bytes,
            truncated,
            language_fence(&file.path),
            file.text.trim_end()
        ));
    }
    out
}

fn language_fence(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
}

fn build_audit_final_prompt(
    sloc: &str,
    docs: &str,
    draft: &str,
    mode: AuditMode,
) -> Result<String> {
    let mode_note = if mode == AuditMode::LogicOnly {
        "Logic-only mode: final report must be grounded in executable/runtime code. Docs and comments were omitted."
    } else {
        "Full audit mode."
    };
    Ok(format!(
        "{}\n\n{}\n\nWorkspace SLOC/context:\n{}\n\n{}\n\nCollected draft findings:\n{}",
        config::session_text_value("audit", "final_reduce_prompt")?,
        mode_note,
        sloc,
        format_audit_docs(docs),
        draft
    ))
}

fn strip_comments_for_logic(path: &str, text: &str) -> String {
    let ext = Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    if matches!(
        ext,
        "rs" | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "java"
            | "kt"
            | "kts"
            | "go"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "cs"
            | "swift"
    ) {
        strip_slash_comments(text)
    } else if matches!(
        ext,
        "py" | "rb" | "sh" | "bash" | "zsh" | "toml" | "yaml" | "yml"
    ) {
        strip_hash_comments(text)
    } else {
        text.to_string()
    }
}

fn strip_hash_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        out.push_str(strip_line_comment(line, '#'));
        out.push('\n');
    }
    out
}

fn strip_line_comment(line: &str, marker: char) -> &str {
    let mut quote = None;
    let mut escape = false;
    for (idx, ch) in line.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == marker => return line[..idx].trim_end(),
            None => {}
        }
    }
    line
}

fn strip_slash_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut quote = None;
    let mut escape = false;
    let mut block = false;
    while let Some(ch) = chars.next() {
        if block {
            if ch == '*' && chars.peek() == Some(&'/') {
                let _ = chars.next();
                block = false;
            } else if ch == '\n' {
                out.push('\n');
            }
            continue;
        }
        if let Some(q) = quote {
            out.push(ch);
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' || ch == '`' {
            quote = Some(ch);
            out.push(ch);
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            for next in chars.by_ref() {
                if next == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            let _ = chars.next();
            block = true;
            continue;
        }
        out.push(ch);
    }
    out
}

fn audit_transparency_line(
    session: &Session,
    mode: AuditMode,
    chunk_lines: usize,
    chunk_bytes: usize,
    file_bytes: usize,
    standards: bool,
    manifest: &AuditManifest,
) -> String {
    format!(
        "<!-- Generated by oy {} audit mode={} model={} agent={} chunk_lines={} chunk_bytes={} file_bytes={} standards={} files={} chunks={} truncated_files={} -->",
        env!("CARGO_PKG_VERSION"),
        mode.label(),
        session.model,
        session.agent,
        chunk_lines,
        chunk_bytes,
        file_bytes,
        standards,
        manifest.file_count,
        manifest.chunk_count,
        manifest.truncated_files
    )
}

fn cleanup_audit_temp_files(paths: &[&Path]) -> Result<()> {
    for path in paths {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("failed removing {}", path.display()));
            }
        }
    }
    Ok(())
}

fn append_audit_section(path: &Path, heading: &str, body: &str) -> Result<()> {
    reject_symlink_destination(path)?;
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "\n## {heading}\n\n{}\n", body.trim())?;
    Ok(())
}

fn load_or_new(
    interactive: bool,
    agent_name: &str,
    continue_session: bool,
    resume: &str,
) -> Result<Session> {
    if continue_session || !resume.is_empty() {
        let name = if continue_session { None } else { Some(resume) };
        if let Some(session) = agent::load_saved(name, interactive)? {
            return Ok(session);
        }
    }
    let root = config::oy_root()?;
    let model = model::resolve_model(None)?;
    let profile = config::agent_profile(agent_name)?;
    let policy = config::tool_policy(&profile.name);
    Ok(Session::new(root, model, interactive, profile.name, policy))
}

fn collect_task(parts: &[String]) -> Result<String> {
    if !parts.is_empty() {
        return Ok(parts.join(" "));
    }
    if std::io::stdin().is_terminal() {
        return Ok(String::new());
    }
    let mut input = String::new();
    use std::io::Read as _;
    std::io::stdin().read_to_string(&mut input)?;
    Ok(input.trim().to_string())
}

fn print_session_intro(mode: &str, session: &Session, prompt: Option<&str>) {
    if crate::ui::is_quiet() {
        return;
    }
    crate::ui::section(mode);
    crate::ui::kv("workspace", session.root.display());
    crate::ui::kv("model", &session.model);
    crate::ui::kv("agent", &session.agent);
    crate::ui::kv("risk", policy_risk_label(&session.policy));
    if let Some(prompt) = prompt {
        crate::ui::kv("prompt", crate::ui::compact_preview(prompt, 100));
    }
}

fn write_workspace_file(root: &Path, requested: &Path, body: &str) -> Result<()> {
    let path = resolve_workspace_output_path(root, requested)?;
    let mut out = body.trim_end().to_string();
    out.push('\n');
    write_bytes_file(&path, out.as_bytes())
}

fn resolve_workspace_output_path(root: &Path, requested: &Path) -> Result<PathBuf> {
    if requested.is_absolute()
        || requested
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!(
            "output path must stay inside workspace: {}",
            requested.display()
        );
    }
    let root = root
        .canonicalize()
        .context("failed to resolve workspace root")?;
    let path = root.join(requested);
    let parent = path.parent().unwrap_or(&root);
    if parent.exists() {
        let resolved_parent = parent
            .canonicalize()
            .with_context(|| format!("failed resolving {}", parent.display()))?;
        if !resolved_parent.starts_with(&root) {
            bail!("output path escapes workspace: {}", requested.display());
        }
    }
    reject_symlink_destination(&path)?;
    Ok(path)
}

fn reject_symlink_destination(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("refusing to write symlink: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed checking {}", path.display())),
    }
}

fn write_bytes_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        let mode = std::fs::metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o600);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(mode)
            .open(path)
            .with_context(|| format!("failed writing {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed writing {}", path.display()))?;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(mode);
        file.set_permissions(perms)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes).with_context(|| format!("failed writing {}", path.display()))
    }
}
