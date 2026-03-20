from __future__ import annotations

import json
import logging
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import msgspec

import oy_cli
from oy_cli.shim import SystemMessage, ToolMessage, ToolResult, ToolSpec, UserMessage


def make_state(root: Path, tool_specs=None, *, unattended_deadline: float = float("inf")):
    return oy_cli.AgentState(
        root=root,
        tool_specs=oy_cli.TOOL_REGISTRY if tool_specs is None else tool_specs,
        unattended_timeout_seconds=3600,
        unattended_deadline=unattended_deadline,
    )


class EchoArgs(msgspec.Struct, omit_defaults=True):
    text: str


def _echo(state, text):
    return f"{state.root.name}:{text}"


class ShimDirectTests(unittest.TestCase):
    def test_get_client_uses_shim_module(self):
        sentinel = object()
        with (
            patch.object(oy_cli, "require_api_env") as require_api_env,
            patch.object(oy_cli, "resolve_active_shim", return_value="openai") as resolve,
            patch.object(oy_cli, "default_region", return_value="ap-southeast-2") as default_region,
            patch.object(oy_cli.Path, "cwd", return_value=Path("/workspace")),
            patch.object(oy_cli, "_shim_get_client", return_value=sentinel),
        ):
            client = oy_cli.get_client("openai:gpt-test")

        require_api_env.assert_called_once_with(Path("/workspace"))
        resolve.assert_called_once_with("openai:gpt-test")
        default_region.assert_called_once_with()
        self.assertIs(client, sentinel)

    def test_ensure_api_env_calls_shim_directly(self):
        calls: list[tuple[str | None, str | None, Path | None]] = []

        def fake_ensure(model_spec, configured_shim, cwd):
            calls.append((model_spec, configured_shim, cwd))
            return True, None

        with (
            patch.object(oy_cli, "_shim_ensure_api_env", side_effect=fake_ensure),
            patch.object(oy_cli, "_model", return_value="openai:gpt-test"),
            patch.object(oy_cli, "_shim_name", return_value="openai"),
        ):
            result = oy_cli.ensure_api_env(Path("/workspace"))

        self.assertEqual(calls, [("openai:gpt-test", "openai", Path("/workspace"))])
        self.assertTrue(result)


class ToolDispatchTests(unittest.TestCase):
    def test_registry_returns_structured_validation_errors(self):
        registry = oy_cli.ToolRegistry(
            {
                "echo": oy_cli.ToolHandler(
                    name="echo",
                    fn=_echo,
                    spec=ToolSpec("echo", "echo text", {"type": "object"}),
                    args_type=EchoArgs,
                )
            }
        )
        state = make_state(Path("/tmp/ok"), registry)
        result = registry.invoke(state, "echo", {})
        self.assertFalse(result.ok)
        self.assertEqual(result.content["tool"], "echo")
        self.assertIn("error_type", result.content)

    def test_agent_state_enforces_unattended_timeout(self):
        state = make_state(
            Path("/tmp/ok"), oy_cli.ToolRegistry(), unattended_deadline=10.0
        )
        with patch.object(oy_cli.time, "monotonic", return_value=10.0):
            with self.assertRaisesRegex(
                TimeoutError, r"reached unattended timeout \(1h\)"
            ):
                state.note_progress()


class TranscriptLifecycleTests(unittest.TestCase):
    def test_with_system_prompt_initializes_transcript(self):
        transcript = oy_cli.Transcript.with_system_prompt("sys")

        self.assertEqual(transcript.messages, [SystemMessage("sys")])

    def test_clear_resets_to_fresh_system_prompt(self):
        transcript = oy_cli.Transcript.with_system_prompt("sys")
        transcript.add_user("hello")

        transcript.clear("next")

        self.assertEqual(transcript.messages, [SystemMessage("next")])


class TranscriptTests(unittest.TestCase):
    def test_prepared_messages_drops_older_context(self):
        transcript = oy_cli.Transcript(
            messages=[
                SystemMessage("sys"),
                UserMessage("abcdef"),
                UserMessage("ghij"),
                UserMessage("kl"),
            ],
            max_context_tokens=18,
            max_message_tokens=100,
        )
        with patch.object(oy_cli, "count_tokens", side_effect=lambda text: len(text)):
            prepared = transcript.prepared_messages()
        self.assertEqual(prepared[0], SystemMessage("sys"))
        self.assertIsInstance(prepared[1], UserMessage)
        self.assertIn("earlier messages omitted", prepared[1].content)
        self.assertEqual(prepared[-1], UserMessage("kl"))

    def test_prepared_messages_uses_headroom_before_dropping_history(self):
        transcript = oy_cli.Transcript(
            messages=[
                SystemMessage("sys"),
                UserMessage("abcdef"),
                UserMessage("ghij"),
            ],
            max_context_tokens=21,
            max_message_tokens=100,
        )
        with (
            patch.object(oy_cli, "count_tokens", side_effect=lambda text: len(text)),
            patch.object(
                oy_cli,
                "headroom_compress",
                return_value=SimpleNamespace(
                    messages=[
                        {"role": "system", "content": "sys"},
                        {"role": "user", "content": ""},
                        {"role": "user", "content": "ghij"},
                    ]
                ),
            ) as mock_headroom,
        ):
            prepared = transcript.prepared_messages(model="gpt-4o")
        mock_headroom.assert_called_once()
        self.assertEqual(
            prepared,
            [
                SystemMessage("sys"),
                UserMessage(""),
                UserMessage("ghij"),
            ],
        )


class BudgetTests(unittest.TestCase):
    def test_runtime_budgets_scale_up_for_large_context_models(self):
        small = oy_cli._derive_runtime_budgets(32_768)
        large = oy_cli._derive_runtime_budgets(131_072)

        self.assertGreater(large.message_tokens, small.message_tokens)
        self.assertGreater(large.tool_output_tokens, small.tool_output_tokens)
        self.assertGreater(large.tool_tail_tokens, small.tool_tail_tokens)
        self.assertGreater(large.default_line_limit, small.default_line_limit)


class ResolvePathTests(unittest.TestCase):
    def test_resolve_path_rejects_traversal(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            with self.assertRaisesRegex(ValueError, "Path traversal denied"):
                oy_cli.resolve_path(root, "../../etc/passwd")


class ReadToolTests(unittest.TestCase):
    def _state(self, root: Path):
        return make_state(root)

    def test_read_file_supports_offset_and_limit(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "demo.txt").write_text("a\nb\nc\n", encoding="utf-8")
            with patch.object(oy_cli, "_print"):
                result = oy_cli.tool_read(
                    self._state(root), path="demo.txt", offset=2, limit=2
                )
        self.assertEqual(result, "2: b\n3: c")

    def test_read_directory_lists_entries(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "src").mkdir()
            (root / "src" / "main.py").write_text("print('hi')\n", encoding="utf-8")
            with patch.object(oy_cli, "_print"):
                result = oy_cli.tool_read(self._state(root), path="src")
        self.assertEqual(result, "src/main.py")


class ListToolTests(unittest.TestCase):
    def test_list_path_accepts_pathlib_glob(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "src").mkdir()
            (root / "src" / "main.py").write_text("print('hi')\n", encoding="utf-8")
            (root / "src" / "util.txt").write_text("helper\n", encoding="utf-8")
            state = make_state(root)
            with patch.object(oy_cli, "_print"):
                result = oy_cli.tool_list(state, path="src/*.py")
        self.assertEqual(result, "src/main.py")


class BashToolTests(unittest.TestCase):
    def _state(self, root: Path):
        return make_state(root)

    def test_bash_returns_structured_text_payload(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            result = SimpleNamespace(returncode=1, stdout="out\n", stderr="err\n")
            with (
                patch.object(
                    oy_cli, "require_command_env", return_value={"PATH": "/bin"}
                ),
                patch.object(oy_cli, "which", return_value="/bin/bash"),
                patch.object(
                    oy_cli, "run_cmd_auto_install", return_value=result
                ) as run_cmd,
                patch.object(oy_cli, "show"),
                patch.object(oy_cli, "_print"),
            ):
                payload = oy_cli.tool_bash(
                    self._state(root), "printf out; printf err >&2", timeout_seconds=30
                )

        run_cmd.assert_called_once_with(
            ["/bin/bash", "-c", "printf out; printf err >&2"],
            cwd=root,
            env={"PATH": "/bin"},
            timeout=30,
            reason="bash command",
        )
        self.assertEqual(payload["command"], "printf out; printf err >&2")
        self.assertEqual(payload["exit_code"], 1)
        self.assertFalse(payload["ok"])
        self.assertEqual(payload["output_format"], "text")
        self.assertIn("[stdout]", payload["output"])
        self.assertIn("[stderr]", payload["output"])
        self.assertFalse(payload["truncated"])

    def test_bash_parses_json_output_when_possible(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            result = SimpleNamespace(
                returncode=0,
                stdout='{"count": 2, "items": [1, 2]}',
                stderr="",
            )
            with (
                patch.object(
                    oy_cli, "require_command_env", return_value={"PATH": "/bin"}
                ),
                patch.object(oy_cli, "which", return_value="/bin/bash"),
                patch.object(oy_cli, "run_cmd_auto_install", return_value=result),
                patch.object(oy_cli, "show"),
                patch.object(oy_cli, "_print"),
            ):
                payload = oy_cli.tool_bash(self._state(root), "echo json")

        self.assertTrue(payload["ok"])
        self.assertEqual(payload["output_format"], "json")
        self.assertEqual(payload["output"], {"count": 2, "items": [1, 2]})
        self.assertFalse(payload["truncated"])

    def test_bash_preview_uses_pretty_json_block(self):
        result = SimpleNamespace(
            returncode=1,
            stdout='{"count": 2, "items": [1, 2]}',
            stderr="warn\n",
        )

        rendered = oy_cli._render_bash_preview(
            "echo json", result, {"output_format": "json"}
        )

        self.assertIn("```bash", rendered)
        self.assertIn("$ echo json", rendered)
        self.assertIn("```json", rendered)
        self.assertIn('  "count": 2,', rendered)
        self.assertIn('  "items": [', rendered)
        self.assertIn("- exit 1", rendered)
        self.assertIn("**stderr**", rendered)


class SearchToolTests(unittest.TestCase):
    def _state(self, root: Path):
        return make_state(root)

    def test_search_uses_ripgrep_output(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            rg_output = "\n".join(
                [
                    json.dumps(
                        {
                            "type": "match",
                            "data": {
                                "path": {"text": str(root / "src" / "main.py")},
                                "lines": {"text": "needle\n"},
                                "line_number": 1,
                                "submatches": [
                                    {"match": {"text": "needle"}, "start": 0, "end": 6}
                                ],
                            },
                        }
                    ),
                    json.dumps({"type": "summary", "data": {"stats": {"matches": 1}}}),
                ]
            ).encode()

            class Result:
                returncode = 0
                stdout = rg_output.decode()
                stderr = ""

            def fake_run_cmd(args, **kwargs):
                self.assertEqual(args[:4], ["rg", "--json", "--line-number", "--color"])
                self.assertIn("needle", args)
                self.assertIn(str(root / "src"), args)
                return Result()

            (root / "src").mkdir()
            (root / "src" / "main.py").write_text("needle\n", encoding="utf-8")
            with (
                patch.object(oy_cli, "command_env", return_value={"PATH": "/test/bin"}),
                patch.object(oy_cli, "run_cmd", fake_run_cmd),
                patch.object(oy_cli, "_print"),
            ):
                result = oy_cli.tool_search(self._state(root), "needle", path="src")

        self.assertIn("src/main.py:1:1:needle", result)

    def test_search_supports_rg_args_passthrough(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            rg_output = "\n".join(
                [
                    json.dumps(
                        {
                            "type": "match",
                            "data": {
                                "path": {"text": str(root / "src" / "main.py")},
                                "lines": {"text": "Needle\n"},
                                "line_number": 1,
                                "submatches": [
                                    {"match": {"text": "Needle"}, "start": 0, "end": 6}
                                ],
                            },
                        }
                    ),
                    json.dumps(
                        {
                            "type": "context",
                            "data": {
                                "path": {"text": str(root / "src" / "main.py")},
                                "lines": {"text": "after\n"},
                                "line_number": 2,
                            },
                        }
                    ),
                ]
            ).encode()

            class Result:
                returncode = 0
                stdout = rg_output.decode()
                stderr = ""

            def fake_run_cmd(args, **kwargs):
                self.assertIn("--json", args)
                self.assertIn("--line-number", args)
                self.assertIn("--glob", args)
                self.assertIn("*.py", args)
                self.assertIn("--ignore-case", args)
                self.assertIn("--word-regexp", args)
                self.assertIn("--fixed-strings", args)
                self.assertIn("--context", args)
                self.assertIn("1", args)
                return Result()

            (root / "src").mkdir()
            (root / "src" / "main.py").write_text("Needle\nafter\n", encoding="utf-8")
            with (
                patch.object(oy_cli, "command_env", return_value={"PATH": "/test/bin"}),
                patch.object(oy_cli, "run_cmd", fake_run_cmd),
                patch.object(oy_cli, "_print"),
            ):
                result = oy_cli.tool_search(
                    self._state(root),
                    "Needle",
                    path="src",
                    args=[
                        "--glob",
                        "*.py",
                        "--ignore-case",
                        "--word-regexp",
                        "--fixed-strings",
                        "--context",
                        "1",
                    ],
                )

        self.assertIn("src/main.py:1:1:Needle", result)
        self.assertIn("src/main.py-2-:after", result)


class PrivatePathPermissionTests(unittest.TestCase):
    def test_save_cfg_hardens_config_directory_permissions(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            config_path = Path(d) / "config" / "oy" / "config.json"
            config_dir = config_path.parent
            config_dir.mkdir(parents=True, mode=0o755, exist_ok=True)
            config_dir.chmod(0o755)

            with patch.dict(oy_cli.os.environ, {"OY_CONFIG": str(config_path)}, clear=False):
                oy_cli._save_cfg({"model": "gpt-test"})

            self.assertEqual(config_dir.stat().st_mode & 0o777, 0o700)
            self.assertEqual(config_path.stat().st_mode & 0o777, 0o600)

    def test_create_prompt_session_hardens_history_path_permissions(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            config_path = Path(d) / "config" / "oy" / "config.json"
            history_dir = config_path.parent
            history_dir.mkdir(parents=True, mode=0o755, exist_ok=True)
            history_dir.chmod(0o755)
            history_path = history_dir / "history"
            history_path.write_text("hello\n", encoding="utf-8")
            history_path.chmod(0o644)

            with patch.object(oy_cli, "CONFIG_PATH", config_path):
                oy_cli._create_prompt_session()

            self.assertEqual(history_dir.stat().st_mode & 0o777, 0o700)
            self.assertEqual(history_path.stat().st_mode & 0o777, 0o600)

    def test_handle_save_hardens_sessions_directory_permissions(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            sessions_dir = Path(d) / "sessions"
            sessions_dir.mkdir(parents=True, mode=0o755, exist_ok=True)
            sessions_dir.chmod(0o755)
            transcript = oy_cli.Transcript.with_system_prompt("sys")

            with (
                patch.object(oy_cli, "_SESSIONS_DIR", sessions_dir),
                patch.object(oy_cli, "_print"),
            ):
                oy_cli._handle_save("demo", transcript, "openai:gpt-test")

            self.assertEqual(sessions_dir.stat().st_mode & 0o777, 0o700)
            self.assertEqual((sessions_dir / "demo.json").stat().st_mode & 0o777, 0o600)

    def test_init_debug_log_hardens_debug_directory_permissions(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            config_path = Path(d) / "config" / "oy" / "config.json"
            debug_dir = config_path.parent
            debug_dir.mkdir(parents=True, mode=0o755, exist_ok=True)
            debug_dir.chmod(0o755)
            logger = logging.Logger("oy.test.debug")

            try:
                with (
                    patch.dict(oy_cli.os.environ, {"OY_DEBUG": "1"}, clear=False),
                    patch.object(oy_cli, "CONFIG_PATH", config_path),
                    patch.object(oy_cli.logging, "getLogger", return_value=logger),
                ):
                    _, log_path = oy_cli._init_debug_log()
            finally:
                for handler in logger.handlers:
                    handler.close()
                logger.handlers.clear()

            self.assertEqual(debug_dir.stat().st_mode & 0o777, 0o700)
            self.assertEqual(Path(log_path).stat().st_mode & 0o777, 0o600)


class OptionalToolInstallerTests(unittest.TestCase):
    def test_mise_install_command_returns_all_requested_recipes(self):
        self.assertEqual(
            oy_cli._mise_install_command(["rg", "unknown"]),
            [
                "mise",
                "use",
                "-g",
                "github:BurntSushi/ripgrep",
            ],
        )

    def test_missing_tool_message_recommends_installing_all_missing_tools(self):
        with patch.object(oy_cli, "which", return_value=None):
            message = oy_cli._missing_tool_install_message(
                ["rg"], "search", ["rg", "unknown"]
            )

        self.assertIn("Missing `rg` for search.", message)
        self.assertIn(
            "mise use -g github:BurntSushi/ripgrep",
            message,
        )

    def test_ensure_optional_tools_installs_all_missing_via_mise(self):
        env = {"PATH": "/test/bin"}
        refreshed = {"PATH": "/test/bin:/installed/bin"}

        calls = []

        def fake_which(name, path=None):
            if path == env["PATH"] and name == "rg":
                return None
            if path == env["PATH"] and name == "mise":
                return "/test/bin/mise"
            if path == refreshed["PATH"] and name == "rg":
                return f"/installed/bin/{name}"
            if path == refreshed["PATH"] and name == "mise":
                return "/installed/bin/mise"
            return None

        class InstallResult:
            returncode = 0
            stdout = "ok"
            stderr = ""

        class EnvResult:
            returncode = 0
            stdout = '{"PATH": "/test/bin:/installed/bin"}'
            stderr = ""

        def fake_run_cmd(args, **kwargs):
            calls.append((args, kwargs))
            return (
                EnvResult()
                if args[:3] == ["/test/bin/mise", "env", "-J"]
                else InstallResult()
            )

        with (
            patch.object(oy_cli, "command_env", side_effect=[env, refreshed]),
            patch.object(oy_cli, "which", side_effect=fake_which),
            patch.object(oy_cli, "run_cmd", side_effect=fake_run_cmd),
            patch.object(oy_cli, "_print"),
        ):
            oy_cli.command_env.cache_clear = lambda: None
            result = oy_cli.ensure_optional_tools("rg", reason="search")

        self.assertEqual(result, refreshed)
        self.assertEqual(
            calls[0][0],
            [
                "mise",
                "use",
                "-g",
                "github:BurntSushi/ripgrep",
                "github:ast-grep/ast-grep",
                "github:boyter/scc",
                "github:ducaale/xh",
                "github:mikefarah/yq",
                "github:starship/starship",
            ],
        )
        self.assertEqual(calls[1][0], ["/test/bin/mise", "env", "-J"])

    def test_run_cmd_auto_install_installs_all_missing_tools_for_missing_binary(self):
        env = {"PATH": "/test/bin"}
        refreshed = {"PATH": "/test/bin:/installed/bin"}

        class Result:
            returncode = 0
            stdout = "ok"
            stderr = ""

        def fake_run_cmd(args, **kwargs):
            if kwargs["env"] == env:
                raise FileNotFoundError(args[0])
            return Result()

        with (
            patch.object(oy_cli, "command_env", return_value=env),
            patch.object(
                oy_cli, "ensure_optional_tools", return_value=refreshed
            ) as install,
            patch.object(oy_cli, "run_cmd", side_effect=fake_run_cmd),
        ):
            result = oy_cli.run_cmd_auto_install(
                ["rg", "needle"], env=env, reason="search"
            )

        install.assert_called_once_with("rg", reason="search", cwd=None)
        self.assertEqual(result.stdout, "ok")

    def test_run_cmd_auto_install_installs_all_missing_tools_for_shell_helper(self):
        env = {"PATH": "/test/bin"}
        refreshed = {"PATH": "/test/bin:/installed/bin"}
        first = iter(
            [
                type(
                    "Result",
                    (),
                    {
                        "returncode": 127,
                        "stdout": "",
                        "stderr": "bash: line 1: rg: command not found",
                    },
                )(),
                type("Result", (), {"returncode": 0, "stdout": "ok", "stderr": ""})(),
            ]
        )

        with (
            patch.object(
                oy_cli, "ensure_optional_tools", return_value=refreshed
            ) as install,
            patch.object(
                oy_cli, "run_cmd", side_effect=lambda *args, **kwargs: next(first)
            ),
        ):
            result = oy_cli.run_cmd_auto_install(
                ["bash", "-c", "rg needle"], env=env, reason="bash command"
            )

        install.assert_called_once_with("rg", reason="bash command", cwd=None)
        self.assertEqual(result.stdout, "ok")

    def test_refresh_mise_env_updates_process_environment(self):
        env = {"PATH": "/test/bin"}
        refreshed = {"PATH": "/test/bin:/installed/bin", "FOO": "bar"}

        class Result:
            returncode = 0
            stdout = '{"PATH": "/test/bin:/installed/bin", "FOO": "bar"}'
            stderr = ""

        def fake_which(name, path=None):
            if name == "mise":
                return "/test/bin/mise"
            if name == "rg" and path == refreshed["PATH"]:
                return "/installed/bin/rg"
            return None

        original = dict(oy_cli.os.environ)
        try:
            with (
                patch.object(oy_cli, "command_env", side_effect=[env, refreshed]),
                patch.object(oy_cli, "which", side_effect=fake_which),
                patch.object(oy_cli, "run_cmd", return_value=Result()),
            ):
                oy_cli.command_env.cache_clear = lambda: None
                result = oy_cli._refresh_mise_env()
                self.assertEqual(oy_cli.os.environ["PATH"], refreshed["PATH"])
                self.assertEqual(oy_cli.os.environ["FOO"], "bar")
        finally:
            oy_cli.os.environ.clear()
            oy_cli.os.environ.update(original)

        self.assertEqual(result, refreshed)


class MainTests(unittest.TestCase):
    def test_main_fails_fast_when_mise_is_missing(self):
        with patch.object(
            oy_cli,
            "command_env",
            side_effect=RuntimeError(
                "`mise` is required; install and activate `mise` before running `oy`."
            ),
        ):
            with self.assertRaises(SystemExit) as ctx:
                oy_cli.main(["--version"])

        self.assertEqual(ctx.exception.code, 1)


class HeadroomSerializationTests(unittest.TestCase):
    def test_serialize_for_headroom_stringifies_tool_content(self):
        message = ToolMessage(
            tool_call_id="call_1",
            name="bash",
            content=ToolResult(ok=False, content={"count": 2, "ok": True}),
        )
        payload = oy_cli._serialize_for_headroom(message)
        self.assertEqual(payload["role"], "tool")
        self.assertFalse(payload["ok"])
        self.assertIsInstance(payload["content"], str)
        parsed = json.loads(payload["content"])
        self.assertEqual(parsed["count"], 2)

    def test_deserialize_from_headroom_restores_openai_tool_calls(self):
        message = oy_cli._deserialize_from_headroom(
            {
                "role": "assistant",
                "content": "checking",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "bash",
                            "arguments": '{"command":"pytest"}',
                        },
                    }
                ],
            }
        )
        self.assertEqual(message.content, "checking")
        self.assertEqual(len(message.tool_calls), 1)
        self.assertEqual(message.tool_calls[0].name, "bash")
        self.assertEqual(message.tool_calls[0].arguments, {"command": "pytest"})


if __name__ == "__main__":
    unittest.main()


class TestWebfetch(unittest.TestCase):
    """Tests for the webfetch tool and SSRF protection."""

    def test_validate_url_safe_allows_public_https(self):
        from oy_cli import _validate_url_safe
        # Should not raise
        result = _validate_url_safe("https://example.com")
        self.assertEqual(result, "https://example.com")

    def test_validate_url_safe_blocks_localhost(self):
        from oy_cli import _validate_url_safe
        with self.assertRaises(ValueError, msg="Local addresses are not allowed"):
            _validate_url_safe("http://localhost/secret")

    def test_validate_url_safe_blocks_loopback_ip(self):
        from oy_cli import _validate_url_safe
        with self.assertRaises(ValueError):
            _validate_url_safe("http://127.0.0.1/secret")

    def test_validate_url_safe_blocks_private_rfc1918(self):
        from oy_cli import _validate_url_safe
        for addr in ("10.0.0.1", "172.16.0.1", "192.168.1.1"):
            with self.assertRaises(ValueError, msg=f"{addr} should be blocked"):
                _validate_url_safe(f"http://{addr}/")

    def test_validate_url_safe_blocks_non_http_schemes(self):
        from oy_cli import _validate_url_safe
        for scheme in ("ftp", "file", "gopher", "dict", "ssh"):
            with self.assertRaises(ValueError, msg=f"{scheme}:// should be blocked"):
                _validate_url_safe(f"{scheme}://example.com/")

    def test_validate_url_safe_blocks_link_local(self):
        from oy_cli import _validate_url_safe
        with self.assertRaises(ValueError):
            _validate_url_safe("http://169.254.169.254/latest/meta-data/")

    def test_webfetch_rejects_post_method(self):
        from oy_cli import _WEBFETCH_ALLOWED_METHODS
        self.assertNotIn("POST", _WEBFETCH_ALLOWED_METHODS)
        self.assertNotIn("PUT", _WEBFETCH_ALLOWED_METHODS)
        self.assertNotIn("DELETE", _WEBFETCH_ALLOWED_METHODS)

    def test_webfetch_allows_safe_methods(self):
        from oy_cli import _WEBFETCH_ALLOWED_METHODS
        self.assertIn("GET", _WEBFETCH_ALLOWED_METHODS)
        self.assertIn("HEAD", _WEBFETCH_ALLOWED_METHODS)
        self.assertIn("OPTIONS", _WEBFETCH_ALLOWED_METHODS)

    def test_webfetch_tool_registered(self):
        from oy_cli import TOOL_REGISTRY
        names = [s.name for s in TOOL_REGISTRY.specs()]
        self.assertIn("webfetch", names)

    def test_webfetch_in_read_only_tools(self):
        from oy_cli import _READ_ONLY_TOOLS
        self.assertIn("webfetch", _READ_ONLY_TOOLS)

    def test_webfetch_args_schema_has_url(self):
        from oy_cli import WebfetchArgs
        import msgspec
        schema = msgspec.json.schema(WebfetchArgs)
        defs = schema.get("$defs", {})
        props = defs.get("WebfetchArgs", schema).get("properties", {})
        self.assertIn("url", props)
