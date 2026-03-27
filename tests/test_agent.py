"""Tests for agent module: transcript, messages, run_turn."""
from __future__ import annotations

from oy_cli import agent
from oy_cli.providers import AssistantMessage, SystemMessage, ToolCall, ToolMessage, ToolResult, UserMessage
from tests.conftest import make_state, tool_handler


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
        monkeypatch.setattr(agent, "_packed_history_note", lambda messages: SystemMessage("packed"))

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
        assert packed == [SystemMessage("sys"), SystemMessage("packed"), UserMessage("mnop"), UserMessage("kl")]

    def test_tool_call_units_kept_together(self, monkeypatch):
        monkeypatch.setattr(agent, "count_tokens", lambda text: len(text))

        kept_as_unit = agent.prepared_messages(
            agent.transcript(
                messages=[
                    SystemMessage("sys"),
                    UserMessage("earlier"),
                    AssistantMessage("", tool_calls=[ToolCall(id="call_1", name="bash", arguments={})]),
                    ToolMessage("call_1", "bash", ToolResult(ok=True, content="tool output")),
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
        from oy_cli import runtime as rt, tools
        from oy_cli.providers import AssistantMessage, ToolCall, ToolMessage, ToolResult

        def echo(state, text: str):
            return f"{state['root'].name}:{text}"

        registry = tool_handler("echo", echo)
        transcript = agent.transcript_with_system_prompt("sys")
        agent.add_user(transcript, "hello")

        responses = iter([
            AssistantMessage("", tool_calls=[ToolCall(id="call_1", name="echo", arguments={"text": "hi"})]),
            AssistantMessage("done"),
        ])
        printed: list[str] = []
        monkeypatch.setattr(rt, "_print", lambda *a, value="", **k: printed.append(value))
        monkeypatch.setattr(rt, "_debug_log", lambda *a, **k: None)
        monkeypatch.setattr(rt, "_note", lambda *a, **k: None)

        client = {"chat_completion": lambda **kwargs: next(responses)}
        code, content = agent.run_turn(
            client,
            transcript,
            make_state(tmp_path, registry=registry),
            "openai:gpt-test",
            tools.tool_specs(registry),
        )

        assert (code, content) == (0, "done")
        assert printed == ["done"]
        assert transcript["messages"][2] == AssistantMessage(
            "", tool_calls=[ToolCall(id="call_1", name="echo", arguments={"text": "hi"})]
        )
        assert transcript["messages"][3] == ToolMessage(
            "call_1", "echo", ToolResult(content=f"{tmp_path.name}:hi"),
        )