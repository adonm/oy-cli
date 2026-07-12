use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::sync::LazyLock;
use std::time::Duration;

use super::super::ToolContext;
use super::super::args::{SighthoundAnalysis, SighthoundArgs};
use super::super::external::{ExternalCommand, discover};
use super::output::SighthoundOutput;
use super::paths::resolve_read_path;

const SCAN_TIMEOUT: Duration = Duration::from_secs(240);
const SCAN_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const MAX_FINDINGS: usize = 200;
const RETURNED_FINDINGS_BYTE_LIMIT: usize = 192 * 1024;
const MAX_RETURNED_STRING_CHARS: usize = 1_000;
const MAX_RETURNED_ARRAY_ITEMS: usize = 20;
const LANGUAGES: &[&str] = &[
    "python",
    "javascript",
    "typescript",
    "tsx",
    "java",
    "php",
    "csharp",
    "go",
    "ruby",
    "html",
    "django",
];

static SIGHTHOUND: LazyLock<std::result::Result<ExternalCommand, String>> =
    LazyLock::new(|| discover_sighthound().map_err(|err| format!("{err:#}")));

pub(crate) fn has_sighthound() -> bool {
    sighthound_command().is_ok()
}

pub(crate) fn tool_sighthound(ctx: &ToolContext, args: SighthoundArgs) -> Result<Value> {
    let target = resolve_read_path(ctx, &args.path)?;
    if !target.is_dir() {
        bail!("Sighthound scan path is not a directory: {}", args.path);
    }
    let language = args
        .language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    if let Some(language) = language.as_deref()
        && !LANGUAGES.contains(&language)
    {
        bail!(
            "unsupported Sighthound language {language:?}; expected one of: {}",
            LANGUAGES.join(", ")
        );
    }
    if !(1..=MAX_FINDINGS).contains(&args.max_findings) {
        bail!("max_findings must be between 1 and {MAX_FINDINGS}");
    }

    let root = ctx
        .root()
        .canonicalize()
        .context("failed to resolve workspace root")?;
    let relative_target = target
        .strip_prefix(&root)
        .context("Sighthound scan path escaped the workspace")?;
    let relative_target = if relative_target.as_os_str().is_empty() {
        OsString::from(".")
    } else {
        relative_target.as_os_str().to_os_string()
    };
    let command = sighthound_command()?;
    let scan = run_sighthound(
        &root,
        command,
        relative_target,
        args.analysis,
        language.as_deref(),
        args.include_test_fixtures,
    )?;
    let prepared = prepare_findings(scan.findings, args.max_findings)?;

    Ok(serde_json::to_value(SighthoundOutput {
        path: args.path,
        format: "sighthound-json",
        command: command.name().to_string(),
        analysis: args.analysis.name(),
        effective_analysis: scan.effective_analysis.name(),
        status: scan.status,
        language,
        finding_count: prepared.total,
        returned_count: prepared.returned,
        truncated: prepared.truncated,
        findings: Value::Array(prepared.findings),
    })?)
}

fn sighthound_command() -> Result<&'static ExternalCommand> {
    SIGHTHOUND.as_ref().map_err(|err| anyhow!(err.clone()))
}

fn discover_sighthound() -> Result<ExternalCommand> {
    discover("Sighthound", "OY_SIGHTHOUND", &["sighthound"], |command| {
        let output = command.probe(&["--version"])?;
        output.require_success(command)?;
        if !String::from_utf8_lossy(&output.stdout)
            .to_ascii_lowercase()
            .contains("sighthound")
        {
            bail!("version output does not identify Sighthound");
        }
        Ok(())
    })
}

fn run_sighthound(
    root: &std::path::Path,
    command: &ExternalCommand,
    target: OsString,
    analysis: SighthoundAnalysis,
    language: Option<&str>,
    include_test_fixtures: bool,
) -> Result<ScanOutput> {
    let output = run_sighthound_once(
        root,
        command,
        &target,
        analysis,
        language,
        include_test_fixtures,
    )?;
    if output.status.success() {
        return Ok(ScanOutput {
            findings: parse_findings(&output.stdout)?,
            effective_analysis: analysis,
            status: "ok",
        });
    }

    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if stderr.contains("no supported files found") {
        return Ok(ScanOutput {
            findings: Vec::new(),
            effective_analysis: analysis,
            status: "no_supported_files",
        });
    }
    if matches!(analysis, SighthoundAnalysis::All) && stderr.contains("no taint flow rules found") {
        let fallback = run_sighthound_once(
            root,
            command,
            &target,
            SighthoundAnalysis::Simple,
            language,
            include_test_fixtures,
        )?;
        fallback.require_success(command)?;
        return Ok(ScanOutput {
            findings: parse_findings(&fallback.stdout)?,
            effective_analysis: SighthoundAnalysis::Simple,
            status: "simple_fallback_no_taint_rules",
        });
    }

    output.require_success(command)?;
    unreachable!("failed Sighthound output should have returned an error")
}

fn run_sighthound_once(
    root: &std::path::Path,
    command: &ExternalCommand,
    target: &OsStr,
    analysis: SighthoundAnalysis,
    language: Option<&str>,
    include_test_fixtures: bool,
) -> Result<super::super::external::ExternalOutput> {
    let mut args = vec![
        OsString::from("--output-format"),
        OsString::from("json"),
        OsString::from("--single-threaded"),
        OsString::from("--threads"),
        OsString::from("1"),
    ];
    match analysis {
        SighthoundAnalysis::All => {}
        SighthoundAnalysis::Simple => args.push(OsString::from("--simple-analysis")),
        SighthoundAnalysis::Taint => args.push(OsString::from("--taint-analysis")),
    }
    if include_test_fixtures {
        args.push(OsString::from("--include-test-fixtures"));
    }
    args.push(target.to_os_string());
    args.extend(language.map(OsString::from));

    command.run(root, args, SCAN_TIMEOUT, SCAN_OUTPUT_LIMIT)
}

fn parse_findings(stdout: &[u8]) -> Result<Vec<Value>> {
    let mut findings: Vec<Value> = serde_json::from_slice(stdout)
        .context("Sighthound output was not a JSON findings array")?;
    findings.sort_by(compare_findings);
    Ok(findings)
}

struct ScanOutput {
    findings: Vec<Value>,
    effective_analysis: SighthoundAnalysis,
    status: &'static str,
}

struct PreparedFindings {
    findings: Vec<Value>,
    total: usize,
    returned: usize,
    truncated: bool,
}

fn prepare_findings(findings: Vec<Value>, max_findings: usize) -> Result<PreparedFindings> {
    let total = findings.len();
    let mut returned = Vec::new();
    let mut encoded_bytes = 0;
    for mut finding in findings.into_iter().take(max_findings) {
        bound_returned_value(&mut finding);
        let finding_bytes = serde_json::to_vec_pretty(&finding)
            .context("failed to measure Sighthound finding output")?
            .len();
        if encoded_bytes + finding_bytes > RETURNED_FINDINGS_BYTE_LIMIT {
            break;
        }
        encoded_bytes += finding_bytes;
        returned.push(finding);
    }
    let returned_count = returned.len();
    Ok(PreparedFindings {
        findings: returned,
        total,
        returned: returned_count,
        truncated: returned_count < total,
    })
}

fn bound_returned_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            let mut chars = text.chars();
            let prefix = chars
                .by_ref()
                .take(MAX_RETURNED_STRING_CHARS)
                .collect::<String>();
            if chars.next().is_some() {
                *text = format!("{prefix}…");
            }
        }
        Value::Array(items) => {
            items.truncate(MAX_RETURNED_ARRAY_ITEMS);
            items.iter_mut().for_each(bound_returned_value);
        }
        Value::Object(fields) => fields.values_mut().for_each(bound_returned_value),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn compare_findings(left: &Value, right: &Value) -> Ordering {
    string_field(left, "file")
        .cmp(string_field(right, "file"))
        .then_with(|| integer_field(left, "line").cmp(&integer_field(right, "line")))
        .then_with(|| integer_field(left, "column").cmp(&integer_field(right, "column")))
        .then_with(|| string_field(left, "finding_type").cmp(string_field(right, "finding_type")))
        .then_with(|| string_field(left, "severity").cmp(string_field(right, "severity")))
        .then_with(|| string_field(left, "snippet").cmp(string_field(right, "snippet")))
}

fn string_field<'a>(value: &'a Value, field: &str) -> &'a str {
    value.get(field).and_then(Value::as_str).unwrap_or("")
}

fn integer_field(value: &Value, field: &str) -> u64 {
    value.get(field).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_executable(dir: &std::path::Path, body: &str) -> ExternalCommand {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let executable = dir.join("sighthound");
        fs::write(&executable, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
        super::super::super::external::test_command(&executable)
    }

    #[test]
    fn findings_have_stable_source_order() {
        let mut findings = [
            json!({"file": "b.py", "line": 1, "column": 1, "finding_type": "B"}),
            json!({"file": "a.py", "line": 9, "column": 1, "finding_type": "A"}),
            json!({"file": "a.py", "line": 2, "column": 1, "finding_type": "C"}),
        ];

        findings.sort_by(compare_findings);

        assert_eq!(findings[0]["line"], 2);
        assert_eq!(findings[1]["line"], 9);
        assert_eq!(findings[2]["file"], "b.py");
    }

    #[test]
    fn scanner_call_uses_fixed_json_and_single_threaded_flags() {
        let dir = tempfile::tempdir().unwrap();
        let command = test_executable(
            dir.path(),
            "[ \"$*\" = \"--output-format json --single-threaded --threads 1 --simple-analysis --include-test-fixtures . python\" ] || exit 23\nprintf '%s\\n' '[{\"file\":\"b.py\",\"line\":2},{\"file\":\"a.py\",\"line\":1}]'",
        );

        let scan = run_sighthound(
            dir.path(),
            &command,
            OsString::from("."),
            SighthoundAnalysis::Simple,
            Some("python"),
            true,
        )
        .unwrap();

        assert_eq!(scan.findings[0]["file"], "a.py");
        assert_eq!(scan.findings[1]["file"], "b.py");
        assert_eq!(scan.effective_analysis.name(), "simple");
        assert_eq!(scan.status, "ok");
    }

    #[test]
    fn unsupported_scope_returns_an_empty_nonfatal_result() {
        let dir = tempfile::tempdir().unwrap();
        let command = test_executable(
            dir.path(),
            "printf '%s\\n' 'Error: No supported files found' >&2\nexit 1",
        );

        let scan = run_sighthound(
            dir.path(),
            &command,
            OsString::from("."),
            SighthoundAnalysis::All,
            None,
            false,
        )
        .unwrap();

        assert!(scan.findings.is_empty());
        assert_eq!(scan.status, "no_supported_files");
    }

    #[test]
    fn all_analysis_falls_back_when_language_has_no_taint_rules() {
        let dir = tempfile::tempdir().unwrap();
        let command = test_executable(
            dir.path(),
            "case \" $* \" in\n  *\" --simple-analysis \"*) printf '%s\\n' '[]' ;;\n  *) printf '%s\\n' 'Error: No taint flow rules found' >&2; exit 1 ;;\nesac",
        );

        let scan = run_sighthound(
            dir.path(),
            &command,
            OsString::from("."),
            SighthoundAnalysis::All,
            Some("html"),
            false,
        )
        .unwrap();

        assert!(scan.findings.is_empty());
        assert_eq!(scan.effective_analysis.name(), "simple");
        assert_eq!(scan.status, "simple_fallback_no_taint_rules");
    }

    #[test]
    fn returned_findings_are_count_and_size_bounded() {
        let findings = (0..250)
            .map(|line| json!({"file": "a.py", "line": line, "snippet": "x".repeat(5_000)}))
            .collect();

        let prepared = prepare_findings(findings, MAX_FINDINGS).unwrap();

        assert_eq!(prepared.total, 250);
        assert!(prepared.returned <= MAX_FINDINGS);
        assert!(prepared.truncated);
        assert!(
            prepared.findings[0]["snippet"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 1_001
        );
    }
}
