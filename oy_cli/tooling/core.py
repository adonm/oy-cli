from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable

import msgspec

from ..protocol import ToolResult, ToolSpec
from ..runtime import BUDGETS
from ..session_text import tool_description

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
    limit: int = BUDGETS.default_line_limit

class ReplaceArgs(msgspec.Struct, omit_defaults=True):
    pattern: str
    replacement: str
    path: str = "."
    limit: int = BUDGETS.default_line_limit

class SlocArgs(msgspec.Struct, omit_defaults=True):
    path: str = "."
    limit: int = BUDGETS.default_line_limit

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
        description = tool_description(name)
        _TOOLS[name] = ToolHandler(
            name=name,
            fn=fn,
            spec=ToolSpec(name, description, _tool_schema(args_type)),
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

__all__ = [
    "AskArgs",
    "BashArgs",
    "ListArgs",
    "ReadArgs",
    "ReplaceArgs",
    "SearchArgs",
    "SlocArgs",
    "TodoArgs",
    "TodoItem",
    "TOOL_REGISTRY",
    "ToolHandler",
    "ToolRegistry",
    "WebfetchArgs",
    "WebfetchOptions",
    "_TODO_STATUSES",
    "_positive_int",
    "tool",
]
