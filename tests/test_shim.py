from __future__ import annotations

import json
import unittest
from pathlib import Path
from unittest.mock import AsyncMock, Mock, patch

import httpx
from oy_cli import providers
from oy_cli import shim


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
        self.assertEqual(providers._decode_tool_call_arguments(raw), {"count": 2})

    def test_salvages_duplicated_json(self):
        self.assertEqual(
            providers._decode_tool_call_arguments('{"ok":true}{"ok":true}'),
            {"ok": True},
        )


class TranslationTests(unittest.TestCase):
    def test_tool_output_text_uses_toon_for_structured_values(self):
        rendered = providers._tool_output_text(
            shim.ToolResult(content={"count": 2, "items": [1, 2]})
        )

        self.assertIsInstance(rendered, str)
        self.assertIn("count", rendered)
        self.assertIn("items", rendered)
        self.assertNotIn('{"count":2', rendered)

    def test_openai_chat_message_uses_toon_for_tool_output(self):
        message = providers._openai_chat_message(
            shim.ToolMessage(
                tool_call_id="call_1",
                name="echo",
                content=shim.ToolResult(content={"count": 2}),
            )
        )

        self.assertEqual(message["role"], "tool")
        self.assertIsInstance(message["content"], str)
        self.assertIn("count", message["content"])
        self.assertNotIn('{"count":2', message["content"])

    def test_responses_input_uses_toon_for_tool_outputs(self):
        items = providers._responses_input_from_messages(
            [
                shim.ToolMessage(
                    tool_call_id="call_1",
                    name="echo",
                    content=shim.ToolResult(content={"count": 2}),
                )
            ]
        )

        self.assertEqual(items[0]["type"], "function_call_output")
        self.assertIn("count", items[0]["output"])
        self.assertNotIn('{"count":2', items[0]["output"])

    def test_openai_tool_call_keeps_json_arguments(self):
        tool_call = providers._openai_tool_call(
            shim.ToolCall(id="call_1", name="echo", arguments={"count": 2})
        )

        self.assertEqual(tool_call["function"]["arguments"], '{"count":2}')

    def test_decodes_responses_output(self):
        message = providers._decode_responses_output(
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

    def test_decodes_responses_output_ignores_blank_text_parts(self):
        message = providers._decode_responses_output(
            {
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            {"text": "\n\n"},
                            {"text": "hello"},
                            {"refusal": "   "},
                            {"refusal": "nope"},
                        ],
                    }
                ]
            }
        )
        self.assertEqual(message.content, "hello\n\nnope")

    def test_assistant_from_blocks_ignores_blank_text_blocks(self):
        message = providers._assistant_from_blocks(
            [
                providers.TextBlock("\n\n"),
                providers.TextBlock("hello"),
                providers.ToolUseBlock(id="call_1", name="echo", arguments={"x": 1}),
            ]
        )
        self.assertEqual(message.content, "hello")
        self.assertEqual(
            message.tool_calls,
            [shim.ToolCall(id="call_1", name="echo", arguments={"x": 1})],
        )

    def test_extract_blocks_ignores_blank_text(self):
        blocks = providers._extract_blocks(
            [{"text": "\n\n"}, {"text": "hello"}],
            text_of=lambda item: item.get("text"),
            tool_of=lambda item, index: None,
        )
        self.assertEqual(blocks, [providers.TextBlock("hello")])

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
        encoded, system = providers._encode_provider_messages(messages, providers.BEDROCK_CODEC)
        self.assertIsNone(system)
        self.assertEqual([item["role"] for item in encoded], ["assistant", "user"])
        self.assertEqual(encoded[1]["content"][1]["toolResult"]["status"], "error")
        tool_text = encoded[1]["content"][0]["toolResult"]["content"][0]["text"]
        self.assertIn("first", tool_text)
        self.assertNotIn('{"error":"second"', encoded[1]["content"][1]["toolResult"]["content"][0]["text"])
        self.assertIn("error", encoded[1]["content"][1]["toolResult"]["content"][0]["text"])


class ReasoningTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        providers._REASONING_SUPPORT_CACHE.clear()

    async def test_chat_completions_client_does_not_send_parallel_hint(self):
        message = Mock(content="done", tool_calls=None)
        choice = Mock(message=message)
        final_response = Mock(choices=[choice])
        create = AsyncMock(return_value=final_response)
        chat = Mock(completions=Mock(create=create))
        async_client = Mock()
        async_client.with_options.return_value = Mock(chat=chat)
        sync_client = Mock()
        client = providers._openai_chat_completions_client(
            async_client,
            sync_client,
            tools_map=lambda tools: [
                {
                    "type": "function",
                    "function": {
                        "name": tools[0].name,
                        "description": tools[0].description,
                        "parameters": tools[0].parameters,
                    },
                }
            ],
        )

        await client.chat_completion(
            "gpt-test",
            [],
            [shim.ToolSpec("echo", "echo text", {"type": "object"})],
        )

        self.assertNotIn("parallel_tool_calls", create.call_args.kwargs)

    async def test_responses_client_falls_back_without_reasoning_when_unsupported(self):
        unsupported = providers.APIStatusError(
            "bad request",
            response=httpx.Response(
                400,
                json={"error": {"message": "Unsupported parameter: reasoning"}},
                request=httpx.Request("POST", "https://example.com"),
            ),
            body=None,
        )
        final_response = {"output": []}

        create = AsyncMock(side_effect=[unsupported, final_response])
        responses = Mock(create=create)
        async_client = Mock()
        async_client.with_options.return_value = Mock(responses=responses)
        sync_client = Mock()

        client = providers._openai_responses_client(async_client, sync_client)
        await client.chat_completion("gpt-test", [])

        self.assertEqual(create.call_count, 2)
        self.assertEqual(
            create.call_args_list[0].kwargs["reasoning"], {"effort": "high"}
        )
        self.assertNotIn("reasoning", create.call_args_list[1].kwargs)

    def test_chat_completion_merge_stops_at_duplicate_key_conflict(self):
        response = Mock(
            choices=[
                Mock(message=Mock(content="\n\n", tool_calls=None, role="assistant")),
                Mock(
                    message=Mock(
                        content="Let me inspect the repo.",
                        tool_calls=None,
                        role="assistant",
                    )
                ),
                Mock(
                    message=Mock(
                        content=None,
                        role="assistant",
                        tool_calls=[
                            {
                                "id": "call_1",
                                "function": {
                                    "name": "list",
                                    "arguments": '{"path":"*"}',
                                },
                            }
                        ],
                    )
                ),
            ]
        )

        message = providers._chat_completion_to_assistant_message(response)

        self.assertEqual(message.content, "Let me inspect the repo.")
        self.assertEqual(
            message.tool_calls,
            [shim.ToolCall(id="call_1", name="list", arguments={"path": "*"})],
        )

    def test_chat_completion_merge_returns_prefix_before_conflicting_duplicate_key(self):
        response = Mock(
            choices=[
                Mock(message=Mock(content="first", tool_calls=None, role="assistant")),
                Mock(message=Mock(content="second", tool_calls=None, role="assistant")),
            ]
        )

        message = providers._chat_completion_to_assistant_message(response)

        self.assertEqual(message.content, "first")
        self.assertEqual(message.tool_calls, [])

    async def test_chat_completions_client_falls_back_without_reasoning_when_unsupported(
        self,
    ):
        unsupported = providers.APIStatusError(
            "bad request",
            response=httpx.Response(
                400,
                json={"error": {"message": "Unknown parameter: reasoning_effort"}},
                request=httpx.Request("POST", "https://example.com"),
            ),
            body=None,
        )
        message = Mock(content="done", tool_calls=None)
        choice = Mock(message=message)
        final_response = Mock(choices=[choice])

        create = AsyncMock(side_effect=[unsupported, final_response])
        chat = Mock(completions=Mock(create=create))
        async_client = Mock()
        async_client.with_options.return_value = Mock(chat=chat)
        sync_client = Mock()

        client = providers._openai_chat_completions_client(
            async_client,
            sync_client,
            tools_map=lambda tools: [],
        )
        result = await client.chat_completion("gpt-test", [])

        self.assertEqual(result.content, "done")
        self.assertEqual(create.call_count, 2)
        self.assertEqual(create.call_args_list[0].kwargs["reasoning_effort"], "high")
        self.assertNotIn("reasoning_effort", create.call_args_list[1].kwargs)

    async def test_responses_client_skips_reasoning_after_cached_rejection(self):
        final_response = {"output": []}
        create = AsyncMock(return_value=final_response)
        responses = Mock(create=create)
        async_client = Mock()
        async_client.with_options.return_value = Mock(responses=responses)
        sync_client = Mock()
        providers._mark_reasoning_unsupported("responses", "gpt-test")

        client = providers._openai_responses_client(async_client, sync_client)
        await client.chat_completion("gpt-test", [])

        self.assertNotIn("reasoning", create.call_args.kwargs)


class ShimApiSurfaceTests(unittest.TestCase):
    def test_module_all_exports_public_boundary(self):
        expected = {
            "AssistantMessage",
            "ChatMessage",
            "CompletionClient",
            "SystemMessage",
            "ToolCall",
            "ToolMessage",
            "ToolResult",
            "ToolSpec",
            "UserMessage",
            "command_env",
            "default_region",
            "detect_available_shims",
            "ensure_api_env",
            "get_client",
            "join_model_spec",
            "list_models_for_shim",
            "load_json",
            "require_api_env",
            "run_cmd",
            "save_json",
            "split_model_spec",
            "validate_shim",
            "which",
        }

        self.assertEqual(set(shim.__all__), expected | {"ShimSpec", "resolve_shim"})

    def test_ensure_api_env_uses_resolved_shim_spec(self):
        spec = _dummy_spec("alpha", ensure_env=lambda cwd: None)
        with (
            patch.object(shim, "resolve_shim", return_value="alpha") as resolve_shim,
            patch.object(shim, "_shim_spec", return_value=spec) as shim_spec,
        ):
            ok, error = shim.ensure_api_env("alpha:model", None, Path("/workspace"))

        resolve_shim.assert_called_once_with("alpha:model", None)
        shim_spec.assert_called_once_with("alpha")
        self.assertEqual((ok, error), (True, None))

    def test_get_client_uses_shim_registry_builder(self):
        sentinel = _dummy_client(["demo"])
        spec = shim.ShimSpec(
            name="alpha",
            ensure_env=lambda cwd: None,
            build_client=lambda region, cwd: sentinel,
            list_models=lambda region, cwd: ["demo"],
        )
        with patch.object(shim, "_shim_spec", return_value=spec) as shim_spec:
            client = shim.get_client("alpha", region="us-east-1")

        shim_spec.assert_called_once_with("alpha")
        self.assertIs(client, sentinel)


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


class RunCmdTests(unittest.TestCase):
    def test_run_cmd_raises_clean_error_when_binary_missing(self):
        with self.assertRaisesRegex(
            FileNotFoundError, r"\[Errno 2\] No such file or directory: 'missing-tool'"
        ):
            shim.run_cmd(["missing-tool"], env={"PATH": ""})

    def test_run_cmd_allows_explicit_relative_paths(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            script = root / "hello.sh"
            script.write_text("#!/bin/sh\nprintf hi\n", encoding="utf-8")
            script.chmod(0o755)

            result = shim.run_cmd(["./hello.sh"], cwd=root, env={"PATH": ""})

        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "hi")


class CommandEnvTests(unittest.TestCase):
    def tearDown(self):
        shim.command_env.cache_clear()

    def test_command_env_returns_launch_environment(self):
        with patch.dict(
            providers.os.environ, {"PATH": "/test/bin", "HOME": "/tmp/home"}, clear=True
        ):
            env = shim.command_env()

        self.assertEqual(env["PATH"], "/test/bin")
        self.assertEqual(env["HOME"], "/tmp/home")


if __name__ == "__main__":
    unittest.main()
