//! Small adapter for OpenCode 2 API operations without a dedicated CLI command.

use super::host::OpenCodeHost;
use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::Value;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt as _;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const ERROR_DETAIL_BYTES: usize = 8 * 1024;
const API_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(super) struct OpenCodeApi<'a> {
    host: &'a OpenCodeHost,
}

#[derive(Debug, Clone)]
pub(super) struct Model {
    pub(super) id: String,
    pub(super) provider_id: String,
    pub(super) name: String,
    pub(super) raw: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowResult {
    pub session_id: String,
    pub admitted: Value,
    pub assistant: Value,
    pub text: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct RuntimeHealth {
    pub healthy: bool,
    pub service_version: bool,
    pub openapi: bool,
    pub location: bool,
    pub agents: bool,
    pub commands: bool,
    pub skills: bool,
    pub permissions: bool,
    pub mcp_connected: bool,
    pub models: bool,
    pub providers: bool,
    pub plugins: bool,
}

impl<'a> OpenCodeApi<'a> {
    pub(super) fn new(host: &'a OpenCodeHost) -> Self {
        Self { host }
    }

    pub(super) fn models(&self, directory: &Path) -> Result<Vec<Model>> {
        let directory_text = directory.to_str().ok_or_else(|| {
            anyhow!(
                "workspace directory is not valid UTF-8: {}",
                directory.display()
            )
        })?;
        let location = format!("location[directory]={directory_text}");
        let output = self.invoke(
            &["api", "v2.model.list", "--param", location.as_str()],
            directory,
        )?;
        let response: Value = serde_json::from_slice(&output.stdout)
            .context("OpenCode model API returned invalid JSON")?;
        reject_api_error(&response)?;
        response
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("OpenCode model API response is missing `data`"))?
            .iter()
            .cloned()
            .map(parse_model)
            .collect()
    }

    pub(crate) fn default_model(&self, directory: &Path) -> Result<String> {
        let response = self.operation("v2.model.default", directory)?;
        model_spec(
            response
                .get("data")
                .ok_or_else(|| anyhow!("OpenCode default model response is missing `data`"))?,
        )
    }

    pub(crate) fn find_session(&self, directory: &Path, title: &str) -> Result<Option<String>> {
        let directory_text = directory.to_str().ok_or_else(|| {
            anyhow!(
                "workspace directory is not valid UTF-8: {}",
                directory.display()
            )
        })?;
        let directory_param = format!("directory={directory_text}");
        let search_param = format!("search={title}");
        let output = self.invoke(
            &[
                "api",
                "v2.session.list",
                "--param",
                directory_param.as_str(),
                "--param",
                search_param.as_str(),
                "--param",
                "limit=1",
                "--param",
                "order=desc",
            ],
            directory,
        )?;
        let response: Value = serde_json::from_slice(&output.stdout)
            .context("OpenCode session list returned invalid JSON")?;
        reject_api_error(&response)?;
        Ok(response
            .pointer("/data/0/id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned))
    }

    pub(crate) fn create_session(
        &self,
        directory: &Path,
        agent: &str,
        model: Option<&str>,
    ) -> Result<String> {
        let directory_text = directory.to_str().ok_or_else(|| {
            anyhow!(
                "workspace directory is not valid UTF-8: {}",
                directory.display()
            )
        })?;
        let mut data = serde_json::json!({
            "agent": agent,
            "location": { "directory": directory_text }
        });
        if let Some(model) = model {
            data["model"] = model_ref(model)?;
        }
        let encoded = data.to_string();
        let output = self.invoke(
            &["api", "v2.session.create", "--data", encoded.as_str()],
            directory,
        )?;
        let response: Value = serde_json::from_slice(&output.stdout)
            .context("OpenCode session create returned invalid JSON")?;
        reject_api_error(&response)?;
        response
            .pointer("/data/id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("OpenCode session create response is missing id"))
    }

    pub(crate) fn rename_session(
        &self,
        directory: &Path,
        session_id: &str,
        title: &str,
    ) -> Result<()> {
        let session = format!("sessionID={session_id}");
        let data = serde_json::json!({ "title": title }).to_string();
        self.json_operation(
            "v2.session.rename",
            &[session.as_str()],
            Some(data.as_str()),
            directory,
            API_TIMEOUT,
        )?;
        Ok(())
    }

    pub(crate) fn run_prompt(
        &self,
        directory: &Path,
        session_id: &str,
        prompt: &str,
    ) -> Result<WorkflowResult> {
        let session_param = format!("sessionID={session_id}");
        let data = serde_json::json!({
            "text": prompt,
            "delivery": "queue",
            "resume": true
        })
        .to_string();
        let admitted = self.json_operation(
            "v2.session.prompt",
            &[session_param.as_str()],
            Some(data.as_str()),
            directory,
            API_TIMEOUT,
        )?;
        let admitted_id = admitted
            .pointer("/data/id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("OpenCode prompt admission is missing message id"))?
            .to_string();
        self.json_operation(
            "v2.session.wait",
            &[session_param.as_str()],
            None,
            directory,
            Duration::from_secs(2 * 60 * 60),
        )?;
        let messages = self.json_operation(
            "v2.message.list",
            &[session_param.as_str(), "limit=50", "order=desc"],
            None,
            directory,
            API_TIMEOUT,
        )?;
        let assistant = messages
            .get("data")
            .and_then(Value::as_array)
            .and_then(|messages| {
                let boundary = messages.iter().position(|message| {
                    message.get("id").and_then(Value::as_str) == Some(admitted_id.as_str())
                });
                boundary
                    .map_or(messages.as_slice(), |index| &messages[..index])
                    .iter()
                    .find(|message| {
                        message.get("type").and_then(Value::as_str) == Some("assistant")
                    })
            })
            .cloned()
            .ok_or_else(|| anyhow!("OpenCode session completed without an assistant message"))?;
        if let Some(error) = assistant.pointer("/error/message").and_then(Value::as_str) {
            bail!("OpenCode assistant failed: {error}");
        }
        let text = assistant
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<String>();
        Ok(WorkflowResult {
            session_id: session_id.to_string(),
            admitted,
            assistant,
            text,
        })
    }

    pub(crate) fn evict_location(&self, directory: &Path) -> Result<()> {
        let directory_text = directory.to_str().ok_or_else(|| {
            anyhow!(
                "workspace directory is not valid UTF-8: {}",
                directory.display()
            )
        })?;
        let location = format!("location[directory]={directory_text}");
        let output = self.invoke(
            &[
                "api",
                "v2.debug.location.evict",
                "--param",
                location.as_str(),
            ],
            directory,
        )?;
        if output.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let response: Value = serde_json::from_slice(&output.stdout)
            .context("OpenCode location eviction returned invalid JSON")?;
        reject_api_error(&response)
    }

    pub(crate) fn ensure_agent(&self, directory: &Path, agent: &str) -> Result<()> {
        for attempt in 0..8 {
            let response = self.operation("v2.agent.list", directory)?;
            if data_array(&response)?
                .iter()
                .any(|entry| entry.get("id").and_then(Value::as_str) == Some(agent))
            {
                return Ok(());
            }
            if attempt < 7 {
                std::thread::sleep(Duration::from_millis(250 * (attempt + 1) as u64));
            }
        }
        bail!(
            "OpenCode effective configuration is missing agent `{agent}`; run `oy setup` and retry"
        )
    }

    pub(crate) fn ensure_workflow(&self, directory: &Path, agent: &str, skill: &str) -> Result<()> {
        for attempt in 0..8 {
            let agents = self.operation("v2.agent.list", directory)?;
            let commands = self.operation("v2.command.list", directory)?;
            let skills = self.operation("v2.skill.list", directory)?;
            let mcp = self.operation("v2.mcp.list", directory)?;
            let agent_ok = exact_workflow_agent(data_array(&agents)?, agent, skill);
            let skill_ok = exact_skill(data_array(&skills)?, skill);
            let command_ok = workflow_commands(data_array(&commands)?);
            let mcp_ok = data_array(&mcp)?.iter().any(|entry| {
                entry.get("name").and_then(Value::as_str) == Some("oy")
                    && entry.pointer("/status/status").and_then(Value::as_str) == Some("connected")
            });
            if agent_ok && skill_ok && command_ok && mcp_ok {
                return Ok(());
            }
            if attempt < 7 {
                std::thread::sleep(Duration::from_millis(250 * (attempt + 1) as u64));
            }
        }
        bail!(
            "OpenCode effective configuration is missing skill `{skill}` or connected oy MCP; run `oy setup` and retry"
        )
    }

    pub(crate) fn runtime_health(&self, directory: &Path) -> Result<RuntimeHealth> {
        let health_response = self.unscoped_operation("v2.health.get", directory)?;
        let openapi_response = self.raw_get("/openapi.json", directory)?;
        let location_response = self.operation("v2.location.get", directory)?;
        let mut last = None;
        for attempt in 0..8 {
            let current = self.runtime_snapshot(
                directory,
                &health_response,
                &openapi_response,
                &location_response,
            )?;
            let ready = current.agents
                && current.commands
                && current.skills
                && current.permissions
                && current.mcp_connected
                && current.models
                && current.providers
                && current.plugins
                && current.openapi
                && current.location;
            if ready {
                return Ok(current);
            }
            last = Some(current);
            if attempt < 7 {
                std::thread::sleep(Duration::from_millis(250 * (attempt + 1) as u64));
            }
        }
        last.ok_or_else(|| anyhow!("OpenCode runtime health produced no result"))
    }

    fn runtime_snapshot(
        &self,
        directory: &Path,
        health: &Value,
        openapi: &Value,
        location: &Value,
    ) -> Result<RuntimeHealth> {
        let agent_response = self.operation("v2.agent.list", directory)?;
        let command_response = self.operation("v2.command.list", directory)?;
        let skill_response = self.operation("v2.skill.list", directory)?;
        let mcp_response = self.operation("v2.mcp.list", directory)?;
        let model_response = self.operation("v2.model.list", directory)?;
        let provider_response = self.operation("v2.provider.list", directory)?;
        let plugin_response = self.operation("v2.plugin.list", directory)?;
        let agents = data_array(&agent_response)?
            .iter()
            .filter_map(|value| value.get("id").and_then(Value::as_str))
            .collect::<std::collections::BTreeSet<_>>();
        let skills = data_array(&skill_response)?
            .iter()
            .filter_map(|value| {
                value
                    .get("id")
                    .or_else(|| value.get("name"))
                    .and_then(Value::as_str)
            })
            .collect::<std::collections::BTreeSet<_>>();
        let mcp = data_array(&mcp_response)?;
        let models = data_array(&model_response)?;
        let providers = data_array(&provider_response)?;
        let required_operations = [
            "v2.health.get",
            "v2.location.get",
            "v2.agent.list",
            "v2.command.list",
            "v2.skill.list",
            "v2.mcp.list",
            "v2.model.list",
            "v2.model.default",
            "v2.provider.list",
            "v2.plugin.list",
            "v2.session.create",
            "v2.session.list",
            "v2.session.rename",
            "v2.session.prompt",
            "v2.session.wait",
            "v2.message.list",
        ];
        let operation_ids = openapi
            .get("paths")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|paths| paths.values())
            .filter_map(Value::as_object)
            .flat_map(|methods| methods.values())
            .filter_map(|operation| operation.get("operationId").and_then(Value::as_str))
            .collect::<std::collections::BTreeSet<_>>();
        let location_ok = directory.to_str().is_some_and(|expected| {
            location.get("directory").and_then(Value::as_str) == Some(expected)
        });
        let requested_model = std::env::var("OY_OPENCODE_MODEL").ok();
        let model_ok = requested_model
            .as_deref()
            .map_or(!models.is_empty(), |requested| {
                models.iter().any(|model| model_matches(model, requested))
            });
        Ok(RuntimeHealth {
            healthy: health.get("healthy").and_then(Value::as_bool) == Some(true),
            service_version: health.get("version").and_then(Value::as_str)
                == self.host.installation_version(),
            openapi: required_operations
                .iter()
                .all(|operation| operation_ids.contains(operation)),
            location: location_ok,
            agents: [
                "oy",
                "oy-plan",
                "oy-edit",
                "oy-auto",
                "oy-auditor",
                "oy-reviewer",
                "oy-enhancer",
            ]
            .iter()
            .all(|name| {
                agents.contains(name)
                    && exact_workflow_agent(
                        data_array(&agent_response).unwrap_or(&Vec::new()),
                        name,
                        workflow_skill_for_agent(name),
                    )
            }),
            commands: workflow_commands(data_array(&command_response)?),
            skills: workflow_skills(data_array(&skill_response)?)
                && ["oy-audit", "oy-review", "oy-enhance"]
                    .iter()
                    .all(|name| skills.contains(name)),
            permissions: workflow_agent_permissions(data_array(&agent_response)?),
            mcp_connected: mcp.iter().any(|entry| {
                entry.get("name").and_then(Value::as_str) == Some("oy")
                    && entry.pointer("/status/status").and_then(Value::as_str) == Some("connected")
            }),
            models: model_ok,
            providers: !providers.is_empty(),
            plugins: plugin_response.get("data").is_some_and(Value::is_array),
        })
    }

    fn operation(&self, operation: &str, directory: &Path) -> Result<Value> {
        let directory_text = directory.to_str().ok_or_else(|| {
            anyhow!(
                "workspace directory is not valid UTF-8: {}",
                directory.display()
            )
        })?;
        let location = format!("location[directory]={directory_text}");
        let output = self.invoke(&["api", operation, "--param", location.as_str()], directory)?;
        let value: Value = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("OpenCode API {operation} returned invalid JSON"))?;
        reject_api_error(&value)?;
        Ok(value)
    }

    fn json_operation(
        &self,
        operation: &str,
        params: &[&str],
        data: Option<&str>,
        directory: &Path,
        timeout: Duration,
    ) -> Result<Value> {
        let mut owned = vec!["api".to_string(), operation.to_string()];
        for param in params {
            owned.extend(["--param".to_string(), (*param).to_string()]);
        }
        if let Some(data) = data {
            owned.extend(["--data".to_string(), data.to_string()]);
        }
        let borrowed = owned.iter().map(String::as_str).collect::<Vec<_>>();
        let output = self.invoke_with_timeout(&borrowed, directory, timeout)?;
        if output.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        let value: Value = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("OpenCode API {operation} returned invalid JSON"))?;
        reject_api_error(&value)?;
        Ok(value)
    }

    fn unscoped_operation(&self, operation: &str, directory: &Path) -> Result<Value> {
        let output = self.invoke(&["api", operation], directory)?;
        let value: Value = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("OpenCode API {operation} returned invalid JSON"))?;
        reject_api_error(&value)?;
        Ok(value)
    }

    fn raw_get(&self, path: &str, directory: &Path) -> Result<Value> {
        let output = self.invoke(&["api", "get", path], directory)?;
        serde_json::from_slice(&output.stdout)
            .with_context(|| format!("OpenCode API GET {path} returned invalid JSON"))
    }

    fn invoke(&self, args: &[&str], directory: &Path) -> Result<Output> {
        self.invoke_with_timeout(args, directory, API_TIMEOUT)
    }

    fn invoke_with_timeout(
        &self,
        args: &[&str],
        directory: &Path,
        timeout: Duration,
    ) -> Result<Output> {
        let mut stdout = tempfile::tempfile().context("failed to create OpenCode stdout buffer")?;
        let mut stderr = tempfile::tempfile().context("failed to create OpenCode stderr buffer")?;
        let mut child = Command::new(self.host.executable())
            .args(args)
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout.try_clone()?))
            .stderr(Stdio::from(stderr.try_clone()?))
            .spawn()
            .with_context(|| {
                format!("failed to invoke {} api", self.host.executable().display())
            })?;
        let status = match child.wait_timeout(timeout)? {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                bail!(
                    "OpenCode API request timed out after {} seconds",
                    timeout.as_secs()
                );
            }
        };
        let stdout = read_bounded(&mut stdout, "stdout")?;
        let stderr = read_bounded(&mut stderr, "stderr")?;
        let output = Output {
            status,
            stdout,
            stderr,
        };
        if output.stdout.len() > MAX_RESPONSE_BYTES || output.stderr.len() > MAX_RESPONSE_BYTES {
            bail!("OpenCode model API response exceeded the 16 MiB limit");
        }
        if !output.status.success() {
            let detail = text_detail(if output.stderr.is_empty() {
                &output.stdout
            } else {
                &output.stderr
            });
            bail!("OpenCode model API exited with {}: {detail}", output.status);
        }
        Ok(output)
    }
}

fn workflow_agent_permissions(agents: &[Value]) -> bool {
    [
        ("oy-auditor", "oy-audit"),
        ("oy-reviewer", "oy-review"),
        ("oy-enhancer", "oy-enhance"),
    ]
    .iter()
    .all(|(id, skill)| exact_workflow_agent(agents, id, skill))
}

fn exact_workflow_agent(agents: &[Value], id: &str, skill: &str) -> bool {
    let Some(agent) = agents
        .iter()
        .find(|agent| agent.get("id").and_then(Value::as_str) == Some(id))
    else {
        return false;
    };
    let expected_mode = if matches!(id, "oy-auditor" | "oy-reviewer" | "oy-enhancer") {
        "all"
    } else {
        "primary"
    };
    agent.get("mode").and_then(Value::as_str) == Some(expected_mode)
        && agent.get("system").and_then(Value::as_str).map(str::trim) == canonical_agent_body(id)
        && permissions_end_with(agent, expected_permission_suffix(id, skill))
}

fn workflow_skill_for_agent(id: &str) -> &str {
    match id {
        "oy-auditor" => "oy-audit",
        "oy-reviewer" => "oy-review",
        "oy-enhancer" => "oy-enhance",
        _ => "",
    }
}

fn canonical_agent_body(id: &str) -> Option<&str> {
    let source = match id {
        "oy" => super::OY_AGENT,
        "oy-plan" => super::OY_PLAN_AGENT,
        "oy-edit" => super::OY_EDIT_AGENT,
        "oy-auto" => super::OY_AUTO_AGENT,
        "oy-auditor" => super::OY_AUDITOR_AGENT,
        "oy-reviewer" => super::OY_REVIEWER_AGENT,
        "oy-enhancer" => super::OY_ENHANCER_AGENT,
        _ => return None,
    };
    source.splitn(3, "---").nth(2).map(str::trim)
}

fn expected_permission_suffix<'a>(id: &str, skill: &'a str) -> Vec<(&'a str, &'a str, &'a str)> {
    match id {
        "oy-auditor" => vec![
            ("*", "*", "deny"),
            ("execute", "*", "allow"),
            ("skill", skill, "allow"),
            ("oy_workflow_status", "*", "allow"),
            ("oy_repo_manifest", "*", "allow"),
            ("oy_repo_chunks", "*", "allow"),
            ("oy_existing_report", "*", "allow"),
            ("oy_sighthound", "*", "allow"),
            ("oy_render_audit_report", "*", "allow"),
        ],
        "oy-reviewer" => vec![
            ("*", "*", "deny"),
            ("execute", "*", "allow"),
            ("skill", skill, "allow"),
            ("oy_workflow_status", "*", "allow"),
            ("oy_git_diff_input", "*", "allow"),
            ("oy_repo_chunks", "*", "allow"),
            ("oy_repo_manifest", "*", "allow"),
            ("oy_existing_report", "*", "allow"),
            ("oy_render_review_report", "*", "allow"),
        ],
        "oy-enhancer" => vec![
            ("edit", "*", "allow"),
            ("shell", "*", "deny"),
            ("skill", skill, "allow"),
            ("oy_workflow_status", "*", "allow"),
        ],
        "oy" => vec![("edit", "*", "ask"), ("shell", "*", "ask")],
        "oy-plan" => vec![
            ("edit", "*", "deny"),
            ("shell", "*", "deny"),
            ("lsp", "*", "deny"),
        ],
        "oy-edit" => vec![("edit", "*", "allow"), ("shell", "*", "ask")],
        "oy-auto" => vec![("edit", "*", "allow"), ("shell", "*", "allow")],
        _ => Vec::new(),
    }
}

fn permissions_end_with(agent: &Value, expected: Vec<(&str, &str, &str)>) -> bool {
    let Some(permissions) = agent.get("permissions").and_then(Value::as_array) else {
        return false;
    };
    if expected.is_empty() || permissions.len() < expected.len() {
        return false;
    }
    permissions[permissions.len() - expected.len()..]
        .iter()
        .zip(expected)
        .all(|(actual, (action, resource, effect))| {
            actual.get("action").and_then(Value::as_str) == Some(action)
                && actual.get("resource").and_then(Value::as_str) == Some(resource)
                && actual.get("effect").and_then(Value::as_str) == Some(effect)
        })
}

fn workflow_commands(commands: &[Value]) -> bool {
    [
        ("oy-audit", "oy-auditor", "oy-audit"),
        ("oy-review", "oy-reviewer", "oy-review"),
        ("oy-enhance", "oy-enhancer", "oy-enhance"),
    ]
    .iter()
    .all(|(name, agent, skill)| {
        commands.iter().any(|command| {
            command.get("name").and_then(Value::as_str) == Some(*name)
                && command.get("agent").and_then(Value::as_str) == Some(*agent)
                && command.get("template").and_then(Value::as_str)
                    == Some(match *skill {
                        "oy-audit" => {
                            "Load the `oy-audit` skill and execute it locally.\n\n$ARGUMENTS"
                        }
                        "oy-review" => {
                            "Load the `oy-review` skill and execute it locally.\n\n$ARGUMENTS"
                        }
                        _ => "Load the `oy-enhance` skill and execute it locally.\n\n$ARGUMENTS",
                    })
        })
    })
}

fn workflow_skills(skills: &[Value]) -> bool {
    ["oy-audit", "oy-review", "oy-enhance"]
        .iter()
        .all(|id| exact_skill(skills, id))
}

fn exact_skill(skills: &[Value], id: &str) -> bool {
    let source = match id {
        "oy-audit" => super::OY_AUDIT_SKILL,
        "oy-review" => super::OY_REVIEW_SKILL,
        "oy-enhance" => super::OY_ENHANCE_SKILL,
        _ => return false,
    };
    let expected = source.splitn(3, "---").nth(2).map(str::trim);
    skills.iter().any(|skill| {
        skill
            .get("id")
            .or_else(|| skill.get("name"))
            .and_then(Value::as_str)
            == Some(id)
            && skill.get("content").and_then(Value::as_str).map(str::trim) == expected
    })
}

fn data_array(value: &Value) -> Result<&Vec<Value>> {
    value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("OpenCode API response is missing array `data`"))
}

fn read_bounded(file: &mut std::fs::File, stream: &str) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed reading OpenCode model API {stream}"))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        bail!("OpenCode model API {stream} exceeded the 16 MiB limit");
    }
    Ok(bytes)
}

fn parse_model(raw: Value) -> Result<Model> {
    let required = |field: &str| {
        raw.get(field)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("OpenCode model is missing string `{field}`"))
    };
    Ok(Model {
        id: required("id")?,
        provider_id: required("providerID")?,
        name: required("name")?,
        raw,
    })
}

fn model_spec(raw: &Value) -> Result<String> {
    let provider = raw
        .get("providerID")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("model is missing providerID"))?;
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("model is missing id"))?;
    Ok(match raw.get("variant").and_then(Value::as_str) {
        Some(variant) => format!("{provider}/{id}#{variant}"),
        None => format!("{provider}/{id}"),
    })
}

fn model_matches(raw: &Value, requested: &str) -> bool {
    let (base, variant) = requested
        .split_once('#')
        .map_or((requested, None), |(base, variant)| (base, Some(variant)));
    if model_spec(raw).ok().as_deref() != Some(base) {
        return false;
    }
    variant.is_none_or(|variant| {
        raw.get("variants")
            .and_then(Value::as_array)
            .is_some_and(|variants| {
                variants.iter().any(|entry| {
                    entry.as_str() == Some(variant)
                        || entry.get("id").and_then(Value::as_str) == Some(variant)
                })
            })
    })
}

fn model_ref(model: &str) -> Result<Value> {
    let (provider_id, model) = model
        .split_once('/')
        .ok_or_else(|| anyhow!("OpenCode model must use provider/model format"))?;
    let (id, variant) = model
        .split_once('#')
        .map_or((model, None), |(id, variant)| (id, Some(variant)));
    let mut value = serde_json::json!({ "providerID": provider_id, "id": id });
    if let Some(variant) = variant {
        value["variant"] = Value::String(variant.to_string());
    }
    Ok(value)
}

fn reject_api_error(response: &Value) -> Result<()> {
    let Some(tag) = response.get("_tag").and_then(Value::as_str) else {
        return Ok(());
    };
    let message = response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("no error detail");
    bail!("OpenCode model API failed ({tag}): {message}")
}

fn text_detail(bytes: &[u8]) -> String {
    let end = bytes.len().min(ERROR_DETAIL_BYTES);
    let mut detail = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
    if bytes.len() > end {
        detail.push_str(" [truncated]");
    }
    detail
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_model_and_tagged_error() {
        let model = parse_model(json!({
            "id": "gpt-test",
            "providerID": "openai",
            "name": "GPT Test"
        }))
        .unwrap();
        assert_eq!(model.provider_id, "openai");
        assert!(
            reject_api_error(&json!({
                "_tag": "ServiceUnavailableError",
                "message": "catalog unavailable"
            }))
            .is_err()
        );
    }
}
