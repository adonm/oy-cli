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
import sys
import tarfile
import zipfile
from dataclasses import dataclass
from functools import partial
from pathlib import Path
from typing import Any, BinaryIO, Callable, Iterable
from urllib.parse import urlparse

import httpx
import pathspec
from pygount import DuplicatePool, ProjectSummary, SourceAnalysis
import regex
import zstandard

import msgspec
from markdownify import markdownify

from . import runtime as rt
from .providers import ToolResult, ToolSpec, serialize_toon


class ListArgs(msgspec.Struct, omit_defaults=True):
    path: str = "*"
    limit: int = rt.BUDGETS.default_line_limit


class ReadArgs(msgspec.Struct, omit_defaults=True):
    path: str
    offset: int = 1
    limit: int = rt.BUDGETS.default_line_limit


class BashArgs(msgspec.Struct, omit_defaults=True):
    command: str
    timeout_seconds: int = 120


class SearchArgs(msgspec.Struct, omit_defaults=True):
    pattern: str
    path: str = "."
    limit: int = rt.BUDGETS.default_line_limit


class ReplaceArgs(msgspec.Struct, omit_defaults=True):
    pattern: str
    replacement: str
    path: str = "."
    limit: int = rt.BUDGETS.default_line_limit


class SlocArgs(msgspec.Struct, omit_defaults=True):
    path: str = "."
    limit: int = rt.BUDGETS.default_line_limit


class AskArgs(msgspec.Struct, omit_defaults=True):
    question: str
    choices: list[str] | None = None


class WebfetchOptions(msgspec.Struct, omit_defaults=True):
    follow_redirects: bool = False
    timeout_seconds: int = 30


class WebfetchArgs(msgspec.Struct, omit_defaults=True):
    url: str
    method: str = "GET"
    headers: dict[str, str] = msgspec.field(default_factory=dict)
    options: WebfetchOptions = msgspec.field(default_factory=WebfetchOptions)


_TODO_STATUSES = {"pending", "in_progress", "done"}


class TodoItem(msgspec.Struct, frozen=True):
    id: str
    task: str
    status: str = "pending"


class TodoArgs(msgspec.Struct, omit_defaults=True):
    todos: list[TodoItem]


@dataclass(frozen=True, slots=True)
class ToolHandler:
    name: str
    fn: Callable[..., Any]
    spec: ToolSpec
    args_type: Any

    def invoke(self, state: Any, args: dict[str, Any] | None = None) -> ToolResult:
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


def _tool_schema(args_type: Any):
    schema = msgspec.json.schema(args_type)

    def resolve(node: Any, defs: dict[str, Any]):
        if isinstance(node, list):
            return [resolve(item, defs) for item in node]
        if not isinstance(node, dict):
            return node
        if "$ref" in node and isinstance(node["$ref"], str):
            name = node["$ref"].removeprefix("#/$defs/")
            resolved = resolve(defs.get(name, {}), defs)
            extras = {
                key: resolve(value, defs)
                for key, value in node.items()
                if key not in {"$defs", "$ref"}
            }
            if isinstance(resolved, dict):
                resolved = {**resolved, **extras}
                resolved.pop("title", None)
            else:
                return resolved
        else:
            resolved = {
                key: resolve(value, defs)
                for key, value in node.items()
                if key != "$defs"
            }
            resolved.pop("title", None)

        if isinstance(resolved, dict):
            if "enum" in resolved and "type" not in resolved:
                if all(isinstance(value, str) for value in resolved["enum"]):
                    resolved["type"] = "string"
                elif all(isinstance(value, int) for value in resolved["enum"]):
                    resolved["type"] = "integer"
            if "properties" in resolved and "type" not in resolved:
                resolved["type"] = "object"

        return resolved

    return resolve(schema, schema.get("$defs", {}))


def tool(args_type: Any):
    def deco(fn: Callable[..., Any]):
        name = _tool_name(fn.__name__)
        _TOOLS[name] = ToolHandler(
            name=name,
            fn=fn,
            spec=ToolSpec(name, rt.tool_description(name), _tool_schema(args_type)),
            args_type=args_type,
        )
        return fn

    return deco


class ToolRegistry:
    def __init__(self, tools: dict[str, ToolHandler] | None = None):
        self._tools = _TOOLS if tools is None else tools

    @classmethod
    def select(
        cls,
        *,
        include: set[str] | frozenset[str] | None = None,
        exclude: set[str] | frozenset[str] = frozenset(),
    ):
        if include is None and not exclude:
            return cls()
        return cls(
            {
                name: tool
                for name, tool in _TOOLS.items()
                if (include is None or name in include) and name not in exclude
            }
        )

    def specs(self):
        return [tool.spec for tool in self._tools.values()]

    def invoke(self, state: Any, name: str, args: dict[str, Any] | None = None):
        return (
            handler.invoke(state, args)
            if (handler := self._tools.get(name))
            else ToolResult(ok=False, content=f"Tool '{name}' unavailable")
        )


TOOL_REGISTRY = ToolRegistry.select()


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
        if rt.count_tokens(rendered) <= rt.BUDGETS.tool_output_tokens:
            return rendered, truncated, "toon"
    rendered = rt.clip_tokens(
        serialize_toon(value),
        limit=rt.BUDGETS.tool_output_tokens,
        tail=rt.BUDGETS.tool_tail_tokens,
    )
    return rendered, True, "toon"


def _summarize_text_output(text: str) -> tuple[str, bool]:
    if not text:
        return "", False
    text = rt._truncate_long_lines(text)
    lines, collapsed = _collapse_repeated_lines(text.splitlines())
    rendered = "\n".join(lines)
    if len(lines) > 80 or rt.count_tokens(rendered) > rt.BUDGETS.tool_output_tokens:
        keep = set(range(min(30, len(lines))))
        keep.update(range(max(len(lines) - 20, 0), len(lines)))
        for idx, line in enumerate(lines):
            if rt._BASH_IMPORTANT_LINE_RE.search(line):
                keep.update({idx - 1, idx, idx + 1})
        rendered = _render_selected_lines(lines, keep)
        collapsed = True
    clipped = rt.clip_tokens(
        rendered,
        limit=rt.BUDGETS.tool_output_tokens,
        tail=rt.BUDGETS.tool_tail_tokens,
    )
    return clipped, collapsed or clipped != rendered


@tool(AskArgs)
def tool_ask(state: Any, question: str, choices: list[str] | None = None):
    rt.note_tool(state, "ask", question=question, choices=choices)
    if not sys.stdin.isatty():
        raise ValueError("Cannot ask question: stdin is not a TTY")
    rt._print("prompt", question, err=True)
    if not choices:
        return rt.Prompt.ask("Answer", console=rt.STDERR).strip()
    rt._print(
        value="## Options\n\n"
        + "\n".join(
            f"{i}. {rt._fmt('inline', choice)}" for i, choice in enumerate(choices, 1)
        ),
        err=True,
    )
    while True:
        response = rt.Prompt.ask("Selection", console=rt.STDERR).strip()
        if response.isdigit() and 0 < int(response) <= len(choices):
            return choices[int(response) - 1]
        if response in choices:
            return response
        rt._print(
            "warning",
            f"Enter a number 1-{len(choices)} or type the choice exactly.",
            err=True,
        )


def _format_todos(todos: list[TodoItem]) -> str:
    if not todos:
        return "<empty todo list>"
    status_icons = {"pending": "[ ]", "in_progress": "[~]", "done": "[x]"}
    return "\n".join(
        f"{status_icons.get(item.status, '[ ]')} {item.id}: {item.task}" for item in todos
    )


@tool(TodoArgs)
def tool_todo(state: Any, todos: list[TodoItem] | list[dict[str, Any]] | None = None):
    if todos is None:
        todos = []
    validated: list[TodoItem] = []
    for item in todos:
        if isinstance(item, dict):
            item = msgspec.convert(item, TodoItem)
        if item.status not in _TODO_STATUSES:
            raise ValueError(
                f"Invalid status {item.status!r} for todo {item.id!r}. "
                f"Use one of: {', '.join(sorted(_TODO_STATUSES))}"
            )
        validated.append(item)
    state.todos = validated
    result = _format_todos(state.todos)
    rt.note_tool(state, "todo", _suffix=f"({len(state.todos)} items)")
    rt.show(result)
    return result


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


def _bash_payload(command: str, result) -> dict[str, Any]:
    parsed = _parse_bash_json_output(result.stdout, result.stderr)
    if parsed is not None:
        content, truncated, content_format = _summarize_json_output(parsed)
    else:
        content, truncated = _summarize_text_output(
            _merge_bash_streams(result.stdout, result.stderr)
        )
        content_format = "text"
    return _tool_content_payload(
        command=command,
        exit_code=result.returncode,
        ok=result.returncode == 0,
        content=content,
        content_format=content_format,
        truncated=truncated,
    )


def _render_bash_preview(command: str, result, payload: dict[str, Any]) -> str:
    if payload.get("content_format") != "toon":
        return rt._fmt("bash", command, (result.stdout, result.returncode, result.stderr))

    toon_text = payload.get("content") or result.stdout
    blocks = [
        rt._fmt("block", f"$ {command}", "bash"),
        rt._fmt("block", toon_text, "text"),
    ]
    if result.returncode:
        blocks.append(rt._fmt("status", f"exit {result.returncode}"))
    if result.stderr.strip():
        blocks.extend(["**stderr**", rt._fmt("block", result.stderr.rstrip(), "text")])
    return "\n\n".join(blocks)


@tool(BashArgs)
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
    env = rt.require_command_env(state.root)
    bash_path = rt.which("bash", env.get("PATH"))
    if not bash_path:
        raise ValueError("bash is not installed or not on PATH")
    result = rt.run_cmd(
        [bash_path, "-c", command],
        cwd=state.root,
        env=env,
        timeout=timeout_seconds,
    )
    payload = _bash_payload(command, result)
    rt.show(_render_bash_preview(command, result, payload))
    return payload


def _validate_url_safe(url: str) -> str:
    parsed = urlparse(url)
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


def _webfetch_is_html_response(response: httpx.Response, text: str) -> bool:
    content_type = (
        response.headers.get("content-type", "").split(";", 1)[0].strip().lower()
    )
    if content_type in _WEBFETCH_HTML_CONTENT_TYPES:
        return True
    return text.lstrip().lower().startswith(("<!doctype html", "<html"))


def _html_to_markdown(text: str) -> str:
    return markdownify(text)


def _webfetch_summarize_response_body(
    response: httpx.Response,
) -> tuple[str, bool, str]:
    text = response.text
    content_format = "text"
    if (
        _webfetch_is_html_response(response, text)
        and rt.count_tokens(text) > rt.BUDGETS.tool_output_tokens
    ):
        markdown = _html_to_markdown(text)
        if markdown:
            text = markdown
            content_format = "markdown"
    summarized, truncated = _summarize_text_output(text)
    return summarized, truncated, content_format


def _webfetch_response_headers(response: httpx.Response) -> dict[str, str]:
    def display_name(name: str) -> str:
        return "-".join(part.capitalize() for part in name.split("-"))

    return {
        display_name(key): (
            "<redacted>"
            if key.lower() in _WEBFETCH_REDACTED_RESPONSE_HEADERS
            else response.headers[key]
        )
        for key in response.headers.keys()
    }


def _webfetch_structured_text(payload: dict[str, Any]) -> str:
    return serialize_toon(payload)


def _webfetch_response_text(response: httpx.Response, text: str | None = None) -> str:
    version = response.http_version or "HTTP/1.1"
    header_lines = [
        f"{key}: {value}" for key, value in _webfetch_response_headers(response).items()
    ]
    parts = [f"{version} {response.status_code} {response.reason_phrase}".rstrip()]
    if header_lines:
        parts.append("\n".join(header_lines))
    if text is None:
        text = response.text
    if text:
        parts.append(text)
    return "\n\n".join(parts)


def _webfetch_payload(
    response: httpx.Response,
    *,
    method: str,
    text: str,
    truncated: bool,
    content_format: str = "text",
) -> dict[str, Any]:
    return _tool_content_payload(
        method=method,
        url=str(response.url),
        ok=response.is_success,
        status_code=response.status_code,
        reason_phrase=response.reason_phrase,
        http_version=response.http_version or "HTTP/1.1",
        headers=_webfetch_response_headers(response),
        content=text,
        content_format=content_format,
        truncated=truncated,
    )


def _webfetch_error_payload(
    url: str, *, method: str, exc: httpx.HTTPError
) -> dict[str, Any]:
    return {
        "method": method,
        "url": url,
        "ok": False,
        "error_type": type(exc).__name__,
        "message": str(exc),
    }


@tool(WebfetchArgs)
def tool_webfetch(
    state: Any,
    url: str,
    method: str = "GET",
    headers: dict[str, str] | None = None,
    options: dict[str, Any] | WebfetchOptions | None = None,
):
    method = method.upper()
    if method not in _WEBFETCH_ALLOWED_METHODS:
        raise ValueError(
            f"Only {', '.join(sorted(_WEBFETCH_ALLOWED_METHODS))} methods are allowed, got: {method!r}"
        )
    _validate_url_safe(url)
    headers = _sanitize_webfetch_headers(headers)
    options = msgspec.convert(options or {}, WebfetchOptions)
    rt.note_tool(
        state,
        "webfetch",
        _defaults={
            "method": "GET",
            "headers": {},
            "follow_redirects": False,
            "timeout_seconds": 30,
        },
        url=url,
        method=method,
        headers=headers,
        follow_redirects=options.follow_redirects,
        timeout_seconds=options.timeout_seconds,
    )
    try:
        with rt.http_client(
            timeout=options.timeout_seconds,
            follow_redirects=options.follow_redirects,
        ) as client:
            response = client.request(method, url, headers=headers)
    except httpx.HTTPError as exc:
        payload = _webfetch_error_payload(url, method=method, exc=exc)
        rt.show(f"[!] {type(exc).__name__}: {exc}")
        return payload
    text, truncated, content_format = _webfetch_summarize_response_body(response)
    payload = _webfetch_payload(
        response,
        method=method,
        text=text,
        truncated=truncated,
        content_format=content_format,
    )
    rt.show(_webfetch_structured_text(payload))
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

@dataclass(frozen=True, slots=True)
class SearchMatch:
    source: str
    line_number: int | None = None
    text: str = ""
    error: str | None = None

@dataclass(frozen=True, slots=True)
class ReplaceResult:
    source: str
    replacements: int = 0
    skipped: str | None = None
    error: str | None = None

@dataclass(frozen=True, slots=True)
class SlocLanguage:
    language: str
    file_count: int
    code_count: int
    documentation_count: int
    empty_count: int
    string_count: int

@dataclass(frozen=True, slots=True)
class SlocStateCount:
    state: str
    file_count: int

@dataclass(frozen=True, slots=True)
class SlocError:
    path: str
    message: str

@dataclass(frozen=True, slots=True)
class SlocReport:
    total_file_count: int
    total_code_count: int
    total_documentation_count: int
    total_empty_count: int
    total_string_count: int
    total_line_count: int
    languages: tuple[SlocLanguage, ...]
    state_counts: tuple[SlocStateCount, ...]
    errors: tuple[SlocError, ...]

def _ignore_spec(root: Path) -> pathspec.PathSpec:
    patterns = [".git/"]
    gitignore = root / ".gitignore"
    if gitignore.is_file():
        patterns.extend(gitignore.read_text(encoding="utf-8", errors="replace").splitlines())
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

def _iter_files(target: Path, *, ignore_root: str | Path | None = None) -> list[Path]:
    root = _resolve_ignore_root(target, ignore_root)
    spec = _ignore_spec(root)
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

def _search_file(path: Path, compiled: regex.Pattern) -> list[SearchMatch]:
    matches: list[SearchMatch] = []
    for source, stream in _streams(path):
        try:
            with stream as handle:
                for line_number, raw_line in enumerate(handle, 1):
                    if compiled.search(raw_line):
                        text = raw_line.decode("utf-8", errors="replace").rstrip("\r\n")
                        matches.append(
                            SearchMatch(
                                source=source,
                                line_number=line_number,
                                text=text,
                            )
                        )
        except Exception as exc:
            matches.append(SearchMatch(source=source, error=str(exc)))
    return matches

def search(
    target: str | Path,
    pattern: str,
    *,
    threads: int | None = None,
    ignore_root: str | Path | None = None,
) -> list[SearchMatch]:
    path = Path(target).resolve()
    if not path.exists():
        raise ValueError(f"Path not found: {target}")
    if not (path.is_dir() or path.is_file()):
        raise ValueError(f"Unsupported search target: {target}")
    try:
        compiled = regex.compile(pattern.encode("utf-8"))
    except regex.error as exc:
        raise ValueError(f"Invalid search pattern: {exc}") from exc
    files = _iter_files(path, ignore_root=ignore_root)
    if not files:
        return []
    worker_count = min(len(files), max(1, threads or _DEFAULT_THREADS))
    if worker_count == 1:
        results: list[SearchMatch] = []
        for file_path in files:
            results.extend(_search_file(file_path, compiled))
        return results
    results: list[SearchMatch] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=worker_count) as pool:
        for batch in pool.map(partial(_search_file, compiled=compiled), files):
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
) -> ReplaceResult:
    if path.is_symlink():
        return ReplaceResult(source=str(path), skipped="symlink")
    if _is_archive(path):
        return ReplaceResult(source=str(path), skipped="archive")
    try:
        raw = path.read_bytes()
    except OSError as exc:
        return ReplaceResult(source=str(path), error=str(exc))
    if b"\x00" in raw:
        return ReplaceResult(source=str(path), skipped="binary file")
    try:
        original_text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        return ReplaceResult(source=str(path), error=f"cannot decode utf-8: {exc}")
    updated_text, replacements = compiled.subn(replacement, original_text)
    if replacements == 0:
        return ReplaceResult(source=str(path))
    try:
        with path.open("w", encoding="utf-8", newline="") as handle:
            handle.write(updated_text)
    except OSError as exc:
        return ReplaceResult(source=str(path), error=str(exc))
    return ReplaceResult(source=str(path), replacements=replacements)

def replace(
    target: str | Path,
    pattern: str,
    replacement: str,
    *,
    threads: int | None = None,
    ignore_root: str | Path | None = None,
) -> list[ReplaceResult]:
    path = Path(target).resolve()
    if not path.exists():
        raise ValueError(f"Path not found: {target}")
    if not (path.is_dir() or path.is_file()):
        raise ValueError(f"Unsupported replace target: {target}")
    try:
        compiled = regex.compile(pattern)
    except regex.error as exc:
        raise ValueError(f"Invalid replace pattern: {exc}") from exc
    files = _iter_files(path, ignore_root=ignore_root)
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

def sloc(
    target: str | Path,
    *,
    ignore_root: str | Path | None = None,
) -> SlocReport:
    path = Path(target).resolve()
    if not path.exists():
        raise ValueError(f"Path not found: {target}")
    if not (path.is_dir() or path.is_file()):
        raise ValueError(f"Unsupported sloc target: {target}")

    summary = ProjectSummary()
    duplicate_pool = DuplicatePool()
    state_counts: Counter[str] = Counter()
    errors: list[SlocError] = []
    group = path.name if path.is_dir() else (path.parent.name or ".")

    for file_path in _iter_files(path, ignore_root=ignore_root):
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
            continue
        state_counts[analysis.state.name] += 1
        if analysis.state.name == "error":
            errors.append(
                SlocError(
                    path=str(file_path),
                    message=analysis.state_info or "unknown pygount error",
                )
            )

    languages = sorted(
        (
            SlocLanguage(
                language=language_summary.language,
                file_count=language_summary.file_count,
                code_count=language_summary.code_count,
                documentation_count=language_summary.documentation_count,
                empty_count=language_summary.empty_count,
                string_count=language_summary.string_count,
            )
            for language_summary in summary.language_to_language_summary_map.values()
            if not language_summary.is_pseudo_language
        ),
        key=lambda item: (-item.code_count, -item.file_count, item.language.lower()),
    )
    ordered_states = tuple(
        SlocStateCount(state=state, file_count=file_count)
        for state, file_count in sorted(
            state_counts.items(), key=lambda item: (-item[1], item[0])
        )
    )

    return SlocReport(
        total_file_count=summary.total_file_count,
        total_code_count=summary.total_code_count,
        total_documentation_count=summary.total_documentation_count,
        total_empty_count=summary.total_empty_count,
        total_string_count=summary.total_string_count,
        total_line_count=summary.total_line_count,
        languages=tuple(languages),
        state_counts=ordered_states,
        errors=tuple(errors),
    )

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

@tool(ListArgs)
def tool_list(state: Any, path: str = "*", limit: int = rt.BUDGETS.default_line_limit):
    rt.note_tool(
        state,
        "list",
        _defaults={"path": "*", "limit": rt.BUDGETS.default_line_limit},
        path=path,
        limit=limit,
    )
    text = _path_listing(_glob_paths(state.root, path), state.root, limit=limit)
    return rt._show_and_clip(text)

@tool(ReadArgs)
def tool_read(
    state: Any, path: str, offset: int = 1, limit: int = rt.BUDGETS.default_line_limit
):
    rt.note_tool(
        state,
        "read",
        _defaults={"offset": 1, "limit": rt.BUDGETS.default_line_limit},
        path=path,
        offset=offset,
        limit=limit,
    )
    target = rt.resolve_path(state.root, path)
    if not target.exists():
        raise ValueError(f"read path does not exist: {rt._rel(state.root, target)}")
    if target.is_dir():
        text = _path_listing(
            sorted(target.iterdir(), key=lambda item: item.as_posix()),
            state.root,
            limit=limit,
            empty="<empty directory>",
        )
        return rt._show_and_clip(text)
    start = max(_positive_int(offset, "offset"), 1) - 1
    lines = target.read_text(encoding="utf-8", errors="replace").splitlines()
    shown = lines[start : start + _shown_line_limit(limit)]
    text = "\n".join(
        f"{lineno}: {line}" for lineno, line in enumerate(shown, start + 1)
    )
    return rt._show_and_clip(text or "<empty file>")

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

def _search_preview_line(root: Path, match: SearchMatch) -> str:
    path_text = _search_display_path(root, match.source)
    if match.error:
        return f"[!] {path_text}: {match.error}"
    text = rt._truncate_long_lines(match.text)
    return f"{path_text}:{match.line_number}:1:{text}"

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
    results: list[SearchMatch],
    *,
    limit: int,
) -> tuple[dict[str, Any], str, int, int, int]:
    matches = [match for match in results if not match.error]
    errors = [match for match in results if match.error]
    shown_limit = _shown_line_limit(limit)
    shown_matches = matches[:shown_limit]
    payload: dict[str, Any] = {
        "pattern": pattern,
        "path": path,
        "match_count": len(matches),
        "matches": [
            {
                "path": _search_display_path(root, match.source),
                "line_number": match.line_number,
                "column": 1,
                "text": rt._truncate_long_lines(match.text),
            }
            for match in shown_matches
        ],
        "truncated": len(matches) > len(shown_matches),
    }
    payload.update(_optional_counts(error=len(errors)))
    if errors:
        payload["errors"] = [
            {"path": _search_display_path(root, match.source), "message": match.error}
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

def _search_contents(root: Path, pattern: str, path: str, *, limit: int):
    target = rt.resolve_path(root, path)
    if not target.exists():
        raise ValueError(f"search path does not exist: {rt._rel(root, target)}")
    results = search(target, pattern, ignore_root=root)
    return _search_payload(root, pattern, path, results, limit=limit)

@tool(SearchArgs)
def tool_search(
    state: Any, pattern: str, path: str = ".", limit: int = rt.BUDGETS.default_line_limit
):
    defaults = {"path": ".", "limit": rt.BUDGETS.default_line_limit}
    payload, preview, matches, shown, errors = _search_contents(
        state.root, pattern, path, limit=limit
    )
    rt.note_tool(
        state,
        "search",
        _defaults=defaults,
        _suffix=_search_summary(matches, shown, errors),
        pattern=pattern,
        path=path,
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

def _replace_preview_line(root: Path, result: ReplaceResult) -> str:
    path_text = rt._rel(root, Path(result.source))
    if result.error:
        return f"[!] {path_text}: {result.error}"
    if result.skipped:
        return f"[-] {path_text}: skipped ({result.skipped})"
    return f"{path_text}: {result.replacements} replacement(s)"

def _replace_payload(
    root: Path,
    pattern: str,
    replacement: str,
    path: str,
    results: list[ReplaceResult],
    *,
    limit: int,
) -> tuple[dict[str, Any], str, int, int, int, int]:
    changed = [result for result in results if result.replacements]
    skipped = [result for result in results if result.skipped]
    errors = [result for result in results if result.error]
    shown_limit = _shown_line_limit(limit)
    shown_changed = changed[:shown_limit]
    replacement_count = sum(result.replacements for result in changed)
    payload: dict[str, Any] = {
        "pattern": pattern,
        "replacement": replacement,
        "path": path,
        "changed_file_count": len(changed),
        "replacement_count": replacement_count,
        "changed_files": [
            {
                "path": rt._rel(root, Path(result.source)),
                "replacements": result.replacements,
            }
            for result in shown_changed
        ],
        "truncated": len(changed) > len(shown_changed),
    }
    payload.update(_optional_counts(skipped=len(skipped), error=len(errors)))
    if skipped:
        payload["skipped"] = [
            {"path": rt._rel(root, Path(result.source)), "reason": result.skipped}
            for result in skipped[:shown_limit]
        ]
    if errors:
        payload["errors"] = [
            {"path": rt._rel(root, Path(result.source)), "message": result.error}
            for result in errors[:shown_limit]
        ]
    preview_lines = [_replace_preview_line(root, result) for result in shown_changed]
    if skipped:
        preview_lines.extend(
            _replace_preview_line(root, result) for result in skipped[:shown_limit]
        )
    if errors:
        preview_lines.extend(
            _replace_preview_line(root, result) for result in errors[:shown_limit]
        )
    preview = "\n".join(preview_lines) if preview_lines else "<no changes>"
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
    root: Path, pattern: str, replacement: str, path: str, *, limit: int
):
    target = rt.resolve_path(root, path)
    if not target.exists():
        raise ValueError(f"replace path does not exist: {rt._rel(root, target)}")
    results = replace(target, pattern, replacement, ignore_root=root)
    return _replace_payload(root, pattern, replacement, path, results, limit=limit)

@tool(ReplaceArgs)
def tool_replace(
    state: Any,
    pattern: str,
    replacement: str,
    path: str = ".",
    limit: int = rt.BUDGETS.default_line_limit,
):
    defaults = {"path": ".", "limit": rt.BUDGETS.default_line_limit}
    payload, preview, changed, replacements, skipped, errors = _replace_contents(
        state.root, pattern, replacement, path, limit=limit
    )
    rt.note_tool(
        state,
        "replace",
        _defaults=defaults,
        _suffix=_replace_summary(changed, replacements, skipped, errors),
        pattern=pattern,
        replacement=replacement,
        path=path,
        limit=limit,
    )
    rt.show(preview)
    return payload

def _sloc_summary(report: SlocReport) -> str:
    parts = []
    if report.total_file_count:
        plural = "s" if report.total_file_count != 1 else ""
        parts.append(f"{report.total_file_count} file{plural}")
    if report.total_code_count:
        parts.append(f"{report.total_code_count} code lines")
    non_countable = sum(item.file_count for item in report.state_counts)
    if non_countable:
        parts.append(f"{non_countable} non-countable")
    if report.errors:
        plural = "s" if len(report.errors) != 1 else ""
        parts.append(f"{len(report.errors)} error{plural}")
    return "(" + "; ".join(parts) + ")" if parts else "(no source files)"

def _sloc_totals_line(report: SlocReport) -> str:
    return (
        "totals: "
        f"{report.total_file_count} files, "
        f"{report.total_code_count} code, "
        f"{report.total_documentation_count} comments, "
        f"{report.total_empty_count} empty, "
        f"{report.total_string_count} strings, "
        f"{report.total_line_count} lines"
    )

def _sloc_language_preview_line(language: Any) -> str:
    file_plural = "s" if language.file_count != 1 else ""
    return (
        f"{language.language}: "
        f"{language.code_count} code, "
        f"{language.documentation_count} comments, "
        f"{language.empty_count} empty, "
        f"{language.string_count} strings "
        f"({language.file_count} file{file_plural})"
    )

def _sloc_state_preview_line(state_count: Any) -> str:
    file_plural = "s" if state_count.file_count != 1 else ""
    return f"other/{state_count.state}: {state_count.file_count} file{file_plural}"

def _sloc_payload(
    root: Path, path: str, report: SlocReport, *, limit: int
) -> tuple[dict[str, Any], str]:
    shown_limit = _shown_line_limit(limit)
    shown_languages = list(report.languages[:shown_limit])
    payload: dict[str, Any] = {
        "path": path,
        "total_file_count": report.total_file_count,
        "total_code_count": report.total_code_count,
        "total_documentation_count": report.total_documentation_count,
        "total_empty_count": report.total_empty_count,
        "total_string_count": report.total_string_count,
        "total_line_count": report.total_line_count,
        "language_count": len(report.languages),
        "languages": [
            {
                "language": language.language,
                "file_count": language.file_count,
                "code_count": language.code_count,
                "documentation_count": language.documentation_count,
                "empty_count": language.empty_count,
                "string_count": language.string_count,
            }
            for language in shown_languages
        ],
        "truncated": len(report.languages) > len(shown_languages),
    }
    if report.state_counts:
        payload["state_counts"] = [
            {"state": state_count.state, "file_count": state_count.file_count}
            for state_count in report.state_counts
        ]
    payload.update(_optional_counts(error=len(report.errors)))
    if report.errors:
        payload["errors"] = [
            {"path": rt._rel(root, Path(error.path)), "message": error.message}
            for error in report.errors[:shown_limit]
        ]
    if not report.total_file_count and not report.state_counts:
        return payload, "<no source files>"
    preview_lines = [_sloc_totals_line(report)]
    preview_lines.extend(
        _sloc_language_preview_line(language) for language in shown_languages
    )
    if len(report.languages) > len(shown_languages):
        preview_lines.append(
            f"... [{len(report.languages) - len(shown_languages)} more languages omitted]"
        )
    preview_lines.extend(
        _sloc_state_preview_line(state_count) for state_count in report.state_counts
    )
    preview_lines.extend(
        f"[!] {rt._rel(root, Path(error.path))}: {error.message}"
        for error in report.errors[:shown_limit]
    )
    return payload, "\n".join(preview_lines)

def _sloc_contents(root: Path, path: str, *, limit: int):
    target = rt.resolve_path(root, path)
    if not target.exists():
        raise ValueError(f"sloc path does not exist: {rt._rel(root, target)}")
    report = sloc(target, ignore_root=root)
    return _sloc_payload(root, path, report, limit=limit), report

@tool(SlocArgs)
def tool_sloc(state: Any, path: str = ".", limit: int = rt.BUDGETS.default_line_limit):
    defaults = {"path": ".", "limit": rt.BUDGETS.default_line_limit}
    (payload, preview), report = _sloc_contents(state.root, path, limit=limit)
    rt.note_tool(
        state,
        "sloc",
        _defaults=defaults,
        _suffix=_sloc_summary(report),
        path=path,
        limit=limit,
    )
    rt.show(preview)
    return payload


__all__ = [
    "AskArgs",
    "BashArgs",
    "ListArgs",
    "ReadArgs",
    "ReplaceArgs",
    "ReplaceResult",
    "SearchArgs",
    "SearchMatch",
    "SlocArgs",
    "SlocReport",
    "TodoArgs",
    "TodoItem",
    "TOOL_REGISTRY",
    "ToolHandler",
    "ToolRegistry",
    "WebfetchArgs",
    "WebfetchOptions",
    "_TODO_STATUSES",
    "_WEBFETCH_ALLOWED_METHODS",
    "_bash_payload",
    "_collapse_repeated_lines",
    "_format_todos",
    "_html_to_markdown",
    "_iter_files",
    "_parse_bash_json_output",
    "_positive_int",
    "_render_bash_preview",
    "_render_selected_lines",
    "_shown_line_limit",
    "_summarize_json_output",
    "_summarize_text_output",
    "_tool_content_payload",
    "_validate_url_safe",
    "_webfetch_payload",
    "_webfetch_response_headers",
    "_webfetch_response_text",
    "_webfetch_structured_text",
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
