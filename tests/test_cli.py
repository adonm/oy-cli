"""Tests for CLI module: main, chat, load/save."""

from __future__ import annotations

import json
import os
import subprocess
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
        "agent": "default",
        "yolo": False,
    }
    session.update(overrides)
    return session


def _stub_session(monkeypatch, tmp_path, *, interactive=False):
    monkeypatch.setattr(
        cli,
        "resolve_session",
        lambda **kwargs: _session_state(
            tmp_path,
            interactive=kwargs.get("interactive", interactive),
            system_prompt=kwargs.get("system_prompt", "sys"),
            agent=kwargs.get("agent", "default"),
        ),
    )


def _capture_defopt_run(monkeypatch):
    seen = {}

    def fake_run(_functions, *, argv, **_kwargs):
        seen["argv"] = argv
        return 0

    monkeypatch.setattr(cli.defopt, "run", fake_run)
    return seen


class TestCLI:
    @pytest.mark.parametrize(
        ("argv", "expected"),
        [(["fix", "tests"], ["run", "fix", "tests"]), (["ralph", "fix", "tests"], ["ralph", "fix", "tests"]), (["audit-logic", "auth"], ["audit-logic", "auth"]), (["audit", "auth", "--from", "HEAD~1"], ["audit", "auth", "--from", "HEAD~1"]), (["audit", "auth", "--phase", "phase3"], ["audit", "auth", "--phase", "phase3"]), (["renovate-local"], ["renovate-local"]), (["--continue"], ["run", "--continue-session"]), (["-c"], ["run", "--continue-session"]), (["--resume", "abc123"], ["run", "--resume", "abc123"])],
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
            lambda *_a, **_k: pytest.fail("defopt.run should not be called"),
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
            cli, "_print_session_intro", lambda *_a, **k: intro.update(k)
        )
        patch_runtime(
            monkeypatch, _note=lambda *a, **k: notes.append((a, k)), _print=None
        )
        monkeypatch.setattr(rt, "ralph_limit_seconds", lambda _default=10800: 120)
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
        monkeypatch.setattr(cli, "_print_session_intro", lambda *_a, **_k: None)
        patch_runtime(monkeypatch, _note=None, _print=None)
        monkeypatch.setattr(rt, "ralph_limit_seconds", lambda _default=10800: 1)
        monkeypatch.setattr(cli.time, "monotonic", lambda: next(monotonic_values))
        monkeypatch.setattr(cli.time, "sleep", lambda _seconds: None)
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


class TestRenovateLocal:
    def test_renovate_local_prepares_workspace_and_runs_foreground(self, tmp_path, monkeypatch):
        notes = []
        printed = []
        seen = {}

        monkeypatch.setattr(cli, "workspace_root", lambda: tmp_path)
        monkeypatch.setattr(cli, "_renovate_github_token", lambda: "ghs_test")
        monkeypatch.setattr(cli.rt, "command_env", lambda _cwd=None: {"PATH": "/bin"})

        def fake_run(command, cwd=None, env=None, check=False):
            seen["command"] = command
            seen["cwd"] = cwd
            seen["env"] = env
            seen["check"] = check
            class Result:
                returncode = 0
            return Result()

        monkeypatch.setattr(cli.subprocess, "run", fake_run)
        patch_runtime(
            monkeypatch,
            _note=lambda message, **_k: notes.append(message),
            _print=lambda *a, **k: printed.append(k.get("value", a[1] if len(a) > 1 else a[0] if a else "")),
        )

        assert cli.renovate_local() == 0

        assert (tmp_path / ".tmp").is_dir()
        assert (tmp_path / ".gitignore").read_text(encoding="utf-8") == ".tmp/\n"
        assert (tmp_path / "renovate.json").read_text(encoding="utf-8") == cli._DEFAULT_RENOVATE_CONFIG
        assert seen["cwd"] == tmp_path
        assert seen["check"] is False
        assert seen["env"]["RENOVATE_GITHUB_COM_TOKEN"] == "ghs_test"
        assert seen["command"] == [
            "renovate",
            "--platform=local",
            "--require-config=ignored",
            "--dry-run=lookup",
            "--report-type=file",
            "--report-path",
            f".tmp/renovate-{cli.time.strftime('%Y-%m-%d')}.json",
        ]
        assert notes == [
            "created .tmp/",
            "updated .gitignore: .tmp/",
            "created default Renovate config: renovate.json",
            f"renovate report written: .tmp/renovate-{cli.time.strftime('%Y-%m-%d')}.json",
        ]
        assert printed and printed[0].startswith("## Renovate Local")


class TestModelSelectionUI:
    def test_resolve_model_choice_uses_shared_model_list_ui(self, monkeypatch):
        printed = []
        monkeypatch.setattr(rt, "list_all_model_ids", lambda: ["openai:gpt-test"])
        monkeypatch.setattr(rt, "_model", lambda _configured=None: "openai:gpt-test")
        monkeypatch.setattr(rt, "can_prompt", lambda: True)
        monkeypatch.setattr(rt, "ask", lambda *_a, **_k: "openai:gpt-test")
        patch_runtime(
            monkeypatch,
            _print=lambda *a, **k: printed.append(k.get("value", a[1] if len(a) > 1 else a[0] if a else "")),
        )

        assert cli.resolve_model_choice() == "openai:gpt-test"
        assert printed and printed[0].startswith("## Choose a Model")
        assert "Enter a number, exact model ID, or filter text." in printed[0]


class TestGitStatusSummary:
    def test_git_diff_shortstat_tracks_untracked_and_staged_changes(self, tmp_path):
        workspace = tmp_path / "repo"
        workspace.mkdir()
        assert cli._git_diff_shortstat(workspace) is None

        subprocess.run(["git", "init"], cwd=workspace, check=True, capture_output=True, text=True)
        subprocess.run(["git", "config", "user.email", "test@example.com"], cwd=workspace, check=True, capture_output=True, text=True)
        subprocess.run(["git", "config", "user.name", "Test User"], cwd=workspace, check=True, capture_output=True, text=True)
        (workspace / "tracked.txt").write_text("one\n", encoding="utf-8")
        subprocess.run(["git", "add", "tracked.txt"], cwd=workspace, check=True, capture_output=True, text=True)
        subprocess.run(["git", "commit", "-m", "init"], cwd=workspace, check=True, capture_output=True, text=True)

        assert cli._git_diff_shortstat(workspace) == "git diff: clean"

        (workspace / "tracked.txt").write_text("two\n", encoding="utf-8")
        assert cli._git_diff_shortstat(workspace) == "1 change; 1 modified; lines +1 -1"

        (workspace / "untracked.txt").write_text("new\n", encoding="utf-8")
        assert cli._git_diff_shortstat(workspace) == "2 changes; 1 modified; 1 untracked; lines +1 -1"

        subprocess.run(["git", "add", "untracked.txt"], cwd=workspace, check=True, capture_output=True, text=True)
        assert cli._git_diff_shortstat(workspace) == "2 changes; 1 staged; 1 modified; lines +2 -1"


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
        assert "- `/audit-logic [focus]` -- run or resume a logic-focused audit that ignores docs/comments" in printed[-1]
        assert "- `/quit` or `/exit` -- end session" in printed[-1]

    def test_ask_usage_and_note_are_explicit_about_webfetch(
        self, tmp_path, monkeypatch
    ):
        printed = []
        notes = []
        patch_runtime(
            monkeypatch,
            _note=lambda message, **_k: notes.append(message),
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
            cli, "transcript_with_system_prompt", lambda _prompt: {"messages": []}
        )
        monkeypatch.setattr(cli, "new_agent_state", lambda **_k: {"state": True})
        monkeypatch.setattr(
            cli, "add_user", lambda tx, question: tx.update({"question": question})
        )
        monkeypatch.setattr(rt, "unattended_limit_seconds", lambda: 60)
        monkeypatch.setattr(rt, "get_client", lambda _model: object())
        monkeypatch.setattr(cli, "tool_specs", lambda _registry: [])
        seen = {}
        monkeypatch.setattr(
            cli, "run_turn", lambda *_args, **_kwargs: seen.update({"called": True})
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
            "agent": "plan",
            "saved_at": "2026-03-25T12:34:56",
            "transcript": cli._transcript_data(
                agent.transcript(messages=[SystemMessage("old"), UserMessage("hello")])
            ),
        }
        (tmp_path / "saved.json").write_text(json.dumps(saved), encoding="utf-8")

        loaded, model, loaded_agent = cli._handle_load(
            "saved",
            agent.transcript_with_system_prompt("sys"),
            "openai:gpt-old",
            "new system",
            "default",
        )
        assert model == "openai:gpt-test"
        assert loaded_agent == "plan"
        assert loaded["messages"] == [SystemMessage("new system"), UserMessage("hello")]
        assert cli._chat_command("/audit-logic auth", loaded, "new system", model) == ("audit_logic", "auth")
        assert cli._chat_command("/audit --phase phase3 auth", loaded, "new system", model) == ("audit", "--phase phase3 auth")
        assert cli._chat_command("/yolo", loaded, "new system", model) == ("yolo",)
        assert cli._session_file("bad name/..") == tmp_path / "bad_name___.json"
        assert cli._chat_command("/clear", loaded, "new system", model) is True
        assert loaded["messages"] == [SystemMessage("new system")]


class TestSessionContinuation:
    def test_handle_save_persists_agent(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path)
        patch_runtime(monkeypatch, _note=None, _print=None)
        tx = agent.transcript_with_system_prompt("sys")
        cli._handle_save("demo", tx, "openai:gpt-test", "plan")
        saved = json.loads((tmp_path / "demo.json").read_text(encoding="utf-8"))
        assert saved["model"] == "openai:gpt-test"
        assert saved["agent"] == "plan"

    def test_run_resume_uses_saved_transcript_and_agent(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path)
        saved = {
            "model": "openai:gpt-saved",
            "agent": "plan",
            "saved_at": "2026-03-25T12:34:56",
            "transcript": cli._transcript_data(
                agent.transcript(messages=[SystemMessage("old"), UserMessage("hello")])
            ),
        }
        (tmp_path / "saved.json").write_text(json.dumps(saved), encoding="utf-8")
        monkeypatch.setattr(cli, "workspace_root", lambda: tmp_path)
        monkeypatch.setattr(rt, "_model", lambda _configured=None: "openai:gpt-live")
        monkeypatch.setattr(rt, "_sys_file", lambda: None)
        monkeypatch.setattr(rt, "yolo_enabled", lambda _default=False: False)
        monkeypatch.setattr(rt, "can_prompt", lambda: False)
        monkeypatch.setattr(rt, "unattended_limit_seconds", lambda: 60)
        monkeypatch.setattr(cli, "_print_session_intro", lambda *_a, **_k: None)
        seen = {}
        monkeypatch.setattr(
            cli,
            "run_agent",
            lambda *a, **k: seen.update({"args": a, "kwargs": k}) or (0, ""),
        )

        assert cli.run("ship", "it", resume="saved") == 0
        assert seen["args"][1] == "openai:gpt-saved"
        assert seen["kwargs"]["agent"] == "plan"
        assert seen["kwargs"]["transcript"]["messages"][0] == SystemMessage(
            seen["args"][3]
        )

    def test_chat_continue_loads_latest_session(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path)
        saved = {
            "model": "openai:gpt-saved",
            "agent": "accept-edits",
            "saved_at": "2026-03-25T12:34:56",
            "transcript": cli._transcript_data(
                agent.transcript(messages=[SystemMessage("old"), UserMessage("hello")])
            ),
        }
        (tmp_path / "latest.json").write_text(json.dumps(saved), encoding="utf-8")
        monkeypatch.setattr(cli, "_create_prompt_session", lambda: object())
        monkeypatch.setattr(cli, "workspace_root", lambda: tmp_path)
        monkeypatch.setattr(rt, "_model", lambda _configured=None: "openai:gpt-live")
        monkeypatch.setattr(rt, "_sys_file", lambda: None)
        monkeypatch.setattr(rt, "yolo_enabled", lambda _default=False: False)
        monkeypatch.setattr(rt, "can_prompt", lambda: True)
        monkeypatch.setattr(cli, "_set_terminal_title", lambda *_a, **_k: None)
        monkeypatch.setattr(cli, "_read_input", lambda *_a, **_k: (_ for _ in ()).throw(EOFError()))
        intro = {}
        monkeypatch.setattr(cli, "_print_session_intro", lambda *_a, **k: intro.update(k))
        patch_runtime(monkeypatch, print_console=None, rule_console=None, _note=None)

        assert cli.chat(continue_session=True) == 0
        assert intro["session"] == "continued"


class TestChatRollback:
    def test_rolls_back_on_agent_error(self, tmp_path, monkeypatch):
        inputs = iter(["hello", "quit"])
        rollback_calls = []
        errors = []

        monkeypatch.setattr(cli, "_create_prompt_session", lambda: object())
        _stub_session(monkeypatch, tmp_path, interactive=True)
        monkeypatch.setattr(cli, "_print_session_intro", lambda *_a, **_k: None)
        monkeypatch.setattr(cli, "_set_terminal_title", lambda *_a, **_k: None)
        monkeypatch.setattr(cli, "_read_input", lambda *_a, **_k: next(inputs))
        monkeypatch.setattr(cli, "checkpoint", lambda _tx: 7)
        monkeypatch.setattr(
            cli, "rollback", lambda _tx, point: rollback_calls.append(point)
        )
        monkeypatch.setattr(
            cli,
            "run_agent",
            lambda *_a, **_k: (_ for _ in ()).throw(RuntimeError("boom")),
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




class TestAuditWorkflow:
    def test_ensure_audit_session_bootstraps_chunk_plan(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _note=None, _print=None)

        (tmp_path / "src").mkdir()
        (tmp_path / "src" / "main.py").write_text("print('hi')\n", encoding="utf-8")
        (tmp_path / "README.md").write_text("# demo\n", encoding="utf-8")
        (tmp_path / "ignored.py").write_text("print('skip')\n", encoding="utf-8")
        (tmp_path / ".gitignore").write_text("ignored.py\n", encoding="utf-8")
        (tmp_path / "script").write_text("#!/bin/sh\necho hi\n", encoding="utf-8")
        (tmp_path / "image.png").write_bytes(b"png\x00")

        artifacts = cli._ensure_audit_session(tmp_path, focus="auth")

        assert artifacts["created"] is True
        session_path = artifacts["session_path"]
        state = rt.load_toon(session_path, {})
        assert state["run_config"]["command"] == "oy audit"
        assert state["run_config"]["phase2_workers"] == rt.audit_settings()["phase2_workers"]
        assert state["run_config"]["phase2_launch_delay_seconds"] == 10
        assert isinstance(state["run_config"]["model"], str) and state["run_config"]["model"]
        assert state["focus"] == "auth"
        assert state["workspace"] == str(tmp_path)
        assert state["active_phase"] == "phase2"
        assert {item["path"] for item in state["files"]} == {".gitignore", "README.md", "script", "src/main.py"}
        assert state["chunks"]
        assert all(chunk["estimated_tokens"] <= 64_000 or len(chunk["paths"]) == 1 for chunk in state["chunks"])
        assert all(chunk.get("segment_count", 0) == 0 for chunk in state["chunks"])
        assert state["totals"]["chunk_count"] == len(state["chunks"])
        assert state["phases"][0]["status"] == "done"
        assert state["phases"][1]["status"] == "pending"

    def test_ensure_audit_session_normalizes_existing_issues_into_inbox(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _note=None, _print=None)
        (tmp_path / "app.py").write_text("print('hi')\n", encoding="utf-8")
        (tmp_path / "ISSUES.md").write_text(
            "# Audit Findings\n\n"
            "## H1 · old title\n\n"
            "old detail\n\n"
            "## Short audit log\n\n"
            "- earlier run\n",
            encoding="utf-8",
        )

        artifacts = cli._ensure_audit_session(tmp_path, focus="auth")

        issues_text = (tmp_path / "ISSUES.md").read_text(encoding="utf-8")
        state = rt.load_toon(artifacts["session_path"], {})
        assert artifacts["created"] is True
        expected = f"> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL={state["run_config"]["model"]} oy audit`"
        assert expected in issues_text
        assert issues_text.startswith("# Audit Issues\n")
        assert "## Inbox" in issues_text
        assert "### H1 · old title" in issues_text
        assert "## Short audit log" in issues_text
        assert state["totals"]["findings"] == 1
        assert any("Normalised ISSUES.md into audit inbox format" in note for note in state["notes"])

    def test_audit_prepare_issues_md_upserts_transparency_snippet(self, tmp_path):
        state = {
            "run_config": {"command": "oy audit", "mode": cli._AUDIT_DEFAULT_MODE, "model": "copilot:gpt5.4", "agent": "default", "max_context_tokens": 131072, "phase2_workers": 8, "phase2_launch_delay_seconds": 0},
            "totals": {"queued": 1, "total_code_count": 1, "counted_files": 1},
            "sloc": {},
        }
        (tmp_path / "ISSUES.md").write_text(
            "# Audit Issues\n\n"
            "> **Last audit**: 2026-01-01 · commit `abc`\n\n"
            "## Inbox\n\n"
            "Append-only review inbox for phase2. Add new candidate findings here without merging or renumbering; phase3 rewrites the final report.\n\n"
            "No inbox findings recorded yet.\n",
            encoding="utf-8",
        )

        result = cli._audit_prepare_issues_md(tmp_path, state)
        issues_text = (tmp_path / "ISSUES.md").read_text(encoding="utf-8")

        assert result["changed"] is True
        assert "> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=copilot:gpt5.4 oy audit`" in issues_text
        assert issues_text.startswith("# Audit Issues\n\n> Generated with [oy-cli]")

    def test_audit_prepare_issues_md_upserts_transparency_without_report_title_on_line_one(self, tmp_path):
        state = {
            "run_config": {"command": "oy audit", "mode": cli._AUDIT_DEFAULT_MODE, "model": "copilot:gpt5.4", "agent": "default", "max_context_tokens": 131072, "phase2_workers": 8, "phase2_launch_delay_seconds": 0},
            "totals": {"queued": 1, "total_code_count": 1, "counted_files": 1},
            "sloc": {},
        }
        (tmp_path / "ISSUES.md").write_text(
            "<!-- banner -->\n\n"
            "# Audit Issues\n\n"
            "> **Last audit**: 2026-01-01 · commit `abc`\n",
            encoding="utf-8",
        )

        cli._audit_prepare_issues_md(tmp_path, state)
        issues_text = (tmp_path / "ISSUES.md").read_text(encoding="utf-8")

        assert issues_text.startswith("<!-- banner -->\n\n# Audit Issues\n\n> Generated with [oy-cli]")
        assert issues_text.count("> Generated with [oy-cli]") == 1

    def test_audit_prepare_issues_md_records_phase1_dependency_assessment_once(self, tmp_path):
        state = {
            "run_config": {"command": "oy audit", "mode": cli._AUDIT_DEFAULT_MODE, "model": "copilot:gpt5.4", "agent": "default", "max_context_tokens": 131072, "phase2_workers": 8, "phase2_launch_delay_seconds": 0},
            "totals": {"queued": 1, "total_code_count": 1, "counted_files": 1},
            "sloc": {},
        }
        (tmp_path / ".tmp").mkdir()
        (tmp_path / ".tmp" / "renovate-2026-01-02.json").write_text(
            json.dumps(
                {
                    "repositories": [
                        {
                            "warnings": ["Using local RE2 fallback"],
                            "packageFiles": [
                                {
                                    "packageFile": "package.json",
                                    "manager": "npm",
                                    "updates": [
                                        {"depName": "lodash", "currentVersion": "4.17.20", "newVersion": "4.17.21", "updateType": "patch", "manager": "npm", "packageFile": "package.json"},
                                        {"depName": "actions/checkout", "currentVersion": "v4", "newVersion": "v5", "updateType": "major", "manager": "github-actions", "packageFile": ".github/workflows/ci.yml"},
                                    ],
                                }
                            ],
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )

        cli._audit_prepare_issues_md(tmp_path, state)
        cli._audit_prepare_issues_md(tmp_path, state)
        issues_text = (tmp_path / "ISSUES.md").read_text(encoding="utf-8")

        assert issues_text.count("Phase1 dependency assessment:") == 1
        assert "## Short audit log" in issues_text
        assert "`.tmp/renovate-2026-01-02.json`" in issues_text
        assert "no clear dependency or GitHub Actions risk beyond routine maintenance" in issues_text
        assert "Warnings: Using local RE2 fallback." in issues_text

    def test_audit_plan_chunks_clusters_by_directory_before_splitting(self):
        files = [
            {"path": "api/auth/a.py", "code_count": 50, "estimated_tokens": 20_000},
            {"path": "api/auth/b.py", "code_count": 40, "estimated_tokens": 20_000},
            {"path": "api/payments/c.py", "code_count": 30, "estimated_tokens": 20_000},
            {"path": "web/ui/d.ts", "code_count": 20, "estimated_tokens": 20_000},
        ]

        chunks = cli._audit_plan_chunks(files, target_tokens=64_000)

        assert [chunk["paths"] for chunk in chunks] == [
            ["api/auth/a.py", "api/auth/b.py", "api/payments/c.py"],
            ["web/ui/d.ts"],
        ]
        assert all(chunk["estimated_tokens"] <= 64_000 for chunk in chunks)

    def test_audit_chunk_segments_split_large_single_file_to_fit_prompt(self, tmp_path):
        big = tmp_path / "big.py"
        big.write_text("\n".join(f"value_{i} = {i}" for i in range(6000)), encoding="utf-8")
        chunk = {"id": "chunk-001", "paths": ["big.py"], "estimated_tokens": 200_000, "files": 1}

        segments = cli._audit_chunk_segments(
            tmp_path,
            chunk,
            max_context_tokens=4096,
            inbox_text=cli._audit_inbox_section(),
            prompt_text="prompt",
            system_prompt="sys",
        )

        assert len(segments) > 1
        assert segments[0]["start"] == 0
        assert all(segments[i]["start"] < segments[i]["end"] for i in range(len(segments)))
        assert segments[-1]["end"] >= segments[-1]["start"]

    def test_audit_inbox_context_compacts_to_recent_entries(self, tmp_path):
        entries = "\n\n".join(
            f"### Finding {i}\n\n- detail {'x' * 120}"
            for i in range(8)
        )
        (tmp_path / "ISSUES.md").write_text(
            "# Audit Issues\n\n"
            "## Inbox\n\n"
            "Append-only review inbox for phase2. Add new candidate findings here without merging or renumbering; phase3 rewrites the final report.\n\n"
            f"{entries}\n",
            encoding="utf-8",
        )

        compact = cli._audit_compact_inbox_text(cli._audit_read_inbox(tmp_path), max_tokens=150)

        assert "Finding 7" in compact
        assert "Finding 0" not in compact
        assert rt.count_tokens(compact) <= 160

    def test_audit_walk_files_respects_gitignore(self, tmp_path):
        (tmp_path / "keep.py").write_text("print('keep')\n", encoding="utf-8")
        (tmp_path / "skip.py").write_text("print('skip')\n", encoding="utf-8")
        (tmp_path / "build").mkdir()
        (tmp_path / "build" / "gen.py").write_text("print('gen')\n", encoding="utf-8")
        (tmp_path / ".gitignore").write_text("skip.py\nbuild/\n", encoding="utf-8")

        assert cli._audit_walk_files(tmp_path) == [".gitignore", "keep.py"]

    def test_audit_walk_files_skips_nul_binary_but_keeps_other_files(self, tmp_path):
        (tmp_path / "notes").write_text("hello\n", encoding="utf-8")
        (tmp_path / "data.bin").write_bytes(b"abc\x00def")
        (tmp_path / "bad.txt").write_bytes(b"\xff\xfe")
        (tmp_path / "big.txt").write_text("x" * (600 * 1024), encoding="utf-8")

        assert cli._audit_walk_files(tmp_path) == ["bad.txt", "big.txt", "notes"]

    def test_audit_walk_files_logic_mode_skips_docs_and_lockfiles(self, tmp_path):
        (tmp_path / "docs").mkdir()
        (tmp_path / "docs" / "guide.md").write_text("# guide\n", encoding="utf-8")
        (tmp_path / "README.md").write_text("# demo\n", encoding="utf-8")
        (tmp_path / "uv.lock").write_text("version = 1\n", encoding="utf-8")
        (tmp_path / "src").mkdir()
        (tmp_path / "src" / "main.py").write_text("print('hi')\n", encoding="utf-8")

        assert cli._audit_walk_files(tmp_path, mode=cli._AUDIT_LOGIC_MODE) == ["src/main.py"]

    def test_audit_file_excerpt_logic_mode_strips_comments_and_docstrings(self, tmp_path):
        path = tmp_path / "main.py"
        path.write_text(
            '"""module docs"""\n# comment\ndef run():\n    """fn docs"""\n    value = 1  # inline\n    return value\n',
            encoding="utf-8",
        )

        excerpt = cli._audit_file_excerpt(tmp_path, "main.py", mode=cli._AUDIT_LOGIC_MODE)

        assert 'module docs' not in excerpt
        assert '# comment' not in excerpt
        assert 'fn docs' not in excerpt
        assert 'inline' not in excerpt
        assert 'def run()' in excerpt
        assert 'return value' in excerpt

    def test_audit_session_path_separates_logic_mode(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")

        assert cli._audit_session_path(tmp_path) != cli._audit_session_path(tmp_path, mode=cli._AUDIT_LOGIC_MODE)
        assert cli._audit_session_path(tmp_path, mode=cli._AUDIT_LOGIC_MODE).name.endswith('-logic.toon')

    def test_audit_session_path_separates_scope(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")

        unscoped = cli._audit_session_path(tmp_path)
        scoped = cli._audit_session_path(tmp_path, scope={"ref": "HEAD~1"})

        assert scoped != unscoped
        assert scoped.name.endswith('.toon')
        assert '-from-' in scoped.name

    def test_audit_parse_scope_supports_date_and_ref(self):
        assert cli._audit_parse_scope('2026-04-01') == {"date": "2026-04-01"}
        assert cli._audit_parse_scope('HEAD~3') == {"ref": "HEAD~3"}
        assert cli._audit_parse_scope('ref:HEAD~3 date:2026-04-01') == {"ref": "HEAD~3", "date": "2026-04-01"}
        with pytest.raises(ValueError):
            cli._audit_parse_scope('date:20260401')

    def test_build_audit_prompt_includes_scope_note(self, tmp_path):
        prompt = cli._build_audit_prompt(
            interactive=False,
            focus='auth',
            session_path=tmp_path / 'audit.toon',
            scope={"ref": "HEAD~1", "date": "2026-04-01"},
        )

        assert 'Additional focus: auth' in prompt
        assert 'Scoped with `--from` after commit `HEAD~1` and date `2026-04-01`.' in prompt

    def test_audit_returns_error_for_invalid_from(self, monkeypatch):
        patch_runtime(monkeypatch, _print=None, _note=None)

        assert cli.audit(from_='date:20260401') == 1

    def test_ensure_audit_session_scopes_files_with_git_name_only(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _note=None, _print=None)

        subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True, text=True)
        subprocess.run(["git", "config", "user.email", "test@example.com"], cwd=tmp_path, check=True, capture_output=True, text=True)
        subprocess.run(["git", "config", "user.name", "Test User"], cwd=tmp_path, check=True, capture_output=True, text=True)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        (tmp_path / "b.py").write_text("print('b')\n", encoding="utf-8")
        subprocess.run(["git", "add", "a.py", "b.py"], cwd=tmp_path, check=True, capture_output=True, text=True)
        subprocess.run(["git", "commit", "-m", "base"], cwd=tmp_path, check=True, capture_output=True, text=True)
        base = subprocess.run(["git", "rev-parse", "HEAD"], cwd=tmp_path, check=True, capture_output=True, text=True).stdout.strip()
        (tmp_path / "b.py").write_text("print(\'b2\')\n", encoding="utf-8")
        subprocess.run(["git", "add", "b.py"], cwd=tmp_path, check=True, capture_output=True, text=True)
        subprocess.run(["git", "commit", "-m", "update b"], cwd=tmp_path, check=True, capture_output=True, text=True)

        artifacts = cli._ensure_audit_session(tmp_path, focus="auth", scope={"ref": base})

        state = rt.load_toon(artifacts["session_path"], {})
        issues_text = (tmp_path / "ISSUES.md").read_text(encoding="utf-8")
        assert [item["path"] for item in state["files"]] == ["b.py"]
        assert state["scope"] == {"ref": base}
        assert state["run_config"]["from"] == f"ref:{base}"
        assert state["totals"]["queued"] == 1
        assert state["totals"]["total_code_count"] == 1
        assert any("Scoped with `--from` after commit" in note for note in state["notes"])
        assert "> **Scope**: 1 reviewable files" in issues_text

    def test_audit_uses_python_workflow(self, tmp_path, monkeypatch):
        _stub_session(monkeypatch, tmp_path)
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        monkeypatch.setattr(rt, "unattended_limit_seconds", lambda: 60)

        artifacts = {
            "session_path": tmp_path / ".sessions" / "audits" / "repo.toon",
            "state_data": {"status": "in_progress"},
            "created": True,
        }
        monkeypatch.setattr(cli, "_prepare_audit_run", lambda **_kwargs: (artifacts, f"prompt {artifacts['session_path']} 64k chunks"))
        intro = {}
        monkeypatch.setattr(cli, "_print_session_intro", lambda *_a, **k: intro.update(k))
        seen = {}
        monkeypatch.setattr(cli, "_run_audit_workflow", lambda **kwargs: seen.update(kwargs) or 0)

        assert cli.audit("auth", from_="HEAD~1") == 0
        assert "64k chunks" in seen["prompt"]
        assert str(artifacts["session_path"]) in seen["prompt"]
        assert seen["workspace"] == tmp_path
        assert seen["session_path"] == artifacts["session_path"]
        assert intro["audit_state"] == artifacts["session_path"]

    def test_audit_phase_option_forwards_requested_phase(self, tmp_path, monkeypatch):
        _stub_session(monkeypatch, tmp_path)
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        monkeypatch.setattr(rt, "unattended_limit_seconds", lambda: 60)

        artifacts = {
            "session_path": tmp_path / ".sessions" / "audits" / "repo.toon",
            "state_data": {"status": "in_progress"},
            "created": True,
        }
        monkeypatch.setattr(cli, "_prepare_audit_run", lambda **_kwargs: (artifacts, "prompt"))
        monkeypatch.setattr(cli, "_print_session_intro", lambda *_a, **_k: None)
        seen = {}
        monkeypatch.setattr(cli, "_run_audit_workflow", lambda **kwargs: seen.update(kwargs) or 0)

        assert cli.audit("auth", phase="phase3") == 0
        assert seen["phase"] == "phase3"
        assert seen["session_path"] == artifacts["session_path"]

    def test_audit_logic_uses_logic_mode_workflow(self, tmp_path, monkeypatch):
        _stub_session(monkeypatch, tmp_path)
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        monkeypatch.setattr(rt, "unattended_limit_seconds", lambda: 60)

        artifacts = {
            "session_path": tmp_path / ".sessions" / "audits" / "repo-logic.toon",
            "state_data": {"status": "in_progress"},
            "created": True,
        }
        monkeypatch.setattr(cli, "_prepare_audit_run", lambda **_kwargs: (artifacts, f"logic prompt {artifacts['session_path']} stripped comments"))
        intro = {}
        monkeypatch.setattr(cli, "_print_session_intro", lambda *_a, **k: intro.update(k))
        seen = {}
        monkeypatch.setattr(cli, "_run_audit_workflow", lambda **kwargs: seen.update(kwargs) or 0)

        assert cli.audit_logic("auth", from_="2026-04-01") == 0
        assert "stripped comments" in seen["prompt"]
        assert seen["mode"] == cli._AUDIT_LOGIC_MODE
        assert seen["system_prompt"] == cli.LOGIC_AUDIT_PHASE1_SYSTEM_PROMPT
        assert intro["audit_state"] == artifacts["session_path"]

    def test_prepare_audit_run_prompts_to_resume_unfinished_audit(self, tmp_path, monkeypatch):
        _stub_session(monkeypatch, tmp_path)
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        session = cli.resolve_session(interactive=True, system_prompt=cli.AUDIT_SYSTEM_PROMPT, include_system_file=False)
        existing = cli._ensure_audit_session(tmp_path, focus="")
        prompts = []
        monkeypatch.setattr(rt, "can_prompt", lambda: True)
        monkeypatch.setattr(rt, "select", lambda *a, **k: prompts.append((a, k)) or "resume")
        patch_runtime(monkeypatch, _print=None, _note=None)

        artifacts, prompt = cli._prepare_audit_run(session=session, focus="auth", interactive=True)

        assert artifacts["session_path"] == existing["session_path"]
        assert prompts
        assert "Resume it?" in prompts[0][0][0]
        assert "64k" in prompt

    def test_run_audit_workflow_retries_with_smaller_chunk_when_issues_unchanged(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        session_path = cli._audit_session_path(tmp_path)
        rt._ensure_private_dir(session_path.parent)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        (tmp_path / "b.py").write_text("print('b')\n", encoding="utf-8")
        rt.save_toon(session_path, {
            "version": cli._AUDIT_STATE_VERSION,
            "workspace": str(tmp_path),
            "focus": "",
            "status": "in_progress",
            "active_phase": "phase2",
            "phases": [
                {"id": "phase1", "status": "done", "label": "plan", "notes": []},
                {"id": "phase2", "status": "pending", "label": "review", "notes": []},
                {"id": "phase3", "status": "pending", "label": "summary", "notes": []},
            ],
            "sloc": {"counted_files": 2, "total_code_count": 2},
            "files": [
                {"path": "a.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10},
                {"path": "b.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10},
            ],
            "chunks": [{"id": "chunk-001", "paths": ["a.py", "b.py"], "estimated_tokens": 20, "files": 2, "segments": [], "segment_count": 0}],
            "completed_chunks": [],
            "completed_segments": [],
            "failed_chunks": [],
            "notes": [],
            "totals": {"queued": 2, "reviewed": 0, "findings": 0, "counted_files": 2, "total_code_count": 2, "total_line_count": 2, "chunk_count": 1, "completed_chunks": 0},
        })
        calls = []

        def fake_run_agent(*_args, **kwargs):
            calls.append(kwargs)
            if len(calls) == 2:
                (tmp_path / "ISSUES.md").write_text("# Audit Issues\n\n## Important finding\n\n## Concise follow-up\n", encoding="utf-8")
            elif len(calls) == 3:
                (tmp_path / "ISSUES.md").write_text("# Audit Issues\n\n## Important finding\n\n## Another finding\n", encoding="utf-8")
            elif len(calls) == 4:
                (tmp_path / "ISSUES.md").write_text("# Audit Issues\n\n## Important finding\n\n- Another finding\n", encoding="utf-8")
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)

        assert cli._run_audit_workflow(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
        ) == 0
        saved = rt.load_toon(session_path, {})
        assert len(calls) == 2
        assert saved["status"] == "done"
        assert saved["completed_chunks"]
        assert "chunk-001" in saved["completed_chunks"]
        assert not saved.get("completed_segments")
        assert "Important finding" in (tmp_path / "ISSUES.md").read_text(encoding="utf-8")

    def test_run_audit_workflow_reviews_oversized_chunk_via_segments(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        session_path = cli._audit_session_path(tmp_path)
        rt._ensure_private_dir(session_path.parent)
        (tmp_path / "big.py").write_text("\n".join(f"value_{i} = {i}" for i in range(6000)), encoding="utf-8")
        rt.save_toon(session_path, {
            "version": cli._AUDIT_STATE_VERSION,
            "workspace": str(tmp_path),
            "focus": "",
            "status": "in_progress",
            "active_phase": "phase2",
            "phases": [
                {"id": "phase1", "status": "done", "label": "plan", "notes": []},
                {"id": "phase2", "status": "pending", "label": "review", "notes": []},
                {"id": "phase3", "status": "pending", "label": "summary", "notes": []},
            ],
            "sloc": {"counted_files": 1, "total_code_count": 6000},
            "files": [{"path": "big.py", "language": "Python", "code_count": 6000, "line_count": 6000, "size_bytes": 100000, "estimated_tokens": 200000}],
            "chunks": [{"id": "chunk-001", "paths": ["big.py"], "estimated_tokens": 200000, "files": 1, "segments": [], "segment_count": 0}],
            "completed_chunks": [],
            "completed_segments": [],
            "failed_chunks": [],
            "notes": [],
            "totals": {"queued": 1, "reviewed": 0, "findings": 0, "counted_files": 1, "total_code_count": 6000, "total_line_count": 6000, "chunk_count": 1, "completed_chunks": 0},
        })
        prompts = []

        def fake_run_agent(prompt, *_args, **kwargs):
            prompts.append(prompt)
            if "Chunk contents:" in prompt:
                payload = kwargs["tool_registry"]["inbox_append"]["fn"](
                    {"interactive": False, "unattended_deadline": 999999, "unattended_limit_seconds": 999999},
                    f"### Segment {len([item for item in prompts if 'Chunk contents:' in item])}\n\n- detail",
                )
                assert payload["buffered"] is True
            else:
                (tmp_path / "ISSUES.md").write_text("# Audit Issues\n\n## Summary\n", encoding="utf-8")
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)

        assert cli._run_audit_workflow(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
            max_context_tokens=4096,
        ) == 0
        saved = rt.load_toon(session_path, {})
        assert len([prompt for prompt in prompts if "Chunk contents:" in prompt]) > 1
        assert any("Review only segment 1/" in prompt for prompt in prompts)
        assert any("Review only segment 2/" in prompt for prompt in prompts)
        assert "chunk-001" in saved["completed_chunks"]
        assert len(saved["completed_segments"]) > 1
        assert saved["chunks"][0]["segment_count"] > 1
        assert len(saved["chunks"][0]["segments"]) > 1
        issues_text = (tmp_path / "ISSUES.md").read_text(encoding="utf-8")
        assert "## Summary" in issues_text

    def test_run_audit_workflow_uses_non_truncating_audit_transcript(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        session_path = cli._audit_session_path(tmp_path)
        rt._ensure_private_dir(session_path.parent)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        rt.save_toon(session_path, {
            "version": cli._AUDIT_STATE_VERSION,
            "workspace": str(tmp_path),
            "focus": "",
            "status": "in_progress",
            "active_phase": "phase2",
            "phases": [
                {"id": "phase1", "status": "done", "label": "plan", "notes": []},
                {"id": "phase2", "status": "pending", "label": "review", "notes": []},
                {"id": "phase3", "status": "pending", "label": "summary", "notes": []},
            ],
            "sloc": {"counted_files": 1, "total_code_count": 1},
            "files": [{"path": "a.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10}],
            "chunks": [{"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1, "segments": [], "segment_count": 0}],
            "completed_chunks": [],
            "completed_segments": [],
            "failed_chunks": [],
            "notes": [],
            "totals": {"queued": 1, "reviewed": 0, "findings": 0, "counted_files": 1, "total_code_count": 1, "total_line_count": 1, "chunk_count": 1, "completed_chunks": 0},
        })
        seen = []

        def fake_run_agent(*_args, **kwargs):
            seen.append(kwargs["transcript"])
            if len(seen) == 1:
                (tmp_path / "ISSUES.md").write_text("# Audit Issues\n\n## Finding\n\n## Another finding\n", encoding="utf-8")
            else:
                (tmp_path / "ISSUES.md").write_text("# Audit Issues\n\n## Finding\n\n## Another finding\n\n## Summary\n", encoding="utf-8")
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)

        assert cli._run_audit_workflow(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
            max_context_tokens=4321,
        ) == 0
        assert len(seen) >= 2
        assert all(tx["max_context_tokens"] == 4321 for tx in seen)
        assert all(tx["max_message_tokens"] == 4321 for tx in seen)
        assert all(tx["messages"] == [] for tx in seen)
        issues_text = (tmp_path / "ISSUES.md").read_text(encoding="utf-8")
        assert issues_text.startswith("# Audit Issues\n\n> Generated with [oy-cli]")

    def test_ensure_audit_session_resume_backfills_missing_run_config_and_transparency(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        session_path = cli._audit_session_path(tmp_path)
        rt._ensure_private_dir(session_path.parent)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        (tmp_path / "ISSUES.md").write_text(
            "# Audit Issues\n\n"
            "> **Last audit**: 2026-01-01 · commit `abc`\n\n"
            "## Inbox\n\n"
            "Append-only review inbox for phase2. Add new candidate findings here without merging or renumbering; phase3 rewrites the final report.\n\n"
            "No inbox findings recorded yet.\n",
            encoding="utf-8",
        )
        rt.save_toon(session_path, {
            "version": cli._AUDIT_STATE_VERSION,
            "workspace": str(tmp_path),
            "mode": cli._AUDIT_DEFAULT_MODE,
            "focus": "",
            "status": "in_progress",
            "active_phase": "phase2",
            "phases": [
                {"id": "phase1", "status": "done", "label": "plan", "notes": []},
                {"id": "phase2", "status": "pending", "label": "review", "notes": []},
                {"id": "phase3", "status": "pending", "label": "summary", "notes": []},
            ],
            "sloc": {"counted_files": 1, "total_code_count": 1},
            "files": [{"path": "a.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10}],
            "chunks": [{"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1, "segments": [], "segment_count": 0}],
            "completed_chunks": [],
            "completed_segments": [],
            "failed_chunks": [],
            "notes": [],
            "totals": {"queued": 1, "reviewed": 0, "findings": 0, "counted_files": 1, "total_code_count": 1, "total_line_count": 1, "chunk_count": 1, "completed_chunks": 0},
        })

        artifacts = cli._ensure_audit_session(
            tmp_path,
            run_config={"command": "oy audit", "mode": cli._AUDIT_DEFAULT_MODE, "model": "copilot:gpt5.4", "agent": "default", "max_context_tokens": 131072, "phase2_workers": 8, "phase2_launch_delay_seconds": 0},
        )

        state = rt.load_toon(session_path, {})
        issues_text = (tmp_path / "ISSUES.md").read_text(encoding="utf-8")
        assert artifacts["created"] is False
        assert state["run_config"]["model"] == "copilot:gpt5.4"
        assert issues_text.startswith("# Audit Issues\n\n> Generated with [oy-cli](https://github.com/wagov-dtt/oy-cli): `OY_MODEL=copilot:gpt5.4 oy audit`")

    def test_run_audit_workflow_uses_limited_tool_registry(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        session_path = cli._audit_session_path(tmp_path)
        rt._ensure_private_dir(session_path.parent)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        rt.save_toon(session_path, {
            "version": cli._AUDIT_STATE_VERSION,
            "workspace": str(tmp_path),
            "focus": "",
            "status": "in_progress",
            "active_phase": "phase2",
            "phases": [
                {"id": "phase1", "status": "done", "label": "plan", "notes": []},
                {"id": "phase2", "status": "pending", "label": "review", "notes": []},
                {"id": "phase3", "status": "pending", "label": "summary", "notes": []},
            ],
            "sloc": {"counted_files": 1, "total_code_count": 1},
            "files": [{"path": "a.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10}],
            "chunks": [{"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1, "segments": [], "segment_count": 0}],
            "completed_chunks": [],
            "completed_segments": [],
            "failed_chunks": [],
            "notes": [],
            "totals": {"queued": 1, "reviewed": 0, "findings": 0, "counted_files": 1, "total_code_count": 1, "total_line_count": 1, "chunk_count": 1, "completed_chunks": 0},
        })
        seen = []

        def fake_run_agent(*_args, **kwargs):
            seen.append(kwargs)
            if len(seen) == 1:
                (tmp_path / "ISSUES.md").write_text("# Audit Issues\n\n## Finding\n\n## Another finding\n", encoding="utf-8")
            else:
                (tmp_path / "ISSUES.md").write_text("# Audit Issues\n\n## Finding\n\n## Another finding\n\n## Summary\n", encoding="utf-8")
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)

        assert cli._run_audit_workflow(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
        ) == 0
        assert set(seen[0]["tool_registry"]) == {"search", "inbox_append"}
        assert set(seen[-1]["tool_registry"]) == {"replace"}
        assert seen[0]["auto_approve_tools"] == {"inbox_append"}
        assert seen[-1]["auto_approve_tools"] == {"replace"}
        assert seen[0]["pin_user_prompt"] is True
        assert seen[-1]["pin_user_prompt"] is True
        assert seen[0]["wait_label_suffix"] == "audit phase2 | files 0/1 | chunks 0/1 | chunk-001 | findings 0"
        assert seen[-1]["wait_label_suffix"] == "audit phase3 | files 1/1 | chunks 1/1 | findings 2"


    def test_audit_wait_label_suffix_renders_progress(self):
        state = {
            "active_phase": "phase2",
            "totals": {"queued": 7, "reviewed": 3, "chunk_count": 4, "completed_chunks": 1, "findings": 2},
        }

        assert cli._audit_wait_label_suffix(state, chunk={"id": "chunk-002"}) == (
            "audit phase2 | files 3/7 | chunks 1/4 | chunk-002 | findings 2"
        )
        assert cli._audit_wait_label_suffix({
            "active_phase": "phase3",
            "totals": {"queued": 7, "reviewed": 7, "chunk_count": 4, "completed_chunks": 4, "findings": 5},
        }) == "audit phase3 | files 7/7 | chunks 4/4 | findings 5"

    def test_audit_wait_label_suffix_supports_phase3_detail(self):
        assert cli._audit_wait_label_suffix({
            "active_phase": "phase3",
            "totals": {"queued": 7, "reviewed": 7, "chunk_count": 4, "completed_chunks": 4, "findings": 5},
        }, detail="condense-001") == "audit phase3 | files 7/7 | chunks 4/4 | condense-001 | findings 5"

    def test_audit_system_prompt_for_mode_supports_phase_specific_prompts(self):
        assert cli._audit_system_prompt_for_mode(cli._AUDIT_DEFAULT_MODE, phase="phase1") == cli.AUDIT_PHASE1_SYSTEM_PROMPT
        assert cli._audit_system_prompt_for_mode(cli._AUDIT_DEFAULT_MODE, phase="phase2") == cli.AUDIT_PHASE2_SYSTEM_PROMPT
        assert cli._audit_system_prompt_for_mode(cli._AUDIT_DEFAULT_MODE, phase="phase3") == cli.AUDIT_PHASE3_SYSTEM_PROMPT
        assert cli._audit_system_prompt_for_mode(cli._AUDIT_LOGIC_MODE, phase="phase2") == cli.LOGIC_AUDIT_PHASE2_SYSTEM_PROMPT


    def test_audit_append_inbox_adds_entries_without_rewriting_report(self, tmp_path):
        (tmp_path / "ISSUES.md").write_text(
            "# Audit Issues\n\n"
            "> scope\n\n"
            "## Inbox\n\n"
            "Append-only review inbox for phase2. Add new candidate findings here without merging or renumbering; phase3 rewrites the final report.\n\n"
            "No inbox findings recorded yet.\n\n"
            "## Short audit log\n\n"
            "- bootstrapped\n",
            encoding="utf-8",
        )

        payload = cli._audit_append_inbox(tmp_path, "### New finding\n\n- evidence")
        text = (tmp_path / "ISSUES.md").read_text(encoding="utf-8")

        assert payload["path"] == "ISSUES.md"
        assert payload["chars_appended"] == len("### New finding\n\n- evidence")
        assert "### New finding" in text
        assert "No inbox findings recorded yet." not in text
        assert "## Short audit log" in text

    def test_run_audit_chunk_uses_inbox_context(self, tmp_path, monkeypatch):
        patch_runtime(monkeypatch, _print=None, _note=None)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        (tmp_path / "ISSUES.md").write_text(
            "# Audit Issues\n\n"
            "## Historical finding\n\nold detail\n\n"
            "## Inbox\n\n"
            "Append-only review inbox for phase2. Add new candidate findings here without merging or renumbering; phase3 rewrites the final report.\n\n"
            "### Inbox finding\n\n- short note\n\n"
            "## Short audit log\n\n"
            "- Phase1 dependency assessment: inspected newest relevant Renovate report `.tmp/renovate-2026-01-02.json`: no clear dependency risk.\n",
            encoding="utf-8",
        )
        seen = {}

        def fake_run_agent(prompt, *_args, **kwargs):
            seen["prompt"] = prompt
            seen["kwargs"] = kwargs
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)

        assert cli._run_audit_chunk(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
            chunk={"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1},
            state={"sloc": {}, "completed_chunks": [], "totals": {"queued": 1, "reviewed": 0, "findings": 1}},
        ) == (0, "")
        assert "Current audit inbox:" in seen["prompt"]
        assert "### Inbox finding" in seen["prompt"]
        assert "Historical finding" not in seen["prompt"]
        assert "Phase1 dependency assessment" not in seen["prompt"]
        assert set(seen["kwargs"]["tool_registry"]) == {"search", "inbox_append"}
        assert seen["kwargs"]["auto_approve_tools"] == {"inbox_append"}
        assert seen["kwargs"]["pin_user_prompt"] is True

    def test_run_audit_chunk_uses_segment_excerpt(self, tmp_path, monkeypatch):
        patch_runtime(monkeypatch, _print=None, _note=None)
        (tmp_path / "a.py").write_text("\n".join(f"value_{i} = {i}" for i in range(400)), encoding="utf-8")
        (tmp_path / "ISSUES.md").write_text(
            "# Audit Issues\n\n"
            "## Inbox\n\n"
            "Append-only review inbox for phase2. Add new candidate findings here without merging or renumbering; phase3 rewrites the final report.\n\n"
            "### Inbox finding\n\n- short note\n",
            encoding="utf-8",
        )
        seen = {}

        def fake_run_agent(prompt, *_args, **kwargs):
            seen["prompt"] = prompt
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)
        segment = {"id": "chunk-001#01", "index": 1, "start": 0, "end": 50, "estimated_tokens": 50}

        assert cli._run_audit_chunk(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
            chunk={"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 400, "files": 1, "segment_count": 2},
            state={"sloc": {}, "completed_chunks": [], "totals": {"queued": 1, "reviewed": 0, "findings": 1}},
            segment=segment,
        ) == (0, "")
        assert "Review only segment 1/2" in seen["prompt"]
        assert "<segment tokens 0:50>" in seen["prompt"]

    def test_audit_review_worker_buffers_inbox_entries_until_serial_merge(self, tmp_path, monkeypatch):
        patch_runtime(monkeypatch, _print=None, _note=None)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        (tmp_path / "ISSUES.md").write_text(
            "# Audit Issues\n\n"
            "## Inbox\n\n"
            "Append-only review inbox for phase2. Add new candidate findings here without merging or renumbering; phase3 rewrites the final report.\n\n"
            "No inbox findings recorded yet.\n",
            encoding="utf-8",
        )

        def fake_run_agent(_prompt, *_args, **kwargs):
            payload = kwargs["tool_registry"]["inbox_append"]["fn"]({"interactive": False, "unattended_deadline": 999999, "unattended_limit_seconds": 999999}, "### Buffered finding\n\n- detail")
            assert payload["buffered"] is True
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)
        result = cli._audit_review_worker(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
            chunk={"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1},
            state={"sloc": {}, "completed_chunks": [], "totals": {"queued": 1, "reviewed": 0, "findings": 0}},
            inbox_text=cli._audit_inbox_context(tmp_path),
        )
        assert result["code"] == 0
        assert result["entries"] == ["### Buffered finding\n\n- detail"]
        assert "Buffered finding" not in (tmp_path / "ISSUES.md").read_text(encoding="utf-8")

        merged = cli._audit_merge_buffered_entries(tmp_path, result["entries"])
        assert merged["merged"] == 1
        assert "Buffered finding" in (tmp_path / "ISSUES.md").read_text(encoding="utf-8")


    def test_audit_review_worker_reviews_all_segments_for_oversized_chunk(self, tmp_path, monkeypatch):
        patch_runtime(monkeypatch, _print=None, _note=None)
        (tmp_path / "a.py").write_text("\n".join(f"value_{i} = {i}" for i in range(400)), encoding="utf-8")
        (tmp_path / "ISSUES.md").write_text(
            "# Audit Issues\n\n"
            "## Inbox\n\n"
            "Append-only review inbox for phase2. Add new candidate findings here without merging or renumbering; phase3 rewrites the final report.\n\n"
            "No inbox findings recorded yet.\n",
            encoding="utf-8",
        )
        prompts = []

        def fake_run_agent(prompt, *_args, **kwargs):
            prompts.append(prompt)
            payload = kwargs["tool_registry"]["inbox_append"]["fn"](
                {"interactive": False, "unattended_deadline": 999999, "unattended_limit_seconds": 999999},
                f"### Segment {len(prompts)}\n\n- detail",
            )
            assert payload["buffered"] is True
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)
        segment_one = {"id": "chunk-001#01", "index": 1, "start": 0, "end": 50, "estimated_tokens": 50, "path": "a.py"}
        segment_two = {"id": "chunk-001#02", "index": 2, "start": 50, "end": 100, "estimated_tokens": 50, "path": "a.py"}

        result = cli._audit_review_worker(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
            chunk={"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 400, "files": 1, "segments": [segment_one, segment_two], "segment_count": 2},
            state={"sloc": {}, "completed_chunks": [], "completed_segments": [], "totals": {"queued": 1, "reviewed": 0, "findings": 0}},
            inbox_text=cli._audit_inbox_context(tmp_path),
        )
        assert result["code"] == 0
        assert len(prompts) == 2
        assert "Review only segment 1/2 from a.py" in prompts[0]
        assert "Review only segment 2/2 from a.py" in prompts[1]
        assert "### Segment 1" in prompts[1]
        assert result["completed_segments"] == ["chunk-001#01", "chunk-001#02"]
        assert result["entries"] == ["### Segment 1\n\n- detail", "### Segment 2\n\n- detail"]

    def test_run_audit_workflow_persists_threaded_worker_results_as_completed(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        session_path = cli._audit_session_path(tmp_path)
        rt._ensure_private_dir(session_path.parent)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        (tmp_path / "b.py").write_text("print('b')\n", encoding="utf-8")
        rt.save_toon(session_path, {
            "version": cli._AUDIT_STATE_VERSION,
            "workspace": str(tmp_path),
            "focus": "",
            "status": "in_progress",
            "active_phase": "phase2",
            "run_config": {"command": "oy audit", "mode": cli._AUDIT_DEFAULT_MODE, "model": "copilot:gpt5.4", "agent": "default", "max_context_tokens": 131072, "phase2_workers": 2, "phase2_launch_delay_seconds": 0},
            "phases": [
                {"id": "phase1", "status": "done", "label": "plan", "notes": []},
                {"id": "phase2", "status": "pending", "label": "review", "notes": []},
                {"id": "phase3", "status": "pending", "label": "summary", "notes": []},
            ],
            "sloc": {"counted_files": 2, "total_code_count": 2},
            "files": [
                {"path": "a.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10},
                {"path": "b.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10},
            ],
            "chunks": [
                {"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1, "segments": [], "segment_count": 0},
                {"id": "chunk-002", "paths": ["b.py"], "estimated_tokens": 10, "files": 1, "segments": [], "segment_count": 0},
            ],
            "completed_chunks": [],
            "completed_segments": [],
            "failed_chunks": [],
            "notes": [],
            "totals": {"queued": 2, "reviewed": 0, "findings": 0, "counted_files": 2, "total_code_count": 2, "total_line_count": 2, "chunk_count": 2, "completed_chunks": 0},
        })
        observed = {"persisted": False}

        def fake_review_worker(*, chunk, workspace, **_kwargs):
            if chunk["id"] == "chunk-001":
                return {
                    "chunk": dict(chunk),
                    "code": 0,
                    "message": "",
                    "entries": ["### Finding one\n\n- detail"],
                    "before_text": cli._audit_read_issues(workspace),
                    "completed_segments": [],
                }
            deadline = cli.time.monotonic() + 2
            while cli.time.monotonic() < deadline:
                saved = rt.load_toon(session_path, {})
                issues_text = cli._audit_read_issues(workspace)
                if "chunk-001" in saved.get("completed_chunks", []) and "Finding one" in issues_text:
                    observed["persisted"] = True
                    break
                cli.time.sleep(0.01)
            return {
                "chunk": dict(chunk),
                "code": 0 if observed["persisted"] else 1,
                "message": "" if observed["persisted"] else "chunk-001 did not persist before batch end",
                "entries": ["### Finding two\n\n- detail"] if observed["persisted"] else [],
                "before_text": cli._audit_read_issues(workspace),
                "completed_segments": [],
            }

        def fake_run_summary(**kwargs):
            workspace = kwargs["workspace"]
            text = cli._audit_read_issues(workspace)
            (workspace / "ISSUES.md").write_text(text + "\n## Summary\n", encoding="utf-8")
            return 0, ""

        monkeypatch.setattr(cli, "_audit_review_worker", fake_review_worker)
        monkeypatch.setattr(cli, "_run_audit_summary", fake_run_summary)

        assert cli._run_audit_workflow(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
        ) == 0
        saved = rt.load_toon(session_path, {})
        assert observed["persisted"] is True
        assert saved["completed_chunks"] == ["chunk-001", "chunk-002"]
        assert "Finding one" in (tmp_path / "ISSUES.md").read_text(encoding="utf-8")

    def test_run_audit_workflow_persists_completed_chunks_before_failing_batch(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        session_path = cli._audit_session_path(tmp_path)
        rt._ensure_private_dir(session_path.parent)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        (tmp_path / "b.py").write_text("print('b')\n", encoding="utf-8")
        rt.save_toon(session_path, {
            "version": cli._AUDIT_STATE_VERSION,
            "workspace": str(tmp_path),
            "focus": "",
            "status": "in_progress",
            "active_phase": "phase2",
            "run_config": {"command": "oy audit", "mode": cli._AUDIT_DEFAULT_MODE, "model": "copilot:gpt5.4", "agent": "default", "max_context_tokens": 131072, "phase2_workers": 2, "phase2_launch_delay_seconds": 0},
            "phases": [
                {"id": "phase1", "status": "done", "label": "plan", "notes": []},
                {"id": "phase2", "status": "pending", "label": "review", "notes": []},
                {"id": "phase3", "status": "pending", "label": "summary", "notes": []},
            ],
            "sloc": {"counted_files": 2, "total_code_count": 2},
            "files": [
                {"path": "a.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10},
                {"path": "b.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10},
            ],
            "chunks": [
                {"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1, "segments": [], "segment_count": 0},
                {"id": "chunk-002", "paths": ["b.py"], "estimated_tokens": 10, "files": 1, "segments": [], "segment_count": 0},
            ],
            "completed_chunks": [],
            "completed_segments": [],
            "failed_chunks": [],
            "notes": [],
            "totals": {"queued": 2, "reviewed": 0, "findings": 0, "counted_files": 2, "total_code_count": 2, "total_line_count": 2, "chunk_count": 2, "completed_chunks": 0},
        })

        def fake_review_worker(*, chunk, workspace, **_kwargs):
            if chunk["id"] == "chunk-001":
                text = cli._audit_read_issues(workspace)
                (workspace / "ISSUES.md").write_text(text + "\n### Finding one\n\n- detail\n", encoding="utf-8")
                return {
                    "chunk": dict(chunk),
                    "code": 0,
                    "message": "",
                    "entries": [],
                    "before_text": text,
                    "completed_segments": [],
                }
            return {
                "chunk": dict(chunk),
                "code": 1,
                "message": "boom",
                "entries": [],
                "before_text": cli._audit_read_issues(workspace),
                "completed_segments": [],
                "failed_segment": None,
            }

        monkeypatch.setattr(cli, "_audit_review_worker", fake_review_worker)

        assert cli._run_audit_workflow(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
        ) == 1
        saved = rt.load_toon(session_path, {})
        assert "chunk-001" in saved["completed_chunks"]
        assert saved["failed_chunks"]
        assert saved["failed_chunks"][0]["id"] == "chunk-002"

    def test_run_audit_workflow_staggers_thread_launches(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        session_path = cli._audit_session_path(tmp_path)
        rt._ensure_private_dir(session_path.parent)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        (tmp_path / "b.py").write_text("print('b')\n", encoding="utf-8")
        rt.save_toon(session_path, {
            "version": cli._AUDIT_STATE_VERSION,
            "workspace": str(tmp_path),
            "focus": "",
            "status": "in_progress",
            "active_phase": "phase2",
            "run_config": {"command": "oy audit", "mode": cli._AUDIT_DEFAULT_MODE, "model": "copilot:gpt5.4", "agent": "default", "max_context_tokens": 131072, "phase2_workers": 8, "phase2_launch_delay_seconds": 10},
            "phases": [
                {"id": "phase1", "status": "done", "label": "plan", "notes": []},
                {"id": "phase2", "status": "pending", "label": "review", "notes": []},
                {"id": "phase3", "status": "pending", "label": "summary", "notes": []},
            ],
            "sloc": {"counted_files": 2, "total_code_count": 2},
            "files": [
                {"path": "a.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10},
                {"path": "b.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10},
            ],
            "chunks": [
                {"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1, "segments": [], "segment_count": 0},
                {"id": "chunk-002", "paths": ["b.py"], "estimated_tokens": 10, "files": 1, "segments": [], "segment_count": 0},
            ],
            "completed_chunks": [],
            "completed_segments": [],
            "failed_chunks": [],
            "notes": [],
            "totals": {"queued": 2, "reviewed": 0, "findings": 0, "counted_files": 2, "total_code_count": 2, "total_line_count": 2, "chunk_count": 2, "completed_chunks": 0},
        })
        sleeps = []
        monkeypatch.setattr(cli.time, "sleep", lambda seconds: sleeps.append(seconds))

        calls = []

        def fake_run_agent(*_args, **_kwargs):
            calls.append(1)
            if len(calls) == 3:
                (tmp_path / "ISSUES.md").write_text("# Audit Issues\n\n## Summary\n", encoding="utf-8")
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)

        assert cli._run_audit_workflow(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
        ) == 0
        assert sleeps == [10]

    def test_run_audit_workflow_phase2_only_skips_summary(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        session_path = cli._audit_session_path(tmp_path)
        rt._ensure_private_dir(session_path.parent)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        rt.save_toon(session_path, {
            "version": cli._AUDIT_STATE_VERSION,
            "workspace": str(tmp_path),
            "focus": "",
            "status": "in_progress",
            "active_phase": "phase2",
            "run_config": {"command": "oy audit", "mode": cli._AUDIT_DEFAULT_MODE, "model": "copilot:gpt5.4", "agent": "default", "max_context_tokens": 131072, "phase2_workers": 1, "phase2_launch_delay_seconds": 0},
            "phases": [
                {"id": "phase1", "status": "done", "label": "plan", "notes": []},
                {"id": "phase2", "status": "pending", "label": "review", "notes": []},
                {"id": "phase3", "status": "pending", "label": "summary", "notes": []},
            ],
            "sloc": {"counted_files": 1, "total_code_count": 1},
            "files": [{"path": "a.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10}],
            "chunks": [{"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1, "segments": [], "segment_count": 0}],
            "completed_chunks": [],
            "completed_segments": [],
            "failed_chunks": [],
            "notes": [],
            "totals": {"queued": 1, "reviewed": 0, "findings": 0, "counted_files": 1, "total_code_count": 1, "total_line_count": 1, "chunk_count": 1, "completed_chunks": 0},
        })
        (tmp_path / "ISSUES.md").write_text("# Audit Issues\n\n## Inbox\n\n- awaiting review\n", encoding="utf-8")

        def fake_review_worker(*, chunk, workspace, **_kwargs):
            text = cli._audit_read_issues(workspace)
            (workspace / "ISSUES.md").write_text(text + "\n### Finding one\n\n- detail\n", encoding="utf-8")
            return {
                "chunk": dict(chunk),
                "code": 0,
                "message": "",
                "entries": [],
                "before_text": text,
                "completed_segments": [],
            }

        monkeypatch.setattr(cli, "_audit_review_worker", fake_review_worker)
        monkeypatch.setattr(cli, "_run_audit_summary", lambda **_kwargs: pytest.fail("phase2-only run should not summarize"))

        assert cli._run_audit_workflow(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
            session_path=session_path,
            phase="phase2",
        ) == 0
        saved = rt.load_toon(session_path, {})
        assert saved["status"] == "in_progress"
        assert saved["active_phase"] == "phase3"
        assert saved["completed_chunks"] == ["chunk-001"]

    def test_run_audit_workflow_phase3_only_skips_review_and_runs_summary(self, tmp_path, monkeypatch):
        monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path / ".sessions")
        patch_runtime(monkeypatch, _print=None, _note=None)
        session_path = cli._audit_session_path(tmp_path)
        rt._ensure_private_dir(session_path.parent)
        (tmp_path / "a.py").write_text("print('a')\n", encoding="utf-8")
        rt.save_toon(session_path, {
            "version": cli._AUDIT_STATE_VERSION,
            "workspace": str(tmp_path),
            "focus": "",
            "status": "in_progress",
            "active_phase": "phase2",
            "run_config": {"command": "oy audit", "mode": cli._AUDIT_DEFAULT_MODE, "model": "copilot:gpt5.4", "agent": "default", "max_context_tokens": 131072, "phase2_workers": 1, "phase2_launch_delay_seconds": 0},
            "phases": [
                {"id": "phase1", "status": "done", "label": "plan", "notes": []},
                {"id": "phase2", "status": "done", "label": "review", "notes": []},
                {"id": "phase3", "status": "pending", "label": "summary", "notes": []},
            ],
            "sloc": {"counted_files": 1, "total_code_count": 1},
            "files": [{"path": "a.py", "language": "Python", "code_count": 1, "line_count": 1, "size_bytes": 10, "estimated_tokens": 10}],
            "chunks": [{"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1, "segments": [], "segment_count": 0}],
            "completed_chunks": ["chunk-001"],
            "completed_segments": [],
            "failed_chunks": [],
            "notes": [],
            "totals": {"queued": 1, "reviewed": 1, "findings": 1, "counted_files": 1, "total_code_count": 1, "total_line_count": 1, "chunk_count": 1, "completed_chunks": 1},
        })
        (tmp_path / "ISSUES.md").write_text("# Audit Issues\n\n## Inbox\n\n### Finding\n\n- detail\n", encoding="utf-8")
        called = {"review": 0, "summary": 0}

        monkeypatch.setattr(cli, "_audit_review_worker", lambda **_kwargs: (_ for _ in ()).throw(AssertionError("phase3-only run should not review chunks")))
        monkeypatch.setattr(cli, "_audit_prepare_summary_input", lambda **kwargs: (0, cli._audit_read_issues(kwargs["workspace"])))

        def fake_run_summary(**kwargs):
            called["summary"] += 1
            text = cli._audit_read_issues(kwargs["workspace"])
            (kwargs["workspace"] / "ISSUES.md").write_text(text + "\n## Summary\n", encoding="utf-8")
            return 0, ""

        monkeypatch.setattr(cli, "_run_audit_summary", fake_run_summary)

        assert cli._run_audit_workflow(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            system_prompt="sys",
            unattended_limit_seconds=60,
            agent="default",
            session_path=session_path,
            phase="phase3",
        ) == 0
        saved = rt.load_toon(session_path, {})
        assert called["summary"] == 1
        assert saved["status"] == "done"
        assert saved["active_phase"] == "phase3"

    def test_audit_prepare_summary_input_iteratively_condenses_prefix_until_final_pass_fits(self, tmp_path, monkeypatch):
        patch_runtime(monkeypatch, _print=None, _note=None)
        issues_path = tmp_path / "ISSUES.md"
        huge_text = "# Audit Issues\n\n" + "\n\n".join(
            f"## Finding {i}\n\n- detail {'x' * 1200}"
            for i in range(220)
        ) + "\n"
        issues_path.write_text(huge_text, encoding="utf-8")
        state = {
            "active_phase": "phase3",
            "completed_chunks": ["chunk-001"],
            "notes": [],
            "totals": {"queued": 1, "reviewed": 1, "findings": 220, "chunk_count": 1, "completed_chunks": 1},
        }
        waits = []

        def fake_run_agent(prompt, _model, workspace, _system_prompt, _timeout, interactive=False, transcript=None, agent=None, tool_registry=None, auto_approve_tools=None, wait_label_suffix=None, pin_user_prompt=None):
            assert interactive is False
            assert transcript is not None
            assert agent == "default"
            assert pin_user_prompt is True
            waits.append(wait_label_suffix)
            assert auto_approve_tools == {"replace_prefix"}
            assert set(tool_registry) == {"replace_prefix"}
            current = cli._audit_read_issues(workspace)
            shortened = cli._audit_summary_prefix_text(current, max_tokens=512)
            payload = tool_registry["replace_prefix"]["fn"](
                {"interactive": False, "unattended_deadline": 999999, "unattended_limit_seconds": 999999, "root": workspace},
                shortened,
            )
            assert payload["after_tokens"] < payload["before_tokens"]
            return 0, ""

        monkeypatch.setattr(cli, "run_agent", fake_run_agent)
        monkeypatch.setattr(cli.rt, "count_tokens", lambda text: len(text))
        monkeypatch.setattr(cli.rt, "encode_tokens", lambda text: list(text))
        monkeypatch.setattr(cli.rt, "decode_tokens", lambda tokens: "".join(tokens))

        code, prepared = cli._audit_prepare_summary_input(
            prompt="prompt",
            model="openai:gpt-test",
            workspace=tmp_path,
            unattended_limit_seconds=60,
            agent="default",
            state=state,
            max_context_tokens=20000,
            mode=cli._AUDIT_DEFAULT_MODE,
        )
        assert code == 0
        assert waits
        assert waits[0].startswith("audit phase3 | files 1/1 | chunks 1/1 | condense-001 | findings 220")
        assert len(prepared) <= cli._audit_summary_content_budget(
            max_context_tokens=20000,
            prompt_text=cli._audit_summary_prompt("prompt", state, issues_rel="ISSUES.md"),
            system_prompt=cli._audit_system_prompt_for_mode(cli._AUDIT_DEFAULT_MODE, phase="phase3"),
        )
        assert len(cli._audit_read_issues(tmp_path)) == len(prepared)

