from __future__ import annotations
import asyncio
import json
import os
import readline
import re
import shlex
import sys
from pathlib import Path
from typing import Any, cast
from urllib.parse import urlparse
import defopt
import httpx
import tiktoken
from shim import (
    command_env,
    load_json,
    run_cmd,
    save_json,
    which,
    CompletionClient,
    bedrock_base_url,
    default_region,
    detect_available_shims,
    ensure_api_env as ensure_shim_api_env,
    get_client as build_shim_client,
    join_model_spec,
    list_model_ids as list_shim_model_ids,
    list_models_for_shim,
    make_bedrock_token,
    require_api_env as require_shim_api_env,
    resolve_shim as resolve_model_shim,
    split_model_spec,
    validate_shim,
)
from markdownify import markdownify as html_to_markdown
from openai import (
    AuthenticationError,
    BadRequestError,
    PermissionDeniedError,
    RateLimitError,
)
from rich.console import Console
from rich.markdown import Markdown
from rich.prompt import Prompt
from rich.status import Status

__version__ = "0.2.1"


def _env(name: str, default, type_fn=None):
    """Read OY_<NAME> from environment, falling back to *default*.

    Type coercion is automatic: if *default* is an int the env value is cast
    to int, likewise for float.  Pass *type_fn* to override.
    """
    raw = os.environ.get(f"OY_{name}")
    if raw is None:
        return default
    if type_fn is not None:
        return type_fn(raw)
    return type(default)(raw)


# Per-tool payloads: hard token cap keeps each tool result ≤ one 4k-token message.
MAX_TOOL_OUTPUT_TOKENS = _env("MAX_TOOL_OUTPUT_TOKENS", 4096)
MAX_TOOL_TAIL_TOKENS = _env("MAX_TOOL_TAIL_TOKENS", 1024)   # tail slice preserved for bash head+tail display
# Context window management: hard cap at 128k tokens, individual strings at 4k.
MAX_CONTEXT_TOKENS = _env("MAX_CONTEXT_TOKENS", 131072)
MAX_MESSAGE_TOKENS = _env("MAX_MESSAGE_TOKENS", 4096)
DEFAULT_MODEL = _env("DEFAULT_MODEL", "moonshotai.kimi-k2.5")
DEFAULT_MAX_STEPS = _env("DEFAULT_MAX_STEPS", 512)
DEFAULT_MAX_TOOL_CALLS = _env("DEFAULT_MAX_TOOL_CALLS", 512)
# Default line/entry limit for list, read, glob tools (overridable via OY_DEFAULT_LINE_LIMIT).
DEFAULT_LINE_LIMIT = _env("DEFAULT_LINE_LIMIT", 500)
CONFIG_PATH = Path.home() / ".config" / "oy" / "config.json"
BASE_SYSTEM_PROMPT = """You are oy, a tiny coding cli with tools.

Work by inspecting first, then making explicit changes. Prefer simple auditable solutions.
Keep going until done or genuinely blocked; if blocked, say what you tried and next steps.

Use grugbrain-style simplicity for complexity, OWASP-minded judgment for security, and performance-aware judgment to avoid obvious waste.
"""
INTERACTIVE_SYSTEM_PROMPT = """Use ask only when significant clarification or direction is needed.
"""
NONINTERACTIVE_SYSTEM_PROMPT = """Non-interactive mode: do not pause for approval.
"""
AUDIT_SYSTEM_PROMPT = """Audit the repo for security, unnecessary complexity, and major obvious performance issues.

Fetch current OWASP ASVS and MASVS with httpx, inspect the codebase, and write/merge prioritised findings to ISSUES.md.
Each finding should include location, category (security|complexity|performance), reference, recommendation, and status: OPEN.
Avoid removing project or human context.
"""
SEARCH_BACKENDS = {
    "rg": lambda exe, pattern, path, glob: [
        exe,
        "--line-number",
        "--column",
        "--color",
        "never",
        "--hidden",
        "--glob",
        "!.git",
        *(["--glob", glob] if glob else []),
        pattern,
        path,
    ],
    "grep": lambda exe, pattern, path, glob: [
        exe,
        "-rnE",
        "--exclude-dir=.git",
        *(["--include", glob] if glob else []),
        pattern,
        path,
    ],
}
STR, INT, BOOL = {"type": "string"}, {"type": "integer"}, {"type": "boolean"}
STRINGS = {"type": "array", "items": STR}
APPLY_OPERATION = {
    "type": "object",
    "properties": {
        "op": {"type": "string", "enum": ["replace", "write", "move", "delete"]},
        "path": STR,
        "old": STR,
        "new": STR,
        "replace_all": BOOL,
        "content": STR,
        "overwrite": BOOL,
        "to": STR,
    },
    "required": ["op", "path"],
}
APPLY_OPERATIONS = {"type": "array", "items": APPLY_OPERATION}
STDOUT = Console()
STDERR = Console(stderr=True)
# Rich Console auto-detects terminal width; no manual override needed.
HTML_MARKERS = ("text/html", "application/xhtml+xml")
HTTPX_PRESET = {"type": "string", "enum": ["page", "json", "post_json"]}
HTTPX_RESPONSE_MODE = {"type": "string", "enum": ["auto", "headers", "body", "json"]}
MAP = {"type": "object"}
ANY_JSON = {}


def markdown(text="", *, stderr=False):
    console = STDERR if stderr else STDOUT
    if text:
        console.print(Markdown(str(text)))
    else:
        console.print()


def code_block(text, language="text"):
    body = str(text).rstrip("\n")
    return f"```{language}\n{body}\n```"


def format_bash_result(command, returncode, stdout, stderr):
    """Format bash command output as a pretty markdown block."""
    parts = ["```bash", f"$ {command}"]
    stdout = (stdout or "").rstrip()
    stderr = (stderr or "").rstrip()
    if stdout:
        parts.append(stdout)
    if returncode != 0:
        parts.append(f"# exit {returncode}")
    if stderr:
        parts.extend(["# stderr:", stderr])
    parts.append("```")
    return "\n".join(parts)


def inline_code(text):
    value = str(text).replace("`", "\\`")
    return f"`{value}`"


def status(text=""):
    if text:
        markdown(f"- {text}", stderr=True)


def warning(text=""):
    markdown(f"- **Warning:** {text}", stderr=True)


def error(text=""):
    message = str(text).strip()
    body = message if "\n" in message else f"- {message}"
    markdown(f"## Error\n\n{body}", stderr=True)


def prompt_text(text=""):
    markdown(f"### {text}", stderr=True)


def fail(message, code=1):
    error(message)
    return code


def abort(message, code=1):
    raise SystemExit(fail(message, code))


def clip_tokens(text, limit=MAX_TOOL_OUTPUT_TOKENS, tail_tokens=0):
    """Truncate text to a token limit, optionally preserving a tail slice.

    For bash output, use tail_tokens > 0 to show both head and tail with a marker.
    Otherwise, just truncates at limit with an omission note.
    """
    enc = get_tokenizer()
    ids = enc.encode(text)
    if len(ids) <= limit:
        return text
    omitted_tokens = len(ids) - limit
    if 0 < tail_tokens < limit:
        head_tokens = max(limit - tail_tokens, 1)
        head = enc.decode(ids[:head_tokens])
        tail = enc.decode(ids[-tail_tokens:])
        marker = f"\n... [{omitted_tokens} tokens omitted; showing first {head_tokens} and last {tail_tokens}]\n"
        return head + marker + tail
    kept = enc.decode(ids[:limit])
    return f"{kept}\n... [{omitted_tokens} tokens omitted after {limit}]"


def preview(value, limit=72):
    text = (
        value
        if isinstance(value, str)
        else json.dumps(value, ensure_ascii=True, separators=(",", ":"))
    )
    text = " ".join(text.split())
    return text if len(text) <= limit else text[: limit - 3] + "..."


def compact_markdown(text):
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    text = re.sub(r"\n{3,}", "\n\n", text)
    return text.strip()


def should_markdownify_html(content_type, text):
    """Determine if HTTP response body should be converted from HTML to markdown.

    Checks content-type header and probes the first 500 chars for HTML markers.
    """
    lowered = (content_type or "").lower()
    if any(marker in lowered for marker in HTML_MARKERS):
        return True
    probe = text.lstrip()[:500].lower()
    return (
        probe.startswith("<!doctype html")
        or probe.startswith("<html")
        or ("<body" in probe and "<p" in probe)
    )


def format_http_text_body(text, content_type):
    if not should_markdownify_html(content_type, text):
        return text
    converted = compact_markdown(
        html_to_markdown(
            text,
            heading_style="ATX",
            bullets="-",
            strip=["script", "style", "noscript", "svg", "canvas"],
        )
    )
    return converted or text


def parse_json_path(path):
    return [part for part in (path or "").split(".") if part]


def select_json_path(value, path):
    current = value
    for part in parse_json_path(path):
        if isinstance(current, list):
            if not part.isdigit():
                raise ValueError(
                    f"json_path expected list index, got {inline_code(part)}"
                )
            index = int(part)
            try:
                current = current[index]
            except IndexError as exc:
                raise ValueError(f"json_path index out of range: {index}") from exc
            continue
        if isinstance(current, dict):
            if part not in current:
                raise ValueError(f"json_path key not found: {inline_code(part)}")
            current = current[part]
            continue
        raise ValueError(f"json_path cannot descend into {type(current).__name__}")
    return current


def normalize_mapping(value, name):
    if value is None:
        return None
    if not isinstance(value, dict):
        raise ValueError(f"{name} must be an object")
    return {str(key): "" if item is None else str(item) for key, item in value.items()}


def redact_header_value(name, value):
    lowered = name.lower()
    if lowered in {"authorization", "proxy-authorization", "cookie", "set-cookie"}:
        return "<redacted>"
    if any(marker in lowered for marker in ("token", "secret", "api-key", "apikey")):
        return "<redacted>"
    return value


def render_response_headers(headers):
    return "\n".join(
        f"{name}: {redact_header_value(name, value)}" for name, value in headers.items()
    )


def httpx_error_message(exc, timeout_seconds):
    message = str(exc).strip() or exc.__class__.__name__
    lowered = message.lower()
    if isinstance(exc, httpx.TimeoutException):
        return f"request timed out after {timeout_seconds} seconds"
    if "certificate verify failed" in lowered or "tls" in lowered:
        return "TLS verification failed; check the certificate chain or use a trusted HTTPS endpoint"
    if isinstance(exc, httpx.NetworkError):
        return f"network error: {message}"
    return f"request failed: {message}"


def render_httpx_output(response, response_mode, json_path=None):
    content_type = response.headers.get("content-type", "")
    lines = [
        f"url: {response.url}",
        f"status: {response.status_code}",
        f"reason: {response.reason_phrase}",
        f"content-type: {content_type or '<unknown>'}",
    ]
    mode = response_mode
    if mode == "auto":
        ct_lowered = (content_type or "").lower()
        is_json = "application/json" in ct_lowered or "+json" in ct_lowered
        mode = "json" if json_path or is_json else "body"
    if mode == "headers":
        header_block = render_response_headers(response.headers)
        lines.append("headers:")
        lines.append(header_block or "<none>")
        return "\n".join(lines)
    if mode == "json":
        try:
            payload = response.json()
        except json.JSONDecodeError as exc:
            raise ValueError("response body is not valid JSON") from exc
        else:
            if json_path:
                payload = select_json_path(payload, json_path)
                lines.append(f"json-path: {json_path}")
            lines.append("body-format: json")
            lines.append("")
            body = (
                payload
                if isinstance(payload, str)
                else json.dumps(payload, ensure_ascii=True, indent=2)
            )
            lines.append(body)
            return "\n".join(lines)
    body = format_http_text_body(response.text, content_type)
    if body != response.text:
        lines.append("body-format: markdown")
    lines.append("")
    lines.append(body)
    return "\n".join(lines)


def show(text, lines=2):
    """Display a preview of tool output with intelligent truncation.

    Renders as Markdown for proper code block display.
    Shows first N lines (default 2) with truncation indicator if needed.
    For code blocks, detects and preserves proper fencing.
    """
    if not text:
        return

    lines_to_show = max(lines, 0)
    text_lines = text.splitlines()

    if len(text_lines) <= lines_to_show:
        # Output fits, render as markdown
        STDERR.print(Markdown(text), overflow="fold")
        return

    # Need to truncate: show first N lines
    snippet = "\n".join(text_lines[:lines_to_show])
    total_lines = len(text_lines)
    omitted_lines = total_lines - lines_to_show

    # Check if we've created an unclosed code block by truncation
    # Count fences in snippet vs full text
    snippet_fences = snippet.count("```")
    full_fences = text.count("```")
    needs_close = snippet_fences % 2 == 1 and full_fences % 2 == 0

    # Build output as markdown with truncation marker
    parts = [snippet]
    if omitted_lines > 0:
        msg = "line" if omitted_lines == 1 else "lines"
        parts.append(f"\n... [{omitted_lines} more {msg}]")
    if needs_close:
        parts.append("\n```")

    STDERR.print(Markdown("\n".join(parts)), overflow="fold")


def rel(root, path):
    try:
        return path.relative_to(root).as_posix() or "."
    except ValueError:
        return path.as_posix()


def config_path():
    return Path(os.environ.get("OY_CONFIG", str(CONFIG_PATH))).expanduser()


def load_config():
    data = load_json(config_path(), {})
    return data if isinstance(data, dict) else {}


def save_config(data):
    save_json(config_path(), data)


def pick_default_model():
    """Pick a sensible default from all available signed-in shims."""
    try:
        available = list_all_model_ids()
    except Exception:
        return DEFAULT_MODEL
    for suffix in ("glm-5", "kimi-k2.5"):
        if match := next((model for model in available if model.endswith(suffix)), None):
            return match
    if available:
        return available[0]
    return DEFAULT_MODEL


def env_or_config(choice, env_name, config_key, default=None):
    if choice:
        return choice
    if value := os.environ.get(env_name):
        return value
    return load_config().get(config_key, default)


def current_shim(choice: str | None = None) -> str | None:
    """Return the configured shim, or None to mean 'infer from model spec or env'."""
    return env_or_config(choice, "OY_SHIM", "shim")


def current_model(choice: str | None = None) -> str:
    """Return the active model spec ('shim:model' or bare model id)."""
    if value := env_or_config(choice, "OY_MODEL", "model"):
        if isinstance(value, str) and ":" not in value:
            if shim := current_shim():
                return join_model_spec(shim, value)
        return value
    return pick_default_model()


def env_flag(name: str, default: bool = False) -> bool:
    value = os.environ.get(name)
    if value is None or not value.strip():
        return default
    lowered = value.strip().lower()
    if lowered in {"1", "true", "yes", "on"}:
        return True
    if lowered in {"0", "false", "no", "off"}:
        return False
    abort(
        f"Invalid value for {inline_code(name)}: {inline_code(value)}. Use 1/0, true/false, yes/no, or on/off."
    )
    return default


def current_workspace() -> Path:
    return Path(os.environ.get("OY_ROOT", ".")).expanduser()


def current_system_file() -> Path | None:
    raw = os.environ.get("OY_SYSTEM_FILE")
    return Path(raw).expanduser() if raw else None


def current_non_interactive() -> bool:
    return env_flag("OY_NON_INTERACTIVE", False)


def resolve_active_shim(model_spec: str | None = None) -> str:
    try:
        return validate_shim(resolve_model_shim(model_spec, current_shim()))
    except RuntimeError as exc:
        abort(str(exc))
    return "openai"


def ensure_api_env(cwd=None, refresh=False):
    _ = refresh
    return ensure_shim_api_env(current_model(), current_shim(), cwd)[0]


def require_api_env(cwd=None):
    try:
        require_shim_api_env(current_model(), current_shim(), cwd)
    except RuntimeError as exc:
        abort(str(exc))


def require_tools(env, *tools):
    missing = [tool for tool in tools if not which(tool, env.get("PATH"))]
    if missing:
        abort(
            "Required tools are missing.\n\n"
            + "\n".join(
                f"- {tool}: install `{tool}` and make sure it is on PATH"
                for tool in missing
            )
        )


def require_runtime(cwd=None):
    require_api_env(cwd)
    require_tools(command_env(cwd), "bash")


def get_client(model_spec: str | None = None) -> CompletionClient:
    require_api_env(Path.cwd())
    spec = model_spec or current_model()
    shim = resolve_active_shim(spec)
    return build_shim_client(
        shim, model_spec=spec, region=default_region(), cwd=Path.cwd()
    )


def resolve_path(root, raw):
    """Resolve a path relative to root, preventing escape via .. traversal.

    Raises ValueError if the resolved path would escape root.
    This is a security measure to constrain file access to the workspace.
    """
    path = (root / raw).resolve()
    if path == root or root in path.parents:
        return path
    raise ValueError(f"Path traversal denied: '{raw}' escapes workspace")


def apply_exact_replace(text, old, new, replace_all=False):
    if not old:
        raise ValueError("replace operation old must not be empty")
    count = text.count(old)
    if count == 0:
        raise ValueError("replace target not found")
    if count > 1 and not replace_all:
        raise ValueError(
            "replace target matched multiple locations; set replace_all=true"
        )
    updated = text.replace(old, new) if replace_all else text.replace(old, new, 1)
    return updated, count


def note_tool(state, name, *, _defaults=None, _suffix="", **details):
    if state["tool_calls"] >= state["max_tool_calls"]:
        raise ValueError(
            f"reached max tool calls ({state['max_tool_calls']}) without a final response"
        )
    state["tool_calls"] += 1
    defaults = _defaults or {}
    parts = [
        inline_code(key.replace("_", "-"))
        if value is True
        else f"{key.replace('_', '-')}: {inline_code(preview(value, 50))}"
        for key, value in details.items()
        if value not in (None, "", False) and value != defaults.get(key)
    ]
    detail_text = ", ".join(parts)
    message = f"tool {inline_code(name)}" + (f": {detail_text}" if detail_text else "")
    if _suffix:
        message += f"  {_suffix}"
    # Use bullet for mutating tools (apply, bash), plain for idempotent reads
    if name in {"apply", "bash"}:
        markdown(f"● {message}", stderr=True)
    else:
        markdown(message, stderr=True)


def _oneline(text, limit=60):
    """Collapse *text* to a single line, truncated to *limit* chars."""
    flat = " ".join((text or "").split())
    return flat if len(flat) <= limit else flat[: limit - 1] + "…"


def render_apply_op(op):
    kind = op.get("op", "?")
    path = op.get("path", "?")
    match kind:
        case "replace":
            flag = " *(all)*" if op.get("replace_all") else ""
            return [
                f"  replace `{path}`{flag}",
                f"  − `{_oneline(op.get('old', ''))}`",
                f"  + `{_oneline(op.get('new', ''))}`",
            ]
        case "write":
            overwrite = " *(overwrite)*" if op.get("overwrite") else " *(new)*"
            return [
                f"  write `{path}`{overwrite}",
                f"  + `{_oneline(op.get('content', ''))}`",
            ]
        case "move":
            return [f"  ⚠ move `{path}` → `{op.get('to', '?')}`"]
        case "delete":
            return [f"  ⚠ delete `{path}`"]
        case _:
            return [f"  {kind} `{path}`"]


def note_apply_ops(operations):
    """Print a per-operation preview for an apply tool call."""
    for op in operations:
        for line in render_apply_op(op):
            markdown(line, stderr=True)


class ToolRegistry:
    """Registry of tool definitions for the agent loop.

    Usage::

        @TOOL_REGISTRY.tool(
            "Description sent to the model.",
            params={"path": STR, "limit": INT},
            required=[],
        )
        def tool_list(state, path=".", limit=DEFAULT_LINE_LIMIT):
            ...

    Each decorated function is stored under its bare name (stripped of the
    leading ``tool_`` prefix if present) as ``(fn, description, props, required)``.
    """

    def __init__(self):
        self._specs: dict[str, tuple] = {}

    def tool(self, description: str, *, params: dict, required: list[str]):
        """Decorator that registers a tool function."""
        def decorator(fn):
            name = fn.__name__
            if name.startswith("tool_"):
                name = name[5:]
            self._specs[name] = (fn, description, params, required)
            return fn
        return decorator

    def __contains__(self, name):
        return name in self._specs

    def __iter__(self):
        return iter(self._specs)

    def items(self):
        return self._specs.items()

    def get(self, name):
        return self._specs.get(name)

    def without(self, *names):
        """Return a filtered copy of the registry excluding the given tool names."""
        copy = ToolRegistry()
        copy._specs = {k: v for k, v in self._specs.items() if k not in names}
        return copy


TOOL_REGISTRY = ToolRegistry()


@TOOL_REGISTRY.tool(
    "List a directory. Use this first on unfamiliar trees. Returns sorted entries, one per line, with / for directories.",
    params={"path": STR, "limit": INT},
    required=[],
)
def tool_list(state, path=".", limit=DEFAULT_LINE_LIMIT):
    note_tool(state, "list", _defaults={"path": ".", "limit": DEFAULT_LINE_LIMIT}, path=path, limit=limit)
    target = resolve_path(state["root"], path)
    if not target.is_dir():
        raise ValueError("path is not a directory")
    text = (
        "\n".join(
            rel(state["root"], item) + ("/" if item.is_dir() else "")
            for item in sorted(target.iterdir(), key=lambda item: item.as_posix())[
                : max(limit, 1)
            ]
        )
        or "<empty directory>"
    )
    show(text, 1)
    return clip_tokens(text)


@TOOL_REGISTRY.tool(
    "Read a file or directory. Use before editing. Files return line-numbered text; directories fall back to list. Use offset/limit for large files.",
    params={"path": STR, "offset": INT, "limit": INT},
    required=["path"],
)
def tool_read(state, path, offset=1, limit=DEFAULT_LINE_LIMIT):
    target = resolve_path(state["root"], path)
    if target.is_dir():
        note_tool(
            state, "read",
            _defaults={"path": ".", "offset": 1, "limit": DEFAULT_LINE_LIMIT},
            path=path, offset=offset, limit=limit,
        )
        return tool_list(state, path, limit)
    lines = target.read_text(encoding="utf-8", errors="replace").splitlines()
    total = len(lines)
    start = max(offset, 1) - 1
    end = min(start + max(limit, 1), total)
    suffix = f"*(lines {start + 1}\u2013{end} of {total})*" if total else ""
    note_tool(
        state, "read",
        _defaults={"offset": 1, "limit": DEFAULT_LINE_LIMIT},
        _suffix=suffix,
        path=path, offset=offset, limit=limit,
    )
    return clip_tokens(
        "\n".join(
            f"{i + 1}: {line}"
            for i, line in enumerate(lines[start : start + max(limit, 1)], start=start)
        )
        or "<empty file>"
    )


@TOOL_REGISTRY.tool(
    "Edit files inside the workspace. Operations: replace, write, move, delete. Read first and keep edits precise.",
    params={"operations": APPLY_OPERATIONS},
    required=["operations"],
)
def tool_apply(state, operations):
    if isinstance(operations, dict):
        operations = [operations]
    if not isinstance(operations, list) or not operations:
        raise ValueError(
            "operations must be a non-empty array or a single operation object"
        )
    note_tool(state, "apply", operations=len(operations))
    note_apply_ops(operations)
    root = state["root"]
    summaries = []
    for index, operation in enumerate(operations, 1):
        if not isinstance(operation, dict):
            raise ValueError(f"operation {index} must be an object")
        kind = operation.get("op")
        path = operation.get("path")
        if not isinstance(kind, str) or not kind:
            raise ValueError(f"operation {index} is missing a valid op")
        if not isinstance(path, str) or not path:
            raise ValueError(f"operation {index} is missing a valid path")
        target = resolve_path(root, path)
        match kind:
            case "replace":
                old = operation.get("old")
                new = operation.get("new")
                replace_all = operation.get("replace_all", False)
                if not isinstance(old, str) or not isinstance(new, str):
                    raise ValueError(f"replace operation {index} requires string old and new")
                if not isinstance(replace_all, bool):
                    raise ValueError(f"replace operation {index} replace_all must be boolean")
                if not target.exists():
                    raise ValueError(f"file does not exist: {rel(root, target)}")
                if target.is_dir():
                    raise ValueError(f"cannot replace text in directory: {rel(root, target)}")
                updated, count = apply_exact_replace(
                    target.read_text(encoding="utf-8", errors="replace"), old, new, replace_all
                )
                target.write_text(updated, encoding="utf-8")
                summaries.append(f"replaced {rel(root, target)} ({count} match{'es' if count != 1 else ''})")
            case "write":
                content = operation.get("content")
                overwrite = operation.get("overwrite", False)
                if not isinstance(content, str):
                    raise ValueError(f"write operation {index} requires string content")
                if not isinstance(overwrite, bool):
                    raise ValueError(f"write operation {index} overwrite must be boolean")
                if target.exists() and target.is_dir():
                    raise ValueError(f"cannot write directory: {rel(root, target)}")
                if target.exists() and not overwrite:
                    raise ValueError(f"file already exists: {rel(root, target)}; set overwrite=true")
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(content, encoding="utf-8")
                summaries.append(f"wrote {rel(root, target)}")
            case "move":
                destination_raw = operation.get("to")
                if not isinstance(destination_raw, str) or not destination_raw:
                    raise ValueError(f"move operation {index} requires a valid to path")
                if not target.exists():
                    raise ValueError(f"file does not exist: {rel(root, target)}")
                if target.is_dir():
                    raise ValueError(f"cannot move directory: {rel(root, target)}")
                destination = resolve_path(root, destination_raw)
                if destination == target:
                    raise ValueError(f"move destination matches source: {rel(root, target)}")
                if destination.exists():
                    raise ValueError(f"destination already exists: {rel(root, destination)}")
                destination.parent.mkdir(parents=True, exist_ok=True)
                target.rename(destination)
                summaries.append(f"⚠ moved {rel(root, target)} -> {rel(root, destination)}")
            case "delete":
                if not target.exists():
                    raise ValueError(f"file does not exist: {rel(root, target)}")
                if target.is_dir():
                    raise ValueError(f"cannot delete directory: {rel(root, target)}")
                target.unlink()
                summaries.append(f"⚠ deleted {rel(root, target)}")
            case _:
                raise ValueError(
                    f"operation {index} has unsupported op {inline_code(kind)}; use replace, write, move, or delete"
                )
    out = "\n".join(summaries)
    show(out, len(summaries))
    return out


@TOOL_REGISTRY.tool(
    "Run shell commands for tests, builds, git, and scripts. Do not use for routine file inspection. Returns stdout and stderr together.",
    params={"command": STR, "timeout_seconds": INT},
    required=["command"],
)
def tool_bash(state, command, timeout_seconds=120):
    note_tool(state, "bash", _defaults={"timeout": 120}, command=command, timeout=timeout_seconds)
    env = command_env(state["root"])
    result = run_cmd(
        [which("bash", env.get("PATH")) or "bash", "-c", command],
        cwd=state["root"],
        env=env,
        timeout=timeout_seconds,
    )
    out = format_bash_result(command, result.returncode, result.stdout, result.stderr)
    out = clip_tokens(out, tail_tokens=MAX_TOOL_TAIL_TOKENS)
    show(out, 8)
    return out


@TOOL_REGISTRY.tool(
    "Search file contents by text or regex. Use file_glob to narrow by filename pattern. Returns matching lines with file and line numbers.",
    params={"pattern": STR, "path": STR, "file_glob": STR},
    required=["pattern"],
)
def tool_grep(state, pattern, path=".", file_glob=None):
    env, target = command_env(state["root"]), resolve_path(state["root"], path)
    if not target.exists():
        raise ValueError(f"search path does not exist: {rel(state['root'], target)}")
    if target.is_file() and file_glob:
        raise ValueError("file_glob only works when path is a directory")
    search_path = str(target)
    for name, build in SEARCH_BACKENDS.items():
        if not (exe := which(name, env.get("PATH"))):
            continue
        result = run_cmd(
            build(exe, pattern, search_path, file_glob), cwd=state["root"], env=env
        )
        if result.returncode not in (0, 1):
            detail = (result.stderr or result.stdout or f"{name} failed").strip()
            raise ValueError(
                f"{name} search failed for {rel(state['root'], target)}: {detail}"
            )
        out = result.stdout.strip() or "<no matches>"
        matches = len(out.splitlines()) if out != "<no matches>" else 0
        suffix = f"*({matches} match{'es' if matches != 1 else ''})*" if matches else ""
        note_tool(
            state, "grep",
            _defaults={"path": "."},
            _suffix=suffix,
            pattern=pattern, path=path, glob=file_glob,
        )
        show(out, 3)
        return clip_tokens(out)
    note_tool(state, "grep", _defaults={"path": "."}, pattern=pattern, path=path, glob=file_glob)
    raise ValueError("grep requires `rg` or `grep` on PATH")


@TOOL_REGISTRY.tool(
    "Find files by name pattern like '*.py' or 'src/**/*.js'. Use when you know the path shape. Supports *, ?, and **.",
    params={"pattern": STR, "path": STR},
    required=["pattern"],
)
def tool_glob(state, pattern, path="."):
    note_tool(state, "glob", _defaults={"path": "."}, pattern=pattern, path=path)
    base = resolve_path(state["root"], path)
    out = (
        "\n".join(
            rel(state["root"], match) + ("/" if match.is_dir() else "")
            for match in sorted(base.glob(pattern), key=lambda item: item.as_posix())[
                :DEFAULT_LINE_LIMIT
            ]
        )
        or "<no matches>"
    )
    show(out, 1)
    return clip_tokens(out)


@TOOL_REGISTRY.tool(
    "Fetch web pages or APIs over HTTP(S). Presets: page, json, post_json. Use json_path to extract nested fields. Sensitive headers are redacted.",
    params={
        "url": STR,
        "preset": HTTPX_PRESET,
        "method": STR,
        "headers": MAP,
        "params": MAP,
        "body": STR,
        "json_body": ANY_JSON,
        "timeout_seconds": INT,
        "response_mode": HTTPX_RESPONSE_MODE,
        "json_path": STR,
        "max_tokens": INT,
    },
    required=["url"],
)
def tool_httpx(
    state,
    url,
    preset=None,
    method=None,
    headers=None,
    params=None,
    body=None,
    json_body=None,
    timeout_seconds=20,
    response_mode="auto",
    json_path=None,
    max_tokens=MAX_TOOL_OUTPUT_TOKENS,
):
    if preset is not None and preset not in HTTPX_PRESET["enum"]:
        raise ValueError("preset must be one of page, json, or post_json")
    if not isinstance(method, str) and method is not None:
        raise ValueError("method must be a string")
    method = (
        (method or ("POST" if body is not None or json_body is not None else "GET"))
        .strip()
        .upper()
    )
    if preset == "post_json" and method == "GET":
        method = "POST"
    if response_mode == "auto" and preset in {"json", "post_json"}:
        response_mode = "json"
    elif response_mode == "body" and json_path:
        response_mode = "json"
    note_tool(
        state,
        "httpx",
        _defaults={
            "method": "GET",
            "response_mode": "auto",
            "timeout": 20,
            "max_tokens": MAX_TOOL_OUTPUT_TOKENS,
        },
        preset=preset,
        method=method,
        url=url,
        response_mode=response_mode,
        json_path=json_path,
        timeout=timeout_seconds,
        max_tokens=max_tokens,
    )
    parsed = urlparse(url if "://" in url else f"https://{url}")
    if parsed.scheme not in {"http", "https"}:
        raise ValueError("httpx only supports http and https")
    if body is not None and json_body is not None:
        raise ValueError("provide either body or json_body, not both")
    if not method:
        raise ValueError("method must be a non-empty string")
    if not isinstance(timeout_seconds, int) or timeout_seconds <= 0:
        raise ValueError("timeout_seconds must be a positive integer")
    if not isinstance(max_tokens, int) or max_tokens <= 0:
        raise ValueError("max_tokens must be a positive integer")
    if response_mode not in HTTPX_RESPONSE_MODE["enum"]:
        raise ValueError("response_mode must be one of auto, headers, body, or json")
    if json_path is not None and not isinstance(json_path, str):
        raise ValueError("json_path must be a string")
    if response_mode == "headers" and json_path:
        raise ValueError("json_path requires body or json output")
    if body is not None and not isinstance(body, str):
        raise ValueError("body must be a string")
    if json_body is not None and not isinstance(
        json_body, (dict, list, str, int, float, bool)
    ):
        raise ValueError("json_body must be valid JSON-like data")
    request_headers = normalize_mapping(headers, "headers")
    request_params = normalize_mapping(params, "params")
    status("Fetching HTTP content.")
    try:
        with httpx.Client(
            follow_redirects=True, timeout=float(timeout_seconds)
        ) as http:
            response = http.request(
                method,
                parsed.geturl(),
                headers=request_headers,
                params=request_params,
                content=body,
                json=json_body,
            )
    except httpx.HTTPError as exc:
        raise ValueError(httpx_error_message(exc, timeout_seconds)) from exc
    out = render_httpx_output(response, response_mode, json_path=json_path)
    show(out, 1)
    return clip_tokens(out, max_tokens)


@TOOL_REGISTRY.tool(
    "Ask the user a question in interactive runs. Use for significant ambiguity or decisions. Provide choices when useful.",
    params={"question": STR, "choices": STRINGS},
    required=["question"],
)
def tool_ask(state, question, choices=None):
    note_tool(state, "ask", question=question, choices=choices)
    if not sys.stdin.isatty():
        raise ValueError("Cannot ask question: stdin is not a TTY")
    prompt_text(question)
    if not choices:
        return Prompt.ask("Answer", console=STDERR).strip()
    markdown(
        "## Options\n\n"
        + "\n".join(
            f"{i}. {inline_code(choice)}" for i, choice in enumerate(choices, 1)
        ),
        stderr=True,
    )
    while True:
        response = Prompt.ask("Selection", console=STDERR).strip()
        if response.isdigit() and 0 < int(response) <= len(choices):
            return choices[int(response) - 1]
        if response in choices:
            return response
        warning(f"Enter a number from 1 to {len(choices)} or an exact choice.")


def run_is_interactive(non_interactive: bool = False) -> bool:
    return sys.stdin.isatty() and not non_interactive


def active_system_prompt(interactive):
    return BASE_SYSTEM_PROMPT + (
        INTERACTIVE_SYSTEM_PROMPT if interactive else NONINTERACTIVE_SYSTEM_PROMPT
    )


def active_tool_specs(interactive):
    return TOOL_REGISTRY if interactive else TOOL_REGISTRY.without("ask")


def chat_tools(tool_specs):
    return [
        {
            "type": "function",
            "function": {
                "name": name,
                "description": desc,
                "parameters": {
                    "type": "object",
                    "properties": props,
                    "required": required,
                },
            },
        }
        for name, (_, desc, props, required) in tool_specs.items()
    ]


def parse_tool_arguments(args_str: str) -> dict[str, Any]:
    """Parse tool arguments from LLM output.

    Handles malformed output where some LLMs emit duplicated JSON (same object twice).
    Hunts for valid JSON starting near the midpoint if initial parse fails.
    """

    def decode(candidate: str) -> dict[str, Any]:
        parsed = json.loads(candidate)
        parsed = json.loads(parsed) if isinstance(parsed, str) else parsed
        if not isinstance(parsed, dict):
            raise ValueError("Tool arguments must decode to a JSON object")
        return parsed

    try:
        return decode(args_str)
    except (json.JSONDecodeError, ValueError) as exc:
        # Workaround for LLMs that duplicate the JSON in tool call arguments
        mid = len(args_str) // 2

        # Hunt for '{' in a window around the midpoint
        for i in range(max(0, mid - 15), min(len(args_str), mid + 15)):
            if args_str[i] == "{":
                try:
                    return decode(args_str[i:])
                except (json.JSONDecodeError, ValueError):
                    pass
        raise exc


def run_tool(state, tool_name, tool_args):
    """Execute a tool call with the given arguments.

    Returns a string result. Errors are caught and returned as error messages
    rather than raised, to keep the agent loop running.
    """
    name = tool_name[5:] if tool_name.startswith("tool_") else tool_name
    registry = state.get("tool_specs", TOOL_REGISTRY)
    if name not in registry:
        return f"Error: Tool '{name}' is unavailable in this run"
    try:
        return str(registry.get(name)[0](state, **tool_args))
    except Exception as exc:
        return f"Error in {name}: {type(exc).__name__}: {exc}"


# ---------------------------------------------------------------------------
# Token counting and context management
# ---------------------------------------------------------------------------

_tokenizer: tiktoken.Encoding | None = None


def get_tokenizer() -> tiktoken.Encoding:
    """Return the shared cl100k_base tokenizer, initialising it once."""
    global _tokenizer
    if _tokenizer is None:
        _tokenizer = tiktoken.get_encoding("cl100k_base")
    return _tokenizer


def count_tokens(text: str) -> int:
    """Count tokens in a string using cl100k_base."""
    return len(get_tokenizer().encode(text))


def truncate_str_to_tokens(text: str, max_tokens: int = MAX_MESSAGE_TOKENS) -> str:
    """Truncate *text* to at most *max_tokens* tokens.

    If truncation is needed, appends a note reporting how many lines and
    characters were removed so the model knows the content was cut.
    """
    enc = get_tokenizer()
    ids = enc.encode(text)
    if len(ids) <= max_tokens:
        return text
    kept = enc.decode(ids[:max_tokens])
    omitted_chars = len(text) - len(kept)
    # Count lines in the omitted portion
    omitted_lines = text[len(kept):].count("\n")
    line_word = "line" if omitted_lines == 1 else "lines"
    kept = kept.rstrip()
    return (
        f"{kept}\n"
        f"... [truncated: {omitted_lines} {line_word}, "
        f"{omitted_chars} chars omitted to fit {max_tokens}-token limit]"
    )


def _msg_token_count(msg: dict) -> int:
    """Approximate token count for a single message dict."""
    # 4 tokens overhead per message (role + framing), per OpenAI cookbook.
    overhead = 4
    content = msg.get("content") or ""
    if isinstance(content, str):
        return overhead + count_tokens(content)
    if isinstance(content, list):
        total = overhead
        for item in content:
            if isinstance(item, dict) and item.get("type") == "text":
                total += count_tokens(item.get("text") or "")
            else:
                total += count_tokens(json.dumps(item, ensure_ascii=True))
        return total
    return overhead + count_tokens(json.dumps(content, ensure_ascii=True))


def _truncate_msg_content(msg: dict) -> dict:
    """Return a copy of *msg* with string content fields truncated to MAX_MESSAGE_TOKENS."""
    content = msg.get("content")
    if isinstance(content, str) and content:
        truncated = truncate_str_to_tokens(content)
        if truncated is not content:
            msg = {**msg, "content": truncated}
    elif isinstance(content, list):
        new_items = []
        changed = False
        for item in content:
            if isinstance(item, dict) and item.get("type") == "text":
                original = item.get("text") or ""
                clipped = truncate_str_to_tokens(original)
                if clipped is not original:
                    item = {**item, "text": clipped}
                    changed = True
            new_items.append(item)
        if changed:
            msg = {**msg, "content": new_items}
    return msg


def pack_messages_to_context(
    messages: list[dict],
    max_context_tokens: int = MAX_CONTEXT_TOKENS,
) -> list[dict]:
    """Enforce the hard context-token ceiling, packing history newest-to-oldest.

    Algorithm:
    1. Separate the leading system message(s) from the rest.
    2. Count system tokens first — they always stay.
    3. Walk the remaining messages newest-to-oldest, accumulating tokens until
       the budget is exhausted.
    4. If any messages were dropped, insert a single placeholder note directly
       after the last system message so the model knows history was trimmed.
    """
    system_msgs: list[dict] = []
    other_msgs: list[dict] = []
    for msg in messages:
        if msg.get("role") == "system":
            system_msgs.append(msg)
        else:
            other_msgs.append(msg)

    system_tokens = sum(_msg_token_count(m) for m in system_msgs)
    budget = max_context_tokens - system_tokens

    if budget <= 0:
        # System prompt alone exceeds the budget — just return it.
        return system_msgs

    # Greedily keep messages from newest to oldest.
    kept: list[dict] = []
    used = 0
    for msg in reversed(other_msgs):
        cost = _msg_token_count(msg)
        if used + cost > budget:
            break
        kept.append(msg)
        used += cost

    kept.reverse()  # restore chronological order
    dropped = len(other_msgs) - len(kept)

    if dropped:
        placeholder = {
            "role": "user",
            "content": (
                f"... [{dropped} earlier "
                f"{'message' if dropped == 1 else 'messages'} omitted "
                f"to fit {max_context_tokens // 1024}k-token context limit]"
            ),
        }
        return system_msgs + [placeholder] + kept

    return system_msgs + kept


def session_tokens(messages: list) -> int:
    """Count total tokens across all messages in a session."""
    return sum(_msg_token_count(m) for m in messages)


def format_tokens(n: int) -> str:
    """Format a token count as a compact human-readable string."""
    if n < 1000:
        return f"{n} tokens"
    return f"{n / 1000:.1f}k tokens"


def list_all_model_ids() -> list[str]:
    """Return prefixed model specs merged from every signed-in shim."""
    shims = detect_available_shims()
    if not shims:
        abort(
            "No shims are configured. Set OPENAI_API_KEY, sign in with Codex CLI, "
            "authenticate with Gemini CLI, sign in with Claude Code, or configure AWS CLI."
        )
    all_models: list[str] = []
    for shim in shims:
        status(f"Loading models from {inline_code(shim)}.")
        all_models.extend(
            list_models_for_shim(shim, region=default_region(), cwd=Path.cwd())
        )
    return all_models


def list_model_ids() -> list[str]:
    """Return model IDs for the currently configured shim only (no prefix)."""
    require_api_env()
    spec = current_model()
    shim = resolve_active_shim(spec)
    status(f"Loading models from {inline_code(shim)}.")
    return list_shim_model_ids(shim, region=default_region(), cwd=Path.cwd())


async def run_turn(client, messages, state, model_spec, tool_defs, max_steps):
    # Strip shim prefix before sending to the API
    _, model = split_model_spec(model_spec)
    for _ in range(max_steps):
        # Truncate any over-long individual message strings, then enforce the
        # hard 128k-token context cap by dropping oldest messages first.
        prepared = [_truncate_msg_content(m) for m in messages]
        prepared = pack_messages_to_context(prepared)
        size = session_tokens(prepared)
        size_str = format_tokens(size)
        spinner = Status(
            f"Waiting for {model_spec} · {size_str}",
            console=STDERR,
            spinner="dots",
        )
        spinner.start()
        try:
            message = await cast(CompletionClient, client).chat_completion(
                model=model,
                messages=prepared,
                tools=tool_defs,
                tool_choice="auto",
            )
        finally:
            spinner.stop()
        calls = []
        for call in message.get("tool_calls") or []:
            if call.get("type") != "function":
                continue
            function = call["function"]
            tool_args = parse_tool_arguments(function["arguments"])
            function["arguments"] = json.dumps(tool_args)
            calls.append((call["id"], function["name"], tool_args))
        output = message.get("content") or ""
        output = (
            output if isinstance(output, str) else json.dumps(output, ensure_ascii=True)
        )
        if calls:
            messages.append(message)
            results = [
                (call_id, run_tool(state, name, args)) for call_id, name, args in calls
            ]
            messages.extend(
                {"role": "tool", "tool_call_id": call_id, "content": result}
                for call_id, result in results
            )
            continue
        markdown(output)
        return 0, output
    return fail(f"reached max steps ({max_steps}) without a final response"), ""


async def run_agent(
    prompt,
    model,
    root,
    system_prompt,
    max_steps,
    max_tool_calls,
    interactive,
    messages: list[dict[str, Any]] | None = None,
):
    tool_specs = active_tool_specs(interactive)
    state = {
        "root": root,
        "tool_calls": 0,
        "max_tool_calls": max_tool_calls,
        "tool_specs": tool_specs,
    }
    tool_defs = chat_tools(tool_specs)
    if messages is None:
        messages = [
            {"role": "system", "content": system_prompt},
        ]

    # If the last message is a system message and it's different from the current one, update it
    if messages and messages[0]["role"] == "system":
        messages[0]["content"] = system_prompt

    messages.append({"role": "user", "content": prompt})

    async def run_with_client(client):
        return await run_turn(client, messages, state, model, tool_defs, max_steps)

    try:
        return await run_with_client(get_client(model))
    except (AuthenticationError, PermissionDeniedError) as exc:
        if ensure_api_env(root, refresh=True):
            warning("Credentials expired. Refreshing.")
            try:
                return await run_with_client(get_client(model))
            except (AuthenticationError, PermissionDeniedError) as retry_exc:
                kind = (
                    "authentication"
                    if isinstance(retry_exc, AuthenticationError)
                    else "permission"
                )
                return fail(f"API {kind} error: {retry_exc}"), ""
            except Exception as retry_exc:
                return fail(str(retry_exc)), ""
        kind = (
            "authentication" if isinstance(exc, AuthenticationError) else "permission"
        )
        return fail(f"API {kind} error: {exc}"), ""
    except RateLimitError as exc:
        return fail(f"API rate limit: {exc}"), ""
    except BadRequestError as exc:
        return fail(f"API bad request: {exc}"), ""
    except Exception as exc:
        return fail(str(exc)), ""


def read_system_prompt(system_file, interactive):
    system_prompt = active_system_prompt(interactive)
    if system_file is None:
        return system_prompt
    if not system_file.exists():
        abort(f"System file does not exist: {inline_code(system_file)}")
    if system_file.is_dir():
        abort(f"System file is a directory: {inline_code(system_file)}")
    extra = ""
    try:
        extra = system_file.read_text(encoding="utf-8")
    except OSError as exc:
        abort(f"Could not read system file {inline_code(system_file)}: {exc}")
    return system_prompt + "\n\n" + extra


def audit(prompt: str = ""):
    """Run a security and complexity audit of the repository.

    :param prompt: Additional audit focus instructions.
    """

    workspace = current_workspace().resolve()
    if not workspace.is_dir():
        abort(f"Workspace root is not a directory: {inline_code(workspace)}")
    require_runtime(workspace)
    chosen_model = current_model(None)

    audit_prompt = "Conduct a security and complexity audit."
    if prompt:
        audit_prompt += f" Additional focus: {prompt}"

    intro = [
        "## Audit",
        "",
        f"- workspace: {inline_code(workspace)}",
        f"- model: {inline_code(chosen_model)}",
        f"- mode: {inline_code('non-interactive')}",
    ]
    if prompt:
        intro.append(f"- focus: {inline_code(preview(prompt, 100))}")
    markdown("\n".join(intro), stderr=True)

    code, _ = asyncio.run(
        run_agent(
            audit_prompt,
            chosen_model,
            workspace,
            AUDIT_SYSTEM_PROMPT,
            DEFAULT_MAX_STEPS,
            DEFAULT_MAX_TOOL_CALLS,
            interactive=False,
        )
    )

    return code


def _setup_readline():
    """Configure readline with persistent history for shell-like UX."""
    history_path = CONFIG_PATH.parent / "history"
    history_path.parent.mkdir(parents=True, exist_ok=True)
    try:
        readline.read_history_file(history_path)
    except FileNotFoundError:
        pass
    readline.set_history_length(1000)
    import atexit
    atexit.register(readline.write_history_file, str(history_path))


def chat():
    """Start an interactive anonymous session."""

    _setup_readline()
    workspace = current_workspace().resolve()
    if not workspace.is_dir():
        abort(f"Workspace root is not a directory: {inline_code(workspace)}")
    require_runtime(workspace)
    chosen_model = current_model(None)
    interactive = True
    system_file = current_system_file()
    system_prompt = read_system_prompt(system_file, interactive)

    intro = [
        "## Chat",
        "",
        f"- workspace: {inline_code(workspace)}",
        f"- model: {inline_code(chosen_model)}",
        f"- mode: {inline_code('interactive')}",
    ]
    if system_file is not None:
        intro.append(f"- system file: {inline_code(system_file.resolve())}")
    markdown("\n".join(intro), stderr=True)

    messages: list[dict[str, Any]] = [{"role": "system", "content": system_prompt}]

    while True:
        try:
            STDERR.print()
            STDERR.rule(style="dim")
            prompt = input("oy ❯ ")
            if not prompt.strip():
                continue
            if prompt.strip().lower() in ("exit", "quit"):
                break

            code, _ = asyncio.run(
                run_agent(
                    prompt,
                    chosen_model,
                    workspace,
                    system_prompt,
                    DEFAULT_MAX_STEPS,
                    DEFAULT_MAX_TOOL_CALLS,
                    interactive,
                    messages=messages,
                )
            )
            size_str = format_tokens(session_tokens(messages))
            STDERR.print(f"[dim]· {size_str}[/dim]")
        except (KeyboardInterrupt, EOFError):
            markdown("\n## Session Ended", stderr=True)
            break
    return 0


def run(
    *prompt: str,
):
    """Run the coding assistant in a workspace.

    :param prompt: Prompt text to send. Starts an interactive chat if omitted.
    """

    task = (
        " ".join(prompt)
        if prompt
        else (sys.stdin.read().strip() if not sys.stdin.isatty() else "")
    )
    if not task:
        return chat()

    workspace = current_workspace().resolve()
    if not workspace.is_dir():
        abort(f"Workspace root is not a directory: {inline_code(workspace)}")
    require_runtime(workspace)
    chosen_model = current_model(None)
    system_file = current_system_file()
    interactive = run_is_interactive(current_non_interactive())
    system_prompt = read_system_prompt(system_file, interactive)
    intro = [
        "## Run",
        "",
        f"- workspace: {inline_code(workspace)}",
        f"- model: {inline_code(chosen_model)}",
        f"- mode: {inline_code('interactive' if interactive else 'non-interactive')}",
        f"- prompt: {inline_code(preview(task, 100))}",
    ]
    if system_file is not None:
        intro.append(f"- system file: {inline_code(system_file.resolve())}")
    markdown("\n".join(intro), stderr=True)
    return asyncio.run(
        run_agent(
            task,
            chosen_model,
            workspace,
            system_prompt,
            DEFAULT_MAX_STEPS,
            DEFAULT_MAX_TOOL_CALLS,
            interactive,
        )
    )[0]


def append_items(lines, *items):
    if items:
        lines.extend(["", *items])
    return lines


def render_model_list(
    items, *, title, query=None, current=None, stderr=False, limit=None
):
    shown = list(items if limit is None else items[:limit])
    lines = [title]
    if current:
        append_items(lines, f"- current model: {inline_code(current)}")
    if query:
        append_items(lines, f"- filter: {inline_code(query)}")
    if shown:
        append_items(lines, *[f"{i}. {inline_code(item)}" for i, item in enumerate(shown, 1)])
    else:
        append_items(lines, "- no matching models")
    if len(items) > len(shown):
        append_items(lines, f"- showing {len(shown)} of {len(items)} matches")
    markdown("\n".join(lines), stderr=stderr)


def filter_models(items, query):
    needle = query.strip().lower()
    return [item for item in items if needle in item.lower()]


def select_model_by_number(items, value):
    if not value.isdigit():
        return None
    index = int(value)
    if 1 <= index <= len(items):
        return items[index - 1]
    return None


def resolve_model_choice(model_id=None):
    available = list_all_model_ids()
    current = current_model(None)
    if model_id in available:
        return model_id
    if model_id and not sys.stdin.isatty():
        matches = filter_models(available, model_id)
        if matches:
            render_model_list(
                matches,
                title="## Matching Models",
                query=model_id,
                current=current,
                stderr=True,
            )
        abort(
            f"No exact model match for {inline_code(model_id)}. Re-run in a TTY to filter and choose interactively."
        )
    if not sys.stdin.isatty():
        return None
    markdown(
        "## Choose a Model\n\n- Enter an exact model ID to save it.\n- Enter text to filter the list.\n- Enter a number to pick from the currently listed models.",
        stderr=True,
    )
    if model_id is None:
        render_model_list(
            available,
            title="## Available Models",
            current=current,
            stderr=True,
        )
    shown = available
    query = (
        model_id
        or Prompt.ask("Model or filter", console=STDERR, default=current).strip()
    )
    while True:
        query = query.strip() or current
        if query in available:
            return query
        if choice := select_model_by_number(shown, query):
            return choice
        matches = filter_models(available, query)
        render_model_list(
            matches,
            title="## Matching Models",
            query=query,
            current=current,
            stderr=True,
        )
        shown = matches
        query = Prompt.ask("Model or filter", console=STDERR).strip()


def bedrock_token(*, region: str | None = None):
    """Print export statements for Bedrock-backed OpenAI credentials.

    :param region: AWS region to use when generating the token.
    """

    chosen = default_region() if region is None else region
    status(f"Generating Bedrock credentials for {inline_code(chosen)}.")
    token = make_bedrock_token(chosen, cwd=Path.cwd())
    markdown(
        "## Bedrock Credentials\n\n"
        + "Paste this into another shell if you want to reuse the current Bedrock session.\n\n"
        + code_block(
            "\n".join(
                [
                    f"export OPENAI_BASE_URL={shlex.quote(bedrock_base_url(chosen))}",
                    f"export OPENAI_API_KEY={shlex.quote(token)}",
                ]
            ),
            language="bash",
        )
    )
    return 0


def models(query: str | None = None):
    """Pick the default model interactively.

    :param query: Exact model ID to save, or a filter string when running in a TTY.
    """

    if query is None and not sys.stdin.isatty():
        render_model_list(list_all_model_ids(), title="## Available Models")
        return 0
    chosen = resolve_model_choice(query)
    if chosen is None:
        render_model_list(list_all_model_ids(), title="## Available Models")
        return 0
    shim, bare_model = split_model_spec(chosen)
    cfg = load_config()
    cfg["model"] = bare_model
    if shim:
        cfg["shim"] = shim
    else:
        cfg.pop("shim", None)
    save_config(cfg)
    markdown(
        f"## Default Model Updated\n\n"
        f"- selected: {inline_code(chosen)}"
        + (f"\n- shim: {inline_code(shim)}" if shim else "")
    )
    return 0


def model():
    """Show the current default model."""

    spec = current_model(None)
    shim = resolve_active_shim(spec)
    _, bare = split_model_spec(spec)
    markdown(
        f"## Current Model\n\n"
        f"- model: {inline_code(bare)}\n"
        f"- shim: {inline_code(shim)}"
    )
    return 0


def main(argv: list[str] | None = None):
    args = list(sys.argv[1:] if argv is None else argv)
    commands = {"run", "chat", "models", "model", "audit", "-h", "--help"}
    if not args:
        args = ["run"] if not sys.stdin.isatty() else ["--help"]
    elif args[0] in {"-v", "--version"}:
        render_markdown(f"oy {__version__}")
        return 0
    elif args[0] not in commands:
        args = ["run", *args]
    result = defopt.run([run, chat, models, model, audit], argv=args, version=False, short={})
    return 0 if result is None else result


if __name__ == "__main__":
    raise SystemExit(main())
