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
        lambda **_kwargs: _session_state(tmp_path, interactive=interactive),
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
        [(["fix", "tests"], ["run", "fix", "tests"]), (["ralph", "fix", "tests"], ["ralph", "fix", "tests"]), (["renovate-local"], ["renovate-local"]), (["--continue"], ["run", "--continue-session"]), (["-c"], ["run", "--continue-session"]), (["--resume", "abc123"], ["run", "--resume", "abc123"])],
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
        assert state["focus"] == "auth"
        assert state["workspace"] == str(tmp_path)
        assert state["active_phase"] == "phase2"
        assert {item["path"] for item in state["files"]} == {".gitignore", "README.md", "script", "src/main.py"}
        assert state["chunks"]
        assert all(chunk["estimated_tokens"] <= 64_000 or len(chunk["paths"]) == 1 for chunk in state["chunks"])
        assert state["totals"]["chunk_count"] == len(state["chunks"])
        assert state["phases"][0]["status"] == "done"
        assert state["phases"][1]["status"] == "pending"

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

        assert cli.audit("auth") == 0
        assert "64k chunks" in seen["prompt"]
        assert str(artifacts["session_path"]) in seen["prompt"]
        assert seen["workspace"] == tmp_path
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
            "chunks": [{"id": "chunk-001", "paths": ["a.py", "b.py"], "estimated_tokens": 20, "files": 2}],
            "completed_chunks": [],
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
        assert len(calls) == 4
        assert saved["status"] == "done"
        assert saved["failed_chunks"]
        assert saved["completed_chunks"]
        assert "Important finding" in (tmp_path / "ISSUES.md").read_text(encoding="utf-8")

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
            "chunks": [{"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1}],
            "completed_chunks": [],
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
        assert len(seen) == 2
        assert all(tx["max_context_tokens"] == 4321 for tx in seen)
        assert all(tx["max_message_tokens"] == 4321 for tx in seen)
        assert all(tx["messages"] == [] for tx in seen)

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
            "chunks": [{"id": "chunk-001", "paths": ["a.py"], "estimated_tokens": 10, "files": 1}],
            "completed_chunks": [],
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
        assert set(seen[0]["tool_registry"]) == {"search", "replace"}
        assert set(seen[1]["tool_registry"]) == {"replace"}
        assert seen[0]["auto_approve_tools"] == {"replace"}
        assert seen[1]["auto_approve_tools"] == {"replace"}
