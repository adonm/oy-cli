"""Tests for agent module: transcript, messages, run_turn."""

from __future__ import annotations

from oy_cli import agent, tools
from oy_cli.providers import (
    AssistantMessage,
    SystemMessage,
    ToolCall,
    ToolMessage,
    ToolResult,
    UserMessage,
)
from tests.conftest import make_state, patch_runtime, tool_handler


def _call(id: str, name: str, **arguments) -> ToolCall:
    return ToolCall(id=id, name=name, arguments=arguments)


def _assistant_call(id: str, name: str, **arguments) -> AssistantMessage:
    return AssistantMessage("", tool_calls=[_call(id, name, **arguments)])


def _transcript(text: str = "hello"):
    tx = agent.transcript_with_system_prompt("sys")
    agent.add_user(tx, text)
    return tx


class TestTranscriptLifecycle:
    def test_basic_operations(self):
        tx = agent.transcript_with_system_prompt("sys")
        agent.add_user(tx, "hello")
        agent.clear_transcript(tx, "next")
        assert tx["messages"] == [SystemMessage("next")]
        assert agent.undo_last_turn(tx) is False

    def test_prepared_messages_truncate(self, monkeypatch):
        monkeypatch.setattr(agent, "count_tokens", lambda text: len(text))
        truncated = agent.prepared_messages(
            agent.transcript(
                messages=[
                    SystemMessage("sys"),
                    UserMessage("abcdef"),
                    UserMessage("ghij"),
                    UserMessage("kl"),
                ],
                max_context_tokens=18,
                max_message_tokens=100,
            )
        )
        assert truncated[0] == SystemMessage("sys")
        assert truncated[1]["role"] == "user"
        assert "earlier messages omitted" in truncated[1]["content"]
        assert truncated[-1] == UserMessage("kl")

    def test_prepared_messages_pack_with_toons(self, monkeypatch):
        monkeypatch.setattr(agent, "count_tokens", lambda text: len(text))
        monkeypatch.setattr(
            agent, "_packed_history_note", lambda messages: SystemMessage("packed")
        )
        packed = agent.prepared_messages(
            agent.transcript(
                messages=[
                    SystemMessage("sys"),
                    UserMessage("abcdef"),
                    UserMessage("ghij"),
                    UserMessage("mnop"),
                    UserMessage("kl"),
                ],
                max_context_tokens=80,
                max_message_tokens=100,
            ),
            model="gpt-4o",
        )
        assert packed == [
            SystemMessage("sys"),
            SystemMessage("packed"),
            UserMessage("mnop"),
            UserMessage("kl"),
        ]

    def test_tool_call_units_kept_together(self, monkeypatch):
        monkeypatch.setattr(agent, "count_tokens", lambda text: len(text))
        kept_as_unit = agent.prepared_messages(
            agent.transcript(
                messages=[
                    SystemMessage("sys"),
                    UserMessage("earlier"),
                    _assistant_call("call_1", "bash"),
                    ToolMessage(
                        "call_1", "bash", ToolResult(ok=True, content="tool output")
                    ),
                    UserMessage("tail"),
                ],
                max_context_tokens=23,
                max_message_tokens=100,
            )
        )
        assert kept_as_unit == [
            SystemMessage("sys"),
            UserMessage("... [3 earlier messages omitted to fit context limit]"),
            UserMessage("tail"),
        ]


class TestRunTurn:
    def test_executes_tool_calls_until_final_answer(self, monkeypatch, tmp_path):
        registry = tool_handler(
            "echo", lambda state, text: f"{state['root'].name}:{text}"
        )
        responses = iter(
            [_assistant_call("call_1", "echo", text="hi"), AssistantMessage("done")]
        )
        printed: list[str] = []
        patch_runtime(
            monkeypatch,
            _print=lambda *a, value="", **k: printed.append(value),
            _debug_log=None,
            _note=None,
        )

        transcript = _transcript()
        code, content = agent.run_turn(
            {"chat_completion": lambda **kwargs: next(responses)},
            transcript,
            make_state(tmp_path, registry=registry),
            "openai:gpt-test",
            tools.tool_specs(registry),
        )

        assert (code, content) == (0, "done")
        assert printed == ["done"]
        assert transcript["messages"][2] == _assistant_call("call_1", "echo", text="hi")
        assert transcript["messages"][3] == ToolMessage(
            "call_1", "echo", ToolResult(content=f"{tmp_path.name}:hi")
        )

