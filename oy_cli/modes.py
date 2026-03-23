from __future__ import annotations

from .session_text import session_text

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

__all__ = [
    "AUDIT_SYSTEM_PROMPT",
    "BASE_SYSTEM_PROMPT",
    "INTERACTIVE_SYSTEM_PROMPT",
    "NONINTERACTIVE_SYSTEM_PROMPT",
    "_READ_ONLY_TOOLS",
    "active_system_prompt",
    "active_tool_specs",
    "ask_system_prompt",
    "audit_system_prompt",
    "base_system_prompt",
    "interactive_system_prompt",
    "noninteractive_system_prompt",
    "read_only_tool_specs",
]
