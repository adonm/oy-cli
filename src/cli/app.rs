//! CLI entry point: argument parsing and command dispatch.

use crate::audit;
use anyhow::Result;
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

mod audit_cmd;
mod doctor_cmd;
mod enhance_cmd;
mod model_cmd;
mod review_cmd;
mod session_cmd;
mod upgrade_cmd;

use audit_cmd::AuditFormat;
use doctor_cmd::DoctorArgs;
use enhance_cmd::EnhanceArgs;
use model_cmd::ModelArgs;
use review_cmd::ReviewArgs;
use session_cmd::{ChatArgs, RunArgs};
use upgrade_cmd::UpgradeArgs;

#[derive(Debug, Parser)]
#[command(
    name = "oy",
    version,
    about = "Repeatable repository audits and reviews for opencode.",
    after_help = "Examples:\n  oy audit                        (write ISSUES.md)\n  oy review main                  (write REVIEW.md for git diff main)\n  oy enhance <finding-id>         (fix one reported finding)\n  oy setup --dry-run              (preview integration changes)\n  oy setup --workspace\n  oy doctor\n  oy                              (launch opencode with --agent oy)\n\nPrimary direction: deterministic-input audit/review/report workflows. Model conclusions are not deterministic. Running `oy` without a subcommand and compatibility wrappers refresh the generated integration before delegating to opencode; unknown top-level commands/flags pass through."
)]
struct Cli {
    #[arg(long, global = true, conflicts_with_all = ["verbose", "json"], help = "Select quiet output where supported")]
    quiet: bool,
    #[arg(long, global = true, conflicts_with_all = ["quiet", "json"], help = "Select verbose output where supported")]
    verbose: bool,
    #[arg(long, global = true, conflicts_with_all = ["quiet", "verbose"], help = "Print machine-readable JSON where supported")]
    json: bool,
    #[arg(
        long,
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
    /// Install integration files globally, or in this workspace with --workspace.
    Setup(SetupArgs),
    /// Launch opencode with oy MCP wiring.
    Open(OpenArgs),
    /// Start the oy MCP server over stdio.
    Mcp,
    /// Delegate one task to `opencode run`; prompt can be args or stdin.
    Run(RunArgs),
    /// Launch opencode with oy session/mode conveniences.
    Chat(ChatArgs),
    /// Delegate to `opencode models`.
    Model(ModelArgs),
    /// Show config paths, executable/helper availability, and safety mode.
    Doctor(DoctorArgs),
    /// Explain oy safety modes and their opencode mapping.
    Modes,
    /// Run a deterministic-input security audit and write Markdown or SARIF.
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
    /// Run a deterministic-input code-quality review and write REVIEW.md.
    Review(ReviewArgs),
    /// Fix one finding from ISSUES.md or REVIEW.md.
    Enhance(EnhanceArgs),
    /// Upgrade oy and opencode when both are managed by mise.
    Upgrade(UpgradeArgs),
}

#[derive(Debug, Args)]
struct SetupArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Install project-local .opencode files instead of global config"
    )]
    workspace: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Preview generated integration files without writing"
    )]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct OpenArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Print the opencode command that would run"
    )]
    dry_run: bool,
    #[arg(
        long,
        alias = "explain",
        default_value_t = false,
        help = "Explain selected mode, agent, and opencode arguments without launching"
    )]
    explain: bool,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

pub async fn run(argv: Vec<String>) -> Result<i32> {
    let cli = match Cli::try_parse_from(std::iter::once("oy".to_string()).chain(argv.clone())) {
        Ok(cli) => cli,
        Err(err) if should_passthrough_to_opencode(&argv, err.kind()) => {
            crate::ui::init_output_mode(None);
            let (args, mode) = opencode_passthrough_args(argv);
            return crate::opencode::open_command(args, mode, false);
        }
        Err(err) => return print_clap_error(err),
    };
    crate::ui::init_output_mode(cli_output_mode(&cli));
    let mode = cli.mode;
    match cli.command {
        Some(Command::Setup(args)) => crate::opencode::setup_command(args.workspace, args.dry_run),
        Some(Command::Open(args)) => {
            crate::opencode::open_command(args.args, mode, args.dry_run || args.explain)
        }
        Some(Command::Mcp) => crate::mcp::serve_stdio().await,
        Some(Command::Run(args)) => crate::opencode::run_task_command(
            args.task,
            args.shared.continue_session,
            args.shared.resume,
            args.shared.mode,
        ),
        Some(Command::Chat(args)) => crate::opencode::chat_command(
            args.shared.continue_session,
            args.shared.resume,
            args.shared.mode,
        ),
        Some(Command::Model(args)) => crate::opencode::models_command(args.model),
        Some(Command::Doctor(args)) => doctor_cmd::doctor_command(args).await,
        Some(Command::Modes) => modes_command(),
        Some(Command::Audit {
            format,
            out,
            max_chunks,
            focus,
        }) => crate::opencode::audit_workflow_command(
            focus,
            out.unwrap_or_else(|| audit::default_output_path(format.into())),
            max_chunks,
            format.into(),
        ),
        Some(Command::Review(args)) => crate::opencode::review_workflow_command(
            args.target,
            args.focus,
            args.out.unwrap_or_else(review_cmd::default_output_path),
            args.max_chunks,
        ),
        Some(Command::Enhance(args)) => crate::opencode::enhance_workflow_command(
            args.review_target,
            args.focus,
            args.audit_max_chunks,
            args.review_max_chunks,
            args.mode,
        ),
        Some(Command::Upgrade(args)) => upgrade_cmd::upgrade_command(args),
        None => crate::opencode::open_command(Vec::new(), mode, false),
    }
}

fn modes_command() -> Result<i32> {
    crate::ui::section("oy modes");
    crate::ui::line("  default / ask       -> opencode --agent oy");
    crate::ui::line("                         edits ask, bash asks");
    crate::ui::line("  plan / read         -> opencode --agent oy-plan");
    crate::ui::line("                         edits denied, bash denied");
    crate::ui::line("  edit / accept-edits -> opencode --agent oy-edit");
    crate::ui::line("                         edits allowed, bash asks");
    crate::ui::line("  auto / yolo         -> opencode --agent oy-auto --auto");
    crate::ui::line("                         host prompts auto-approved unless explicitly denied");
    Ok(0)
}

fn print_clap_error(err: clap::Error) -> Result<i32> {
    let code = if err.use_stderr() { 2 } else { 0 };
    err.print()?;
    Ok(code)
}

fn should_passthrough_to_opencode(argv: &[String], kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand
    ) && !starts_with_oy_command(argv)
}

fn starts_with_oy_command(argv: &[String]) -> bool {
    const OY_COMMANDS: &[&str] = &[
        "setup", "open", "mcp", "run", "chat", "model", "doctor", "modes", "audit", "review",
        "enhance", "upgrade",
    ];
    first_action_arg(argv).is_some_and(|arg| OY_COMMANDS.contains(&arg))
}

fn first_action_arg(argv: &[String]) -> Option<&str> {
    let mut idx = 0;
    while idx < argv.len() {
        let arg = argv[idx].as_str();
        match arg {
            "--" => return None,
            "--quiet" | "--verbose" | "--json" => idx += 1,
            "--mode" => idx += 2,
            _ if arg.starts_with("--mode=") => idx += 1,
            _ if arg.starts_with('-') => return None,
            _ => return Some(arg),
        }
    }
    None
}

fn opencode_passthrough_args(argv: Vec<String>) -> (Vec<String>, crate::config::SafetyMode) {
    let mut mode = crate::config::SafetyMode::Default;
    let mut args = Vec::with_capacity(argv.len());
    let mut idx = 0;
    let mut before_action = true;
    while idx < argv.len() {
        let arg = argv[idx].as_str();
        if before_action {
            if arg == "--mode" {
                if let Some(value) = argv.get(idx + 1)
                    && let Ok(parsed) = crate::config::SafetyMode::parse(value)
                {
                    mode = parsed;
                    idx += 2;
                    continue;
                }
            } else if let Some(value) = arg.strip_prefix("--mode=") {
                if let Ok(parsed) = crate::config::SafetyMode::parse(value) {
                    mode = parsed;
                    idx += 1;
                    continue;
                }
            } else if arg == "--" {
                args.extend(argv[idx..].iter().cloned());
                break;
            } else if !arg.starts_with('-') {
                before_action = false;
            }
        }
        args.push(argv[idx].clone());
        idx += 1;
    }
    (args, mode)
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
    fn upgrade_is_an_oy_command() {
        let cli = parse_cli_for_test(&["oy", "upgrade", "--dry-run"]);
        assert!(matches!(cli.command, Some(Command::Upgrade(_))));
        assert!(!should_passthrough_to_opencode(
            &["upgrade".to_string(), "--bogus".to_string()],
            ErrorKind::UnknownArgument
        ));
    }

    #[test]
    fn setup_and_open_accept_dry_run_flags() {
        let cli = parse_cli_for_test(&["oy", "setup", "--workspace", "--dry-run"]);
        let Some(Command::Setup(args)) = cli.command else {
            panic!("expected setup command");
        };
        assert!(args.workspace);
        assert!(args.dry_run);

        let cli = parse_cli_for_test(&["oy", "open", "--dry-run", "--", "run", "hello"]);
        let Some(Command::Open(args)) = cli.command else {
            panic!("expected open command");
        };
        assert!(args.dry_run);
        assert_eq!(args.args, vec!["run", "hello"]);
    }

    #[test]
    fn modes_is_an_oy_command() {
        let cli = parse_cli_for_test(&["oy", "modes"]);
        assert!(matches!(cli.command, Some(Command::Modes)));
        assert!(!should_passthrough_to_opencode(
            &["modes".to_string(), "--bogus".to_string()],
            ErrorKind::UnknownArgument
        ));
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

    #[test]
    fn unknown_top_level_action_passes_through_to_opencode() {
        let argv = vec!["tui".to_string(), "--foo".to_string()];
        assert!(should_passthrough_to_opencode(
            &argv,
            ErrorKind::InvalidSubcommand
        ));
        assert_eq!(first_action_arg(&argv), Some("tui"));
    }

    #[test]
    fn known_oy_command_errors_do_not_pass_through() {
        let argv = vec!["review".to_string(), "--bogus".to_string()];
        assert!(!should_passthrough_to_opencode(
            &argv,
            ErrorKind::UnknownArgument
        ));
    }

    #[test]
    fn passthrough_keeps_opencode_agent_flags_and_consumes_oy_mode() {
        let (args, mode) = opencode_passthrough_args(vec![
            "--mode".to_string(),
            "plan".to_string(),
            "--agent".to_string(),
            "build".to_string(),
        ]);

        assert_eq!(mode, crate::config::SafetyMode::Plan);
        assert_eq!(args, vec!["--agent", "build"]);
    }

    #[test]
    fn opencode_agent_flag_before_known_word_still_passes_through() {
        let argv = vec![
            "--agent".to_string(),
            "custom".to_string(),
            "run".to_string(),
        ];

        assert!(should_passthrough_to_opencode(
            &argv,
            ErrorKind::UnknownArgument
        ));
    }
}
