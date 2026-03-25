from __future__ import annotations

import asyncio
import logging
from types import SimpleNamespace
import unittest
from pathlib import Path
import httpx
from unittest.mock import patch

import msgspec

import oy_cli
from oy_cli import AssistantMessage, SystemMessage, ToolCall, ToolMessage, ToolResult, ToolSpec, UserMessage


def make_state(
    root: Path,
    tool_specs=None,
    *,
    unattended_deadline: float = float("inf"),
    interactive: bool = False,
):
    return oy_cli.AgentState(
        root=root,
        tool_specs=oy_cli.TOOL_REGISTRY if tool_specs is None else tool_specs,
        unattended_timeout_seconds=3600,
        unattended_deadline=unattended_deadline,
        interactive=interactive,
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
            patch.object(oy_cli.runtime.Path, "cwd", return_value=Path("/workspace")),
            patch.object(oy_cli.runtime, "_shim_get_client", return_value=sentinel),
        ):
            client = oy_cli.get_client("openai:gpt-test")

        require_api_env.assert_called_once_with(Path("/workspace"))
        resolve.assert_called_once_with("openai:gpt-test")
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

    def test_mutating_tools_prompt_for_approval_in_interactive_mode(self):
        calls: list[str] = []

        def mutating(state, text):
            calls.append(text)
            return f"done:{text}"

        registry = oy_cli.ToolRegistry(
            {
                "mutating": oy_cli.ToolHandler(
                    name="mutating",
                    fn=mutating,
                    spec=ToolSpec("mutating", "mutates state", {"type": "object"}),
                    args_type=EchoArgs,
                    mutating=True,
                )
            }
        )
        state = make_state(Path("/tmp/ok"), registry, interactive=True)
        with (
            patch.object(oy_cli.runtime, "_print") as print_mock,
            patch.object(oy_cli.runtime.Prompt, "select", return_value="once") as ask_mock,
        ):
            result = registry.invoke(state, "mutating", {"text": "hi"})

        ask_mock.assert_called_once_with(
            "Approve mutating tool?",
            ["once", "all", "deny"],
            console=oy_cli.runtime.STDERR,
            default="once",
            prompt_label="Approval",
            option_text=unittest.mock.ANY,
        )
        print_mock.assert_called_once()
        self.assertTrue(result.ok)
        self.assertEqual(result.content, "done:hi")
        self.assertEqual(calls, ["hi"])

    def test_mutating_tools_can_be_approved_for_rest_of_session(self):
        calls: list[str] = []

        def mutating(state, text):
            calls.append(text)
            return f"done:{text}"

        registry = oy_cli.ToolRegistry(
            {
                "mutating": oy_cli.ToolHandler(
                    name="mutating",
                    fn=mutating,
                    spec=ToolSpec("mutating", "mutates state", {"type": "object"}),
                    args_type=EchoArgs,
                    mutating=True,
                )
            }
        )
        state = make_state(Path("/tmp/ok"), registry, interactive=True)
        with (
            patch.object(oy_cli.runtime, "_print"),
            patch.object(oy_cli.runtime, "_note"),
            patch.object(oy_cli.runtime.Prompt, "select", return_value="all") as ask_mock,
        ):
            first = registry.invoke(state, "mutating", {"text": "first"})
            second = registry.invoke(state, "mutating", {"text": "second"})

        self.assertTrue(first.ok)
        self.assertTrue(second.ok)
        self.assertTrue(state.approve_all_mutating_tools)
        self.assertEqual(calls, ["first", "second"])
        ask_mock.assert_called_once()

    def test_mutating_tools_can_be_denied_in_interactive_mode(self):
        invoked = False

        def mutating(state, text):
            nonlocal invoked
            invoked = True
            return f"done:{text}"

        registry = oy_cli.ToolRegistry(
            {
                "mutating": oy_cli.ToolHandler(
                    name="mutating",
                    fn=mutating,
                    spec=ToolSpec("mutating", "mutates state", {"type": "object"}),
                    args_type=EchoArgs,
                    mutating=True,
                )
            }
        )
        state = make_state(Path("/tmp/ok"), registry, interactive=True)
        with (
            patch.object(oy_cli.runtime, "_print"),
            patch.object(oy_cli.runtime, "_note"),
            patch.object(oy_cli.runtime.Prompt, "select", return_value="deny") as ask_mock,
        ):
            result = registry.invoke(state, "mutating", {"text": "nope"})

        ask_mock.assert_called_once()
        self.assertFalse(result.ok)
        self.assertEqual(result.content["tool"], "mutating")
        self.assertEqual(result.content["error_type"], "PermissionError")
        self.assertIn("denied approval", result.content["message"])
        self.assertFalse(invoked)

    def test_mutating_tools_do_not_prompt_in_noninteractive_mode(self):
        calls: list[str] = []

        def mutating(state, text):
            calls.append(text)
            return f"done:{text}"

        registry = oy_cli.ToolRegistry(
            {
                "mutating": oy_cli.ToolHandler(
                    name="mutating",
                    fn=mutating,
                    spec=ToolSpec("mutating", "mutates state", {"type": "object"}),
                    args_type=EchoArgs,
                    mutating=True,
                )
            }
        )
        state = make_state(Path("/tmp/ok"), registry, interactive=False)
        with patch.object(oy_cli.runtime.Prompt, "select") as ask_mock:
            result = registry.invoke(state, "mutating", {"text": "ci"})

        ask_mock.assert_not_called()
        self.assertTrue(result.ok)
        self.assertEqual(result.content, "done:ci")
        self.assertEqual(calls, ["ci"])


class PromptRuntimeTests(unittest.TestCase):
    def test_prompt_needs_thread_false_without_running_loop(self):
        self.assertFalse(oy_cli.runtime._prompt_needs_thread())

    def test_prompt_ask_uses_in_thread_when_loop_running(self):
        sessions: list[object] = []

        class SessionStub:
            def prompt(self, *args, **kwargs):
                sessions.append(kwargs)
                return "ok"

        async def run_test():
            with patch.object(oy_cli.runtime, "_prompt_session", return_value=SessionStub()):
                return oy_cli.runtime.Prompt.ask("Q", console=oy_cli.runtime.STDERR)

        result = asyncio.run(run_test())
        self.assertEqual(result, "ok")
        self.assertEqual(len(sessions), 1)
        self.assertTrue(sessions[0]["in_thread"])

    def test_prompt_select_uses_in_thread_when_loop_running(self):
        sessions: list[object] = []

        class SessionStub:
            def prompt(self, *args, **kwargs):
                sessions.append(kwargs)
                return "1"

        async def run_test():
            with patch.object(oy_cli.runtime, "_prompt_session", return_value=SessionStub()):
                return oy_cli.runtime.Prompt.select("Pick", ["one", "two"], console=oy_cli.runtime.STDERR)

        result = asyncio.run(run_test())
        self.assertEqual(result, "one")
        self.assertEqual(len(sessions), 1)
        self.assertTrue(sessions[0]["in_thread"])


class PromptGuardTests(unittest.TestCase):
    def test_require_prompt_reports_noninteractive_env(self):
        with patch.dict(oy_cli.runtime.os.environ, {"OY_NON_INTERACTIVE": "1"}, clear=False):
            with self.assertRaisesRegex(ValueError, "OY_NON_INTERACTIVE=1"):
                oy_cli.runtime.require_prompt("ask question")


class AskToolTests(unittest.TestCase):
    def test_ask_tool_uses_prompt_text_for_freeform_answers(self):
        state = make_state(Path("/tmp/ok"))
        with (
            patch.object(oy_cli.runtime, "require_prompt"),
            patch.object(oy_cli.runtime, "note_tool"),
            patch.object(oy_cli.runtime.Prompt, "ask", return_value="typed answer") as ask_mock,
        ):
            result = oy_cli.tool_ask(state, question="What now?")

        self.assertEqual(result, "typed answer")
        ask_mock.assert_called_once_with("What now?", console=oy_cli.runtime.STDERR, default="")

    def test_ask_tool_uses_select_for_choices(self):
        state = make_state(Path("/tmp/ok"))
        with (
            patch.object(oy_cli.runtime, "require_prompt"),
            patch.object(oy_cli.runtime, "note_tool"),
            patch.object(oy_cli.runtime.Prompt, "select", return_value="beta") as select_mock,
        ):
            result = oy_cli.tool_ask(state, question="Pick one", choices=["alpha", "beta"])

        self.assertEqual(result, "beta")
        select_mock.assert_called_once_with(
            "Pick one",
            ["alpha", "beta"],
            console=oy_cli.runtime.STDERR,
            prompt_label="Selection",
            option_text=unittest.mock.ANY,
        )

    def test_ask_tool_requires_tty(self):
        state = make_state(Path("/tmp/ok"))
        with (
            patch.object(oy_cli.runtime, "require_prompt", side_effect=ValueError("Cannot ask question: stdin is not a TTY")),
            patch.object(oy_cli.runtime, "note_tool"),
        ):
            with self.assertRaisesRegex(ValueError, "Cannot ask question: stdin is not a TTY"):
                oy_cli.tool_ask(state, question="blocked")


class TranscriptLifecycleTests(unittest.TestCase):
    def test_with_system_prompt_initializes_transcript(self):
        transcript = oy_cli.Transcript.with_system_prompt("sys")

        self.assertEqual(transcript.messages, [SystemMessage("sys")])

    def test_clear_resets_to_fresh_system_prompt(self):
        transcript = oy_cli.Transcript.with_system_prompt("sys")
        transcript.add_user("hello")

        transcript.clear("next")

        self.assertEqual(transcript.messages, [SystemMessage("next")])

    def test_undo_last_turn_removes_last_user_message_and_followups(self):
        transcript = oy_cli.Transcript(
            messages=[
                SystemMessage("sys"),
                UserMessage("first"),
                UserMessage("second"),
                AssistantMessage("working", tool_calls=[ToolCall(id="call_1", name="bash", arguments={})]),
                ToolMessage("call_1", "bash", ToolResult(ok=True, content="done")),
            ]
        )

        undone = transcript.undo_last_turn()

        self.assertTrue(undone)
        self.assertEqual(
            transcript.messages,
            [
                SystemMessage("sys"),
                UserMessage("first"),
            ],
        )

    def test_undo_last_turn_returns_false_without_user_messages(self):
        transcript = oy_cli.Transcript.with_system_prompt("sys")

        undone = transcript.undo_last_turn()

        self.assertFalse(undone)
        self.assertEqual(transcript.messages, [SystemMessage("sys")])


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
    def test_list_path_dot_lists_workspace_root(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "src").mkdir()
            (root / "README.md").write_text("hi\n", encoding="utf-8")
            state = make_state(root)
            with patch.object(oy_cli.runtime, "show"):
                result = oy_cli.tool_list(state, path=".")
        self.assertEqual(result, "README.md\nsrc/")

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
                patch.object(oy_cli.runtime, "_note"),
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
                patch.object(oy_cli.runtime, "_note"),
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
        self.assertIn("[status] exit 1", rendered)
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


class SessionTextPromptTests(unittest.TestCase):
    def test_interactive_suffix_stays_simple(self):
        text = oy_cli.interactive_system_prompt()
        self.assertIn("Use `ask` only for genuine ambiguity or irreversible user-facing choices.", text)
        self.assertIn("Batch changes to minimise prompts.", text)
        self.assertNotIn("approval layer", text)
        self.assertNotIn("need approval", text)
        self.assertNotIn("reply approve", text)

    def test_noninteractive_suffix_stays_generic(self):
        text = oy_cli.noninteractive_system_prompt()
        self.assertIn("Non-interactive mode: do not pause for questions;", text)
        self.assertNotIn("approval", text)
        self.assertNotIn("approve", text)
        self.assertNotIn("clarification", text)

    def test_ask_tool_description_stays_generic(self):
        text = oy_cli.tool_description("ask")
        self.assertIn("Ask the user in interactive runs.", text)
        self.assertNotIn("prompt_toolkit", text)




class RunModeTests(unittest.TestCase):
    def test_run_forces_noninteractive_even_with_tty_stdin(self):
        session = oy_cli.runtime.SessionContext(
            workspace=Path("/workspace"),
            model="openai:gpt-test",
            interactive=False,
            system_prompt="sys",
            system_file=None,
        )

        async def fake_run_agent(*args, **kwargs):
            self.assertFalse(args[5])
            return 0, "ok"

        with (
            patch.object(oy_cli.cli, "_resolve_session", return_value=session) as resolve_mock,
            patch.object(oy_cli.cli, "_print_session_intro"),
            patch.object(oy_cli.cli, "run_agent", side_effect=fake_run_agent),
        ):
            code = oy_cli.run("do", "thing")

        self.assertEqual(code, 0)
        resolve_mock.assert_called_once_with(interactive=False)



class ChatCommandTests(unittest.TestCase):
    def test_chat_command_undo_reports_success(self):
        transcript = oy_cli.Transcript(messages=[SystemMessage("sys"), UserMessage("hello")])

        with (
            patch.object(oy_cli.runtime, "split_model_spec", return_value=("openai", "gpt-test")),
            patch.object(oy_cli.runtime, "_note") as note_mock,
        ):
            result = oy_cli._chat_command("/undo", transcript, "sys", "openai:gpt-test")

        self.assertTrue(result)
        self.assertEqual(transcript.messages, [SystemMessage("sys")])
        note_mock.assert_called_once_with("undid last turn", tag="note")

    def test_chat_command_undo_reports_when_empty(self):
        transcript = oy_cli.Transcript.with_system_prompt("sys")

        with (
            patch.object(oy_cli.runtime, "split_model_spec", return_value=("openai", "gpt-test")),
            patch.object(oy_cli.runtime, "_print") as print_mock,
        ):
            result = oy_cli._chat_command("/undo", transcript, "sys", "openai:gpt-test")

        self.assertTrue(result)
        self.assertEqual(transcript.messages, [SystemMessage("sys")])
        print_mock.assert_called_once_with("warning", "Nothing to undo.", err=True)


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

    def test_history_path_hardens_history_file_permissions(self):
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
                resolved = oy_cli.runtime._history_path()

            self.assertEqual(resolved, history_path)
            self.assertEqual(history_dir.stat().st_mode & 0o777, 0o700)
            self.assertEqual(history_path.stat().st_mode & 0o777, 0o600)

    def test_create_prompt_session_uses_prompt_factory(self):
        sentinel = object()
        with (
            patch.object(oy_cli.runtime, "_history_path", return_value=Path("/tmp/history")),
            patch.object(oy_cli.runtime, "FileHistory", side_effect=lambda p: ("history", p)) as history_mock,
            patch.object(oy_cli.runtime.Prompt, "session", return_value=sentinel) as session_mock,
        ):
            result = oy_cli._create_prompt_session()

        self.assertIs(result, sentinel)
        history_mock.assert_called_once_with("/tmp/history")
        session_mock.assert_called_once_with(
            console=oy_cli.runtime.STDERR,
            history=("history", "/tmp/history"),
            choices=[
                "/help",
                "/tokens",
                "/model",
                "/debug",
                "/ask",
                "/audit",
                "/save",
                "/load",
                "/undo",
                "/clear",
                "/quit",
                "/exit",
            ],
            multiline=False,
            enable_open_in_editor=True,
        )

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

        with patch.object(oy_cli.cli, "_git_diff_shortstat", return_value="1 file changed, 2 insertions(+)"):
            result = oy_cli._read_input(PromptSessionStub(), Path("/workspace"))

        self.assertEqual(result, "hello")
        self.assertEqual(len(prompts), 1)
        self.assertEqual(
            prompts[0].value,
            "\x1b[2m1 file changed, 2 insertions(+)\x1b[0m\n\x1b[1;32moy ❯\x1b[0m ",
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
                patch.object(oy_cli.runtime, "_note"),
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
        from oy_cli import _READ_ONLY_TOOLS

        self.assertIn("webfetch", _READ_ONLY_TOOLS)

    def test_sloc_in_read_only_tools(self):
        from oy_cli import _READ_ONLY_TOOLS

        self.assertIn("sloc", _READ_ONLY_TOOLS)

    def test_noninteractive_tool_registry_excludes_ask(self):
        from oy_cli import active_tool_specs

        names = [spec.name for spec in active_tool_specs(False).specs()]
        self.assertNotIn("ask", names)

    def test_read_only_tool_registry_matches_declared_names(self):
        from oy_cli import _READ_ONLY_TOOLS, read_only_tool_specs

        self.assertEqual({spec.name for spec in read_only_tool_specs().specs()}, _READ_ONLY_TOOLS)

    def test_webfetch_args_schema_has_url_and_options(self):
        from oy_cli import WebfetchArgs
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
        from oy_cli import TodoArgs

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
