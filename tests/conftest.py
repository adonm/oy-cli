"""Shared fixtures and helpers for oy-cli tests."""
from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from oy_cli import agent, providers, runtime as rt, tools
from oy_cli.providers import (
    AssistantMessage, SystemMessage, ToolCall, ToolMessage, ToolResult, UserMessage,
)


@pytest.fixture(autouse=True)
def _reset_provider_state():
    providers._REASONING_SUPPORT_CACHE.clear()
    providers.command_env.cache_clear()
    yield
    providers._REASONING_SUPPORT_CACHE.clear()
    providers.command_env.cache_clear()


def make_state(
    root: Path,
    *,
    interactive: bool = False,
    yolo: bool = False,
    registry: dict[str, dict[str, object]] | None = None,
):
    return agent.agent_state(
        root=root,
        tool_registry=tools.TOOL_REGISTRY if registry is None else registry,
        unattended_timeout_seconds=3600,
        unattended_deadline=float("inf"),
        interactive=interactive,
        approve_all_mutating_tools=yolo,
        yolo=yolo,
    )


def raw_response(**overrides):
    data = {
        "status_code": 200,
        "headers": {"Content-Type": "text/plain"},
        "text": "hello world",
        "content": b"hello world",
        "url": "https://example.com",
        "reason": "OK",
        "http_version": 2,
    }
    data.update(overrides)
    return SimpleNamespace(**data)


def api_error(message: str, *, status_code: int = 400):
    return providers.APIStatusError(
        message,
        response=providers.response_adapter(
            status_code=status_code,
            headers={},
            text=json.dumps({"error": {"message": message}}),
            content=b"",
            url="https://example.com",
            reason_phrase="Bad Request",
        ),
        body=None,
    )


def tool_handler(name: str, fn, *, mutating: bool = False):
    return {
        name: {
            "name": name,
            "fn": fn,
            "description": name,
            "parameters": {"type": "object"},
            "mutating": mutating,
        }
    }


class DummyHttpClient:
    def __init__(self, response=None, error=None, **kwargs):
        self.response = response
        self.error = error
        self.kwargs = kwargs
        self.called = None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False

    def request(self, method, url, headers=None):
        self.called = (method, url, headers)
        if self.error:
            raise self.error
        return self.response


__all__ = [
    "make_state", "raw_response", "api_error", "tool_handler", "DummyHttpClient",
    "AssistantMessage", "SystemMessage", "ToolCall", "ToolMessage", "ToolResult", "UserMessage",
]