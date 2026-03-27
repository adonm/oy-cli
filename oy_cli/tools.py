from __future__ import annotations

import bz2
from collections import Counter
import concurrent.futures
import gzip
import ipaddress
import json
import lzma
import os
import socket
import tarfile
import zipfile
from functools import partial
import inspect
from pathlib import Path
import types
from typing import Any, BinaryIO, Callable, Iterable, Literal, NotRequired, Required, TypedDict, Union, get_args, get_origin, get_type_hints, is_typeddict
from urllib3.util import parse_url

import pathspec
from pygount import DuplicatePool, ProjectSummary, SourceAnalysis
import regex
import zstandard

from markdownify import markdownify

from . import runtime as rt
from .providers import (
    DEFAULT_WEBFETCH_TIMEOUT_SECONDS,
    HTTPError,
    ResponseAdapter,
    ToolResult,
    TransportError,
    adapt_response,
    serialize_toon,
)


class ValidationError(ValueError):
    pass


_NONE_TYPE = type(None)
_PRIMITIVE_SCHEMAS = {
    str: {"type": "string"},
    int: {"type": "integer"},
    float: {"type": "number"},
    bool: {"type": "boolean"},
}
_SUPPORTED_PARAM_KINDS = {
    inspect.Parameter.POSITIONAL_OR_KEYWORD,
    inspect.Parameter.KEYWORD_ONLY,
}


def _field_types(type_: type, *, include_extras: bool = False) -> dict[str, Any]:
    return get_type_hints(type_, include_extras=include_extras)


def _unwrap_required(type_: Any) -> Any:
    origin = get_origin(type_)
    if origin in (Required, NotRequired):
        return get_args(type_)[0]
    return type_

def _jsonable(value: Any) -> Any:
    if isinstance(value, dict):
        return {str(key): _jsonable(item) for key, item in value.items()}
    if isinstance(value, (list, tuple, set)):
        return [_jsonable(item) for item in value]
    if isinstance(value, Path):
        return str(value)
    return value

def coerce(value: Any, type_: Any) -> Any:
    type_ = _unwrap_required(type_)
    if type_ is Any:
        return value

    origin = get_origin(type_)
    args = get_args(type_)

    if origin in (types.UnionType, Union):
        if value is None and _NONE_TYPE in args:
            return None
        last_error: ValidationError | None = None
        for candidate in args:
            if candidate is _NONE_TYPE:
                continue
            try:
                return coerce(value, candidate)
            except ValidationError as exc:
                last_error = exc
        if last_error is not None:
            raise last_error
        raise ValidationError(f"Unsupported union type: {type_}")

    if origin is Literal:
        if value not in args:
            raise ValidationError(f"Invalid enum value {value!r}")
        return value

    if origin is list:
        if not isinstance(value, list):
            raise ValidationError("Expected array")
        item_type = args[0] if args else Any
        return [coerce(item, item_type) for item in value]

    if origin is dict:
        if not isinstance(value, dict):
            raise ValidationError("Expected object")
        key_type, value_type = args if len(args) == 2 else (Any, Any)
        items: dict[Any, Any] = {}
        for key, item in value.items():
            if key_type is str:
                key = str(key)
            elif key_type is not Any:
                key = coerce(key, key_type)
            items[key] = coerce(item, value_type)
        return items

    if is_typeddict(type_):
        if not isinstance(value, dict):
            raise ValidationError(f"Expected object for {type_.__name__}")
        type_hints = _field_types(type_, include_extras=True)
        fields_by_name = {name: _unwrap_required(annotation) for name, annotation in type_hints.items()}
        extra = set(value) - set(fields_by_name)
        if extra:
            names = ", ".join(sorted(map(str, extra)))
            raise ValidationError(f"Unexpected fields: {names}")
        required = getattr(type_, "__required_keys__", frozenset(fields_by_name))
        items: dict[str, Any] = {}
        for name, annotation in fields_by_name.items():
            if name not in value:
                if name in required:
                    raise ValidationError(f"Missing required field '{name}'")
                continue
            try:
                items[name] = coerce(value[name], annotation)
            except ValidationError as exc:
                raise ValidationError(f"Invalid field '{name}': {exc}") from exc
        return items



    if type_ is str:
        if not isinstance(value, str):
            raise ValidationError("Expected string")
        return value
    if type_ is int:
        if not isinstance(value, int) or isinstance(value, bool):
            raise ValidationError("Expected integer")
        return value
    if type_ is float:
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise ValidationError("Expected number")
        return float(value)
    if type_ is bool:
        if not isinstance(value, bool):
            raise ValidationError("Expected boolean")
        return value

    return value



def json_schema(type_: Any) -> dict[str, Any]:
    type_ = _unwrap_required(type_)
    if type_ is Any:
        return {}

    origin = get_origin(type_)
    args = get_args(type_)

    if origin in (types.UnionType, Union):
        non_none = [arg for arg in args if arg is not _NONE_TYPE]
        if len(non_none) == 1 and len(non_none) != len(args):
            return json_schema(non_none[0])
        return {"anyOf": [json_schema(arg) for arg in args]}

    if origin is Literal:
        enum = list(args)
        schema: dict[str, Any] = {"enum": enum}
        if enum:
            sample = enum[0]
            if isinstance(sample, str):
                schema["type"] = "string"
            elif isinstance(sample, bool):
                schema["type"] = "boolean"
            elif isinstance(sample, int):
                schema["type"] = "integer"
        return schema

    if origin is list:
        return {"type": "array", "items": json_schema(args[0] if args else Any)}

    if origin is dict:
        value_type = args[1] if len(args) == 2 else Any
        schema = {"type": "object"}
        value_schema = json_schema(value_type)
        if value_schema:
            schema["additionalProperties"] = value_schema
        return schema

    if is_typeddict(type_):
        type_hints = _field_types(type_, include_extras=True)
        properties = {
            name: json_schema(_unwrap_required(annotation))
            for name, annotation in type_hints.items()
        }
        required_keys = getattr(type_, "__required_keys__", frozenset(type_hints))
        required = [name for name in type_hints if name in required_keys]
        schema = {"type": "object", "properties": properties, "additionalProperties": False}
        if required:
            schema["required"] = required
        return schema

    if type_ in _PRIMITIVE_SCHEMAS:
        return dict(_PRIMITIVE_SCHEMAS[type_])

    if type_ is _NONE_TYPE:
        return {"type": "null"}

    return {}



def coerce_arguments(
    fn: Callable[..., Any], args: dict[str, Any] | None, *, skip: set[str] | frozenset[str]
) -> dict[str, Any]:
    if args is None:
        args = {}
    if not isinstance(args, dict):
        raise ValidationError("Expected object")
    signature = inspect.signature(fn)
    type_hints = get_type_hints(fn, include_extras=True)
    parameters = [
        parameter
        for parameter in signature.parameters.values()
        if parameter.name not in skip and parameter.kind in _SUPPORTED_PARAM_KINDS
    ]
    allowed = {parameter.name for parameter in parameters}
    extra = set(args) - allowed
    if extra:
        names = ", ".join(sorted(map(str, extra)))
        raise ValidationError(f"Unexpected fields: {names}")
    kwargs: dict[str, Any] = {}
    for parameter in parameters:
        annotation = type_hints.get(parameter.name, Any)
        if parameter.name in args:
            try:
                kwargs[parameter.name] = coerce(args[parameter.name], annotation)
            except ValidationError as exc:
                raise ValidationError(f"Invalid field '{parameter.name}': {exc}") from exc
            continue
        if parameter.default is inspect.Signature.empty:
            raise ValidationError(f"Missing required field '{parameter.name}'")
        kwargs[parameter.name] = parameter.default
    return kwargs



def signature_schema(
    fn: Callable[..., Any], *, skip: set[str] | frozenset[str]
) -> dict[str, Any]:
    signature = inspect.signature(fn)
    type_hints = get_type_hints(fn, include_extras=True)
    properties: dict[str, Any] = {}
    required: list[str] = []
    for parameter in signature.parameters.values():
        if parameter.name in skip or parameter.kind not in _SUPPORTED_PARAM_KINDS:
            continue
        schema = json_schema(type_hints.get(parameter.name, Any))
        if parameter.default is inspect.Signature.empty:
            required.append(parameter.name)
        else:
            schema = {**schema, "default": _jsonable(parameter.default)}
        properties[parameter.name] = schema
    result = {"type": "object", "properties": properties, "additionalProperties": False}
    if required:
        result["required"] = required
    return result


_TODO_STATUSES = ("pending", "in_progress", "done")
_TODO_ITEM_KEYS = frozenset({"id", "task", "status"})


class TodoItemInput(TypedDict, total=False):
    id: Required[str]
    task: Required[str]
    status: NotRequired[Literal["pending", "in_progress", "done"]]


def _tool_name(name: str) -> str:
    return name[5:] if name.startswith("tool_") else name


def _tool_schema(fn: Callable[..., Any]):
    return signature_schema(fn, skip={"state"})


def _invoke_tool(handler: dict[str, Any], state: Any, args: dict[str, Any] | None = None):
    name = handler["name"]
    try:
        builtins = coerce_arguments(handler["fn"], args or {}, skip={"state"})
        if handler.get("mutating") and not _approve_mutating_tool(state, name, builtins):
            return ToolResult(
                ok=False,
                content={
                    "tool": name,
                    "error_type": "PermissionError",
                    "message": f"User denied approval for mutating tool '{name}'",
                },
            )
        return ToolResult(content=handler["fn"](state, **builtins))
    except Exception as exc:
        return ToolResult(
            ok=False,
            content={"tool": name, "error_type": type(exc).__name__, "message": str(exc)},
        )


_TOOLS: dict[str, dict[str, Any]] = {}


def tool(*, mutating: bool = False):
    def deco(fn: Callable[..., Any]):
        name = _tool_name(fn.__name__)
        _TOOLS[name] = {
            "name": name,
            "fn": fn,
            "description": rt.tool_description(name),
            "parameters": _tool_schema(fn),
            "mutating": mutating,
        }
        return fn

    return deco


def tool_specs(tools: dict[str, dict[str, Any]] | None = None):
    registry = _TOOLS if tools is None else tools
    return [
        {
            "name": tool["name"],
            "description": tool["description"],
            "parameters": tool["parameters"],
        }
        for tool in registry.values()
    ]


def select_tools(*, include=None, exclude=frozenset()):
    if include is None and not exclude:
        return _TOOLS
    return {
        name: tool
        for name, tool in _TOOLS.items()
        if (include is None or name in include) and name not in exclude
    }


def invoke_tool(registry: dict[str, dict[str, Any]], state: Any, name: str, args: dict[str, Any] | None = None):
    handler = registry.get(name)
    return _invoke_tool(handler, state, args) if handler else {"ok": False, "content": f"Tool '{name}' unavailable"}


TOOL_REGISTRY = _TOOLS


_MUTATING_TOOL_APPROVAL_CHOICES = ["once", "all", "deny"]


def _tool_approval_prompt(name: str, args: dict[str, Any]) -> str:
    details = ", ".join(
        f"{key.replace('_', '-')}: {rt.preview(value, 80)}"
        for key, value in args.items()
        if value not in (None, "", False)
    )
    suffix = f" — {details}" if details else ""
    return f"Approve `{name}`{suffix}?"


def _approve_mutating_tool(state: Any, name: str, args: dict[str, Any]) -> bool:
    if (
        state.get("yolo", False)
        or not state.get("interactive", False)
        or state.get("approve_all_mutating_tools", False)
    ):
        return True
    choice = rt.select(
        _tool_approval_prompt(name, args),
        _MUTATING_TOOL_APPROVAL_CHOICES,
        console=rt.STDERR,
        default="once",
        prompt_label="Approval",
        option_text=lambda option, index: {
            "once": f"{index}. once",
            "all": f"{index}. all session",
            "deny": f"{index}. deny",
        }[option],
    ).strip()
    if choice == "all":
        state["approve_all_mutating_tools"] = True
        rt._note("approved all mutating tools for this session", tag="note")
        return True
    if choice == "once":
        return True
    rt._note(f"denied mutating tool {rt._fmt('inline', name)}", tag="note")
    return False


def _positive_int(value: int, name: str) -> int:
    if not isinstance(value, int) or value <= 0:
        raise ValueError(f"{name} must be a positive integer")
    return value


def _shown_line_limit(limit: int) -> int:
    return max(limit, 1)


def _tool_content_payload(
    *, content: str, content_format: str, truncated: bool, **fields: Any
) -> dict[str, Any]:
    return {
        **fields,
        "content": content,
        "content_format": content_format,
        "truncated": truncated,
    }


def _collapse_repeated_lines(lines: list[str]) -> tuple[list[str], bool]:
    if not lines:
        return [], False
    collapsed: list[str] = []
    changed = False
    i = 0
    while i < len(lines):
        line = lines[i]
        j = i + 1
        while j < len(lines) and lines[j] == line:
            j += 1
        count = j - i
        if count > 1:
            collapsed.append(f"{line}  [repeated {count}x]")
            changed = True
        else:
            collapsed.append(line)
        i = j
    return collapsed, changed


def _render_selected_lines(lines: list[str], keep: set[int]) -> str:
    selected: list[str] = []
    last = -1
    for idx in sorted(keep):
        if idx < 0 or idx >= len(lines):
            continue
        if idx > last + 1:
            selected.append(f"... [{idx - last - 1} lines omitted]")
        selected.append(lines[idx])
        last = idx
    if last < len(lines) - 1:
        selected.append(f"... [{len(lines) - last - 1} lines omitted]")
    return "\n".join(selected)


def _indented_preview_lines(text: str, *, indent: str = "  ") -> list[str]:
    return [f"{indent}{line}" if line else indent for line in text.splitlines()] or [indent]


def _parse_bash_json_output(stdout: str, stderr: str):
    if stderr.strip() or not stdout.strip():
        return None
    try:
        return json.loads(stdout)
    except json.JSONDecodeError:
        return None


def _summarize_json_value(value: Any, *, depth: int = 0, width: int = 32):
    if depth >= 6:
        return "<max-depth>", True
    if isinstance(value, dict):
        items = list(value.items())
        limit = width if depth == 0 else max(width // 2, 8)
        out: dict[str, Any] = {}
        truncated = False
        for key, child in items[:limit]:
            summarized, child_truncated = _summarize_json_value(
                child, depth=depth + 1, width=width
            )
            out[str(key)] = summarized
            truncated = truncated or child_truncated
        if len(items) > limit:
            out["..."] = f"{len(items) - limit} more keys"
            truncated = True
        return out, truncated
    if isinstance(value, list):
        limit = width if depth == 0 else max(width // 2, 8)
        out = []
        truncated = False
        for child in value[:limit]:
            summarized, child_truncated = _summarize_json_value(
                child, depth=depth + 1, width=width
            )
            out.append(summarized)
            truncated = truncated or child_truncated
        if len(value) > limit:
            out.append(f"... {len(value) - limit} more items")
            truncated = True
        return out, truncated
    if isinstance(value, str):
        clipped = rt.clip_tokens(value, limit=512 if depth == 0 else 128, tail=32)
        return clipped, clipped != value
    return value, False


def _summarize_json_output(value: Any) -> tuple[Any, bool, str]:
    for width in (32, 16, 8, 4):
        summarized, truncated = _summarize_json_value(value, width=width)
        rendered = serialize_toon(summarized)
        if rt.count_tokens(rendered) <= rt.BUDGETS["tool_output_tokens"]:
            return rendered, truncated, "toon"
    rendered = rt.clip_tokens(
        serialize_toon(value),
        limit=rt.BUDGETS["tool_output_tokens"],
        tail=rt.BUDGETS["tool_tail_tokens"],
    )
    return rendered, True, "toon"


def _summarize_text_output(text: str) -> tuple[str, bool]:
    if not text:
        return "", False
    text = rt._truncate_long_lines(text)
    lines, collapsed = _collapse_repeated_lines(text.splitlines())
    rendered = "\n".join(lines)
    if len(lines) > 80 or rt.count_tokens(rendered) > rt.BUDGETS["tool_output_tokens"]:
        keep = set(range(min(30, len(lines))))
        keep.update(range(max(len(lines) - 20, 0), len(lines)))
        for idx, line in enumerate(lines):
            if rt._BASH_IMPORTANT_LINE_RE.search(line):
                keep.update({idx - 1, idx, idx + 1})
        rendered = _render_selected_lines(lines, keep)
        collapsed = True
    clipped = rt.clip_tokens(
        rendered,
        limit=rt.BUDGETS["tool_output_tokens"],
        tail=rt.BUDGETS["tool_tail_tokens"],
    )
    return clipped, collapsed or clipped != rendered


@tool()
def tool_ask(state: Any, question: str, choices: list[str] | None = None):
    rt.note_tool(state, "ask", question=question, choices=choices)
    rt.require_prompt("ask question")
    if not choices:
        return rt.ask(question, console=rt.STDERR, default="").strip()
    return rt.select(
        question,
        choices,
        console=rt.STDERR,
        prompt_label="Selection",
        option_text=lambda option, index: f"{index}. {rt._fmt('inline', option)}",
    ).strip()


def _todo_line(item: dict[str, str]) -> str:
    status_icons = {"pending": "[ ]", "in_progress": "[~]", "done": "[x]"}
    todo_id = " ".join(str(item.get("id", "")).split())
    task = " ".join(str(item.get("task", "")).split())
    prefix = status_icons.get(item.get("status", "pending"), "[ ]")
    return f"{prefix} {todo_id}: {task}" if task else f"{prefix} {todo_id}"


def _format_todos(todos: list[dict[str, str]]) -> str:
    if not todos:
        return "<empty todo list>"
    return "\n".join(_todo_line(item) for item in todos)


def _todo_preview(todos: list[dict[str, str]]) -> str:
    preview_lines = [f"count: {len(todos)}", "todos:"]
    if not todos:
        preview_lines.append("  <empty todo list>")
        return "\n".join(preview_lines)
    preview_lines.extend(f"  {_todo_line(item)}" for item in todos)
    return "\n".join(preview_lines)


@tool()
def tool_todo(state: Any, todos: list[TodoItemInput]):
    state["todos"] = []
    for item in todos:
        if not isinstance(item, dict):
            raise ValidationError("Expected object")
        extra = set(item) - _TODO_ITEM_KEYS
        if extra:
            names = ", ".join(sorted(map(str, extra)))
            raise ValidationError(f"Unexpected fields: {names}")
        todo_id = item.get("id")
        task = item.get("task")
        status = item.get("status", "pending")
        if not isinstance(todo_id, str):
            raise ValidationError("Invalid field 'id': Expected string")
        if not isinstance(task, str):
            raise ValidationError("Invalid field 'task': Expected string")
        if status not in _TODO_STATUSES:
            raise ValidationError(f"Invalid enum value {status!r}")
        state["todos"].append({"id": todo_id, "task": task, "status": status})
    payload = {"items": list(state["todos"]), "count": len(state["todos"])}
    rt.note_tool(state, 'todo', _suffix=f'({len(state["todos"])} items)')
    rt.show(_todo_preview(state["todos"]))
    return payload


def _merge_bash_streams(stdout: str, stderr: str) -> str:
    stdout = stdout.rstrip()
    stderr = stderr.rstrip()
    if stdout and stderr:
        return f"[stdout]\n{stdout}\n\n[stderr]\n{stderr}"
    if stdout:
        return stdout
    if stderr:
        return f"[stderr]\n{stderr}"
    return ""


def _bash_payload(command: str, result) -> tuple[dict[str, Any], str]:
    stdout = result.stdout or ""
    stderr = result.stderr or ""
    summarized_stdout, stdout_truncated = _summarize_text_output(stdout)
    summarized_stderr, stderr_truncated = _summarize_text_output(stderr)
    payload = {
        "command": command,
        "returncode": result.returncode,
        "stdout": summarized_stdout,
        "stderr": summarized_stderr,
        "stdout_truncated": stdout_truncated,
        "stderr_truncated": stderr_truncated,
    }
    preview_lines = [f"$ {command}", f"exit: {result.returncode}"]
    if summarized_stdout:
        preview_lines.append("stdout:")
        preview_lines.extend(_indented_preview_lines(summarized_stdout))
    if summarized_stderr:
        preview_lines.append("stderr:")
        preview_lines.extend(_indented_preview_lines(summarized_stderr))
    if not summarized_stdout and not summarized_stderr:
        preview_lines.append("<no output>")
    return payload, "\n".join(preview_lines)


@tool(mutating=True)
def tool_bash(state: Any, command: str, timeout_seconds: int = 120):
    if len(command.encode("utf-8", errors="replace")) > rt.MAX_BASH_CMD_BYTES:
        raise ValueError(
            f"command too large ({len(command)} chars); limit is {rt.MAX_BASH_CMD_BYTES} bytes"
        )
    rt.note_tool(
        state,
        "bash",
        _defaults={"timeout": 120},
        command=command,
        timeout=timeout_seconds,
    )
    env = rt.require_command_env(state["root"])
    bash_path = rt.which("bash", env.get("PATH"))
    if not bash_path:
        raise ValueError("bash is not installed or not on PATH")
    result = rt.run_cmd(
        [bash_path, "-c", command],
        cwd=state["root"],
        env=env,
        timeout=timeout_seconds,
    )
    payload, preview = _bash_payload(command, result)
    rt.show(preview)
    return payload


def _validate_url_safe(url: str) -> str:
    parsed = parse_url(url)
    if parsed.scheme not in ("http", "https"):
        raise ValueError(f"Only http/https URLs are allowed, got: {parsed.scheme!r}")
    hostname = parsed.hostname
    if not hostname:
        raise ValueError(f"No hostname in URL: {url!r}")
    local_hosts = {
        "localhost",
        "localhost.localdomain",
        "ip6-localhost",
        "ip6-loopback",
    }
    if hostname.lower() in local_hosts:
        raise ValueError(f"Local addresses are not allowed: {hostname!r}")
    try:
        addrinfos = socket.getaddrinfo(
            hostname, parsed.port or (443 if parsed.scheme == "https" else 80)
        )
    except socket.gaierror as exc:
        raise ValueError(f"Cannot resolve hostname {hostname!r}: {exc}") from exc
    for _family, _type, _proto, _canonname, sockaddr in addrinfos:
        ip = ipaddress.ip_address(sockaddr[0])
        if ip.is_private or ip.is_reserved or ip.is_loopback or ip.is_link_local:
            raise ValueError(
                f"URL resolves to non-public address ({ip}); "
                "private/reserved/loopback/link-local addresses are blocked"
            )
    return url


_WEBFETCH_ALLOWED_METHODS = {"GET", "HEAD", "OPTIONS"}
_WEBFETCH_BLOCKED_HEADERS = frozenset(
    {
        "authorization",
        "cookie",
        "host",
        "proxy-authorization",
        "x-forwarded-for",
        "x-real-ip",
    }
)
_WEBFETCH_REDACTED_RESPONSE_HEADERS = frozenset(
    {"set-cookie", "www-authenticate", "proxy-authenticate", "location"}
)
_WEBFETCH_HTML_CONTENT_TYPES = ("text/html", "application/xhtml+xml")


def _webfetch_is_text_response(response: ResponseAdapter) -> bool:
    content_type = response["headers"].get("content-type", "").split(";", 1)[0].strip().lower()
    return (
        not content_type
        or content_type.startswith("text/")
        or content_type in {
            "application/json",
            "application/xml",
            "application/javascript",
            "application/x-javascript",
            "application/x-www-form-urlencoded",
            "image/svg+xml",
        }
        or content_type.endswith("+json")
        or content_type.endswith("+xml")
    )


def _sanitize_webfetch_headers(headers: dict[str, str] | None) -> dict[str, str]:
    if not headers:
        return {}
    clean: dict[str, str] = {}
    for key, value in headers.items():
        key_str, val_str = str(key), str(value)
        if key_str.lower() in _WEBFETCH_BLOCKED_HEADERS:
            raise ValueError(f"Header {key_str!r} is not allowed in webfetch requests")
        if "\r" in val_str or "\n" in val_str:
            raise ValueError(
                f"Header value for {key_str!r} contains invalid CRLF characters"
            )
        clean[key_str] = val_str
    return clean


def _webfetch_is_html_response(response: ResponseAdapter, text: str) -> bool:
    content_type = (
        response["headers"].get("content-type", "").split(";", 1)[0].strip().lower()
    )
    if content_type in _WEBFETCH_HTML_CONTENT_TYPES:
        return True
    return text.lstrip().lower().startswith(("<!doctype html", "<html"))


def _html_to_markdown(text: str) -> str:
    return markdownify(text)


def _webfetch_summarize_response_body(
    response: ResponseAdapter,
) -> tuple[str, bool, str]:
    text = response["text"]
    content_format = "text"
    if (
        _webfetch_is_html_response(response, text)
        and rt.count_tokens(text) > rt.BUDGETS["tool_output_tokens"]
    ):
        markdown = _html_to_markdown(text)
        if markdown:
            text = markdown
            content_format = "markdown"
    summarized, truncated = _summarize_text_output(text)
    return summarized, truncated, content_format


def _webfetch_response_headers(response: ResponseAdapter) -> dict[str, str]:
    def display_name(name: str) -> str:
        return "-".join(part.capitalize() for part in name.split("-"))

    return {
        display_name(key): (
            "<redacted>"
            if key.lower() in _WEBFETCH_REDACTED_RESPONSE_HEADERS
            else response["headers"][key]
        )
        for key in response["headers"].keys()
    }



def _webfetch_payload(
    response: ResponseAdapter,
    *,
    method: str,
    text: str | None = None,
    truncated: bool | None = None,
    content_format: str = "text",
) -> dict[str, Any]:
    payload = {
        "method": method,
        "url": str(response["url"]),
        "status_code": response["status_code"],
        "reason_phrase": response["reason_phrase"],
        "http_version": response["http_version"] or "HTTP/1.1",
        "headers": _webfetch_response_headers(response),
    }
    if text is None:
        payload["binary"] = True
        payload["content_bytes"] = len(response["content"])
        return payload
    payload["text"] = text
    payload["format"] = content_format
    payload["truncated"] = bool(truncated)
    return payload


def _webfetch_error_payload(
    url: str, *, method: str, exc: HTTPError
) -> dict[str, Any]:
    return {
        "method": method,
        "url": url,
        "ok": False,
        "error_type": type(exc).__name__,
        "message": str(exc),
    }


@tool()
def tool_webfetch(
    state: Any,
    url: str,
    method: str = "GET",
    headers: dict[str, str] | None = None,
    follow_redirects: bool = False,
    timeout_seconds: int = DEFAULT_WEBFETCH_TIMEOUT_SECONDS,
):
    method = method.upper()
    if method not in _WEBFETCH_ALLOWED_METHODS:
        raise ValueError(
            f"Only {', '.join(sorted(_WEBFETCH_ALLOWED_METHODS))} methods are allowed, got: {method!r}"
        )
    _validate_url_safe(url)
    headers = _sanitize_webfetch_headers(headers)
    rt.note_tool(
        state,
        "webfetch",
        _defaults={
            "method": "GET",
            "headers": {},
            "follow_redirects": False,
            "timeout_seconds": DEFAULT_WEBFETCH_TIMEOUT_SECONDS,
        },
        url=url,
        method=method,
        headers=headers,
        follow_redirects=follow_redirects,
        timeout_seconds=timeout_seconds,
    )
    try:
        with rt.tool_session(
            timeout=timeout_seconds,
            follow_redirects=follow_redirects,
        ) as client:
            response = adapt_response(client.request(method, url, headers=headers))
    except (HTTPError, TransportError) as exc:
        payload = _webfetch_error_payload(url, method=method, exc=exc)
        rt.show(serialize_toon(payload))
        return payload
    if _webfetch_is_text_response(response):
        text, truncated, content_format = _webfetch_summarize_response_body(response)
        payload = _webfetch_payload(
            response,
            method=method,
            text=text,
            truncated=truncated,
            content_format=content_format,
        )
    else:
        payload = _webfetch_payload(response, method=method)
    rt.show(serialize_toon(payload))
    return payload

_STREAM_OPENERS = {
    ".gz": gzip.open,
    ".bz2": bz2.open,
    ".xz": lzma.open,
    ".zst": zstandard.open,
}
_ARCHIVE_SUFFIXES = frozenset({".zip", ".tar"})
_ARCHIVE_NAMES = (".tar", ".tgz", ".tar.gz")
_DEFAULT_THREADS = min(32, (os.cpu_count() or 4))
_DEFAULT_SLOC_TOP_FILES = 20

def _ignore_spec(
    root: Path,
    exclude: str | list[str] | None = None,
    *,
    include_gitignore: bool = True,
) -> pathspec.PathSpec:
    patterns = [".git/"]
    gitignore = root / ".gitignore"
    if include_gitignore and gitignore.is_file():
        patterns.extend(gitignore.read_text(encoding="utf-8", errors="replace").splitlines())
    if isinstance(exclude, str):
        patterns.extend(line.strip() for line in exclude.splitlines() if line.strip())
    elif exclude:
        patterns.extend(str(item).strip() for item in exclude if str(item).strip())
    return pathspec.GitIgnoreSpec.from_lines(patterns)

def _resolve_ignore_root(target: Path, ignore_root: str | Path | None) -> Path:
    root = (
        Path(ignore_root).resolve()
        if ignore_root is not None
        else (target if target.is_dir() else target.parent).resolve()
    )
    try:
        target.relative_to(root)
    except ValueError:
        return (target if target.is_dir() else target.parent).resolve()
    return root

def _is_ignored(path: Path, spec: pathspec.PathSpec, root: Path) -> bool:
    rel_path = path.relative_to(root).as_posix()
    if path.is_dir():
        rel_path += "/"
    return spec.match_file(rel_path)

def _iter_files(
    target: Path,
    *,
    ignore_root: str | Path | None = None,
    exclude: str | list[str] | None = None,
) -> list[Path]:
    root = _resolve_ignore_root(target, ignore_root)
    spec = _ignore_spec(root, exclude=exclude)
    if target.is_file():
        return [] if _is_ignored(target, spec, root) else [target]
    files: list[Path] = []
    for current_dir, dirs, names in os.walk(target):
        current = Path(current_dir)
        dirs[:] = [name for name in dirs if not _is_ignored(current / name, spec, root)]
        for name in names:
            file_path = current / name
            if not _is_ignored(file_path, spec, root):
                files.append(file_path)
    files.sort()
    return files

def _streams(path: Path) -> Iterable[tuple[str, BinaryIO]]:
    if path.suffix == ".zip":
        with zipfile.ZipFile(path, "r") as archive:
            for name in sorted(archive.namelist()):
                if not name.endswith("/"):
                    yield f"{path}::{name}", archive.open(name, "r")
        return
    if path.name.endswith(_ARCHIVE_NAMES):
        with tarfile.open(path, "r:*") as archive:
            for member in archive.getmembers():
                if member.isfile() and (stream := archive.extractfile(member)) is not None:
                    yield f"{path}::{member.name}", stream
        return
    opener = _STREAM_OPENERS.get(path.suffix, open)
    yield str(path), opener(path, "rb")

def _search_pattern(pattern: str, *, fuzzy: str | None = None) -> str:
    if fuzzy is None:
        return pattern
    constraint = fuzzy.strip()
    if constraint.startswith("{") and constraint.endswith("}"):
        constraint = constraint[1:-1].strip()
    if not constraint:
        raise ValueError("fuzzy must not be empty")
    return f"(?:{pattern}){{{constraint}}}"


def _search_flags(*, best_match: bool = False, enhance_match: bool = False) -> regex.RegexFlag:
    flags = regex.RegexFlag(0)
    if best_match:
        flags |= regex.BESTMATCH
    if enhance_match:
        flags |= regex.ENHANCEMATCH
    return flags


def _highlight_search_text(text: str, compiled: regex.Pattern) -> tuple[str, int]:
    spans: list[tuple[int, int]] = []
    for match in compiled.finditer(text):
        start, end = match.span()
        if spans and start <= spans[-1][1]:
            spans[-1] = (spans[-1][0], max(spans[-1][1], end))
        else:
            spans.append((start, end))
    if not spans:
        return text, 1
    parts: list[str] = []
    last = 0
    for start, end in spans:
        parts.append(text[last:start])
        if start < end:
            parts.append(f"{rt._SEARCH_HIGHLIGHT_OPEN}{text[start:end]}{rt._SEARCH_HIGHLIGHT_CLOSE}")
        last = end
    parts.append(text[last:])
    return "".join(parts), spans[0][0] + 1


def _search_file(
    path: Path,
    compiled_bytes: regex.Pattern,
    compiled_text: regex.Pattern,
) -> list[dict[str, Any]]:
    matches: list[dict[str, Any]] = []
    for source, stream in _streams(path):
        try:
            with stream as handle:
                for line_number, raw_line in enumerate(handle, 1):
                    if not compiled_bytes.search(raw_line):
                        continue
                    text = raw_line.decode("utf-8", errors="replace").rstrip("\r\n")
                    highlighted_text, column = _highlight_search_text(text, compiled_text)
                    matches.append(
                        {
                            "source": source,
                            "line_number": line_number,
                            "column": column,
                            "text": text,
                            "preview_text": highlighted_text,
                        }
                    )
        except Exception as exc:
            matches.append({"source": source, "error": str(exc)})
    return matches

def search(
    target: str | Path,
    pattern: str,
    *,
    fuzzy: str | None = None,
    best_match: bool = False,
    enhance_match: bool = False,
    threads: int | None = None,
    ignore_root: str | Path | None = None,
    exclude: str | list[str] | None = None,
) -> list[dict[str, Any]]:
    path = Path(target).resolve()
    if not path.exists():
        raise ValueError(f"Path not found: {target}")
    if not (path.is_dir() or path.is_file()):
        raise ValueError(f"Unsupported search target: {target}")
    search_pattern = _search_pattern(pattern, fuzzy=fuzzy)
    flags = _search_flags(best_match=best_match, enhance_match=enhance_match)
    try:
        compiled_bytes = regex.compile(search_pattern.encode("utf-8"), flags)
        compiled_text = regex.compile(search_pattern, flags)
    except regex.error as exc:
        raise ValueError(f"Invalid search pattern: {exc}") from exc
    files = _iter_files(path, ignore_root=ignore_root, exclude=exclude)
    if not files:
        return []
    worker_count = min(len(files), max(1, threads or _DEFAULT_THREADS))
    worker = partial(
        _search_file,
        compiled_bytes=compiled_bytes,
        compiled_text=compiled_text,
    )
    if worker_count == 1:
        results: list[dict[str, Any]] = []
        for file_path in files:
            results.extend(worker(file_path))
        return results
    results: list[dict[str, Any]] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=worker_count) as pool:
        for batch in pool.map(worker, files):
            results.extend(batch)
    return results

def _is_archive(path: Path) -> bool:
    return (
        path.suffix in _STREAM_OPENERS
        or path.suffix in _ARCHIVE_SUFFIXES
        or path.name.endswith(_ARCHIVE_NAMES)
    )

def _replace_file(
    path: Path,
    compiled: regex.Pattern,
    replacement: str,
) -> dict[str, Any]:
    if path.is_symlink():
        return {"source": str(path), "skipped": "symlink"}
    if _is_archive(path):
        return {"source": str(path), "skipped": "archive"}
    try:
        raw = path.read_bytes()
    except OSError as exc:
        return {"source": str(path), "error": str(exc)}
    if b"\x00" in raw:
        return {"source": str(path), "skipped": "binary file"}
    try:
        original_text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        return {"source": str(path), "error": f"cannot decode utf-8: {exc}"}
    updated_text, replacements = compiled.subn(replacement, original_text)
    if replacements == 0:
        return {"source": str(path), "replacements": 0}
    try:
        with path.open("w", encoding="utf-8", newline="") as handle:
            handle.write(updated_text)
    except OSError as exc:
        return {"source": str(path), "error": str(exc)}
    return {"source": str(path), "replacements": replacements}

def replace(
    target: str | Path,
    pattern: str,
    replacement: str,
    *,
    threads: int | None = None,
    ignore_root: str | Path | None = None,
    exclude: str | list[str] | None = None,
) -> list[dict[str, Any]]:
    path = Path(target).resolve()
    if not path.exists():
        raise ValueError(f"Path not found: {target}")
    if not (path.is_dir() or path.is_file()):
        raise ValueError(f"Unsupported replace target: {target}")
    try:
        compiled = regex.compile(pattern)
    except regex.error as exc:
        raise ValueError(f"Invalid replace pattern: {exc}") from exc
    files = _iter_files(path, ignore_root=ignore_root, exclude=exclude)
    if not files:
        return []
    worker_count = min(len(files), max(1, threads or _DEFAULT_THREADS))
    if worker_count == 1:
        return [_replace_file(file_path, compiled, replacement) for file_path in files]
    with concurrent.futures.ThreadPoolExecutor(max_workers=worker_count) as pool:
        return list(
            pool.map(
                partial(_replace_file, compiled=compiled, replacement=replacement),
                files,
            )
        )

type _SlocFileSummary = dict[str, str | int]


def sloc(
    target: str | Path,
    *,
    ignore_root: str | Path | None = None,
    exclude: str | list[str] | None = None,
) -> dict[str, Any]:
    path = Path(target).resolve()
    if not path.exists():
        raise ValueError(f"Path not found: {target}")
    if not (path.is_dir() or path.is_file()):
        raise ValueError(f"Unsupported sloc target: {target}")

    summary = ProjectSummary()
    duplicate_pool = DuplicatePool()
    state_counts: Counter[str] = Counter()
    errors: list[dict[str, str]] = []
    file_summaries: list[_SlocFileSummary] = []
    group = path.name if path.is_dir() else (path.parent.name or ".")

    for file_path in _iter_files(path, ignore_root=ignore_root, exclude=exclude):
        if file_path.is_symlink():
            state_counts["symlink"] += 1
            continue
        analysis = SourceAnalysis.from_file(
            str(file_path),
            group=group,
            encoding="utf-8",
            fallback_encoding="utf-8",
            duplicate_pool=duplicate_pool,
        )
        summary.add(analysis)
        if analysis.is_countable:
            file_summaries.append(
                {
                    "path": str(file_path),
                    "language": analysis.language,
                    "code_count": analysis.code_count,
                    "documentation_count": analysis.documentation_count,
                    "empty_count": analysis.empty_count,
                    "string_count": analysis.string_count,
                    "line_count": analysis.line_count,
                }
            )
            continue
        state_counts[analysis.state.name] += 1
        if analysis.state.name == "error":
            errors.append({"path": str(file_path), "message": analysis.state_info or "unknown pygount error"})

    languages = sorted(
        (
            {
                "language": language_summary.language,
                "file_count": language_summary.file_count,
                "code_count": language_summary.code_count,
                "documentation_count": language_summary.documentation_count,
                "empty_count": language_summary.empty_count,
                "string_count": language_summary.string_count,
            }
            for language_summary in summary.language_to_language_summary_map.values()
            if not language_summary.is_pseudo_language
        ),
        key=lambda item: (-item["code_count"], -item["file_count"], item["language"].lower()),
    )
    top_files = [
        item
        for item in sorted(
            file_summaries,
            key=lambda item: (-int(item["code_count"]), -int(item["line_count"]), str(item["path"]).lower()),
        )
    ]
    ordered_states = tuple(
        {"state": state, "file_count": file_count}
        for state, file_count in sorted(
            state_counts.items(), key=lambda item: (-item[1], item[0])
        )
    )

    return {
        "total_file_count": summary.total_file_count,
        "total_code_count": summary.total_code_count,
        "total_documentation_count": summary.total_documentation_count,
        "total_empty_count": summary.total_empty_count,
        "total_string_count": summary.total_string_count,
        "total_line_count": summary.total_line_count,
        "languages": languages,
        "top_files": top_files,
        "state_counts": list(ordered_states),
        "errors": errors,
    }

def _join_paths(paths: list[Path], root: Path, empty: str = "<no matches>") -> str:
    return (
        "\n".join(
            rt._rel(root, path) + ("/" if path.is_dir() else "")
            for path in paths
        )
        or empty
    )

def _path_listing(
    paths: list[Path], root: Path, *, limit: int, empty: str = "<no matches>"
) -> str:
    return _join_paths(paths[: _shown_line_limit(limit)], root, empty)

def _glob_paths(
    root: Path,
    pattern: str,
    *,
    exclude: str | list[str] | None = None,
) -> list[Path]:
    if pattern in {".", "./"}:
        spec = _ignore_spec(root, exclude=exclude, include_gitignore=False)
        return [
            item
            for item in sorted(root.iterdir(), key=lambda item: item.as_posix())
            if not _is_ignored(item.resolve(), spec, root)
        ]
    if Path(pattern).is_absolute() or ".." in Path(pattern).parts:
        raise ValueError(f"Path traversal denied: '{pattern}'")
    spec = _ignore_spec(root, exclude=exclude, include_gitignore=False)
    matches: list[Path] = []
    for candidate in root.glob(pattern):
        try:
            resolved = candidate.resolve()
        except OSError:
            continue
        if (resolved == root or root in resolved.parents) and not _is_ignored(resolved, spec, root):
            matches.append(resolved)
    return sorted(set(matches), key=lambda item: item.as_posix())

@tool()
def tool_list(
    state: Any,
    path: str = "*",
    exclude: str | list[str] | None = None,
    limit: int = rt.BUDGETS["default_line_limit"],
):
    rt.note_tool(
        state,
        "list",
        _defaults={"path": "*", "exclude": None, "limit": rt.BUDGETS["default_line_limit"]},
        path=path,
        exclude=exclude,
        limit=limit,
    )
    matches = _glob_paths(state["root"], path, exclude=exclude)
    payload = {
        "path": path,
        "items": [
            rt._rel(state["root"], item) + ("/" if item.is_dir() else "")
            for item in matches[: _shown_line_limit(limit)]
        ],
        "count": len(matches),
        "truncated": len(matches) > _shown_line_limit(limit),
    }
    if exclude is not None:
        payload["exclude"] = exclude
    rt.show(serialize_toon(payload))
    return payload

@tool()
def tool_read(
    state: Any, path: str, offset: int = 1, limit: int = rt.BUDGETS["default_line_limit"]
):
    rt.note_tool(
        state,
        "read",
        _defaults={"offset": 1, "limit": rt.BUDGETS["default_line_limit"]},
        path=path,
        offset=offset,
        limit=limit,
    )
    target = rt.resolve_path(state["root"], path)
    if not target.exists():
        raise ValueError(f"read path does not exist: {rt._rel(state['root'], target)}")
    if target.is_dir():
        raise ValueError(f"read path is a directory: {rt._rel(state['root'], target)}")
    start = max(_positive_int(offset, "offset"), 1) - 1
    lines = target.read_text(encoding="utf-8", errors="replace").splitlines()
    shown = lines[start : start + _shown_line_limit(limit)]
    text = "\n".join(shown)
    payload = {
        "path": path,
        "offset": offset,
        "limit": limit,
        "text": text,
        "line_count": len(lines),
        "truncated": start + len(shown) < len(lines),
    }
    end_line = start + len(shown)
    if text:
        language = rt.preview_language_for_path(path, text)
        preview = {
            "path": path,
            "lines": f"{start + 1}-{end_line} of {len(lines)}",
            f"text.{language}" if language != "text" else "text": text,
        }
        rt.show(serialize_toon(preview))
    else:
        rt.show(f"path: {path}\nlines: {start + 1}-{end_line} of {len(lines)}\n<empty file>")
    return payload

def _search_summary(matches: int, shown: int, errors: int = 0) -> str:
    if not matches and not errors:
        return "(no matches)"
    parts = []
    if matches:
        extra = f"; showing {shown} of {matches}" if shown < matches else ""
        plural = "es" if matches != 1 else ""
        parts.append(f"{matches} match{plural}{extra}")
    if errors:
        plural = "s" if errors != 1 else ""
        parts.append(f"{errors} error{plural}")
    return "(" + "; ".join(parts) + ")"

def _search_display_path(root: Path, source: str) -> str:
    if "::" in source:
        container, member = source.split("::", 1)
        return f"{rt._rel(root, Path(container))}::{member}"
    return rt._rel(root, Path(source))

def _search_preview_line(root: Path, match: dict[str, Any]) -> str:
    path_text = _search_display_path(root, match["source"])
    if match.get("error"):
        return f"[!] {path_text}: {match['error']}"
    text = rt._truncate_long_lines(match.get("preview_text") or match["text"])
    return f"{path_text}:{match['line_number']}:{match.get('column', 1)}:{text}"

def _optional_counts(**counts: int) -> dict[str, int]:
    return {
        f"{name}_count": value
        for name, value in counts.items()
        if value
    }

def _search_payload(
    root: Path,
    pattern: str,
    path: str,
    results: list[dict[str, Any]],
    *,
    fuzzy: str | None = None,
    best_match: bool = False,
    enhance_match: bool = False,
    exclude: str | list[str] | None = None,
    limit: int,
) -> tuple[dict[str, Any], str, int, int, int]:
    matches = [match for match in results if not match.get("error")]
    errors = [match for match in results if match.get("error")]
    shown_limit = _shown_line_limit(limit)
    shown_matches = matches[:shown_limit]
    payload: dict[str, Any] = {
        "pattern": pattern,
        "path": path,
        "match_count": len(matches),
        "matches": [
            {
                "path": _search_display_path(root, match["source"]),
                "line_number": match["line_number"],
                "column": match.get("column", 1),
                "text": rt._truncate_long_lines(match["text"]),
            }
            for match in shown_matches
        ],
        "truncated": len(matches) > len(shown_matches),
    }
    payload.update(
        {
            key: value
            for key, value in {
                "fuzzy": fuzzy,
                "best_match": best_match or None,
                "enhance_match": enhance_match or None,
                "exclude": exclude,
            }.items()
            if value is not None
        }
    )
    payload.update(_optional_counts(error=len(errors)))
    if errors:
        payload["errors"] = [
            {"path": _search_display_path(root, match["source"]), "message": match["error"]}
            for match in errors[:shown_limit]
        ]
    preview_lines = [_search_preview_line(root, match) for match in shown_matches]
    if errors:
        preview_lines.extend(
            _search_preview_line(root, match) for match in errors[:shown_limit]
        )
    preview = "\n".join(preview_lines) if preview_lines else "<no matches>"
    if len(matches) > len(shown_matches):
        preview += f"\n... [{len(matches) - len(shown_matches)} more matches omitted]"
    return payload, preview, len(matches), len(shown_matches), len(errors)

def _search_contents(
    root: Path,
    pattern: str,
    path: str,
    *,
    fuzzy: str | None = None,
    best_match: bool = False,
    enhance_match: bool = False,
    exclude: str | list[str] | None = None,
    limit: int,
):
    target = rt.resolve_path(root, path)
    if not target.exists():
        raise ValueError(f"search path does not exist: {rt._rel(root, target)}")
    results = search(
        target,
        pattern,
        fuzzy=fuzzy,
        best_match=best_match,
        enhance_match=enhance_match,
        ignore_root=root,
        exclude=exclude,
    )
    return _search_payload(
        root,
        pattern,
        path,
        results,
        fuzzy=fuzzy,
        best_match=best_match,
        enhance_match=enhance_match,
        exclude=exclude,
        limit=limit,
    )

@tool()
def tool_search(
    state: Any,
    pattern: str,
    path: str = ".",
    fuzzy: str | None = None,
    best_match: bool = False,
    enhance_match: bool = False,
    exclude: str | list[str] | None = None,
    limit: int = rt.BUDGETS["default_line_limit"],
):
    defaults = {
        "path": ".",
        "fuzzy": None,
        "best_match": False,
        "enhance_match": False,
        "exclude": None,
        "limit": rt.BUDGETS["default_line_limit"],
    }
    payload, preview, matches, shown, errors = _search_contents(
        state["root"],
        pattern,
        path,
        fuzzy=fuzzy,
        best_match=best_match,
        enhance_match=enhance_match,
        exclude=exclude,
        limit=limit,
    )
    rt.note_tool(
        state,
        "search",
        _defaults=defaults,
        _suffix=_search_summary(matches, shown, errors),
        pattern=pattern,
        path=path,
        fuzzy=fuzzy,
        best_match=best_match,
        enhance_match=enhance_match,
        exclude=exclude,
        limit=limit,
    )
    rt.show(preview)
    return payload

def _replace_summary(
    changed_files: int, replacements: int, skipped: int = 0, errors: int = 0
) -> str:
    if not changed_files and not skipped and not errors:
        return "(no changes)"
    parts = []
    if changed_files:
        plural = "s" if changed_files != 1 else ""
        parts.append(f"{changed_files} file{plural} changed")
    if replacements:
        plural = "s" if replacements != 1 else ""
        parts.append(f"{replacements} replacement{plural}")
    if skipped:
        plural = "s" if skipped != 1 else ""
        parts.append(f"{skipped} skipped")
    if errors:
        plural = "s" if errors != 1 else ""
        parts.append(f"{errors} error{plural}")
    return "(" + "; ".join(parts) + ")"

def _replace_preview_line(root: Path, result: dict[str, Any]) -> str:
    path_text = rt._rel(root, Path(result["source"]))
    if result.get("error"):
        return f"[!] {path_text} — {result['error']}"
    if result.get("skipped"):
        return f"skip: {path_text} — {result['skipped']}"
    replacements = result.get("replacements", 0)
    plural = "s" if replacements != 1 else ""
    return f"change: {path_text} — {replacements} replacement{plural}"

def _replace_payload(
    root: Path,
    pattern: str,
    replacement: str,
    path: str,
    results: list[dict[str, Any]],
    *,
    exclude: str | list[str] | None = None,
    limit: int,
) -> tuple[dict[str, Any], str, int, int, int, int]:
    changed = [result for result in results if result.get("replacements")]
    skipped = [result for result in results if result.get("skipped")]
    errors = [result for result in results if result.get("error")]
    shown_limit = _shown_line_limit(limit)
    shown_changed = changed[:shown_limit]
    replacement_count = sum(result.get("replacements", 0) for result in changed)
    payload: dict[str, Any] = {
        "pattern": pattern,
        "replacement": replacement,
        "path": path,
        "changed_file_count": len(changed),
        "replacement_count": replacement_count,
        "changed_files": [
            {
                "path": rt._rel(root, Path(result["source"])),
                "replacements": result.get("replacements", 0),
            }
            for result in shown_changed
        ],
        "truncated": len(changed) > len(shown_changed),
    }
    if exclude is not None:
        payload["exclude"] = exclude
    payload.update(_optional_counts(skipped=len(skipped), error=len(errors)))
    if skipped:
        payload["skipped"] = [
            {"path": rt._rel(root, Path(result["source"])), "reason": result["skipped"]}
            for result in skipped[:shown_limit]
        ]
    if errors:
        payload["errors"] = [
            {"path": rt._rel(root, Path(result["source"])), "message": result["error"]}
            for result in errors[:shown_limit]
        ]
    preview_lines = [
        f"replace: /{pattern}/ -> {replacement!r}",
        f"path: {path}",
    ]
    if shown_changed:
        preview_lines.append("changed:")
        preview_lines.extend(
            f"  {_replace_preview_line(root, result)}" for result in shown_changed
        )
    if skipped:
        preview_lines.append("skipped:")
        preview_lines.extend(
            f"  {_replace_preview_line(root, result)}" for result in skipped[:shown_limit]
        )
    if errors:
        preview_lines.append("errors:")
        preview_lines.extend(
            f"  {_replace_preview_line(root, result)}" for result in errors[:shown_limit]
        )
    preview = "\n".join(preview_lines) if len(preview_lines) > 2 else "<no changes>"
    if len(changed) > len(shown_changed):
        preview += (
            f"\n... [{len(changed) - len(shown_changed)} more changed files omitted]"
        )
    return (
        payload,
        preview,
        len(changed),
        replacement_count,
        len(skipped),
        len(errors),
    )

def _replace_contents(
    root: Path,
    pattern: str,
    replacement: str,
    path: str,
    *,
    exclude: str | list[str] | None = None,
    limit: int,
):
    target = rt.resolve_path(root, path)
    if not target.exists():
        raise ValueError(f"replace path does not exist: {rt._rel(root, target)}")
    results = replace(target, pattern, replacement, ignore_root=root, exclude=exclude)
    return _replace_payload(root, pattern, replacement, path, results, exclude=exclude, limit=limit)

@tool(mutating=True)
def tool_replace(
    state: Any,
    pattern: str,
    replacement: str,
    path: str = ".",
    exclude: str | list[str] | None = None,
    limit: int = rt.BUDGETS["default_line_limit"],
):
    defaults = {"path": ".", "exclude": None, "limit": rt.BUDGETS["default_line_limit"]}
    payload, preview, changed, replacements, skipped, errors = _replace_contents(
        state["root"], pattern, replacement, path, exclude=exclude, limit=limit
    )
    rt.note_tool(
        state,
        "replace",
        _defaults=defaults,
        _suffix=_replace_summary(changed, replacements, skipped, errors),
        pattern=pattern,
        replacement=replacement,
        path=path,
        exclude=exclude,
        limit=limit,
    )
    rt.show(preview)
    return payload

def _sloc_summary(report: dict[str, Any]) -> str:
    parts = []
    if report["total_file_count"]:
        plural = "s" if report["total_file_count"] != 1 else ""
        parts.append(f"{report['total_file_count']} file{plural}")
    if report["total_code_count"]:
        parts.append(f"{report['total_code_count']} code lines")
    non_countable = sum(item["file_count"] for item in report["state_counts"])
    if non_countable:
        parts.append(f"{non_countable} non-countable")
    if report["errors"]:
        plural = "s" if len(report["errors"]) != 1 else ""
        parts.append(f"{len(report['errors'])} error{plural}")
    return "(" + "; ".join(parts) + ")" if parts else "(no source files)"

def _sloc_totals_line(report: dict[str, Any]) -> str:
    return (
        "totals: "
        f"{report['total_file_count']} files, "
        f"{report['total_code_count']} code, "
        f"{report['total_documentation_count']} comments, "
        f"{report['total_empty_count']} empty, "
        f"{report['total_string_count']} strings, "
        f"{report['total_line_count']} lines"
    )

def _sloc_language_preview_line(language: Any) -> str:
    file_plural = "s" if language["file_count"] != 1 else ""
    return (
        f"{language['language']}: "
        f"{language['code_count']} code, "
        f"{language['documentation_count']} comments, "
        f"{language['empty_count']} empty, "
        f"{language['string_count']} strings "
        f"({language['file_count']} file{file_plural})"
    )

def _sloc_state_preview_line(state_count: Any) -> str:
    file_plural = "s" if state_count["file_count"] != 1 else ""
    return f"other/{state_count['state']}: {state_count['file_count']} file{file_plural}"


def _sloc_file_preview_line(file_summary: Any) -> str:
    file_plural = "s" if file_summary["code_count"] != 1 else ""
    return (
        f"file: {file_summary['path']} — "
        f"{file_summary['code_count']} code line{file_plural}, "
        f"{file_summary['line_count']} total, "
        f"{file_summary['language']}"
    )


def _sloc_payload(
    root: Path,
    path: str,
    report: dict[str, Any],
    *,
    exclude: str | list[str] | None = None,
    limit: int,
) -> tuple[dict[str, Any], str]:
    shown_limit = _shown_line_limit(limit)
    shown_languages = list(report["languages"][:shown_limit])
    shown_top_files = list(report["top_files"][:_DEFAULT_SLOC_TOP_FILES])
    payload: dict[str, Any] = {
        "path": path,
        "total_file_count": report["total_file_count"],
        "total_code_count": report["total_code_count"],
        "total_documentation_count": report["total_documentation_count"],
        "total_empty_count": report["total_empty_count"],
        "total_string_count": report["total_string_count"],
        "total_line_count": report["total_line_count"],
        "language_count": len(report["languages"]),
        "languages": [
            {
                "language": language["language"],
                "file_count": language["file_count"],
                "code_count": language["code_count"],
                "documentation_count": language["documentation_count"],
                "empty_count": language["empty_count"],
                "string_count": language["string_count"],
            }
            for language in shown_languages
        ],
        "top_file_count": len(report["top_files"]),
        "top_files": [
            {
                "path": rt._rel(root, Path(file_summary["path"])),
                "language": file_summary["language"],
                "code_count": file_summary["code_count"],
                "documentation_count": file_summary["documentation_count"],
                "empty_count": file_summary["empty_count"],
                "string_count": file_summary["string_count"],
                "line_count": file_summary["line_count"],
            }
            for file_summary in shown_top_files
        ],
        "truncated": (
            len(report["languages"]) > len(shown_languages)
            or len(report["top_files"]) > len(shown_top_files)
        ),
    }
    if exclude is not None:
        payload["exclude"] = exclude
    if report["state_counts"]:
        payload["state_counts"] = [
            {"state": state_count["state"], "file_count": state_count["file_count"]}
            for state_count in report["state_counts"]
        ]
    payload.update(_optional_counts(error=len(report["errors"])))
    if report["errors"]:
        payload["errors"] = [
            {"path": rt._rel(root, Path(error["path"])), "message": error["message"]}
            for error in report["errors"][:shown_limit]
        ]
    if not report["total_file_count"] and not report["state_counts"]:
        return payload, "<no source files>"
    preview_lines = [_sloc_totals_line(report)]
    preview_lines.extend(
        _sloc_language_preview_line(language) for language in shown_languages
    )
    if len(report["languages"]) > len(shown_languages):
        preview_lines.append(
            f"... [{len(report['languages']) - len(shown_languages)} more languages omitted]"
        )
    preview_lines.extend(
        _sloc_file_preview_line(file_summary) for file_summary in payload["top_files"]
    )
    if len(report["top_files"]) > len(shown_top_files):
        preview_lines.append(
            f"... [{len(report['top_files']) - len(shown_top_files)} more files omitted]"
        )
    preview_lines.extend(
        _sloc_state_preview_line(state_count) for state_count in report["state_counts"]
    )
    preview_lines.extend(
        f"[!] {rt._rel(root, Path(error['path']))}: {error['message']}"
        for error in report["errors"][:shown_limit]
    )
    return payload, "\n".join(preview_lines)


def _sloc_contents(
    root: Path,
    path: str,
    *,
    exclude: str | list[str] | None = None,
    limit: int,
):
    target = rt.resolve_path(root, path)
    if not target.exists():
        raise ValueError(f"sloc path does not exist: {rt._rel(root, target)}")
    report = sloc(target, ignore_root=root, exclude=exclude)
    return _sloc_payload(root, path, report, exclude=exclude, limit=limit), report

@tool()
def tool_sloc(
    state: Any,
    path: str = ".",
    exclude: str | list[str] | None = None,
    limit: int = rt.BUDGETS["default_line_limit"],
):
    defaults = {"path": ".", "exclude": None, "limit": rt.BUDGETS["default_line_limit"]}
    (payload, preview), report = _sloc_contents(
        state["root"], path, exclude=exclude, limit=limit
    )
    rt.note_tool(
        state,
        "sloc",
        _defaults=defaults,
        _suffix=_sloc_summary(report),
        path=path,
        exclude=exclude,
        limit=limit,
    )
    rt.show(preview)
    return payload


__all__ = [
    "TOOL_REGISTRY",
    "_TODO_STATUSES",
    "_WEBFETCH_ALLOWED_METHODS",
    "_bash_payload",
    "_collapse_repeated_lines",
    "_format_todos",
    "_todo_preview",
    "_html_to_markdown",
    "_iter_files",
    "_parse_bash_json_output",
    "_positive_int",
    "_render_selected_lines",
    "_shown_line_limit",
    "_summarize_json_output",
    "_summarize_text_output",
    "_tool_content_payload",
    "_validate_url_safe",
    "_webfetch_payload",
    "_webfetch_response_headers",
    "replace",
    "search",
    "sloc",
    "tool",
    "tool_ask",
    "tool_bash",
    "tool_list",
    "tool_read",
    "tool_replace",
    "tool_search",
    "tool_sloc",
    "tool_todo",
    "tool_webfetch",
]
