//! CLI entry point: argument parsing and command dispatch.

use crate::audit;
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod audit_cmd;
mod doctor_cmd;
mod enhance_cmd;
mod model_cmd;
mod review_cmd;
mod session_cmd;

use audit_cmd::{AuditArgs, AuditFormat};
use doctor_cmd::DoctorArgs;
use enhance_cmd::EnhanceArgs;
use model_cmd::ModelArgs;
use review_cmd::ReviewArgs;
use session_cmd::{ChatArgs, RunArgs};

#[derive(Debug, Parser)]
#[command(
    name = "oy",
    version,
    about = "Small local AI coding assistant for your shell.",
    after_help = "Examples:\n  oy                              (start interactive chat)\n  oy doctor\n  oy model\n  oy run \"inspect this repo and summarize risks\"\n  oy chat --mode plan\n  oy run --out plan.md \"write a migration plan\"\n\nDefault: running `oy` with no subcommand starts an interactive chat. Use `oy run \"prompt\"` for one-shot tasks.\n\nSafety: file tools stay inside the workspace, but oy is not a sandbox. Use --mode plan or a container/VM for untrusted repos."
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
    /// Run one task in the current workspace; prompt can be args or stdin.
    Run(RunArgs),
    /// Start an interactive chat session with slash commands and history.
    Chat(ChatArgs),
    /// List, choose, and save model ids/routing shims.
    Model(ModelArgs),
    /// Check setup, auth, paths, and safety-relevant defaults.
    Doctor(DoctorArgs),
    /// Audit the current workspace and write findings.
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
    /// Strict code-quality review for a branch/commit diff or the whole workspace.
    Review(ReviewArgs),
    /// Audit, review, then address selected findings one committed change at a time.
    Enhance(EnhanceArgs),
}

pub async fn run(argv: Vec<String>) -> Result<i32> {
    let cli = Cli::parse_from(std::iter::once("oy".to_string()).chain(argv));
    crate::ui::init_output_mode(cli_output_mode(&cli));
    match cli.command {
        Some(Command::Run(args)) => session_cmd::run_command(args).await,
        Some(Command::Chat(args)) => session_cmd::chat_command(args).await,
        Some(Command::Model(args)) => model_cmd::model_command(args).await,
        Some(Command::Doctor(args)) => doctor_cmd::doctor_command(args).await,
        Some(Command::Audit {
            format,
            out,
            max_chunks,
            focus,
        }) => {
            audit_cmd::audit_command(AuditArgs {
                focus,
                out: out.unwrap_or_else(|| audit::default_output_path(format.into())),
                max_chunks,
                format: format.into(),
            })
            .await
        }
        Some(Command::Review(args)) => review_cmd::review_command(args).await,
        Some(Command::Enhance(args)) => enhance_cmd::enhance_command(args).await,
        None => {
            // Default: interactive chat.
            let args = ChatArgs {
                shared: session_cmd::SharedModeArgs {
                    mode: cli.mode,
                    continue_session: false,
                    resume: String::new(),
                },
            };
            session_cmd::chat_command(args).await
        }
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
        assert!(model_cmd::is_exact_model_spec("copilot::gpt-4.1-mini"));
        assert!(model_cmd::is_exact_model_spec("openai/gpt-4.1-mini"));
        assert!(model_cmd::is_exact_model_spec("copilot::gpt-5.5"));
        assert!(!model_cmd::is_exact_model_spec("gpt"));
        assert!(!model_cmd::is_exact_model_spec("nova"));
    }

    #[test]
    fn no_subcommand_defaults_to_chat() {
        let cli = parse_cli_for_test(&["oy"]);
        assert!(cli.command.is_none(), "expected None for default-to-chat");
    }
}
