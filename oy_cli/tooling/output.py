from __future__ import annotations

import json
import re
from typing import Any

from .. import runtime as rt
from ..serialization import serialize_toon

_BASH_IMPORTANT_LINE_RE = re.compile(
    r"(?i)(error|warn(?:ing)?|fail(?:ed|ure)?|exception|traceback|fatal|denied|not found|timed out)"
)

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
            omitted = idx - last - 1
            selected.append(f"... [{omitted} lines omitted]")
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
            if _BASH_IMPORTANT_LINE_RE.search(line):
                keep.update({idx - 1, idx, idx + 1})
        rendered = _render_selected_lines(lines, keep)
        collapsed = True
    clipped = rt.clip_tokens(
        rendered,
        limit=rt.BUDGETS.tool_output_tokens,
        tail=rt.BUDGETS.tool_tail_tokens,
    )
    return clipped, collapsed or clipped != rendered

__all__ = [
    "_parse_bash_json_output",
    "_shown_line_limit",
    "_summarize_json_output",
    "_summarize_text_output",
    "_tool_content_payload",
]
