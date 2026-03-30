from __future__ import annotations

from functools import lru_cache
import io
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
from typing import Any, Callable
import tomllib

import tiktoken
from prompt_toolkit import PromptSession
from prompt_toolkit.auto_suggest import AutoSuggestFromHistory
from prompt_toolkit.completion import FuzzyCompleter, WordCompleter
from prompt_toolkit.formatted_text import ANSI
from prompt_toolkit.history import InMemoryHistory
from prompt_toolkit.output.defaults import create_output
from prompt_toolkit.shortcuts import print_formatted_text
from prompt_toolkit.validation import ValidationError, Validator
from pygments import highlight
from pygments.formatters import Terminal256Formatter
from pygments.lexers import TextLexer, get_lexer_by_name, get_lexer_for_filename
from pygments.util import ClassNotFound
from rich.console import Console as RichConsole, Group
from rich.highlighter import ReprHighlighter
from rich.padding import Padding
from rich.syntax import Syntax
from rich.text import Text

from .providers import (
    _ensure_private_dir,
    command_env,
    http_client,
    join_model_spec,
    llm_session,
    load_json,
    normalize_jsonlike,
    run_cmd,
    save_json,
    split_model_spec,
    tool_session,
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
_TOON_LINE_RE = re.compile(r"^(?P<indent>\s*)(?P<key>[A-Za-z0-9_.-]+(?:\[\d+\])?(?:\{[^}]+\})?)(?:: ?(?P<value>.*))?$")
_SEARCH_PREVIEW_LINE_RE = re.compile(r"^(?P<path>.+):(?P<line>\d+):(?P<column>\d+):(?P<text>.*)$")
_REPLACE_SKIPPED_PREVIEW_LINE_RE = re.compile(r"^(?P<path>.+): skipped \((?P<reason>.+)\)$")
_REPLACE_CHANGED_PREVIEW_LINE_RE = re.compile(r"^(?P<path>.+): (?P<count>\d+) replacement\(s\)$")
_SEARCH_HIGHLIGHT_OPEN = "⟦"
_SEARCH_HIGHLIGHT_CLOSE = "⟧"
_MULTILINE_PREVIEW_KEYS = frozenset({"stderr", "stdout", "text"})
_RICH_REPR_HIGHLIGHTER = ReprHighlighter()

def _ansi(style: str, text: str) -> str:
    return f"\x1b[{style}m{text}\x1b[0m" if text else ""


def _sanitize_terminal_text(text: str) -> str:
    return _CONTROL_TEXT_RE.sub("?", str(text).replace("\x1b", r"\x1b"))


def has_tty_stdin() -> bool:
    return sys.stdin.isatty()


def stdin_is_interactive() -> bool:
    return has_tty_stdin() and not _flag("OY_NON_INTERACTIVE", False)



def prompt_unavailable_reason() -> str | None:
    if _flag("OY_NON_INTERACTIVE", False):
        return "interactive prompting disabled by OY_NON_INTERACTIVE=1"
    if not has_tty_stdin():
        return "stdin is not a TTY"
    return None


def can_prompt() -> bool:
    return prompt_unavailable_reason() is None


def require_prompt(action: str = "prompt") -> None:
    if reason := prompt_unavailable_reason():
        raise ValueError(f"Cannot {action}: {reason}")


def _history_path(name: str = "history") -> Path:
    path = CONFIG_PATH.parent / name
    _ensure_private_dir(path.parent)
    path.touch(mode=0o600, exist_ok=True)
    path.chmod(0o600)
    return path


type Console = dict[str, bool]



def console_stream(console: Console):
    return sys.stderr if console.get("stderr", False) else sys.stdout


def console_output(console: Console):
    return create_output(stdout=console_stream(console))


def print_console(
    console: Console,
    *values: Any,
    sep: str = " ",
    end: str = "\n",
    highlight: bool | None = None,
):
    _ = highlight
    stream = console_stream(console)
    if not values:
        stream.write(end)
        stream.flush()
        return
    print_formatted_text(
        ANSI(sep.join(map(str, values))),
        output=console_output(console),
        end=end,
        flush=True,
        include_default_pygments_style=False,
    )


def _terminal_width(default: tuple[int, int] = (80, 20), minimum: int = 20) -> int:
    return max(shutil.get_terminal_size(default).columns - 1, minimum)


def rule_console(console: Console, style: str | None = None):
    line = "─" * _terminal_width()
    print_console(console, _ansi("2", line) if style == "dim" else line)


def _prompt_session(
    *,
    console: Console | None = None,
    history=None,
    completer=None,
    validator=None,
    auto_suggest=None,
    multiline: bool = False,
    enable_open_in_editor: bool = False,
) -> PromptSession:
    return PromptSession(
        history=history or InMemoryHistory(),
        completer=completer,
        validator=validator,
        auto_suggest=auto_suggest,
        multiline=multiline,
        enable_open_in_editor=enable_open_in_editor,
        output=(console_output(console) if console is not None else create_output(stdout=sys.stderr)),
        include_default_pygments_style=False,
        complete_while_typing=bool(completer),
        validate_while_typing=bool(validator),
        reserve_space_for_menu=8 if completer else 0,
        mouse_support=False,
    )


def _choice_completer(choices: list[str] | None):
    if not choices:
        return None
    return FuzzyCompleter(WordCompleter(choices, sentence=True, match_middle=True))


def _prompt_text(label: str, *, default: str | None = None, choices: list[str] | None = None) -> str:
    prompt_text = _ansi("1;36", _sanitize_terminal_text(str(label)))
    if choices:
        prompt_text += _ansi("2", f" ({'/'.join(map(str, choices))})")
    if default not in (None, ""):
        prompt_text += _ansi("2", f" [{default}]")
    return prompt_text + ": "


class _ChoiceValidator(Validator):
    def __init__(self, choices: list[str]):
        self.choices = choices

    def validate(self, document) -> None:
        value = document.text.strip()
        if value in self.choices:
            return
        raise ValidationError(
            message=f"Enter one of: {', '.join(self.choices)}.",
            cursor_position=len(document.text),
        )


def ask(
    message: str,
    *,
    console: Console | None = None,
    default: str | None = None,
    choices: list[str] | None = None,
    history=None,
    prompt_label: str | None = None,
) -> str:
    response = _prompt_session(
        console=console,
        history=history,
        completer=_choice_completer(choices),
        validator=(_ChoiceValidator(choices) if choices else None),
    ).prompt(
        ANSI(_prompt_text(prompt_label or message, default=default, choices=choices)),
        default="" if default is None else str(default),
    ).strip()
    return response if response or default is None else str(default)


def select(
    message: str,
    options: list[str],
    *,
    console: Console | None = None,
    default: str | None = None,
    allow_custom: bool = False,
    option_text: Callable[[str, int], str] | None = None,
    prompt_label: str = "Selection",
    history=None,
) -> str:
    if not options:
        raise ValueError("select requires at least one option")
    render = option_text or (lambda option, index: f"{index}. {option}")
    _print("prompt", message, err=True)
    _print(
        value="## Options\n\n" + "\n".join(render(option, index) for index, option in enumerate(options, 1)),
        err=True,
    )
    aliases = {str(index): option for index, option in enumerate(options, 1)}
    allowed = list(aliases) + options

    class _SelectValidator(Validator):
        def validate(self, document) -> None:
            value = document.text.strip()
            if not value and default not in (None, ""):
                return
            if value in aliases or value in options or (allow_custom and value):
                return
            hint = f"Enter 1-{len(options)}, type an option exactly" + (
                ", or enter custom text." if allow_custom else "."
            )
            raise ValidationError(message=hint, cursor_position=len(document.text))

    response = _prompt_session(
        console=console,
        history=history,
        completer=_choice_completer(allowed),
        validator=_SelectValidator(),
    ).prompt(
        ANSI(_prompt_text(prompt_label, default=default)),
        default="" if default is None else str(default),
    ).strip()
    value = response if response or default is None else str(default)
    if value in aliases:
        return aliases[value]
    if value in options or (allow_custom and value):
        return value
    raise ValueError(f"Invalid selection: {value}")


def prompt_session(
    *,
    console: Console | None = None,
    history=None,
    choices: list[str] | None = None,
    validator=None,
    multiline: bool = False,
    enable_open_in_editor: bool = False,
) -> PromptSession:
    return _prompt_session(
        console=console,
        history=history,
        completer=_choice_completer(choices),
        validator=validator,
        auto_suggest=AutoSuggestFromHistory(),
        multiline=multiline,
        enable_open_in_editor=enable_open_in_editor,
    )


def yes_no(
    message: str,
    *,
    console: Console | None = None,
    default: bool = False,
    history=None,
) -> bool:
    default_choice = "y" if default else "n"
    return ask(
        message,
        console=console,
        default=default_choice,
        choices=["y", "n"],
        history=history,
    ) == "y"


STDOUT, STDERR = {"stderr": False}, {"stderr": True}


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


def _rich_console_buffer(console: Console, *, width: int | None = None) -> tuple[RichConsole, io.StringIO]:
    buffer = io.StringIO()
    stream = console_stream(console)
    return (
        RichConsole(
            file=buffer,
            force_terminal=stream.isatty(),
            color_system="auto",
            width=width or _terminal_width(),
            soft_wrap=True,
            highlight=False,
            legacy_windows=False,
        ),
        buffer,
    )


def _looks_like_markdown(text: str) -> bool:
    stripped = text.lstrip()
    return bool(
        stripped.startswith(("#", "- ", "* ", "> ", "```"))
        or "\n#" in text
        or "\n- " in text
        or "\n* " in text
        or "\n```" in text
        or "=====\n" in text
    )


def _preview_language(key: str, text: str) -> str:
    base_key, _, explicit_language = key.partition(".")
    if explicit_language:
        return explicit_language
    if base_key == "text" and _looks_like_markdown(text):
        return "markdown"
    if base_key in {"stdout", "stderr"} and "$ " in text:
        return "bash"
    return "text"


def preview_language_for_path(path: str, text: str) -> str:
    try:
        lexer = get_lexer_for_filename(path, text)
    except ClassNotFound:
        return _preview_language("text", text)
    aliases = getattr(lexer, "aliases", ())
    return aliases[0] if aliases else _preview_language("text", text)


def _preview_text(value: str, *, style: str | None = None) -> Text:
    return Text(_sanitize_terminal_text(value), style=style)


def _preview_repr(value: str, *, style: str | None = None) -> Text:
    text = _preview_text(value, style=style)
    _RICH_REPR_HIGHLIGHTER.highlight(text)
    return text


def _decode_preview_string(value: str) -> str | None:
    if len(value) < 2 or not (value.startswith('"') and value.endswith('"')):
        return None
    try:
        decoded = json.loads(value)
    except json.JSONDecodeError:
        return None
    return decoded if isinstance(decoded, str) else None


def _render_preview_multiline(key: str, decoded: str, *, indent: str = "") -> Padding:
    return Padding(
        Syntax(
            _sanitize_terminal_text(decoded).rstrip("\n") or " ",
            _preview_language(key, decoded),
            theme="ansi_dark",
            word_wrap=True,
            background_color="default",
        ),
        pad=(0, 0, 0, len(indent) + 2),
    )


def _render_preview_csv(line: str) -> Text:
    indent = line[: len(line) - len(line.lstrip(" "))]
    values = line[len(indent) :].split(",")
    text = Text(indent)
    for index, value in enumerate(values):
        if index:
            text.append(",", style="dim")
        text.append_text(_preview_repr(value))
    return text


def _render_search_preview_text(text: str) -> Text:
    sanitized = _sanitize_terminal_text(text)
    if _SEARCH_HIGHLIGHT_OPEN not in sanitized:
        return Text(sanitized)
    rendered = Text()
    remaining = sanitized
    while remaining:
        start = remaining.find(_SEARCH_HIGHLIGHT_OPEN)
        if start < 0:
            rendered.append(remaining)
            break
        rendered.append(remaining[:start])
        remaining = remaining[start + len(_SEARCH_HIGHLIGHT_OPEN) :]
        end = remaining.find(_SEARCH_HIGHLIGHT_CLOSE)
        if end < 0:
            rendered.append(_SEARCH_HIGHLIGHT_OPEN + remaining)
            break
        highlighted = remaining[:end]
        if highlighted:
            rendered.append(highlighted, style="bold yellow")
        remaining = remaining[end + len(_SEARCH_HIGHLIGHT_CLOSE) :]
    return rendered


def _render_preview_line(line: str) -> list[Any]:
    if not line:
        return [Text("")]
    if line.startswith("... ["):
        return [_preview_text(line, style="dim italic")]
    if line.startswith("[!] "):
        text = Text("[!]", style="bold yellow")
        if body := line[4:]:
            text.append(" ")
            text.append_text(_preview_text(body))
        return [text]
    if line.startswith("skip: ") and " — " in line:
        path, reason = line[6:].rsplit(" — ", 1)
        text = Text()
        text.append("skip", style="yellow")
        text.append(":", style="dim")
        text.append(" ")
        text.append(_sanitize_terminal_text(path), style="cyan")
        text.append(" — ", style="dim")
        text.append_text(_preview_text(reason, style="dim"))
        return [text]
    if line.startswith("change: ") and " — " in line:
        path, detail = line[8:].rsplit(" — ", 1)
        if count_match := re.match(r"^(?P<count>\d+) replacements?$", detail):
            text = Text()
            text.append("change", style="magenta")
            text.append(":", style="dim")
            text.append(" ")
            text.append(_sanitize_terminal_text(path), style="cyan")
            text.append(" — ", style="dim")
            text.append_text(_preview_repr(count_match.group("count"), style="bold magenta"))
            plural = "s" if count_match.group("count") != "1" else ""
            text.append(f" replacement{plural}", style="dim")
            return [text]
    if match := _REPLACE_SKIPPED_PREVIEW_LINE_RE.match(line):
        text = Text()
        text.append(_sanitize_terminal_text(match.group("path")), style="cyan")
        text.append(": ", style="dim")
        text.append("skipped", style="yellow")
        text.append(" (", style="dim")
        text.append_text(_preview_text(match.group("reason"), style="dim"))
        text.append(")", style="dim")
        return [text]
    if match := _REPLACE_CHANGED_PREVIEW_LINE_RE.match(line):
        text = Text()
        text.append(_sanitize_terminal_text(match.group("path")), style="cyan")
        text.append(": ", style="dim")
        text.append_text(_preview_repr(match.group("count"), style="bold magenta"))
        text.append(" replacement(s)", style="dim")
        return [text]
    if match := _SEARCH_PREVIEW_LINE_RE.match(line):
        text = Text()
        text.append(_sanitize_terminal_text(match.group("path")), style="cyan")
        text.append(":", style="dim")
        text.append(match.group("line"), style="magenta")
        text.append(":", style="dim")
        text.append(match.group("column"), style="dim")
        text.append(":", style="dim")
        text.append_text(_render_search_preview_text(match.group("text")))
        return [text]
    if match := _TOON_LINE_RE.match(line):
        indent, key, value = match.group("indent", "key", "value")
        header = Text(indent)
        header.append(key, style="bold cyan" if not indent else "cyan")
        header.append(":", style="dim")
        if value:
            base_key, _, explicit_language = key.partition(".")
            decoded = _decode_preview_string(value) if base_key in _MULTILINE_PREVIEW_KEYS else None
            if explicit_language:
                return [header, _render_preview_multiline(key, decoded if decoded is not None else value, indent=indent)]
            if decoded and "\n" in decoded:
                return [header, _render_preview_multiline(key, decoded, indent=indent)]
            header.append(" ")
            header.append_text(_preview_repr(value))
        return [header]
    if line.startswith("  ") and "," in line:
        return [_render_preview_csv(line)]
    if line.startswith("  "):
        text = Text(line[: len(line) - len(line.lstrip(" "))])
        text.append(_sanitize_terminal_text(line.lstrip()), style="cyan")
        return [text]
    return [_preview_text(line)]


def _render_preview_text(text: str, *, console: Console = STDERR) -> str:
    rich_console, buffer = _rich_console_buffer(console)
    rich_console.print(Group(*[renderable for line in str(text).splitlines() for renderable in _render_preview_line(line)]))
    return buffer.getvalue().rstrip("\n")

def _env(name, default, t=None):
    value = os.environ.get(f"OY_{name}")
    return default if value is None else (t or type(default))(value)

MAX_BASH_CMD_BYTES = _env("MAX_BASH_CMD_BYTES", 65536)
MAX_CONTEXT_TOKENS = _env("MAX_CONTEXT_TOKENS", 131072)
DEFAULT_UNATTENDED_LIMIT_SECONDS = 3600
DEFAULT_SELF_CONSISTENCY_BEST_OF = 3
CONFIG_PATH = Path.home() / ".config" / "oy" / "config.json"

type RuntimeBudgets = dict[str, int]
type SessionContext = dict[str, Any]
type SavedModelConfig = dict[str, str | None]


def runtime_budgets(
    *,
    message_tokens: int,
    tool_output_tokens: int,
    tool_tail_tokens: int,
    default_line_limit: int,
) -> RuntimeBudgets:
    return {
        "message_tokens": message_tokens,
        "tool_output_tokens": tool_output_tokens,
        "tool_tail_tokens": tool_tail_tokens,
        "default_line_limit": default_line_limit,
    }


def session_context(
    *,
    workspace: Path,
    model: str,
    interactive: bool,
    system_prompt: str,
    system_file: Path | None = None,
    yolo: bool = False,
    best_of: int = 1,
) -> SessionContext:
    return {
        "workspace": workspace,
        "model": model,
        "interactive": interactive,
        "system_prompt": system_prompt,
        "system_file": system_file,
        "yolo": yolo,
        "best_of": best_of,
    }


def model_config(model: str | None = None, shim: str | None = None) -> SavedModelConfig:
    return {"model": model, "shim": shim}


def model_config_from_model_spec(model_spec: str) -> SavedModelConfig:
    shim, model = split_model_spec(model_spec)
    return model_config(model=model, shim=shim)


def resolved_model(config: SavedModelConfig) -> str | None:
    model = config["model"]
    shim = config["shim"]
    if not model:
        return None
    if ":" in model or not shim:
        return model
    return join_model_spec(shim, model)


def merge_model_config(
    config: SavedModelConfig, base: dict[str, Any] | None = None
) -> dict[str, Any]:
    data = dict(base or {})
    if config["model"]:
        data["model"] = config["model"]
    else:
        data.pop("model", None)
    if config["shim"]:
        data["shim"] = config["shim"]
    else:
        data.pop("shim", None)
    return data

def _clamp_int(value: int, lower: int, upper: int) -> int:
    return max(lower, min(value, upper))

def _derive_runtime_budgets(context_tokens: int) -> RuntimeBudgets:
    tool_output_tokens = _clamp_int(context_tokens // 24, 2048, 8192)
    return runtime_budgets(
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


def active_tool_registry(interactive: bool):
    from .tools import TOOL_REGISTRY, select_tools

    return TOOL_REGISTRY if interactive else select_tools(exclude={"ask"})


def ask_system_prompt(system_prompt: str) -> str:
    return system_prompt + _ASK_SYSTEM_SUFFIX


def read_only_tool_registry():
    from .tools import select_tools

    return select_tools(include=_READ_ONLY_TOOLS)

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
    data = normalize_jsonlike(msg)
    return data if isinstance(data, dict) else {"message": data}

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
    print_console(console, _render_markdownish(_fmt(kind, value, extra))) if value else print_console(console)


def _note(label: str, *, tag: str | None = None) -> None:
    body = _sanitize_terminal_text(label)
    tag_text = _sanitize_terminal_text(tag) if tag else "note"
    print_console(STDERR, _ansi("2", f"[{tag_text}] {body}".rstrip()))


def _warn(message: str) -> None:
    _print("warning", message, err=True)


def _error(message: str) -> None:
    _print("error", message, err=True)


def fail(message, code=1):
    _print("error", str(message).strip(), err=True)
    return code

def abort(message, code=1):
    raise SystemExit(fail(message, code))

def clip_tokens(text, limit=None, tail=0):
    limit = BUDGETS["tool_output_tokens"] if limit is None else limit
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


def parse_duration_seconds(value: str, *, name: str = "duration") -> int:
    text = value.strip().lower()
    if not text:
        abort(f"Invalid {name}={value!r}. Use a positive duration like 3h, 90m, or 3600s.")
    if text.isdigit():
        seconds = int(text)
    else:
        match = re.fullmatch(r"([0-9]+)([hms])", text)
        if not match:
            abort(f"Invalid {name}={value!r}. Use a positive duration like 3h, 90m, or 3600s.")
        amount = int(match.group(1))
        unit = match.group(2)
        seconds = amount * {"h": 3600, "m": 60, "s": 1}[unit]
    if seconds <= 0:
        abort(f"Invalid {name}={value!r}. Duration must be positive.")
    return seconds


def _duration_env_seconds(*names: str, default: int) -> int:
    values = [(name, os.environ.get(name)) for name in names]
    configured = [(name, value) for name, value in values if value is not None]
    if not configured:
        return default
    if len(configured) > 1:
        rendered = ", ".join(f"{name}={value!r}" for name, value in configured)
        abort(f"Conflicting duration env vars: {rendered}. Set only one.")
    name, value = configured[0]
    return parse_duration_seconds(value, name=name)


def unattended_limit_seconds(default: int = DEFAULT_UNATTENDED_LIMIT_SECONDS) -> int:
    return _duration_env_seconds(
        "OY_UNATTENDED_LIMIT",
        default=default,
    )


def ralph_limit_seconds(default: int = 3 * 3600) -> int:
    return _duration_env_seconds("OY_RALPH_LIMIT", default=default)

_MAX_PREVIEW_LINES = 20
_MAX_LINE_WIDTH = 512

def _truncate_long_lines(text: str, limit: int = _MAX_LINE_WIDTH) -> str:
    lines = text.split("\n")
    changed = False
    for index, line in enumerate(lines):
        if len(line) > limit:
            lines[index] = line[:limit] + f"... [{len(line) - limit} chars truncated]"
            changed = True
    return "\n".join(lines) if changed else text


def _summarize_preview_lines(text: str, *, max_lines: int = _MAX_PREVIEW_LINES) -> str:
    text = _truncate_long_lines(text)
    lines = text.splitlines()
    if len(lines) <= max_lines:
        return text
    head_count = max(max_lines // 2, 2)
    tail_count = max(max_lines - head_count, 2)
    keep: list[int] = []
    keep.extend(range(head_count))
    for index in range(head_count, len(lines) - tail_count):
        if _BASH_IMPORTANT_LINE_RE.search(lines[index]):
            keep.append(index)
    keep.extend(range(len(lines) - tail_count, len(lines)))
    keep = sorted(set(keep))
    if len(keep) > max_lines:
        important_mid = [index for index in keep if head_count <= index < len(lines) - tail_count]
        budget = max_lines - head_count - tail_count
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
    return "\n".join(selected + [f"... [{len(lines)} lines total]"])


def show(text):
    if not text:
        return
    print_console(STDERR, _render_preview_text(_summarize_preview_lines(text), console=STDERR))

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


def load_model_config() -> SavedModelConfig:
    data = _load_cfg()
    if not isinstance(data, dict):
        return model_config()
    model = data.get("model")
    shim = data.get("shim")
    return model_config(
        model=model if isinstance(model, str) else None,
        shim=shim if isinstance(shim, str) else None,
    )


def save_model_config(model_spec: str) -> SavedModelConfig:
    config = model_config_from_model_spec(model_spec)
    _save_cfg(merge_model_config(config, _load_cfg()))
    return config


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
    if not can_prompt():
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
        response = ask("Model number or ID", console=STDERR).strip()
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
        _warn(f"No match for {_fmt('inline', response)}. Try again.")
    save_model_config(chosen)
    _print(value=f"## Default Model Saved\n\n- selected: {_fmt('inline', chosen)}", err=True)
    return chosen

def _shim_name(configured=None):
    if configured:
        return configured
    if value := os.environ.get("OY_SHIM"):
        return value
    return load_model_config()["shim"]


def _model(configured=None):
    if configured:
        return configured
    if value := os.environ.get("OY_MODEL"):
        return join_model_spec(shim_name, value) if ":" not in value and (shim_name := _shim_name()) else value
    if value := resolved_model(load_model_config()):
        return value
    return _pick_model()

def yolo_enabled(default: bool = False) -> bool:
    return _flag("OY_YOLO", default)


def default_best_of_for_model(model_spec: str | None = None) -> int:
    _, model = split_model_spec(model_spec or _model())
    lowered = model.lower()
    if "glm-5" in lowered or "kimi-k2.5" in lowered or "kimi-k2" in lowered:
        return DEFAULT_SELF_CONSISTENCY_BEST_OF
    return 1


def self_consistency_best_of(default: int | None = None, *, model_spec: str | None = None) -> int:
    fallback = default_best_of_for_model(model_spec) if default is None else default
    value = os.environ.get("OY_BEST_OF")
    if value is None or not value.strip():
        return fallback
    try:
        parsed = int(value.strip())
    except ValueError:
        abort(f"Invalid OY_BEST_OF={value.strip()}. Use a positive integer.")
    if parsed <= 0:
        abort(f"Invalid OY_BEST_OF={parsed}. Use a positive integer.")
    return parsed


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
    from . import agent as ag

    ag.note_progress(state)
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

def truncate_str_to_tokens(text: str, max_tokens: int = BUDGETS["message_tokens"]) -> str:
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
            "No shims are configured. Set OPENAI_API_KEY, sign in with Codex CLI, authenticate GitHub CLI, run `opencode auth`, or configure Bedrock Mantle via AWS CLI credentials / SSO and `AWS_REGION`."
        )
    all_models: list[str] = []
    for shim in shims:
        _print("status", f"Loading models from {_fmt('inline', shim)}.", err=True)
        try:
            all_models.extend(
                list_models_for_shim(shim, cwd=Path.cwd(), ignore_errors=False)
            )
        except Exception as exc:
            message = str(exc).strip().splitlines()[0] if str(exc).strip() else type(exc).__name__
            _warn(f"Could not load models from {_fmt('inline', shim)}: {message}")
    return all_models

_tokenizer: tiktoken.Encoding | None = None

__all__ = [
    "AUDIT_SYSTEM_PROMPT",
    "BASE_SYSTEM_PROMPT",
    "BUDGETS",
    "INTERACTIVE_SYSTEM_PROMPT",
    "NONINTERACTIVE_SYSTEM_PROMPT",
    "CONFIG_PATH",
    "DEFAULT_SELF_CONSISTENCY_BEST_OF",
    "DEFAULT_UNATTENDED_LIMIT_SECONDS",
    "MAX_BASH_CMD_BYTES",
    "MAX_CONTEXT_TOKENS",
    "STDERR",
    "STDOUT",
    "RuntimeBudgets",
    "SavedModelConfig",
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
    "_duration_env_seconds",
    "_ensure_private_dir",
    "_flag",
    "_fmt",
    "_format_duration",
    "_init_debug_log",
    "parse_duration_seconds",
    "ralph_limit_seconds",
    "unattended_limit_seconds",
    "_load_cfg",
    "_model",
    "_msg_to_dict",
    "_note",
    "_pick_model",
    "_print",
    "_rel",
    "_save_cfg",
    "_shim_name",
    "_sys_file",
    "_truncate_long_lines",
    "active_system_prompt",
    "ask",
    "base_system_prompt",
    "active_tool_registry",
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
    "llm_session",
    "tool_session",
    "join_model_spec",
    "list_all_model_ids",
    "load_json",
    "load_model_config",
    "logging",
    "load_session_text",
    "noninteractive_system_prompt",
    "note_tool",
    "os",
    "preview",
    "preview_language_for_path",
    "prompt_session",
    "re",
    "read_only_tool_registry",
    "render_model_list",
    "session_text",
    "select",
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
    "save_model_config",
    "show",
    "split_model_spec",
    "self_consistency_best_of",
    "default_best_of_for_model",
    "sys",
    "time",
    "tool_description",
    "truncate_str_to_tokens",
    "which",
    "yes_no",
    "yolo_enabled",
    "Path",
]
