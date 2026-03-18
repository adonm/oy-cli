from __future__ import annotations
import asyncio
from dataclasses import dataclass
import json
import logging
import os
import re
import shlex
import sys
import time
from pathlib import Path
from typing import Any, Callable, TypeAlias, cast
import defopt
from headroom import compress as headroom_compress
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
    ToolCall,
    ToolMessage,
    ToolResult,
    ToolSpec,
    UserMessage,
    which,
    CompletionClient,
    default_region,
    detect_available_shims,
    ensure_api_env as ensure_shim_api_env,
    get_client as build_shim_client,
    join_model_spec,
    list_models_for_shim,
    require_api_env as require_shim_api_env,
    resolve_shim as resolve_model_shim,
    split_model_spec,
    validate_shim,
)
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


MAX_BASH_CMD_BYTES = _env("MAX_BASH_CMD_BYTES", 65536)
MAX_CONTEXT_TOKENS = _env("MAX_CONTEXT_TOKENS", 131072)
DEFAULT_UNATTENDED_TIMEOUT_SECONDS = _env("UNATTENDED_TIMEOUT_SECONDS", 3600)
CONFIG_PATH = Path.home() / ".config" / "oy" / "config.json"
DEPENDENCY_TIMEOUT_SECONDS = 600
OPTIONAL_TOOL_INSTALLERS = {
    "rg": {
        "label": "ripgrep",
        "mise": "github:BurntSushi/ripgrep",
        "brew": "ripgrep",
    },
    "srgn": {
        "label": "srgn",
        "mise": "github:alexpovel/srgn",
        "brew": "srgn",
    },
    "tokei": {
        "label": "tokei",
        "mise": "github:XAMPPRocky/tokei",
        "brew": "tokei",
    },
    "curlie": {
        "label": "curlie",
        "mise": "github:rs/curlie",
        "brew": "curlie",
    },
    "yq": {
        "label": "yq",
        "mise": "github:mikefarah/yq",
        "brew": "yq",
    },
}


def _clamp_int(value: int, lower: int, upper: int) -> int:
    return max(lower, min(value, upper))


@dataclass(frozen=True, slots=True)
class RuntimeBudgets:
    message_tokens: int
    tool_output_tokens: int
    tool_tail_tokens: int
    default_line_limit: int


def _derive_runtime_budgets(context_tokens: int) -> RuntimeBudgets:
    tool_output_tokens = _clamp_int(context_tokens // 24, 2048, 8192)
    return RuntimeBudgets(
        message_tokens=_clamp_int(context_tokens // 16, tool_output_tokens, 12288),
        tool_output_tokens=tool_output_tokens,
        tool_tail_tokens=_clamp_int(tool_output_tokens // 5, 512, 2048),
        default_line_limit=_clamp_int(tool_output_tokens // 6, 200, 1200),
    )


BUDGETS = _derive_runtime_budgets(MAX_CONTEXT_TOKENS)

# ---------------------------------------------------------------------------
# Debug logging -- activated by OY_DEBUG=1
# Writes all LLM request/response messages to a JSON-lines tmpfile.
# The logger is initialised eagerly at import time so it is available even
# when early startup code (workspace resolution, model selection) fails.
# ---------------------------------------------------------------------------


def _init_debug_log() -> tuple[logging.Logger | None, str | None]:
    """Create a debug JSONL logger if OY_DEBUG is truthy.  Called once at import."""
    raw = os.environ.get("OY_DEBUG", "").strip().lower()
    if raw not in {"1", "true", "yes", "on"}:
        return None, None
    debug_dir = CONFIG_PATH.parent
    debug_dir.mkdir(parents=True, exist_ok=True)
    path = str(debug_dir / "debug.jsonl")
    logger = logging.getLogger("oy.debug")
    logger.setLevel(logging.DEBUG)
    logger.propagate = False
    handler = logging.FileHandler(path, encoding="utf-8")
    handler.setFormatter(logging.Formatter("%(message)s"))
    logger.addHandler(handler)
    return logger, path


_debug_logger, _debug_log_path = _init_debug_log()


def _msg_to_dict(msg) -> dict[str, Any]:
    """Serialize a ChatMessage to a plain dict for debug logging."""
    return msgspec.to_builtins(msg)


def _debug_log(event: str, **data: Any) -> None:
    """Write a timestamped JSON-lines entry to the debug log (no-op if disabled)."""
    if _debug_logger is None:
        return
    import time as _time
    entry = {
        "ts": _time.time(),
        "event": event,
        **data,
    }
    _debug_logger.debug(json.dumps(entry, default=str, ensure_ascii=False))


# ---------------------------------------------------------------------------
# Prompts – parsed from README.md (single source of truth)
# ---------------------------------------------------------------------------

def _load_readme() -> str:
    """Return the README text, preferring the file next to this module.

    For editable/dev installs the file is always fresher than cached metadata.
    For proper installs the file won't exist, so we fall back to package metadata
    (setuptools embeds README.md via the ``readme`` key in pyproject.toml).
    """
    readme_path = Path(__file__).resolve().parent / "README.md"
    if readme_path.exists():
        return readme_path.read_text(encoding="utf-8")
    try:
        from importlib.metadata import metadata as _metadata
        text = _metadata("oy-cli").get_payload()
        if text:
            return text
    except Exception:
        pass
    raise RuntimeError("Cannot locate README.md for prompt extraction")


def _parse_prompts(readme: str) -> dict[str, str]:
    """Extract prompts from README: ``### Header`` followed by a ````markdown`` code block.

    Header is slugified (lowercased, non-alpha stripped) so
    ``Non-Interactive Appendix`` → ``noninteractiveappendix``.
    Returns {slug: content} stripped.
    """
    prompts: dict[str, str] = {}
    pattern = re.compile(
        r"^### ([^\n]+)\n+```markdown\n(.*?)```", re.MULTILINE | re.DOTALL
    )
    for m in pattern.finditer(readme):
        slug = re.sub(r"[^a-z0-9]", "", m.group(1).strip().lower())
        prompts[slug] = m.group(2).strip()
    return prompts


def _parse_tool_descriptions(readme: str) -> dict[str, str]:
    """Extract tool descriptions from the first table under ``## Tools``."""
    tools_match = re.search(r"^## Tools\b.*?\n(\|.+?\|\n)+", readme, re.MULTILINE | re.DOTALL)
    if not tools_match:
        raise RuntimeError("Could not find ## Tools table in README")
    descs: dict[str, str] = {}
    for line in tools_match.group(0).splitlines():
        m = re.match(r"\| `(\w+)` \| (.+?) \|$", line)
        if m:
            descs[m.group(1)] = m.group(2)
    return descs


_README = _load_readme()
_PROMPTS = _parse_prompts(_README)
_TOOL_DESCS = _parse_tool_descriptions(_README)

BASE_SYSTEM_PROMPT = _PROMPTS["baseprompt"]
INTERACTIVE_SYSTEM_PROMPT = _PROMPTS["interactiveappendix"]
NONINTERACTIVE_SYSTEM_PROMPT = _PROMPTS["noninteractiveappendix"]
AUDIT_SYSTEM_PROMPT = _PROMPTS["auditprompt"]
STDOUT, STDERR = Console(), Console(stderr=True)


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


def clip_tokens(text, limit=None, tail=0):
    """Truncate *text* to *limit* tokens, optionally keeping *tail* tokens from the end."""
    limit = BUDGETS.tool_output_tokens if limit is None else limit
    ids = encode_tokens(text)
    n = len(ids)
    if n <= limit:
        return text
    omitted = n - limit
    if 0 < tail < limit:
        h = max(limit - tail, 1)
        return (
            f"{decode_tokens(ids[:h])}\n"
            f"... [{omitted} tokens omitted; showing first {h} and last {tail}]\n"
            f"{decode_tokens(ids[-tail:])}"
        )
    return f"{decode_tokens(ids[:limit])}\n... [{omitted} tokens omitted after {limit}]"


def preview(v, lim=72):
    """Return a one-line preview of *v*, truncated to *lim* characters."""
    s = " ".join(
        (v if isinstance(v, str) else json.dumps(v, separators=(",", ":"))).split()
    )
    return s if len(s) <= lim else s[: lim - 3] + "..."


def _format_duration(seconds: int) -> str:
    if seconds % 3600 == 0:
        return f"{seconds // 3600}h"
    if seconds % 60 == 0:
        return f"{seconds // 60}m"
    return f"{seconds}s"


def _show_and_clip(text, lines, limit=None, tail=0):
    """Render tool output, then return a clipped version for the model."""
    limit = BUDGETS.tool_output_tokens if limit is None else limit
    show(text, lines)
    return clip_tokens(text, limit=limit, tail=tail)


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


def _installer_recipes(tool: str):
    spec = OPTIONAL_TOOL_INSTALLERS.get(tool, {})
    recipes: list[tuple[str, list[str]]] = []
    if target := spec.get("mise"):
        recipes.append(("mise", ["mise", "use", "-g", target]))
    if package := spec.get("brew"):
        recipes.append(("brew", ["brew", "install", package]))
    return recipes


def _preferred_installer(tool: str, env: dict[str, str]):
    for installer, command in _installer_recipes(tool):
        if binary := which(installer, env.get("PATH")):
            return [binary, *command[1:]], installer
    return None, None


def _missing_tool_install_message(tool: str, reason: str):
    recipes = _installer_recipes(tool)
    lines = [
        f"Missing {_fmt('inline', tool)} for {reason}.",
        "",
        "Set up `mise` (preferred) or Homebrew, then rerun oy.",
    ]
    if recipes:
        lines += ["", "Preferred:", f"  {shlex.join(recipes[0][1])}"]
        if len(recipes) > 1:
            lines += ["", "Fallback:", f"  {shlex.join(recipes[1][1])}"]
    return "\n".join(lines)


def ensure_optional_tool(tool: str, *, reason: str, cwd=None):
    env = dict(command_env(cwd))
    if which(tool, env.get("PATH")):
        return env

    command, installer = _preferred_installer(tool, env)
    if command is None or installer is None:
        abort(_missing_tool_install_message(tool, reason))

    label = OPTIONAL_TOOL_INSTALLERS.get(tool, {}).get("label", tool)
    _print("status", f"Installing {label} via {installer}.", err=True)
    result = run_cmd(
        command,
        cwd=cwd if cwd and cwd.is_dir() else None,
        env=env,
        timeout=DEPENDENCY_TIMEOUT_SECONDS,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        extra = f"\n\nInstaller output:\n{detail}" if detail else ""
        abort(
            f"Failed to install {label} via {installer}.{extra}\n\n"
            + _missing_tool_install_message(tool, reason)
        )

    command_env.cache_clear()
    refreshed = dict(command_env(cwd))
    if which(tool, refreshed.get("PATH")):
        return refreshed

    abort(
        f"Installed {label}, but {_fmt('inline', tool)} is still unavailable on PATH.\n\n"
        + _missing_tool_install_message(tool, reason)
    )


_MISSING_COMMAND_RE = re.compile(
    r"(?m)(?:^|: )(?:line \d+: )?(?P<name>[^:\s]+): (?:command not found|not found)$"
)


def _missing_shell_command(stderr: str) -> str | None:
    match = _MISSING_COMMAND_RE.search(stderr.strip())
    if not match:
        return None
    name = Path(match.group("name")).name
    return name if name in OPTIONAL_TOOL_INSTALLERS else None


def run_cmd_auto_install(
    cmd, *, cwd=None, env=None, timeout=120, stdin_text=None, reason="command"
):
    current_env = dict(command_env(cwd) if env is None else env)
    installed: set[str] = set()
    for _ in range(len(OPTIONAL_TOOL_INSTALLERS) + 1):
        try:
            result = run_cmd(
                cmd,
                cwd=cwd,
                env=current_env,
                timeout=timeout,
                stdin_text=stdin_text,
            )
        except FileNotFoundError:
            name = Path(cmd[0]).name if cmd else ""
            if name not in OPTIONAL_TOOL_INSTALLERS or name in installed:
                raise
            current_env = ensure_optional_tool(name, reason=reason, cwd=cwd)
            installed.add(name)
            continue
        if missing := _missing_shell_command(result.stderr):
            if missing not in installed:
                current_env = ensure_optional_tool(missing, reason=reason, cwd=cwd)
                installed.add(missing)
                continue
        return result
    raise RuntimeError("Too many helper installation attempts")


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


def note_tool(state: AgentState, name, *, _defaults=None, _suffix="", **details):
    state.note_progress()
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
    # Use bullet for mutating tools (bash), plain for inspection tools
    if name in {"bash"}:
        _print(value=f"* {message}", err=True)
    else:
        _print(value=message, err=True)


# Tool schemas and argument decoding are msgspec-native now.
class ListArgs(msgspec.Struct, omit_defaults=True):
    path: str = "*"
    limit: int = BUDGETS.default_line_limit


class ReadArgs(msgspec.Struct, omit_defaults=True):
    path: str
    offset: int = 1
    limit: int = BUDGETS.default_line_limit


class BashArgs(msgspec.Struct, omit_defaults=True):
    command: str
    timeout_seconds: int = 120


class SearchArgs(msgspec.Struct, omit_defaults=True):
    pattern: str
    path: str = "."
    args: list[str] = []
    limit: int = BUDGETS.default_line_limit


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

    def specs(self):
        return [t.spec for t in self._tools.values()]

    def invoke(self, state: AgentState, name: str, args: dict[str, Any] | None = None):
        return (
            handler.invoke(state, args)
            if (handler := self._tools.get(name))
            else ToolResult(ok=False, content=f"Tool '{name}' unavailable")
        )


TOOL_REGISTRY = ToolRegistry()


class AgentState(msgspec.Struct, omit_defaults=True):
    root: Path
    tool_specs: ToolRegistry
    unattended_timeout_seconds: int
    unattended_deadline: float

    def remaining_unattended_seconds(self) -> float:
        return self.unattended_deadline - time.monotonic()

    def note_progress(self) -> None:
        if self.remaining_unattended_seconds() <= 0:
            raise TimeoutError(
                "reached unattended timeout "
                f"({_format_duration(self.unattended_timeout_seconds)}) without a final response"
            )


def _message_text(message: ChatMessage) -> str:
    """Return a message body as plain text for token counting/rendering."""
    if isinstance(message, ToolMessage):
        return _headroom_tool_output(message.content)
    return message.content


def _message_tokens(message: ChatMessage) -> int:
    return 4 + count_tokens(_message_text(message))


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


def _headroom_tool_output(result: ToolResult) -> str:
    value = result.content
    return value if isinstance(value, str) else json.dumps(value, ensure_ascii=True, default=str)


def _headroom_tool_call(call: ToolCall) -> dict[str, Any]:
    return {
        "id": call.id,
        "type": "function",
        "function": {
            "name": call.name,
            "arguments": json.dumps(call.arguments, ensure_ascii=True, separators=(",", ":")),
        },
    }


def _serialize_for_headroom(message: ChatMessage) -> dict[str, Any]:
    match message:
        case SystemMessage():
            return {"role": "system", "content": message.content}
        case UserMessage():
            return {"role": "user", "content": message.content}
        case AssistantMessage():
            payload: dict[str, Any] = {"role": "assistant", "content": message.content}
            if message.tool_calls:
                payload["tool_calls"] = [_headroom_tool_call(call) for call in message.tool_calls]
            return payload
        case ToolMessage():
            return {
                "role": "tool",
                "tool_call_id": message.tool_call_id,
                "name": message.name,
                "ok": message.content.ok,
                "content": _headroom_tool_output(message.content),
            }
    raise TypeError(f"Unsupported message type: {type(message).__name__}")


def _decode_headroom_tool_arguments(arguments: Any) -> dict[str, Any]:
    if isinstance(arguments, dict):
        return arguments
    if arguments in (None, ""):
        return {}
    if not isinstance(arguments, str):
        raise RuntimeError("Tool arguments must be a JSON object or JSON string")
    parsed = msgspec.json.decode(arguments)
    parsed = msgspec.json.decode(parsed) if isinstance(parsed, str) else parsed
    if not isinstance(parsed, dict):
        raise RuntimeError("Tool arguments must decode to a JSON object")
    return parsed


def _deserialize_headroom_tool_call(payload: dict[str, Any]) -> ToolCall:
    function = payload.get("function")
    if isinstance(function, dict):
        return ToolCall(
            id=str(payload.get("id") or ""),
            name=str(function.get("name") or ""),
            arguments=_decode_headroom_tool_arguments(function.get("arguments")),
        )
    return ToolCall(
        id=str(payload.get("id") or ""),
        name=str(payload.get("name") or ""),
        arguments=_decode_headroom_tool_arguments(payload.get("arguments")),
    )


def _deserialize_from_headroom(message: dict[str, Any]) -> ChatMessage:
    role = message.get("role")
    if role == "system":
        return SystemMessage(str(message.get("content") or ""))
    if role == "user":
        return UserMessage(str(message.get("content") or ""))
    if role == "assistant":
        tool_calls = message.get("tool_calls")
        return AssistantMessage(
            str(message.get("content") or ""),
            tool_calls=[
                _deserialize_headroom_tool_call(call)
                for call in tool_calls
                if isinstance(call, dict)
            ] if isinstance(tool_calls, list) else [],
        )
    if role == "tool":
        return ToolMessage(
            tool_call_id=str(message.get("tool_call_id") or ""),
            name=str(message.get("name") or ""),
            content=ToolResult(
                ok=bool(message.get("ok", True)),
                content=message.get("content"),
            ),
        )
    raise RuntimeError(f"Unsupported headroom message role: {role!r}")


def _compress_messages_with_headroom(
    messages: list[ChatMessage], model: str, model_limit: int
) -> list[ChatMessage]:
    result = headroom_compress(
        [_serialize_for_headroom(message) for message in messages],
        model=model,
        model_limit=model_limit,
    )
    return [_deserialize_from_headroom(message) for message in result.messages]


class Transcript(msgspec.Struct, omit_defaults=True):
    messages: list[ChatMessage] = msgspec.field(default_factory=list)
    max_context_tokens: int = MAX_CONTEXT_TOKENS
    max_message_tokens: int = BUDGETS.message_tokens

    def set_system_prompt(self, system_prompt: str) -> None:
        if self.messages and isinstance(self.messages[0], SystemMessage):
            self.messages[0] = SystemMessage(system_prompt)
        else:
            self.messages[:0] = [SystemMessage(system_prompt)]

    def checkpoint(self) -> int:
        """Save the current message count so we can rollback on failure."""
        return len(self.messages)

    def rollback(self, checkpoint: int) -> None:
        """Discard all messages added after *checkpoint*."""
        del self.messages[checkpoint:]

    def add_user(self, prompt: str) -> None:
        self.messages.append(UserMessage(prompt))

    def add_assistant(self, message: AssistantMessage) -> None:
        self.messages.append(message)

    def add_tool_outputs(self, calls, results) -> None:
        self.messages.extend(
            ToolMessage(tool_call_id=i, name=n, content=r)
            for (i, n, _), (_, r) in zip(calls, results, strict=False)
        )

    def message_tokens(self, message: ChatMessage) -> int:
        return _message_tokens(message)

    def prepared_messages(self, model: str | None = None) -> list[ChatMessage]:
        msgs = [_truncate_message(m, self.max_message_tokens) for m in self.messages]
        if model:
            msgs = _compress_messages_with_headroom(msgs, model, self.max_context_tokens)
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

    def prepared_tokens(self, model: str | None = None) -> int:
        return sum(map(_message_tokens, self.prepared_messages(model=model)))


def _join_paths(paths, root, empty="<no matches>"):
    return (
        "\n".join(_rel(root, p) + ("/" if p.is_dir() else "") for p in paths) or empty
    )


def _glob_paths(root: Path, pattern: str) -> list[Path]:
    if Path(pattern).is_absolute() or ".." in Path(pattern).parts:
        raise ValueError(f"Path traversal denied: '{pattern}'")
    matches: list[Path] = []
    for candidate in root.glob(pattern):
        try:
            resolved = candidate.resolve()
        except OSError:
            continue
        if resolved == root or root in resolved.parents:
            matches.append(resolved)
    return sorted(set(matches), key=lambda item: item.as_posix())


@tool(_TOOL_DESCS["list"], ListArgs)
def tool_list(state, path="*", limit=BUDGETS.default_line_limit):
    note_tool(
        state,
        "list",
        _defaults={"path": "*", "limit": BUDGETS.default_line_limit},
        path=path,
        limit=limit,
    )
    text = _join_paths(_glob_paths(state.root, path)[: max(limit, 1)], state.root)
    return _show_and_clip(text, 1)


@tool(_TOOL_DESCS["read"], ReadArgs)
def tool_read(state, path, offset=1, limit=BUDGETS.default_line_limit):
    note_tool(
        state,
        "read",
        _defaults={"offset": 1, "limit": BUDGETS.default_line_limit},
        path=path,
        offset=offset,
        limit=limit,
    )
    target = resolve_path(state.root, path)
    if not target.exists():
        raise ValueError(f"read path does not exist: {_rel(state.root, target)}")
    if target.is_dir():
        text = _join_paths(
            sorted(target.iterdir(), key=lambda item: item.as_posix())[: max(limit, 1)],
            state.root,
            "<empty directory>",
        )
        return _show_and_clip(text, 1)
    start = max(_positive_int(offset, "offset"), 1) - 1
    lines = target.read_text(encoding="utf-8", errors="replace").splitlines()
    shown = lines[start : start + max(limit, 1)]
    text = "\n".join(f"{lineno}: {line}" for lineno, line in enumerate(shown, start + 1))
    return _show_and_clip(text or "<empty file>", 1)


@tool(_TOOL_DESCS["bash"], BashArgs)
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
    result = run_cmd_auto_install(
        [which("bash", command_env(state.root).get("PATH")) or "bash", "-c", command],
        cwd=state.root,
        env=command_env(state.root),
        timeout=timeout_seconds,
        reason="bash command",
    )
    out = _fmt("bash", command, (result.stdout, result.returncode, result.stderr))
    return _show_and_clip(out, 8, tail=BUDGETS.tool_tail_tokens)


def _search_summary(matches: int, shown: int) -> str:
    if not matches:
        return "*(no matches)*"
    extra = f"; showing {shown} of {matches}" if shown < matches else ""
    plural = "es" if matches != 1 else ""
    return f"*({matches} match{plural}{extra})*"


def _trim_search_lines(lines: list[str], limit: int) -> tuple[str, int, int]:
    total = len(lines)
    shown = lines[: max(limit, 1)]
    if not shown:
        return "<no matches>", total, 0
    out = "\n".join(shown)
    if total > len(shown):
        out += f"\n... [{total - len(shown)} more matches omitted]"
    return out, total, len(shown)


def _search_contents(
    root: Path,
    pattern: str,
    path: str,
    *,
    limit: int,
    args: list[str] | None = None,
):
    target = resolve_path(root, path)
    if not target.exists():
        raise ValueError(f"search path does not exist: {_rel(root, target)}")

    rg_args = [
        "rg",
        "--json",
        "--line-number",
        "--color",
        "never",
        *(args or []),
        pattern,
        str(target),
    ]
    result = run_cmd_auto_install(
        rg_args,
        cwd=root,
        env=command_env(root),
        reason="search",
    )
    if result.returncode not in (0, 1):
        err = result.stderr.strip()
        detail = f": {err}" if err else ""
        raise ValueError(f"rg failed with exit status {result.returncode}{detail}")

    lines: list[str] = []
    for raw in result.stdout.splitlines():
        if not raw.strip():
            continue
        event = json.loads(raw)
        kind = event.get("type")
        data = event.get("data", {})
        if kind == "match":
            path_text = data.get("path", {}).get("text", "")
            line_number = data.get("line_number")
            text_value = data.get("lines", {}).get("text", "").rstrip("\n")
            submatches = data.get("submatches") or []
            column = (submatches[0].get("start", 0) + 1) if submatches else 1
            rel = _rel(root, Path(path_text)) if path_text else "."
            lines.append(f"{rel}:{line_number}:{column}:{text_value}")
        elif kind == "context":
            path_text = data.get("path", {}).get("text", "")
            line_number = data.get("line_number")
            text_value = data.get("lines", {}).get("text", "").rstrip("\n")
            rel = _rel(root, Path(path_text)) if path_text else "."
            lines.append(f"{rel}-{line_number}-:{text_value}")

    out, total, shown = _trim_search_lines(lines, limit)
    return out, total, shown


@tool(_TOOL_DESCS["search"], SearchArgs)
def tool_search(state, pattern, path=".", args=None, limit=BUDGETS.default_line_limit):
    defaults = {"path": ".", "args": [], "limit": BUDGETS.default_line_limit}
    out, matches, shown = _search_contents(
        state.root,
        pattern,
        path,
        limit=limit,
        args=args,
    )
    note_tool(
        state,
        "search",
        _defaults=defaults,
        _suffix=_search_summary(matches, shown),
        pattern=pattern,
        path=path,
        args=args,
        limit=limit,
    )
    return _show_and_clip(out, 3)


def _positive_int(value, name):
    if not isinstance(value, int) or value <= 0:
        raise ValueError(f"{name} must be a positive integer")
    return value


@tool(_TOOL_DESCS["ask"], AskArgs)
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
            f"Enter a number 1-{len(choices)} or type the choice exactly.",
            err=True,
        )


def active_system_prompt(interactive):
    """Build the system prompt, choosing interactive or non-interactive suffix."""
    suffix = INTERACTIVE_SYSTEM_PROMPT if interactive else NONINTERACTIVE_SYSTEM_PROMPT
    return BASE_SYSTEM_PROMPT + "\n" + suffix + "\n"


def active_tool_specs(interactive):
    """Return the tool registry, excluding ``ask`` in non-interactive mode."""
    return (
        TOOL_REGISTRY
        if interactive
        else ToolRegistry({name: tool for name, tool in _TOOLS.items() if name != "ask"})
    )


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


def encode_tokens(text: str) -> list[int]:
    """Encode *text* with the shared tokenizer."""
    return get_tokenizer().encode(text, disallowed_special=())


def decode_tokens(token_ids: list[int]) -> str:
    """Decode token ids with the shared tokenizer."""
    return get_tokenizer().decode(token_ids)


def count_tokens(text: str) -> int:
    """Count tokens in a string using cl100k_base."""
    return len(encode_tokens(text))


def truncate_str_to_tokens(
    text: str, max_tokens: int = BUDGETS.message_tokens
) -> str:
    """Truncate *text* to at most *max_tokens* tokens.

    If truncation is needed, appends a note reporting how many lines and
    characters were removed so the model knows the content was cut.
    """
    ids = encode_tokens(text)
    if len(ids) <= max_tokens:
        return text
    kept = decode_tokens(ids[:max_tokens])
    omitted_chars = len(text) - len(kept)
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


async def run_turn(
    client,
    transcript: Transcript,
    state: AgentState,
    model_spec,
    tool_defs,
):
    # Strip shim prefix before sending to the API
    _, model = split_model_spec(model_spec)
    step = 0
    while True:
        state.note_progress()
        prepared = transcript.prepared_messages(model=model)
        _debug_log(
            "request",
            model=model_spec,
            step=step,
            messages=[_msg_to_dict(m) for m in prepared],
            tool_count=len(tool_defs),
        )
        size_str = format_tokens(sum(map(_message_tokens, prepared)))
        spinner = Status(
            f"Waiting for {model_spec} | {size_str}",
            console=STDERR,
            spinner="dots",
        )
        spinner.start()

        def on_retry(attempt, max_attempts, error_ctx=None):
            excerpt = ""
            if error_ctx:
                lines = error_ctx.strip().splitlines()
                excerpt = " | ".join(line.strip() for line in lines[:3] if line.strip())
            spinner.console.log(
                f"[dim]\\-> retry {attempt}/{max_attempts}{': ' + excerpt if excerpt else ''}[/dim]"
            )
            spinner.update(
                f"Retrying {model_spec} (attempt {attempt}/{max_attempts}) | {size_str}"
            )

        try:
            message = await asyncio.wait_for(
                cast(CompletionClient, client).chat_completion(
                    model=model,
                    messages=prepared,
                    tools=tool_defs,
                    tool_choice="auto",
                    on_retry=on_retry,
                ),
                timeout=state.remaining_unattended_seconds(),
            )
        except asyncio.TimeoutError as exc:
            raise TimeoutError(
                "reached unattended timeout "
                f"({_format_duration(state.unattended_timeout_seconds)}) without a final response"
            ) from exc
        finally:
            spinner.stop()
        calls = [(call.id, call.name, call.arguments) for call in message.tool_calls]
        output = message.content
        _debug_log(
            "response",
            model=model_spec,
            step=step,
            assistant=_msg_to_dict(message),
        )
        if calls:
            transcript.add_assistant(message)
            results = [
                (call_id, run_tool(state, name, args)) for call_id, name, args in calls
            ]
            _debug_log(
                "tool_results",
                model=model_spec,
                step=step,
                results=[
                    {"call_id": cid, "name": n, "ok": r.ok}
                    for (cid, n, _), (_, r) in zip(calls, results, strict=False)
                ],
            )
            transcript.add_tool_outputs(calls, results)
            step += 1
            continue
        _print(value=output)
        return 0, output


def _api_error_kind(e):
    return "authentication" if isinstance(e, AuthenticationError) else "permission"


async def run_agent(
    prompt,
    model,
    root,
    system_prompt,
    unattended_timeout_seconds,
    interactive,
    transcript: Transcript | None = None,
):
    tool_specs = active_tool_specs(interactive)
    unattended_timeout_seconds = _positive_int(
        unattended_timeout_seconds, "unattended_timeout_seconds"
    )
    state = AgentState(
        root=root,
        tool_specs=tool_specs,
        unattended_timeout_seconds=unattended_timeout_seconds,
        unattended_deadline=time.monotonic() + unattended_timeout_seconds,
    )
    transcript = transcript or Transcript()
    transcript.set_system_prompt(system_prompt)
    transcript.add_user(prompt)

    async def runner(client):
        return await run_turn(client, transcript, state, model, chat_tools(tool_specs))

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
    if _debug_log_path:
        lines.append(f"- debug log: {_fmt('inline', _debug_log_path)}")
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
            DEFAULT_UNATTENDED_TIMEOUT_SECONDS,
            interactive=False,
        )
    )
    return code


def _setup_readline():
    """Configure readline with persistent history for shell-like UX."""
    try:
        import readline
    except ImportError:
        return  # no readline on minimal builds (Alpine, WASM)
    history_path = CONFIG_PATH.parent / "history"
    history_path.parent.mkdir(parents=True, exist_ok=True)
    try:
        readline.read_history_file(history_path)
    except FileNotFoundError:
        pass
    readline.set_history_length(1000)
    # Ensure history file has restrictive permissions (M2: OWASP ASVS V8.3.4)
    history_path.touch(mode=0o600, exist_ok=True)
    import atexit

    atexit.register(readline.write_history_file, str(history_path))


def _drain_stdin(timeout: float = 0.05) -> str:
    """Read any data already buffered on stdin (e.g. the tail of a paste).

    Uses select() with a short timeout.  Returns the extra text, or "".
    Only works on real ttys; returns "" for piped stdin.
    """
    import select
    if not sys.stdin.isatty():
        return ""
    chunks: list[str] = []
    while True:
        ready, _, _ = select.select([sys.stdin], [], [], timeout)
        if not ready:
            break
        chunk = os.read(sys.stdin.fileno(), 4096)
        if not chunk:
            break
        chunks.append(chunk.decode("utf-8", errors="replace"))
        # After first chunk, use a tighter timeout for the rest.
        timeout = 0.01
    return "".join(chunks)


def _read_input():
    '''Read user input, with automatic paste detection.

    Input modes:
    1. Single line  -- type and press Enter.
    2. Paste        -- paste multiline text; lines that arrive within a
       few milliseconds of Enter are collected automatically.
    3. Block mode   -- start with ``"""`` to open a fenced block;
       close it with ``"""`` on its own line.

    Paste detection works by draining stdin right after readline returns.
    During normal typing there is nothing buffered, so it is a no-op.
    During a paste, the remaining lines are already queued up.
    '''
    line = input("oy > ")

    # --- block mode: triple-quote fence (still supported) ------------------
    stripped = line.strip()
    if stripped == '"""' or stripped.startswith('"""'):
        if stripped == '"""':
            parts: list[str] = []
        else:
            parts = [stripped[3:]]
        while True:
            try:
                cont = input('... ')
            except EOFError:
                break
            if cont.strip() == '"""':
                break
            parts.append(cont)
        return "\n".join(parts)

    # --- paste detection: drain any remaining buffered input ---------------
    extra = _drain_stdin()
    if extra:
        # Strip trailing newline that the terminal added from the final Enter.
        return line + "\n" + extra.rstrip("\n")

    return line
def _chat_command(cmd, transcript, system_prompt, model_spec):
    """Handle a /command.  Return True if handled, None to exit, False if unknown."""
    cmd = cmd.strip().lower()
    _, model = split_model_spec(model_spec)
    if cmd in ("/help", "/?"):
        _print(value="\n".join([
            "## Commands",
            "",
            "- `/help` -- show this help",
            "- `/tokens` -- show context usage",
            "- `/clear` -- reset conversation (keeps system prompt)",
            "- `/quit` or `/exit` -- end session",
            "",
            "Context is compressed with Headroom before model requests.",
            "Tip: paste multiline text — extra lines are detected automatically.",
            'Tip: type `"""` to start a multiline block, `"""` to end it.',
        ]), err=True)
        return True
    if cmd == "/tokens":
        total = transcript.session_tokens()
        prepped = transcript.prepared_tokens(model=model)
        budget = transcript.max_context_tokens
        msgs = len(transcript.messages)
        _print(value="\n".join([
            "## Context",
            "",
            f"- messages: {msgs}",
            f"- session tokens: {format_tokens(total)}",
            f"- prepared tokens: {format_tokens(prepped)}",
            f"- context budget: {format_tokens(budget)}",
            f"- remaining: ~{format_tokens(max(budget - prepped, 0))}",
        ]), err=True)
        return True
    if cmd == "/clear":
        transcript.messages = [SystemMessage(system_prompt)]
        _print(value="Conversation cleared.", err=True)
        return True
    if cmd in ("/quit", "/exit"):
        return None  # sentinel: exit
    return False


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
    _print(value="Type `/help` for commands.", err=True)

    transcript = Transcript(messages=[SystemMessage(system_prompt)])

    while True:
        try:
            STDERR.print()
            STDERR.rule(style="dim")
            prompt = _read_input()
        except KeyboardInterrupt:
            STDERR.print()
            continue
        except EOFError:
            _print(value="\n## Session Ended", err=True)
            break

        if not prompt.strip():
            continue

        # Slash commands
        if prompt.strip().startswith("/"):
            result = _chat_command(prompt.strip(), transcript, system_prompt, chosen_model)
            if result is None:
                break
            if result:
                continue
            _print("warning", f"Unknown command: {prompt.strip().split()[0]}", err=True)
            continue

        # Legacy exit
        if prompt.strip().lower() in ("exit", "quit"):
            break

        cp = transcript.checkpoint()
        try:
            code, _ = asyncio.run(
                run_agent(
                    prompt,
                    chosen_model,
                    workspace,
                    system_prompt,
                    DEFAULT_UNATTENDED_TIMEOUT_SECONDS,
                    interactive,
                    transcript=transcript,
                )
            )
        except KeyboardInterrupt:
            transcript.rollback(cp)
            _print(value="\nCancelled — your message is in readline history (press ↑).", err=True)
            continue
        except Exception as exc:
            transcript.rollback(cp)
            _print("error", f"Agent error: {exc}", err=True)
            _print(value="Your message is in readline history (press ↑).", err=True)
            continue

        _, model = split_model_spec(chosen_model)
        prepped = transcript.prepared_tokens(model=model)
        budget = transcript.max_context_tokens
        remaining = max(budget - prepped, 0)
        STDERR.print(f"[dim]| {format_tokens(prepped)} used, ~{format_tokens(remaining)} remaining[/dim]")
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
            DEFAULT_UNATTENDED_TIMEOUT_SECONDS,
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


def model(query: str | None = None):
    """Show or set the default model.

    :param query: Exact model ID to save, or a filter string when running in a TTY.
    """
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
