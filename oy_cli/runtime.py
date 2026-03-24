from __future__ import annotations

from dataclasses import dataclass
from functools import lru_cache
import json
import logging
import os
import re
import shutil
import sys
import time
from importlib.metadata import version as _meta_version
from importlib.resources import files
from pathlib import Path
from typing import Any
import tomllib

import msgspec
import tiktoken
from prompt_toolkit.formatted_text import ANSI
from prompt_toolkit.output.defaults import create_output
from prompt_toolkit.shortcuts import print_formatted_text, prompt as prompt_input
from pygments import highlight
from pygments.formatters import Terminal256Formatter
from pygments.lexers import TextLexer, get_lexer_by_name
from pygments.util import ClassNotFound

from .providers import (
    command_env,
    http_client,
    join_model_spec,
    load_json,
    run_cmd,
    save_json,
    split_model_spec,
    which,
)
from .providers import (
    detect_available_shims,
    ensure_api_env as _shim_ensure_api_env,
    get_client as _shim_get_client,
    list_models_for_shim,
    require_api_env as _shim_require_api_env,
    resolve_shim as _shim_resolve_shim,
    validate_shim,
)

__version__ = _meta_version("oy-cli")

_TRUTHY_VALUES = {"1", "true", "yes", "on"}
_FALSY_VALUES = {"0", "false", "no", "off"}

# Show head + tail, with important lines (errors/warnings) prioritised.
_BASH_IMPORTANT_LINE_RE = re.compile(
    r"(?i)(error|warn(?:ing)?|fail(?:ed|ure)?|exception|traceback|fatal|denied|not found|timed out)"
)

_FENCE_RE = re.compile(r"^```([a-zA-Z0-9_+.-]*)\s*$")
_INLINE_TOKEN_RE = re.compile(r"`[^`\n]+`|\*\*[^*\n]+?\*\*|\*[^*\n]+?\*")
_CONTROL_TEXT_RE = re.compile(r"[\x00-\x08\x0b-\x1f\x7f]")

def _ansi(style: str, text: str) -> str:
    return f"\x1b[{style}m{text}\x1b[0m" if text else ""


def _sanitize_terminal_text(text: str) -> str:
    return _CONTROL_TEXT_RE.sub("?", str(text).replace("\x1b", r"\x1b"))


class Console:
    def __init__(self, *, stderr: bool = False):
        self._stderr = stderr

    @property
    def stream(self):
        return sys.stderr if self._stderr else sys.stdout

    @property
    def output(self):
        return create_output(stdout=self.stream)

    def print(self, *values: Any, sep: str = " ", end: str = "\n", highlight: bool | None = None):
        _ = highlight
        if not values:
            self.stream.write(end)
            self.stream.flush()
            return
        print_formatted_text(
            ANSI(sep.join(map(str, values))),
            output=self.output,
            end=end,
            flush=True,
            include_default_pygments_style=False,
        )

    def rule(self, style: str | None = None):
        width = max(shutil.get_terminal_size((80, 20)).columns - 1, 20)
        line = "─" * width
        self.print(_ansi("2", line) if style == "dim" else line)


class Prompt:
    @staticmethod
    def ask(
        message: str,
        *,
        console: Console | None = None,
        default: str | None = None,
        choices: list[str] | None = None,
    ) -> str:
        prompt_text = _ansi("1;36", _sanitize_terminal_text(str(message)))
        if choices:
            prompt_text += _ansi("2", f" ({'/'.join(map(str, choices))})")
        if default not in (None, ""):
            prompt_text += _ansi("2", f" [{default}]")
        prompt_text += ": "
        while True:
            response = prompt_input(
                ANSI(prompt_text),
                default="" if default is None else str(default),
                output=(console.output if isinstance(console, Console) else create_output(stdout=sys.stderr)),
                include_default_pygments_style=False,
            ).strip()
            if not choices or response in choices:
                return response
            _print(
                "warning",
                f"Enter one of: {', '.join(_fmt('inline', choice) for choice in choices)}.",
                err=True,
            )


STDOUT, STDERR = Console(), Console(stderr=True)


@lru_cache(maxsize=1)
def _terminal_formatter() -> Terminal256Formatter:
    return Terminal256Formatter(style="native")


@lru_cache(maxsize=None)
def _code_lexer(language: str):
    aliases = {
        "plain": "text",
        "plaintext": "text",
        "console": "bash",
        "shell": "bash",
        "sh": "bash",
    }
    name = aliases.get(
        (language or "text").strip().lower(),
        (language or "text").strip().lower() or "text",
    )
    try:
        return get_lexer_by_name(name)
    except ClassNotFound:
        return TextLexer()


def _highlight_code(text: str, language: str = "text") -> str:
    return highlight(
        _sanitize_terminal_text(text).rstrip("\n"),
        _code_lexer(language),
        _terminal_formatter(),
    ).rstrip("\n")


def _apply_inline_styles(text: str) -> str:
    text = _sanitize_terminal_text(text)
    parts: list[str] = []
    last = 0
    for match in _INLINE_TOKEN_RE.finditer(text):
        start, end = match.span()
        parts.append(text[last:start])
        token = match.group(0)
        if token.startswith("`"):
            parts.append(_ansi("38;5;81", token[1:-1].replace(r"\`", "`")))
        elif token.startswith("**"):
            inner = token[2:-2]
            style = "1;33" if inner.rstrip(":").lower() == "warning" else "1"
            parts.append(_ansi(style, inner))
        else:
            parts.append(_ansi("2", token[1:-1]))
        last = end
    parts.append(text[last:])
    return "".join(parts)


def _render_text_line(line: str) -> str:
    if not line:
        return ""
    if line.startswith("### "):
        return _ansi("1;35", _sanitize_terminal_text(line[4:].strip()))
    if line.startswith("## "):
        heading = _sanitize_terminal_text(line[3:].strip())
        return _ansi("1;31" if heading.lower() == "error" else "1;36", heading)
    if line.startswith("# "):
        return _ansi("1;34", _sanitize_terminal_text(line[2:].strip()))
    if line.startswith("[warning] "):
        return _ansi("1;33", "[warning]") + " " + _apply_inline_styles(line[10:])
    if line.startswith("[status] "):
        return _ansi("2", "[status]") + " " + _apply_inline_styles(line[9:])
    if line.startswith("[note] "):
        return _ansi("2", "[note]") + " " + _apply_inline_styles(line[7:])
    if line.startswith("[tool] "):
        return _ansi("2", "[tool]") + " " + _apply_inline_styles(line[7:])
    if line.startswith("[wait] "):
        return _ansi("2", "[wait]") + " " + _apply_inline_styles(line[7:])
    if line.startswith("[!] "): 
        return _ansi("1;33", "[!]") + " " + _apply_inline_styles(line[4:])
    if line.startswith("... ["):
        return _ansi("2", _sanitize_terminal_text(line))
    return _apply_inline_styles(line)


def _render_markdownish(text: str) -> str:
    rendered: list[str] = []
    code_lines: list[str] = []
    code_language = "text"
    in_code_block = False
    for line in str(text).splitlines():
        fence = _FENCE_RE.match(line)
        if in_code_block:
            if fence:
                rendered.append(_highlight_code("\n".join(code_lines), code_language))
                code_lines.clear()
                code_language = "text"
                in_code_block = False
            else:
                code_lines.append(line)
            continue
        if fence:
            code_language = fence.group(1) or "text"
            in_code_block = True
            code_lines.clear()
            continue
        rendered.append(_render_text_line(line))
    if in_code_block:
        rendered.append(_highlight_code("\n".join(code_lines), code_language))
    return "\n".join(rendered)

def _env(name, default, t=None):
    value = os.environ.get(f"OY_{name}")
    return default if value is None else (t or type(default))(value)

MAX_BASH_CMD_BYTES = _env("MAX_BASH_CMD_BYTES", 65536)
MAX_CONTEXT_TOKENS = _env("MAX_CONTEXT_TOKENS", 131072)
DEFAULT_UNATTENDED_TIMEOUT_SECONDS = _env("UNATTENDED_TIMEOUT_SECONDS", 3600)
CONFIG_PATH = Path.home() / ".config" / "oy" / "config.json"

@dataclass(frozen=True, slots=True)
class RuntimeBudgets:
    message_tokens: int
    tool_output_tokens: int
    tool_tail_tokens: int
    default_line_limit: int

@dataclass(frozen=True, slots=True)
class SessionContext:
    workspace: Path
    model: str
    interactive: bool
    system_prompt: str
    system_file: Path | None = None

def _clamp_int(value: int, lower: int, upper: int) -> int:
    return max(lower, min(value, upper))

def _derive_runtime_budgets(context_tokens: int) -> RuntimeBudgets:
    tool_output_tokens = _clamp_int(context_tokens // 24, 2048, 8192)
    return RuntimeBudgets(
        message_tokens=_clamp_int(context_tokens // 16, tool_output_tokens, 12288),
        tool_output_tokens=tool_output_tokens,
        tool_tail_tokens=_clamp_int(tool_output_tokens // 5, 512, 2048),
        default_line_limit=_clamp_int(tool_output_tokens // 6, 200, 1200),
    )

BUDGETS = _derive_runtime_budgets(MAX_CONTEXT_TOKENS)

@lru_cache(maxsize=1)
def load_session_text() -> dict[str, Any]:
    raw = files("oy_cli").joinpath("session_text.toml").read_text(encoding="utf-8")
    data = tomllib.loads(raw)
    if not isinstance(data, dict):
        raise RuntimeError("session_text.toml must decode to a table")
    return data


def session_text(*keys: str, **values: Any) -> str:
    node: Any = load_session_text()
    for key in keys:
        if not isinstance(node, dict) or key not in node:
            joined = ".".join((*keys,))
            raise KeyError(f"Missing session text key: {joined}")
        node = node[key]
    if not isinstance(node, str):
        joined = ".".join(keys)
        raise TypeError(f"Session text key must point to a string: {joined}")
    return node.format(**values) if values else node


def tool_description(name: str) -> str:
    return session_text("tools", name, "description")


def base_system_prompt() -> str:
    return session_text("system", "base").strip()


def interactive_system_prompt() -> str:
    return session_text("system", "interactive_suffix").strip()


def noninteractive_system_prompt() -> str:
    return session_text("system", "noninteractive_suffix").strip()


def audit_system_prompt() -> str:
    return session_text("system", "audit").strip()


BASE_SYSTEM_PROMPT = base_system_prompt()
INTERACTIVE_SYSTEM_PROMPT = interactive_system_prompt()
NONINTERACTIVE_SYSTEM_PROMPT = noninteractive_system_prompt()
AUDIT_SYSTEM_PROMPT = audit_system_prompt()
_ASK_SYSTEM_SUFFIX = "\n" + session_text("system", "ask_suffix").strip() + "\n"
_READ_ONLY_TOOLS = {"list", "read", "search", "sloc", "webfetch"}


def active_system_prompt(interactive: bool) -> str:
    suffix = INTERACTIVE_SYSTEM_PROMPT if interactive else NONINTERACTIVE_SYSTEM_PROMPT
    return BASE_SYSTEM_PROMPT + "\n" + suffix + "\n"


def active_tool_specs(interactive: bool):
    from .tools import TOOL_REGISTRY, ToolRegistry

    return TOOL_REGISTRY if interactive else ToolRegistry.select(exclude={"ask"})


def ask_system_prompt(system_prompt: str) -> str:
    return system_prompt + _ASK_SYSTEM_SUFFIX


def read_only_tool_specs():
    from .tools import ToolRegistry

    return ToolRegistry.select(include=_READ_ONLY_TOOLS)

def _ensure_private_dir(path: Path) -> Path:
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.chmod(0o700)
    return path

def _init_debug_log() -> tuple[logging.Logger | None, str | None]:
    if os.environ.get("OY_DEBUG", "").strip().lower() not in _TRUTHY_VALUES:
        return None, None
    debug_dir = _ensure_private_dir(CONFIG_PATH.parent)
    log_path = debug_dir / "debug.jsonl"
    fd = os.open(str(log_path), os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    os.close(fd)
    log_path.chmod(0o600)
    logger = logging.getLogger("oy.debug")
    logger.setLevel(logging.DEBUG)
    logger.propagate = False
    if not logger.handlers:
        handler = logging.FileHandler(str(log_path), encoding="utf-8")
        handler.setFormatter(logging.Formatter("%(message)s"))
        logger.addHandler(handler)
    return logger, str(log_path)

_debug_logger, _debug_log_path = _init_debug_log()

def _msg_to_dict(msg) -> dict[str, Any]:
    return msgspec.to_builtins(msg)

def _debug_log(event: str, **data: Any) -> None:
    if _debug_logger is None:
        return
    _debug_logger.debug(
        json.dumps({"ts": time.time(), "event": event, **data}, default=str, ensure_ascii=False)
    )

def _fmt(kind, value="", extra=None):
    text = str(value)
    if kind == "bash":
        out, rc, err = extra
        return "\n".join(
            [
                "```bash",
                f"$ {value}",
                (out or "").rstrip(),
                *([f"# exit {rc}"] if rc else []),
                *( ["# stderr:", err.rstrip()] if err else []),
                "```",
            ]
        )
    return {
        "md": text,
        "block": f"```{extra or 'text'}\n{text.rstrip()}\n```",
        "inline": f"`{text.replace('`', '\\`')}`",
        "status": f"[status] {text}",
        "warning": f"[warning] {text}",
        "prompt": f"### {text}",
        "error": f"## Error\n\n{text if chr(10) in text else f'- {text}'}",
    }[kind]

def _print(kind="md", value="", *, err=False, extra=None):
    console = STDERR if err else STDOUT
    console.print(_render_markdownish(_fmt(kind, value, extra))) if value else console.print()


def _note(label: str, *, tag: str | None = None) -> None:
    body = _sanitize_terminal_text(label)
    tag_text = _sanitize_terminal_text(tag) if tag else "note"
    STDERR.print(_ansi("2", f"[{tag_text}] {body}".rstrip()))

def fail(message, code=1):
    _print("error", str(message).strip(), err=True)
    return code

def abort(message, code=1):
    raise SystemExit(fail(message, code))

def clip_tokens(text, limit=None, tail=0):
    limit = BUDGETS.tool_output_tokens if limit is None else limit
    token_ids = encode_tokens(text)
    count = len(token_ids)
    if count <= limit:
        return text
    omitted = count - limit
    if 0 < tail < limit:
        head = max(limit - tail, 1)
        return (
            f"{decode_tokens(token_ids[:head])}\n"
            f"... [{omitted} tokens omitted; showing first {head} and last {tail}]\n"
            f"{decode_tokens(token_ids[-tail:])}"
        )
    return f"{decode_tokens(token_ids[:limit])}\n... [{omitted} tokens omitted after {limit}]"

def preview(value, limit=72):
    text = " ".join(
        (value if isinstance(value, str) else json.dumps(value, separators=(",", ":"))).split()
    )
    return text if len(text) <= limit else text[: limit - 3] + "..."

def _format_duration(seconds: int) -> str:
    if seconds % 3600 == 0:
        return f"{seconds // 3600}h"
    if seconds % 60 == 0:
        return f"{seconds // 60}m"
    return f"{seconds}s"

def _show_and_clip(text, limit=None, tail=0):
    limit = BUDGETS.tool_output_tokens if limit is None else limit
    show(text)
    return clip_tokens(text, limit=limit, tail=tail)

_MAX_SHOW_LINES = 10
_MAX_LINE_WIDTH = 512

def _truncate_long_lines(text: str, limit: int = _MAX_LINE_WIDTH) -> str:
    lines = text.split("\n")
    changed = False
    for index, line in enumerate(lines):
        if len(line) > limit:
            lines[index] = line[:limit] + f"... [{len(line) - limit} chars truncated]"
            changed = True
    return "\n".join(lines) if changed else text

def _wrap_code_block(text: str) -> str:
    if "```" in text:
        return text
    return f"```text\n{text.rstrip()}\n```"


def show(text):
    if not text:
        return
    text = _truncate_long_lines(text)
    lines = text.splitlines()
    if len(lines) <= _MAX_SHOW_LINES:
        STDERR.print(_render_markdownish(_wrap_code_block(text)))
        return
    head_count = max(_MAX_SHOW_LINES // 2, 2)
    tail_count = max(_MAX_SHOW_LINES - head_count, 2)
    keep: list[int] = []
    keep.extend(range(head_count))
    for index in range(head_count, len(lines) - tail_count):
        if _BASH_IMPORTANT_LINE_RE.search(lines[index]):
            keep.append(index)
    keep.extend(range(len(lines) - tail_count, len(lines)))
    keep = sorted(set(keep))
    if len(keep) > _MAX_SHOW_LINES:
        important_mid = [index for index in keep if head_count <= index < len(lines) - tail_count]
        budget = _MAX_SHOW_LINES - head_count - tail_count
        important_mid = important_mid[:budget]
        keep = sorted(
            set(list(range(head_count)) + important_mid + list(range(len(lines) - tail_count, len(lines))))
        )
    selected: list[str] = []
    last = -1
    for index in keep:
        if index > last + 1:
            selected.append(f"... [{index - last - 1} lines hidden]")
        selected.append(lines[index])
        last = index
    wrapped = _wrap_code_block("\n".join(selected))
    wrapped += f"\n\n*[{len(lines)} lines total]*"
    STDERR.print(_render_markdownish(wrapped))

def _rel(root, path):
    try:
        return path.relative_to(root).as_posix() or "."
    except ValueError:
        return "<outside workspace>"

def _cfg_path():
    return Path(os.environ.get("OY_CONFIG", str(CONFIG_PATH))).expanduser()

def _load_cfg():
    data = load_json(_cfg_path(), {})
    return data if isinstance(data, dict) else {}

def _save_cfg(data):
    save_json(_cfg_path(), data)

def render_model_list(items, *, title, query=None, current=None, err=False, limit=None):
    shown = list(items if limit is None else items[:limit])
    lines = [title]
    if current:
        lines += ["", f"- current model: {_fmt('inline', current)}"]
    if query:
        lines += ["", f"- filter: {_fmt('inline', query)}"]
    lines += [""] + ([f"{i}. {_fmt('inline', item)}" for i, item in enumerate(shown, 1)] or ["- no matching models"])
    if len(items) > len(shown):
        lines += ["", f"- showing {len(shown)} of {len(items)} matches"]
    _print(value="\n".join(lines), err=err)

def _pick_model():
    if not sys.stdin.isatty() or _flag("OY_NON_INTERACTIVE", False):
        abort(
            "No model configured.\n\n"
            "Pick one interactively:\n"
            "  oy model\n\n"
            "Or set directly:\n"
            "  OY_MODEL=openai:gpt-4o oy ...\n"
        )
    try:
        available = list_all_model_ids()
    except Exception:
        abort(
            "No model configured and could not list available models.\n\n"
            "Set OY_MODEL or run `oy model` to pick one."
        )
    if not available:
        abort(
            "No model configured and no models found from available shims.\n\n"
            "Set OY_MODEL or run `oy model` to pick one."
        )
    _print(
        value="## No model configured\n\n"
        "Pick a default model to save (recommended: a `glm-5` or `kimi-k2.5` variant if available).\n",
        err=True,
    )
    render_model_list(available, title="## Available Models", err=True)
    while True:
        response = Prompt.ask("Model number or ID", console=STDERR).strip()
        if response.isdigit() and 1 <= int(response) <= len(available):
            chosen = available[int(response) - 1]
            break
        if response in available:
            chosen = response
            break
        matches = [model for model in available if response.lower() in model.lower()]
        if len(matches) == 1:
            chosen = matches[0]
            break
        if matches:
            render_model_list(matches, title="## Matching Models", query=response, err=True)
            continue
        _print("warning", f"No match for {_fmt('inline', response)}. Try again.", err=True)
    shim_name, bare_model = split_model_spec(chosen)
    cfg = {**_load_cfg(), "model": bare_model}
    if shim_name:
        cfg["shim"] = shim_name
    else:
        cfg.pop("shim", None)
    _save_cfg(cfg)
    _print(value=f"## Default Model Saved\n\n- selected: {_fmt('inline', chosen)}", err=True)
    return chosen

def _env_or_cfg(configured, env_name, key, default=None):
    return configured or os.environ.get(env_name) or _load_cfg().get(key, default)

def _shim_name(configured=None):
    return _env_or_cfg(configured, "OY_SHIM", "shim")

def _model(configured=None):
    if value := _env_or_cfg(configured, "OY_MODEL", "model"):
        return join_model_spec(shim_name, value) if isinstance(value, str) and ":" not in value and (shim_name := _shim_name()) else value
    return _pick_model()

def _flag(name, default=False):
    value = os.environ.get(name)
    if not value or not value.strip():
        return default
    value = value.strip().lower()
    if value in _TRUTHY_VALUES:
        return True
    if value in _FALSY_VALUES:
        return False
    abort(f"Invalid {name}={value}. Use 1/0, true/false, yes/no, on/off.")

def _sys_file():
    return Path(value).expanduser() if (value := os.environ.get("OY_SYSTEM_FILE")) else None

def _wrap_runtime_error(fn, *args):
    try:
        return fn(*args)
    except RuntimeError as exc:
        abort(str(exc))

def resolve_active_shim(spec=None):
    return _wrap_runtime_error(validate_shim, _shim_resolve_shim(spec, _shim_name()))

def ensure_api_env(cwd=None):
    return _shim_ensure_api_env(_model(), _shim_name(), cwd)[0]

def require_api_env(cwd=None):
    _wrap_runtime_error(_shim_require_api_env, _model(), _shim_name(), cwd)

def require_command_env(cwd=None):
    return dict(_wrap_runtime_error(command_env, cwd))

def get_client(spec=None):
    require_api_env(Path.cwd())
    model_spec = spec or _model()
    return _shim_get_client(resolve_active_shim(model_spec), cwd=Path.cwd())

def resolve_path(root, path):
    resolved = (root / path).resolve()
    if resolved == root or root in resolved.parents:
        return resolved
    raise ValueError(f"Path traversal denied: '{path}'")

def note_tool(state, name, *, _defaults=None, _suffix="", **details):
    state.note_progress()
    defaults = _defaults or {}
    parts = [
        key.replace("_", "-")
        if value is True
        else f"{key.replace('_', '-')}: {_sanitize_terminal_text(value if isinstance(value, str) else preview(value))}"
        for key, value in details.items()
        if value not in (None, "", False) and value != defaults.get(key)
    ]
    label = _sanitize_terminal_text(name)
    if parts:
        label += f" {', '.join(parts)}"
    if _suffix:
        label += f"  {_sanitize_terminal_text(_suffix)}"
    _note(label, tag="tool")

def get_tokenizer() -> tiktoken.Encoding:
    global _tokenizer
    if _tokenizer is None:
        _tokenizer = tiktoken.get_encoding("cl100k_base")
    return _tokenizer

def encode_tokens(text: str) -> list[int]:
    return get_tokenizer().encode(text, disallowed_special=())

def decode_tokens(token_ids: list[int]) -> str:
    return get_tokenizer().decode(token_ids)

def count_tokens(text: str) -> int:
    return len(encode_tokens(text))

def truncate_str_to_tokens(text: str, max_tokens: int = BUDGETS.message_tokens) -> str:
    token_ids = encode_tokens(text)
    if len(token_ids) <= max_tokens:
        return text
    kept = decode_tokens(token_ids[:max_tokens])
    omitted_chars = len(text) - len(kept)
    omitted_lines = text[len(kept) :].count("\n")
    line_word = "line" if omitted_lines == 1 else "lines"
    kept = kept.rstrip()
    return (
        f"{kept}\n"
        f"... [truncated: {omitted_lines} {line_word}, {omitted_chars} chars omitted to fit {max_tokens}-token limit]"
    )

def format_tokens(count: int) -> str:
    if count < 1000:
        return f"{count} tokens"
    return f"{count / 1000:.1f}k tokens"

def list_all_model_ids() -> list[str]:
    shims = detect_available_shims()
    if not shims:
        abort(
            "No shims are configured. Set OPENAI_API_KEY, sign in with Codex CLI, authenticate GitHub CLI, run `opencode auth`, or configure AWS CLI for Bedrock Mantle."
        )
    all_models: list[str] = []
    for shim in shims:
        _print("status", f"Loading models from {_fmt('inline', shim)}.", err=True)
        all_models.extend(list_models_for_shim(shim, cwd=Path.cwd()))
    return all_models

_tokenizer: tiktoken.Encoding | None = None

__all__ = [
    "AUDIT_SYSTEM_PROMPT",
    "BASE_SYSTEM_PROMPT",
    "BUDGETS",
    "INTERACTIVE_SYSTEM_PROMPT",
    "NONINTERACTIVE_SYSTEM_PROMPT",
    "CONFIG_PATH",
    "DEFAULT_UNATTENDED_TIMEOUT_SECONDS",
    "MAX_BASH_CMD_BYTES",
    "MAX_CONTEXT_TOKENS",
    "STDERR",
    "STDOUT",
    "RuntimeBudgets",
    "SessionContext",
    "__version__",
    "_ASK_SYSTEM_SUFFIX",
    "_BASH_IMPORTANT_LINE_RE",
    "_READ_ONLY_TOOLS",
    "_TRUTHY_VALUES",
    "_FALSY_VALUES",
    "_cfg_path",
    "_debug_log",
    "_debug_log_path",
    "_debug_logger",
    "_derive_runtime_budgets",
    "_ensure_private_dir",
    "_flag",
    "_fmt",
    "_format_duration",
    "_init_debug_log",
    "_load_cfg",
    "_model",
    "_msg_to_dict",
    "_note",
    "_pick_model",
    "_print",
    "_rel",
    "_save_cfg",
    "_show_and_clip",
    "_shim_name",
    "_sys_file",
    "_truncate_long_lines",
    "_wrap_code_block",
    "active_system_prompt",
    "base_system_prompt",
    "active_tool_specs",
    "ask_system_prompt",
    "audit_system_prompt",
    "abort",
    "clip_tokens",
    "command_env",
    "count_tokens",
    "decode_tokens",
    "encode_tokens",
    "ensure_api_env",
    "fail",
    "format_tokens",
    "get_client",
    "get_tokenizer",
    "interactive_system_prompt",
    "http_client",
    "join_model_spec",
    "list_all_model_ids",
    "load_json",
    "logging",
    "load_session_text",
    "noninteractive_system_prompt",
    "note_tool",
    "os",
    "preview",
    "Prompt",
    "re",
    "read_only_tool_specs",
    "render_model_list",
    "session_text",
    "require_api_env",
    "require_command_env",
    "resolve_active_shim",
    "resolve_path",
    "run_cmd",
    "_shim_ensure_api_env",
    "_shim_get_client",
    "_shim_require_api_env",
    "_shim_resolve_shim",
    "save_json",
    "show",
    "split_model_spec",
    "sys",
    "time",
    "tool_description",
    "truncate_str_to_tokens",
    "which",
    "Path",
]
