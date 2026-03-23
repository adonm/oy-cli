from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Awaitable, Callable, TypeAlias

import msgspec

from .serialization import JSONLike


class ToolCall(msgspec.Struct, omit_defaults=True):
    id: str
    name: str
    arguments: dict[str, Any] = msgspec.field(default_factory=dict)


class ToolResult(msgspec.Struct, omit_defaults=True):
    ok: bool = True
    content: JSONLike = None


class ToolSpec(msgspec.Struct, omit_defaults=True):
    name: str
    description: str
    parameters: dict[str, Any]


class SystemMessage(msgspec.Struct, tag="system", tag_field="role", omit_defaults=True):
    content: str


class UserMessage(msgspec.Struct, tag="user", tag_field="role", omit_defaults=True):
    content: str


class AssistantMessage(
    msgspec.Struct, tag="assistant", tag_field="role", omit_defaults=True
):
    content: str = ""
    tool_calls: list[ToolCall] = msgspec.field(default_factory=list)
    thought_signatures: dict[str, str] = msgspec.field(default_factory=dict)


class ToolMessage(msgspec.Struct, tag="tool", tag_field="role", omit_defaults=True):
    tool_call_id: str
    name: str = ""
    content: ToolResult = msgspec.field(default_factory=ToolResult)


ChatMessage: TypeAlias = SystemMessage | UserMessage | AssistantMessage | ToolMessage


@dataclass(frozen=True, slots=True)
class CompletionClient:
    chat_completion: Callable[
        [str, list[ChatMessage], list[ToolSpec] | None, str, Any],
        Awaitable[AssistantMessage],
    ]
    list_models: Callable[[], list[str]]


__all__ = [
    "AssistantMessage",
    "ChatMessage",
    "CompletionClient",
    "SystemMessage",
    "ToolCall",
    "ToolMessage",
    "ToolResult",
    "ToolSpec",
    "UserMessage",
]
