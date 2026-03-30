"""Tests for CLI module: main, chat, load/save."""

from __future__ import annotations

import json

import pytest

from oy_cli import agent, cli, runtime as rt
from oy_cli.providers import SystemMessage, UserMessage
from tests.conftest import patch_runtime


def _stub_session(monkeypatch, tmp_path, *, interactive=False):
    monkeypatch.setattr(
        cli,
        "resolve_session",
        lambda **kwargs: {
            "workspace": tmp_path,
            "model": "openai:gpt-test",
            "interactive": interactive,
            "system_prompt": "sys",
            "system_file": None,
            "yolo": False,
            "best_of": 3,
        },
    )


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
        with pytest.raises(SystemExit):
            cli.main(["--yolo", "fix", "tests"])
        assert rt.yolo_enabled() is False


class TestRalph:
    def test_main_accepts_ralph_command(self, monkeypatch):
        seen = {}

        def fake_run(functions, *, argv, **kwargs):
            seen["argv"] = argv
            return 0

        monkeypatch.setattr(cli.defopt, "run", fake_run)
        assert cli.main(["ralph", "fix", "tests"]) == 0
        assert seen["argv"] == ["ralph", "fix", "tests"]

    def test_ralph_runs_prompt_until_deadline(self, tmp_path, monkeypatch):
        notes = []
        sleeps = []
        calls = []
        intro = {}
        monotonic_values = iter([0, 0, 60, 60, 120, 120])

        _stub_session(monkeypatch, tmp_path)
        monkeypatch.setattr(
            cli, "_print_session_intro", lambda *a, **k: intro.update(k)
        )
        patch_runtime(
            monkeypatch, _note=lambda *a, **k: notes.append((a, k)), _print=None
        )
        monkeypatch.setattr(rt, "ralph_limit_seconds", lambda default=10800: 120)
        monkeypatch.setattr(cli.time, "monotonic", lambda: next(monotonic_values))
        monkeypatch.setattr(cli.time, "sleep", lambda seconds: sleeps.append(seconds))

        def fake_run_agent(*args, **kwargs):
            calls.append((args, kwargs))
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)

        assert cli.ralph("fix", "tests") == 0
        assert len(calls) == 2
        assert all(call[1].get("yolo") is True for call in calls)
        assert all(call[1].get("best_of") == 3 for call in calls)
        assert all(call[0][0] == "fix tests" for call in calls)
        assert sleeps == [60]
        assert intro["schedule"] == "until 2m deadline, 1m delay"
        assert notes[0][0][0] == "ralph run 1 (~2m remaining)"
        assert notes[1][0][0] == "ralph run 2 (~1m remaining)"

    def test_ralph_reads_stdin_when_task_missing(self, tmp_path, monkeypatch):
        calls = []
        monotonic_values = iter([0, 0, 1])

        monkeypatch.setattr(cli.sys.stdin, "read", lambda: "from stdin")
        monkeypatch.setattr(rt, "has_tty_stdin", lambda: False)
        _stub_session(monkeypatch, tmp_path)
        monkeypatch.setattr(cli, "_print_session_intro", lambda *a, **k: None)
        patch_runtime(monkeypatch, _note=None, _print=None)
        monkeypatch.setattr(rt, "ralph_limit_seconds", lambda default=10800: 1)
        monkeypatch.setattr(cli.time, "monotonic", lambda: next(monotonic_values))
        monkeypatch.setattr(cli.time, "sleep", lambda seconds: None)
        monkeypatch.setattr(
            cli,
            "run_agent",
            lambda *args, **kwargs: calls.append((args, kwargs)) or (0, ""),
        )

        assert cli.ralph() == 0
        assert len(calls) == 1
        assert calls[0][1].get("best_of") == 3
        assert calls[0][0][0] == "from stdin"

    def test_ralph_limit_seconds_parses_env(self, monkeypatch):
        monkeypatch.setenv("OY_RALPH_LIMIT", "90m")
        assert rt.ralph_limit_seconds() == 5400


class TestChatCommands:
    def test_ask_usage_and_note_are_explicit_about_webfetch(
        self, tmp_path, monkeypatch
    ):
        printed = []
        notes = []
        patch_runtime(
            monkeypatch,
            _note=lambda message, **k: notes.append(message),
            _print=lambda *a, **k: printed.append(
                k.get("value", a[1] if len(a) > 1 else a[0] if a else "")
            ),
        )
        monkeypatch.setattr(
            cli,
            "read_only_tool_registry",
            lambda: {"list": object(), "webfetch": object()},
        )
        monkeypatch.setattr(
            cli, "transcript_with_system_prompt", lambda prompt: {"messages": []}
        )
        monkeypatch.setattr(cli, "new_agent_state", lambda **k: {"state": True})
        monkeypatch.setattr(
            cli, "add_user", lambda tx, question: tx.update({"question": question})
        )
        monkeypatch.setattr(rt, "unattended_limit_seconds", lambda: 60)
        monkeypatch.setattr(rt, "get_client", lambda model: object())
        monkeypatch.setattr(cli, "tool_specs", lambda registry: [])
        seen = {}
        monkeypatch.setattr(
            cli, "run_turn", lambda *args, **kwargs: seen.update({"called": True})
        )

        cli._handle_ask(
            "",
            "openai:gpt-test",
            {
                "workspace": tmp_path,
                "system_prompt": "sys",
                "interactive": True,
                "best_of": 1,
            },
            {"messages": []},
        )
        assert (
            printed[-1]
            == "Usage: `/ask <question>` — research the codebase without bash or file changes. Public webfetch is still allowed."
        )

        cli._handle_ask(
            "where is auth?",
            "openai:gpt-test",
            {
                "workspace": tmp_path,
                "system_prompt": "sys",
                "interactive": True,
                "best_of": 1,
            },
            {"messages": []},
        )
        assert (
            notes[-1]
            == "research mode (no bash or file changes; public webfetch allowed)"
        )
        assert seen == {"called": True}

    def test_load_and_chat_commands(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path)
        patch_runtime(monkeypatch, _note=None, _print=None)

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
        assert cli._session_file("bad name/..") == tmp_path / "bad_name___.json"
        assert cli._chat_command("/clear", loaded, "new system", model) is True
        assert loaded["messages"] == [SystemMessage("new system")]


class TestChatRollback:
    def test_rolls_back_on_agent_error(self, tmp_path, monkeypatch):
        inputs = iter(["hello", "quit"])
        rollback_calls = []
        errors = []

        monkeypatch.setattr(cli, "_create_prompt_session", lambda: object())
        _stub_session(monkeypatch, tmp_path, interactive=True)
        monkeypatch.setattr(cli, "_print_session_intro", lambda *a, **k: None)
        monkeypatch.setattr(cli, "_set_terminal_title", lambda *a, **k: None)
        monkeypatch.setattr(cli, "_read_input", lambda *a, **k: next(inputs))
        monkeypatch.setattr(cli, "checkpoint", lambda tx: 7)
        monkeypatch.setattr(
            cli, "rollback", lambda tx, point: rollback_calls.append(point)
        )
        monkeypatch.setattr(
            cli,
            "run_agent",
            lambda *a, **k: (_ for _ in ()).throw(RuntimeError("boom")),
        )
        patch_runtime(
            monkeypatch,
            print_console=None,
            rule_console=None,
            _note=None,
            _error=errors.append,
        )

        assert cli.chat() == 0
        assert rollback_calls == [7]
        assert errors == ["Agent error: boom"]
