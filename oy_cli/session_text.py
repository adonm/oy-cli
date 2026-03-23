from __future__ import annotations

from functools import lru_cache
from importlib.resources import files
from typing import Any
import tomllib


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


__all__ = ["load_session_text", "session_text", "tool_description"]
