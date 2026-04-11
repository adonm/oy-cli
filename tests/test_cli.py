"""Tests for CLI module: main, chat, load/save."""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from oy_cli import agent, cli, runtime as rt
from oy_cli.providers import SystemMessage, UserMessage
from tests.conftest import patch_runtime


def _session_state(tmp_path, **overrides):
    session = {
        "workspace": tmp_path,
        "model": "openai:gpt-test",
        "interactive": False,
        "system_prompt": "sys",
        "system_file": None,
        "yolo": False,
    }
    session.update(overrides)
    return session


def _stub_session(monkeypatch, tmp_path, *, interactive=False):
    monkeypatch.setattr(
        cli,
        "resolve_session",
        lambda **kwargs: _session_state(tmp_path, interactive=interactive),
    )


def _capture_defopt_run(monkeypatch):
    seen = {}

    def fake_run(functions, *, argv, **kwargs):
        seen["argv"] = argv
        return 0

    monkeypatch.setattr(cli.defopt, "run", fake_run)
    return seen


class TestCLI:
    @pytest.mark.parametrize(
        ("argv", "expected"),
        [(["fix", "tests"], ["run", "fix", "tests"]), (["ralph", "fix", "tests"], ["ralph", "fix", "tests"])],
    )
    def test_main_normalizes_commands(self, monkeypatch, argv, expected):
        seen = _capture_defopt_run(monkeypatch)
        assert cli.main(argv) == 0
        assert seen["argv"] == expected

    def test_main_rejects_top_level_yolo(self, monkeypatch):
        monkeypatch.delenv("OY_YOLO", raising=False)
        with pytest.raises(SystemExit):
            cli.main(["--yolo", "fix", "tests"])
        assert rt.yolo_enabled() is False


    def test_main_prints_version_without_running_defopt(self, monkeypatch):
        printed = []
        monkeypatch.setattr(rt, "__version__", "0.4.6")
        patch_runtime(
            monkeypatch,
            _print=lambda *a, **k: printed.append(
                k.get("value", a[1] if len(a) > 1 else a[0] if a else "")
            ),
        )
        monkeypatch.setattr(
            cli.defopt,
            "run",
            lambda *a, **k: pytest.fail("defopt.run should not be called"),
        )

        assert cli.main(["--version"]) == 0
        assert printed == ["oy 0.4.6"]


class TestRalph:
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
        assert calls[0][0][0] == "from stdin"

    def test_ralph_limit_seconds_parses_env(self, monkeypatch):
        monkeypatch.setenv("OY_RALPH_LIMIT", "90m")
        assert rt.ralph_limit_seconds() == 5400

    def test_ralph_sandboxes_model_config_and_locks_model(self, tmp_path, monkeypatch):
        saved_config = tmp_path / "saved-config.json"
        monkeypatch.setenv("OY_CONFIG", str(saved_config))
        monkeypatch.setenv("OY_MODEL", "other-model")
        monkeypatch.setenv("OY_SHIM", "other-shim")
        rt.command_env.cache_clear()

        with cli._ralph_run_env("openai:gpt-test"):
            assert os.environ["OY_MODEL"] == "gpt-test"
            assert os.environ["OY_SHIM"] == "openai"
            assert os.environ["OY_LOCK_MODEL"] == "1"
            assert os.environ["OY_CONFIG"] != str(saved_config)
            assert rt._model(None) == "openai:gpt-test"
            assert cli._handle_model_switch("copilot:gpt-next", "openai:gpt-test") == "openai:gpt-test"
            assert not saved_config.exists()
            assert rt.save_model_config("copilot:gpt-next") == {
                "model": "gpt-next",
                "shim": "copilot",
            }
            assert not saved_config.exists()
            assert Path(os.environ["OY_CONFIG"]).exists()

        assert os.environ["OY_CONFIG"] == str(saved_config)
        assert os.environ["OY_MODEL"] == "other-model"
        assert os.environ["OY_SHIM"] == "other-shim"
        assert "OY_LOCK_MODEL" not in os.environ
        assert not saved_config.exists()


class TestAudit:
    def test_audit_creates_default_renovate_config_when_missing(self, tmp_path, monkeypatch):
        notes = []
        seen = {}

        _stub_session(monkeypatch, tmp_path)
        monkeypatch.setattr(cli, "_print_session_intro", lambda *a, **k: None)
        monkeypatch.setattr(rt, "unattended_limit_seconds", lambda: 60)

        def fake_run_agent(*args, **kwargs):
            seen["args"] = args
            seen["kwargs"] = kwargs
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)
        patch_runtime(monkeypatch, _note=lambda message, **k: notes.append(message))

        assert not (tmp_path / "renovate.json").exists()

        assert cli.audit("deps") == 0

        assert (tmp_path / "renovate.json").read_text(encoding="utf-8") == cli._DEFAULT_RENOVATE_CONFIG
        assert notes == ["created default Renovate config: renovate.json"]
        assert seen["args"][0] == "Conduct a security and complexity audit. Additional focus: deps"

    def test_audit_keeps_existing_supported_renovate_config(self, tmp_path, monkeypatch):
        notes = []
        config_dir = tmp_path / ".github"
        config_dir.mkdir()
        existing = config_dir / "renovate.json"
        existing.write_text('{"extends": ["local>example/preset"]}\n', encoding="utf-8")

        _stub_session(monkeypatch, tmp_path)
        monkeypatch.setattr(cli, "_print_session_intro", lambda *a, **k: None)
        monkeypatch.setattr(rt, "unattended_limit_seconds", lambda: 60)
        monkeypatch.setattr(cli, "run_agent", lambda *args, **kwargs: (0, ""))
        patch_runtime(monkeypatch, _note=lambda message, **k: notes.append(message))

        assert cli.audit() == 0

        assert not (tmp_path / "renovate.json").exists()
        assert existing.read_text(encoding="utf-8") == '{"extends": ["local>example/preset"]}\n'
        assert notes == []


class TestModelSelectionUI:
    def test_resolve_model_choice_uses_shared_model_list_ui(self, monkeypatch):
        printed = []
        monkeypatch.setattr(rt, "list_all_model_ids", lambda: ["openai:gpt-test"])
        monkeypatch.setattr(rt, "_model", lambda configured=None: "openai:gpt-test")
        monkeypatch.setattr(rt, "can_prompt", lambda: True)
        monkeypatch.setattr(rt, "ask", lambda *a, **k: "openai:gpt-test")
        patch_runtime(
            monkeypatch,
            _print=lambda *a, **k: printed.append(k.get("value", a[1] if len(a) > 1 else a[0] if a else "")),
        )

        assert cli.resolve_model_choice() == "openai:gpt-test"
        assert printed and printed[0].startswith("## Choose a Model")
        assert "Enter a number, exact model ID, or filter text." in printed[0]


class TestChatCommands:
    def test_help_lists_chat_commands(self, monkeypatch):
        printed = []
        patch_runtime(
            monkeypatch,
            _print=lambda *a, **k: printed.append(
                k.get("value", a[1] if len(a) > 1 else a[0] if a else "")
            ),
        )

        assert (
            cli._chat_command("/help", {"messages": []}, "sys", "openai:gpt-test")
            is True
        )
        assert "- `/ask <question>` -- research-only query" in printed[-1]
        assert "- `/quit` or `/exit` -- end session" in printed[-1]

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
        session = _session_state(tmp_path, interactive=True)

        cli._handle_ask("", "openai:gpt-test", session, {"messages": []})
        assert printed[-1] == cli._ASK_USAGE

        cli._handle_ask("where is auth?", "openai:gpt-test", session, {"messages": []})
        assert notes[-1] == cli._ASK_MODE_NOTE
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


