from __future__ import annotations

import json
import unittest
from pathlib import Path
from unittest.mock import AsyncMock, Mock, patch

import httpx
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
        self.assertEqual(encoded[1]["content"][1]["toolResult"]["status"], "error")


class ReasoningTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        shim._REASONING_SUPPORT_CACHE.clear()

    async def test_chat_completions_client_does_not_send_parallel_hint(self):
        message = Mock(content="done", tool_calls=None)
        choice = Mock(message=message)
        final_response = Mock(choices=[choice])
        create = AsyncMock(return_value=final_response)
        chat = Mock(completions=Mock(create=create))
        async_client = Mock()
        async_client.with_options.return_value = Mock(chat=chat)
        sync_client = Mock()
        client = shim._openai_chat_completions_client(
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
        unsupported = shim.APIStatusError(
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

        client = shim._openai_responses_client(async_client, sync_client)
        await client.chat_completion("gpt-test", [])

        self.assertEqual(create.call_count, 2)
        self.assertEqual(
            create.call_args_list[0].kwargs["reasoning"], {"effort": "high"}
        )
        self.assertNotIn("reasoning", create.call_args_list[1].kwargs)

    async def test_chat_completions_client_falls_back_without_reasoning_when_unsupported(
        self,
    ):
        unsupported = shim.APIStatusError(
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

        client = shim._openai_chat_completions_client(
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
        shim._mark_reasoning_unsupported("responses", "gpt-test")

        client = shim._openai_responses_client(async_client, sync_client)
        await client.chat_completion("gpt-test", [])

        self.assertNotIn("reasoning", create.call_args.kwargs)


class ShimHelperTests(unittest.TestCase):
    def test_static_env_checker_ignores_cwd(self):
        calls: list[str] = []

        checker = shim._static_env_checker(lambda: calls.append("ok"))
        checker(None)
        checker(Path("/tmp/work"))

        self.assertEqual(calls, ["ok", "ok"])

    def test_region_client_builder_ignores_cwd(self):
        calls: list[str | None] = []

        builder = shim._region_client_builder(
            lambda region: calls.append(region) or _dummy_client([region or "default"])
        )
        client = builder("us-east-1", Path("/tmp/work"))

        self.assertEqual(calls, ["us-east-1"])
        self.assertEqual(client.list_models(), ["us-east-1"])


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
            "list_all_model_ids",
            "list_models_for_shim",
            "load_json",
            "require_api_env",
            "run_cmd",
            "save_json",
            "split_model_spec",
            "validate_shim",
            "which",
        }

        self.assertEqual(set(shim.__all__), expected)

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
            client = shim.get_client("alpha", model_spec="alpha:demo", region="us-east-1")

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
            FileNotFoundError, r"Command `missing-tool` was not found on PATH"
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

    def test_command_env_requires_mise_on_path(self):
        with patch.object(shim, "which", return_value=None):
            with self.assertRaisesRegex(
                RuntimeError,
                r"`mise` is required; install and activate `mise` before running `oy`",
            ):
                shim.command_env()

    def test_command_env_returns_launch_environment_when_mise_is_available(self):
        with (
            patch.object(shim, "which", return_value="/usr/local/bin/mise"),
            patch.object(shim, "run_cmd") as run_cmd,
            patch.dict(
                shim.os.environ, {"PATH": "/test/bin", "HOME": "/tmp/home"}, clear=True
            ),
        ):
            env = shim.command_env()

        run_cmd.assert_not_called()
        self.assertEqual(env["PATH"], "/test/bin")
        self.assertEqual(env["HOME"], "/tmp/home")


if __name__ == "__main__":
    unittest.main()
