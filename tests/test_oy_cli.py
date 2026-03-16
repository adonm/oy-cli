from __future__ import annotations

import unittest
from pathlib import Path
from unittest.mock import patch

import msgspec

import oy_cli
from shim import SystemMessage, ToolSpec, UserMessage


class EchoArgs(msgspec.Struct, omit_defaults=True):
    text: str


def _echo(state, text):
    return f"{state.root.name}:{text}"


class ToolDispatchTests(unittest.TestCase):
    def _state(self, registry):
        return oy_cli.AgentState(
            root=Path("/tmp/ok"),
            max_tool_calls=2,
            tool_specs=registry,
        )

    def test_registry_invokes_normalized_tool_name(self):
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
        result = registry.invoke(self._state(registry), "tool_echo", {"text": "hi"})
        self.assertTrue(result.ok)
        self.assertEqual(result.content, "ok:hi")

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
        result = registry.invoke(self._state(registry), "echo", {})
        self.assertFalse(result.ok)
        self.assertEqual(result.content["tool"], "echo")
        self.assertIn("error_type", result.content)

    def test_chat_tools_returns_registered_specs(self):
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
        self.assertEqual(oy_cli.chat_tools(registry), [registry.get("echo").spec])

    def test_agent_state_enforces_tool_call_limit(self):
        state = oy_cli.AgentState(
            root=Path("/tmp/ok"),
            max_tool_calls=1,
            tool_specs=oy_cli.ToolRegistry(),
        )
        state.note_tool_call()
        with self.assertRaisesRegex(ValueError, "reached max tool calls"):
            state.note_tool_call()

    def test_model_command_shows_current_model_in_non_tty(self):
        with (
            patch.object(oy_cli, "_model", return_value="openai:gpt-4o"),
            patch.object(oy_cli, "resolve_active_shim", return_value="openai"),
            patch.object(oy_cli, "split_model_spec", return_value=("openai", "gpt-4o")),
            patch.object(oy_cli.sys.stdin, "isatty", return_value=False),
            patch.object(oy_cli, "_print") as mock_print,
        ):
            self.assertEqual(oy_cli.model(), 0)
        self.assertIn("Current Model", mock_print.call_args.kwargs["value"])


class TranscriptTests(unittest.TestCase):
    def test_set_system_prompt_replaces_first_system_message(self):
        transcript = oy_cli.Transcript(
            messages=[SystemMessage("old"), UserMessage("hi")]
        )
        transcript.set_system_prompt("new")
        self.assertEqual(transcript.messages[0], SystemMessage("new"))
        self.assertEqual(transcript.messages[1], UserMessage("hi"))

    def test_truncate_message_adds_notice_for_long_content(self):
        transcript = oy_cli.Transcript(max_message_tokens=3)
        with patch.object(
            oy_cli,
            "truncate_str_to_tokens",
            return_value="abc\n... [truncated: 1 line, 3 chars omitted to fit 3-token limit]",
        ):
            truncated = transcript.truncate_message(UserMessage("abcdef"))
        self.assertIsInstance(truncated, UserMessage)
        self.assertIn("truncated:", truncated.content)

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

    def test_prepared_tokens_counts_prepared_transcript(self):
        transcript = oy_cli.Transcript(
            messages=[SystemMessage("sys"), UserMessage("abc"), UserMessage("de")],
            max_context_tokens=20,
            max_message_tokens=10,
        )
        with patch.object(oy_cli, "count_tokens", side_effect=lambda text: len(text)):
            self.assertEqual(transcript.prepared_tokens(), 20)


class PickModelTests(unittest.TestCase):
    def test_pick_model_aborts_in_non_interactive_mode(self):
        with patch.object(oy_cli.sys.stdin, "isatty", return_value=False):
            with self.assertRaises(SystemExit):
                oy_cli._pick_model()

    def test_pick_model_aborts_when_non_interactive_env_set(self):
        with (
            patch.object(oy_cli.sys.stdin, "isatty", return_value=True),
            patch.dict(oy_cli.os.environ, {"OY_NON_INTERACTIVE": "1"}),
        ):
            with self.assertRaises(SystemExit):
                oy_cli._pick_model()


class ResolvePathTests(unittest.TestCase):
    def test_resolve_path_allows_child(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "sub").mkdir()
            result = oy_cli.resolve_path(root, "sub")
            self.assertEqual(result, (root / "sub").resolve())

    def test_resolve_path_rejects_traversal(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            with self.assertRaisesRegex(ValueError, "Path traversal denied"):
                oy_cli.resolve_path(root, "../../etc/passwd")


class ReplaceTests(unittest.TestCase):
    def test_replace_single_match(self):
        text, count = oy_cli._replace("hello world", "world", "earth")
        self.assertEqual(text, "hello earth")
        self.assertEqual(count, 1)

    def test_replace_rejects_empty_old(self):
        with self.assertRaisesRegex(ValueError, "old is empty"):
            oy_cli._replace("hello", "", "x")

    def test_replace_rejects_missing_target(self):
        with self.assertRaisesRegex(ValueError, "not found"):
            oy_cli._replace("hello", "xyz", "x")

    def test_replace_rejects_ambiguous_match(self):
        with self.assertRaisesRegex(ValueError, "multiple matches"):
            oy_cli._replace("aa", "a", "b")

    def test_replace_all(self):
        text, count = oy_cli._replace("aa", "a", "b", replace_all=True)
        self.assertEqual(text, "bb")
        self.assertEqual(count, 2)


class RelTests(unittest.TestCase):
    def test_rel_returns_relative_path_inside_workspace(self):
        root = Path("/workspace")
        p = Path("/workspace/src/main.py")
        self.assertEqual(oy_cli._rel(root, p), "src/main.py")

    def test_rel_returns_placeholder_for_outside_workspace(self):
        root = Path("/workspace")
        p = Path("/etc/passwd")
        self.assertEqual(oy_cli._rel(root, p), "<outside workspace>")


class ClipTokensTests(unittest.TestCase):
    def test_short_text_unchanged(self):
        result = oy_cli.clip_tokens("hello", limit=1000)
        self.assertEqual(result, "hello")

    def test_long_text_truncated(self):
        result = oy_cli.clip_tokens("a " * 10000, limit=10)
        self.assertIn("tokens omitted", result)


if __name__ == "__main__":
    unittest.main()
