from __future__ import annotations

import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import msgspec

import oy_cli
from shim import SystemMessage, ToolMessage, ToolResult, ToolSpec, UserMessage


class EchoArgs(msgspec.Struct, omit_defaults=True):
    text: str


def _echo(state, text):
    return f"{state.root.name}:{text}"


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
        state = oy_cli.AgentState(
            root=Path("/tmp/ok"),
            tool_specs=registry,
            unattended_timeout_seconds=3600,
            unattended_deadline=float("inf"),
        )
        result = registry.invoke(state, "echo", {})
        self.assertFalse(result.ok)
        self.assertEqual(result.content["tool"], "echo")
        self.assertIn("error_type", result.content)

    def test_agent_state_enforces_unattended_timeout(self):
        state = oy_cli.AgentState(
            root=Path("/tmp/ok"),
            tool_specs=oy_cli.ToolRegistry(),
            unattended_timeout_seconds=3600,
            unattended_deadline=10.0,
        )
        with patch.object(oy_cli.time, "monotonic", return_value=10.0):
            with self.assertRaisesRegex(TimeoutError, r"reached unattended timeout \(1h\)"):
                state.note_progress()


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


class EditToolTests(unittest.TestCase):
    def test_edit_replace_supports_regex(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            path = root / "demo.txt"
            path.write_text("alpha-1\nalpha-2\n", encoding="utf-8")
            state = oy_cli.AgentState(
                root=root,
                tool_specs=oy_cli.TOOL_REGISTRY,
                unattended_timeout_seconds=3600,
                unattended_deadline=float("inf"),
            )
            with patch.object(oy_cli, "_print"):
                result = oy_cli.tool_edit(
                    state,
                    {
                        "op": "replace",
                        "path": "demo.txt",
                        "old": r"alpha-\d",
                        "new": "beta",
                        "regex": True,
                        "replace_all": True,
                    },
                )
            updated = path.read_text(encoding="utf-8")
        self.assertIn("replaced demo.txt (2 matches)", result)
        self.assertEqual(updated, "beta\nbeta\n")

    def test_edit_write_can_target_line_range(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            path = root / "demo.txt"
            path.write_text("a\nb\nc\n", encoding="utf-8")
            state = oy_cli.AgentState(
                root=root,
                tool_specs=oy_cli.TOOL_REGISTRY,
                unattended_timeout_seconds=3600,
                unattended_deadline=float("inf"),
            )
            with patch.object(oy_cli, "_print"):
                result = oy_cli.tool_edit(
                    state,
                    {
                        "op": "write",
                        "path": "demo.txt",
                        "content": "x\ny\n",
                        "start_line": 2,
                        "end_line": 3,
                    },
                )
            updated = path.read_text(encoding="utf-8")
        self.assertEqual(result, "wrote demo.txt")
        self.assertEqual(updated, "a\nx\ny\n")


class ListToolTests(unittest.TestCase):
    def test_list_path_accepts_pathlib_glob(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "src").mkdir()
            (root / "src" / "main.py").write_text("print('hi')\n", encoding="utf-8")
            (root / "src" / "util.txt").write_text("helper\n", encoding="utf-8")
            state = oy_cli.AgentState(
                root=root,
                tool_specs=oy_cli.TOOL_REGISTRY,
                unattended_timeout_seconds=3600,
                unattended_deadline=float("inf"),
            )
            with patch.object(oy_cli, "_print"):
                result = oy_cli.tool_list(state, path="src/*.py")
        self.assertEqual(result, "src/main.py")


class SearchToolTests(unittest.TestCase):
    def _state(self, root: Path):
        return oy_cli.AgentState(
            root=root,
            tool_specs=oy_cli.TOOL_REGISTRY,
            unattended_timeout_seconds=3600,
            unattended_deadline=float("inf"),
        )

    def test_search_path_accepts_pathlib_glob(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "src").mkdir()
            (root / "src" / "main.py").write_text("needle\n", encoding="utf-8")
            (root / "src" / "guide.txt").write_text("needle\n", encoding="utf-8")
            with patch.object(oy_cli, "_print"):
                result = oy_cli.tool_search(self._state(root), "needle", path="src/*.py")
        self.assertIn("src/main.py:1:1:needle", result)
        self.assertNotIn("src/guide.txt", result)

    def test_search_can_target_line_range_in_single_file(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "demo.txt").write_text("skip\nneedle\nskip\nneedle\n", encoding="utf-8")
            with patch.object(oy_cli, "_print"):
                result = oy_cli.tool_search(
                    self._state(root),
                    "needle",
                    path="demo.txt",
                    start_line=4,
                    end_line=4,
                )
        self.assertEqual(result, "demo.txt:4:1:needle")


class JsonPathTests(unittest.TestCase):
    def test_depth_cap_raises(self):
        obj = 0
        for _ in range(25):
            obj = {"a": obj}
        deep = ".".join(["a"] * 21)
        with self.assertRaisesRegex(ValueError, "json_path exceeded max depth"):
            oy_cli._json_path(obj, deep)


class HeadroomSerializationTests(unittest.TestCase):
    def test_serialize_for_headroom_stringifies_tool_content(self):
        message = ToolMessage(
            tool_call_id="call_1",
            name="httpx",
            content=ToolResult(ok=False, content={"count": 2, "ok": True}),
        )
        payload = oy_cli._serialize_for_headroom(message)
        self.assertEqual(payload["role"], "tool")
        self.assertFalse(payload["ok"])
        self.assertIsInstance(payload["content"], str)
        self.assertIn('"count": 2', payload["content"])

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
