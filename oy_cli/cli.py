from __future__ import annotations

import argparse
import concurrent.futures
from contextlib import contextmanager
from threading import Lock, RLock
from datetime import UTC, datetime
import hashlib
import os
import re
import subprocess
import sys
from tempfile import TemporaryDirectory
import time
from pathlib import Path, PurePosixPath

import defopt
from prompt_toolkit.history import FileHistory
from pygments import lex
from pygments.lexers import get_lexer_for_filename
from pygments.token import Comment, String
from pygments.util import ClassNotFound

from . import runtime as rt
from . import tools as tools_lib
from .agent import (
    Transcript,
    add_user,
    checkpoint,
    clear_transcript,
    new_agent_state,
    prepared_tokens,
    rollback,
    run_agent,
    run_turn,
    set_system_prompt,
    session_tokens,
    transcript,
    transcript_with_system_prompt,
    undo_last_turn,
)

from .runtime import (
    AUDIT_PHASE1_SYSTEM_PROMPT,
    AUDIT_PHASE2_SYSTEM_PROMPT,
    AUDIT_PHASE3_SYSTEM_PROMPT,
    AUDIT_SYSTEM_PROMPT,
    LOGIC_AUDIT_PHASE1_SYSTEM_PROMPT,
    LOGIC_AUDIT_PHASE2_SYSTEM_PROMPT,
    LOGIC_AUDIT_PHASE3_SYSTEM_PROMPT,
    LOGIC_AUDIT_SYSTEM_PROMPT,
    active_system_prompt,
    ask_system_prompt,
    read_only_tool_registry,
    session_text,
)
from .tools import _iter_files, sloc, tool_specs


def _audit_transcript(*, max_context_tokens: int) -> Transcript:
    return transcript(
        messages=[],
        max_context_tokens=max_context_tokens,
        max_message_tokens=max_context_tokens,
    )


_AUDIT_PHASES = (
    ("phase1", "backlog", "Use the per-file SLOC report to build and refresh the audit backlog."),
    ("phase2", "review", "Use tiktoken-sized chunks to review backlog files, update progress, and merge findings into ISSUES.md."),
    ("phase3", "summary", "Summarise and reorganise ISSUES.md around the most critical, actionable findings, then close the audit."),
)
_AUDIT_PHASE_IDS = tuple(phase_id for phase_id, _, _ in _AUDIT_PHASES)
_AUDIT_STATE_VERSION = 6
_AUDIT_REVIEW_STATUSES = {"reviewed", "done", "flagged", "skipped"}
_AUDIT_DONE_STATUSES = {"done", "completed"}
_AUDIT_RETRYABLE_STATUSES = {"in_progress"}
_AUDIT_BINARY_SAMPLE_BYTES = 8192
_AUDIT_SESSION_DIRNAME = "audits"
_AUDIT_MAX_ITERATIONS = 512
_AUDIT_MAX_STALLS = 2
_AUDIT_MAX_AGENT_FAILURES = 2
_AUDIT_DEFAULT_MODE = "default"
_AUDIT_LOGIC_MODE = "logic"
_AUDIT_VALID_MODES = {_AUDIT_DEFAULT_MODE, _AUDIT_LOGIC_MODE}
_AUDIT_SCOPE_DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
def _audit_schema(key: str) -> str:
    return session_text("audit", key)


def _audit_scope_key(scope: dict[str, str] | None) -> str:
    if not isinstance(scope, dict):
        return "all"
    parts = []
    if isinstance(scope.get("ref"), str) and scope.get("ref"):
        parts.append(f"ref:{scope['ref']}")
    if isinstance(scope.get("date"), str) and scope.get("date"):
        parts.append(f"date:{scope['date']}")
    return ",".join(parts) if parts else "all"


def _audit_scope_suffix(scope: dict[str, str] | None) -> str:
    key = _audit_scope_key(scope)
    if key == "all":
        return ""
    digest = hashlib.sha1(key.encode("utf-8")).hexdigest()[:12]
    return f"-from-{digest}"


def _audit_scope_note(scope: dict[str, str] | None) -> str | None:
    if not isinstance(scope, dict):
        return None
    parts = []
    if isinstance(scope.get("ref"), str) and scope.get("ref"):
        parts.append(f"commit `{scope['ref']}`")
    if isinstance(scope.get("date"), str) and scope.get("date"):
        parts.append(f"date `{scope['date']}`")
    if not parts:
        return None
    return "Scoped with `--from` after " + " and ".join(parts) + "."


def _audit_parse_scope(from_: str = "") -> dict[str, str]:
    tokens = [part for part in str(from_ or "").split() if part]
    scope: dict[str, str] = {}
    for token in tokens:
        lowered = token.lower()
        if lowered.startswith("ref:"):
            value = token[4:].strip()
            if not value:
                raise ValueError("Invalid audit --from value: empty ref")
            if scope.get("ref") and scope.get("ref") != value:
                raise ValueError("Invalid audit --from value: multiple commit refs provided")
            scope["ref"] = value
            continue
        if lowered.startswith("date:"):
            value = token[5:].strip()
            if not _AUDIT_SCOPE_DATE_RE.fullmatch(value):
                raise ValueError("Invalid audit --from value: dates must use YYYY-MM-DD")
            if scope.get("date") and scope.get("date") != value:
                raise ValueError("Invalid audit --from value: multiple dates provided")
            scope["date"] = value
            continue
        if _AUDIT_SCOPE_DATE_RE.fullmatch(token):
            if scope.get("date") and scope.get("date") != token:
                raise ValueError("Invalid audit --from value: multiple dates provided")
            scope["date"] = token
            continue
        if scope.get("ref") and scope.get("ref") != token:
            raise ValueError("Invalid audit --from value: multiple commit refs provided")
        scope["ref"] = token
    return scope


def _audit_git_changed_paths(workspace: Path, *, scope: dict[str, str] | None = None) -> set[str] | None:
    if not isinstance(scope, dict) or not scope:
        return None
    since_ref = str(scope.get("ref") or "").strip()
    since_date = str(scope.get("date") or "").strip()
    cmd = ["git", "-C", str(workspace), "log", "--name-only", "--format="]
    if since_date:
        cmd.append(f"--since={since_date} 00:00:00")
    if since_ref:
        cmd.append(f"{since_ref}..HEAD")
    cmd.extend(["--", "."])
    try:
        result = rt.run_cmd(cmd, timeout=20)
    except Exception as exc:
        raise ValueError(f"Could not inspect git history for audit scope: {exc}") from exc
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "git log failed"
        raise ValueError(f"Could not inspect git history for audit scope: {detail}")
    changed = {
        line.strip()
        for line in result.stdout.splitlines()
        if line.strip() and not line.startswith((" ", "	"))
    }
    return changed


def _audit_filter_paths(paths: list[str], changed_paths: set[str] | None) -> list[str]:
    if changed_paths is None:
        return list(paths)
    return [path for path in paths if path in changed_paths]


def _audit_h2(title: str) -> str:
    return f"{_audit_schema('report_h2_prefix')}{title}"


def _audit_h3(title: str) -> str:
    return f"{_audit_schema('report_h3_prefix')}{title}"


def _audit_non_finding_headings() -> set[str]:
    return {
        _audit_schema('inbox_title'),
        _audit_schema('findings_title'),
        _audit_schema('concise_rollups_title'),
        _audit_schema('resolved_title'),
        _audit_schema('short_audit_log_title'),
        _audit_schema('summary_title'),
    }


def _audit_preserved_section_titles() -> set[str]:
    return {
        _audit_schema('resolved_title'),
        _audit_schema('short_audit_log_title'),
    }
_AUDIT_DOC_DIRS = {"doc", "docs", "documentation"}
_AUDIT_DOC_SUFFIXES = {".adoc", ".asciidoc", ".md", ".mdx", ".org", ".rst"}
_AUDIT_DOC_NAME_PREFIXES = (
    "authors",
    "changelog",
    "contributing",
    "history",
    "license",
    "licence",
    "notice",
    "readme",
    "release-notes",
    "releases",
    "security",
)
_AUDIT_LOCKFILE_NAMES = {
    "bun.lock",
    "bun.lockb",
    "cargo.lock",
    "composer.lock",
    "flake.lock",
    "gemfile.lock",
    "go.sum",
    "mix.lock",
    "npm-shrinkwrap.json",
    "package-lock.json",
    "package.resolved",
    "packages.lock.json",
    "pipfile.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "podfile.lock",
    "pubspec.lock",
    "uv.lock",
    "yarn.lock",
}
_AUDIT_LOGIC_SEARCH_EXCLUDES = (
    "*.adoc",
    "*.asciidoc",
    "*.md",
    "*.mdx",
    "*.org",
    "*.rst",
    "AUTHORS",
    "CHANGELOG",
    "CONTRIBUTING",
    "HISTORY",
    "LICENSE",
    "LICENCE",
    "NOTICE",
    "README",
    "RELEASE",
    "RELEASE-NOTES",
    "RELEASES",
    "SECURITY",
    "doc/**",
    "docs/**",
    "documentation/**",
    "**/doc/**",
    "**/docs/**",
    "**/documentation/**",
    "bun.lock",
    "bun.lockb",
    "cargo.lock",
    "composer.lock",
    "flake.lock",
    "gemfile.lock",
    "go.sum",
    "mix.lock",
    "npm-shrinkwrap.json",
    "package-lock.json",
    "package.resolved",
    "packages.lock.json",
    "pipfile.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "podfile.lock",
    "pubspec.lock",
    "uv.lock",
    "yarn.lock",
)
_AUDIT_COMMENT_GAP_RE = re.compile(r"\n{3,}")


_SESSIONS_DIR: Path | None = None

_ASK_RULES = "no bash or file changes; public webfetch still allowed"
_ASK_USAGE = f"Usage: `/ask <question>` — research the codebase with {_ASK_RULES}."
_ASK_MODE_NOTE = f"research mode ({_ASK_RULES})"
_CHAT_COMMAND_HELP = (
    ("/help", "show this help"),
    ("/tokens", "show context usage"),
    ("/model [filter]", "show or switch model"),
    ("/debug", "toggle debug logging"),
    ("/yolo", "allow all tools for the rest of this session"),
    ("/ask <question>", f"research-only query ({_ASK_RULES})"),
    ("/audit [focus]", "run or resume a security/complexity audit"),
    ("/audit-logic [focus]", "run or resume a logic-focused audit that ignores docs/comments"),
    ("/save [name]", "save session transcript"),
    ("/load [name]", "load a saved session"),
    ("/undo", "remove the last prompt and its follow-up messages"),
    ("/clear", "reset conversation (keeps system prompt)"),
    ("/quit", "end session"),
    ("/exit", "end session"),
)
_PROMPT_COMMANDS = [command for command, _ in _CHAT_COMMAND_HELP if command != "/exit"]
_DEFAULT_RENOVATE_CONFIG = '''{
  "extends": ["config:recommended", "helpers:pinGitHubActionDigests"]
}
'''
_CHAT_ACTIONS = {
    "/model": "model",
    "/debug": "debug",
    "/yolo": "yolo",
    "/ask": "ask",
    "/audit": "audit",
    "/audit-logic": "audit_logic",
    "/save": "save",
    "/load": "load",
}
_CHAT_ACTIONS_WITH_ARGS = {"model", "ask", "audit", "audit_logic", "save", "load"}


def _sessions_dir() -> Path:
    return _SESSIONS_DIR or (rt.CONFIG_PATH.parent / "sessions")


def _session_file(name: str) -> Path:
    safe_name = "".join(
        char if char.isascii() and (char.isalnum() or char in "_-") else "_"
        for char in name
    )
    return _sessions_dir() / f"{safe_name}.json"


def _list_saved_sessions() -> list[Path]:
    sessions_dir = rt._ensure_private_dir(_sessions_dir())
    return sorted(
        sessions_dir.glob("*.json"), key=lambda path: path.stat().st_mtime, reverse=True
    )


def _resolve_saved_session(name: str | None) -> Path | None:
    sessions = _list_saved_sessions()
    if not sessions:
        return None
    if not name:
        return sessions[0]
    target = None
    if name.isdigit():
        index = int(name) - 1
        if 0 <= index < len(sessions):
            target = sessions[index]
    if target is None:
        candidate = _session_file(name)
        if candidate.exists():
            target = candidate
    if target is None:
        matches = [path for path in sessions if name.lower() in path.stem.lower()]
        if len(matches) == 1:
            target = matches[0]
        elif matches:
            rt.abort(f"Ambiguous session match for {rt._fmt('inline', name)}.")
    return target


def _apply_session_title(workspace: Path, model_spec: str) -> None:
    _, model = rt.split_model_spec(model_spec)
    _set_terminal_title(f"oy · {model} · {workspace.name}")


def _task_text(task: tuple[str, ...]) -> str:
    return " ".join(task) or (sys.stdin.read().strip() if not rt.has_tty_stdin() else "")


@contextmanager
def _ralph_run_env(current_model: str):
    saved_env = {
        "OY_MODEL": os.environ.get("OY_MODEL"),
        "OY_SHIM": os.environ.get("OY_SHIM"),
        "OY_CONFIG": os.environ.get("OY_CONFIG"),
        "OY_LOCK_MODEL": os.environ.get("OY_LOCK_MODEL"),
    }
    rt.command_env.cache_clear()
    try:
        shim, model = rt.split_model_spec(current_model)
        os.environ["OY_MODEL"] = model
        if shim:
            os.environ["OY_SHIM"] = shim
        else:
            os.environ.pop("OY_SHIM", None)
        os.environ["OY_LOCK_MODEL"] = "1"
        with TemporaryDirectory(prefix="oy-ralph-") as tmpdir:
            os.environ["OY_CONFIG"] = str(Path(tmpdir) / "config.json")
            rt.command_env.cache_clear()
            yield
    finally:
        for name, value in saved_env.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
        rt.command_env.cache_clear()


def _transcript_data(transcript: Transcript) -> dict[str, object]:
    return {
        "messages": list(transcript["messages"]),
        "max_context_tokens": transcript["max_context_tokens"],
        "max_message_tokens": transcript["max_message_tokens"],
    }


def _load_transcript(data: object) -> Transcript:
    if not isinstance(data, dict):
        raise ValueError("Invalid transcript payload")
    messages = data.get("messages")
    if not isinstance(messages, list):
        raise ValueError("Invalid transcript messages")
    max_context_tokens = data.get("max_context_tokens", rt.MAX_CONTEXT_TOKENS)
    max_message_tokens = data.get("max_message_tokens", rt.BUDGETS["message_tokens"])
    if not isinstance(max_context_tokens, int) or not isinstance(
        max_message_tokens, int
    ):
        raise ValueError("Invalid transcript token limits")
    return transcript(
        messages=list(messages),
        max_context_tokens=max_context_tokens,
        max_message_tokens=max_message_tokens,
    )


def load_system_prompt(system_file, interactive, *, agent: str = "default"):
    base = active_system_prompt(interactive)
    profile = rt.agent_profile(agent)
    parts = [base]
    if profile["system_prompt_suffix"]:
        parts.append(profile["system_prompt_suffix"])
    if system_file is None:
        return "\n\n".join(parts)
    if not system_file.exists():
        rt.abort(f"System file does not exist: {rt._fmt('inline', system_file)}")
    if system_file.is_dir():
        rt.abort(f"System file is a directory: {rt._fmt('inline', system_file)}")
    try:
        extra = system_file.read_text(encoding="utf-8")
    except OSError as exc:
        rt.abort(f"Could not read system file {rt._fmt('inline', system_file)}: {exc}")
    return "\n\n".join([*parts, extra])


def _set_terminal_title(title: str) -> None:
    if sys.stderr.isatty():
        sys.stderr.write(f"\033]0;{title}\007")
        sys.stderr.flush()


def _print_session_intro(heading: str, session_info, **extras) -> None:
    lines = [
        f"## {heading}",
        "",
        f"- workspace: {rt._fmt('inline', session_info['workspace'])}",
        f"- model: {rt._fmt('inline', session_info['model'])}",
        f"- agent: {rt._fmt('inline', session_info['agent'])}",
        f"- mode: {rt._fmt('inline', 'interactive' if session_info['interactive'] else 'non-interactive')}",
    ]
    if session_info["system_file"] is not None:
        extras["system file"] = session_info["system_file"].resolve()
    for key, value in extras.items():
        if value is not None:
            lines.append(f"- {key}: {rt._fmt('inline', value)}")
    if rt._debug_log_path:
        lines.append(f"- debug log: {rt._fmt('inline', rt._debug_log_path)}")
    rt._print(value="\n".join(lines), err=True)
    _apply_session_title(session_info["workspace"], session_info["model"])


def _ensure_renovate_config(workspace: Path) -> Path:
    path = workspace / "renovate.json"
    if path.exists():
        if not path.is_file():
            raise RuntimeError(
                f"Renovate config path is not a file: {rt._fmt('inline', path)}"
            )
        return path
    try:
        path.write_text(_DEFAULT_RENOVATE_CONFIG, encoding="utf-8")
    except OSError as exc:
        raise RuntimeError(
            f"Could not create default Renovate config {rt._fmt('inline', path)}: {exc}"
        ) from exc
    rt._note(f"created default Renovate config: {path.name}", tag="note")
    return path



def _ensure_tmp_dir(workspace: Path) -> Path:
    path = workspace / ".tmp"
    if path.exists() and not path.is_dir():
        raise RuntimeError(f"Temporary path is not a directory: {rt._fmt('inline', path)}")
    existed = path.exists()
    path.mkdir(parents=True, exist_ok=True)
    if not existed:
        rt._note("created .tmp/", tag="note")
    return path


def _tmp_is_gitignored(lines: list[str]) -> bool:
    patterns = {line.strip() for line in lines if line.strip() and not line.lstrip().startswith("#")}
    return any(pattern in patterns for pattern in (".tmp", ".tmp/", "/.tmp", "/.tmp/"))


def _ensure_tmp_gitignored(workspace: Path) -> None:
    path = workspace / ".gitignore"
    if path.exists() and not path.is_file():
        raise RuntimeError(f"Gitignore path is not a file: {rt._fmt('inline', path)}")
    try:
        existing = path.read_text(encoding="utf-8") if path.exists() else ""
    except OSError as exc:
        raise RuntimeError(
            f"Could not read {rt._fmt('inline', path)}: {exc}"
        ) from exc
    if _tmp_is_gitignored(existing.splitlines()):
        return
    updated = existing if not existing or existing.endswith("\n") else f"{existing}\n"
    updated += ".tmp/\n"
    try:
        path.write_text(updated, encoding="utf-8")
    except OSError as exc:
        raise RuntimeError(
            f"Could not update {rt._fmt('inline', path)}: {exc}"
        ) from exc
    rt._note("updated .gitignore: .tmp/", tag="note")


def _audit_sessions_dir() -> Path:
    return rt._ensure_private_dir(_sessions_dir() / _AUDIT_SESSION_DIRNAME)


def _audit_workspace_key(workspace: Path) -> str:
    digest = hashlib.sha1(str(workspace.resolve()).encode("utf-8")).hexdigest()[:12]
    safe_name = "".join(
        char if char.isascii() and (char.isalnum() or char in "_.-") else "_"
        for char in workspace.name
    ).strip("._") or "workspace"
    return f"{safe_name}-{digest}"


def _audit_session_path(workspace: Path, *, mode: str = _AUDIT_DEFAULT_MODE, scope: dict[str, str] | None = None) -> Path:
    suffix = "" if mode == _AUDIT_DEFAULT_MODE else f"-{mode}"
    return _audit_sessions_dir() / f"{_audit_workspace_key(workspace)}{suffix}{_audit_scope_suffix(scope)}.toon"


def _audit_empty_phase(phase_id: str, label: str) -> dict[str, object]:
    return {"id": phase_id, "label": label, "status": "pending", "notes": []}


def _audit_section(mode: str) -> str:
    return "audit_logic" if mode == _AUDIT_LOGIC_MODE else "audit"


def _audit_command(mode: str, *, chat: bool = False) -> str:
    if mode == _AUDIT_LOGIC_MODE:
        return "/audit-logic" if chat else "oy audit-logic"
    return "/audit" if chat else "oy audit"


def _audit_title(mode: str) -> str:
    return "Audit Logic" if mode == _AUDIT_LOGIC_MODE else "Audit"


def _audit_mode_name(mode: str) -> str:
    return "logic audit" if mode == _AUDIT_LOGIC_MODE else "audit"


def _audit_system_prompt_for_mode(mode: str, *, phase: str | None = None) -> str:
    if mode == _AUDIT_LOGIC_MODE:
        return {
            None: LOGIC_AUDIT_SYSTEM_PROMPT,
            'phase1': LOGIC_AUDIT_PHASE1_SYSTEM_PROMPT,
            'phase2': LOGIC_AUDIT_PHASE2_SYSTEM_PROMPT,
            'phase3': LOGIC_AUDIT_PHASE3_SYSTEM_PROMPT,
        }[phase]
    return {
        None: AUDIT_SYSTEM_PROMPT,
        'phase1': AUDIT_PHASE1_SYSTEM_PROMPT,
        'phase2': AUDIT_PHASE2_SYSTEM_PROMPT,
        'phase3': AUDIT_PHASE3_SYSTEM_PROMPT,
    }[phase]


def _audit_is_doc_path(path: str) -> bool:
    posix = PurePosixPath(path)
    lowered_parts = [part.lower() for part in posix.parts]
    if any(part in _AUDIT_DOC_DIRS for part in lowered_parts[:-1]):
        return True
    name = posix.name.lower()
    if posix.suffix.lower() in _AUDIT_DOC_SUFFIXES:
        return True
    return posix.suffix == "" and name in _AUDIT_DOC_NAME_PREFIXES


def _audit_is_lockfile(path: str) -> bool:
    return PurePosixPath(path).name.lower() in _AUDIT_LOCKFILE_NAMES


def _audit_should_skip_path(path: str, *, mode: str) -> bool:
    if path.startswith('.tmp/'):
        return True
    if mode != _AUDIT_LOGIC_MODE:
        return False
    return _audit_is_doc_path(path) or _audit_is_lockfile(path)


def _audit_logic_search_exclude(exclude: str | list[str] | None) -> str | list[str] | None:
    if exclude is None:
        return list(_AUDIT_LOGIC_SEARCH_EXCLUDES)
    if isinstance(exclude, str):
        return [exclude, *_AUDIT_LOGIC_SEARCH_EXCLUDES]
    return [*exclude, *_AUDIT_LOGIC_SEARCH_EXCLUDES]


def _audit_mask_text(text: str) -> str:
    return "".join("\n" if char == "\n" else " " for char in text)


def _audit_strip_comments(path: str, text: str) -> str:
    try:
        lexer = get_lexer_for_filename(path, stripnl=False, ensurenl=False)
    except ClassNotFound:
        return text
    parts: list[str] = []
    for token_type, value in lex(text, lexer):
        if token_type in Comment or token_type in String.Doc:
            parts.append(_audit_mask_text(value))
        else:
            parts.append(value)
    cleaned = "".join(parts)
    cleaned = "\n".join(line.rstrip() for line in cleaned.splitlines())
    cleaned = _AUDIT_COMMENT_GAP_RE.sub("\n\n", cleaned)
    return cleaned.strip()


def _audit_render_text(path: str, text: str, *, mode: str) -> str:
    if mode != _AUDIT_LOGIC_MODE:
        return text
    stripped = _audit_strip_comments(path, text)
    return stripped or "<logic-only excerpt empty after stripping comments/docstrings>"


def _audit_is_reviewable_file(path: Path) -> bool:
    try:
        with path.open("rb") as handle:
            sample = handle.read(_AUDIT_BINARY_SAMPLE_BYTES)
    except OSError:
        return False
    return b"\x00" not in sample


def _audit_walk_files(workspace: Path, *, mode: str = _AUDIT_DEFAULT_MODE) -> list[str]:
    queue: list[str] = []
    for file_path in _iter_files(workspace, ignore_root=workspace):
        rel = rt._rel(workspace, file_path)
        if _audit_should_skip_path(rel, mode=mode) or not _audit_is_reviewable_file(file_path):
            continue
        queue.append(rel)
    return queue


def _audit_sloc_plan(workspace: Path, files: list[str]) -> dict[str, object]:
    if not files:
        return {
            "counted_files": 0,
            "languages": [],
            "top_files": [],
            "largest_files": [],
            "total_code_count": 0,
            "total_line_count": 0,
            "total_file_count": 0,
            "non_countable_files": 0,
        }
    report = sloc(workspace, ignore_root=workspace, exclude=[path for path in _audit_walk_files(workspace) if path not in set(files)])
    by_path = {
        rt._rel(workspace, Path(str(item.get("path", "")))): item
        for item in report.get("top_files", [])
        if isinstance(item, dict) and isinstance(item.get("path"), str)
    }
    top_files = []
    for rel in files:
        summary = by_path.get(rel)
        if summary is None:
            continue
        top_files.append(
            {
                "path": rel,
                "language": summary.get("language", "text"),
                "code_count": int(summary.get("code_count", 0) or 0),
                "line_count": int(summary.get("line_count", 0) or 0),
            }
        )
    top_files.sort(
        key=lambda item: (
            -int(item["code_count"]),
            -int(item["line_count"]),
            str(item["path"]).lower(),
        )
    )
    largest_files = [
        {
            "path": item["path"],
            "language": item["language"],
            "code_count": item["code_count"],
            "line_count": item["line_count"],
        }
        for item in top_files[:20]
    ]
    return {
        "counted_files": len(top_files),
        "languages": list(report.get("languages", []))[:10],
        "top_files": top_files,
        "largest_files": largest_files,
        "total_code_count": int(report.get("total_code_count", 0) or 0),
        "total_line_count": int(report.get("total_line_count", 0) or 0),
        "total_file_count": int(report.get("total_file_count", 0) or 0),
        "non_countable_files": max(len(files) - len(top_files), 0),
    }


def _audit_file_size(workspace: Path, path: str) -> int:
    try:
        return max((workspace / path).stat().st_size, 0)
    except OSError:
        return 0


def _audit_file_tokens(workspace: Path, path: str, *, mode: str = _AUDIT_DEFAULT_MODE) -> int:
    try:
        if mode == _AUDIT_LOGIC_MODE:
            text = (workspace / path).read_text(encoding="utf-8")
            return max(rt.count_tokens(_audit_render_text(path, text, mode=mode)), 0)
        return max(rt.count_file_tokens(workspace / path), 0)
    except UnicodeDecodeError:
        try:
            text = (workspace / path).read_text(encoding="utf-8", errors="replace")
        except OSError:
            return 0
        return max(rt.count_tokens(_audit_render_text(path, text, mode=mode)), 0)
    except OSError:
        return 0


def _audit_priority(item: dict[str, object]) -> tuple[int, int, int, str]:
    return (
        -int(item.get("code_count", 0) or 0),
        -int(item.get("estimated_tokens", 0) or 0),
        -int(item.get("size_bytes", 0) or 0),
        str(item.get("path", "")).lower(),
    )


def _audit_file_items(workspace: Path, sloc_plan: dict[str, object], *, mode: str = _AUDIT_DEFAULT_MODE, paths: list[str] | None = None) -> list[dict[str, object]]:
    sloc_by_path = {
        str(item.get("path")): item
        for item in sloc_plan.get("top_files", [])
        if isinstance(item, dict) and isinstance(item.get("path"), str)
    }
    files = []
    selected_paths = list(paths) if paths is not None else _audit_walk_files(workspace, mode=mode)
    for path in selected_paths:
        summary = sloc_by_path.get(path, {})
        line_count = int(summary.get("line_count", 0) or 0)
        files.append(
            {
                "path": path,
                "language": summary.get("language", "text") if isinstance(summary.get("language"), str) else "text",
                "code_count": int(summary.get("code_count", 0) or 0),
                "line_count": line_count,
                "size_bytes": _audit_file_size(workspace, path),
                "estimated_tokens": max(_audit_file_tokens(workspace, path, mode=mode), line_count, 1),
            }
        )
    return sorted(files, key=_audit_priority)


def _audit_cluster_key(path: str) -> tuple[str, ...]:
    parent = PurePosixPath(path).parent
    if str(parent) in {"", "."}:
        return ()
    return parent.parts


def _audit_cluster_score(files: list[dict[str, object]]) -> tuple[int, int, str]:
    paths = [str(item.get("path", "")) for item in files]
    total = sum(max(int(item.get("estimated_tokens", 0) or 0), 1) for item in files)
    code = sum(int(item.get("code_count", 0) or 0) for item in files)
    anchor = min(paths) if paths else ""
    return (-code, -total, anchor)


def _audit_chunk_payload(items: list[dict[str, object]], *, chunk_id: int) -> dict[str, object]:
    return {
        "id": f"chunk-{chunk_id:03d}",
        "paths": [str(entry["path"]) for entry in items],
        "estimated_tokens": sum(max(int(entry.get("estimated_tokens", 0) or 0), 1) for entry in items),
        "files": len(items),
        "segments": [],
        "segment_count": 0,
    }


def _audit_segment_id(chunk_id: str, index: int) -> str:
    return f"{chunk_id}#{index:02d}"


def _audit_segment_payload(*, chunk_id: str, index: int, start: int, end: int, estimated_tokens: int, path: str | None = None) -> dict[str, object]:
    payload = {
        "id": _audit_segment_id(chunk_id, index),
        "index": index,
        "start": max(int(start or 0), 0),
        "end": max(int(end or 0), 0),
        "estimated_tokens": max(int(estimated_tokens or 0), 1),
    }
    if isinstance(path, str) and path:
        payload["path"] = path
    return payload


def _audit_partition_cluster(files: list[dict[str, object]], *, target_tokens: int) -> list[list[dict[str, object]]]:
    total = sum(max(int(item.get("estimated_tokens", 0) or 0), 1) for item in files)
    if total <= target_tokens or len(files) <= 1:
        return [files]

    by_dir: dict[tuple[str, ...], list[dict[str, object]]] = {}
    for item in files:
        key = _audit_cluster_key(str(item.get("path", "")))
        by_dir.setdefault(key, []).append(item)

    if len(by_dir) > 1:
        groups = [by_dir[key] for key in sorted(by_dir, key=lambda key: (len(key), key))]
        return _audit_pack_groups(groups, target_tokens=target_tokens)

    midpoint = max(len(files) // 2, 1)
    return [
        *(_audit_partition_cluster(files[:midpoint], target_tokens=target_tokens) if files[:midpoint] else []),
        *(_audit_partition_cluster(files[midpoint:], target_tokens=target_tokens) if files[midpoint:] else []),
    ]


def _audit_pack_groups(groups: list[list[dict[str, object]]], *, target_tokens: int) -> list[list[dict[str, object]]]:
    planned: list[list[dict[str, object]]] = []
    current: list[dict[str, object]] = []
    current_total = 0
    for group in sorted(groups, key=_audit_cluster_score):
        group_total = sum(max(int(item.get("estimated_tokens", 0) or 0), 1) for item in group)
        if group_total > target_tokens:
            if current:
                planned.append(current)
                current = []
                current_total = 0
            planned.extend(_audit_partition_cluster(group, target_tokens=target_tokens))
            continue
        if current and current_total + group_total > target_tokens:
            planned.append(current)
            current = []
            current_total = 0
        current.extend(group)
        current_total += group_total
    if current:
        planned.append(current)
    return planned


def _audit_plan_chunks(files: list[dict[str, object]], *, target_tokens: int = 64_000) -> list[dict[str, object]]:
    groups: dict[tuple[str, ...], list[dict[str, object]]] = {}
    for item in files:
        groups.setdefault(_audit_cluster_key(str(item.get("path", ""))), []).append(item)
    packed = _audit_pack_groups(list(groups.values()), target_tokens=target_tokens)
    return [
        _audit_chunk_payload(group, chunk_id=index)
        for index, group in enumerate(packed, start=1)
        if group
    ]


def _audit_chunk_content_budget(*, max_context_tokens: int = rt.MAX_CONTEXT_TOKENS, inbox_text: str = "", prompt_text: str = "", system_prompt: str = "") -> int:
    settings = rt.audit_settings(context_tokens=max_context_tokens)
    margin = int(settings.get('review_prompt_margin_tokens', 0) or 0)
    reserved = (
        rt.count_tokens(system_prompt or '')
        + rt.count_tokens(prompt_text or '')
        + rt.count_tokens(inbox_text or '')
        + margin
    )
    return max(int(max_context_tokens or 0) - reserved, 0)


def _audit_segment_file_text(workspace: Path, path: str, *, mode: str = _AUDIT_DEFAULT_MODE) -> str:
    return _audit_file_excerpt(workspace, path, mode=mode)


def _audit_chunk_segments(
    workspace: Path,
    chunk: dict[str, object],
    *,
    max_context_tokens: int = rt.MAX_CONTEXT_TOKENS,
    inbox_text: str = "",
    prompt_text: str = "",
    system_prompt: str = "",
    mode: str = _AUDIT_DEFAULT_MODE,
) -> list[dict[str, object]]:
    paths = [path for path in chunk.get('paths', []) if isinstance(path, str)]
    if not paths:
        return []
    budget = _audit_chunk_content_budget(
        max_context_tokens=max_context_tokens,
        inbox_text=inbox_text,
        prompt_text=prompt_text,
        system_prompt=system_prompt,
    )
    if budget <= 0:
        return []
    settings = rt.audit_settings(context_tokens=max_context_tokens)
    overlap = min(int(settings.get('review_segment_overlap_tokens', 0) or 0), max(budget // 8, 0))
    segments: list[dict[str, object]] = []
    index = 1
    for path in paths:
        excerpt = _audit_segment_file_text(workspace, path, mode=mode)
        tokens = rt.encode_tokens(excerpt)
        if not tokens:
            continue
        start = 0
        while start < len(tokens):
            end = min(start + budget, len(tokens))
            segment_tokens = tokens[start:end]
            if not segment_tokens:
                break
            segments.append(
                _audit_segment_payload(
                    chunk_id=str(chunk.get('id', 'chunk')),
                    index=index,
                    start=start,
                    end=end,
                    estimated_tokens=len(segment_tokens),
                    path=path,
                )
            )
            if end >= len(tokens):
                break
            next_start = end - overlap if overlap and end - overlap > start else end
            if next_start <= start:
                next_start = end
            start = next_start
            index += 1
        index += 1
    return segments


def _audit_normalize_chunk(chunk: dict[str, object], files_by_path: dict[str, dict[str, object]], *, target_tokens: int = 64_000) -> dict[str, object]:
    paths = [
        path for path in chunk.get("paths", [])
        if isinstance(path, str) and path in files_by_path
    ]
    total = 0
    trimmed: list[str] = []
    for path in paths:
        estimate = max(int(files_by_path[path].get("estimated_tokens", 0) or 0), 1)
        if trimmed and total + estimate > target_tokens:
            break
        trimmed.append(path)
        total += estimate
    if not trimmed and paths:
        trimmed = [paths[0]]
        total = max(int(files_by_path[paths[0]].get("estimated_tokens", 0) or 0), 1)
    raw_segments = chunk.get('segments', []) if isinstance(chunk.get('segments'), list) else []
    segments = []
    for item in raw_segments:
        if not isinstance(item, dict):
            continue
        segments.append(
            _audit_segment_payload(
                chunk_id=str(chunk.get('id', 'chunk')),
                index=int(item.get('index', len(segments) + 1) or len(segments) + 1),
                start=int(item.get('start', 0) or 0),
                end=int(item.get('end', 0) or 0),
                estimated_tokens=int(item.get('estimated_tokens', 0) or 0),
                path=str(item.get('path')) if isinstance(item.get('path'), str) and item.get('path') else None,
            )
        )
    return {
        "id": str(chunk.get("id", "chunk")),
        "paths": trimmed,
        "estimated_tokens": total,
        "files": len(trimmed),
        "segments": segments,
        "segment_count": max(int(chunk.get('segment_count', 0) or 0), len(segments)),
    }


def _audit_split_chunk(chunk: dict[str, object], files_by_path: dict[str, dict[str, object]]) -> list[dict[str, object]]:
    paths = [path for path in chunk.get("paths", []) if isinstance(path, str) and path in files_by_path]
    if len(paths) <= 1:
        return []
    midpoint = max(len(paths) // 2, 1)
    result = []
    for index, group in enumerate((paths[:midpoint], paths[midpoint:]), start=1):
        if not group:
            continue
        result.append(
            _audit_normalize_chunk(
                {
                    "id": f"{chunk['id']}.{index}",
                    "paths": group,
                    "segments": [],
                    "segment_count": 0,
                },
                files_by_path,
            )
        )
    return [item for item in result if item["paths"]]


def _audit_default_state(*, focus: str, workspace: Path, sloc_plan: dict[str, object], files: list[dict[str, object]], chunks: list[dict[str, object]], mode: str = _AUDIT_DEFAULT_MODE, run_config: dict[str, object] | None = None, scope: dict[str, str] | None = None) -> dict[str, object]:
    now = datetime.now(UTC).isoformat()
    return {
        "version": _AUDIT_STATE_VERSION,
        "workspace": str(workspace),
        "mode": mode,
        "focus": focus,
        "run_config": dict(run_config or {}),
        "scope": dict(scope or {}),
        "status": "in_progress",
        "created_at": now,
        "updated_at": now,
        "active_phase": "phase1",
        "phases": [_audit_empty_phase(phase_id, label) for phase_id, label, _ in _AUDIT_PHASES],
        "sloc": sloc_plan,
        "files": files,
        "chunks": chunks,
        "completed_chunks": [],
        "completed_segments": [],
        "failed_chunks": [],
        "notes": [f"Bootstrapped by `{_audit_command(mode)}`."],
        "totals": {
            "queued": len(files),
            "reviewed": 0,
            "findings": 0,
            "counted_files": sum(int(item.get("code_count", 0) or 0) > 0 for item in files),
            "total_code_count": sum(int(item.get("code_count", 0) or 0) for item in files),
            "total_line_count": sum(int(item.get("line_count", 0) or 0) for item in files),
            "chunk_count": len(chunks),
            "completed_chunks": 0,
        },
    }


def _audit_normalize_state(data: dict[str, object], *, workspace: Path | None = None) -> dict[str, object]:
    state = dict(data)
    state["version"] = _AUDIT_STATE_VERSION
    if workspace is not None:
        state["workspace"] = str(workspace)
    if not isinstance(state.get("workspace"), str) or not state.get("workspace"):
        state["workspace"] = str(workspace or Path.cwd())
    if not isinstance(state.get("mode"), str) or state.get("mode") not in _AUDIT_VALID_MODES:
        state["mode"] = _AUDIT_DEFAULT_MODE
    state["run_config"] = _audit_resolve_run_config(state.get("run_config"), mode=str(state.get("mode") or _AUDIT_DEFAULT_MODE))
    scope_data = state.get("scope") if isinstance(state.get("scope"), dict) else {}
    state["scope"] = {
        key: str(value).strip()
        for key in ("ref", "date")
        for value in [scope_data.get(key)]
        if isinstance(value, str) and str(value).strip()
    }
    if not isinstance(state.get("focus"), str):
        state["focus"] = ""
    if not isinstance(state.get("status"), str) or not state.get("status"):
        state["status"] = "in_progress"
    state["notes"] = [note for note in state.get("notes", []) if isinstance(note, str) and note.strip()] if isinstance(state.get("notes"), list) else []
    raw_completed = [item for item in state.get("completed_chunks", []) if isinstance(item, str) and item] if isinstance(state.get("completed_chunks"), list) else []
    raw_segments = [item for item in state.get("completed_segments", []) if isinstance(item, str) and item] if isinstance(state.get("completed_segments"), list) else []
    state["completed_chunks"] = [item for item in raw_completed if "#" not in item]
    state["completed_segments"] = [*raw_segments, *(item for item in raw_completed if "#" in item)]
    state["failed_chunks"] = [item for item in state.get("failed_chunks", []) if isinstance(item, dict)] if isinstance(state.get("failed_chunks"), list) else []
    files = []
    for item in state.get("files", []):
        if not isinstance(item, dict) or not isinstance(item.get("path"), str):
            continue
        files.append(
            {
                "path": item["path"],
                "language": item.get("language", "text") if isinstance(item.get("language"), str) else "text",
                "code_count": int(item.get("code_count", 0) or 0),
                "line_count": int(item.get("line_count", 0) or 0),
                "size_bytes": int(item.get("size_bytes", 0) or 0),
                "estimated_tokens": int(item.get("estimated_tokens", 0) or 0),
            }
        )
    state["files"] = sorted(files, key=_audit_priority)
    valid_paths = {str(item["path"]) for item in state["files"]}
    chunks = []
    for item in state.get("chunks", []):
        if not isinstance(item, dict):
            continue
        paths = [path for path in item.get("paths", []) if isinstance(path, str) and path in valid_paths]
        if not paths:
            continue
        chunks.append(
            {
                "id": str(item.get("id", f"chunk-{len(chunks) + 1:03d}")),
                "paths": paths,
                "estimated_tokens": int(item.get("estimated_tokens", 0) or 0),
                "files": len(paths),
                "segments": [segment for segment in item.get('segments', []) if isinstance(segment, dict)],
                "segment_count": int(item.get('segment_count', 0) or 0),
            }
        )
    state["chunks"] = chunks
    phases = {
        phase_id: _audit_empty_phase(phase_id, label)
        for phase_id, label, _ in _AUDIT_PHASES
    }
    for phase in state.get("phases", []):
        if isinstance(phase, dict) and phase.get("id") in phases:
            current = phases[str(phase["id"])]
            if isinstance(phase.get("status"), str) and phase.get("status"):
                current["status"] = str(phase["status"])
            if isinstance(phase.get("notes"), list):
                current["notes"] = [note for note in phase["notes"] if isinstance(note, str) and note.strip()]
    state["phases"] = [phases[phase_id] for phase_id, _, _ in _AUDIT_PHASES]
    return _audit_refresh_state(state)


def _audit_load_state(path: Path) -> dict[str, object] | None:
    with _AUDIT_STATE_LOCK:
        data = rt.load_toon(path, None)
    if not isinstance(data, dict):
        return None
    return _audit_normalize_state(data)


def _audit_load_state_unlocked(path: Path) -> dict[str, object] | None:
    data = rt.load_toon(path, None)
    if not isinstance(data, dict):
        return None
    return _audit_normalize_state(data)


def _write_audit_state(path: Path, state: dict[str, object]) -> None:
    with _AUDIT_STATE_LOCK:
        state["updated_at"] = datetime.now(UTC).isoformat()
        if not rt.save_toon(path, state):
            raise RuntimeError(f"Could not write audit state {rt._fmt('inline', path)}")


def _audit_update_state(path: Path, update_fn) -> dict[str, object]:
    with _AUDIT_STATE_LOCK:
        state = _audit_load_state_unlocked(path)
        if state is None:
            raise RuntimeError(f"Audit state missing or invalid: {rt._fmt('inline', path)}")
        updated = update_fn(state)
        state = updated if isinstance(updated, dict) else state
        state["updated_at"] = datetime.now(UTC).isoformat()
        if not rt.save_toon(path, state):
            raise RuntimeError(f"Could not write audit state {rt._fmt('inline', path)}")
        return state


def _audit_refresh_state(state: dict[str, object], *, focus: str = "", force_phase: str = "") -> dict[str, object]:
    if focus:
        state["focus"] = focus
    files = [item for item in state.get("files", []) if isinstance(item, dict)]
    chunks = [item for item in state.get("chunks", []) if isinstance(item, dict)]
    completed = {item for item in state.get("completed_chunks", []) if isinstance(item, str)}
    completed_segments = {item for item in state.get("completed_segments", []) if isinstance(item, str)}
    totals = state.get("totals") if isinstance(state.get("totals"), dict) else {}
    reviewed_paths = {
        path
        for chunk in chunks
        for path in chunk.get("paths", [])
        if isinstance(path, str) and (
            str(chunk.get("id")) in completed
            or (
                bool(chunk.get('segments'))
                and all(
                    isinstance(segment, dict) and str(segment.get('id') or '') in completed_segments
                    for segment in chunk.get('segments', [])
                )
            )
        )
    }
    state["totals"] = {
        "queued": len(files),
        "reviewed": len(reviewed_paths),
        "findings": int(totals.get("findings", 0) or 0),
        "counted_files": sum(int(item.get("code_count", 0) or 0) > 0 for item in files),
        "total_code_count": sum(int(item.get("code_count", 0) or 0) for item in files),
        "total_line_count": sum(int(item.get("line_count", 0) or 0) for item in files),
        "chunk_count": len(chunks),
        "completed_chunks": len(completed),
    }
    phase_map = {phase['id']: phase for phase in state.get('phases', []) if isinstance(phase, dict) and isinstance(phase.get('id'), str)}
    for phase_id, label, _ in _AUDIT_PHASES:
        phase_map.setdefault(phase_id, _audit_empty_phase(phase_id, label))['label'] = label
    forced = str(force_phase or '').strip().lower()
    if forced not in _AUDIT_PHASE_IDS:
        forced = ''
    phase_map['phase1']['status'] = 'in_progress' if forced == 'phase1' else 'done'
    if state.get('status') in _AUDIT_DONE_STATUSES:
        phase_map['phase2']['status'] = 'done'
        phase_map['phase3']['status'] = 'done'
        state['active_phase'] = 'phase3'
    elif forced == 'phase1':
        phase_map['phase2']['status'] = 'pending'
        phase_map['phase3']['status'] = 'pending'
        state['active_phase'] = 'phase1'
    elif forced == 'phase2':
        phase_map['phase2']['status'] = 'in_progress'
        phase_map['phase3']['status'] = 'pending'
        state['active_phase'] = 'phase2'
    elif forced == 'phase3':
        phase_map['phase2']['status'] = 'done' if len(completed) >= len(chunks) else ('in_progress' if completed else 'pending')
        phase_map['phase3']['status'] = 'in_progress'
        state['active_phase'] = 'phase3'
    elif len(completed) < len(chunks):
        phase_map['phase2']['status'] = 'in_progress' if completed else 'pending'
        phase_map['phase3']['status'] = 'pending'
        state['active_phase'] = 'phase2'
    else:
        phase_map['phase2']['status'] = 'done'
        phase_map['phase3']['status'] = 'in_progress'
        state['active_phase'] = 'phase3'
    state['phases'] = [phase_map[phase_id] for phase_id, _, _ in _AUDIT_PHASES]
    return state


def _audit_state_summary(state: dict[str, object]) -> str:
    totals = state.get("totals") if isinstance(state.get("totals"), dict) else {}
    queued = int(totals.get("queued", 0) or 0)
    reviewed = int(totals.get("reviewed", 0) or 0)
    chunks = int(totals.get("chunk_count", 0) or 0)
    completed = int(totals.get("completed_chunks", 0) or 0)
    percent = 100.0 if queued == 0 else min((reviewed / queued) * 100.0, 100.0)
    return (
        f"phase={state.get('active_phase', 'unknown')} "
        f"progress={reviewed}/{queued} ({percent:.1f}%) "
        f"chunks={completed}/{chunks} findings={totals.get('findings', 0)}"
    )


def _audit_wait_label_suffix(state: dict[str, object], *, chunk: dict[str, object] | None = None, detail: str | None = None) -> str:
    totals = state.get("totals") if isinstance(state.get("totals"), dict) else {}
    queued = int(totals.get("queued", 0) or 0)
    reviewed = int(totals.get("reviewed", 0) or 0)
    chunks = int(totals.get("chunk_count", 0) or 0)
    completed = int(totals.get("completed_chunks", 0) or 0)
    findings = int(totals.get("findings", 0) or 0)
    phase = str(state.get("active_phase") or "phase2")
    parts = [f"audit {phase}", f"files {reviewed}/{queued}", f"chunks {completed}/{chunks}"]
    if isinstance(chunk, dict):
        chunk_id = str(chunk.get("id") or "").strip()
        if chunk_id:
            parts.append(chunk_id)
    if isinstance(detail, str) and detail.strip():
        parts.append(detail.strip())
    parts.append(f"findings {findings}")
    return " | ".join(parts)


def _audit_pending_state(workspace: Path, *, mode: str = _AUDIT_DEFAULT_MODE, scope: dict[str, str] | None = None) -> tuple[Path, dict[str, object]] | None:
    path = _audit_session_path(workspace, mode=mode, scope=scope)
    state = _audit_load_state(path)
    if state is None or state.get("status") in _AUDIT_DONE_STATUSES:
        return None
    return path, state


def _audit_run_config(*, model: str | None = None, agent: str = "default", max_context_tokens: int = rt.MAX_CONTEXT_TOKENS, mode: str, scope: dict[str, str] | None = None) -> dict[str, object]:
    resolved_model = str(model or rt._model() or "").strip()
    return {
        "command": _audit_command(mode),
        "mode": mode,
        "model": resolved_model,
        "agent": agent,
        "max_context_tokens": int(max_context_tokens or 0),
        "phase2_workers": int(rt.audit_settings(context_tokens=max_context_tokens).get('phase2_workers', 1) or 1),
        "phase2_launch_delay_seconds": int(rt.audit_settings(context_tokens=max_context_tokens).get('phase2_launch_delay_seconds', 10) or 0),
        "from": _audit_scope_key(scope),
    }


def _audit_resolve_run_config(run_config: dict[str, object] | None = None, *, fallback: dict[str, object] | None = None, mode: str | None = None) -> dict[str, object]:
    current = dict(run_config) if isinstance(run_config, dict) else {}
    fallback_config = dict(fallback) if isinstance(fallback, dict) else {}
    resolved_mode = str(current.get('mode') or fallback_config.get('mode') or mode or _AUDIT_DEFAULT_MODE)
    if resolved_mode not in _AUDIT_VALID_MODES:
        resolved_mode = _AUDIT_DEFAULT_MODE
    resolved_context = int(current.get('max_context_tokens') or fallback_config.get('max_context_tokens') or 0)
    return {
        "command": str(current.get('command') or fallback_config.get('command') or _audit_command(resolved_mode)),
        "mode": resolved_mode,
        "model": str(current.get('model') or fallback_config.get('model') or '').strip(),
        "agent": str(current.get('agent') or fallback_config.get('agent') or 'default'),
        "max_context_tokens": resolved_context,
        "phase2_workers": int(current.get('phase2_workers') or fallback_config.get('phase2_workers') or rt.audit_settings(context_tokens=resolved_context or rt.MAX_CONTEXT_TOKENS).get('phase2_workers', 1) or 1),
        "phase2_launch_delay_seconds": int(current.get('phase2_launch_delay_seconds') or fallback_config.get('phase2_launch_delay_seconds') or rt.audit_settings(context_tokens=resolved_context or rt.MAX_CONTEXT_TOKENS).get('phase2_launch_delay_seconds', 10) or 0),
        "from": str(current.get('from') or fallback_config.get('from') or 'all'),
    }


def _audit_transparency_snippet(run_config: dict[str, object] | None = None) -> str:
    resolved = _audit_resolve_run_config(run_config)
    prefix = _audit_schema('transparency_prefix')
    command = str(resolved['command'])
    model = str(resolved.get('model') or '').strip()
    if model:
        command = f"OY_MODEL={model} {command}"
    return f"> {prefix} `{command}`"


def _audit_upsert_transparency(text: str, run_config: dict[str, object] | None = None) -> str:
    snippet = _audit_transparency_snippet(run_config)
    prefix = f"> {_audit_schema('transparency_prefix')}"
    lines = [line for line in text.splitlines() if not line.startswith(prefix)]
    report_titles = {_audit_schema('report_title'), _audit_schema('legacy_report_title')}
    if not any(line.strip() for line in lines):
        return f"{_audit_schema('report_title')}\n\n{snippet}\n"
    insert_at = next((index for index, line in enumerate(lines) if line.strip() in report_titles), None)
    if insert_at is None:
        insert_at = next((index for index, line in enumerate(lines) if line.strip()), 0)
    head = lines[: insert_at + 1]
    tail = lines[insert_at + 1 :]
    while tail and not tail[0].strip():
        tail = tail[1:]
    rebuilt = [*head, "", snippet]
    if tail:
        rebuilt.extend(["", *tail])
    return "\n".join(rebuilt).rstrip() + "\n"

def _ensure_audit_session(workspace: Path, focus: str = "", *, restart: bool = False, mode: str = _AUDIT_DEFAULT_MODE, run_config: dict[str, object] | None = None, scope: dict[str, str] | None = None) -> dict[str, object]:
    scope = dict(scope or {})
    path = _audit_session_path(workspace, mode=mode, scope=scope)
    run_config = dict(run_config or _audit_run_config(mode=mode, scope=scope))
    if not restart:
        state = _audit_load_state(path)
        if state is not None and state.get("status") not in _AUDIT_DONE_STATUSES:
            state['run_config'] = _audit_resolve_run_config(state.get('run_config'), fallback=run_config, mode=str(state.get('mode') or mode))
            state['scope'] = dict(state.get('scope') or scope)
            prepared_issues = _audit_prepare_issues_md(workspace, state)
            state['totals']['findings'] = int(prepared_issues.get('findings', 0) or 0)
            _audit_refresh_state(state)
            _write_audit_state(path, state)
            return {"session_path": path, "state_data": state, "created": False}
    file_paths = _audit_walk_files(workspace, mode=mode)
    changed_paths = _audit_git_changed_paths(workspace, scope=scope)
    scoped_paths = _audit_filter_paths(file_paths, changed_paths)
    sloc_plan = _audit_sloc_plan(workspace, scoped_paths)
    files = _audit_file_items(workspace, sloc_plan, mode=mode, paths=scoped_paths)
    chunks = _audit_plan_chunks(files, target_tokens=rt.audit_settings()["review_chunk_target_tokens"])
    state = _audit_default_state(
        focus=focus,
        workspace=workspace,
        sloc_plan=sloc_plan,
        files=files,
        chunks=chunks,
        mode=mode,
        run_config=run_config,
        scope=scope,
    )
    prepared_issues = _audit_prepare_issues_md(workspace, state)
    state['totals']['findings'] = int(prepared_issues.get('findings', 0) or 0)
    state["notes"].append(
        f"Planned {len(chunks)} chunk(s) from {len(files)} file(s) at 64k tokens using sloc+tiktoken; prompt-fit segmenting runs at review time when needed."
    )
    if prepared_issues.get('changed'):
        state['notes'].append('Normalised ISSUES.md into audit inbox format before phase2 review.')
    if scope_note := _audit_scope_note(scope):
        state['notes'].append(scope_note)
    _audit_refresh_state(state, focus=focus)
    _write_audit_state(path, state)
    return {"session_path": path, "state_data": state, "created": True}


def _audit_resume_decision(workspace: Path, *, interactive: bool, mode: str = _AUDIT_DEFAULT_MODE, scope: dict[str, str] | None = None) -> str:
    pending = _audit_pending_state(workspace, mode=mode, scope=scope)
    if pending is None:
        return "resume"
    path, state = pending
    message = (
        f"Found unfinished {_audit_mode_name(mode)} for {rt._fmt('inline', workspace)} at {rt._fmt('inline', path)} "
        f"({_audit_state_summary(state)})."
    )
    if interactive and rt.can_prompt():
        return rt.select(
            message + " Resume it?",
            ["resume", "restart", "cancel"],
            console=rt.STDERR,
            default="resume",
            option_text=lambda option, index: f"{index}. {rt._fmt('inline', option)}",
        ).strip()
    rt._note(message + " Resuming.", tag="note")
    return "resume"


def _build_audit_prompt(*, interactive: bool, focus: str, session_path: Path, mode: str = _AUDIT_DEFAULT_MODE, scope: dict[str, str] | None = None) -> str:
    section = _audit_section(mode)
    prompt = session_text(section, "repo_user_prompt" if interactive else "default_user_prompt")
    prompt += session_text(section, "workflow_suffix", session_path=session_path)
    prompt += session_text("audit", "reference_suffix")
    if focus:
        prompt += session_text(section, "focus_suffix", focus=focus)
    if scope_note := _audit_scope_note(scope):
        prompt += f" {scope_note}"
    return prompt


def _prepare_audit_run(*, session, focus: str, interactive: bool, mode: str = _AUDIT_DEFAULT_MODE, scope: dict[str, str] | None = None) -> tuple[dict[str, object], str]:
    run_config = _audit_run_config(
        model=str(session.get("model") or ""),
        agent=str(session.get("agent") or "default"),
        max_context_tokens=int(session.get("max_context_tokens", rt.MAX_CONTEXT_TOKENS) or rt.MAX_CONTEXT_TOKENS),
        mode=mode,
        scope=scope,
    )
    decision = _audit_resume_decision(session["workspace"], interactive=interactive, mode=mode, scope=scope)
    if decision == "cancel":
        raise RuntimeError("audit cancelled")
    artifacts = _ensure_audit_session(
        session["workspace"],
        focus=focus,
        restart=(decision == "restart"),
        mode=mode,
        run_config=run_config,
        scope=scope,
    )
    return artifacts, _build_audit_prompt(
        interactive=interactive,
        focus=focus,
        session_path=artifacts["session_path"],
        mode=mode,
        scope=scope,
    )


def _audit_issues_path(workspace: Path) -> Path:
    return workspace / "ISSUES.md"


def _audit_read_issues(workspace: Path) -> str:
    with _AUDIT_ISSUES_LOCK:
        issues_path = _audit_issues_path(workspace)
        if not issues_path.exists():
            return ""
        try:
            return issues_path.read_text(encoding="utf-8")
        except OSError:
            return ""


_AUDIT_ISSUES_LOCK = RLock()
_AUDIT_STATE_LOCK = RLock()


def _audit_write_issues(workspace: Path, text: str) -> None:
    with _AUDIT_ISSUES_LOCK:
        _audit_issues_path(workspace).write_text(text, encoding="utf-8")


def _audit_inbox_section(entries: str | None = None) -> str:
    placeholder = _audit_schema('inbox_placeholder')
    body = entries.strip() if isinstance(entries, str) and entries.strip() else placeholder
    return (
        f"{_audit_h2(_audit_schema('inbox_title'))}\n\n"
        f"{_audit_schema('inbox_note')}\n\n"
        f"{body}\n"
    )


def _audit_markdown_blocks(text: str, *, prefix: str) -> list[dict[str, object]]:
    pattern = re.compile(rf"(?m)^{re.escape(prefix)}(?P<title>.+)$")
    matches = list(pattern.finditer(text))
    result = []
    for index, match in enumerate(matches):
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        result.append(
            {
                "title": match.group('title').strip(),
                "body": text[match.end():end].strip(),
                "start": match.start(),
                "end": end,
                "full": text[match.start():end].strip(),
            }
        )
    return result


def _audit_split_legacy_entries(text: str) -> tuple[str, list[str]]:
    blocks = _audit_markdown_blocks(text, prefix=_audit_schema('report_h3_prefix'))
    if not blocks:
        return text.rstrip(), []
    intro = text[: int(blocks[0]['start'])].rstrip()
    entries = [str(block['full']).strip() for block in blocks if str(block['full']).strip()]
    return intro, entries


def _audit_clean_inbox_entries(text: str) -> str:
    cleaned = text.strip()
    if not cleaned:
        return ''
    note = _audit_schema('inbox_note')
    if cleaned.startswith(note):
        cleaned = cleaned[len(note):].lstrip()
    placeholder = _audit_schema('inbox_placeholder')
    if cleaned == placeholder:
        return ''
    if cleaned.startswith(placeholder):
        cleaned = cleaned[len(placeholder):].lstrip()
    return cleaned.strip()


def _audit_normalize_intro(text: str) -> str:
    cleaned = text.strip()
    if not cleaned:
        return _audit_schema('report_title')
    lines = cleaned.splitlines()
    if lines and lines[0].strip() == _audit_schema('legacy_report_title'):
        lines[0] = _audit_schema('report_title')
    return "\n".join(lines).rstrip()


def _audit_render_section(title: str, body: str) -> str:
    rendered = _audit_h2(title)
    if body.strip():
        rendered += f"\n\n{body.strip()}"
    return rendered + "\n"


def _audit_normalize_issues_text(text: str) -> str:
    raw = text.strip()
    if not raw:
        return ''
    sections = _audit_markdown_blocks(text, prefix=_audit_schema('report_h2_prefix'))
    inbox_entries: list[str] = []
    preserved_sections: list[str] = []
    intro = text.rstrip()
    if sections:
        intro, legacy_entries = _audit_split_legacy_entries(text[: int(sections[0]['start'])])
        inbox_entries.extend(legacy_entries)
        for section in sections:
            title = str(section['title']).strip()
            body = str(section['body']).strip()
            if title == _audit_schema('inbox_title'):
                cleaned = _audit_clean_inbox_entries(body)
                if cleaned:
                    inbox_entries.append(cleaned)
                continue
            if title == _audit_schema('findings_title'):
                cleaned = _audit_clean_inbox_entries(body)
                if cleaned:
                    inbox_entries.append(cleaned if cleaned.startswith(_audit_schema('report_h3_prefix')) else f"{_audit_h3(_audit_schema('findings_title'))}\n\n{cleaned}")
                continue
            if title in _audit_preserved_section_titles():
                preserved_sections.append(_audit_render_section(title, body).rstrip())
                continue
            if title == _audit_schema('summary_title'):
                continue
            entry = _audit_h3(title)
            if body:
                entry += f"\n\n{body}"
            inbox_entries.append(entry)
    else:
        intro, legacy_entries = _audit_split_legacy_entries(text)
        inbox_entries.extend(legacy_entries)
    intro_text = _audit_normalize_intro(intro)
    inbox_body = "\n\n".join(entry.strip() for entry in inbox_entries if entry.strip()) or _audit_schema('inbox_placeholder')
    parts = [intro_text, _audit_inbox_section(inbox_body).rstrip(), *preserved_sections]
    return "\n\n".join(part for part in parts if part.strip()) + "\n"


def _audit_prepare_issues_md(workspace: Path, state: dict[str, object]) -> dict[str, object]:
    issues_path = _audit_issues_path(workspace)
    run_config = _audit_resolve_run_config(state.get('run_config'), mode=str(state.get('mode') or _AUDIT_DEFAULT_MODE))
    state['run_config'] = run_config
    if not issues_path.exists():
        _audit_seed_issues_md(workspace, state)
    before = _audit_read_issues(workspace)
    after = _audit_normalize_issues_text(before)
    prepared = _audit_upsert_transparency(after or before, run_config)
    final = _audit_upsert_phase1_dependency_assessment(prepared, workspace)
    changed = final != before
    if changed:
        _audit_write_issues(workspace, final)
    return {'changed': changed, 'findings': _audit_issue_count(final)}


def _audit_section_bounds(text: str, title: str) -> tuple[int, int] | None:
    match = re.search(rf"(?m)^{re.escape(_audit_h2(title))}\s*$", text)
    if match is None:
        return None
    next_match = re.search(rf"(?m)^{re.escape(_audit_schema('report_h2_prefix'))}\S", text[match.end():])
    end = match.end() + next_match.start() if next_match else len(text)
    return match.start(), end


def _audit_inbox_bounds(text: str) -> tuple[int, int] | None:
    return _audit_section_bounds(text, _audit_schema('inbox_title'))


def _audit_upsert_section(text: str, *, title: str, body: str) -> str:
    rendered = _audit_render_section(title, body).rstrip()
    bounds = _audit_section_bounds(text, title)
    if bounds is not None:
        start, end = bounds
        head = text[:start].rstrip()
        tail = text[end:].lstrip("\n")
        updated = ((head + "\n\n") if head else "") + rendered
        if tail:
            updated += "\n\n" + tail
        return updated.rstrip() + "\n"
    inbox_bounds = _audit_inbox_bounds(text)
    if inbox_bounds is not None:
        _start, end = inbox_bounds
        head = text[:end].rstrip()
        tail = text[end:].lstrip("\n")
        updated = head + "\n\n" + rendered
        if tail:
            updated += "\n\n" + tail
        return updated.rstrip() + "\n"
    head = text.rstrip()
    return (((head + "\n\n") if head else "") + rendered).rstrip() + "\n"


def _audit_latest_renovate_report(workspace: Path) -> Path | None:
    tmp_dir = workspace / ".tmp"
    candidates = sorted(
        (path for path in tmp_dir.glob("renovate-*.json") if path.is_file()),
        key=lambda path: path.name,
        reverse=True,
    )
    return candidates[0] if candidates else None


def _audit_extract_messages(value: object) -> list[str]:
    if isinstance(value, str):
        stripped = value.strip()
        return [stripped] if stripped else []
    if isinstance(value, dict):
        result: list[str] = []
        for key in ('message', 'warning', 'title', 'text'):
            candidate = value.get(key)
            if isinstance(candidate, str) and candidate.strip():
                result.append(candidate.strip())
        return result
    if isinstance(value, list):
        result: list[str] = []
        for item in value:
            result.extend(_audit_extract_messages(item))
        return result
    return []


def _audit_unique_text(items: list[str]) -> list[str]:
    seen: set[str] = set()
    unique: list[str] = []
    for item in items:
        cleaned = item.strip()
        if not cleaned or cleaned in seen:
            continue
        seen.add(cleaned)
        unique.append(cleaned)
    return unique


def _audit_renovate_warnings(data: object) -> list[str]:
    collected: list[str] = []

    def walk(value: object) -> None:
        if isinstance(value, dict):
            for key, nested in value.items():
                if str(key).lower() in {'warning', 'warnings'}:
                    collected.extend(_audit_extract_messages(nested))
                walk(nested)
            return
        if isinstance(value, list):
            for item in value:
                walk(item)

    walk(data)
    return _audit_unique_text(collected)


def _audit_renovate_updates(data: object) -> list[dict[str, object]]:
    updates: list[dict[str, object]] = []
    seen: set[tuple[str, ...]] = set()

    def add_update(value: object) -> None:
        if not isinstance(value, dict):
            return
        if not any(isinstance(value.get(key), str) and str(value.get(key)).strip() for key in ('depName', 'packageName', 'packageFile', 'manager')):
            return
        if not any(key in value for key in ('currentVersion', 'currentValue', 'newVersion', 'newValue', 'updateType', 'skipReason', 'isVulnerabilityAlert')):
            return
        fingerprint = tuple(
            str(value.get(key) or '')
            for key in ('packageFile', 'depName', 'packageName', 'currentVersion', 'currentValue', 'newVersion', 'newValue', 'updateType', 'skipReason')
        )
        if fingerprint in seen:
            return
        seen.add(fingerprint)
        updates.append(value)

    def walk(value: object) -> None:
        if isinstance(value, dict):
            for key, nested in value.items():
                if str(key) == 'updates' and isinstance(nested, list):
                    for item in nested:
                        add_update(item)
                walk(nested)
            return
        if isinstance(value, list):
            for item in value:
                walk(item)

    walk(data)
    return updates


def _audit_renovate_is_vulnerability_alert(update: dict[str, object]) -> bool:
    skip_reason = str(update.get('skipReason') or '').strip().lower()
    return bool(update.get('isVulnerabilityAlert')) or 'vulnerability' in skip_reason or any(key in update for key in ('vulnerabilityFixVersion', 'vulnerabilitySeverity'))


def _audit_renovate_is_actions_update(update: dict[str, object]) -> bool:
    manager = str(update.get('manager') or '').strip().lower()
    package_file = str(update.get('packageFile') or '').strip().lower()
    return manager == 'github-actions' or package_file.startswith('.github/workflows/')


def _audit_renovate_is_major_update(update: dict[str, object]) -> bool:
    return bool(update.get('isMajor')) or str(update.get('updateType') or '').strip().lower() == 'major'


def _audit_renovate_update_note(update: dict[str, object]) -> str:
    name = (
        str(update.get('depName') or '').strip()
        or str(update.get('packageName') or '').strip()
        or str(update.get('packageFile') or '').strip()
        or 'update'
    )
    current = str(update.get('currentVersion') or update.get('currentValue') or '').strip()
    new = str(update.get('newVersion') or update.get('newValue') or '').strip()
    detail_parts = []
    if current and new:
        detail_parts.append(f"{current} -> {new}")
    elif new:
        detail_parts.append(f"-> {new}")
    if _audit_renovate_is_actions_update(update):
        detail_parts.append('GitHub Actions')
    elif manager := str(update.get('manager') or '').strip():
        detail_parts.append(manager)
    if update_type := str(update.get('updateType') or '').strip():
        detail_parts.append(update_type)
    if skip_reason := str(update.get('skipReason') or '').strip():
        detail_parts.append(f"skip={skip_reason}")
    return f"`{name}`" + (f" ({', '.join(detail_parts)})" if detail_parts else '')


def _audit_phase1_dependency_assessment_line(workspace: Path) -> str:
    report_path = _audit_latest_renovate_report(workspace)
    if report_path is None:
        return "- Phase1 dependency assessment: no `.tmp/renovate-*.json` report found; skipped dependency/update review. Run `oy renovate-local` if needed."
    report_rel = rt._rel(workspace, report_path)
    data = rt.load_json(report_path, None)
    if not isinstance(data, (dict, list)):
        return f"- Phase1 dependency assessment: could not parse `{report_rel}`; skipped dependency/update review."
    warnings = _audit_renovate_warnings(data)
    updates = _audit_renovate_updates(data)
    vulnerability_updates = [update for update in updates if _audit_renovate_is_vulnerability_alert(update)]
    malicious_updates = [update for update in updates if str(update.get('skipReason') or '').strip().lower() == 'malicious-update-proposed']
    major_updates = [update for update in updates if _audit_renovate_is_major_update(update)]
    actions_updates = [update for update in updates if _audit_renovate_is_actions_update(update)]
    parts = [f"- Phase1 dependency assessment: inspected newest relevant Renovate report `{report_rel}`"]
    if vulnerability_updates or malicious_updates:
        findings = []
        if vulnerability_updates:
            count = len(vulnerability_updates)
            findings.append(f"{count} vulnerability alert{'s' if count != 1 else ''}")
        if malicious_updates:
            count = len(malicious_updates)
            findings.append(f"{count} malicious update proposal{'s' if count != 1 else ''}")
        if major_updates:
            count = len(major_updates)
            findings.append(f"{count} major update{'s' if count != 1 else ''}")
        if actions_updates:
            count = len(actions_updates)
            findings.append(f"{count} GitHub Actions update{'s' if count != 1 else ''}")
        parts.append(': flagged ' + ', '.join(findings) + '.')
        priority_examples = vulnerability_updates + malicious_updates + [
            update for update in major_updates + actions_updates
            if update not in vulnerability_updates and update not in malicious_updates
        ]
        example_notes = _audit_unique_text([_audit_renovate_update_note(update) for update in priority_examples])[:3]
        if example_notes:
            parts.append(' Examples: ' + '; '.join(example_notes) + '.')
    else:
        summary = []
        if updates:
            count = len(updates)
            summary.append(f"{count} update candidate{'s' if count != 1 else ''}")
        if major_updates:
            count = len(major_updates)
            summary.append(f"{count} major")
        if actions_updates:
            count = len(actions_updates)
            summary.append(f"{count} GitHub Actions")
        if warnings:
            count = len(warnings)
            summary.append(f"{count} warning{'s' if count != 1 else ''}")
        parts.append(': no clear dependency or GitHub Actions risk beyond routine maintenance')
        parts.append(f" ({', '.join(summary)})." if summary else '.')
    if warnings:
        warning_preview = '; '.join(rt.preview(item, 80) for item in warnings[:2])
        parts.append(f" Warnings: {warning_preview}.")
    return ''.join(parts)


def _audit_upsert_phase1_dependency_assessment(text: str, workspace: Path) -> str:
    title = _audit_schema('short_audit_log_title')
    line = _audit_phase1_dependency_assessment_line(workspace)
    sections = _audit_markdown_blocks(text, prefix=_audit_schema('report_h2_prefix'))
    body = line
    for section in sections:
        if str(section['title']).strip() != title:
            continue
        lines = [item.rstrip() for item in str(section['body']).splitlines()]
        kept = [item for item in lines if item.strip() and not item.startswith('- Phase1 dependency assessment:')]
        body = "\n".join([line, *kept]).strip()
        break
    return _audit_upsert_section(text, title=title, body=body)


def _audit_ensure_inbox(workspace: Path) -> str:
    with _AUDIT_ISSUES_LOCK:
        text = _audit_read_issues(workspace)
        if _audit_inbox_bounds(text) is not None:
            return text
        if text.strip():
            text = text.rstrip() + "\n\n" + _audit_inbox_section()
        else:
            text = _audit_inbox_section()
        if not text.endswith("\n"):
            text += "\n"
        _audit_write_issues(workspace, text)
        return text


def _audit_read_inbox(workspace: Path) -> str:
    text = _audit_read_issues(workspace)
    bounds = _audit_inbox_bounds(text)
    if bounds is None:
        return _audit_inbox_section()
    return text[bounds[0]:bounds[1]].strip() + "\n"


def _audit_append_inbox(workspace: Path, content: str) -> dict[str, object]:
    entry = content.strip()
    if not entry:
        raise ValueError("content must be non-empty")
    with _AUDIT_ISSUES_LOCK:
        text = _audit_ensure_inbox(workspace)
        bounds = _audit_inbox_bounds(text)
        if bounds is None:
            raise RuntimeError("audit inbox missing")
        start, end = bounds
        section = text[start:end]
        placeholder = _audit_schema('inbox_placeholder')
        if placeholder in section:
            updated = text[:start] + section.replace(placeholder, entry, 1) + text[end:]
        else:
            before = text[:end].rstrip()
            after = text[end:].lstrip("\n")
            updated = before + "\n\n" + entry + "\n"
            if after:
                updated += "\n" + after
        if not updated.endswith("\n"):
            updated += "\n"
        _audit_write_issues(workspace, updated)
    return {
        "path": rt._rel(workspace, _audit_issues_path(workspace)),
        "chars_appended": len(entry),
        "preview": rt.preview(entry, 120),
    }


def _audit_compact_inbox_text(text: str, *, max_tokens: int) -> str:
    cleaned = text.strip()
    if not cleaned:
        return _audit_inbox_section()
    if rt.count_tokens(cleaned) <= max_tokens:
        return cleaned + ("" if cleaned.endswith("\n") else "\n")
    lines = [line.rstrip() for line in cleaned.splitlines()]
    kept: list[str] = []
    entry: list[str] = []
    note_line = _audit_schema('inbox_note')
    heading_line = _audit_h2(_audit_schema('inbox_title'))
    for line in lines:
        if line == heading_line or line == note_line:
            continue
        if line.startswith(_audit_schema('report_h3_prefix')) or line.startswith('- ['):
            if entry:
                kept.append(" ".join(part.strip() for part in entry if part.strip()))
            entry = [line]
            continue
        if line.strip():
            entry.append(line.strip())
    if entry:
        kept.append(" ".join(part.strip() for part in entry if part.strip()))
    compact = _audit_inbox_section()
    selected: list[str] = []
    for item in reversed(kept):
        candidate_items = [item, *selected]
        candidate = _audit_inbox_section("\n\n".join(reversed(candidate_items))).strip()
        if selected and rt.count_tokens(candidate) > max_tokens:
            break
        selected.append(item)
        compact = candidate
        if rt.count_tokens(compact) >= max_tokens:
            break
    if not selected:
        placeholder = _audit_schema('inbox_placeholder')
        compact = _audit_inbox_section(rt.truncate_str_to_tokens(kept[-1] if kept else placeholder, max_tokens=max(max_tokens, 1)))
    return compact.strip() + "\n"


def _audit_inbox_context(workspace: Path, *, max_context_tokens: int = rt.MAX_CONTEXT_TOKENS) -> str:
    settings = rt.audit_settings(context_tokens=max_context_tokens)
    limit = int(settings.get('review_inbox_context_tokens', 0) or 0)
    return _audit_compact_inbox_text(_audit_read_inbox(workspace), max_tokens=max(limit, 1))


def _audit_file_excerpt(workspace: Path, path: str, *, mode: str = _AUDIT_DEFAULT_MODE, start_token: int | None = None, end_token: int | None = None) -> str:
    try:
        text = (workspace / path).read_text(encoding="utf-8")
    except UnicodeDecodeError:
        text = (workspace / path).read_text(encoding="utf-8", errors="replace")
    except OSError as exc:
        return f"{_audit_h2(path)}\n<read failed: {exc}>\n"
    rendered = _audit_render_text(path, text, mode=mode)
    if start_token is not None or end_token is not None:
        tokens = rt.encode_tokens(rendered)
        start = max(int(start_token or 0), 0)
        end = len(tokens) if end_token is None else max(int(end_token or 0), 0)
        excerpt_tokens = tokens[start:end]
        rendered = rt.decode_tokens(excerpt_tokens) if excerpt_tokens else "<empty excerpt>"
        rendered = (
            f"<segment tokens {start}:{end}>\n"
            f"{rendered}"
        )
    return f"{_audit_h2(path)}\n{rendered}\n"


def _audit_chunk_text(workspace: Path, chunk: dict[str, object], *, mode: str = _AUDIT_DEFAULT_MODE, segment: dict[str, object] | None = None) -> str:
    if isinstance(segment, dict):
        segment_path = str(segment.get('path')) if isinstance(segment.get('path'), str) and segment.get('path') else ''
        if not segment_path and len([path for path in chunk.get('paths', []) if isinstance(path, str)]) == 1:
            segment_path = str(chunk['paths'][0])
        if segment_path:
            return _audit_file_excerpt(
                workspace,
                segment_path,
                mode=mode,
                start_token=int(segment.get('start', 0) or 0),
                end_token=int(segment.get('end', 0) or 0),
            )
    return "\n".join(
        _audit_file_excerpt(workspace, path, mode=mode)
        for path in chunk.get("paths", [])
        if isinstance(path, str)
    )


def _audit_limited_tool_registry(
    workspace: Path,
    *,
    allow_search: bool,
    allow_inbox_append: bool = False,
    allow_replace: bool = True,
    prefix_replace_tokens: int = 0,
    mode: str = _AUDIT_DEFAULT_MODE,
    inbox_append_fn = None,
) -> dict[str, dict[str, object]]:
    issues_rel = rt._rel(workspace, _audit_issues_path(workspace))

    def audit_search(state: object, pattern: str, path: str = ".", exclude: str | list[str] | None = None, limit: int = rt.BUDGETS["default_line_limit"], fuzzy: str | None = None, best_match: bool = False, enhance_match: bool = False):
        effective_exclude = _audit_logic_search_exclude(exclude) if mode == _AUDIT_LOGIC_MODE else exclude
        return tools_lib.tool_search(
            state,
            pattern=pattern,
            path=path,
            exclude=effective_exclude,
            limit=limit,
            fuzzy=fuzzy,
            best_match=best_match,
            enhance_match=enhance_match,
        )

    def issues_replace(state: object, pattern: str, replacement: str):
        return tools_lib.tool_replace(
            state,
            pattern=pattern,
            replacement=replacement,
            path=issues_rel,
            exclude=None,
            limit=rt.BUDGETS["default_line_limit"],
        )

    def issues_inbox_append(state: object, content: str):
        payload = (inbox_append_fn or _audit_append_inbox)(workspace, content)
        rt.note_tool(state, "inbox_append", path=issues_rel, chars=len(content))
        rt.show(f"{issues_rel}: appended {payload['chars_appended']} chars to audit inbox")
        return payload

    def issues_replace_prefix(state: object, content: str):
        current = _audit_read_issues(workspace)
        tokens = rt.encode_tokens(current)
        cut = min(max(int(prefix_replace_tokens or 0), 1), len(tokens))
        suffix = rt.decode_tokens(tokens[cut:]) if cut < len(tokens) else ""
        updated = f"{content}{suffix}"
        _audit_write_issues(workspace, updated)
        before_tokens = len(tokens)
        after_tokens = rt.count_tokens(updated)
        rt.note_tool(
            state,
            "replace_prefix",
            path=issues_rel,
            prefix_tokens=cut,
            replacement_chars=len(content),
        )
        rt.show(f"{issues_rel}: rewrote first {cut} tokens; total {before_tokens}->{after_tokens} tokens")
        return {
            "path": issues_rel,
            "prefix_tokens": cut,
            "replacement_chars": len(content),
            "before_tokens": before_tokens,
            "after_tokens": after_tokens,
        }

    registry = {}
    if allow_replace:
        registry["replace"] = {
            "name": "replace",
            "fn": issues_replace,
            "description": f"Replace text only inside `{issues_rel}`.",
            "parameters": tools_lib.signature_schema(issues_replace, skip={"state"}),
            "mutating": True,
        }
    if allow_inbox_append:
        registry["inbox_append"] = {
            "name": "inbox_append",
            "fn": issues_inbox_append,
            "description": f"Append text only to the phase2 inbox inside `{issues_rel}`. Prefer this over merging findings during chunk review.",
            "parameters": tools_lib.signature_schema(issues_inbox_append, skip={"state"}),
            "mutating": True,
        }
    if prefix_replace_tokens > 0:
        registry["replace_prefix"] = {
            "name": "replace_prefix",
            "fn": issues_replace_prefix,
            "description": f"Rewrite only the first {int(prefix_replace_tokens)} tokens inside `{issues_rel}` and leave the unseen suffix untouched.",
            "parameters": tools_lib.signature_schema(issues_replace_prefix, skip={"state"}),
            "mutating": True,
        }
    if allow_search:
        registry["search"] = {
            "name": "search",
            "fn": audit_search,
            "description": tools_lib.TOOL_REGISTRY["search"]["description"],
            "parameters": tools_lib.TOOL_REGISTRY["search"]["parameters"],
            "mutating": False,
        }
    return registry


def _audit_review_prompt(base_prompt: str, state: dict[str, object], chunk: dict[str, object], *, issues_rel: str, mode: str = _AUDIT_DEFAULT_MODE, segment: dict[str, object] | None = None) -> str:
    sloc_plan = state.get("sloc") if isinstance(state.get("sloc"), dict) else {}
    segment_note = ""
    if isinstance(segment, dict):
        segment_path = str(segment.get('path')) if isinstance(segment.get('path'), str) and segment.get('path') else ''
        segment_note = (
            f" Review only segment {segment.get('index', 1)}/{max(int(chunk.get('segment_count', 0) or 0), 1)}"
            + (f" from {segment_path}" if segment_path else "")
            + f" for this chunk ({segment.get('start', 0)}:{segment.get('end', 0)} tokens)."
        )
    return (
        base_prompt
        + session_text(
            "audit",
            "iteration_suffix",
            iteration=len(state.get("completed_chunks", [])) + 1,
            phase="phase2",
            queued=state.get("totals", {}).get("queued", 0),
            pending=max(
                int(state.get("totals", {}).get("queued", 0) or 0)
                - int(state.get("totals", {}).get("reviewed", 0) or 0),
                0,
            ),
            in_progress=0,
            reviewed=state.get("totals", {}).get("reviewed", 0),
            findings=state.get("totals", {}).get("findings", 0),
        )
        + f" Review only this chunk: {', '.join(chunk['paths'])}."
        + segment_note
        + f" Chunk budget is {chunk['estimated_tokens']} estimated tokens; planned target is 64000."
        + f" Use only `search` and `inbox_append`. `inbox_append` is scoped to `{issues_rel}`."
        + " Phase2 is append-only: add new candidate findings to the inbox, and leave dedupe, ordering, and condensation for phase3."
        + " Prefer inbox entries that start with `###` and keep severity, evidence, references, impact, and remediation concise."
        + " If this chunk was segmented to fit context, search adjacent code only when needed and avoid repeating earlier segment findings."
        + " Do not touch any file except ISSUES.md."
        + (" Search defaults exclude docs and lockfiles in this mode." if mode == _AUDIT_LOGIC_MODE else "")
        + " "
        + session_text(_audit_section(mode), "review_suffix")
        + (
            f" Repo summary: counted_files={sloc_plan.get('counted_files', 0)}, total_code_lines={sloc_plan.get('total_code_count', 0)}."
            if sloc_plan
            else ""
        )
    )


def _audit_review_segments(workspace: Path, chunk: dict[str, object], *, prompt: str, max_context_tokens: int = rt.MAX_CONTEXT_TOKENS, mode: str = _AUDIT_DEFAULT_MODE) -> list[dict[str, object]]:
    system_prompt = _audit_system_prompt_for_mode(mode, phase="phase2")
    inbox_text = _audit_inbox_context(workspace, max_context_tokens=max_context_tokens)
    review_prompt = _audit_review_prompt(prompt, {"completed_chunks": [], "totals": {}, "sloc": {}}, chunk, issues_rel="ISSUES.md", mode=mode)
    segments = _audit_chunk_segments(
        workspace,
        chunk,
        max_context_tokens=max_context_tokens,
        inbox_text=inbox_text,
        prompt_text=review_prompt,
        system_prompt=system_prompt,
        mode=mode,
    )
    return segments or [_audit_segment_payload(chunk_id=str(chunk.get('id', 'chunk')), index=1, start=0, end=0, estimated_tokens=max(int(chunk.get('estimated_tokens', 0) or 0), 1))]


def _audit_summary_prompt(base_prompt: str, state: dict[str, object], *, issues_rel: str, mode: str = _AUDIT_DEFAULT_MODE) -> str:
    return (
        base_prompt
        + session_text(
            "audit",
            "iteration_suffix",
            iteration=max(len(state.get("completed_chunks", [])), 1),
            phase="phase3",
            queued=state.get("totals", {}).get("queued", 0),
            pending=0,
            in_progress=0,
            reviewed=state.get("totals", {}).get("reviewed", 0),
            findings=state.get("totals", {}).get("findings", 0),
        )
        + f" Final pass: consume the phase2 inbox and rewrite `{issues_rel}` to preserve detail for the 10-15 most important issues and make the rest very concise."
        + " Treat repeated per-segment findings as one issue unless later segments add materially new evidence."
        + " Dedupe overlapping inbox items, keep ordering actionable, preserve evidence, and do not touch any other file."
        + " "
        + session_text(_audit_section(mode), "summary_suffix")
    )


def _audit_summary_content_budget(*, max_context_tokens: int = rt.MAX_CONTEXT_TOKENS, prompt_text: str = "", system_prompt: str = "") -> int:
    settings = rt.audit_settings(context_tokens=max_context_tokens)
    margin = int(settings.get('review_prompt_margin_tokens', 0) or 0)
    reserved = (
        rt.count_tokens(system_prompt or '')
        + rt.count_tokens(prompt_text or '')
        + margin
    )
    return max(int(max_context_tokens or 0) - reserved, 0)


def _audit_summary_prefix_budget(*, max_context_tokens: int = rt.MAX_CONTEXT_TOKENS, prompt_text: str = "", system_prompt: str = "") -> int:
    return min(
        _audit_summary_content_budget(
            max_context_tokens=max_context_tokens,
            prompt_text=prompt_text,
            system_prompt=system_prompt,
        ),
        128_000,
    )


def _audit_summary_prefix_text(text: str, *, max_tokens: int) -> str:
    tokens = rt.encode_tokens(text)
    if not tokens:
        return ""
    return rt.decode_tokens(tokens[: max(max_tokens, 1)])


def _audit_summary_prefix_condense_prompt(base_prompt: str, state: dict[str, object], *, issues_rel: str, prefix_text: str, iteration: int, total_tokens: int, content_budget: int, prefix_tokens: int, mode: str = _AUDIT_DEFAULT_MODE) -> str:
    return (
        _audit_summary_prompt(base_prompt, state, issues_rel=issues_rel, mode=mode)
        + f" Budget-prep pass {iteration}: `{issues_rel}` is too large to fit the final phase3 prompt."
        + f" Rewrite only the first {prefix_tokens} tokens so they are materially shorter."
        + " Use only `replace_prefix`; it rewrites just that visible prefix and leaves the unseen suffix untouched."
        + " This is not the final report pass. Preserve the report header/transparency block and the highest-signal evidence from the visible prefix."
        + " Aggressively dedupe repeated findings, collapse lower-signal entries, and shorten prose."
        + " Do not invent facts from the unseen tail or touch any other file."
        + f" Current ISSUES.md size is about {total_tokens} tokens; the final summary input budget is about {content_budget} tokens."
        + "\n\nVisible ISSUES.md prefix to condense:\n\n"
        + prefix_text
    )


def _run_audit_summary_prefix_condense(*, prompt: str, model: str, workspace: Path, unattended_limit_seconds: int, agent: str, state: dict[str, object], max_context_tokens: int = rt.MAX_CONTEXT_TOKENS, iteration: int, content_budget: int, prefix_tokens: int, mode: str = _AUDIT_DEFAULT_MODE) -> tuple[int, str]:
    issues_path = _audit_issues_path(workspace)
    issues_rel = rt._rel(workspace, issues_path)
    issues_text = _audit_read_issues(workspace)
    return run_agent(
        _audit_summary_prefix_condense_prompt(
            prompt,
            state,
            issues_rel=issues_rel,
            prefix_text=_audit_summary_prefix_text(issues_text, max_tokens=prefix_tokens),
            iteration=iteration,
            total_tokens=rt.count_tokens(issues_text),
            content_budget=content_budget,
            prefix_tokens=prefix_tokens,
            mode=mode,
        ),
        model,
        workspace,
        _audit_system_prompt_for_mode(mode, phase="phase3"),
        unattended_limit_seconds,
        interactive=False,
        transcript=_audit_transcript(max_context_tokens=max_context_tokens),
        agent=agent,
        tool_registry=_audit_limited_tool_registry(
            workspace,
            allow_search=False,
            allow_replace=False,
            prefix_replace_tokens=prefix_tokens,
            mode=mode,
        ),
        auto_approve_tools={"replace_prefix"},
        wait_label_suffix=_audit_wait_label_suffix(state, detail=f"condense-{iteration:03d}"),
        pin_user_prompt=True,
    )


def _audit_prepare_summary_input(*, prompt: str, model: str, workspace: Path, unattended_limit_seconds: int, agent: str, state: dict[str, object], max_context_tokens: int = rt.MAX_CONTEXT_TOKENS, mode: str = _AUDIT_DEFAULT_MODE, session_path: Path | None = None) -> tuple[int, str]:
    issues_rel = rt._rel(workspace, _audit_issues_path(workspace))
    prompt_text = _audit_summary_prompt(prompt, state, issues_rel=issues_rel, mode=mode)
    system_prompt = _audit_system_prompt_for_mode(mode, phase="phase3")
    content_budget = _audit_summary_content_budget(
        max_context_tokens=max_context_tokens,
        prompt_text=prompt_text,
        system_prompt=system_prompt,
    )
    prefix_tokens = _audit_summary_prefix_budget(
        max_context_tokens=max_context_tokens,
        prompt_text=prompt_text,
        system_prompt=system_prompt,
    )
    if content_budget <= 0 or prefix_tokens <= 0:
        return rt.fail("Audit phase3 summary budget is too small"), _audit_read_issues(workspace)
    for iteration in range(1, 33):
        before_text = _audit_read_issues(workspace)
        before_tokens = rt.count_tokens(before_text)
        if before_tokens <= content_budget:
            return 0, before_text
        code, message = _run_audit_summary_prefix_condense(
            prompt=prompt,
            model=model,
            workspace=workspace,
            unattended_limit_seconds=unattended_limit_seconds,
            agent=agent,
            state=state,
            max_context_tokens=max_context_tokens,
            iteration=iteration,
            content_budget=content_budget,
            prefix_tokens=prefix_tokens,
            mode=mode,
        )
        after_text = _audit_read_issues(workspace)
        after_tokens = rt.count_tokens(after_text)
        if code != 0:
            return code, message
        if after_text == before_text:
            return rt.fail("Audit phase3 prefix-condense pass did not update ISSUES.md"), after_text
        if after_tokens >= before_tokens:
            return rt.fail("Audit phase3 prefix-condense pass did not reduce ISSUES.md token count"), after_text
        state['totals']['findings'] = _audit_issue_count(after_text)
        state['notes'] = [
            *state.get('notes', [])[-9:],
            f"Condensed phase3 prefix pass {iteration}: ISSUES.md tokens {before_tokens}->{after_tokens}.",
        ]
        _audit_refresh_state(state, force_phase='phase3')
        if session_path is not None:
            _write_audit_state(session_path, state)
    return rt.fail("Audit phase3 prefix-condense exceeded 32 passes"), _audit_read_issues(workspace)


def _audit_issue_count(text: str) -> int:
    bounds = _audit_inbox_bounds(text)
    if bounds is not None:
        inbox = text[bounds[0]:bounds[1]]
        inbox_count = sum(1 for line in inbox.splitlines() if line.startswith(_audit_schema('report_h3_prefix')))
        if inbox_count:
            return inbox_count
    return sum(
        1
        for line in text.splitlines()
        if line.startswith(_audit_schema('report_h2_prefix')) and line[len(_audit_schema('report_h2_prefix')):].strip() not in _audit_non_finding_headings()
    )


def _audit_git_commit_summary(workspace: Path) -> str:
    try:
        result = rt.run_cmd(
            ["git", "-C", str(workspace), "log", "-1", "--pretty=format:%h%x09%s"],
            timeout=10,
        )
    except Exception:
        return "unknown"
    if result.returncode != 0 or not result.stdout.strip():
        return "unknown"
    short, _, subject = result.stdout.partition("	")
    return f"{short.strip()} ({subject.strip()})" if subject.strip() else short.strip()


def _audit_seed_issues_md(workspace: Path, state: dict[str, object]) -> None:
    issues_path = _audit_issues_path(workspace)
    if issues_path.exists():
        return
    run_config = _audit_resolve_run_config(state.get('run_config'), mode=str(state.get('mode') or _AUDIT_DEFAULT_MODE))
    state['run_config'] = run_config
    sloc_plan = state.get("sloc") if isinstance(state.get("sloc"), dict) else {}
    today = datetime.now(UTC).date().isoformat()
    commit_summary = _audit_git_commit_summary(workspace)
    text = (
        f"{_audit_schema('report_title')}\n\n"
        + _audit_transparency_snippet(run_config)
        + "\n\n"
        + f"> **Last audit**: {today} · commit `{commit_summary}` · cross-checked against [OWASP ASVS 5.0](https://owasp.org/www-project-application-security-verification-standard/) and [grugbrain.dev](https://grugbrain.dev/)\n\n"
        + f"> **Scope**: {state.get('totals', {}).get('queued', 0)} reviewable files · {sloc_plan.get('total_code_count', 0)} code lines · {sloc_plan.get('counted_files', 0)} counted by sloc\n\n"
        + _audit_inbox_section()
    )
    _audit_write_issues(workspace, text)

def _audit_mark_chunk_complete(state: dict[str, object], chunk_id: str, before_text: str, after_text: str, *, note: str | None = None, segment: bool = False) -> None:
    key = "completed_segments" if segment else "completed_chunks"
    completed = [item for item in state.get(key, []) if isinstance(item, str)]
    if chunk_id not in completed:
        completed.append(chunk_id)
    state[key] = completed
    totals = state.get("totals") if isinstance(state.get("totals"), dict) else {}
    state["totals"] = totals
    state["totals"]["findings"] = _audit_issue_count(after_text)
    summary = note or f"Completed {chunk_id}: ISSUES.md changed by {len(after_text) - len(before_text)} chars."
    state["notes"] = [
        *state.get("notes", [])[-9:],
        summary,
    ]
    _audit_refresh_state(state)


def _audit_record_failed_chunk(state: dict[str, object], chunk: dict[str, object], reason: str, *, segment: dict[str, object] | None = None) -> None:
    failed = [item for item in state.get("failed_chunks", []) if isinstance(item, dict)]
    failed.append({
        "id": str(chunk["id"]),
        "segment_id": str(segment.get('id')) if isinstance(segment, dict) and segment.get('id') else '',
        "paths": list(chunk["paths"]),
        "reason": reason,
    })
    state["failed_chunks"] = failed[-20:]
    label = str(segment.get('id')) if isinstance(segment, dict) and segment.get('id') else str(chunk['id'])
    state["notes"] = [*state.get("notes", [])[-9:], f"Failed {label}: {reason}"]
    _audit_refresh_state(state)


def _audit_review_worker(
    *,
    prompt: str,
    model: str,
    workspace: Path,
    system_prompt: str,
    unattended_limit_seconds: int,
    agent: str,
    chunk: dict[str, object],
    state: dict[str, object],
    max_context_tokens: int = rt.MAX_CONTEXT_TOKENS,
    mode: str = _AUDIT_DEFAULT_MODE,
    session_path: Path,
) -> dict[str, object]:
    before_text = _audit_read_issues(workspace)
    segment_entries = [item for item in chunk.get('segments', []) if isinstance(item, dict)]
    segments = segment_entries or [None]
    reviewed_segment_ids: list[str] = []
    for segment in segments:
        latest_state = _audit_load_state(session_path) or state
        completed_segments = {item for item in latest_state.get('completed_segments', []) if isinstance(item, str)}
        segment_id = str(segment.get('id') or '') if isinstance(segment, dict) else ''
        if segment_id and segment_id in completed_segments:
            reviewed_segment_ids.append(segment_id)
            continue
        current_inbox = _audit_inbox_context(workspace, max_context_tokens=max_context_tokens)
        code, message = _run_audit_chunk(
            prompt=prompt,
            model=model,
            workspace=workspace,
            system_prompt=system_prompt,
            unattended_limit_seconds=unattended_limit_seconds,
            agent=agent,
            chunk=chunk,
            state=state,
            max_context_tokens=max_context_tokens,
            mode=mode,
            segment=segment if isinstance(segment, dict) else None,
            inbox_text=current_inbox,
        )
        if code != 0:
            return {
                "chunk": dict(chunk),
                "code": code,
                "message": message,
                "before_text": before_text,
                "completed_segments": reviewed_segment_ids,
                "failed_segment": dict(segment) if isinstance(segment, dict) else None,
            }
        if segment_id:
            reviewed_segment_ids.append(segment_id)
            completed_segments.add(segment_id)
            def _mark_segment(current_state: dict[str, object]) -> dict[str, object]:
                completed = [item for item in current_state.get('completed_segments', []) if isinstance(item, str)]
                if segment_id not in completed:
                    completed.append(segment_id)
                current_state['completed_segments'] = completed
                _audit_refresh_state(current_state)
                return current_state
            _audit_update_state(session_path, _mark_segment)
    return {
        "chunk": dict(chunk),
        "code": 0,
        "message": "",
        "before_text": before_text,
        "completed_segments": reviewed_segment_ids,
        "failed_segment": None,
    }


def _run_audit_chunk(*, prompt: str, model: str, workspace: Path, system_prompt: str, unattended_limit_seconds: int, agent: str, chunk: dict[str, object], state: dict[str, object], max_context_tokens: int = rt.MAX_CONTEXT_TOKENS, mode: str = _AUDIT_DEFAULT_MODE, segment: dict[str, object] | None = None, inbox_text: str | None = None, inbox_append_fn = None) -> tuple[int, str]:
    issues_path = _audit_issues_path(workspace)
    issues_rel = rt._rel(workspace, issues_path)
    review_prompt = _audit_review_prompt(prompt, state, chunk, issues_rel=issues_rel, mode=mode, segment=segment)
    current_inbox = inbox_text if isinstance(inbox_text, str) and inbox_text else _audit_inbox_context(workspace, max_context_tokens=max_context_tokens)
    chunk_text = _audit_chunk_text(workspace, chunk, mode=mode, segment=segment)
    return run_agent(
        review_prompt
        + "\n\nCurrent audit inbox:\n\n"
        + current_inbox
        + "\n\nChunk contents:\n\n"
        + chunk_text,
        model,
        workspace,
        _audit_system_prompt_for_mode(mode, phase="phase2"),
        unattended_limit_seconds,
        interactive=False,
        transcript=_audit_transcript(max_context_tokens=max_context_tokens),
        agent=agent,
        tool_registry=_audit_limited_tool_registry(
            workspace,
            allow_search=True,
            allow_inbox_append=True,
            allow_replace=False,
            mode=mode,
            inbox_append_fn=inbox_append_fn or _audit_append_inbox,
        ),
        auto_approve_tools={"inbox_append"},
        wait_label_suffix=_audit_wait_label_suffix(state, chunk=chunk),
        pin_user_prompt=True,
    )


def _run_audit_summary(*, prompt: str, model: str, workspace: Path, system_prompt: str, unattended_limit_seconds: int, agent: str, state: dict[str, object], max_context_tokens: int = rt.MAX_CONTEXT_TOKENS, mode: str = _AUDIT_DEFAULT_MODE) -> tuple[int, str]:
    _ = system_prompt
    issues_path = _audit_issues_path(workspace)
    issues_rel = rt._rel(workspace, issues_path)
    return run_agent(
        _audit_summary_prompt(prompt, state, issues_rel=issues_rel, mode=mode)
        + "\n\nCurrent ISSUES.md:\n\n"
        + _audit_read_issues(workspace),
        model,
        workspace,
        _audit_system_prompt_for_mode(mode, phase="phase3"),
        unattended_limit_seconds,
        interactive=False,
        transcript=_audit_transcript(max_context_tokens=max_context_tokens),
        agent=agent,
        tool_registry=_audit_limited_tool_registry(workspace, allow_search=False, mode=mode),
        auto_approve_tools={"replace"},
        wait_label_suffix=_audit_wait_label_suffix(state),
        pin_user_prompt=True,
    )

def _run_audit_workflow(*, prompt: str, model: str, workspace: Path, system_prompt: str, unattended_limit_seconds: int, agent: str, transcript: Transcript | None = None, max_context_tokens: int = rt.MAX_CONTEXT_TOKENS, mode: str = _AUDIT_DEFAULT_MODE, session_path: Path | None = None, phase: str = "") -> int:
    _ = transcript
    _ = max_context_tokens
    session_path = session_path or _audit_session_path(workspace, mode=mode)
    state = _audit_load_state(session_path)
    if state is None:
        return rt.fail(f"Audit state missing or invalid: {rt._fmt('inline', session_path)}")
    state['run_config'] = _audit_resolve_run_config(state.get('run_config'), mode=str(state.get('mode') or mode))
    requested_phase = str(phase or '').strip().lower()
    if requested_phase and requested_phase not in _AUDIT_PHASE_IDS:
        return rt.fail(f"Invalid audit phase: {requested_phase}")
    _write_audit_state(session_path, state)
    prepared_issues = _audit_prepare_issues_md(workspace, state)
    state['totals']['findings'] = int(prepared_issues.get('findings', 0) or 0)
    _audit_ensure_inbox(workspace)
    files_by_path = {str(item['path']): item for item in state.get('files', []) if isinstance(item, dict) and isinstance(item.get('path'), str)}
    queue = [
        _audit_normalize_chunk(chunk, files_by_path)
        for chunk in state.get('chunks', [])
        if isinstance(chunk, dict)
    ]
    forced_phase = requested_phase if requested_phase in _AUDIT_PHASE_IDS else ''
    if requested_phase == 'phase1':
        state = _audit_refresh_state(state, force_phase='phase1')
        _write_audit_state(session_path, state)
        rt._note(f"audit plan ready: {_audit_state_summary(state)}", tag="note")
        return 0
    if requested_phase == 'phase3':
        queue = []
    state = _audit_refresh_state(state, force_phase=forced_phase)
    _write_audit_state(session_path, state)
    rt._note(f"audit plan ready: {_audit_state_summary(state)}", tag="note")
    worker_count = int(state.get('run_config', {}).get('phase2_workers') or rt.audit_settings(context_tokens=max_context_tokens).get('phase2_workers', 1) or 1)
    worker_count = max(worker_count, 1)
    launch_delay_seconds = max(int(state.get('run_config', {}).get('phase2_launch_delay_seconds') or rt.audit_settings(context_tokens=max_context_tokens).get('phase2_launch_delay_seconds', 10) or 0), 0)
    if queue:
        rt._note(f"phase2 threaded review enabled: workers={worker_count}, launch_delay={launch_delay_seconds}s", tag="note")
    while queue:
        ready = [
            chunk for chunk in queue
            if chunk.get('paths') and str(chunk.get('id')) not in set(state.get('completed_chunks', []))
        ]
        queue = []
        if not ready:
            break
        inbox_text = _audit_inbox_context(workspace, max_context_tokens=max_context_tokens)
        review_prompt = _audit_review_prompt(prompt, state, {"id": "chunk", "paths": [], "estimated_tokens": 0}, issues_rel="ISSUES.md", mode=mode)
        phase2_system_prompt = _audit_system_prompt_for_mode(mode, phase="phase2")
        budget = _audit_chunk_content_budget(
            max_context_tokens=max_context_tokens,
            inbox_text=inbox_text,
            prompt_text=review_prompt,
            system_prompt=phase2_system_prompt,
        )
        for current_chunk in ready:
            segment_entries = [item for item in current_chunk.get('segments', []) if isinstance(item, dict)]
            current_chunk['segments'] = segment_entries
            current_chunk['segment_count'] = max(int(current_chunk.get('segment_count', 0) or 0), len(segment_entries))
            if not segment_entries and budget > 0 and rt.count_tokens(_audit_chunk_text(workspace, current_chunk, mode=mode)) > budget:
                segment_entries = _audit_review_segments(
                    workspace,
                    current_chunk,
                    prompt=prompt,
                    max_context_tokens=max_context_tokens,
                    mode=mode,
                )
                current_chunk['segments'] = segment_entries
                current_chunk['segment_count'] = len(segment_entries)
            elif not segment_entries:
                current_chunk['segment_count'] = 0
            for index, queued_chunk in enumerate(state.get('chunks', [])):
                if isinstance(queued_chunk, dict) and str(queued_chunk.get('id')) == str(current_chunk.get('id')):
                    state['chunks'][index] = dict(current_chunk)
                    break
        _write_audit_state(session_path, state)
        with concurrent.futures.ThreadPoolExecutor(max_workers=min(worker_count, len(ready))) as pool:
            futures = []
            for index, current_chunk in enumerate(ready):
                futures.append(
                    pool.submit(
                        _audit_review_worker,
                        prompt=prompt,
                        model=model,
                        workspace=workspace,
                        system_prompt=system_prompt,
                        unattended_limit_seconds=unattended_limit_seconds,
                        agent=agent,
                        chunk=current_chunk,
                        state=state,
                        max_context_tokens=max_context_tokens,
                        mode=mode,
                        session_path=session_path,
                    )
                )
                if launch_delay_seconds > 0 and index + 1 < len(ready):
                    time.sleep(launch_delay_seconds)
            batch_failures: list[tuple[dict[str, object], dict[str, object] | None]] = []
            for future in concurrent.futures.as_completed(futures):
                result = future.result()
                current_chunk = _audit_normalize_chunk(dict(result.get('chunk', {})), files_by_path)
                before_text = _audit_read_issues(workspace)
                result_code = int(result.get('code', 1) if result.get('code') is not None else 1)
                failed_segment = result.get('failed_segment') if isinstance(result.get('failed_segment'), dict) else None
                if result_code != 0:
                    reason = 'agent error during chunk review'
                    state = _audit_update_state(
                        session_path,
                        lambda current_state: (
                            _audit_record_failed_chunk(current_state, current_chunk, reason, segment=failed_segment)
                            or current_state
                        ),
                    )
                    if len(current_chunk['paths']) > 1:
                        split = _audit_split_chunk(current_chunk, files_by_path)
                        if split:
                            queue.extend(split)
                            state = _audit_update_state(
                                session_path,
                                lambda current_state: (
                                    current_state.__setitem__('notes', [*current_state.get('notes', [])[-9:], f"Split {current_chunk['id']} after threaded review failure."])
                                    or _audit_refresh_state(current_state)
                                    or current_state
                                ),
                            )
                            continue
                    batch_failures.append((current_chunk, failed_segment))
                    continue
                after_text = _audit_read_issues(workspace)
                segment_note = ''
                if current_chunk.get('segments'):
                    segment_note = f" across {len([item for item in current_chunk.get('segments', []) if isinstance(item, dict)])} segment(s)"
                state = _audit_update_state(
                    session_path,
                    lambda current_state: (
                        _audit_mark_chunk_complete(
                            current_state,
                            str(current_chunk['id']),
                            before_text,
                            after_text,
                            note=(
                                f"Completed {current_chunk['id']}{segment_note} with threaded phase2 review: direct locked inbox writes; "
                                f"ISSUES.md changed by {len(after_text) - len(before_text)} chars."
                            ),
                        )
                        or current_state
                    ),
                )
            if batch_failures:
                failed_chunk, failed_segment = batch_failures[0]
                label = str(failed_segment.get('id')) if isinstance(failed_segment, dict) else str(failed_chunk['id'])
                return rt.fail(f"Audit chunk {label} failed: agent error during chunk review")
    state = _audit_refresh_state(state, force_phase='phase3' if requested_phase == 'phase3' else '')
    _write_audit_state(session_path, state)
    if requested_phase == 'phase2':
        return 0
    code, _ = _audit_prepare_summary_input(
        prompt=prompt,
        model=model,
        workspace=workspace,
        unattended_limit_seconds=unattended_limit_seconds,
        agent=agent,
        state=state,
        max_context_tokens=max_context_tokens,
        mode=mode,
        session_path=session_path,
    )
    if code != 0:
        return code
    state = _audit_refresh_state(state, force_phase='phase3')
    _write_audit_state(session_path, state)
    before_summary = _audit_read_issues(workspace)
    code, _ = _run_audit_summary(
        prompt=prompt,
        model=model,
        workspace=workspace,
        system_prompt=system_prompt,
        unattended_limit_seconds=unattended_limit_seconds,
        agent=agent,
        state=state,
        max_context_tokens=max_context_tokens,
        mode=mode,
    )
    after_summary = _audit_read_issues(workspace)
    final_summary = _audit_upsert_transparency(after_summary, state.get('run_config'))
    if final_summary != after_summary:
        _audit_write_issues(workspace, final_summary)
    after_summary = final_summary
    if code != 0:
        return code
    if before_summary == after_summary:
        return rt.fail("Audit summary pass did not update ISSUES.md")
    state['status'] = 'done'
    state['totals']['findings'] = _audit_issue_count(after_summary)
    state['notes'] = [*state.get('notes', [])[-9:], 'Condensed ISSUES.md to keep top 10-15 detailed findings and the rest concise.']
    _audit_refresh_state(state)
    _write_audit_state(session_path, state)
    return 0

def _renovate_github_token() -> str | None:
    for var in ("RENOVATE_GITHUB_COM_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"):
        token = os.environ.get(var)
        if isinstance(token, str) and token:
            return token
    try:
        result = rt.run_cmd(["gh", "auth", "token"], timeout=10)
    except Exception:
        return None
    token = result.stdout.strip()
    return token if result.returncode == 0 and token else None


def workspace_root():
    workspace = Path(os.environ.get("OY_ROOT", ".")).expanduser().resolve()
    if not workspace.is_dir():
        rt.abort(f"Workspace root is not a directory: {rt._fmt('inline', workspace)}")
    return workspace


def resolve_session(
    *,
    interactive: bool | None = None,
    system_prompt: str | None = None,
    include_system_file: bool = True,
    agent: str = "default",
):
    resolved_interactive = rt.can_prompt() if interactive is None else interactive
    resolved_agent = rt.normalize_agent_profile(agent)
    profile = rt.agent_profile(resolved_agent)
    system_file = rt._sys_file() if include_system_file else None
    model_spec = rt._model(None)
    return rt.session_context(
        workspace=workspace_root(),
        model=model_spec,
        interactive=resolved_interactive,
        system_prompt=(
            load_system_prompt(system_file, resolved_interactive, agent=resolved_agent)
            if system_prompt is None
            else system_prompt
        ),
        system_file=system_file,
        yolo=rt.yolo_enabled() or bool(profile["yolo"]),
        agent=resolved_agent,
    )


def _saved_session_context(path: Path) -> tuple[Transcript, str, str | None]:
    data = rt.load_json(path, None)
    if not isinstance(data, dict) or "transcript" not in data:
        rt.abort(f"Invalid saved session: {rt._fmt('inline', path.name)}")
    loaded = _load_transcript(data["transcript"])
    model = data.get("model")
    if not isinstance(model, str) or not model:
        rt.abort(f"Saved session missing model: {rt._fmt('inline', path.name)}")
    saved_agent = data.get("agent") if isinstance(data.get("agent"), str) else None
    return loaded, model, saved_agent


def _load_saved_session_context(name: str | None) -> tuple[Transcript, str, str | None]:
    target = _resolve_saved_session(name)
    if target is None:
        rt.abort("No saved sessions found.")
    return _saved_session_context(target)


def _resume_target(continue_session: bool, resume: str) -> str | None:
    if continue_session and resume:
        rt.abort("Use either `--continue` or `--resume`, not both.")
    return None if continue_session else (resume or None)


def _resume_state(continue_session: bool, resume: str) -> tuple[bool, Transcript | None, str | None, str | None]:
    target = _resume_target(continue_session, resume)
    if target is None and not continue_session:
        return False, None, None, None
    loaded, loaded_model, loaded_agent = _load_saved_session_context(target)
    return True, loaded, loaded_model, loaded_agent


def _apply_resumed_session(session, loaded, loaded_model, loaded_agent):
    if loaded is None or loaded_model is None:
        return None, session["model"]
    set_system_prompt(loaded, session["system_prompt"])
    session["model"] = loaded_model
    session["agent"] = rt.normalize_agent_profile(loaded_agent or session["agent"])
    return loaded, loaded_model


def _run_audit_entrypoint(*, focus: str, mode: str, from_: str = "", phase: str = "") -> int:
    try:
        scope = _audit_parse_scope(from_)
    except ValueError as exc:
        return rt.fail(str(exc))
    session = resolve_session(
        interactive=False,
        system_prompt=_audit_system_prompt_for_mode(mode, phase="phase1"),
        include_system_file=False,
        agent="default",
    )
    artifacts, audit_prompt = _prepare_audit_run(session=session, focus=focus, interactive=False, mode=mode, scope=scope)
    _print_session_intro(
        _audit_title(mode),
        session,
        focus=rt.preview(focus, 100) if focus else None,
        audit_state=artifacts["session_path"],
    )
    return _run_audit_workflow(
        prompt=audit_prompt,
        model=session["model"],
        workspace=session["workspace"],
        system_prompt=session["system_prompt"],
        unattended_limit_seconds=rt.unattended_limit_seconds(),
        agent=session["agent"],
        max_context_tokens=int(session.get("max_context_tokens", rt.MAX_CONTEXT_TOKENS) or rt.MAX_CONTEXT_TOKENS),
        mode=mode,
        session_path=artifacts["session_path"],
        phase=phase,
    )


def audit(focus: str = "", *, from_: str = "", phase: str = ""):
    """Run a one-shot security and complexity audit.

    :param focus: Optional area to focus on, such as auth, tests, or a file path.
    :param phase: Optional phase selector: phase1, phase2, or phase3.
    """
    return _run_audit_entrypoint(focus=focus, mode=_AUDIT_DEFAULT_MODE, from_=from_, phase=phase)


def audit_logic(focus: str = "", *, from_: str = "", phase: str = ""):
    """Run a one-shot logic-focused security and complexity audit.

    :param focus: Optional area to focus on, such as auth, tests, or a file path.
    :param phase: Optional phase selector: phase1, phase2, or phase3.
    """
    return _run_audit_entrypoint(focus=focus, mode=_AUDIT_LOGIC_MODE, from_=from_, phase=phase)


def _create_prompt_session():
    history_path = rt._history_path()
    return rt.prompt_session(
        console=rt.STDERR,
        history=FileHistory(str(history_path)),
        choices=_PROMPT_COMMANDS,
        multiline=False,
        enable_open_in_editor=True,
    )


def _git_diff_shortstat(workspace: Path) -> str | None:
    try:
        status_result = rt.run_cmd(
            [
                "git",
                "-C",
                str(workspace),
                "status",
                "--short",
                "--untracked-files=all",
            ],
            timeout=5,
        )
    except Exception:
        return None
    if status_result.returncode != 0:
        return None
    lines = [line for line in status_result.stdout.splitlines() if line.strip()]
    if not lines:
        return "git diff: clean"
    counts = {"staged": 0, "modified": 0, "untracked": 0}
    for line in lines:
        code = (line[:2] + "  ")[:2]
        if code[0] == "?" and code[1] == "?":
            counts["untracked"] += 1
            continue
        if code[0] not in {" ", "?"}:
            counts["staged"] += 1
        if code[1] not in {" ", "?"}:
            counts["modified"] += 1
    parts = [f"{len(lines)} change{'s' if len(lines) != 1 else ''}"]
    for key in ("staged", "modified", "untracked"):
        count = counts[key]
        if count:
            parts.append(f"{count} {key}")
    try:
        numstat_result = rt.run_cmd(
            ["git", "-C", str(workspace), "diff", "--numstat", "HEAD", "--"],
            timeout=5,
        )
    except Exception:
        numstat_result = None
    if numstat_result and numstat_result.returncode == 0:
        added = 0
        deleted = 0
        for line in numstat_result.stdout.splitlines():
            if not line.strip():
                continue
            fields = line.split("	", 2)
            if len(fields) < 2:
                continue
            if fields[0].isdigit():
                added += int(fields[0])
            if fields[1].isdigit():
                deleted += int(fields[1])
        line_parts = []
        if added:
            line_parts.append(f"+{added}")
        if deleted:
            line_parts.append(f"-{deleted}")
        if line_parts:
            parts.append("lines " + " ".join(line_parts))
    return "; ".join(parts)


def _read_input(prompt_session, workspace: Path):
    prompt = "\x1b[1;32moy ❯\x1b[0m "
    if summary := _git_diff_shortstat(workspace):
        return prompt_session.prompt(rt.ANSI(f"\x1b[2m{summary}\x1b[0m\n{prompt}"))
    return prompt_session.prompt(rt.ANSI(prompt))


def _chat_command(cmd, transcript, system_prompt, model_spec):
    parts = cmd.strip().split(None, 1)
    name = parts[0].lower()
    arg = parts[1].strip() if len(parts) > 1 else ""
    _, model = rt.split_model_spec(model_spec)
    if name in {"/help", "/?"}:
        lines = ["## Commands", ""]
        lines.extend(
            f"- `{command}` -- {description}"
            for command, description in _CHAT_COMMAND_HELP[:-2]
        )
        lines.append("- `/quit` or `/exit` -- end session")
        lines.extend(
            [
                "",
                "Older conversation history may be packed into TOON before model requests.",
                "Paste multiline text directly — bracketed paste keeps it intact.",
                "Press Meta+E to open your $EDITOR for longer prompts.",
            ]
        )
        rt._print(value="\n".join(lines), err=True)
        return True
    if name == "/tokens":
        total = session_tokens(transcript)
        prepped = prepared_tokens(transcript, model=model)
        budget = transcript["max_context_tokens"]
        rt._print(
            value="\n".join(
                [
                    "## Context",
                    "",
                    f"- messages: {len(transcript['messages'])}",
                    f"- session tokens: {rt.format_tokens(total)}",
                    f"- prepared tokens: {rt.format_tokens(prepped)}",
                    f"- context budget: {rt.format_tokens(budget)}",
                    f"- remaining: ~{rt.format_tokens(max(budget - prepped, 0))}",
                ]
            ),
            err=True,
        )
        return True
    if action := _CHAT_ACTIONS.get(name):
        return (action, arg) if action in _CHAT_ACTIONS_WITH_ARGS else (action,)
    if name == "/undo":
        if undo_last_turn(transcript):
            rt._note("undid last turn", tag="note")
        else:
            rt._warn("Nothing to undo.")
        return True
    if name == "/clear":
        clear_transcript(transcript, system_prompt)
        rt._note("cleared conversation", tag="note")
        return True
    if name in ("/quit", "/exit"):
        return None
    return False


def _handle_model_switch(arg, current_model):
    if rt._flag("OY_LOCK_MODEL", default=False):
        rt._warn("Model changes are disabled for this run.")
        return current_model
    if not arg:
        rt._print(value=_current_model_text(current_model), err=True)
        rt._note("use /model <name> to switch, or /model list to browse", tag="note")
        return current_model
    if arg.lower() == "list":
        try:
            chosen = resolve_model_choice()
        except SystemExit:
            return current_model
        return chosen if chosen else current_model
    try:
        all_models = rt.list_all_model_ids()
    except SystemExit:
        rt._warn("Could not load model list.")
        return current_model
    if arg in all_models:
        rt._note(f"switched model: {arg}", tag="note")
        return arg
    matches, notes = rt.matching_model_ids(arg, all_models)
    if len(matches) == 1:
        rt._note(f"switched model: {matches[0]}", tag="note")
        return matches[0]
    if matches:
        rt.render_model_list(
            matches,
            title="## Matching Models",
            query=arg,
            current=current_model,
            err=True,
            notes=notes,
        )
        rt._print(
            value="Be more specific or use `/model list` to choose interactively.",
            err=True,
        )
    else:
        rt._warn(f"No models matching {rt._fmt('inline', arg)}.")
    return current_model


def _handle_debug_toggle():
    if rt._debug_logger is not None:
        for handler in list(rt._debug_logger.handlers):
            handler.close()
            rt._debug_logger.removeHandler(handler)
        rt._debug_logger = None
        rt._debug_log_path = None
        rt._note("debug logging disabled", tag="note")
    else:
        os.environ["OY_DEBUG"] = "1"
        rt._debug_logger, rt._debug_log_path = rt._init_debug_log()
        rt._note(f"debug logging enabled: {rt._debug_log_path}", tag="note")


def _run_mode(fn, *, cancel_note: str, error_prefix: str) -> None:
    try:
        fn()
    except KeyboardInterrupt:
        rt._note(cancel_note, tag="note")
    except Exception as exc:
        rt._error(f"{error_prefix}: {exc}")


def _handle_ask(question, current_model, session, transcript):
    if not question:
        rt._print(value=_ASK_USAGE, err=True)
        return
    read_only_registry = read_only_tool_registry()
    ask_transcript = transcript_with_system_prompt(
        ask_system_prompt(session["system_prompt"])
    )
    ask_transcript["messages"].extend(
        msg for msg in transcript["messages"][-6:] if msg.get("role") != "system"
    )
    rt._note(_ASK_MODE_NOTE, tag="note")
    state = new_agent_state(
        root=session["workspace"],
        tool_registry=read_only_registry,
        unattended_limit_seconds=rt.unattended_limit_seconds(),
        interactive=session["interactive"],
    )
    add_user(ask_transcript, question)

    def run_research():
        run_turn(
            rt.get_client(current_model),
            ask_transcript,
            state,
            current_model,
            tool_specs(read_only_registry),
        )

    _run_mode(
        run_research,
        cancel_note="research cancelled",
        error_prefix="Research error",
    )


def _handle_audit(focus, current_model, session, transcript=None, *, mode: str = _AUDIT_DEFAULT_MODE):
    phase = ""
    if isinstance(focus, str) and focus.startswith("--phase "):
        parts = focus.split(None, 2)
        if len(parts) >= 2:
            phase = parts[1].strip()
        focus = parts[2].strip() if len(parts) >= 3 else ""
    try:
        artifacts, audit_prompt = _prepare_audit_run(session=session, focus=focus, interactive=True, mode=mode)
    except (RuntimeError, ValueError) as exc:
        if str(exc) == "audit cancelled":
            rt._note(f"{_audit_mode_name(mode)} cancelled", tag="note")
            return
        raise

    rt._note(f"{_audit_mode_name(mode)} mode", tag="note")

    def run_audit():
        code = _run_audit_workflow(
            prompt=audit_prompt,
            model=current_model,
            workspace=session["workspace"],
            system_prompt=_audit_system_prompt_for_mode(mode, phase="phase1"),
            unattended_limit_seconds=rt.unattended_limit_seconds(),
            agent=session["agent"],
            max_context_tokens=int(
                (transcript or {}).get("max_context_tokens", session.get("max_context_tokens", rt.MAX_CONTEXT_TOKENS))
                or rt.MAX_CONTEXT_TOKENS
            ),
            mode=mode,
            session_path=artifacts["session_path"],
            phase=phase,
        )
        if code != 0:
            raise RuntimeError(f"{_audit_mode_name(mode)} failed with exit code {code}")

    _run_mode(
        run_audit,
        cancel_note=f"{_audit_mode_name(mode)} cancelled",
        error_prefix=f"{_audit_title(mode)} error",
    )


def _handle_save(name, transcript, current_model, current_agent):
    rt._ensure_private_dir(_sessions_dir())
    if not name:
        name = time.strftime("%Y%m%d-%H%M%S")
    path = _session_file(name)
    data = {
        "model": current_model,
        "agent": current_agent,
        "saved_at": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "transcript": _transcript_data(transcript),
    }
    rt.save_json(path, data)
    rt._note(f"saved session: {path.name}", tag="note")


def _handle_load(name, transcript, current_model, system_prompt, current_agent):
    sessions = _list_saved_sessions()
    if not sessions:
        rt._warn("No saved sessions found.")
        return transcript, current_model, current_agent
    if not name:
        lines = ["## Saved Sessions", ""]
        for index, path in enumerate(sessions[:20], 1):
            meta = rt.load_json(path, {})
            if not isinstance(meta, dict):
                lines.append(f"{index}. {rt._fmt('inline', path.stem)} — (unreadable)")
                continue
            model = meta.get("model", "?")
            agent = meta.get("agent", "default")
            saved = meta.get("saved_at", "?")
            msgs = len(meta.get("transcript", {}).get("messages", []))
            lines.append(
                f"{index}. {rt._fmt('inline', path.stem)} — {model}, agent: {agent}, {msgs} msgs, {saved}"
            )
        lines.extend(["", "Usage: `/load <name>` or `/load <number>`"])
        rt._print(value="\n".join(lines), err=True)
        return transcript, current_model, current_agent
    target = _resolve_saved_session(name)
    if target is None:
        rt._warn(f"No session found matching {rt._fmt('inline', name)}.")
        return transcript, current_model, current_agent
    try:
        loaded, loaded_model, loaded_agent = _saved_session_context(target)
        set_system_prompt(loaded, system_prompt)
        resolved_agent = rt.normalize_agent_profile(loaded_agent or current_agent)
        rt._note(
            f"loaded session: {target.stem} ({len(loaded['messages'])} messages, model: {loaded_model}, agent: {resolved_agent})",
            tag="note",
        )
        return loaded, loaded_model, resolved_agent
    except Exception as exc:
        rt._error(f"Failed to load session: {exc}")
        return transcript, current_model, current_agent


def chat(
    *,
    yolo: bool = False,
    agent: str = "default",
    continue_session: bool = False,
    resume: str = "",
):
    """Start an interactive multi-turn chat session.

    :param yolo: Allow all tools without per-action approval prompts.
    :param agent: Agent profile (`default`, `plan`, `accept-edits`, `auto-approve`).
    :param continue_session: Continue from the most recent saved session.
    :param resume: Resume a specific saved session by name, number, or partial match.
    """
    if yolo:
        os.environ["OY_YOLO"] = "1"
    prompt_session = _create_prompt_session()
    resumed, loaded, loaded_model, loaded_agent = _resume_state(
        continue_session, resume
    )
    if continue_session and loaded_agent:
        agent = loaded_agent
    session = resolve_session(interactive=True, agent=agent)
    transcript, current_model = _apply_resumed_session(
        session, loaded, loaded_model, loaded_agent
    )
    if transcript is None:
        transcript = transcript_with_system_prompt(session["system_prompt"])
        current_model = session["model"]
    _print_session_intro("Chat", session, session=("continued" if resumed else None))
    rt._note(
        "chat mode; /help for commands" + ("; yolo on" if session["yolo"] else ""),
        tag="note",
    )

    while True:
        try:
            rt.print_console(rt.STDERR)
            rt.rule_console(rt.STDERR, style="dim")
            prompt = _read_input(prompt_session, session["workspace"])
        except KeyboardInterrupt:
            rt.print_console(rt.STDERR)
            continue
        except EOFError:
            rt._note("session ended", tag="note")
            break

        if not prompt.strip():
            continue
        if prompt.strip().startswith("/"):
            result = _chat_command(
                prompt.strip(), transcript, session["system_prompt"], current_model
            )
            if result is None:
                break
            if isinstance(result, tuple):
                if result[0] == "model":
                    current_model = _handle_model_switch(result[1], current_model)
                    _apply_session_title(session["workspace"], current_model)
                elif result[0] == "debug":
                    _handle_debug_toggle()
                elif result[0] == "yolo":
                    if session["yolo"]:
                        rt._note("yolo already enabled for this session", tag="note")
                    else:
                        session["yolo"] = True
                        rt._note(
                            "yolo enabled; all tools allowed for this session",
                            tag="note",
                        )
                elif result[0] == "ask":
                    _handle_ask(result[1], current_model, session, transcript)
                elif result[0] == "audit":
                    _handle_audit(result[1], current_model, session, transcript)
                elif result[0] == "audit_logic":
                    _handle_audit(result[1], current_model, session, transcript, mode=_AUDIT_LOGIC_MODE)
                elif result[0] == "save":
                    _handle_save(result[1], transcript, current_model, session["agent"])
                elif result[0] == "load":
                    transcript, current_model, session["agent"] = _handle_load(
                        result[1],
                        transcript,
                        current_model,
                        session["system_prompt"],
                        session["agent"],
                    )
                    _apply_session_title(session["workspace"], current_model)
                continue
            if result:
                continue
            rt._warn(f"Unknown command: {prompt.strip().split()[0]}")
            continue
        if prompt.strip().lower() in ("exit", "quit"):
            break
        checkpoint_point = checkpoint(transcript)
        try:
            code, _ = run_agent(
                prompt,
                current_model,
                session["workspace"],
                session["system_prompt"],
                rt.unattended_limit_seconds(),
                session["interactive"],
                yolo=session["yolo"],
                transcript=transcript,
                agent=session["agent"],
            )
        except KeyboardInterrupt:
            rollback(transcript, checkpoint_point)
            rt._note("cancelled; prompt still in history (press ↑)", tag="note")
            continue
        except Exception as exc:
            rollback(transcript, checkpoint_point)
            rt._error(f"Agent error: {exc}")
            rt._note("prompt still in history (press ↑)", tag="note")
            continue

        _ = code
        _, model = rt.split_model_spec(current_model)
        prepped = prepared_tokens(transcript, model=model)
        remaining = max(transcript["max_context_tokens"] - prepped, 0)
        rt._note(
            f"context: {rt.format_tokens(prepped)} used, ~{rt.format_tokens(remaining)} remaining",
            tag="note",
        )

    _set_terminal_title("")
    return 0


def run(
    *task: str,
    agent: str = "default",
    continue_session: bool = False,
    resume: str = "",
):
    """Run a one-shot task.

    :param task: Task text. If omitted, read from stdin or start chat in a TTY.
    :param agent: Agent profile (`default`, `plan`, `accept-edits`, `auto-approve`).
    :param continue_session: Continue from the most recent saved session.
    :param resume: Resume a specific saved session by name, number, or partial match.
    """
    task_text = _task_text(task)
    if not task_text:
        return chat(agent=agent, continue_session=continue_session, resume=resume)
    resumed, loaded, loaded_model, loaded_agent = _resume_state(
        continue_session, resume
    )
    session = resolve_session(interactive=False, agent=agent)
    transcript, _ = _apply_resumed_session(session, loaded, loaded_model, loaded_agent)
    _print_session_intro(
        "Run",
        session,
        prompt=rt.preview(task_text, 100),
        session=("continued" if resumed else None),
    )
    return run_agent(
        task_text,
        session["model"],
        session["workspace"],
        session["system_prompt"],
        rt.unattended_limit_seconds(),
        session["interactive"],
        transcript=transcript,
        agent=session["agent"],
    )[0]


def ralph(*task: str, agent: str = "default"):
    """Run a task in yolo mode every minute until the configured deadline.

    Controlled by `OY_RALPH_LIMIT` (default: `3h`).

    :param task: Task text. If omitted, read from stdin.
    :param agent: Agent profile (`default`, `plan`, `accept-edits`, `auto-approve`).
    """
    task_text = _task_text(task)
    if not task_text:
        rt._print(
            value="Usage: `oy ralph <prompt>` — or pipe prompt text on stdin.",
            err=True,
        )
        return 1

    session = resolve_session(interactive=False, agent=agent)
    session["yolo"] = True
    delay_seconds = 60
    limit_seconds = rt.ralph_limit_seconds()
    deadline = time.monotonic() + limit_seconds
    _print_session_intro(
        "Ralph",
        session,
        prompt=rt.preview(task_text, 100),
        schedule=f"until {rt._format_duration(limit_seconds)} deadline, {rt._format_duration(delay_seconds)} delay",
    )

    exit_code = 0
    run_number = 0
    while True:
        now = time.monotonic()
        if run_number > 0 and now >= deadline:
            break
        run_number += 1
        remaining = max(int(deadline - now), 0)
        rt._note(
            f"ralph run {run_number} (~{rt._format_duration(remaining)} remaining)",
            tag="note",
        )
        with _ralph_run_env(session["model"]):
            code, _ = run_agent(
                task_text,
                session["model"],
                session["workspace"],
                session["system_prompt"],
                rt.unattended_limit_seconds(),
                session["interactive"],
                yolo=True,
                agent=session["agent"],
            )
        if code != 0:
            exit_code = code
        sleep_seconds = deadline - time.monotonic()
        if sleep_seconds <= 0:
            break
        time.sleep(min(delay_seconds, sleep_seconds))
    return exit_code


def renovate_local():
    """Run Renovate locally and write a lookup report to `.tmp/renovate-<date>.json`."""
    workspace = workspace_root()
    try:
        tmp_dir = _ensure_tmp_dir(workspace)
        _ensure_tmp_gitignored(workspace)
        config_path = _ensure_renovate_config(workspace)
    except RuntimeError as exc:
        return rt.fail(exc)

    report_path = tmp_dir / f"renovate-{time.strftime('%Y-%m-%d')}.json"
    token = _renovate_github_token()
    if token is None:
        return rt.fail(
            "No GitHub token found (set RENOVATE_GITHUB_COM_TOKEN or run `gh auth login`)."
        )
    command = [
        "renovate",
        "--platform=local",
        "--require-config=ignored",
        "--dry-run=lookup",
        "--report-type=file",
        "--report-path",
        f".tmp/{report_path.name}",
    ]

    rt._print(
        value="\n".join(
            [
                "## Renovate Local",
                "",
                f"- workspace: {rt._fmt('inline', workspace)}",
                f"- report: {rt._fmt('inline', Path('.tmp') / report_path.name)}",
                f"- config: {rt._fmt('inline', config_path.name)}",
            ]
        ),
        err=True,
    )

    env = dict(rt.command_env())
    env["RENOVATE_GITHUB_COM_TOKEN"] = token
    try:
        result = subprocess.run(command, cwd=workspace, env=env, check=False)
    except OSError as exc:
        return rt.fail(f"Could not run Renovate: {exc}")
    if result.returncode == 0:
        rt._note(f"renovate report written: .tmp/{report_path.name}", tag="note")
    return result.returncode


def _current_model_text(model_spec: str) -> str:
    shim = rt.resolve_active_shim(model_spec)
    _, bare = rt.split_model_spec(model_spec)
    return (
        f"## Current Model\n\n- model: {rt._fmt('inline', bare)}\n"
        f"- shim: {rt._fmt('inline', shim)}"
    )


def _render_choose_model_list(items, *, title, current=None, query=None, notes=None):
    rt.render_model_list(
        items,
        title=title,
        current=current,
        query=query,
        err=True,
        prompt_hint="Enter a number, exact model ID, or filter text.",
        notes=notes,
    )


def resolve_model_choice(model_id=None):
    available, current = rt.list_all_model_ids(), rt._model(None)
    if model_id in available:
        return model_id
    if not rt.can_prompt():
        if model_id:
            matches, notes = [model for model in available if model_id.lower() in model.lower()], None
            if matches:
                _render_choose_model_list(
                    matches,
                    title="## Matching Models",
                    query=model_id,
                    current=current,
                    notes=notes,
                )
            rt.abort(
                f"No exact model match for {rt._fmt('inline', model_id)}. Re-run in a TTY to filter and choose interactively."
            )
        return None
    if model_id is None:
        _render_choose_model_list(
            available,
            title="## Choose a Model",
            current=current,
        )
    shown = available
    query = (
        model_id
        or rt.ask("Model or filter", console=rt.STDERR, default=current).strip()
    )
    while True:
        query = query.strip() or current
        if query in available:
            return query
        if query.isdigit() and 1 <= (index := int(query)) <= len(shown):
            return shown[index - 1]
        shown, notes = [model for model in available if query.lower() in model.lower()], None
        _render_choose_model_list(
            shown,
            title="## Matching Models",
            query=query,
            current=current,
            notes=notes,
        )
        query = rt.ask("Model or filter", console=rt.STDERR).strip()


def model(model: str | None = None):
    """Show or change the default model.

    :param model: Exact model id or filter text to select from available models.
    """
    current = rt._model(None)
    if model is None and not rt.can_prompt():
        rt._print(value=_current_model_text(current))
        return 0
    if current:
        rt._print(value=_current_model_text(current), err=True)
        if model is None and not rt.yes_no(
            "Pick a new model?", console=rt.STDERR, default=False
        ):
            return 0
    chosen = resolve_model_choice(model)
    if chosen is None:
        return 1
    config = rt.save_model_config(chosen)
    rt._print(
        value=(
            f"## Default Model Updated\n\n- selected: {rt._fmt('inline', chosen)}"
            + (
                f"\n- shim: {rt._fmt('inline', config['shim'])}"
                if config["shim"]
                else ""
            )
        )
    )
    return 0


def main(argv: list[str] | None = None):
    """Run the top-level `oy` CLI.

    Global behavior:
    - bare text defaults to `run`
    - `--version` works at the top level
    - other flags must follow an explicit subcommand
    """
    args = list(sys.argv[1:] if argv is None else argv)

    commands = {"run", "chat", "ralph", "model", "audit", "audit-logic", "renovate-local", "-h", "--help"}
    if not args:
        args = ["run"] if not rt.stdin_is_interactive() else ["--help"]
    elif args[0] in {"-v", "--version"}:
        rt._print(value=f"oy {rt.__version__}")
        return 0
    elif args[0] in {"--continue", "-c"}:
        args = ["run", "--continue-session", *args[1:]]
    elif args[0] == "--resume":
        args = ["run", "--resume", *args[1:]]
    elif not args[0].startswith("-") and args[0] not in commands:
        args = ["run", *args]
    result = defopt.run(
        {
            "run": run,
            "chat": chat,
            "ralph": ralph,
            "model": model,
            "audit": audit,
            "audit-logic": audit_logic,
            "renovate-local": renovate_local,
        },
        argv=args,
        version=rt.__version__,
        short={},
        show_defaults=False,
        no_negated_flags=True,
        argparse_kwargs={
            "description": "AI coding assistant for your shell.",
            "epilog": """Examples:
  oy "fix the failing tests"
  oy run "fix the flaky test"
  oy chat
  oy chat --agent plan
  oy chat --continue-session
  oy run --resume 20260325
  oy chat --yolo
  oy ralph "fix the flaky test"
  oy audit auth
  oy audit-logic auth
  oy renovate-local
  oy model gpt-5
  OY_MODEL=local-8080:qwen3.5 oy chat""",
            "formatter_class": argparse.RawDescriptionHelpFormatter,
        },
    )
    return 0 if result is None else result


__all__ = [
    "_SESSIONS_DIR",
    "_chat_command",
    "_create_prompt_session",
    "_current_model_text",
    "_git_diff_shortstat",
    "_handle_ask",
    "_handle_audit",
    "_handle_debug_toggle",
    "_handle_load",
    "_handle_model_switch",
    "_handle_save",
    "_print_session_intro",
    "_read_input",
    "resolve_session",
    "_set_terminal_title",
    "workspace_root",
    "audit",
    "audit_logic",
    "chat",
    "main",
    "renovate_local",
    "model",
    "load_system_prompt",
    "ralph",
    "resolve_model_choice",
    "run",
]
