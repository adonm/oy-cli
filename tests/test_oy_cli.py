from __future__ import annotations

import logging
from types import SimpleNamespace
import unittest
from pathlib import Path
import httpx
from unittest.mock import patch

import msgspec

import oy_cli
from oy_cli.shim import AssistantMessage, SystemMessage, ToolCall, ToolMessage, ToolResult, ToolSpec, UserMessage


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
            patch.object(oy_cli.runtime, "require_api_env") as require_api_env,
            patch.object(oy_cli.runtime, "resolve_active_shim", return_value="openai") as resolve,
            patch.object(oy_cli.runtime, "default_region", return_value="ap-southeast-2") as default_region,
            patch.object(oy_cli.runtime.Path, "cwd", return_value=Path("/workspace")),
            patch.object(oy_cli.runtime, "_shim_get_client", return_value=sentinel),
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
            patch.object(oy_cli.runtime, "_shim_ensure_api_env", side_effect=fake_ensure),
            patch.object(oy_cli.runtime, "_model", return_value="openai:gpt-test"),
            patch.object(oy_cli.runtime, "_shim_name", return_value="openai"),
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
        with patch.object(oy_cli.agent.time, "monotonic", return_value=10.0):
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
        with patch.object(oy_cli.agent, "count_tokens", side_effect=lambda text: len(text)):
            prepared = transcript.prepared_messages()
        self.assertEqual(prepared[0], SystemMessage("sys"))
        self.assertIsInstance(prepared[1], UserMessage)
        self.assertIn("earlier messages omitted", prepared[1].content)
        self.assertEqual(prepared[-1], UserMessage("kl"))

    def test_prepared_messages_keeps_tool_calls_and_outputs_together(self):
        transcript = oy_cli.Transcript(
            messages=[
                SystemMessage("sys"),
                UserMessage("earlier"),
                AssistantMessage(
                    "",
                    tool_calls=[ToolCall(id="call_1", name="bash", arguments={})],
                ),
                ToolMessage(
                    "call_1",
                    "bash",
                    ToolResult(ok=True, content="tool output"),
                ),
                UserMessage("tail"),
            ],
            max_context_tokens=23,
            max_message_tokens=100,
        )
        with patch.object(oy_cli.agent, "count_tokens", side_effect=lambda text: len(text)):
            prepared = transcript.prepared_messages()

        self.assertEqual(
            prepared,
            [
                SystemMessage("sys"),
                UserMessage("... [3 earlier messages omitted to fit context limit]"),
                UserMessage("tail"),
            ],
        )

    def test_prepared_messages_packs_older_history_with_toons_before_dropping(self):
        transcript = oy_cli.Transcript(
            messages=[
                SystemMessage("sys"),
                UserMessage("abcdef"),
                UserMessage("ghij"),
                UserMessage("mnop"),
                UserMessage("kl"),
            ],
            max_context_tokens=80,
            max_message_tokens=100,
        )
        packed_note = SystemMessage("packed")
        with (
            patch.object(oy_cli.agent, "count_tokens", side_effect=lambda text: len(text)),
            patch.object(oy_cli.agent, "_packed_history_note", return_value=packed_note),
        ):
            prepared = transcript.prepared_messages(model="gpt-4o")

        self.assertEqual(
            prepared,
            [SystemMessage("sys"), packed_note, UserMessage("mnop"), UserMessage("kl")],
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
            with patch.object(oy_cli.runtime, "show"):
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
            with patch.object(oy_cli.runtime, "show"):
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
            with patch.object(oy_cli.runtime, "show"):
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
                    oy_cli.runtime, "require_command_env", return_value={"PATH": "/bin"}
                ),
                patch.object(oy_cli.runtime, "which", return_value="/bin/bash"),
                patch.object(oy_cli.runtime, "run_cmd", return_value=result) as run_cmd,
                patch.object(oy_cli.runtime, "show"),
                patch.object(oy_cli.runtime, "_print"),
            ):
                payload = oy_cli.tool_bash(
                    self._state(root), "printf out; printf err >&2", timeout_seconds=30
                )

        run_cmd.assert_called_once_with(
            ["/bin/bash", "-c", "printf out; printf err >&2"],
            cwd=root,
            env={"PATH": "/bin"},
            timeout=30,
        )
        self.assertEqual(payload["command"], "printf out; printf err >&2")
        self.assertEqual(payload["exit_code"], 1)
        self.assertFalse(payload["ok"])
        self.assertEqual(payload["content_format"], "text")
        self.assertIn("[stdout]", payload["content"])
        self.assertIn("[stderr]", payload["content"])
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
                    oy_cli.runtime, "require_command_env", return_value={"PATH": "/bin"}
                ),
                patch.object(oy_cli.runtime, "which", return_value="/bin/bash"),
                patch.object(oy_cli.runtime, "run_cmd", return_value=result),
                patch.object(oy_cli.runtime, "show"),
                patch.object(oy_cli.runtime, "_print"),
            ):
                payload = oy_cli.tool_bash(self._state(root), "echo json")

        self.assertTrue(payload["ok"])
        self.assertEqual(payload["content_format"], "toon")
        self.assertIsInstance(payload["content"], str)
        self.assertIn("count", payload["content"])
        self.assertIn("items", payload["content"])
        self.assertFalse(payload["truncated"])

    def test_bash_preview_uses_toon_block(self):
        result = SimpleNamespace(
            returncode=1,
            stdout='{"count": 2, "items": [1, 2]}',
            stderr="warn\n",
        )

        rendered = oy_cli._render_bash_preview(
            "echo json", result, {"content_format": "toon"}
        )

        self.assertIn("```bash", rendered)
        self.assertIn("$ echo json", rendered)
        self.assertIn("count", rendered)
        self.assertIn("items", rendered)
        self.assertNotIn("```json", rendered)
        self.assertIn("- exit 1", rendered)
        self.assertIn("**stderr**", rendered)

    def test_bash_preview_prefers_existing_toon_output(self):
        result = SimpleNamespace(returncode=0, stdout='{"count": 2}', stderr="")

        rendered = oy_cli._render_bash_preview(
            "echo json", result, {"content_format": "toon", "content": "count: 2"}
        )

        self.assertIn("count: 2", rendered)
        self.assertNotIn('{"count": 2}', rendered)


class SearchToolTests(unittest.TestCase):
    def _state(self, root: Path):
        return make_state(root)

    def test_search_returns_structured_matches(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "src").mkdir()
            (root / "src" / "main.py").write_text("needle\n", encoding="utf-8")

            with patch.object(oy_cli.runtime, "show"):
                result = oy_cli.tool_search(self._state(root), "needle", path="src")

        self.assertEqual(result["match_count"], 1)
        self.assertFalse(result["truncated"])
        self.assertEqual(
            result["matches"],
            [{"path": "src/main.py", "line_number": 1, "column": 1, "text": "needle"}],
        )

    def test_search_respects_requested_file_path(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "src").mkdir()
            (root / "src" / "main.py").write_text("needle\n", encoding="utf-8")
            (root / "src" / "other.py").write_text("needle\n", encoding="utf-8")

            with patch.object(oy_cli.runtime, "show"):
                result = oy_cli.tool_search(self._state(root), "needle", path="src/main.py")

        self.assertEqual(result["match_count"], 1)
        self.assertEqual(result["matches"][0]["path"], "src/main.py")

    def test_search_respects_gitignore_and_limit(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / ".gitignore").write_text("ignored.txt\n", encoding="utf-8")
            (root / "src").mkdir()
            (root / "src" / "main.py").write_text("needle\nneedle again\n", encoding="utf-8")
            (root / "ignored.txt").write_text("needle\n", encoding="utf-8")

            with patch.object(oy_cli.runtime, "show"):
                result = oy_cli.tool_search(self._state(root), "needle", limit=1)

        self.assertEqual(result["match_count"], 2)
        self.assertTrue(result["truncated"])
        self.assertEqual(len(result["matches"]), 1)
        self.assertEqual(result["matches"][0]["path"], "src/main.py")

    def test_search_uses_workspace_gitignore_for_subpaths(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / ".gitignore").write_text("src/generated/\n", encoding="utf-8")
            (root / "src" / "generated").mkdir(parents=True)
            (root / "src" / "generated" / "artifact.py").write_text(
                "needle\n", encoding="utf-8"
            )
            (root / "src" / "main.py").write_text("needle\n", encoding="utf-8")

            with patch.object(oy_cli.runtime, "show"):
                result = oy_cli.tool_search(self._state(root), "needle", path="src")

        self.assertEqual(result["match_count"], 1)
        self.assertEqual(result["matches"][0]["path"], "src/main.py")


class ReplaceToolTests(unittest.TestCase):
    def _state(self, root: Path):
        return make_state(root)

    def test_replace_tool_registered(self):
        names = [spec.name for spec in oy_cli.TOOL_REGISTRY.specs()]
        self.assertIn("replace", names)

    def test_replace_args_schema_has_replacement(self):
        schema = msgspec.json.schema(oy_cli.ReplaceArgs)
        defs = schema.get("$defs", {})
        props = defs.get("ReplaceArgs", schema).get("properties", {})
        self.assertIn("pattern", props)
        self.assertIn("replacement", props)

    def test_replace_updates_matching_files(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "src").mkdir()
            target = root / "src" / "main.py"
            target.write_text("alpha needle beta needle\n", encoding="utf-8")

            with patch.object(oy_cli.runtime, "show"):
                result = oy_cli.tool_replace(
                    self._state(root),
                    "needle",
                    "thread",
                    path="src",
                )

            self.assertEqual(target.read_text(encoding="utf-8"), "alpha thread beta thread\n")
            self.assertEqual(result["changed_file_count"], 1)
            self.assertEqual(result["replacement_count"], 2)
            self.assertFalse(result["truncated"])
            self.assertEqual(
                result["changed_files"],
                [{"path": "src/main.py", "replacements": 2}],
            )

    def test_replace_respects_gitignore_and_skips_archives(self):
        import tempfile
        import zipfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / ".gitignore").write_text("ignored.txt\n", encoding="utf-8")
            (root / "src").mkdir()
            target = root / "src" / "main.txt"
            target.write_text("needle\n", encoding="utf-8")
            (root / "ignored.txt").write_text("needle\n", encoding="utf-8")
            with zipfile.ZipFile(root / "bundle.zip", "w") as archive:
                archive.writestr("inner.txt", "needle\n")

            with patch.object(oy_cli.runtime, "show"):
                result = oy_cli.tool_replace(
                    self._state(root),
                    "needle",
                    "thread",
                )

            self.assertEqual(target.read_text(encoding="utf-8"), "thread\n")
            self.assertEqual((root / "ignored.txt").read_text(encoding="utf-8"), "needle\n")
            self.assertEqual(result["changed_file_count"], 1)
            self.assertEqual(result["replacement_count"], 1)
            self.assertEqual(result["skipped_count"], 1)
            self.assertEqual(
                result["skipped"],
                [{"path": "bundle.zip", "reason": "archive"}],
            )

    def test_replace_uses_workspace_gitignore_for_subpaths(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / ".gitignore").write_text("src/generated/\n", encoding="utf-8")
            (root / "src" / "generated").mkdir(parents=True)
            generated = root / "src" / "generated" / "artifact.txt"
            generated.write_text("needle\n", encoding="utf-8")
            main = root / "src" / "main.txt"
            main.write_text("needle\n", encoding="utf-8")

            with patch.object(oy_cli.runtime, "show"):
                result = oy_cli.tool_replace(
                    self._state(root),
                    "needle",
                    "thread",
                    path="src",
                )

            self.assertEqual(main.read_text(encoding="utf-8"), "thread\n")
            self.assertEqual(generated.read_text(encoding="utf-8"), "needle\n")
            self.assertEqual(result["changed_file_count"], 1)
            self.assertEqual(
                result["changed_files"],
                [{"path": "src/main.txt", "replacements": 1}],
            )

    def test_replace_rejects_invalid_pattern(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            state = self._state(root)

            with self.assertRaisesRegex(ValueError, "Invalid replace pattern"):
                oy_cli.tool_replace(state, "(", "thread")


class SlocToolTests(unittest.TestCase):
    def _state(self, root: Path):
        return make_state(root)

    def test_sloc_tool_registered(self):
        names = [spec.name for spec in oy_cli.TOOL_REGISTRY.specs()]
        self.assertIn("sloc", names)

    def test_sloc_args_schema_has_path(self):
        schema = msgspec.json.schema(oy_cli.SlocArgs)
        defs = schema.get("$defs", {})
        props = defs.get("SlocArgs", schema).get("properties", {})
        self.assertIn("path", props)

    def test_sloc_counts_source_lines(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "pkg").mkdir()
            (root / "pkg" / "main.py").write_text(
                '"""docs"""\n\nprint("hi")\n', encoding="utf-8"
            )
            (root / "pkg" / "util.js").write_text(
                '// docs\nconst value = 1;\n', encoding="utf-8"
            )

            with patch.object(oy_cli.runtime, "show"):
                payload = oy_cli.tool_sloc(self._state(root), path="pkg")

        self.assertEqual(payload["path"], "pkg")
        self.assertEqual(payload["total_file_count"], 2)
        self.assertGreaterEqual(payload["total_code_count"], 2)
        self.assertEqual(payload["language_count"], 2)
        languages = {item["language"] for item in payload["languages"]}
        self.assertEqual(languages, {"Python", "JavaScript"})
        self.assertFalse(payload["truncated"])

    def test_sloc_reports_non_countable_files(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            bad = root / "broken.py"
            bad.write_bytes(b'print("ok")\n\x80\n')

            with patch.object(oy_cli.runtime, "show"):
                payload = oy_cli.tool_sloc(self._state(root), path="broken.py")

        self.assertEqual(payload["total_file_count"], 1)
        self.assertEqual(payload["total_code_count"], 0)
        self.assertEqual(payload["error_count"], 1)
        self.assertEqual(payload["state_counts"], [{"state": "error", "file_count": 1}])
        self.assertEqual(payload["errors"][0]["path"], "broken.py")

    def test_sloc_uses_workspace_gitignore_for_subpaths(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / ".gitignore").write_text("pkg/generated/\n", encoding="utf-8")
            (root / "pkg" / "generated").mkdir(parents=True)
            (root / "pkg" / "generated" / "artifact.py").write_text(
                'print("skip")\n', encoding="utf-8"
            )
            (root / "pkg" / "main.py").write_text('print("keep")\n', encoding="utf-8")

            with patch.object(oy_cli.runtime, "show"):
                payload = oy_cli.tool_sloc(self._state(root), path="pkg")

        self.assertEqual(payload["total_file_count"], 1)
        self.assertEqual(payload["languages"][0]["file_count"], 1)


class PrivatePathPermissionTests(unittest.TestCase):
    def test_save_cfg_hardens_config_directory_permissions(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            config_path = Path(d) / "config" / "oy" / "config.json"
            config_dir = config_path.parent
            config_dir.mkdir(parents=True, mode=0o755, exist_ok=True)
            config_dir.chmod(0o755)

            with patch.dict(oy_cli.runtime.os.environ, {"OY_CONFIG": str(config_path)}, clear=False):
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

            with patch.object(oy_cli.runtime, "CONFIG_PATH", config_path):
                oy_cli._create_prompt_session()

            self.assertEqual(history_dir.stat().st_mode & 0o777, 0o700)
            self.assertEqual(history_path.stat().st_mode & 0o777, 0o600)

    def test_git_diff_shortstat_reports_clean_repo(self):
        result = SimpleNamespace(returncode=0, stdout="", stderr="")

        with patch.object(oy_cli.cli, "run_cmd", return_value=result) as run_cmd:
            summary = oy_cli._git_diff_shortstat(Path("/workspace"))

        run_cmd.assert_called_once_with(
            [
                "git",
                "-C",
                "/workspace",
                "diff",
                "--shortstat",
                "--no-ext-diff",
                "HEAD",
                "--",
            ],
            timeout=5,
        )
        self.assertEqual(summary, "git diff: clean")

    def test_read_input_includes_git_diff_context(self):
        prompts: list[object] = []

        class PromptSessionStub:
            def prompt(self, value):
                prompts.append(value)
                return "hello"

        with (
            patch.object(oy_cli.cli, "_git_diff_shortstat", return_value="1 file changed, 2 insertions(+)"),
            patch("prompt_toolkit.formatted_text.ANSI", side_effect=lambda text: text),
        ):
            result = oy_cli._read_input(PromptSessionStub(), Path("/workspace"))

        self.assertEqual(result, "hello")
        self.assertEqual(
            prompts,
            [
                "\x1b[2m1 file changed, 2 insertions(+)\x1b[0m\n"
                "\x1b[1;32moy ❯\x1b[0m "
            ],
        )

    def test_handle_save_hardens_sessions_directory_permissions(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            sessions_dir = Path(d) / "sessions"
            sessions_dir.mkdir(parents=True, mode=0o755, exist_ok=True)
            sessions_dir.chmod(0o755)
            transcript = oy_cli.Transcript.with_system_prompt("sys")

            with (
                patch.object(oy_cli.cli, "_SESSIONS_DIR", sessions_dir),
                patch.object(oy_cli.runtime, "_print"),
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
                    patch.dict(oy_cli.runtime.os.environ, {"OY_DEBUG": "1"}, clear=False),
                    patch.object(oy_cli.runtime, "CONFIG_PATH", config_path),
                    patch.object(oy_cli.runtime.logging, "getLogger", return_value=logger),
                ):
                    _, log_path = oy_cli._init_debug_log()
            finally:
                for handler in logger.handlers:
                    handler.close()
                logger.handlers.clear()

            self.assertEqual(debug_dir.stat().st_mode & 0o777, 0o700)
            self.assertEqual(Path(log_path).stat().st_mode & 0o777, 0o600)


class ToonsPackingTests(unittest.TestCase):
    def test_packed_history_note_uses_toon_text(self):
        note = oy_cli._packed_history_note(
            [UserMessage("hello"), UserMessage("world")]
        )

        self.assertIn("Packed earlier conversation history in TOON.", note.content)
        self.assertIn("messages", note.content)
        self.assertIn("role", note.content)
        self.assertIn("content", note.content)

    def test_pack_messages_with_toons_skips_tool_state(self):
        messages = [
            SystemMessage("sys"),
            UserMessage("hello"),
            AssistantMessage("", tool_calls=[ToolCall(id="call_1", name="bash", arguments={})]),
            ToolMessage("call_1", "bash", ToolResult(ok=True, content={"ok": True})),
            UserMessage("after"),
        ]

        packed = oy_cli._pack_messages_with_toons(messages)

        self.assertEqual(packed, messages)



class TestWebfetch(unittest.TestCase):
    """Tests for the webfetch tool and SSRF protection."""

    def test_validate_url_safe_allows_public_https(self):
        from oy_cli import _validate_url_safe

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
        from oy_cli.modes import _READ_ONLY_TOOLS

        self.assertIn("webfetch", _READ_ONLY_TOOLS)

    def test_sloc_in_read_only_tools(self):
        from oy_cli.modes import _READ_ONLY_TOOLS

        self.assertIn("sloc", _READ_ONLY_TOOLS)

    def test_noninteractive_tool_registry_excludes_ask(self):
        from oy_cli.modes import active_tool_specs

        names = [spec.name for spec in active_tool_specs(False).specs()]
        self.assertNotIn("ask", names)

    def test_read_only_tool_registry_matches_declared_names(self):
        from oy_cli.modes import _READ_ONLY_TOOLS, read_only_tool_specs

        self.assertEqual({spec.name for spec in read_only_tool_specs().specs()}, _READ_ONLY_TOOLS)

    def test_webfetch_args_schema_has_url_and_options(self):
        from oy_cli.tools import WebfetchArgs
        import msgspec

        schema = msgspec.json.schema(WebfetchArgs)
        defs = schema.get("$defs", {})
        props = defs.get("WebfetchArgs", schema).get("properties", {})
        self.assertIn("url", props)
        self.assertIn("options", props)

    def test_webfetch_uses_httpx_client_options(self):
        state = make_state(Path("/tmp/ok"))
        response = httpx.Response(
            200,
            headers={"content-type": "text/plain"},
            text="hello world",
            request=httpx.Request("GET", "https://example.com"),
        )

        class DummyClient:
            def __init__(self, **kwargs):
                self.kwargs = kwargs
                self.called = None

            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc, tb):
                return False

            def request(self, method, url, headers=None):
                self.called = (method, url, headers)
                return response

        created: list[DummyClient] = []

        def fake_http_client(**kwargs):
            client = DummyClient(**kwargs)
            created.append(client)
            return client

        with (
            patch.object(oy_cli.providers.httpx, "Client", side_effect=fake_http_client),
            patch.object(oy_cli.runtime, "show"),
        ):
            payload = oy_cli.tool_webfetch(
                state,
                url="https://example.com",
                method="GET",
                headers={"Accept": "text/plain"},
                options={"timeout_seconds": 12, "follow_redirects": True},
            )

        self.assertEqual(len(created), 1)
        self.assertEqual(created[0].kwargs, {"timeout": 12, "follow_redirects": True})
        self.assertEqual(created[0].called, ("GET", "https://example.com", {"Accept": "text/plain"}))
        self.assertEqual(payload["method"], "GET")
        self.assertEqual(payload["url"], "https://example.com")
        self.assertEqual(payload["status_code"], 200)
        self.assertTrue(payload["ok"])
        self.assertEqual(payload["content"], "hello world")
        self.assertEqual(payload["content_format"], "text")
        self.assertIn("hello world", oy_cli._webfetch_structured_text(payload))

    def test_html_to_markdown_keeps_useful_structure(self):
        html = """
        <html>
          <head><title>ignore me</title><style>.hidden { display: none; }</style></head>
          <body>
            <h1>Title</h1>
            <p>Hello <a href="/docs">docs</a>.</p>
            <ul><li>One</li><li>Two</li></ul>
            <pre>print("hi")</pre>
          </body>
        </html>
        """

        markdown = oy_cli._html_to_markdown(html)

        self.assertIn("Title\n=====", markdown)
        self.assertIn("Hello [docs](/docs).", markdown)
        self.assertIn("* One", markdown)
        self.assertIn("* Two", markdown)
        self.assertIn('```\nprint("hi")\n```', markdown)
        self.assertIn("ignore me", markdown)
        self.assertNotIn("display: none", markdown)

    def test_webfetch_converts_oversized_html_to_markdown(self):
        state = make_state(Path("/tmp/ok"))
        html = "<html><body><h1>Title</h1>" + "".join(
            f"<p>Paragraph {i} with <a href='/doc/{i}'>link {i}</a>.</p>"
            for i in range(20)
        ) + "</body></html>"
        response = httpx.Response(
            200,
            headers={"content-type": "text/html; charset=utf-8"},
            text=html,
            request=httpx.Request("GET", "https://example.com/page"),
        )

        class DummyClient:
            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc, tb):
                return False

            def request(self, method, url, headers=None):
                self.called = (method, url, headers)
                return response

        budgets = oy_cli.RuntimeBudgets(
            message_tokens=64,
            tool_output_tokens=40,
            tool_tail_tokens=10,
            default_line_limit=20,
        )
        client = DummyClient()
        with (
            patch.object(oy_cli.runtime, "BUDGETS", budgets),
            patch.object(oy_cli.runtime, "http_client", return_value=client),
            patch.object(oy_cli.runtime, "show"),
        ):
            payload = oy_cli.tool_webfetch(state, url="https://example.com/page")

        self.assertEqual(payload["content_format"], "markdown")
        self.assertIn("Title\n=====", payload["content"])
        self.assertIn("Paragraph 0 with [link 0](/doc/0).", payload["content"])
        self.assertNotIn("<html", payload["content"].lower())
        self.assertIn("content_format", oy_cli._webfetch_structured_text(payload))

    def test_webfetch_redacts_sensitive_response_headers(self):
        response = httpx.Response(
            302,
            headers={
                "Location": "https://secret.example/next",
                "Set-Cookie": "session=secret",
                "Content-Type": "text/plain",
            },
            text="redirecting",
            request=httpx.Request("GET", "https://example.com"),
        )

        headers = oy_cli._webfetch_response_headers(response)

        self.assertEqual(headers["Location"], "<redacted>")
        self.assertEqual(headers["Set-Cookie"], "<redacted>")
        self.assertEqual(headers["Content-Type"], "text/plain")

    def test_webfetch_structured_text_uses_toon(self):
        response = httpx.Response(
            200,
            headers={"content-type": "text/plain"},
            text="hello world",
            request=httpx.Request("GET", "https://example.com"),
        )

        payload = {
            "method": "GET",
            "url": "https://example.com",
            "status_code": 200,
            "reason_phrase": "OK",
            "http_version": "HTTP/1.1",
            "headers": oy_cli._webfetch_response_headers(response),
            "content": "hello world",
            "content_format": "text",
            "truncated": False,
        }
        rendered = oy_cli._webfetch_structured_text(payload)

        self.assertIn("content", rendered)
        self.assertIn("hello world", rendered)
        self.assertNotIn("HTTP/1.1 200 OK", rendered)

    def test_webfetch_non_2xx_keeps_status_code(self):
        response = httpx.Response(
            404,
            headers={"content-type": "text/plain"},
            text="missing",
            request=httpx.Request("GET", "https://example.com/missing"),
        )

        payload = oy_cli._webfetch_payload(
            response, method="GET", text="missing", truncated=False
        )

        self.assertEqual(payload["status_code"], 404)
        self.assertFalse(payload["ok"])
        self.assertEqual(payload["content"], "missing")
        self.assertEqual(payload["content_format"], "text")
        self.assertIn("missing", oy_cli._webfetch_structured_text(payload))

    def test_webfetch_request_error_returns_structured_payload(self):
        state = make_state(Path("/tmp/ok"))

        class DummyClient:
            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc, tb):
                return False

            def request(self, method, url, headers=None):
                raise httpx.ConnectError("boom", request=httpx.Request(method, url))

        with (
            patch.object(oy_cli.runtime, "http_client", return_value=DummyClient()),
            patch.object(oy_cli.runtime, "show"),
        ):
            payload = oy_cli.tool_webfetch(state, url="https://example.com")

        self.assertEqual(payload["method"], "GET")
        self.assertEqual(payload["url"], "https://example.com")
        self.assertFalse(payload["ok"])
        self.assertEqual(payload["error_type"], "ConnectError")
        self.assertIn("boom", payload["message"])


class TestModuleExecution(unittest.TestCase):
    def test_python_m_oy_cli_help(self):
        result = oy_cli.run_cmd(
            ["uv", "run", "python", "-m", "oy_cli", "--help"],
            cwd=Path.cwd(),
            env=oy_cli.command_env(Path.cwd()),
        )

        self.assertEqual(result.returncode, 0)
        self.assertIn("usage:", result.stdout)
        self.assertIn("{run,chat,model,audit}", result.stdout)


class TestTodo(unittest.TestCase):
    def test_todo_tool_registered(self):
        from oy_cli import TOOL_REGISTRY

        names = [s.name for s in TOOL_REGISTRY.specs()]
        self.assertIn("todo", names)

    def test_todo_args_schema_has_todos(self):
        from oy_cli.tools import TodoArgs

        schema = msgspec.json.schema(TodoArgs)
        defs = schema.get("$defs", {})
        props = defs.get("TodoArgs", schema).get("properties", {})
        self.assertIn("todos", props)

    def test_todo_tool_rejects_invalid_status(self):
        state = make_state(Path("/tmp/ok"))

        with self.assertRaisesRegex(ValueError, r"Invalid status 'blocked'"):
            oy_cli.tool_todo(
                state,
                [{"id": "1", "task": "rename todo tool", "status": "blocked"}],
            )


if __name__ == "__main__":
    unittest.main()
