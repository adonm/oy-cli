use crate::audit;
use crate::config;
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod audit_cmd;
mod doctor_cmd;
mod model_cmd;
mod session_cmd;

use audit_cmd::{AuditArgs, AuditFormat};
use doctor_cmd::DoctorArgs;
use model_cmd::ModelArgs;
use session_cmd::{ChatArgs, RunArgs, SharedModeArgs};

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
}

pub async fn run(argv: Vec<String>) -> Result<i32> {
    let normalized = normalize_args(argv);
    let mut cli = Cli::parse_from(std::iter::once("oy".to_string()).chain(normalized.clone()));
    restore_trailing_audit_options(&mut cli);
    crate::ui::init_output_mode(cli_output_mode(&cli));
    match cli.command.unwrap_or(Command::Run(RunArgs {
        shared: SharedModeArgs {
            mode: config::SafetyMode::Default,
            continue_session: false,
            resume: String::new(),
        },
        out: None,
        task: Vec::new(),
    })) {
        Command::Run(args) => session_cmd::run_command(args).await,
        Command::Chat(args) => session_cmd::chat_command(args).await,
        Command::Model(args) => model_cmd::model_command(args).await,
        Command::Doctor(args) => doctor_cmd::doctor_command(args).await,
        Command::Audit {
            format,
            out,
            max_chunks,
            focus,
        } => {
            audit_cmd::audit_command(AuditArgs {
                focus,
                out: out.unwrap_or_else(|| audit::default_output_path(format.into())),
                max_chunks,
                format: format.into(),
            })
            .await
        }
    }
}

fn restore_trailing_audit_options(cli: &mut Cli) {
    let Some(Command::Audit {
        format: _,
        out: _,
        max_chunks,
        focus,
    }) = &mut cli.command
    else {
        return;
    };
    let mut filtered_focus = Vec::new();
    let mut i = 0usize;
    while i < focus.len() {
        match focus[i].as_str() {
            "--max-chunks" => {
                if let Some(value) = focus.get(i + 1)
                    && let Ok(parsed) = value.parse::<usize>()
                {
                    *max_chunks = parsed;
                    i += 2;
                    continue;
                }
            }
            raw if raw.starts_with("--max-chunks=") => {
                if let Some((_, value)) = raw.split_once('=')
                    && let Ok(parsed) = value.parse::<usize>()
                {
                    *max_chunks = parsed;
                    i += 1;
                    continue;
                }
            }
            _ => {}
        }
        filtered_focus.push(focus[i].clone());
        i += 1;
    }
    *focus = filtered_focus;
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
    let mut cli = Cli::parse_from(args);
    restore_trailing_audit_options(&mut cli);
    cli
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
    fn audit_accepts_sarif_format() {
        let cli = parse_cli_for_test(&["oy", "audit", "--format", "sarif", "auth paths"]);
        let Some(Command::Audit { format, out, .. }) = cli.command else {
            panic!("expected audit command");
        };
        assert_eq!(format, AuditFormat::Sarif);
        assert_eq!(out, None);
    }

    #[test]
    fn exact_model_specs_are_endpoint_qualified_or_provider_ids() {
        assert!(model_cmd::is_exact_model_spec("copilot::gpt-4.1-mini"));
        assert!(model_cmd::is_exact_model_spec("openai/gpt-4.1-mini"));
        assert!(model_cmd::is_exact_model_spec("copilot::gpt-5.5"));
        assert!(!model_cmd::is_exact_model_spec("gpt"));
        assert!(!model_cmd::is_exact_model_spec("nova"));
    }
}
