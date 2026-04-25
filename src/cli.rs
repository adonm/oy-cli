use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use std::io::IsTerminal as _;
use std::path::PathBuf;

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
    #[command(name = "audit-logic")]
    AuditLogic(AuditArgs),
    #[command(name = "renovate-local")]
    RenovateLocal,
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
    focus: Option<String>,
    #[arg(long = "from", default_value = "")]
    from_: String,
    #[arg(long, default_value = "")]
    phase: String,
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
        task: Vec::new(),
    })) {
        Command::Run(args) => run_command(args).await,
        Command::Chat(args) => chat_command(args).await,
        Command::Ralph(args) => ralph_command(args).await,
        Command::Model(args) => model_command(args).await,
        Command::Audit(args) => audit_command(args, false).await,
        Command::AuditLogic(args) => audit_command(args, true).await,
        Command::RenovateLocal => renovate_local().await,
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
        "run",
        "chat",
        "ralph",
        "model",
        "audit",
        "audit-logic",
        "renovate-local",
        "-h",
        "--help",
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
    print_session_intro("Run", &session, Some(&task));
    let answer = agent::run_prompt(&mut session, &task).await?;
    if !answer.is_empty() {
        crate::highlight::stdout_as(&format!("{answer}\n"), "Markdown");
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
        session.yolo = true;
    }
    print_session_intro("Chat", &session, None);
    crate::ui::run_chat(&mut session).await
}

async fn ralph_command(args: RalphArgs) -> Result<i32> {
    let task = collect_task(&args.task)?;
    if task.trim().is_empty() {
        bail!("Usage: `oy ralph <prompt>` — or pipe prompt text on stdin.");
    }
    let mut session = load_or_new(false, &args.agent, false, "")?;
    session.yolo = true;
    print_session_intro("Ralph", &session, Some(&task));
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(config::ralph_limit_seconds());
    let mut exit_code = 0;
    let mut run_number = 0usize;
    while std::time::Instant::now() < deadline {
        run_number += 1;
        eprintln!("ralph run {run_number}");
        if let Err(err) = agent::run_prompt(&mut session, &task).await {
            eprintln!("ralph error: {err:#}");
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
        if let Some(chosen) = choose_model_interactively_without_initial_list(&listing)? {
            config::save_model_config(&chosen)?;
            print_saved_model(&chosen);
        }
    }
    Ok(0)
}

fn print_model_listing(listing: &model::ModelListing) {
    println!("## Models");
    println!(
        "- current: {}",
        current_model_text(
            listing.current.as_deref().unwrap_or("<unset>"),
            listing.current_shim.as_deref(),
        )
    );
    println!("- selectable: {}", listing.all_models.len());

    if !listing.auth.is_empty() {
        println!("\n### Auth / shims");
        for item in &listing.auth {
            let env_var = item.env_var.as_deref().unwrap_or("-");
            let configured = if listing.current_shim.as_deref() == Some(item.adapter.as_str()) {
                " active"
            } else {
                ""
            };
            println!(
                "- {}{}: {} ({})",
                item.adapter, configured, env_var, item.source
            );
            println!("  {}", item.detail);
        }
    }

    if !listing.dynamic.is_empty() {
        println!("\n### Introspected endpoint models");
        for item in &listing.dynamic {
            println!(
                "- {}: {} models via {}",
                item.adapter, item.count, item.source
            );
            for model_name in item.models.iter().take(MODEL_LIST_LIMIT) {
                let marker = if listing.current.as_deref() == Some(model_name.as_str()) {
                    "*"
                } else {
                    " "
                };
                println!("  {marker} {model_name}");
            }
            if item.models.len() > MODEL_LIST_LIMIT {
                println!(
                    "  … {} more; use `oy model <filter>` or interactive selection",
                    item.models.len() - MODEL_LIST_LIMIT
                );
            }
        }
    } else {
        println!("\n### Introspected endpoint models");
        println!("none found from configured OpenAI-compatible endpoints");
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
        println!("\n### Built-in selectable hints");
        for hint in hinted.iter().take(MODEL_LIST_LIMIT) {
            println!("  - {hint}");
        }
        if hinted.len() > MODEL_LIST_LIMIT {
            println!("  … {} more hints", hinted.len() - MODEL_LIST_LIMIT);
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
    println!(
        "saved model: {}",
        saved.model.as_deref().unwrap_or(selection)
    );
    if let Some(shim) = saved.shim {
        println!("shim: {shim}");
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
    choose_from_items(&matches, listing.current.as_deref()).map(|value| value.unwrap_or(normalized))
}

fn choose_model_interactively_without_initial_list(
    listing: &model::ModelListing,
) -> Result<Option<String>> {
    crate::ui::choose_model_with_initial_list(
        listing.current.as_deref(),
        &listing.all_models,
        false,
    )
}

fn choose_from_items(items: &[String], current: Option<&str>) -> Result<Option<String>> {
    crate::ui::choose_model(current, items)
}

async fn audit_command(args: AuditArgs, logic: bool) -> Result<i32> {
    let focus = args.focus.unwrap_or_default();
    let mode = if logic { "logic" } else { "default" };
    let mut session = load_or_new(false, "default", false, "")?;
    let prompt = build_audit_prompt(&session, &focus, &args.from_, &args.phase, logic);
    print_session_intro(
        if logic { "Audit Logic" } else { "Audit" },
        &session,
        Some(&prompt),
    );
    let answer = agent::run_prompt(&mut session, &prompt).await?;
    if answer.trim().is_empty() {
        bail!("audit returned empty output");
    }
    let output = write_audit_report(&session.root, &session.model, mode, &focus, &answer)?;
    println!("wrote {}", output.display());
    Ok(0)
}

async fn renovate_local() -> Result<i32> {
    let workspace = config::oy_root()?;
    let tmp_dir = ensure_tmp_dir(&workspace)?;
    ensure_tmp_gitignored(&workspace)?;
    let config_path = ensure_renovate_config(&workspace)?;
    let report_name = format!("renovate-{}.json", Utc::now().format("%Y-%m-%d"));
    let report_path = tmp_dir.join(&report_name);
    let token = renovate_github_token().await?.ok_or_else(|| anyhow::anyhow!("No GitHub token found (set RENOVATE_GITHUB_COM_TOKEN, GH_TOKEN, or GITHUB_TOKEN; or run `gh auth login`)."))?;
    println!("## Renovate Local");
    println!("- workspace: {}", workspace.display());
    println!("- report: {}", report_path.display());
    println!("- config: {}", config_path.display());
    let status = tokio::process::Command::new("renovate")
        .arg("--platform=local")
        .arg("--require-config=ignored")
        .arg("--dry-run=lookup")
        .arg("--report-type=file")
        .arg("--report-path")
        .arg(format!(".tmp/{report_name}"))
        .current_dir(&workspace)
        .env("RENOVATE_GITHUB_COM_TOKEN", token)
        .status()
        .await
        .context("could not run `renovate`")?;
    if status.success() {
        println!("renovate report written: .tmp/{report_name}");
    }
    Ok(status.code().unwrap_or(1))
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
    Ok(Session::new(
        root,
        model,
        interactive,
        profile.name,
        config::yolo_enabled() || profile.yolo,
    ))
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
    println!("## {mode}");
    println!("- workspace: {}", session.root.display());
    println!("- model: {}", session.model);
    println!("- agent: {}", session.agent);
    if let Some(prompt) = prompt {
        println!("- prompt: {}", preview(prompt, 100));
    }
}

fn preview(text: &str, max: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for (idx, ch) in compact.chars().enumerate() {
        if idx >= max.saturating_sub(3) {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn build_audit_prompt(
    session: &Session,
    focus: &str,
    from_: &str,
    phase: &str,
    logic: bool,
) -> String {
    let mut parts = vec![
        if logic {
            config::session_text_value("audit_logic", "default_user_prompt").unwrap_or_default()
        } else {
            config::session_text_value("audit", "default_user_prompt").unwrap_or_default()
        },
        config::session_text_value("audit", "inspect_suffix").unwrap_or_default(),
        config::session_text_value("audit", "return_suffix").unwrap_or_default(),
        config::session_text_format(
            "audit",
            "model_suffix",
            &[("model", model::to_genai_model_spec(&session.model))],
        )
        .unwrap_or_default(),
    ];
    if !focus.trim().is_empty() {
        if let Ok(value) = config::session_text_format(
            "audit",
            "focus_hint",
            &[("focus", focus.trim().to_string())],
        ) {
            parts.push(value);
        }
    }
    if !from_.trim().is_empty() {
        if let Ok(value) =
            config::session_text_format("audit", "from_hint", &[("from", from_.trim().to_string())])
        {
            parts.push(value);
        }
    }
    if !phase.trim().is_empty() {
        if let Ok(value) = config::session_text_format(
            "audit",
            "phase_hint",
            &[("phase", phase.trim().to_string())],
        ) {
            parts.push(value);
        }
    }
    parts.join(" ")
}

fn write_audit_report(
    workspace: &PathBuf,
    model_spec: &str,
    mode: &str,
    focus: &str,
    report_body: &str,
) -> Result<PathBuf> {
    let path = workspace.join("ISSUES.md");
    let mut out = String::new();
    out.push_str("# Audit Issues\n\n");
    out.push_str(&format!(
        "> Generated by `oy {}` with `OY_MODEL={}`\n\n",
        if mode == "logic" {
            "audit-logic"
        } else {
            "audit"
        },
        model::to_genai_model_spec(model_spec)
    ));
    if !focus.trim().is_empty() {
        out.push_str(&format!("> focus: {}\n\n", focus.trim()));
    }
    out.push_str(report_body.trim());
    out.push('\n');
    std::fs::write(&path, out).with_context(|| format!("failed writing {}", path.display()))?;
    Ok(path)
}

fn ensure_tmp_dir(workspace: &PathBuf) -> Result<PathBuf> {
    let path = workspace.join(".tmp");
    if path.exists() && !path.is_dir() {
        bail!("temporary path is not a directory: {}", path.display());
    }
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn ensure_tmp_gitignored(workspace: &PathBuf) -> Result<()> {
    let path = workspace.join(".gitignore");
    if path.exists() && !path.is_file() {
        bail!("gitignore path is not a file: {}", path.display());
    }
    let existing = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let ignored = existing
        .lines()
        .map(str::trim)
        .any(|line| matches!(line, ".tmp" | ".tmp/" | "/.tmp" | "/.tmp/"));
    if ignored {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(".tmp/\n");
    std::fs::write(&path, updated)?;
    Ok(())
}

fn ensure_renovate_config(workspace: &PathBuf) -> Result<PathBuf> {
    let path = workspace.join("renovate.json");
    if path.exists() {
        if !path.is_file() {
            bail!("renovate config path is not a file: {}", path.display());
        }
        return Ok(path);
    }
    std::fs::write(&path, "{\n  \"extends\": [\"config:recommended\"]\n}\n")?;
    Ok(path)
}

async fn renovate_github_token() -> Result<Option<String>> {
    for key in ["RENOVATE_GITHUB_COM_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return Ok(Some(value));
            }
        }
    }
    let output = tokio::process::Command::new("gh")
        .arg("auth")
        .arg("token")
        .output()
        .await;
    let Ok(output) = output else {
        return Ok(None);
    };
    if !output.status.success() {
        return Ok(None);
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        Ok(None)
    } else {
        Ok(Some(token))
    }
}
