use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use ignore::WalkBuilder;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use crate::agent::{self, Session};
use crate::config;
use crate::model;

const MODEL_LIST_LIMIT: usize = 30;

#[derive(Debug, Parser)]
#[command(name = "oy", version, about = "AI coding assistant for your shell.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Run(RunArgs),
    Chat(ChatArgs),
    Ralph(RalphArgs),
    Model(ModelArgs),
    Audit(AuditArgs),
}

#[derive(Debug, Args, Clone)]
struct SharedAgentArgs {
    #[arg(long, default_value = "default")]
    agent: String,
    #[arg(long = "continue-session", default_value_t = false)]
    continue_session: bool,
    #[arg(long, default_value = "")]
    resume: String,
}

#[derive(Debug, Args, Clone)]
struct RunArgs {
    #[command(flatten)]
    shared: SharedAgentArgs,
    #[arg(long)]
    out: Option<PathBuf>,
    task: Vec<String>,
}

#[derive(Debug, Args, Clone)]
struct ChatArgs {
    #[arg(long, default_value_t = false)]
    yolo: bool,
    #[command(flatten)]
    shared: SharedAgentArgs,
}

#[derive(Debug, Args, Clone)]
struct RalphArgs {
    #[arg(long, default_value = "default")]
    agent: String,
    task: Vec<String>,
}

#[derive(Debug, Args, Clone)]
struct ModelArgs {
    model: Option<String>,
}

#[derive(Debug, Args, Clone)]
struct AuditArgs {
    focus: Vec<String>,
    #[arg(long, default_value_t = 6000)]
    chunk_lines: usize,
}

pub async fn run(argv: Vec<String>) -> Result<i32> {
    let normalized = normalize_args(argv);
    let cli = Cli::parse_from(std::iter::once("oy".to_string()).chain(normalized.clone()));
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
        Command::Audit(args) => audit_command(args).await,
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
    let commands = ["run", "chat", "ralph", "model", "audit", "-h", "--help"];
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
    if let Some(path) = args.out {
        write_workspace_file(&session.root, &path, &answer)?;
        crate::ui::success(format_args!("wrote {}", path.display()));
    } else if !answer.is_empty() {
        crate::ui::markdown(&format!("{answer}\n"));
    }
    Ok(0)
}

async fn chat_command(args: ChatArgs) -> Result<i32> {
    let mut session = load_or_new(
        true,
        &args.shared.agent,
        args.shared.continue_session,
        &args.shared.resume,
    )?;
    if args.yolo {
        session.policy.auto_approve_edits = true;
        session.policy.auto_approve_bash = true;
    }
    print_session_intro("chat", &session, None);
    crate::ui::run_chat(&mut session).await
}

async fn ralph_command(args: RalphArgs) -> Result<i32> {
    let task = collect_task(&args.task)?;
    if task.trim().is_empty() {
        bail!("Usage: `oy ralph <prompt>` — or pipe prompt text on stdin.");
    }
    let mut session = load_or_new(false, &args.agent, false, "")?;
    session.policy.auto_approve_edits = true;
    session.policy.auto_approve_bash = true;
    print_session_intro("ralph", &session, Some(&task));
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(config::ralph_limit_seconds());
    let mut exit_code = 0;
    let mut run_number = 0usize;
    while std::time::Instant::now() < deadline {
        run_number += 1;
        crate::ui::err_line(format_args!("ralph run {run_number}"));
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
        print_saved_model(&normalized);
        return Ok(0);
    }
    print_model_listing(&listing);
    if config::can_prompt() && !listing.all_models.is_empty() {
        if let Some(chosen) = crate::ui::choose_model_with_initial_list(
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
    crate::ui::choose_model(listing.current.as_deref(), &matches)
        .map(|value| value.unwrap_or(normalized))
}

async fn audit_command(args: AuditArgs) -> Result<i32> {
    let root = config::oy_root()?;
    let model = model::resolve_model(None)?;
    let focus = args.focus.join(" ");
    let mut session = Session::new(
        root.clone(),
        model,
        false,
        "auto-approve".to_string(),
        config::tool_policy("auto-approve"),
    );
    print_session_intro(
        "audit",
        &session,
        (!focus.is_empty()).then_some(focus.as_str()),
    );

    let sloc = crate::tools::compact_workspace_snapshot(&root).unwrap_or_default();
    let docs = audit_docs(&root)?;
    let chunks = audit_chunks(&root, args.chunk_lines.max(500))?;
    let draft_path = root.join("ISSUES.md");
    std::fs::write(
        &draft_path,
        format!(
            "# Audit draft\n\n{sloc}\n\n{} chunks planned.\n\n",
            chunks.len()
        ),
    )?;

    for (idx, chunk) in chunks.iter().enumerate() {
        crate::ui::section(&format!("audit chunk {}/{}", idx + 1, chunks.len()));
        let prompt = build_audit_chunk_prompt(&session, &focus, &sloc, &docs, chunk)?;
        let findings = agent::run_prompt(&mut session, &prompt).await?;
        append_audit_section(
            &draft_path,
            &format!("Chunk {}: {}", idx + 1, chunk.label),
            &findings,
        )?;
    }

    crate::ui::section("audit final reduction");
    let draft = std::fs::read_to_string(&draft_path).unwrap_or_default();
    let final_prompt = build_audit_final_prompt(&sloc, &docs, &draft)?;
    let final_report = agent::run_prompt(&mut session, &final_prompt).await?;
    write_workspace_file(&root, Path::new("ISSUES.md"), &final_report)?;
    crate::ui::success("wrote ISSUES.md");
    Ok(0)
}

#[derive(Debug, Clone)]
struct AuditChunk {
    label: String,
    files: Vec<String>,
}

fn audit_chunks(root: &Path, max_lines: usize) -> Result<Vec<AuditChunk>> {
    let mut chunks = Vec::new();
    let mut current = AuditChunk {
        label: String::new(),
        files: Vec::new(),
    };
    let mut current_lines = 0usize;
    for (path, lines) in workspace_text_files(root)? {
        if !current.files.is_empty() && current_lines + lines > max_lines {
            current.label = chunk_label(&current.files);
            chunks.push(current);
            current = AuditChunk {
                label: String::new(),
                files: Vec::new(),
            };
            current_lines = 0;
        }
        current.files.push(path);
        current_lines += lines;
    }
    if !current.files.is_empty() {
        current.label = chunk_label(&current.files);
        chunks.push(current);
    }
    Ok(chunks)
}

fn chunk_label(files: &[String]) -> String {
    match (files.first(), files.last()) {
        (Some(first), Some(last)) if first != last => format!("{first}..{last}"),
        (Some(first), _) => first.clone(),
        _ => "workspace".to_string(),
    }
}

fn workspace_text_files(root: &Path) -> Result<Vec<(String, usize)>> {
    let mut files = Vec::new();
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
        let Ok(raw) = std::fs::read(path) else {
            continue;
        };
        if raw.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(raw) else {
            continue;
        };
        files.push((rel, text.lines().count().max(1)));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

fn skip_audit_file(path: &str) -> bool {
    path == "ISSUES.md"
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

fn build_audit_chunk_prompt(
    session: &Session,
    focus: &str,
    sloc: &str,
    docs: &str,
    chunk: &AuditChunk,
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
        format!("Docs/session context:\n{docs}"),
        format!(
            "Pinned files to inspect with read/search before reporting:\n{}",
            chunk.files.join("\n")
        ),
        config::session_text_value("audit", "return_suffix")?,
    ];
    if !focus.trim().is_empty() {
        parts.push(config::session_text_format(
            "audit",
            "focus_hint",
            &[("focus", focus.to_string())],
        )?);
    }
    Ok(parts.join("\n\n"))
}

fn build_audit_final_prompt(sloc: &str, docs: &str, draft: &str) -> Result<String> {
    Ok(format!(
        "{}\n\nWorkspace SLOC/context:\n{}\n\nDocs/session context:\n{}\n\nCollected draft findings:\n{}",
        config::session_text_value("audit", "final_reduce_prompt")?,
        sloc,
        docs,
        draft
    ))
}

fn append_audit_section(path: &Path, heading: &str, body: &str) -> Result<()> {
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
    crate::ui::section(mode);
    crate::ui::kv("workspace", session.root.display());
    crate::ui::kv("model", &session.model);
    crate::ui::kv("agent", &session.agent);
    if let Some(prompt) = prompt {
        crate::ui::kv("prompt", crate::ui::compact_preview(prompt, 100));
    }
}

fn write_workspace_file(root: &Path, requested: &Path, body: &str) -> Result<()> {
    if requested.is_absolute()
        || requested
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!(
            "output path must stay inside the workspace: {}",
            requested.display()
        );
    }
    let path = root.join(requested);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    let mut out = body.trim_end().to_string();
    out.push('\n');
    std::fs::write(&path, out).with_context(|| format!("failed writing {}", path.display()))
}
