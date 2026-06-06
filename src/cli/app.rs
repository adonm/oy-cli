//! CLI entry point: argument parsing and command dispatch.

use crate::audit;
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

mod audit_cmd;
mod doctor_cmd;
mod enhance_cmd;
mod model_cmd;
mod review_cmd;
mod session_cmd;

use audit_cmd::AuditFormat;
use doctor_cmd::DoctorArgs;
use enhance_cmd::EnhanceArgs;
use model_cmd::ModelArgs;
use review_cmd::ReviewArgs;
use session_cmd::{ChatArgs, RunArgs};

#[derive(Debug, Parser)]
#[command(
    name = "oy",
    version,
    about = "OpenCode launcher plus deterministic oy MCP tools.",
    after_help = "Examples:\n  oy                              (setup global integration and launch OpenCode with --agent oy)\n  oy setup\n  oy setup --workspace\n  oy doctor\n  oy model\n  oy run \"inspect this repo and summarize risks\"\n  oy audit auth paths\n  oy review main --focus tests\n\nDefault: running `oy` with no subcommand installs/updates global OpenCode oy integration and launches OpenCode with an oy agent matching --mode. Legacy commands delegate to OpenCode.\n\nSafety: OpenCode owns model execution, UI, sessions, and permissions. oy MCP tools are deterministic repo-analysis/report helpers."
)]
struct Cli {
    #[arg(long, global = true, conflicts_with_all = ["verbose", "json"], help = "Suppress normal progress output")]
    quiet: bool,
    #[arg(long, global = true, conflicts_with_all = ["quiet", "json"], help = "Show fuller tool previews")]
    verbose: bool,
    #[arg(long, global = true, conflicts_with_all = ["quiet", "verbose"], help = "Print machine-readable JSON where supported")]
    json: bool,
    #[arg(
        long,
        alias = "agent",
        default_value = "default",
        global = true,
        help = "Safety mode: plan, ask, edit, or auto (default: balanced)"
    )]
    mode: crate::config::SafetyMode,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Install OpenCode integration files globally, or in this workspace with --workspace.
    Setup(SetupArgs),
    /// Launch OpenCode with oy MCP wiring.
    Open(OpenArgs),
    /// Start the oy MCP server over stdio for OpenCode.
    Mcp,
    /// Delegate one task to `opencode run`; prompt can be args or stdin.
    Run(RunArgs),
    /// Launch OpenCode, preserving legacy `oy chat` entrypoint.
    Chat(ChatArgs),
    /// Delegate to `opencode models`.
    Model(ModelArgs),
    /// Check setup, auth, paths, and safety-relevant defaults.
    Doctor(DoctorArgs),
    /// Delegate an oy-style audit to OpenCode.
    Audit {
        #[arg(
            long,
            value_enum,
            default_value_t = AuditFormat::Markdown,
            help = "Output format: markdown or sarif"
        )]
        format: AuditFormat,
        #[arg(
            long,
            value_name = "PATH",
            help = "Write findings to a workspace file (default: ISSUES.md or oy.sarif)"
        )]
        out: Option<PathBuf>,
        #[arg(
            long,
            value_name = "N",
            default_value_t = audit::DEFAULT_MAX_REVIEW_CHUNKS,
            help = "Maximum audit chunks to review before failing closed"
        )]
        max_chunks: usize,
        #[arg(value_name = "FOCUS", help = "Optional audit focus text")]
        focus: Vec<String>,
    },
    /// Delegate an oy-style review to OpenCode.
    Review(ReviewArgs),
    /// Delegate oy-style finding remediation to OpenCode.
    Enhance(EnhanceArgs),
}

#[derive(Debug, Args)]
struct SetupArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Install project-local .opencode files instead of global OpenCode config"
    )]
    workspace: bool,
}

#[derive(Debug, Args)]
struct OpenArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

pub async fn run(argv: Vec<String>) -> Result<i32> {
    let cli = Cli::parse_from(std::iter::once("oy".to_string()).chain(argv));
    crate::ui::init_output_mode(cli_output_mode(&cli));
    let mode = cli.mode;
    match cli.command {
        Some(Command::Setup(args)) => crate::opencode::setup_command(args.workspace),
        Some(Command::Open(args)) => crate::opencode::launch_command(args.args, mode),
        Some(Command::Mcp) => crate::mcp::serve_stdio().await,
        Some(Command::Run(args)) => crate::opencode::legacy_run_command(
            args.task,
            args.shared.continue_session,
            args.shared.resume,
            args.shared.mode,
            args.out,
        ),
        Some(Command::Chat(args)) => crate::opencode::legacy_chat_command(
            args.shared.continue_session,
            args.shared.resume,
            args.shared.mode,
        ),
        Some(Command::Model(args)) => crate::opencode::legacy_model_command(args.model),
        Some(Command::Doctor(args)) => doctor_cmd::doctor_command(args).await,
        Some(Command::Audit {
            format,
            out,
            max_chunks,
            focus,
        }) => crate::opencode::legacy_audit_command(
            focus,
            out.unwrap_or_else(|| audit::default_output_path(format.into())),
            max_chunks,
            format.into(),
        ),
        Some(Command::Review(args)) => crate::opencode::legacy_review_command(
            args.target,
            args.focus,
            args.out.unwrap_or_else(review_cmd::default_output_path),
            args.max_chunks,
        ),
        Some(Command::Enhance(args)) => crate::opencode::legacy_enhance_command(
            args.review_target,
            args.focus,
            args.audit_max_chunks,
            args.review_max_chunks,
            args.mode,
        ),
        None => crate::opencode::launch_command(Vec::new(), mode),
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

#[cfg(test)]
fn parse_cli_for_test(args: &[&str]) -> Cli {
    Cli::parse_from(args)
}

#[cfg(test)]
fn command_help_for_test(command: &str) -> String {
    let mut cmd = <Cli as clap::CommandFactory>::command();
    let Some(subcommand) = cmd.find_subcommand_mut(command) else {
        panic!("unknown command: {command}");
    };
    let mut help = Vec::new();
    subcommand.write_long_help(&mut help).expect("write help");
    String::from_utf8(help).expect("utf8 help")
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    #[test]
    fn audit_accepts_max_chunks_flag() {
        let cli = parse_cli_for_test(&["oy", "audit", "--max-chunks", "240", "auth paths"]);
        let Some(Command::Audit {
            max_chunks, focus, ..
        }) = cli.command
        else {
            panic!("expected audit command");
        };
        assert_eq!(max_chunks, 240);
        assert_eq!(focus, vec!["auth paths"]);
    }

    #[test]
    fn help_documents_audit_options() {
        let help = command_help_for_test("audit");
        assert!(help.contains("--max-chunks <N>"));
        assert!(help.contains("--format <FORMAT>"));
    }

    #[test]
    fn doctor_help_snapshot() {
        insta::assert_snapshot!(command_help_for_test("doctor"));
    }

    #[test]
    fn review_accepts_target_and_focus_flags() {
        let cli = parse_cli_for_test(&[
            "oy",
            "review",
            "main",
            "--focus",
            "types and boundaries",
            "--max-chunks",
            "120",
        ]);
        let Some(Command::Review(args)) = cli.command else {
            panic!("expected review command");
        };
        assert_eq!(args.target.as_deref(), Some("main"));
        assert_eq!(args.focus, vec!["types and boundaries"]);
        assert_eq!(args.max_chunks, 120);
    }

    #[test]
    fn audit_accepts_sarif_format() {
        let cli = parse_cli_for_test(&["oy", "audit", "--format", "sarif", "auth paths"]);
        let Some(Command::Audit { format, out, .. }) = cli.command else {
            panic!("expected audit command");
        };
        assert_eq!(format, AuditFormat::Sarif);
        assert_eq!(out, None);
    }

    #[test]
    fn enhance_accepts_auto_mode_and_focus() {
        let cli = parse_cli_for_test(&[
            "oy",
            "enhance",
            "--mode",
            "auto",
            "--review-target",
            "main",
            "security",
        ]);
        let Some(Command::Enhance(args)) = cli.command else {
            panic!("expected enhance command");
        };
        assert_eq!(args.mode, crate::config::SafetyMode::AutoAll);
        assert_eq!(args.review_target.as_deref(), Some("main"));
        assert_eq!(args.focus, vec!["security"]);
    }

    #[test]
    fn help_documents_enhance_options() {
        let help = command_help_for_test("enhance");
        assert!(help.contains("--mode <MODE>"));
        assert!(help.contains("--review-target <TARGET>"));
    }

    #[test]
    fn exact_model_specs_are_endpoint_qualified_or_provider_ids() {
        assert!(model_cmd::is_exact_model_spec("openai/gpt-4.1-mini"));
        assert!(!model_cmd::is_exact_model_spec("gpt"));
    }

    #[test]
    fn no_subcommand_launches_opencode() {
        let cli = parse_cli_for_test(&["oy"]);
        assert!(cli.command.is_none(), "expected None for default launch");
    }
}
