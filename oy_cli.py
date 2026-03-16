from __future__ import annotations
import asyncio
from dataclasses import dataclass
import json
import os
import readline
import re
import shlex
import sys
from pathlib import Path
from typing import Any, Callable, Literal, TypeAlias, cast
from urllib.parse import urlparse
import defopt
import httpx
import msgspec
import tiktoken
from shim import (
    AssistantMessage,
    ChatMessage,
    command_env,
    load_json,
    run_cmd,
    save_json,
    SystemMessage,
    ToolMessage,
    ToolResult,
    ToolSpec,
    UserMessage,
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
from markdownify import markdownify as html_to_md
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

from importlib.metadata import version as _meta_version

__version__ = _meta_version("oy-cli")


def _env(name, default, t=None):
    """Read OY_{name} from the environment, coercing to the type of *default*."""
    v = os.environ.get(f"OY_{name}")
    return default if v is None else (t or type(default))(v)


MAX_TOOL_OUTPUT_TOKENS = _env("MAX_TOOL_OUTPUT_TOKENS", 4096)
MAX_TOOL_TAIL_TOKENS = _env("MAX_TOOL_TAIL_TOKENS", 1024)
MAX_BASH_CMD_BYTES = _env("MAX_BASH_CMD_BYTES", 65536)
MAX_CONTEXT_TOKENS = _env("MAX_CONTEXT_TOKENS", 131072)
MAX_MESSAGE_TOKENS = _env("MAX_MESSAGE_TOKENS", 4096)
DEFAULT_MAX_STEPS = _env("DEFAULT_MAX_STEPS", 512)
DEFAULT_MAX_TOOL_CALLS = _env("DEFAULT_MAX_TOOL_CALLS", 512)
DEFAULT_LINE_LIMIT = _env("DEFAULT_LINE_LIMIT", 500)
CONFIG_PATH = Path.home() / ".config" / "oy" / "config.json"
BASE_SYSTEM_PROMPT = """You are oy, a tiny coding cli with tools.
Work by inspecting first, then making explicit changes. Prefer simple auditable solutions.
Keep going until done or genuinely blocked; if blocked, say what you tried and next steps.
Use grugbrain-style simplicity for complexity, OWASP-minded judgment for security, and performance-aware judgment to avoid obvious waste.
"""
INTERACTIVE_SYSTEM_PROMPT = (
    "Use ask only when significant clarification or direction is needed."
)
NONINTERACTIVE_SYSTEM_PROMPT = "Non-interactive mode: do not pause for approval."
AUDIT_SYSTEM_PROMPT = """Audit the repo for security, unnecessary complexity, and major obvious performance issues.
Fetch current OWASP ASVS and MASVS with httpx, inspect the codebase, and write/merge prioritised findings to ISSUES.md.
Each finding should include location, category (security|complexity|performance), reference, recommendation, and status: OPEN.
Avoid removing project or human context.
"""
SEARCH_BACKENDS = {
    "rg": lambda e, p, d, g: [
        e,
        "--line-number",
        "--column",
        "--color",
        "never",
        "--hidden",
        "--glob",
        "!.git",
        *(["--glob", g] if g else []),
        p,
        d,
    ],
    "grep": lambda e, p, d, g: [
        e,
        "-rnE",
        "--exclude-dir=.git",
        *(["--include", g] if g else []),
        p,
        d,
    ],
}
STDOUT, STDERR = Console(), Console(stderr=True)
_httpx_preset = {"type": "string", "enum": ["page", "json", "post_json"]}
_httpx_mode = {"type": "string", "enum": ["auto", "headers", "body", "json"]}


def _fmt(kind, value="", extra=None):
    """Format *value* as markdown according to *kind* (md, block, inline, bash, etc.)."""
    text = str(value)
    if kind == "bash":
        out, rc, err = extra
        return "\n".join(
            [
                "```bash",
                f"$ {value}",
                (out or "").rstrip(),
                *([f"# exit {rc}"] if rc else []),
                *(["# stderr:", err.rstrip()] if err else []),
                "```",
            ]
        )
    return {
        "md": text,
        "block": f"```{extra or 'text'}\n{text.rstrip()}\n```",
        "inline": f"`{text.replace('`', '\\`')}`",
        "status": f"- {text}",
        "warning": f"- **Warning:** {text}",
        "prompt": f"### {text}",
        "error": f"## Error\n\n{text if chr(10) in text else f'- {text}'}",
    }[kind]


def _print(kind="md", value="", *, err=False, extra=None):
    console = STDERR if err else STDOUT
    console.print(Markdown(_fmt(kind, value, extra))) if value else console.print()


def fail(m, c=1):
    """Print an error to stderr and return exit code *c*."""
    _print("error", str(m).strip(), err=True)
    return c


def abort(m, c=1):
    """Print an error and immediately exit."""
    raise SystemExit(fail(m, c))


def clip_tokens(text, limit=MAX_TOOL_OUTPUT_TOKENS, tail=0):
    """Truncate *text* to *limit* tokens, optionally keeping *tail* tokens from the end."""
    e = get_tokenizer()
    ids = e.encode(text)
    n = len(ids)
    if n <= limit:
        return text
    omitted = n - limit
    if 0 < tail < limit:
        h = max(limit - tail, 1)
        return f"{e.decode(ids[:h])}\n... [{omitted} tokens omitted; showing first {h} and last {tail}]\n{e.decode(ids[-tail:])}"
    return f"{e.decode(ids[:limit])}\n... [{omitted} tokens omitted after {limit}]"


def preview(v, lim=72):
    """Return a one-line preview of *v*, truncated to *lim* characters."""
    s = " ".join(
        (v if isinstance(v, str) else json.dumps(v, separators=(",", ":"))).split()
    )
    return s if len(s) <= lim else s[: lim - 3] + "..."


def _compact_md(t):
    """Collapse runs of 3+ newlines to 2 and normalise line endings."""
    return re.sub(
        r"\n{3,}", "\n\n", t.replace("\r\n", "\n").replace("\r", "\n")
    ).strip()


def _is_html(ct, text):
    """Heuristic: return True if *ct* or the start of *text* looks like HTML."""
    ct = (ct or "").lower()
    if "text/html" in ct or "application/xhtml" in ct:
        return True
    p = text.lstrip()[:500].lower()
    return (
        p.startswith("<!doctype html")
        or p.startswith("<html")
        or ("<body" in p and "<p" in p)
    )


def _http_body(text, ct):
    """Convert HTML responses to compact markdown; pass others through."""
    return (
        text
        if not _is_html(ct, text)
        else _compact_md(
            html_to_md(
                text,
                heading_style="ATX",
                bullets="-",
                strip=["script", "style", "noscript", "svg", "canvas"],
            )
        )
        or text
    )


def _json_path(v, p):
    """Walk into *v* using dot-separated *p* (supports dict keys and list indices)."""
    for part in (p or "").split("."):
        if not part:
            continue
        if isinstance(v, list):
            if not part.isdigit():
                raise ValueError(f"json_path expected index, got {part}")
            v = v[int(part)]
        elif isinstance(v, dict):
            if part not in v:
                raise ValueError(f"json_path key not found: {part}")
            v = v[part]
        else:
            raise ValueError(f"json_path cannot descend into {type(v).__name__}")
    return v


def _norm_map(v, n):
    """Coerce *v* to a ``{str: str}`` dict for HTTP headers/params, or return None."""
    if v is None:
        return None
    if not isinstance(v, dict):
        raise ValueError(f"{n} must be an object")
    return {k: "" if i is None else str(i) for k, i in v.items()}


def _redact_header(k, v):
    """Return ``'<redacted>'`` for sensitive headers, otherwise *v*."""
    kl = k.lower()
    return (
        "<redacted>"
        if kl in {"authorization", "proxy-authorization", "cookie", "set-cookie"}
        or any(m in kl for m in ("token", "secret", "api-key", "apikey"))
        else v
    )


def _render_headers(h):
    return "\n".join(f"{k}: {_redact_header(k, v)}" for k, v in h.items())


def _httpx_err(e, t):
    m = str(e).strip() or e.__class__.__name__
    ml = m.lower()
    if isinstance(e, httpx.TimeoutException):
        return f"request timed out after {t}s"
    if "certificate verify failed" in ml or "tls" in ml:
        return "TLS verification failed"
    return (
        f"network error: {m}"
        if isinstance(e, httpx.NetworkError)
        else f"request failed: {m}"
    )


def render_httpx_output(response, response_mode, json_path=None):
    """Format an httpx *response* for tool output according to *response_mode*."""
    content_type = response.headers.get("content-type", "")
    lines = [
        f"url: {response.url}",
        f"status: {response.status_code}",
        f"reason: {response.reason_phrase}",
        f"content-type: {content_type or '<unknown>'}",
    ]
    if response_mode == "auto":
        response_mode = (
            "json"
            if json_path
            or any(x in content_type.lower() for x in ("application/json", "+json"))
            else "body"
        )
    if response_mode == "headers":
        return "\n".join(
            [*lines, "headers:", _render_headers(response.headers) or "<none>"]
        )
    if response_mode == "json":
        try:
            body = response.json()
        except json.JSONDecodeError as exc:
            raise ValueError("response body is not valid JSON") from exc
        if json_path:
            body = _json_path(body, json_path)
            lines.append(f"json-path: {json_path}")
        return "\n".join(
            [
                *lines,
                "body-format: json",
                "",
                body
                if isinstance(body, str)
                else json.dumps(body, ensure_ascii=True, indent=2),
            ]
        )
    body = _http_body(response.text, content_type)
    return "\n".join(
        lines
        + (["body-format: markdown"] if body != response.text else [])
        + ["", body]
    )


def show(t, n=2):
    """Print the first *n* lines of *t* to stderr as a preview."""
    if not t:
        return
    lines = t.splitlines()
    if len(lines) <= n:
        STDERR.print(Markdown(t), overflow="fold")
        return
    s = "\n".join(lines[:n])
    omitted = len(lines) - n
    s += f"\n... [{omitted} more {'line' if omitted == 1 else 'lines'}]"
    if s.count("```") % 2 == 1 and t.count("```") % 2 == 0:
        s += "\n```"
    STDERR.print(Markdown(s), overflow="fold")


def _rel(r, p):
    try:
        return p.relative_to(r).as_posix() or "."
    except ValueError:
        return "<outside workspace>"


def _cfg_path():
    return Path(os.environ.get("OY_CONFIG", str(CONFIG_PATH))).expanduser()


def _load_cfg():
    data = load_json(_cfg_path(), {})
    return data if isinstance(data, dict) else {}


def _save_cfg(d):
    save_json(_cfg_path(), d)


def _pick_model():
    """Prompt the user to choose and save a default model.

    In non-interactive mode (no TTY or OY_NON_INTERACTIVE=1), aborts with
    instructions.  Otherwise lists available models and asks for a selection.
    """
    if not sys.stdin.isatty() or _flag("OY_NON_INTERACTIVE", False):
        abort(
            "No model configured.\n\n"
            "Pick one interactively:\n"
            "  oy model\n\n"
            "Or set directly:\n"
            "  OY_MODEL=bedrock:us.anthropic.claude-sonnet-4-20250514-v1:0 oy ...\n"
        )
    try:
        avail = list_all_model_ids()
    except Exception:
        abort(
            "No model configured and could not list available models.\n\n"
            "Set OY_MODEL or run `oy model` to pick one."
        )
    if not avail:
        abort(
            "No model configured and no models found from available shims.\n\n"
            "Set OY_MODEL or run `oy model` to pick one."
        )
    _print(
        value="## No model configured\n\n"
        "Pick a default model to save (recommended: a `glm-5` or `kimi-k2.5` variant if available).\n",
        err=True,
    )
    render_model_list(avail, title="## Available Models", err=True)
    while True:
        response = Prompt.ask("Model number or ID", console=STDERR).strip()
        if response.isdigit() and 1 <= int(response) <= len(avail):
            chosen = avail[int(response) - 1]
            break
        if response in avail:
            chosen = response
            break
        matches = [m for m in avail if response.lower() in m.lower()]
        if len(matches) == 1:
            chosen = matches[0]
            break
        if matches:
            render_model_list(
                matches, title="## Matching Models", query=response, err=True
            )
            continue
        _print(
            "warning", f"No match for {_fmt('inline', response)}. Try again.", err=True
        )
    shim_name, bare_model = split_model_spec(chosen)
    cfg = {**_load_cfg(), "model": bare_model}
    if shim_name:
        cfg["shim"] = shim_name
    else:
        cfg.pop("shim", None)
    _save_cfg(cfg)
    _print(
        value=f"## Default Model Saved\n\n- selected: {_fmt('inline', chosen)}",
        err=True,
    )
    return chosen


def _env_or_cfg(c, e, k, d=None):
    return c or os.environ.get(e) or _load_cfg().get(k, d)


def _shim(c=None):
    return _env_or_cfg(c, "OY_SHIM", "shim")


def _model(c=None):
    if v := _env_or_cfg(c, "OY_MODEL", "model"):
        return (
            join_model_spec(s, v)
            if isinstance(v, str) and ":" not in v and (s := _shim())
            else v
        )
    return _pick_model()


def _flag(n, d=False):
    v = os.environ.get(n)
    if not v or not v.strip():
        return d
    v = v.strip().lower()
    if v in {"1", "true", "yes", "on"}:
        return True
    if v in {"0", "false", "no", "off"}:
        return False
    abort(f"Invalid {n}={v}. Use 1/0, true/false, yes/no, on/off.")
    return d


def _ws():
    return Path(os.environ.get("OY_ROOT", ".")).expanduser()


def _sys_file():
    return Path(v).expanduser() if (v := os.environ.get("OY_SYSTEM_FILE")) else None


def _wrap_runtime_error(fn, *args):
    try:
        return fn(*args)
    except RuntimeError as e:
        abort(str(e))


def resolve_active_shim(spec=None):
    return _wrap_runtime_error(validate_shim, resolve_model_shim(spec, _shim()))


def ensure_api_env(cwd=None):
    """Return True if API credentials are available."""
    return ensure_shim_api_env(_model(), _shim(), cwd)[0]


def require_api_env(cwd=None):
    _wrap_runtime_error(require_shim_api_env, _model(), _shim(), cwd)


def require_tools(env, *tools):
    if m := [t for t in tools if not which(t, env.get("PATH"))]:
        abort("Missing: " + ", ".join(m))


def require_runtime(cwd=None):
    require_api_env(cwd)
    require_tools(command_env(cwd), "bash")


def get_client(spec=None):
    require_api_env(Path.cwd())
    s = spec or _model()
    return build_shim_client(
        resolve_active_shim(s), model_spec=s, region=default_region(), cwd=Path.cwd()
    )


def resolve_path(r, p):
    """Resolve *p* under workspace root *r*; raise ValueError on traversal."""
    path = (r / p).resolve()
    if path == r or r in path.parents:
        return path
    raise ValueError(f"Path traversal denied: '{p}'")


def _replace(text, old, new, replace_all=False):
    """Replace *old* with *new* in *text*.  Returns (updated_text, match_count)."""
    if not old:
        raise ValueError("old is empty")
    n = text.count(old)
    if n == 0:
        raise ValueError("not found")
    if n > 1 and not replace_all:
        raise ValueError("multiple matches; set replace_all=true")
    return text.replace(old, new) if replace_all else text.replace(old, new, 1), n


def note_tool(state: AgentState, name, *, _defaults=None, _suffix="", **details):
    state.note_tool_call()
    defaults = _defaults or {}
    parts = [
        _fmt("inline", key.replace("_", "-"))
        if value is True
        else f"{key.replace('_', '-')}: {_fmt('inline', preview(value, 50))}"
        for key, value in details.items()
        if value not in (None, "", False) and value != defaults.get(key)
    ]
    detail_text = ", ".join(parts)
    message = f"tool {_fmt('inline', name)}" + (
        f": {detail_text}" if detail_text else ""
    )
    if _suffix:
        message += f"  {_suffix}"
    # Use bullet for mutating tools (apply, bash), plain for idempotent reads
    if name in {"apply", "bash"}:
        _print(value=f"● {message}", err=True)
    else:
        _print(value=message, err=True)


def _oneline(text, limit=60):
    flat = " ".join((text or "").split())
    return flat if len(flat) <= limit else flat[: limit - 1] + "…"


def note_apply_ops(ops):
    for op in ops:
        kind, path = op.get("op", "?"), op.get("path", "?")
        lines = {
            "replace": [
                f"  replace `{path}`" + (" *(all)*" if op.get("replace_all") else ""),
                f"  − `{_oneline(op.get('old', ''))}`",
                f"  + `{_oneline(op.get('new', ''))}`",
            ],
            "write": [
                f"  write `{path}`"
                + (" *(overwrite)*" if op.get("overwrite") else " *(new)*"),
                f"  + `{_oneline(op.get('content', ''))}`",
            ],
            "move": [f"  ⚠ move `{path}` → `{op.get('to', '?')}`"],
            "delete": [f"  ⚠ delete `{path}`"],
        }.get(kind, [f"  {kind} `{path}`"])
        for line in lines:
            _print(value=line, err=True)


# Tool schemas and argument decoding are msgspec-native now.
class ApplyOperation(msgspec.Struct, omit_defaults=True):
    op: Literal["replace", "write", "move", "delete"]
    path: str
    old: str | None = None
    new: str | None = None
    replace_all: bool = False
    content: str | None = None
    overwrite: bool = False
    to: str | None = None


class ListArgs(msgspec.Struct, omit_defaults=True):
    path: str = "."
    limit: int = DEFAULT_LINE_LIMIT


class ReadArgs(msgspec.Struct, omit_defaults=True):
    path: str
    offset: int = 1
    limit: int = DEFAULT_LINE_LIMIT


class ApplyArgs(msgspec.Struct, omit_defaults=True):
    operations: ApplyOperation | list[ApplyOperation]


class BashArgs(msgspec.Struct, omit_defaults=True):
    command: str
    timeout_seconds: int = 120


class GrepArgs(msgspec.Struct, omit_defaults=True):
    pattern: str
    path: str = "."
    file_glob: str | None = None


class GlobArgs(msgspec.Struct, omit_defaults=True):
    pattern: str
    path: str = "."
    limit: int = DEFAULT_LINE_LIMIT


class HttpxArgs(msgspec.Struct, omit_defaults=True):
    url: str
    preset: Literal["page", "json", "post_json"] | None = None
    method: str | None = None
    headers: dict[str, str] | None = None
    params: dict[str, str] | None = None
    body: str | None = None
    json_body: Any = None
    timeout_seconds: int = 20
    response_mode: Literal["auto", "headers", "body", "json"] = "auto"
    json_path: str | None = None
    max_tokens: int = MAX_TOOL_OUTPUT_TOKENS


class AskArgs(msgspec.Struct, omit_defaults=True):
    question: str
    choices: list[str] | None = None


ToolCallable: TypeAlias = Callable[..., Any]


@dataclass(frozen=True, slots=True)
class ToolHandler:
    name: str
    fn: ToolCallable
    spec: ToolSpec
    args_type: Any

    def invoke(
        self, state: AgentState, args: dict[str, Any] | None = None
    ) -> ToolResult:
        try:
            parsed = msgspec.convert(args or {}, type=self.args_type)
            payload = self.fn(state, **msgspec.to_builtins(parsed))
            return ToolResult(content=payload)
        except Exception as exc:
            return ToolResult(
                ok=False,
                content={
                    "tool": self.name,
                    "error_type": type(exc).__name__,
                    "message": str(exc),
                },
            )


_TOOLS: dict[str, ToolHandler] = {}


def _tool_name(name: str) -> str:
    return name[5:] if name.startswith("tool_") else name


def _tool_schema(args_type):
    schema = msgspec.json.schema(args_type)

    def resolve(node, defs):
        if isinstance(node, list):
            return [resolve(item, defs) for item in node]
        if not isinstance(node, dict):
            return node
        if "$ref" in node and isinstance(node["$ref"], str):
            name = node["$ref"].removeprefix("#/$defs/")
            resolved = resolve(defs.get(name, {}), defs)
            extras = {
                k: resolve(v, defs)
                for k, v in node.items()
                if k not in {"$defs", "$ref"}
            }
            if isinstance(resolved, dict):
                resolved.update(extras)
                resolved.pop("title", None)
                return resolved
            return resolved
        resolved = {k: resolve(v, defs) for k, v in node.items() if k != "$defs"}
        resolved.pop("title", None)
        return resolved

    return resolve(schema, schema.get("$defs", {}))


def tool(desc, args_type):
    def deco(fn):
        name = _tool_name(fn.__name__)
        _TOOLS[name] = ToolHandler(
            name=name,
            fn=fn,
            spec=ToolSpec(name, desc, _tool_schema(args_type)),
            args_type=args_type,
        )
        return fn

    return deco


class ToolRegistry:
    def __init__(self, tools=None):
        self._tools = _TOOLS if tools is None else tools

    def __contains__(self, n):
        return _tool_name(n) in self._tools

    def __iter__(self):
        return iter(self._tools)

    def get(self, n):
        return self._tools.get(_tool_name(n))

    def specs(self):
        return [t.spec for t in self._tools.values()]

    def invoke(self, state: AgentState, name: str, args: dict[str, Any] | None = None):
        name = _tool_name(name)
        return (
            handler.invoke(state, args)
            if (handler := self._tools.get(name))
            else ToolResult(ok=False, content=f"Tool '{name}' unavailable")
        )

    def without(self, *names):
        blocked = {_tool_name(name) for name in names}
        return ToolRegistry({k: v for k, v in self._tools.items() if k not in blocked})


TOOL_REGISTRY = ToolRegistry()


class AgentState(msgspec.Struct, omit_defaults=True):
    root: Path
    max_tool_calls: int
    tool_specs: ToolRegistry
    tool_calls: int = 0

    def note_tool_call(self) -> None:
        if self.tool_calls >= self.max_tool_calls:
            raise ValueError(
                f"reached max tool calls ({self.max_tool_calls}) without a final response"
            )
        self.tool_calls += 1


def _message_tokens(message: ChatMessage) -> int:
    body = (
        json.dumps(message.content.content, ensure_ascii=True, default=str)
        if isinstance(message, ToolMessage)
        else message.content
    )
    return 4 + count_tokens(body)


def _truncate_message(message: ChatMessage, max_tokens: int) -> ChatMessage:
    if isinstance(message, ToolMessage) or not message.content:
        return message
    if (
        truncated := truncate_str_to_tokens(message.content, max_tokens=max_tokens)
    ) is message.content:
        return message
    match message:
        case SystemMessage():
            return SystemMessage(truncated)
        case UserMessage():
            return UserMessage(truncated)
        case AssistantMessage():
            return AssistantMessage(
                truncated,
                tool_calls=message.tool_calls,
                thought_signatures=message.thought_signatures,
            )
    return message


class Transcript(msgspec.Struct, omit_defaults=True):
    messages: list[ChatMessage] = msgspec.field(default_factory=list)
    max_context_tokens: int = MAX_CONTEXT_TOKENS
    max_message_tokens: int = MAX_MESSAGE_TOKENS

    def set_system_prompt(self, system_prompt: str) -> None:
        if self.messages and isinstance(self.messages[0], SystemMessage):
            self.messages[0] = SystemMessage(system_prompt)
        else:
            self.messages[:0] = [SystemMessage(system_prompt)]

    def add_user(self, prompt: str) -> None:
        self.messages.append(UserMessage(prompt))

    def add_assistant(self, message: AssistantMessage) -> None:
        self.messages.append(message)

    def add_tool_outputs(self, calls, results) -> None:
        self.messages.extend(
            ToolMessage(tool_call_id=i, name=n, content=r)
            for (i, n, _), (_, r) in zip(calls, results, strict=False)
        )

    def truncate_message(self, message: ChatMessage) -> ChatMessage:
        return _truncate_message(message, self.max_message_tokens)

    def message_tokens(self, message: ChatMessage) -> int:
        return _message_tokens(message)

    def prepared_messages(self) -> list[ChatMessage]:
        msgs = [_truncate_message(m, self.max_message_tokens) for m in self.messages]
        sys_msgs = [m for m in msgs if isinstance(m, SystemMessage)]
        other = [m for m in msgs if not isinstance(m, SystemMessage)]
        budget = self.max_context_tokens - sum(map(_message_tokens, sys_msgs))
        if budget <= 0:
            return sys_msgs
        kept, used = [], 0
        for message in reversed(other):
            if (cost := _message_tokens(message)) + used <= budget:
                kept.append(message)
                used += cost
        kept.reverse()
        return (
            sys_msgs
            + (
                [
                    UserMessage(
                        f"... [{len(other) - len(kept)} earlier messages omitted to fit context limit]"
                    )
                ]
                if len(kept) < len(other)
                else []
            )
            + kept
        )

    def session_tokens(self) -> int:
        return sum(map(_message_tokens, self.messages))

    def prepared_tokens(self) -> int:
        return sum(map(_message_tokens, self.prepared_messages()))


def _join_paths(paths, root, empty="<no matches>"):
    return (
        "\n".join(_rel(root, p) + ("/" if p.is_dir() else "") for p in paths) or empty
    )


def _list_dir(root, target, limit):
    return _join_paths(
        sorted(target.iterdir(), key=lambda i: i.as_posix())[: max(limit, 1)],
        root,
        "<empty directory>",
    )


@tool(
    "List a directory. Use this first on unfamiliar trees. Returns sorted entries, one per line, with / for directories.",
    ListArgs,
)
def tool_list(state, path=".", limit=DEFAULT_LINE_LIMIT):
    note_tool(
        state,
        "list",
        _defaults={"path": ".", "limit": DEFAULT_LINE_LIMIT},
        path=path,
        limit=limit,
    )
    target = resolve_path(state.root, path)
    if not target.is_dir():
        raise ValueError("path is not a directory")
    text = _list_dir(state.root, target, limit)
    show(text, 1)
    return clip_tokens(text)


@tool(
    "Read a file or directory. Use before editing. Files return line-numbered text; directories fall back to list. Use offset/limit for large files.",
    ReadArgs,
)
def tool_read(state, path, offset=1, limit=DEFAULT_LINE_LIMIT):
    target = resolve_path(state.root, path)
    defaults = {"offset": 1, "limit": DEFAULT_LINE_LIMIT}
    if target.is_dir():
        note_tool(
            state,
            "read",
            _defaults={"path": ".", **defaults},
            path=path,
            offset=offset,
            limit=limit,
        )
        text = _list_dir(state.root, target, limit)
        show(text, 1)
        return clip_tokens(text)
    lines = target.read_text(encoding="utf-8", errors="replace").splitlines()
    start, total = max(offset, 1) - 1, len(lines)
    shown = lines[start : start + max(limit, 1)]
    note_tool(
        state,
        "read",
        _defaults=defaults,
        _suffix=(
            f"*(lines {start + 1}–{min(start + len(shown), total)} of {total})*"
            if total
            else ""
        ),
        path=path,
        offset=offset,
        limit=limit,
    )
    return clip_tokens(
        "\n".join(f"{i}: {line}" for i, line in enumerate(shown, start + 1))
        or "<empty file>"
    )


def _need(op, key, typ, msg):
    """Extract and validate a typed field from an operation dict."""
    value = op.get(key)
    if not isinstance(value, typ) or (typ is str and not value):
        raise ValueError(msg)
    return value


def _require_file(root, target, action):
    rel = _rel(root, target)
    if not target.exists():
        raise ValueError(f"file does not exist: {rel}")
    if target.is_dir():
        raise ValueError(f"cannot {action} directory: {rel}")
    return rel


def _apply_op(root, index, op):
    if not isinstance(op, dict):
        raise ValueError(f"operation {index} must be an object")
    kind = _need(op, "op", str, f"operation {index} is missing a valid op")
    path = _need(op, "path", str, f"operation {index} is missing a valid path")
    target = resolve_path(root, path)
    match kind:
        case "replace":
            rel = _require_file(root, target, "replace text in")
            updated, count = _replace(
                target.read_text(encoding="utf-8", errors="surrogateescape"),
                _need(
                    op,
                    "old",
                    str,
                    f"replace operation {index} requires string old and new",
                ),
                _need(
                    op,
                    "new",
                    str,
                    f"replace operation {index} requires string old and new",
                ),
                _need(
                    op,
                    "replace_all",
                    bool,
                    f"replace operation {index} replace_all must be boolean",
                )
                if "replace_all" in op
                else False,
            )
            target.write_text(updated, encoding="utf-8", errors="surrogateescape")
            return f"replaced {rel} ({count} match{'es' if count != 1 else ''})"
        case "write":
            content = _need(
                op, "content", str, f"write operation {index} requires string content"
            )
            overwrite = (
                _need(
                    op,
                    "overwrite",
                    bool,
                    f"write operation {index} overwrite must be boolean",
                )
                if "overwrite" in op
                else False
            )
            rel = _rel(root, target)
            if target.exists() and target.is_dir():
                raise ValueError(f"cannot write directory: {rel}")
            if target.exists() and not overwrite:
                raise ValueError(f"file already exists: {rel}; set overwrite=true")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content, encoding="utf-8")
            return f"wrote {rel}"
        case "move":
            rel = _require_file(root, target, "move")
            dest = resolve_path(
                root,
                _need(
                    op, "to", str, f"move operation {index} requires a valid to path"
                ),
            )
            if dest == target:
                raise ValueError(f"move destination matches source: {rel}")
            if dest.exists():
                raise ValueError(f"destination already exists: {_rel(root, dest)}")
            dest.parent.mkdir(parents=True, exist_ok=True)
            target.rename(dest)
            return f"⚠ moved {rel} -> {_rel(root, dest)}"
        case "delete":
            rel = _require_file(root, target, "delete")
            target.unlink()
            return f"⚠ deleted {rel}"
    raise ValueError(
        f"operation {index} has unsupported op {_fmt('inline', kind)}; use replace, write, move, or delete"
    )


@tool(
    "Edit files inside the workspace. Operations: replace, write, move, delete. Read first and keep edits precise.",
    ApplyArgs,
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
    out = "\n".join(_apply_op(state.root, i, op) for i, op in enumerate(operations, 1))
    show(out, len(operations))
    return out


@tool(
    "Run shell commands for tests, builds, git, and scripts. Do not use for routine file inspection. Returns stdout and stderr together.",
    BashArgs,
)
def tool_bash(state, command, timeout_seconds=120):
    if len(command.encode("utf-8", errors="replace")) > MAX_BASH_CMD_BYTES:
        raise ValueError(
            f"command too large ({len(command)} chars); limit is {MAX_BASH_CMD_BYTES} bytes"
        )
    note_tool(
        state,
        "bash",
        _defaults={"timeout": 120},
        command=command,
        timeout=timeout_seconds,
    )
    env = command_env(state.root)
    result = run_cmd(
        [which("bash", env.get("PATH")) or "bash", "-c", command],
        cwd=state.root,
        env=env,
        timeout=timeout_seconds,
    )
    out = _fmt("bash", command, (result.stdout, result.returncode, result.stderr))
    out = clip_tokens(out, tail=MAX_TOOL_TAIL_TOKENS)
    show(out, 8)
    return out


@tool(
    "Search file contents by text or regex. Use file_glob to narrow by filename pattern. Returns matching lines with file and line numbers.",
    GrepArgs,
)
def tool_grep(state, pattern, path=".", file_glob=None):
    env, target = command_env(state.root), resolve_path(state.root, path)
    if not target.exists():
        raise ValueError(f"search path does not exist: {_rel(state.root, target)}")
    if target.is_file() and file_glob:
        raise ValueError("file_glob only works when path is a directory")
    search_path = str(target)
    _grep_defaults = {"path": "."}
    for name, build in SEARCH_BACKENDS.items():
        if not (exe := which(name, env.get("PATH"))):
            continue
        result = run_cmd(
            build(exe, pattern, search_path, file_glob), cwd=state.root, env=env
        )
        if result.returncode not in (0, 1):
            detail = (result.stderr or result.stdout or f"{name} failed").strip()
            note_tool(
                state,
                "grep",
                _defaults=_grep_defaults,
                pattern=pattern,
                path=path,
                glob=file_glob,
            )
            raise ValueError(
                f"{name} search failed for {_rel(state.root, target)}: {detail}"
            )
        out = result.stdout.strip() or "<no matches>"
        matches = len(out.splitlines()) if out != "<no matches>" else 0
        suffix = f"*({matches} match{'es' if matches != 1 else ''})*" if matches else ""
        note_tool(
            state,
            "grep",
            _defaults=_grep_defaults,
            _suffix=suffix,
            pattern=pattern,
            path=path,
            glob=file_glob,
        )
        show(out, 3)
        return clip_tokens(out)
    note_tool(
        state,
        "grep",
        _defaults=_grep_defaults,
        pattern=pattern,
        path=path,
        glob=file_glob,
    )
    raise ValueError("grep requires `rg` or `grep` on PATH")


@tool(
    "Find files by name pattern like '*.py' or 'src/**/*.js'. Use when you know the path shape. Supports *, ?, and **.",
    GlobArgs,
)
def tool_glob(state, pattern, path=".", limit=DEFAULT_LINE_LIMIT):
    note_tool(
        state,
        "glob",
        _defaults={"path": ".", "limit": DEFAULT_LINE_LIMIT},
        pattern=pattern,
        path=path,
        limit=limit,
    )
    base = resolve_path(state.root, path)
    # H1: filter glob results to only include paths within the workspace root,
    # since glob patterns or symlinks could escape the workspace boundary.
    results = []
    for p in base.glob(pattern):
        try:
            resolved = p.resolve()
            if resolved == state.root or state.root in resolved.parents:
                results.append(resolved)
        except OSError:
            pass
    results.sort(key=lambda item: item.as_posix())
    out = _join_paths(results[: max(limit, 1)], state.root)
    show(out, 1)
    return clip_tokens(out)


def _enum(value, allowed, name):
    """Validate *value* is in *allowed* or None; raise ValueError otherwise."""
    if value is not None and value not in allowed:
        raise ValueError(
            f"{name} must be one of {', '.join(allowed[:-1])}, or {allowed[-1]}"
        )
    return value


def _positive_int(value, name):
    if not isinstance(value, int) or value <= 0:
        raise ValueError(f"{name} must be a positive integer")
    return value


@tool(
    "Fetch web pages or APIs over HTTP(S). Presets: page, json, post_json. Use json_path to extract nested fields. Sensitive headers are redacted.",
    HttpxArgs,
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
    preset = _enum(preset, _httpx_preset["enum"], "preset")
    if method is not None and not isinstance(method, str):
        raise ValueError("method must be a string")
    if body is not None and not isinstance(body, str):
        raise ValueError("body must be a string")
    if json_body is not None and not isinstance(
        json_body, (dict, list, str, int, float, bool)
    ):
        raise ValueError("json_body must be valid JSON-like data")
    timeout_seconds = _positive_int(timeout_seconds, "timeout_seconds")
    max_tokens = _positive_int(max_tokens, "max_tokens")
    response_mode = _enum(response_mode, _httpx_mode["enum"], "response_mode") or "auto"
    if json_path is not None and not isinstance(json_path, str):
        raise ValueError("json_path must be a string")
    if body is not None and json_body is not None:
        raise ValueError("provide either body or json_body, not both")
    method = (
        (method or ("POST" if body is not None or json_body is not None else "GET"))
        .strip()
        .upper()
    )
    if preset == "post_json" and method == "GET":
        method = "POST"
    if (
        response_mode == "auto"
        and preset in {"json", "post_json"}
        or response_mode == "body"
        and json_path
    ):
        response_mode = "json"
    if response_mode == "headers" and json_path:
        raise ValueError("json_path requires body or json output")
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
    _print("status", "Fetching HTTP content.", err=True)
    try:
        with httpx.Client(
            follow_redirects=True, timeout=float(timeout_seconds)
        ) as http:
            response = http.request(
                method,
                parsed.geturl(),
                headers=_norm_map(headers, "headers"),
                params=_norm_map(params, "params"),
                content=body,
                json=json_body,
            )
    except httpx.HTTPError as exc:
        raise ValueError(_httpx_err(exc, timeout_seconds)) from exc
    out = render_httpx_output(response, response_mode, json_path=json_path)
    show(out, 1)
    return clip_tokens(out, max_tokens)


@tool(
    "Ask the user a question in interactive runs. Use for significant ambiguity or decisions. Provide choices when useful.",
    AskArgs,
)
def tool_ask(state, question, choices=None):
    note_tool(state, "ask", question=question, choices=choices)
    if not sys.stdin.isatty():
        raise ValueError("Cannot ask question: stdin is not a TTY")
    _print("prompt", question, err=True)
    if not choices:
        return Prompt.ask("Answer", console=STDERR).strip()
    _print(
        value="## Options\n\n"
        + "\n".join(
            f"{i}. {_fmt('inline', choice)}" for i, choice in enumerate(choices, 1)
        ),
        err=True,
    )
    while True:
        response = Prompt.ask("Selection", console=STDERR).strip()
        if response.isdigit() and 0 < int(response) <= len(choices):
            return choices[int(response) - 1]
        if response in choices:
            return response
        _print(
            "warning",
            f"Enter a number 1–{len(choices)} or type the choice exactly.",
            err=True,
        )


def active_system_prompt(interactive):
    """Build the system prompt, choosing interactive or non-interactive suffix."""
    return BASE_SYSTEM_PROMPT + (
        INTERACTIVE_SYSTEM_PROMPT if interactive else NONINTERACTIVE_SYSTEM_PROMPT
    )


def active_tool_specs(interactive):
    """Return the tool registry, excluding ``ask`` in non-interactive mode."""
    return TOOL_REGISTRY if interactive else TOOL_REGISTRY.without("ask")


def chat_tools(specs):
    return specs.specs()


def run_tool(state: AgentState, name, args):
    """Dispatch a single tool call and return its ToolResult."""
    return state.tool_specs.invoke(state, name, args)


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
    omitted_lines = text[len(kept) :].count("\n")
    line_word = "line" if omitted_lines == 1 else "lines"
    kept = kept.rstrip()
    return (
        f"{kept}\n"
        f"... [truncated: {omitted_lines} {line_word}, "
        f"{omitted_chars} chars omitted to fit {max_tokens}-token limit]"
    )


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
        _print("status", f"Loading models from {_fmt('inline', shim)}.", err=True)
        all_models.extend(
            list_models_for_shim(shim, region=default_region(), cwd=Path.cwd())
        )
    return all_models


def list_model_ids() -> list[str]:
    """Return model IDs for the currently configured shim only (no prefix)."""
    require_api_env()
    spec = _model()
    shim = resolve_active_shim(spec)
    _print("status", f"Loading models from {_fmt('inline', shim)}.", err=True)
    return list_shim_model_ids(shim, region=default_region(), cwd=Path.cwd())


async def run_turn(
    client,
    transcript: Transcript,
    state: AgentState,
    model_spec,
    tool_defs,
    max_steps,
):
    # Strip shim prefix before sending to the API
    _, model = split_model_spec(model_spec)
    for _ in range(max_steps):
        prepared = transcript.prepared_messages()
        size_str = format_tokens(transcript.prepared_tokens())
        spinner = Status(
            f"Waiting for {model_spec} · {size_str}",
            console=STDERR,
            spinner="dots",
        )
        spinner.start()

        def on_retry(attempt, max_attempts, error_ctx=None):
            excerpt = ""
            if error_ctx:
                lines = error_ctx.strip().splitlines()
                excerpt = " · ".join(line.strip() for line in lines[:3] if line.strip())
            spinner.console.log(
                f"[dim]↳ retry {attempt}/{max_attempts}{': ' + excerpt if excerpt else ''}[/dim]"
            )
            spinner.update(
                f"Retrying {model_spec} (attempt {attempt}/{max_attempts}) · {size_str}"
            )

        try:
            message = await cast(CompletionClient, client).chat_completion(
                model=model,
                messages=prepared,
                tools=tool_defs,
                tool_choice="auto",
                on_retry=on_retry,
            )
        finally:
            spinner.stop()
        calls = [(call.id, call.name, call.arguments) for call in message.tool_calls]
        output = message.content
        if calls:
            transcript.add_assistant(message)
            results = [
                (call_id, run_tool(state, name, args)) for call_id, name, args in calls
            ]
            transcript.add_tool_outputs(calls, results)
            continue
        _print(value=output)
        return 0, output
    return fail(f"reached max steps ({max_steps}) without a final response"), ""


def _api_error_kind(e):
    return "authentication" if isinstance(e, AuthenticationError) else "permission"


async def run_agent(
    prompt,
    model,
    root,
    system_prompt,
    max_steps,
    max_tool_calls,
    interactive,
    transcript: Transcript | None = None,
):
    tool_specs = active_tool_specs(interactive)
    state = AgentState(root=root, max_tool_calls=max_tool_calls, tool_specs=tool_specs)
    transcript = transcript or Transcript()
    transcript.set_system_prompt(system_prompt)
    transcript.add_user(prompt)

    async def runner(client):
        return await run_turn(
            client, transcript, state, model, chat_tools(tool_specs), max_steps
        )

    try:
        return await runner(get_client(model))
    except (AuthenticationError, PermissionDeniedError) as exc:
        if not ensure_api_env(root):
            return fail(f"API {_api_error_kind(exc)} error: {exc}"), ""
        _print("warning", "Credentials expired. Refreshing.", err=True)
        try:
            return await runner(get_client(model))
        except (AuthenticationError, PermissionDeniedError) as exc:
            return fail(f"API {_api_error_kind(exc)} error: {exc}"), ""
        except Exception as exc:
            return fail(str(exc)), ""
    except RateLimitError as exc:
        return fail(f"API rate limit: {exc}"), ""
    except BadRequestError as exc:
        return fail(f"API bad request: {exc}"), ""
    except Exception as exc:
        return fail(str(exc)), ""


def read_system_prompt(system_file, interactive):
    base = active_system_prompt(interactive)
    if system_file is None:
        return base
    if not system_file.exists():
        abort(f"System file does not exist: {_fmt('inline', system_file)}")
    if system_file.is_dir():
        abort(f"System file is a directory: {_fmt('inline', system_file)}")
    try:
        return base + "\n\n" + system_file.read_text(encoding="utf-8")
    except OSError as exc:
        abort(f"Could not read system file {_fmt('inline', system_file)}: {exc}")


def _print_intro(heading, workspace, model, mode, **extras):
    lines = [
        f"## {heading}",
        "",
        f"- workspace: {_fmt('inline', workspace)}",
        f"- model: {_fmt('inline', model)}",
        f"- mode: {_fmt('inline', mode)}",
    ]
    for key, value in extras.items():
        if value is not None:
            lines.append(f"- {key}: {_fmt('inline', value)}")
    _print(value="\n".join(lines), err=True)


def _workspace():
    workspace = _ws().resolve()
    if not workspace.is_dir():
        abort(f"Workspace root is not a directory: {_fmt('inline', workspace)}")
    require_runtime(workspace)
    return workspace


def audit(prompt: str = ""):
    """Run a security and complexity audit of the repository.

    :param prompt: Additional audit focus instructions.
    """

    workspace = _workspace()
    chosen_model = _model(None)
    audit_prompt = "Conduct a security and complexity audit."
    if prompt:
        audit_prompt += f" Additional focus: {prompt}"
    _print_intro(
        "Audit",
        workspace,
        chosen_model,
        "non-interactive",
        focus=preview(prompt, 100) if prompt else None,
    )
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
    workspace = _workspace()
    chosen_model = _model(None)
    interactive = True
    system_file = _sys_file()
    system_prompt = read_system_prompt(system_file, interactive)
    _print_intro(
        "Chat",
        workspace,
        chosen_model,
        "interactive",
        **({"system file": system_file.resolve()} if system_file else {}),
    )

    transcript = Transcript(messages=[SystemMessage(system_prompt)])

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
                    transcript=transcript,
                )
            )
            size_str = format_tokens(transcript.session_tokens())
            STDERR.print(f"[dim]· {size_str}[/dim]")
        except KeyboardInterrupt, EOFError:
            _print(value="\n## Session Ended", err=True)
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

    workspace = _workspace()
    chosen_model = _model(None)
    system_file = _sys_file()
    interactive = sys.stdin.isatty() and not _flag("OY_NON_INTERACTIVE", False)
    system_prompt = read_system_prompt(system_file, interactive)
    _print_intro(
        "Run",
        workspace,
        chosen_model,
        "interactive" if interactive else "non-interactive",
        prompt=preview(task, 100),
        **({"system file": system_file.resolve()} if system_file else {}),
    )
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


def render_model_list(items, *, title, query=None, current=None, err=False, limit=None):
    shown = list(items if limit is None else items[:limit])
    lines = [title]
    if current:
        lines += ["", f"- current model: {_fmt('inline', current)}"]
    if query:
        lines += ["", f"- filter: {_fmt('inline', query)}"]
    lines += [""] + (
        [f"{i}. {_fmt('inline', item)}" for i, item in enumerate(shown, 1)]
        or ["- no matching models"]
    )
    if len(items) > len(shown):
        lines += ["", f"- showing {len(shown)} of {len(items)} matches"]
    _print(value="\n".join(lines), err=err)


def resolve_model_choice(model_id=None):
    available, current = list_all_model_ids(), _model(None)
    if model_id in available:
        return model_id
    if not sys.stdin.isatty():
        if model_id:
            matches = [m for m in available if model_id.strip().lower() in m.lower()]
            if matches:
                render_model_list(
                    matches,
                    title="## Matching Models",
                    query=model_id,
                    current=current,
                    err=True,
                )
            abort(
                f"No exact model match for {_fmt('inline', model_id)}. Re-run in a TTY to filter and choose interactively."
            )
        return None
    _print(
        value="## Choose a Model\n\n- Enter an exact model ID to save it.\n- Enter text to filter the list.\n- Enter a number to pick from the currently listed models.",
        err=True,
    )
    if model_id is None:
        render_model_list(
            available, title="## Available Models", current=current, err=True
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
        if query.isdigit() and 1 <= (idx := int(query)) <= len(shown):
            return shown[idx - 1]
        shown = [m for m in available if query.lower() in m.lower()]
        render_model_list(
            shown, title="## Matching Models", query=query, current=current, err=True
        )
        query = Prompt.ask("Model or filter", console=STDERR).strip()


def model(query: str | None = None, *, region: str | None = None, token: bool = False):
    """Show or set the default model, or print Bedrock credentials.

    :param query: Exact model ID to save, or a filter string when running in a TTY.
    :param region: AWS region to use with --token.
    :param token: Print Bedrock-backed OpenAI credentials instead of model selection.
    """
    if token:
        chosen = default_region() if region is None else region
        _print(
            "status",
            f"Generating Bedrock credentials for {_fmt('inline', chosen)}.",
            err=True,
        )
        value = make_bedrock_token(chosen, cwd=Path.cwd())
        _print(
            value="## Bedrock Credentials\n\n"
            + "Paste this into another shell if you want to reuse the current Bedrock session.\n\n"
            + _fmt(
                "block",
                "\n".join(
                    [
                        f"export OPENAI_BASE_URL={shlex.quote(bedrock_base_url(chosen))}",
                        f"export OPENAI_API_KEY={shlex.quote(value)}",
                    ]
                ),
                "bash",
            )
        )
        return 0
    current = _model(None)
    if query is None and not sys.stdin.isatty():
        shim = resolve_active_shim(current)
        _, bare = split_model_spec(current)
        _print(
            value=f"## Current Model\n\n- model: {_fmt('inline', bare)}\n- shim: {_fmt('inline', shim)}"
        )
        return 0
    # Interactive mode: show current model first if set
    if current:
        shim = resolve_active_shim(current)
        _, bare = split_model_spec(current)
        _print(
            value=f"## Current Model\n\n- model: {_fmt('inline', bare)}\n- shim: {_fmt('inline', shim)}",
            err=True,
        )
        if (
            not Prompt.ask(
                "\nPick a new model?", console=STDERR, choices=["y", "n"], default="n"
            )
            == "y"
        ):
            return 0
    chosen = resolve_model_choice(query)
    if chosen is None:
        return 1
    shim, bare_model = split_model_spec(chosen)
    cfg = {**_load_cfg(), "model": bare_model}
    (cfg.__setitem__("shim", shim) if shim else cfg.pop("shim", None))
    _save_cfg(cfg)
    _print(
        value=f"## Default Model Updated\n\n- selected: {_fmt('inline', chosen)}"
        + (f"\n- shim: {_fmt('inline', shim)}" if shim else "")
    )
    return 0


def main(argv: list[str] | None = None):
    args = list(sys.argv[1:] if argv is None else argv)
    commands = {"run", "chat", "model", "audit", "-h", "--help"}
    if not args:
        args = ["run"] if not sys.stdin.isatty() else ["--help"]
    elif args[0] in {"-v", "--version"}:
        _print(value=f"oy {__version__}")
        return 0
    elif args[0] not in commands:
        args = ["run", *args]
    result = defopt.run([run, chat, model, audit], argv=args, version=False, short={})
    return 0 if result is None else result


if __name__ == "__main__":
    raise SystemExit(main())
