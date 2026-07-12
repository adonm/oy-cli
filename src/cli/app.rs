//! CLI entry point: argument parsing and command dispatch.

use crate::audit;
use anyhow::Result;
use clap::{Args, Parser, Subcommand};

mod audit_cmd;
mod doctor_cmd;
mod enhance_cmd;
mod review_cmd;
mod session_cmd;
mod upgrade_cmd;

#[cfg(test)]
use audit_cmd::AuditFormat;
use audit_cmd::{AuditAction, AuditArgs};
use doctor_cmd::DoctorArgs;
use enhance_cmd::EnhanceArgs;
use review_cmd::{ReviewAction, ReviewArgs};
use session_cmd::RunArgs;
use upgrade_cmd::UpgradeArgs;

#[derive(Debug, Parser)]
#[command(
    name = "oy",
    version,
    about = "A concise autonomous OpenCode agent with repeatable repository audits and reviews.",
    after_help = "Examples:\n  oy run --auto <task>            (autonomous task with the oy agent)\n  oy audit                        (write ISSUES.md)\n  oy review main                  (write REVIEW.md for git diff main)\n  oy enhance <finding-id>         (fix one reported finding)\n  oy setup --dry-run              (preview integration changes)\n  oy setup --workspace\n  oy doctor --check\n  oy                              (launch the OpenCode 2 TUI)\n\nPrimary direction: one concise oy agent plus deterministic-input audit/review/report workflows. OpenCode and the user own permissions; model conclusions are not deterministic."
)]
struct Cli {
    #[arg(long, global = true, conflicts_with_all = ["verbose", "json"], help = "Select quiet output where supported")]
    quiet: bool,
    #[arg(long, global = true, conflicts_with_all = ["quiet", "json"], help = "Select verbose output where supported")]
    verbose: bool,
    #[arg(long, global = true, conflicts_with_all = ["quiet", "verbose"], help = "Print machine-readable JSON where supported")]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Register the version-matched npm plugin globally, or locally with --workspace.
    Setup(SetupArgs),
    /// Start the compatibility oy MCP server over stdio.
    #[command(about = "Start the compatibility oy MCP server over stdio")]
    Mcp,
    /// Run one task through OpenCode 2; prompt can be args or stdin.
    Run(RunArgs),
    /// Show config paths, executable/helper availability, and integration status.
    Doctor(DoctorArgs),
    /// Run a deterministic-input security audit and write Markdown or SARIF.
    Audit(AuditArgs),
    /// Run a deterministic-input code-quality review and write REVIEW.md.
    Review(ReviewArgs),
    /// Fix one finding from ISSUES.md or REVIEW.md.
    Enhance(EnhanceArgs),
    /// Resume the retained OpenCode session for an interrupted bound workflow.
    Recover,
    /// Upgrade mise-managed oy and OpenCode, backing up the previous integration.
    Upgrade(UpgradeArgs),
}

#[derive(Debug, Args)]
struct SetupArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Register the plugin in project-local .opencode config instead of global config"
    )]
    workspace: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Preview plugin/config migration actions without writing"
    )]
    dry_run: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Back up and remove oy-namespaced files and config entries"
    )]
    remove: bool,
}

pub async fn run(argv: Vec<String>) -> Result<i32> {
    let cli = match Cli::try_parse_from(std::iter::once("oy".to_string()).chain(argv)) {
        Ok(cli) => cli,
        Err(err) => return print_clap_error(err),
    };
    crate::ui::init_output_mode(cli_output_mode(&cli));
    match cli.command {
        Some(Command::Setup(args)) => {
            crate::opencode::setup_command(args.workspace, args.dry_run, args.remove)
        }
        Some(Command::Mcp) => crate::mcp::serve_stdio().await,
        Some(Command::Run(args)) => crate::opencode::run_task_command(
            args.task,
            args.shared.continue_session,
            args.shared.resume,
            args.auto,
        ),
        Some(Command::Doctor(args)) => doctor_cmd::doctor_command(args).await,
        Some(Command::Audit(args)) => match args.action {
            Some(AuditAction::Prepare(prepare)) => prepare_artifacts(
                crate::artifacts::Kind::Audit,
                prepare.path,
                None,
                prepare
                    .out
                    .unwrap_or_else(|| audit::default_output_path(prepare.format.into())),
                prepare.format.name(),
                prepare.focus,
                prepare.max_chunks,
            ),
            Some(AuditAction::Finalize(finalize)) => finalize_artifacts(&finalize.run),
            None => crate::opencode::audit_workflow_command(
                args.focus,
                args.out
                    .unwrap_or_else(|| audit::default_output_path(args.format.into())),
                args.max_chunks,
                args.format.into(),
            ),
        },
        Some(Command::Review(args)) => match args.action {
            Some(ReviewAction::Prepare(prepare)) => prepare_artifacts(
                crate::artifacts::Kind::Review,
                prepare.path,
                prepare.target,
                prepare.out.unwrap_or_else(review_cmd::default_output_path),
                "markdown",
                prepare.focus,
                prepare.max_chunks,
            ),
            Some(ReviewAction::Finalize(finalize)) => finalize_artifacts(&finalize.run),
            None => crate::opencode::review_workflow_command(
                args.target,
                args.focus,
                args.out.unwrap_or_else(review_cmd::default_output_path),
                args.max_chunks,
            ),
        },
        Some(Command::Enhance(args)) => crate::opencode::enhance_workflow_command(
            args.review_target,
            args.focus,
            args.audit_max_chunks,
            args.review_max_chunks,
            args.interactive,
        ),
        Some(Command::Recover) => crate::opencode::recover_workflow_command(),
        Some(Command::Upgrade(args)) => upgrade_cmd::upgrade_command(args),
        None => crate::opencode::launch_command(),
    }
}

fn prepare_artifacts(
    kind: crate::artifacts::Kind,
    path: String,
    target: Option<String>,
    output: std::path::PathBuf,
    format: &str,
    focus: Vec<String>,
    max_chunks: usize,
) -> Result<i32> {
    let root = crate::config::oy_root()?;
    let result = crate::artifacts::prepare(
        &root,
        crate::artifacts::PrepareRequest {
            kind,
            path,
            target,
            output,
            format: format.to_string(),
            focus,
            max_chunks,
            model: std::env::var("OY_OPENCODE_MODEL")
                .ok()
                .filter(|model| !model.trim().is_empty()),
        },
    )?;
    crate::ui::line(serde_json::to_string_pretty(&result)?);
    Ok(0)
}

fn finalize_artifacts(run_id: &str) -> Result<i32> {
    let root = crate::config::oy_root()?;
    let result = crate::artifacts::finalize(&root, run_id)?;
    crate::ui::line(serde_json::to_string_pretty(&result)?);
    Ok(0)
}

fn print_clap_error(err: clap::Error) -> Result<i32> {
    let code = if err.use_stderr() { 2 } else { 0 };
    err.print()?;
    Ok(code)
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
        let Some(Command::Audit(args)) = cli.command else {
            panic!("expected audit command");
        };
        assert_eq!(args.max_chunks, 240);
        assert_eq!(args.focus, vec!["auth paths"]);
    }

    #[test]
    fn review_prepare_accepts_file_backed_options() {
        let cli = parse_cli_for_test(&["oy", "review", "prepare", "main", "--max-chunks", "20"]);
        let Some(Command::Review(args)) = cli.command else {
            panic!("expected review command");
        };
        let Some(ReviewAction::Prepare(prepare)) = args.action else {
            panic!("expected review prepare action");
        };
        assert_eq!(prepare.target.as_deref(), Some("main"));
        assert_eq!(prepare.max_chunks, 20);
    }

    #[test]
    fn audit_finalize_requires_run_flag() {
        let run = "a".repeat(48);
        let cli = parse_cli_for_test(&["oy", "audit", "finalize", "--run", &run]);
        let Some(Command::Audit(args)) = cli.command else {
            panic!("expected audit command");
        };
        let Some(AuditAction::Finalize(finalize)) = args.action else {
            panic!("expected audit finalize action");
        };
        assert_eq!(finalize.run, run);
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
    fn command_reference_lists_every_cli_subcommand() {
        let command = <Cli as clap::CommandFactory>::command();
        let reference = include_str!("../../docs/reference.md");

        for subcommand in command.get_subcommands() {
            let name = subcommand.get_name();
            if name == "help" {
                continue;
            }
            assert!(
                reference.contains(&format!("`oy {name}")),
                "docs/reference.md is missing the `{name}` command"
            );
        }
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
        let Some(Command::Audit(args)) = cli.command else {
            panic!("expected audit command");
        };
        assert_eq!(args.format, AuditFormat::Sarif);
        assert_eq!(args.out, None);
    }

    #[test]
    fn enhance_accepts_target_and_focus() {
        let cli = parse_cli_for_test(&["oy", "enhance", "--review-target", "main", "security"]);
        let Some(Command::Enhance(args)) = cli.command else {
            panic!("expected enhance command");
        };
        assert_eq!(args.review_target.as_deref(), Some("main"));
        assert_eq!(args.focus, vec!["security"]);
    }

    #[test]
    fn help_documents_enhance_options() {
        let help = command_help_for_test("enhance");
        assert!(help.contains("--review-target <TARGET>"));
    }

    #[test]
    fn upgrade_is_an_oy_command() {
        let cli = parse_cli_for_test(&["oy", "upgrade", "--dry-run"]);
        assert!(matches!(cli.command, Some(Command::Upgrade(_))));
    }

    #[test]
    fn setup_accepts_dry_run_flag() {
        let cli = parse_cli_for_test(&["oy", "setup", "--workspace", "--dry-run"]);
        let Some(Command::Setup(args)) = cli.command else {
            panic!("expected setup command");
        };
        assert!(args.workspace);
        assert!(args.dry_run);
    }

    #[test]
    fn no_subcommand_launches_opencode() {
        let cli = parse_cli_for_test(&["oy"]);
        assert!(cli.command.is_none(), "expected None for default launch");
    }

    #[test]
    fn run_auto_uses_the_single_oy_agent_flag() {
        let cli = parse_cli_for_test(&["oy", "run", "--auto", "finish the task"]);
        let Some(Command::Run(args)) = cli.command else {
            panic!("expected run command");
        };
        assert!(args.auto);
    }

    #[test]
    fn removed_and_unknown_commands_are_rejected() {
        for command in ["open", "chat", "model", "tui"] {
            assert!(Cli::try_parse_from(["oy", command]).is_err(), "{command}");
        }
    }
}
