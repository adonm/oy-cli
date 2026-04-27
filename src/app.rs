use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use crate::audit;
use crate::config;
use crate::model;
use crate::session::{self, Session};

const MODEL_LIST_LIMIT: usize = 30;

#[derive(Debug, Parser)]
#[command(
    name = "oy",
    version,
    about = "Small local AI coding assistant for your shell.",
    after_help = "Examples:\n  oy doctor\n  oy model\n  oy \"inspect this repo and summarize risks\"\n  oy chat --mode plan\n  oy run --out plan.md \"write a migration plan\"\n\nSafety: file tools stay inside the workspace, but oy is not a sandbox. Use --mode plan or a container/VM for untrusted repos."
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
    /// List, choose, and save model ids/routing shims.
    Model(ModelArgs),
    /// Check setup, auth, paths, and safety-relevant defaults.
    Doctor(DoctorArgs),
    /// Audit the current workspace and write markdown findings.
    Audit(AuditArgs),
}

#[derive(Debug, Args, Clone)]
struct SharedModeArgs {
    #[arg(
        long,
        alias = "agent",
        default_value = "default",
        help = "Safety mode (default: balanced): plan, ask, edit, or auto"
    )]
    mode: String,
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
    shared: SharedModeArgs,
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
    #[command(flatten)]
    shared: SharedModeArgs,
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
    #[arg(
        long,
        alias = "agent",
        default_value = "default",
        help = "Safety mode to inspect (default: balanced): plan, ask, edit, or auto"
    )]
    mode: String,
}

#[derive(Debug, Args, Clone)]
struct AuditArgs {
    #[arg(value_name = "FOCUS", help = "Optional audit focus text")]
    focus: Vec<String>,
    #[arg(
        long,
        value_name = "PATH",
        default_value = "ISSUES.md",
        help = "Write findings to a workspace file (default: ISSUES.md)"
    )]
    out: PathBuf,
}

pub async fn run(argv: Vec<String>) -> Result<i32> {
    let normalized = normalize_args(argv);
    let cli = Cli::parse_from(std::iter::once("oy".to_string()).chain(normalized.clone()));
    crate::ui::init_output_mode(cli_output_mode(&cli));
    match cli.command.unwrap_or(Command::Run(RunArgs {
        shared: SharedModeArgs {
            mode: "default".to_string(),
            continue_session: false,
            resume: String::new(),
        },
        out: None,
        task: Vec::new(),
    })) {
        Command::Run(args) => run_command(args).await,
        Command::Chat(args) => chat_command(args).await,
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
    let commands = ["run", "chat", "model", "doctor", "audit", "-h", "--help"];
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
            shared: args.shared,
        })
        .await;
    }
    let mut session = load_or_new(
        false,
        &args.shared.mode,
        args.shared.continue_session,
        &args.shared.resume,
    )?;
    print_session_intro("run", &session, Some(&task));
    let answer = session::run_prompt(&mut session, &task).await?;
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
        "mode": session.mode,
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
        &args.shared.mode,
        args.shared.continue_session,
        &args.shared.resume,
    )?;
    print_session_intro("chat", &session, None);
    crate::chat::run_chat(&mut session).await
}

async fn model_command(args: ModelArgs) -> Result<i32> {
    if let Some(model_spec) = args
        .model
        .as_deref()
        .filter(|value| is_exact_model_spec(value))
    {
        let normalized = model::canonical_model_spec(model_spec);
        config::save_model_config(&normalized)?;
        if crate::ui::is_json() {
            print_saved_model_json(&normalized)?;
        } else {
            print_saved_model(&normalized);
        }
        return Ok(0);
    }

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
    if config::can_prompt()
        && !listing.all_models.is_empty()
        && let Some(chosen) = crate::chat::choose_model_with_initial_list(
            listing.current.as_deref(),
            &listing.all_models,
            false,
        )?
    {
        config::save_model_config(&chosen)?;
        print_saved_model(&chosen);
    }
    Ok(0)
}

fn is_exact_model_spec(value: &str) -> bool {
    let value = value.trim();
    value.contains("::") || value.contains('/') || value.contains(':') || value.contains('.')
}

fn print_saved_model_json(saved: &str) -> Result<()> {
    let payload = serde_json::json!({ "saved": saved });
    crate::ui::line(serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn print_model_json(listing: &model::ModelListing, saved: Option<&str>) -> Result<()> {
    let payload = serde_json::json!({
        "current": listing.current,
        "current_shim": listing.current_shim,
        "saved": saved,
        "auth": listing.auth,
        "recommended": listing.recommended,
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
    if !listing.recommended.is_empty() {
        crate::ui::kv("recommended", listing.recommended.join(", "));
        if listing.current.is_none() {
            crate::ui::line(format_args!("  Try: oy model {}", listing.recommended[0]));
        }
    }

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
    let mode = config::safety_mode(&args.mode)?;
    let policy = config::tool_policy(mode.name());
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
            "mode": mode.name(),
            "policy": policy,
            "interactive": config::can_prompt(),
            "non_interactive": config::non_interactive(),
            "config_file": config_file,
            "config_dir": config_dir,
            "sessions_dir": sessions_dir,
            "history_dir": history_dir,
            "bash": bash_ok,
            "recommended": listing.recommended,
            "next_step": recommended_next_step(&listing),
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
        return Ok(0);
    }

    crate::ui::section("Doctor");
    crate::ui::kv("workspace", root.display());
    crate::ui::kv("model", listing.current.as_deref().unwrap_or("<unset>"));
    crate::ui::kv("shim", listing.current_shim.as_deref().unwrap_or("<none>"));
    crate::ui::kv("mode", mode.name());
    crate::ui::kv("files-write", format_args!("{:?}", policy.files_write));
    crate::ui::kv("shell", format_args!("{:?}", policy.shell));
    crate::ui::kv("network", crate::ui::bool_text(policy.network));
    crate::ui::kv("risk", config::policy_risk_label(&policy));
    crate::ui::kv("interactive", crate::ui::bool_text(config::can_prompt()));
    crate::ui::kv(
        "bash",
        crate::ui::status_text(bash_ok, if bash_ok { "ok" } else { "missing" }),
    );
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
        crate::ui::line(format_args!("  {}", recommended_next_step(&listing)));
    }
    crate::ui::line("");
    crate::ui::section("Recommended next steps");
    crate::ui::line(format_args!("  1. {}", recommended_next_step(&listing)));
    crate::ui::line("  2. For untrusted repos: `oy chat --mode plan`");
    crate::ui::line(format_args!(
        "  • Read-only container: {}",
        safe_container_command(&root, true)
    ));
    crate::ui::line("");
    crate::ui::section("Safety");
    crate::ui::line(
        "  oy is not a sandbox. Use `oy chat --mode plan` or a disposable container/VM for untrusted repos.",
    );
    crate::ui::line(
        "  Mount only needed credentials/env vars. Do not mount the host Docker socket into AI-assisted containers.",
    );
    Ok(0)
}

fn recommended_next_step(listing: &model::ModelListing) -> String {
    if listing.current.is_some() {
        return "Run `oy \"inspect this repo\"` or `oy chat`.".to_string();
    }
    if let Some(choice) = listing.recommended.first() {
        return format!("Configure a model: `oy model {choice}`.");
    }
    "Configure provider auth, then run `oy model`; see `oy doctor` output.".to_string()
}

fn safe_container_command(root: &Path, read_only: bool) -> String {
    let mode = if read_only { "ro" } else { "rw" };
    format!(
        "docker run --rm -it -v \"{}:/workspace:{mode}\" -w /workspace oy-image oy chat --mode plan",
        root.display()
    )
}

async fn audit_command(args: AuditArgs) -> Result<i32> {
    let focus = args.focus.join(" ");
    let root = config::oy_root()?;
    let model = model::resolve_model(None)?;
    if !crate::ui::is_quiet() {
        crate::ui::section("audit");
        crate::ui::kv("workspace", root.display());
        crate::ui::kv("model", &model);
        crate::ui::kv("mode", "no-tools");
        crate::ui::kv("out", args.out.display());
        if !focus.trim().is_empty() {
            crate::ui::kv("focus", crate::ui::compact_preview(&focus, 100));
        }
    }
    let result = audit::run(audit::AuditOptions {
        root,
        model,
        focus,
        out: args.out,
    })
    .await?;
    if crate::ui::is_json() {
        let payload = serde_json::json!({
            "output": result.output_path,
            "files": result.file_count,
            "chunks": result.chunk_count,
        });
        crate::ui::line(serde_json::to_string_pretty(&payload)?);
    } else {
        crate::ui::success(format_args!(
            "wrote {} ({} files, {} chunks)",
            result.output_path.display(),
            result.file_count,
            result.chunk_count
        ));
    }
    Ok(0)
}

fn load_or_new(
    interactive: bool,
    mode_name: &str,
    continue_session: bool,
    resume: &str,
) -> Result<Session> {
    if continue_session || !resume.is_empty() {
        let name = if continue_session { None } else { Some(resume) };
        if let Some(session) = session::load_saved(name, interactive)? {
            return Ok(session);
        }
    }
    let root = config::oy_root()?;
    let model = model::resolve_model(None)?;
    let mode = config::safety_mode(mode_name)?;
    let policy = config::tool_policy(mode.name());
    Ok(Session::new(
        root,
        model,
        interactive,
        mode.name().to_string(),
        policy,
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
    if crate::ui::is_quiet() {
        return;
    }
    crate::ui::section(mode);
    crate::ui::kv("workspace", session.root.display());
    crate::ui::kv("model", &session.model);
    crate::ui::kv("mode", &session.mode);
    crate::ui::kv("risk", config::policy_risk_label(&session.policy));
    if let Some(prompt) = prompt {
        crate::ui::kv("prompt", crate::ui::compact_preview(prompt, 100));
    }
}

fn write_workspace_file(root: &Path, requested: &Path, body: &str) -> Result<()> {
    let path = config::resolve_workspace_output_path(root, requested)?;
    let mut out = body.trim_end().to_string();
    out.push('\n');
    config::write_workspace_file(&path, out.as_bytes())
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    #[test]
    fn exact_model_specs_are_endpoint_qualified_or_provider_ids() {
        assert!(is_exact_model_spec("copilot::gpt-4.1-mini"));
        assert!(is_exact_model_spec("openai/gpt-4.1-mini"));
        assert!(is_exact_model_spec(
            "bedrock::global.amazon.nova-2-lite-v1:0"
        ));
        assert!(!is_exact_model_spec("gpt"));
        assert!(!is_exact_model_spec("nova"));
    }
}
