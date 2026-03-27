"""Tests for CLI module: main, chat, load/save."""
from __future__ import annotations

import json

from oy_cli import agent, cli, runtime as rt
from oy_cli.providers import AssistantMessage, SystemMessage, UserMessage
from tests.conftest import tool_handler


class TestCLI:
    def test_main_wraps_bare_args(self, monkeypatch):
        seen = {}
        def fake_run(functions, *, argv, **kwargs):
            seen["argv"] = argv
            return 0
        monkeypatch.setattr(cli.defopt, "run", fake_run)
        assert cli.main(["fix", "tests"]) == 0
        assert seen["argv"] == ["run", "fix", "tests"]

    def test_main_rejects_top_level_yolo(self, monkeypatch):
        monkeypatch.delenv("OY_YOLO", raising=False)
        import pytest
        with pytest.raises(SystemExit):
            cli.main(["--yolo", "fix", "tests"])
        assert rt.yolo_enabled() is False


class TestChatCommands:
    def test_load_and_chat_commands(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path)
        monkeypatch.setattr(rt, "_note", lambda *a, **k: None)
        monkeypatch.setattr(rt, "_print", lambda *a, **k: None)

        saved = {
            "model": "openai:gpt-test",
            "saved_at": "2026-03-25T12:34:56",
            "transcript": cli._transcript_data(
                agent.transcript(messages=[SystemMessage("old"), UserMessage("hello")])
            ),
        }
        (tmp_path / "saved.json").write_text(json.dumps(saved), encoding="utf-8")

        loaded, model = cli._handle_load(
            "saved",
            agent.transcript_with_system_prompt("sys"),
            "openai:gpt-old",
            "new system",
        )
        assert model == "openai:gpt-test"
        assert loaded["messages"] == [SystemMessage("new system"), UserMessage("hello")]
        assert cli._chat_command("/yolo", loaded, "new system", model) == ("yolo",)
        assert cli._chat_command("/clear", loaded, "new system", model) is True
        assert loaded["messages"] == [SystemMessage("new system")]


class TestChatRollback:
    def test_rolls_back_on_agent_error(self, tmp_path, monkeypatch):
        inputs = iter(["hello", "quit"])
        rollback_calls = []
        errors = []

        monkeypatch.setattr(cli, "_create_prompt_session", lambda: object())
        monkeypatch.setattr(
            cli, "resolve_session",
            lambda **kwargs: {
                "workspace": tmp_path,
                "model": "openai:gpt-test",
                "interactive": True,
                "system_prompt": "sys",
                "system_file": None,
                "yolo": False,
            },
        )
        monkeypatch.setattr(cli, "_print_session_intro", lambda *a, **k: None)
        monkeypatch.setattr(cli, "_set_terminal_title", lambda *a, **k: None)
        monkeypatch.setattr(cli, "_read_input", lambda *a, **k: next(inputs))
        monkeypatch.setattr(cli, "checkpoint", lambda tx: 7)
        monkeypatch.setattr(cli, "rollback", lambda tx, point: rollback_calls.append(point))
        monkeypatch.setattr(cli, "run_agent", lambda *a, **k: (_ for _ in ()).throw(RuntimeError("boom")))
        monkeypatch.setattr(rt, "print_console", lambda *a, **k: None)
        monkeypatch.setattr(rt, "rule_console", lambda *a, **k: None)
        monkeypatch.setattr(rt, "_note", lambda *a, **k: None)
        monkeypatch.setattr(rt, "_error", errors.append)

        assert cli.chat() == 0
        assert rollback_calls == [7]
        assert errors == ["Agent error: boom"]