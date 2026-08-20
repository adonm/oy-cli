//! CLI entry point: argument parsing and command dispatch.

use crate::audit;
use anyhow::Result;
use clap::{Args, Parser, Subcommand};

mod audit_cmd;
mod doctor_cmd;
mod review_cmd;
mod upgrade_cmd;

#[cfg(test)]
use audit_cmd::AuditFormat;
use audit_cmd::{AuditAction, AuditArgs};
use doctor_cmd::DoctorArgs;
use review_cmd::{ReviewAction, ReviewArgs};
use upgrade_cmd::UpgradeArgs;

#[derive(Debug, Parser)]
#[command(
    name = "oy",
    version,
    about = "Deterministic audit, review, and one-finding-fix workflows for agent skills.",
    subcommand_required = true,
    arg_required_else_help = true,
    after_help = "Examples:\n  oy setup                     (install the oy skills for your agent)\n  oy audit prepare --path .    (prepare deterministic audit evidence)\n  oy audit finalize --run <id> (write ISSUES.md or SARIF)\n  oy review prepare main       (prepare a git-diff review)\n  oy review finalize --run <id>(write REVIEW.md)\n  oy doctor --check\n  oy upgrade\n\noy prepares and verifies review inputs and reports; your agent executes the skills.\nFindings remain model-dependent."
)]
struct Cli {
    #[arg(long, global = true, conflicts_with_all = ["verbose", "json"], help = "Select quiet output where supported")]
    quiet: bool,
    #[arg(long, global = true, conflicts_with_all = ["quiet", "json"], help = "Select verbose output where supported")]
    verbose: bool,
    #[arg(long, global = true, conflicts_with_all = ["quiet", "verbose"], help = "Print machine-readable JSON where supported")]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Install or remove the oy agent skills.
    Setup(SetupArgs),
    /// Show skills installation, paths, and optional tooling status.
    Doctor(DoctorArgs),
    /// Prepare deterministic-input security audit evidence and finalize Markdown or SARIF.
    Audit(AuditArgs),
    /// Prepare deterministic-input code-quality review evidence and finalize REVIEW.md.
    Review(ReviewArgs),
    /// Upgrade mise-managed oy and refresh the agent skills.
    Upgrade(UpgradeArgs),
}

#[derive(Debug, Args)]
struct SetupArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Install in the project-local .agents/skills path instead of the user directory"
    )]
    workspace: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Preview setup or removal actions without writing"
    )]
    dry_run: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Back up and remove oy-owned skill files and legacy config entries"
    )]
    remove: bool,
}

pub fn run(argv: Vec<String>) -> Result<i32> {
    let cli = match Cli::try_parse_from(std::iter::once("oy".to_string()).chain(argv)) {
        Ok(cli) => cli,
        Err(err) => return print_clap_error(err),
    };
    crate::ui::init_output_mode(cli_output_mode(&cli));
    match cli.command {
        Command::Setup(args) => {
            crate::skills::setup_command(args.workspace, args.dry_run, args.remove)
        }
        Command::Doctor(args) => doctor_cmd::doctor_command(args),
        Command::Audit(args) => match args.action {
            AuditAction::Prepare(prepare) => prepare_artifacts(
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
            AuditAction::Finalize(finalize) => finalize_artifacts(&finalize.run),
        },
        Command::Review(args) => match args.action {
            ReviewAction::Prepare(prepare) => prepare_artifacts(
                crate::artifacts::Kind::Review,
                prepare.path,
                prepare.target,
                prepare.out.unwrap_or_else(review_cmd::default_output_path),
                "markdown",
                prepare.focus,
                prepare.max_chunks,
            ),
            ReviewAction::Finalize(finalize) => finalize_artifacts(&finalize.run),
        },
        Command::Upgrade(args) => upgrade_cmd::upgrade_command(args),
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
    let mut root = <Cli as clap::CommandFactory>::command();
    let mut current: &mut clap::Command = &mut root;
    for name in command.split_whitespace() {
        let Some(next) = current.find_subcommand_mut(name) else {
            panic!("unknown command: {command}");
        };
        current = next;
    }
    let mut help = Vec::new();
    current.write_long_help(&mut help).expect("write help");
    String::from_utf8(help).expect("utf8 help")
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    #[test]
    fn audit_prepare_accepts_file_backed_options() {
        let cli = parse_cli_for_test(&[
            "oy",
            "audit",
            "prepare",
            "--max-chunks",
            "240",
            "--focus",
            "auth paths",
        ]);
        let Command::Audit(args) = cli.command else {
            panic!("expected audit command");
        };
        let AuditAction::Prepare(prepare) = args.action else {
            panic!("expected audit prepare action");
        };
        assert_eq!(prepare.max_chunks, 240);
        assert_eq!(prepare.focus, vec!["auth paths"]);
    }

    #[test]
    fn review_prepare_accepts_file_backed_options() {
        let cli = parse_cli_for_test(&["oy", "review", "prepare", "main", "--max-chunks", "20"]);
        let Command::Review(args) = cli.command else {
            panic!("expected review command");
        };
        let ReviewAction::Prepare(prepare) = args.action else {
            panic!("expected review prepare action");
        };
        assert_eq!(prepare.target.as_deref(), Some("main"));
        assert_eq!(prepare.max_chunks, 20);
    }

    #[test]
    fn audit_finalize_requires_run_flag() {
        let run = "a".repeat(48);
        let cli = parse_cli_for_test(&["oy", "audit", "finalize", "--run", &run]);
        let Command::Audit(args) = cli.command else {
            panic!("expected audit command");
        };
        let AuditAction::Finalize(finalize) = args.action else {
            panic!("expected audit finalize action");
        };
        assert_eq!(finalize.run, run);
    }

    #[test]
    fn bare_audit_requires_an_action() {
        assert!(Cli::try_parse_from(["oy", "audit"]).is_err());
    }

    #[test]
    fn bare_review_requires_an_action() {
        assert!(Cli::try_parse_from(["oy", "review"]).is_err());
    }

    #[test]
    fn no_subcommand_shows_help() {
        assert!(Cli::try_parse_from(["oy"]).is_err());
    }

    #[test]
    fn help_documents_audit_options() {
        let help = command_help_for_test("audit");
        assert!(help.contains("prepare"));
        assert!(help.contains("finalize"));
        let prepare = command_help_for_test("audit prepare");
        assert!(prepare.contains("--max-chunks <N>"));
        assert!(prepare.contains("--format <FORMAT>"));
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
    fn audit_accepts_sarif_format() {
        let cli = parse_cli_for_test(&["oy", "audit", "prepare", "--format", "sarif"]);
        let Command::Audit(args) = cli.command else {
            panic!("expected audit command");
        };
        let AuditAction::Prepare(prepare) = args.action else {
            panic!("expected audit prepare action");
        };
        assert_eq!(prepare.format, AuditFormat::Sarif);
        assert_eq!(prepare.out, None);
    }

    #[test]
    fn upgrade_is_an_oy_command() {
        let cli = parse_cli_for_test(&["oy", "upgrade", "--dry-run"]);
        assert!(matches!(cli.command, Command::Upgrade(_)));
    }

    #[test]
    fn setup_accepts_dry_run_flag() {
        let cli = parse_cli_for_test(&["oy", "setup", "--workspace", "--dry-run"]);
        let Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        assert!(args.workspace);
        assert!(args.dry_run);
    }

    #[test]
    fn removed_and_unknown_commands_are_rejected() {
        for command in [
            "open", "chat", "model", "mcp", "tui", "run", "enhance", "recover",
        ] {
            assert!(Cli::try_parse_from(["oy", command]).is_err(), "{command}");
        }
        assert!(
            Cli::try_parse_from(["oy", "doctor", "--install-sighthound"]).is_err(),
            "removed Sighthound installer flag"
        );
        assert!(
            Cli::try_parse_from(["oy", "setup", "--cursor"]).is_err(),
            "removed standalone Cursor setup"
        );
    }
}
