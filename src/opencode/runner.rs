//! OpenCode launch and bound workflow orchestration.

use super::{
    OpenCodeHost, RuntimeHealth, api, host::OPENCODE_ENV, setup::ensure_opencode_integration,
};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::io::{IsTerminal as _, Read as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{artifacts, audit, config, ui};

pub(crate) fn launch_command() -> Result<i32> {
    let root = config::oy_root()?;
    ensure_opencode_integration()?;
    let host = OpenCodeHost::selected_in(&root);
    require_supported_host(&host)?;
    run_opencode(&host, &root, Vec::new(), None)
}

pub(crate) fn run_task_command(
    task: Vec<String>,
    continue_session: bool,
    resume: String,
    auto: bool,
) -> Result<i32> {
    let root = config::oy_root()?;
    ensure_opencode_integration()?;
    let host = OpenCodeHost::selected_in(&root);
    require_supported_host(&host)?;
    api::OpenCodeApi::new(&host).ensure_agent(&root, "oy")?;
    let prompt = collect_prompt(task)?;
    if prompt.trim().is_empty() {
        return launch_with_host(&host, continue_session, resume);
    }
    let mut args = vec!["run".to_string()];
    push_session_args(&mut args, continue_session, &resume);
    push_run_agent_args(&mut args, "oy", auto);
    if let Some(model) = selected_model() {
        args.extend(["--model".to_string(), model]);
    }
    if ui::is_json() {
        args.extend(["--format".to_string(), "json".to_string()]);
    }
    args.push(prompt);
    run_opencode(&host, &root, args, None)
}

fn launch_with_host(host: &OpenCodeHost, continue_session: bool, resume: String) -> Result<i32> {
    let mut args = Vec::new();
    push_session_args(&mut args, continue_session, &resume);
    let root = config::oy_root()?;
    run_opencode(host, &root, args, None)
}

pub(crate) fn audit_workflow_command(
    focus: Vec<String>,
    out: PathBuf,
    max_chunks: usize,
    format: audit::AuditOutputFormat,
) -> Result<i32> {
    if max_chunks == 0 {
        bail!("max_chunks must be greater than zero");
    }
    let root = config::oy_root()?;
    ensure_opencode_integration()?;
    let host = OpenCodeHost::selected_in(&root);
    require_supported_host(&host)?;
    config::resolve_workspace_output_path(&root, &out)?;
    let api = api::OpenCodeApi::new(&host);
    api.ensure_workflow(&root, "oy-audit")?;
    let model = match selected_model() {
        Some(model) => Some(model),
        None => Some(api.default_model(&root)?),
    };
    let (scope, focus) = crate::workflow::resolve_scope(&root, &focus)?;
    let path = match &scope {
        crate::workflow::WorkflowScope::Workspace { path } => path.clone(),
        crate::workflow::WorkflowScope::GitDiff { .. } => {
            unreachable!("audit scope cannot be a git diff")
        }
    };
    let prepared = artifacts::prepare(
        &root,
        artifacts::PrepareRequest {
            kind: artifacts::Kind::Audit,
            path,
            target: None,
            output: out.clone(),
            format: format.name().to_string(),
            focus: focus.clone(),
            max_chunks,
            model: model.clone(),
        },
    )?;
    let artifact_run_id = prepared_run_id(&prepared)?;
    let context = crate::workflow::WorkflowContext {
        schema_version: 1,
        run_id: crate::workflow::new_run_id()?,
        artifact_run_id: Some(artifact_run_id.clone()),
        kind: crate::workflow::WorkflowKind::Audit,
        workspace: root.clone(),
        scope,
        focus,
        output: out.clone(),
        format: format.name().to_string(),
        max_chunks,
        model,
        session_id: None,
        legacy_mode: None,
        output_before: crate::workflow::output_digest(&root, &out)?,
    };
    let descriptor = artifacts::describe_prepared(&root, &artifact_run_id)?;
    let message = prepared_workflow_message("oy-audit", &descriptor, &context)?;
    run_agent_workflow(&host, &root, "oy", message, &context)
}

pub(crate) fn review_workflow_command(
    target: Option<String>,
    focus: Vec<String>,
    out: PathBuf,
    max_chunks: usize,
) -> Result<i32> {
    if max_chunks == 0 {
        bail!("max_chunks must be greater than zero");
    }
    let root = config::oy_root()?;
    ensure_opencode_integration()?;
    let host = OpenCodeHost::selected_in(&root);
    require_supported_host(&host)?;
    config::resolve_workspace_output_path(&root, &out)?;
    let api = api::OpenCodeApi::new(&host);
    api.ensure_workflow(&root, "oy-review")?;
    let model = match selected_model() {
        Some(model) => Some(model),
        None => Some(api.default_model(&root)?),
    };
    let target = target.filter(|target| !target.trim().is_empty());
    let (scope, focus) = if let Some(target) = target.as_deref() {
        (crate::workflow::resolve_diff_scope(&root, target)?, focus)
    } else {
        crate::workflow::resolve_scope(&root, &focus)?
    };
    let path = match &scope {
        crate::workflow::WorkflowScope::Workspace { path } => path.clone(),
        crate::workflow::WorkflowScope::GitDiff { .. } => ".".to_string(),
    };
    let prepared = artifacts::prepare(
        &root,
        artifacts::PrepareRequest {
            kind: artifacts::Kind::Review,
            path,
            target,
            output: out.clone(),
            format: "markdown".to_string(),
            focus: focus.clone(),
            max_chunks,
            model: model.clone(),
        },
    )?;
    let artifact_run_id = prepared_run_id(&prepared)?;
    let context = crate::workflow::WorkflowContext {
        schema_version: 1,
        run_id: crate::workflow::new_run_id()?,
        artifact_run_id: Some(artifact_run_id.clone()),
        kind: crate::workflow::WorkflowKind::Review,
        workspace: root.clone(),
        scope,
        focus,
        output: out.clone(),
        format: "markdown".to_string(),
        max_chunks,
        model,
        session_id: None,
        legacy_mode: None,
        output_before: crate::workflow::output_digest(&root, &out)?,
    };
    let descriptor = artifacts::describe_prepared(&root, &artifact_run_id)?;
    let message = prepared_workflow_message("oy-review", &descriptor, &context)?;
    run_agent_workflow(&host, &root, "oy", message, &context)
}

pub(crate) fn enhance_workflow_command(
    review_target: Option<String>,
    focus: Vec<String>,
    audit_max_chunks: usize,
    review_max_chunks: usize,
    interactive: bool,
) -> Result<i32> {
    if audit_max_chunks == 0 || review_max_chunks == 0 {
        bail!("workflow chunk limits must be greater than zero");
    }
    let root = config::oy_root()?;
    ensure_opencode_integration()?;
    let host = OpenCodeHost::selected_in(&root);
    require_supported_host(&host)?;
    let scope = if let Some(target) = review_target.filter(|target| !target.trim().is_empty()) {
        crate::workflow::resolve_diff_scope(&root, &target)?
    } else {
        crate::workflow::WorkflowScope::Workspace {
            path: ".".to_string(),
        }
    };
    let context = crate::workflow::WorkflowContext {
        schema_version: 1,
        run_id: crate::workflow::new_run_id()?,
        artifact_run_id: None,
        kind: crate::workflow::WorkflowKind::Enhance,
        workspace: root.clone(),
        scope,
        focus,
        output: PathBuf::from("REVIEW.md"),
        format: "markdown".to_string(),
        max_chunks: audit_max_chunks.max(review_max_chunks),
        model: selected_model(),
        session_id: None,
        legacy_mode: None,
        output_before: None,
    };
    let message = format!(
        "Load the `oy-enhance` skill and execute it locally. Bound workflow request: {}",
        context.encode()?
    );
    let agent = "oy";
    let api = api::OpenCodeApi::new(&host);
    api.ensure_workflow(&root, "oy-enhance")?;
    let model = match selected_model() {
        Some(model) => Some(model),
        None => Some(api.default_model(&root)?),
    };
    let mut context = context;
    context.model = model;
    if interactive {
        let session = api.create_session(&root, agent, context.model.as_deref())?;
        context.session_id = Some(session.clone());
        let mut args = vec![
            "mini".to_string(),
            "--session".to_string(),
            session,
            "--agent".to_string(),
            agent.to_string(),
        ];
        if let Some(model) = context.model.clone() {
            args.extend(["--model".to_string(), model]);
        }
        args.extend(["--prompt".to_string(), message]);
        return run_opencode(&host, &root, args, Some(&context));
    }
    run_agent_workflow(&host, &root, agent, message, &context)
}

pub(crate) fn recover_workflow_command() -> Result<i32> {
    let root = config::oy_root()?;
    let retained = crate::workflow::retained(&root)?
        .ok_or_else(|| anyhow::anyhow!("no incomplete oy workflow exists for this workspace"))?;
    ensure_opencode_integration()?;
    let host = OpenCodeHost::selected_in(&root);
    require_supported_host(&host)?;
    let session = api::OpenCodeApi::new(&host)
        .find_session(&root, &format!("oy:{}", retained.run_id))?
        .or(retained.session_id.clone())
        .ok_or_else(|| anyhow::anyhow!("retained workflow session was not found"))?;
    let mut context = retained;
    context.session_id = Some(session);
    let agent = "oy";
    let message = if let Some(run_id) = context.artifact_run_id.as_deref() {
        let descriptor = artifacts::describe_prepared(&root, run_id)?;
        let skill = match context.kind {
            crate::workflow::WorkflowKind::Audit => "oy-audit",
            crate::workflow::WorkflowKind::Review => "oy-review",
            crate::workflow::WorkflowKind::Enhance => {
                unreachable!("enhance has no prepared artifact")
            }
        };
        prepared_workflow_message(skill, &descriptor, &context)?
    } else {
        format!(
            "Resume the bound oy workflow from its retained session and context. Bound workflow request: {}",
            context.encode()?
        )
    };
    run_agent_workflow(&host, &root, agent, message, &context)
}

fn require_supported_host(host: &OpenCodeHost) -> Result<()> {
    host.require_supported().map_err(anyhow::Error::msg)
}

pub(crate) fn runtime_health(host: &OpenCodeHost, root: &Path) -> Result<RuntimeHealth> {
    api::OpenCodeApi::new(host).runtime_health(root)
}

fn prepared_run_id(prepared: &serde_json::Value) -> Result<String> {
    prepared
        .get("run_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("artifact preparation did not return a run id"))
}

fn prepared_workflow_message(
    skill: &str,
    descriptor: &serde_json::Value,
    context: &crate::workflow::WorkflowContext,
) -> Result<String> {
    Ok(format!(
        "Load the `{skill}` skill and execute it locally. Oy has already prepared the immutable evidence below. Do not run prepare or finalize. Read the exact index and every listed chunk, then write only candidate_report and candidate_findings. The outer oy process will validate and finalize them.\nPrepared artifact request: {}\nBound workflow request: {}",
        serde_json::to_string(descriptor)?,
        context.encode()?
    ))
}

fn finalize_bound_artifact(
    root: &Path,
    context: &crate::workflow::WorkflowContext,
) -> Result<Option<serde_json::Value>> {
    context
        .artifact_run_id
        .as_deref()
        .map(|run_id| artifacts::finalize(root, run_id))
        .transpose()
}

fn run_agent_workflow(
    host: &OpenCodeHost,
    root: &Path,
    agent: &str,
    message: String,
    context: &crate::workflow::WorkflowContext,
) -> Result<i32> {
    let api = api::OpenCodeApi::new(host);
    let mut bound = context.clone();
    let session = match &bound.session_id {
        Some(session) => session.clone(),
        None => api.create_session(root, agent, bound.model.as_deref())?,
    };
    bound.session_id = Some(session.clone());
    api.rename_session(root, &session, &format!("oy:{}", bound.run_id))?;
    let lease = crate::workflow::WorkflowLease::acquire(&bound)?;
    let prefix = message
        .split_once("Bound workflow request:")
        .map_or(message.as_str(), |(prefix, _)| prefix);
    let prompt = format!("{prefix}Bound workflow request: {}", bound.encode()?);
    let result = match api.run_prompt(root, &session, &prompt) {
        Ok(result) => result,
        Err(error) => {
            ui::err_line(format_args!(
                "workflow {} interrupted; session {} and recovery context retained at {}",
                bound.run_id,
                session,
                lease.path().display()
            ));
            return Err(error);
        }
    };
    let finalization = match finalize_bound_artifact(root, &bound) {
        Ok(finalization) => finalization,
        Err(error) => {
            ui::err_line(format_args!(
                "workflow {} produced invalid candidates; recovery context retained at {}",
                bound.run_id,
                lease.path().display()
            ));
            return Err(error).context("failed finalizing prepared artifact workflow");
        }
    };
    let output_ok = finalization.is_some()
        || bound.kind == crate::workflow::WorkflowKind::Enhance
        || crate::workflow::output_digest(root, &bound.output)? != bound.output_before;
    if !output_ok {
        ui::err_line(format_args!(
            "workflow {} ended without writing {}; recovery context retained at {}",
            bound.run_id,
            bound.output.display(),
            lease.path().display()
        ));
        return Ok(1);
    }
    if ui::is_json() {
        ui::line(serde_json::to_string_pretty(&json!({
            "run_id": bound.run_id,
            "session_id": result.session_id,
            "admitted": result.admitted,
            "assistant": result.assistant,
            "text": result.text,
            "finalization": finalization,
        }))?);
    } else if !result.text.trim().is_empty() {
        ui::line(result.text);
    }
    lease.complete();
    Ok(0)
}

fn selected_model() -> Option<String> {
    std::env::var("OY_OPENCODE_MODEL")
        .ok()
        .filter(|model| !model.trim().is_empty())
}

fn push_session_args(args: &mut Vec<String>, continue_session: bool, resume: &str) {
    if continue_session {
        args.push("--continue".to_string());
    }
    if !resume.trim().is_empty() {
        args.extend(["--session".to_string(), resume.to_string()]);
    }
}

fn push_run_agent_args(args: &mut Vec<String>, agent: &str, auto: bool) {
    args.extend(["--agent".to_string(), agent.to_string()]);
    if auto {
        args.push("--auto".to_string());
    }
}

fn run_opencode(
    host: &OpenCodeHost,
    root: &Path,
    args: Vec<String>,
    context: Option<&crate::workflow::WorkflowContext>,
) -> Result<i32> {
    let lease = context
        .map(crate::workflow::WorkflowLease::acquire)
        .transpose()?;
    let mut command = Command::new(host.executable());
    command.args(args).current_dir(root);
    let status = command.status().with_context(|| {
        format!(
            "failed to launch {}; install it or set {OPENCODE_ENV} to an OpenCode executable",
            host.executable().display()
        )
    })?;
    let code = status.code().unwrap_or(1);
    if let Some(lease) = lease {
        if status.success() {
            let context = context.expect("workflow lease requires context");
            let finalization = match finalize_bound_artifact(root, context) {
                Ok(finalization) => finalization,
                Err(error) => {
                    ui::err_line(format_args!(
                        "workflow {} produced invalid candidates; recovery context retained at {}",
                        context.run_id,
                        lease.path().display()
                    ));
                    return Err(error).context("failed finalizing prepared artifact workflow");
                }
            };
            let output_ok = finalization.is_some()
                || context.kind == crate::workflow::WorkflowKind::Enhance
                || crate::workflow::output_digest(root, &context.output)? != context.output_before;
            if output_ok {
                lease.complete();
            } else {
                ui::err_line(format_args!(
                    "workflow {} ended without writing {}; recovery context retained at {}",
                    context.run_id,
                    context.output.display(),
                    lease.path().display()
                ));
                return Ok(1);
            }
        } else if let Some(context) = context {
            ui::err_line(format_args!(
                "workflow {} interrupted; session {} and recovery context retained at {}",
                context.run_id,
                context.session_id.as_deref().unwrap_or("unknown"),
                lease.path().display()
            ));
        }
    }
    Ok(code)
}

fn collect_prompt(parts: Vec<String>) -> Result<String> {
    if !parts.is_empty() {
        return Ok(parts.join(" "));
    }
    if std::io::stdin().is_terminal() {
        return Ok(String::new());
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    Ok(input.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_run_arguments_preserve_sessions_agents_and_auto() {
        let mut args = vec!["run".to_string()];
        push_session_args(&mut args, true, "");
        push_run_agent_args(&mut args, "oy", true);
        assert_eq!(args, vec!["run", "--continue", "--agent", "oy", "--auto"]);

        let mut resumed = Vec::new();
        push_session_args(&mut resumed, false, "ses_123");
        assert_eq!(resumed, vec!["--session", "ses_123"]);
    }

    #[test]
    fn bound_artifact_finalization_rejects_direct_report_mutation() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("lib.rs"), "fn checked() {}\n").unwrap();
        let prepared = artifacts::prepare(
            root.path(),
            artifacts::PrepareRequest {
                kind: artifacts::Kind::Audit,
                path: ".".to_string(),
                target: None,
                output: PathBuf::from("ISSUES.md"),
                format: "markdown".to_string(),
                focus: Vec::new(),
                max_chunks: 10,
                model: None,
            },
        )
        .unwrap();
        let artifact_run_id = prepared_run_id(&prepared).unwrap();
        std::fs::write(
            root.path()
                .join(prepared["candidate_report"].as_str().unwrap()),
            "# Audit Issues\n\n## Findings summary\n\nNo concrete findings.\n\n## Detailed findings\n",
        )
        .unwrap();
        std::fs::write(
            root.path()
                .join(prepared["candidate_findings"].as_str().unwrap()),
            "[]\n",
        )
        .unwrap();
        let context = crate::workflow::WorkflowContext {
            schema_version: 1,
            run_id: crate::workflow::new_run_id().unwrap(),
            artifact_run_id: Some(artifact_run_id),
            kind: crate::workflow::WorkflowKind::Audit,
            workspace: root.path().to_path_buf(),
            scope: crate::workflow::WorkflowScope::Workspace {
                path: ".".to_string(),
            },
            focus: Vec::new(),
            output: PathBuf::from("ISSUES.md"),
            format: "markdown".to_string(),
            max_chunks: 10,
            model: None,
            session_id: None,
            legacy_mode: None,
            output_before: None,
        };

        std::fs::write(root.path().join("ISSUES.md"), "unvalidated mutation\n").unwrap();
        let error = finalize_bound_artifact(root.path(), &context).unwrap_err();
        assert!(error.to_string().contains("output_changed"));

        std::fs::remove_file(root.path().join("ISSUES.md")).unwrap();
        let receipt = finalize_bound_artifact(root.path(), &context)
            .unwrap()
            .unwrap();
        assert_eq!(
            receipt["run_id"],
            context.artifact_run_id.as_deref().unwrap()
        );
        assert!(
            std::fs::read_to_string(root.path().join("ISSUES.md"))
                .unwrap()
                .contains("oy evidence: schema")
        );
    }
}
