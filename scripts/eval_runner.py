#!/usr/bin/env python3
"""Local oy prompt-evaluation runner.

This script intentionally stays outside the Rust binary. It orchestrates public
repo clones and opencode-backed `oy audit`/`oy review` runs under `.tmp/eval/`,
then validates report shape and records a small scorecard. It does not call a
model directly and is not intended for default CI.
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import json
import os
import re
import shutil
import subprocess
import time
import tomllib
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CORPUS = REPO_ROOT / "docs" / "eval-corpus.toml"
EVAL_ROOT = REPO_ROOT / ".tmp" / "eval"
GENERATED_REPORT_MARKER = "Generated with [oy-cli]"


@dataclasses.dataclass(frozen=True)
class Task:
    id: str
    lane: str
    workflow: str
    repo: str
    url: str
    checkout: str
    enabled: bool = True
    license: str = ""
    target: str = ""
    focus: str = ""
    max_chunks: int = 80
    fetch_refs: tuple[str, ...] = ()
    source: str = ""
    notes: str = ""
    expected_paths: tuple[str, ...] = ()
    quality_keywords: tuple[str, ...] = ()
    min_quality_matches: int = 0
    max_findings: int | None = None

    @classmethod
    def from_raw(cls, raw: dict[str, Any]) -> "Task":
        required = ["id", "lane", "workflow", "repo", "url", "checkout"]
        missing = [name for name in required if not str(raw.get(name, "")).strip()]
        if missing:
            raise ValueError(f"task missing required keys {missing}: {raw!r}")
        workflow = str(raw["workflow"])
        if workflow not in {"audit", "review"}:
            raise ValueError(f"{raw['id']}: workflow must be audit or review")
        max_findings = raw.get("max_findings")
        return cls(
            id=str(raw["id"]),
            lane=str(raw["lane"]),
            workflow=workflow,
            repo=str(raw["repo"]),
            url=str(raw["url"]),
            checkout=str(raw["checkout"]),
            enabled=bool(raw.get("enabled", True)),
            license=str(raw.get("license", "")),
            target=str(raw.get("target", "")),
            focus=str(raw.get("focus", "")),
            max_chunks=int(raw.get("max_chunks", 80)),
            fetch_refs=tuple(str(value) for value in raw.get("fetch_refs", [])),
            source=str(raw.get("source", "")),
            notes=str(raw.get("notes", "")),
            expected_paths=tuple(str(value) for value in raw.get("expected_paths", [])),
            quality_keywords=tuple(str(value) for value in raw.get("quality_keywords", [])),
            min_quality_matches=int(raw.get("min_quality_matches", 0)),
            max_findings=None if max_findings is None else int(max_findings),
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    sub = parser.add_subparsers(dest="command", required=True)

    list_parser = sub.add_parser("list", help="List corpus tasks")
    list_parser.add_argument("--all", action="store_true", help="Include disabled tasks")

    sub.add_parser("validate", help="Validate corpus schema only")

    run_parser = sub.add_parser("run", help="Run selected eval tasks")
    run_parser.add_argument("--task", action="append", default=[], help="Task id to run; repeatable")
    run_parser.add_argument("--all", action="store_true", help="Run disabled tasks too")
    run_parser.add_argument("--run-id", default="", help="Stable run id; defaults to UTC timestamp")
    run_parser.add_argument("--model-slug", default="model", help="Short label included in default run id")
    run_parser.add_argument(
        "--opencode-model",
        default="",
        help="Run eval tasks through opencode -m provider/model instead of the oy wrapper",
    )
    run_parser.add_argument("--dry-run", action="store_true", help="Print planned commands without cloning or running opencode")
    run_parser.add_argument("--skip-build", action="store_true", help="Do not run cargo build before eval")
    run_parser.add_argument("--strict-quality", action="store_true", help="Exit non-zero when quality checks fail")

    compare_parser = sub.add_parser("compare", help="Compare two completed run summary.json files")
    compare_parser.add_argument("baseline", type=Path)
    compare_parser.add_argument("candidate", type=Path)

    args = parser.parse_args()
    tasks = load_tasks(args.corpus)
    if args.command == "list":
        list_tasks(tasks, include_disabled=args.all)
        return 0
    if args.command == "validate":
        print(f"valid corpus: {args.corpus} ({len(tasks)} tasks)")
        return 0
    if args.command == "compare":
        compare_runs(args.baseline, args.candidate)
        return 0
    if args.command == "run":
        selected = select_tasks(tasks, args.task, include_disabled=args.all)
        return run_tasks(selected, args)
    raise AssertionError(args.command)


def load_tasks(path: Path) -> list[Task]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    tasks = [Task.from_raw(raw) for raw in data.get("tasks", [])]
    seen: set[str] = set()
    duplicates: set[str] = set()
    for task in tasks:
        if task.id in seen:
            duplicates.add(task.id)
        seen.add(task.id)
    if duplicates:
        raise ValueError(f"duplicate task ids: {sorted(duplicates)}")
    if not tasks:
        raise ValueError(f"no tasks in {path}")
    return tasks


def list_tasks(tasks: list[Task], *, include_disabled: bool) -> None:
    for task in tasks:
        if not include_disabled and not task.enabled:
            continue
        status = "enabled" if task.enabled else "disabled"
        target = f" -> {task.target[:12]}" if task.target else ""
        print(f"{task.id}\t{status}\t{task.lane}\t{task.workflow}\t{task.repo}@{task.checkout[:12]}{target}")


def select_tasks(tasks: list[Task], ids: list[str], *, include_disabled: bool) -> list[Task]:
    by_id = {task.id: task for task in tasks}
    if ids:
        missing = [task_id for task_id in ids if task_id not in by_id]
        if missing:
            raise ValueError(f"unknown task ids: {missing}")
        return [by_id[task_id] for task_id in ids]
    return [task for task in tasks if task.enabled or include_disabled]


def run_tasks(tasks: list[Task], args: argparse.Namespace) -> int:
    model_slug = args.model_slug
    if model_slug == "model" and args.opencode_model:
        model_slug = args.opencode_model
    run_id = args.run_id or f"{utc_stamp()}-{slug(model_slug)}"
    run_dir = EVAL_ROOT / "runs" / run_id
    repos_dir = EVAL_ROOT / "repos"
    env = os.environ.copy()
    env["PATH"] = f"{REPO_ROOT / 'target' / 'debug'}{os.pathsep}{env.get('PATH', '')}"
    env["OY_EVAL_OPENCODE_MODEL"] = args.opencode_model

    if args.dry_run:
        print(f"dry run id: {run_id}")
        for task in tasks:
            print_plan(task, run_dir, args.opencode_model)
        return 0

    EVAL_ROOT.mkdir(parents=True, exist_ok=True)
    repos_dir.mkdir(parents=True, exist_ok=True)
    run_dir.mkdir(parents=True, exist_ok=True)

    if not args.skip_build:
        checked_run(["cargo", "build", "--locked"], cwd=REPO_ROOT, env=env)

    results = []
    started = time.time()
    for task in tasks:
        result = run_one_task(task, repos_dir, run_dir, env)
        results.append(result)
        write_summary(run_dir, run_id, results, started)

    write_summary(run_dir, run_id, results, started)
    protocol_failed = any(not result["protocol_ok"] for result in results)
    quality_failed = any(not result["quality_ok"] for result in results)
    print(f"wrote summary: {run_dir / 'summary.json'}")
    if protocol_failed or (args.strict_quality and quality_failed):
        return 1
    return 0


def run_one_task(task: Task, repos_dir: Path, run_dir: Path, env: dict[str, str]) -> dict[str, Any]:
    print(f"==> {task.id}")
    repo_dir = repos_dir / task.id
    task_run_dir = run_dir / task.id
    task_run_dir.mkdir(parents=True, exist_ok=True)
    started = time.time()
    command_status = 0
    error = ""
    try:
        ensure_repo(task, repo_dir, env)
        out_rel = Path(".oy-eval") / run_dir.name / task.id / report_filename(task)
        checked_run(["oy", "setup", "--workspace"], cwd=repo_dir, env=task_env(env, repo_dir))
        command = eval_command(task, out_rel, env.get("OY_EVAL_OPENCODE_MODEL", ""))
        checked_run(command, cwd=repo_dir, env=task_env(env, repo_dir))
        report_path = repo_dir / out_rel
        copied_report = task_run_dir / report_path.name
        shutil.copy2(report_path, copied_report)
        validation = validate_report(task, copied_report)
        mutations = unexpected_mutations(repo_dir, env)
        protocol_ok = bool(validation["protocol_ok"] and not mutations)
        quality_ok = bool(validation["quality_ok"])
    except subprocess.CalledProcessError as exc:
        command_status = exc.returncode
        error = str(exc)
        protocol_ok = False
        quality_ok = False
        validation = empty_validation(error)
        mutations = []
    except Exception as exc:  # noqa: BLE001 - keep eval running after one bad task.
        command_status = 1
        error = str(exc)
        protocol_ok = False
        quality_ok = False
        validation = empty_validation(error)
        mutations = []
    elapsed = round(time.time() - started, 3)
    result = {
        "id": task.id,
        "lane": task.lane,
        "workflow": task.workflow,
        "repo": task.repo,
        "source": task.source,
        "checkout": task.checkout,
        "target": task.target,
        "opencode_model": env.get("OY_EVAL_OPENCODE_MODEL", ""),
        "elapsed_seconds": elapsed,
        "command_status": command_status,
        **validation,
        "protocol_ok": protocol_ok,
        "quality_ok": quality_ok,
        "unexpected_mutations": mutations,
        "error": error,
    }
    status = "ok" if protocol_ok and quality_ok else "check"
    print(f"<== {task.id}: {status} ({elapsed}s)")
    return result


def ensure_repo(task: Task, repo_dir: Path, env: dict[str, str]) -> None:
    if not repo_dir.exists():
        checked_run(["git", "clone", "--no-tags", "--filter=blob:none", task.url, str(repo_dir)], cwd=REPO_ROOT, env=env)
    checked_run(["git", "remote", "set-url", "origin", task.url], cwd=repo_dir, env=env)
    refs = task.fetch_refs or (task.checkout,)
    for ref in refs:
        checked_run(["git", "fetch", "--no-tags", "--depth", "1", "origin", ref], cwd=repo_dir, env=env)
    checked_run(["git", "checkout", "--detach", task.checkout], cwd=repo_dir, env=env)
    checked_run(["git", "clean", "-fd", "--", "."], cwd=repo_dir, env=env)
    checked_run(["git", "reset", "--hard", task.checkout], cwd=repo_dir, env=env)
    if task.target:
        checked_run(["git", "fetch", "--no-tags", "--depth", "1", "origin", task.target], cwd=repo_dir, env=env)


def eval_command(task: Task, out_rel: Path, opencode_model: str = "") -> list[str]:
    if opencode_model:
        return opencode_command(task, out_rel, opencode_model)
    return oy_command(task, out_rel)


def oy_command(task: Task, out_rel: Path) -> list[str]:
    if task.workflow == "audit":
        command = ["oy", "audit", "--out", str(out_rel), "--max-chunks", str(task.max_chunks)]
        if task.focus:
            command.append(task.focus)
        return command
    command = ["oy", "review", "--out", str(out_rel), "--max-chunks", str(task.max_chunks)]
    if task.focus:
        command.extend(["--focus", task.focus])
    if task.target:
        command.append(task.target)
    return command


def opencode_command(task: Task, out_rel: Path, model: str) -> list[str]:
    command_name = "oy-audit" if task.workflow == "audit" else "oy-review"
    return [
        "opencode",
        "run",
        "--model",
        model,
        "--command",
        command_name,
        workflow_message(task, out_rel, model=model),
    ]


def workflow_message(task: Task, out_rel: Path, *, model: str = "") -> str:
    if task.workflow == "audit":
        message = "Run an oy audit for this workspace."
        if task.focus:
            message += f" Focus: {sentence(task.focus)}"
        message += f" Write output to {out_rel}. Use max_chunks {task.max_chunks}. Format: markdown."
        if model:
            message += f" Model: {model}. Pass this exact model string to the report renderer."
        return message
    message = "Run an oy review."
    if task.target:
        message += f" Target: {task.target}."
    if task.focus:
        message += f" Focus: {sentence(task.focus)}"
    message += f" Write output to {out_rel}. Use max_chunks {task.max_chunks}."
    if model:
        message += f" Model: {model}. Pass this exact model string to the report renderer."
    return message


def sentence(text: str) -> str:
    text = text.strip()
    if text.endswith((".", "?", "!")):
        return text
    return f"{text}."


def validate_report(task: Task, report_path: Path) -> dict[str, Any]:
    text = report_path.read_text(encoding="utf-8")
    expected_title = "# Audit Issues" if task.workflow == "audit" else "# Code Quality Review"
    protocol_errors = []
    if expected_title not in text:
        protocol_errors.append(f"missing title {expected_title!r}")
    if GENERATED_REPORT_MARKER not in text:
        protocol_errors.append("missing oy transparency line")
    findings, findings_error = parse_findings(text)
    if findings_error:
        protocol_errors.append(findings_error)

    lower = text.lower()
    quality_matches = [value for value in task.quality_keywords if value.lower() in lower]
    path_matches = [value for value in task.expected_paths if value.lower() in lower]
    quality_errors = []
    total_quality_matches = len(set(quality_matches + path_matches))
    if total_quality_matches < task.min_quality_matches:
        quality_errors.append(
            f"quality matches {total_quality_matches} < required {task.min_quality_matches}"
        )
    if task.max_findings is not None and len(findings) > task.max_findings:
        quality_errors.append(f"findings {len(findings)} > max_findings {task.max_findings}")

    return {
        "report": str(report_path.relative_to(REPO_ROOT)),
        "protocol_ok": not protocol_errors,
        "quality_ok": not quality_errors,
        "protocol_errors": protocol_errors,
        "quality_errors": quality_errors,
        "findings_count": len(findings),
        "quality_matches": quality_matches,
        "expected_path_matches": path_matches,
    }


def parse_findings(text: str) -> tuple[list[Any], str]:
    match = re.search(r"```(?:json\s+)?oy-findings\s*(.*?)```", text, flags=re.DOTALL)
    if not match:
        return [], "missing json oy-findings block"
    try:
        parsed = json.loads(match.group(1))
    except json.JSONDecodeError as exc:
        return [], f"invalid oy-findings JSON: {exc}"
    if not isinstance(parsed, list):
        return [], "oy-findings JSON is not an array"
    return parsed, ""


def unexpected_mutations(repo_dir: Path, env: dict[str, str]) -> list[str]:
    output = checked_run(
        ["git", "status", "--porcelain=v1", "--untracked-files=all"],
        cwd=repo_dir,
        env=env,
        capture=True,
    )
    unexpected = []
    for line in output.splitlines():
        path = line[3:] if len(line) > 3 else line
        if path.startswith(".opencode/") or path.startswith(".oy-eval/"):
            continue
        unexpected.append(line)
    return unexpected


def write_summary(run_dir: Path, run_id: str, results: list[dict[str, Any]], started: float) -> None:
    summary = {
        "run_id": run_id,
        "started_at": dt.datetime.fromtimestamp(started, tz=dt.UTC).isoformat(),
        "elapsed_seconds": round(time.time() - started, 3),
        "oy_commit": git_output(["git", "rev-parse", "HEAD"], cwd=REPO_ROOT),
        "opencode_model": results[0].get("opencode_model", "") if results else "",
        "results": results,
    }
    (run_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    (run_dir / "summary.md").write_text(markdown_summary(summary), encoding="utf-8")


def markdown_summary(summary: dict[str, Any]) -> str:
    lines = [f"# oy eval {summary['run_id']}", ""]
    lines.append(f"- oy commit: `{summary['oy_commit']}`")
    if summary.get("opencode_model"):
        lines.append(f"- opencode model: `{summary['opencode_model']}`")
    lines.append(f"- elapsed: {summary['elapsed_seconds']}s")
    lines.append("")
    lines.append("| Task | Lane | Workflow | Protocol | Quality | Findings | Notes |")
    lines.append("|---|---|---|---:|---:|---:|---|")
    for result in summary["results"]:
        notes = "; ".join(result.get("protocol_errors", []) + result.get("quality_errors", []))
        lines.append(
            "| {id} | {lane} | {workflow} | {protocol} | {quality} | {findings} | {notes} |".format(
                id=result["id"],
                lane=result["lane"],
                workflow=result["workflow"],
                protocol="✅" if result["protocol_ok"] else "❌",
                quality="✅" if result["quality_ok"] else "⚠️",
                findings=result.get("findings_count", 0),
                notes=notes.replace("|", "\\|"),
            )
        )
    lines.append("")
    return "\n".join(lines)


def compare_runs(baseline_path: Path, candidate_path: Path) -> None:
    baseline = load_summary(baseline_path)
    candidate = load_summary(candidate_path)
    baseline_results = {result["id"]: result for result in baseline["results"]}
    candidate_results = {result["id"]: result for result in candidate["results"]}
    print("| Task | Baseline | Candidate | Verdict |")
    print("|---|---|---|---|")
    for task_id in sorted(set(baseline_results) | set(candidate_results)):
        old = baseline_results.get(task_id)
        new = candidate_results.get(task_id)
        verdict = compare_result(old, new)
        print(f"| {task_id} | {result_cell(old)} | {result_cell(new)} | {verdict} |")


def load_summary(path: Path) -> dict[str, Any]:
    summary_path = path / "summary.json" if path.is_dir() else path
    return json.loads(summary_path.read_text(encoding="utf-8"))


def compare_result(old: dict[str, Any] | None, new: dict[str, Any] | None) -> str:
    if old is None or new is None:
        return "inconclusive"
    if old.get("protocol_ok") and not new.get("protocol_ok"):
        return "worse"
    if not old.get("protocol_ok") and new.get("protocol_ok"):
        return "better"
    if old.get("quality_ok") and not new.get("quality_ok"):
        return "worse"
    if not old.get("quality_ok") and new.get("quality_ok"):
        return "better"
    old_matches = len(old.get("quality_matches", [])) + len(old.get("expected_path_matches", []))
    new_matches = len(new.get("quality_matches", [])) + len(new.get("expected_path_matches", []))
    if new_matches > old_matches:
        return "better"
    if new_matches < old_matches:
        return "worse"
    return "same"


def result_cell(result: dict[str, Any] | None) -> str:
    if result is None:
        return "missing"
    protocol = "P✅" if result.get("protocol_ok") else "P❌"
    quality = "Q✅" if result.get("quality_ok") else "Q⚠️"
    findings = result.get("findings_count", 0)
    return f"{protocol} {quality} f={findings}"


def print_plan(task: Task, run_dir: Path, opencode_model: str = "") -> None:
    out_rel = Path(".oy-eval") / run_dir.name / task.id / report_filename(task)
    print(f"\n[{task.id}] {task.workflow} {task.repo}")
    print(f"  clone: {task.url}")
    for ref in task.fetch_refs or (task.checkout,):
        print(f"  fetch: {ref}")
    print(f"  checkout: {task.checkout}")
    print(f"  command: {shell_join(eval_command(task, out_rel, opencode_model))}")


def report_filename(task: Task) -> str:
    return "ISSUES.md" if task.workflow == "audit" else "REVIEW.md"


def task_env(env: dict[str, str], repo_dir: Path) -> dict[str, str]:
    out = env.copy()
    out["OY_ROOT"] = str(repo_dir)
    return out


def checked_run(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    capture: bool = False,
) -> str:
    kwargs: dict[str, Any] = {
        "cwd": cwd,
        "env": env,
        "check": True,
        "text": True,
    }
    if capture:
        kwargs.update({"stdout": subprocess.PIPE, "stderr": subprocess.PIPE})
    print(f"$ {shell_join(command)}", flush=True)
    completed = subprocess.run(command, **kwargs)
    return completed.stdout if capture else ""


def git_output(command: list[str], *, cwd: Path) -> str:
    return subprocess.check_output(command, cwd=cwd, text=True).strip()


def shell_join(command: list[str]) -> str:
    return " ".join(shell_quote(part) for part in command)


def shell_quote(value: str) -> str:
    if re.fullmatch(r"[A-Za-z0-9_./:=+-]+", value):
        return value
    return "'" + value.replace("'", "'\\''") + "'"


def empty_validation(error: str) -> dict[str, Any]:
    return {
        "report": "",
        "protocol_ok": False,
        "quality_ok": False,
        "protocol_errors": [error],
        "quality_errors": ["task did not produce a valid report"],
        "findings_count": 0,
        "quality_matches": [],
        "expected_path_matches": [],
    }


def utc_stamp() -> str:
    return dt.datetime.now(dt.UTC).strftime("%Y%m%dT%H%M%SZ")


def slug(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "-", value).strip("-") or "model"


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BrokenPipeError:
        raise SystemExit(1)
