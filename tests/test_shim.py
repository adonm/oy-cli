from __future__ import annotations

import base64
import json
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import AsyncMock, Mock, patch

import httpx
import oy_cli.aws_sigv4 as aws_sigv4
from oy_cli import providers


async def _unused_chat_completion(
    model, messages, tools=None, tool_choice="auto", on_retry=None
):
    raise AssertionError("chat_completion should not be called in this test")


def _dummy_client(models: list[str] | None = None) -> providers.CompletionClient:
    return providers.CompletionClient(
        chat_completion=_unused_chat_completion,
        list_models=lambda: list(models or []),
    )


def _dummy_spec(
    name: str,
    *,
    ensure_env=None,
    list_models=None,
) -> providers.ShimSpec:
    return providers.ShimSpec(
        name=name,
        ensure_env=ensure_env or (lambda cwd: None),
        build_client=lambda cwd: _dummy_client(),
        list_models=list_models or (lambda cwd: []),
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


class BedrockCredentialTests(unittest.TestCase):
    def test_internal_bedrock_signer_matches_known_sigv4_token(self):
        token = aws_sigv4.bedrock_bearer_token(
            aws_sigv4.AwsCredentials(
                access_key="AKIDEXAMPLE",
                secret_key="wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
                session_token="session-token",
            ),
            "us-east-1",
            expires=600,
            now=datetime(2026, 3, 25, 12, 34, 56, tzinfo=timezone.utc),
        )

        raw = base64.b64decode(token.removeprefix("bedrock-api-key-")).decode()
        self.assertEqual(
            raw,
            "bedrock.amazonaws.com/?Action=CallWithBearerToken&Version=1&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIDEXAMPLE%2F20260325%2Fus-east-1%2Fbedrock%2Faws4_request&X-Amz-Date=20260325T123456Z&X-Amz-Expires=600&X-Amz-SignedHeaders=host&X-Amz-Security-Token=session-token&X-Amz-Signature=702fccf760edb52d49251f73bafd6b263f155102fc6def0bc63d1615b45f5900",
        )

    def test_make_bedrock_token_loads_aws_credentials(self):
        creds = {
            "access_key": "AKIDEXAMPLE",
            "secret_key": "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "session_token": "session-token",
        }
        fixed_now = datetime(2026, 3, 25, 12, 34, 56, tzinfo=timezone.utc)

        with (
            patch.object(providers, "load_aws_credentials", return_value=creds) as load_creds,
            patch("oy_cli.aws_sigv4._utc_now", return_value=fixed_now),
        ):
            token = providers.make_bedrock_token("us-east-1", expires=600)

        load_creds.assert_called_once_with(None)
        self.assertTrue(token.startswith("bedrock-api-key-"))


class TranslationTests(unittest.TestCase):
    def test_tool_output_text_uses_toon_for_structured_values(self):
        rendered = providers._tool_output_text(
            providers.ToolResult(content={"count": 2, "items": [1, 2]})
        )

        self.assertIsInstance(rendered, str)
        self.assertIn("count", rendered)
        self.assertIn("items", rendered)
        self.assertNotIn('{"count":2', rendered)

    def test_openai_chat_message_uses_toon_for_tool_output(self):
        message = providers._openai_chat_message(
            providers.ToolMessage(
                tool_call_id="call_1",
                name="echo",
                content=providers.ToolResult(content={"count": 2}),
            )
        )

        self.assertEqual(message["role"], "tool")
        self.assertIsInstance(message["content"], str)
        self.assertIn("count", message["content"])
        self.assertNotIn('{"count":2', message["content"])

    def test_responses_input_uses_toon_for_tool_outputs(self):
        items = providers._responses_input_from_messages(
            [
                providers.ToolMessage(
                    tool_call_id="call_1",
                    name="echo",
                    content=providers.ToolResult(content={"count": 2}),
                )
            ]
        )

        self.assertEqual(items[0]["type"], "function_call_output")
        self.assertIn("count", items[0]["output"])
        self.assertNotIn('{"count":2', items[0]["output"])

    def test_openai_tool_call_keeps_json_arguments(self):
        tool_call = providers._openai_tool_call(
            providers.ToolCall(id="call_1", name="echo", arguments={"count": 2})
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
            [providers.ToolCall(id="call_1", name="echo", arguments={"value": "x"})],
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
            [providers.ToolSpec("echo", "echo text", {"type": "object"})],
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
            [providers.ToolCall(id="call_1", name="list", arguments={"path": "*"})],
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
            "APIStatusError",
            "AssistantMessage",
            "ChatMessage",
            "CompletionClient",
            "JSONLike",
            "SystemMessage",
            "ToolCall",
            "ToolMessage",
            "ToolResult",
            "ToolSpec",
            "UserMessage",
            "command_env",
            "detect_available_shims",
            "ensure_api_env",
            "get_client",
            "join_model_spec",
            "list_models_for_shim",
            "load_json",
            "normalize_jsonlike",
            "require_api_env",
            "resolve_shim",
            "run_cmd",
            "save_json",
            "serialize_json",
            "serialize_toon",
            "split_model_spec",
            "validate_shim",
            "which",
        }

        self.assertEqual(set(providers.__all__), expected | {"ShimSpec"})

    def test_ensure_api_env_uses_resolved_shim_spec(self):
        spec = _dummy_spec("alpha", ensure_env=lambda cwd: None)
        with (
            patch.object(providers, "resolve_shim", return_value="alpha") as resolve_shim,
            patch.object(providers, "_shim_spec", return_value=spec) as shim_spec,
        ):
            ok, error = providers.ensure_api_env("alpha:model", None, Path("/workspace"))

        resolve_shim.assert_called_once_with("alpha:model", None)
        shim_spec.assert_called_once_with("alpha")
        self.assertEqual((ok, error), (True, None))

    def test_get_client_uses_shim_registry_builder(self):
        sentinel = _dummy_client(["demo"])
        spec = providers.ShimSpec(
            name="alpha",
            ensure_env=lambda cwd: None,
            build_client=lambda cwd: sentinel,
            list_models=lambda cwd: ["demo"],
        )
        with patch.object(providers, "_shim_spec", return_value=spec) as shim_spec:
            client = providers.get_client("alpha")

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
            patch.object(providers, "SHIM_ORDER", ("alpha", "beta", "gamma")),
            patch.object(providers, "KNOWN_SHIMS", set(specs)),
            patch.dict(providers.SHIM_SPECS, specs, clear=True),
        ):
            self.assertEqual(providers.detect_available_shims(), ["alpha", "gamma"])
        self.assertEqual(calls, ["alpha", "beta", "gamma"])


class RunCmdTests(unittest.TestCase):
    def test_run_cmd_raises_clean_error_when_binary_missing(self):
        with self.assertRaisesRegex(
            FileNotFoundError, r"\[Errno 2\] No such file or directory: 'missing-tool'"
        ):
            providers.run_cmd(["missing-tool"], env={"PATH": ""})

    def test_run_cmd_allows_explicit_relative_paths(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            script = root / "hello.sh"
            script.write_text("#!/bin/sh\nprintf hi\n", encoding="utf-8")
            script.chmod(0o755)

            result = providers.run_cmd(["./hello.sh"], cwd=root, env={"PATH": ""})

        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "hi")


class CommandEnvTests(unittest.TestCase):
    def tearDown(self):
        providers.command_env.cache_clear()

    def test_command_env_returns_launch_environment(self):
        with patch.dict(
            providers.os.environ, {"PATH": "/test/bin", "HOME": "/tmp/home"}, clear=True
        ):
            env = providers.command_env()

        self.assertEqual(env["PATH"], "/test/bin")
        self.assertEqual(env["HOME"], "/tmp/home")


if __name__ == "__main__":
    unittest.main()
