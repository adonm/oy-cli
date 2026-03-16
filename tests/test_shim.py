from __future__ import annotations

import json
import unittest
from pathlib import Path
from unittest.mock import patch

import shim


async def _unused_chat_completion(
    model, messages, tools=None, tool_choice="auto", on_retry=None
):
    raise AssertionError("chat_completion should not be called in this test")


def _dummy_client(models: list[str] | None = None) -> shim.CompletionClient:
    return shim.CompletionClient(
        chat_completion=_unused_chat_completion,
        list_models=lambda: list(models or []),
    )


def _dummy_spec(
    name: str,
    *,
    ensure_env=None,
    list_models=None,
) -> shim.ShimSpec:
    return shim.ShimSpec(
        name=name,
        ensure_env=ensure_env or (lambda cwd: None),
        build_client=lambda region, cwd: _dummy_client(),
        list_models=list_models or (lambda region, cwd: []),
    )


class DecodeToolCallArgumentsTests(unittest.TestCase):
    def test_decodes_double_encoded_json(self):
        raw = json.dumps('{"count": 2}')
        self.assertEqual(shim._decode_tool_call_arguments(raw), {"count": 2})

    def test_salvages_duplicated_json(self):
        self.assertEqual(
            shim._decode_tool_call_arguments('{"ok":true}{"ok":true}'),
            {"ok": True},
        )


class TranslationTests(unittest.TestCase):
    def test_decodes_responses_output(self):
        message = shim._decode_responses_output(
            {
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"text": "hello"}, {"refusal": "nope"}],
                    },
                    {
                        "type": "function_call",
                        "call_id": "call_1",
                        "name": "echo",
                        "arguments": '{"value":"x"}',
                    },
                ]
            }
        )
        self.assertEqual(message.content, "hello\n\nnope")
        self.assertEqual(
            message.tool_calls,
            [shim.ToolCall(id="call_1", name="echo", arguments={"value": "x"})],
        )

    def test_bedrock_encoding_merges_adjacent_tool_results(self):
        messages: list[shim.ChatMessage] = [
            shim.AssistantMessage(
                tool_calls=[shim.ToolCall(id="call_1", name="echo", arguments={"x": 1})]
            ),
            shim.ToolMessage(
                tool_call_id="call_1",
                name="echo",
                content=shim.ToolResult(content="first"),
            ),
            shim.ToolMessage(
                tool_call_id="call_2",
                name="echo",
                content=shim.ToolResult(ok=False, content={"error": "second"}),
            ),
        ]
        encoded, system = shim._encode_provider_messages(messages, shim.BEDROCK_CODEC)
        self.assertIsNone(system)
        self.assertEqual([item["role"] for item in encoded], ["assistant", "user"])
        self.assertEqual(len(encoded[1]["content"]), 2)
        self.assertEqual(encoded[1]["content"][1]["toolResult"]["status"], "error")


class ShimRegistryTests(unittest.TestCase):
    def test_detect_available_shims_follows_registry_order(self):
        calls: list[str] = []

        def ok(name: str):
            def ensure_env(cwd):
                _ = cwd
                calls.append(name)

            return ensure_env

        def fail(name: str):
            def ensure_env(cwd):
                _ = cwd
                calls.append(name)
                raise RuntimeError(name)

            return ensure_env

        specs = {
            "alpha": _dummy_spec("alpha", ensure_env=ok("alpha")),
            "beta": _dummy_spec("beta", ensure_env=fail("beta")),
            "gamma": _dummy_spec("gamma", ensure_env=ok("gamma")),
        }
        with (
            patch.object(shim, "SHIM_ORDER", ("alpha", "beta", "gamma")),
            patch.object(shim, "KNOWN_SHIMS", set(specs)),
            patch.dict(shim.SHIM_SPECS, specs, clear=True),
        ):
            self.assertEqual(shim.detect_available_shims(), ["alpha", "gamma"])
        self.assertEqual(calls, ["alpha", "beta", "gamma"])

    def test_list_models_for_shim_uses_registry_lister(self):
        specs = {
            "alpha": _dummy_spec(
                "alpha", list_models=lambda region, cwd: ["one", "two"]
            )
        }
        with (
            patch.object(shim, "SHIM_ORDER", ("alpha",)),
            patch.object(shim, "KNOWN_SHIMS", {"alpha"}),
            patch.dict(shim.SHIM_SPECS, specs, clear=True),
        ):
            self.assertEqual(
                shim.list_models_for_shim("alpha"),
                ["alpha:one", "alpha:two"],
            )


class LoadJsonTests(unittest.TestCase):
    def test_load_json_returns_default_on_missing_file(self):
        result = shim.load_json(Path("/nonexistent/path.json"), {"default": True})
        self.assertEqual(result, {"default": True})

    def test_load_json_returns_default_on_invalid_json(self):
        import tempfile

        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            f.write("not json")
            f.flush()
            result = shim.load_json(Path(f.name), "fallback")
            self.assertEqual(result, "fallback")

    def test_load_json_reads_valid_file(self):
        import tempfile

        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            f.write('{"key": "value"}')
            f.flush()
            result = shim.load_json(Path(f.name), {})
            self.assertEqual(result, {"key": "value"})


class ExpiryMsTests(unittest.TestCase):
    def test_expiry_ms_with_valid_seconds(self):
        result = shim.expiry_ms(3600)
        import time

        expected_approx = int((time.time() + 3600.0 - 60) * 1000)
        self.assertAlmostEqual(result, expected_approx, delta=2000)

    def test_expiry_ms_with_invalid_value_uses_default(self):
        result = shim.expiry_ms("not-a-number")
        import time

        expected_approx = int((time.time() + 3600.0 - 60) * 1000)
        self.assertAlmostEqual(result, expected_approx, delta=2000)


class SplitModelSpecTests(unittest.TestCase):
    def test_bare_model_returns_none_prefix(self):
        shim_name, model = shim.split_model_spec("gpt-4o")
        self.assertIsNone(shim_name)
        self.assertEqual(model, "gpt-4o")

    def test_prefixed_model_splits_correctly(self):
        shim_name, model = shim.split_model_spec("bedrock:us.anthropic.claude-3")
        self.assertEqual(shim_name, "bedrock")
        self.assertEqual(model, "us.anthropic.claude-3")

    def test_unknown_prefix_treated_as_bare(self):
        shim_name, model = shim.split_model_spec("unknown:model")
        self.assertIsNone(shim_name)
        self.assertEqual(model, "unknown:model")


if __name__ == "__main__":
    unittest.main()
