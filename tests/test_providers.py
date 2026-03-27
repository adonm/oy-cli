"""Tests for providers module: HTTP, auth, shims, message handling."""
from __future__ import annotations

import json
from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from oy_cli import providers
from oy_cli.providers import AssistantMessage, ToolCall, ToolMessage, ToolResult
from tests.conftest import api_error, DummyHttpClient, raw_response


class TestHTTPClient:
    def test_adapt_response_accepts_response_objects(self):
        response = raw_response()
        adapted = providers.adapt_response(response)
        assert adapted["status_code"] == 200
        assert adapted["headers"]["content-type"] == "text/plain"
        assert adapted["http_version"] == "HTTP/2"

    def test_default_http_sessions_use_expected_defaults(self):
        llm = providers.llm_session()
        tool = providers.tool_session()
        try:
            assert llm.timeout == providers.DEFAULT_HTTP_TIMEOUT
            assert llm.follow_redirects is False
            assert tool.timeout == providers.DEFAULT_WEBFETCH_TIMEOUT_SECONDS
            assert tool.follow_redirects is False
        finally:
            llm.close()
            tool.close()


class TestSigV4Signing:
    def test_bedrock_mantle_request_headers(self):
        headers = providers._bedrock_request_headers(
            {
                "access_key": "AKIDEXAMPLE",
                "secret_key": "wJalrXUtnFEMI/I/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "session_token": "TOKEN",
            },
            "ap-southeast-2",
            "POST",
            "https://bedrock-mantle.ap-southeast-2.api.aws/v1/chat/completions",
            body=b'{"model":"zai.glm-4.6"}',
            headers={"Content-Type": "application/json"},
        )
        assert headers["Content-Type"] == "application/json"
        assert headers["Host"] == "bedrock-mantle.ap-southeast-2.api.aws"
        assert headers["X-Amz-Security-Token"] == "TOKEN"
        assert "Credential=AKIDEXAMPLE/" in headers["Authorization"]
        assert "/ap-southeast-2/bedrock-mantle/aws4_request" in headers["Authorization"]


class TestMessageDecoding:
    def test_tool_call_arguments_json_variants(self):
        assert providers._decode_tool_call_arguments(json.dumps('{"count":2}')) == {"count": 2}
        assert providers._decode_tool_call_arguments('{"ok":true}{"ok":true}') == {"ok": True}

    def test_responses_output_decoding(self):
        decoded = providers._decode_responses_output({
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"text": "hello"}, {"refusal": "nope"}, {"text": "   "}],
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "echo",
                    "arguments": '{"value":"x"}',
                },
            ]
        })
        assert decoded == AssistantMessage(
            "hello\n\nnope",
            tool_calls=[ToolCall(id="call_1", name="echo", arguments={"value": "x"})],
        )

    def test_chat_completion_to_assistant_message(self):
        chat_message = providers._chat_completion_to_assistant_message(
            SimpleNamespace(
                choices=[
                    SimpleNamespace(message=SimpleNamespace(content="hello", tool_calls=None)),
                    SimpleNamespace(
                        message=SimpleNamespace(
                            content="hello",
                            tool_calls=[
                                SimpleNamespace(
                                    id="call_2",
                                    function=SimpleNamespace(name="echo", arguments='{"count":2}'),
                                )
                            ],
                        )
                    ),
                ]
            )
        )
        assert chat_message == AssistantMessage(
            "hello",
            tool_calls=[ToolCall(id="call_2", name="echo", arguments={"count": 2})],
        )

    def test_reasoning_only_message(self):
        reasoning_only = providers._chat_completion_to_assistant_message(
            SimpleNamespace(
                choices=[
                    SimpleNamespace(
                        message=SimpleNamespace(content="", reasoning="thoughts", tool_calls=None)
                    )
                ]
            )
        )
        assert reasoning_only == AssistantMessage("thoughts")


class TestReasoningFallback:
    def test_drops_unsupported_reasoning_after_first_rejection(self):
        responses_create = Mock(
            side_effect=[api_error("Unsupported parameter: reasoning"), {"output": []}]
        )
        responses_client = providers._responses_client(responses_create, lambda: ["gpt-test"])
        assert responses_client["chat_completion"]("gpt-test", [])["content"] == ""
        assert responses_create.call_args_list[0].args[0]["reasoning"] == {"effort": "high"}
        assert "reasoning" not in responses_create.call_args_list[1].args[0]

        # Cached call should not include reasoning
        responses_cached = Mock(return_value={"output": []})
        responses_client = providers._responses_client(responses_cached, lambda: ["gpt-test"])
        responses_client["chat_completion"]("gpt-test", [])
        assert "reasoning" not in responses_cached.call_args.args[0]

    def test_drops_reasoning_effort_for_chat_completions(self):
        final = SimpleNamespace(
            choices=[SimpleNamespace(message=SimpleNamespace(content="done", tool_calls=None))]
        )
        chat_create = Mock(
            side_effect=[api_error("Unknown parameter: reasoning_effort"), final]
        )
        chat_client = providers._chat_client(chat_create, lambda: ["gpt-test"], tools_map=lambda _: [])
        assert chat_client["chat_completion"]("gpt-test", []) == AssistantMessage("done")
        assert chat_create.call_args_list[0].args[0]["reasoning_effort"] == "high"
        assert "reasoning_effort" not in chat_create.call_args_list[1].args[0]


class TestMantleClient:
    def test_uses_sigv4_client_in_aws_credentials_mode(self, monkeypatch, tmp_path):
        sentinel = {"chat_completion": lambda *a, **k: None, "list_models": lambda: ["alpha", "beta"]}
        monkeypatch.setattr(providers, "default_region", lambda choice=None: "ap-southeast-2")
        monkeypatch.setattr(
            providers, "load_aws_credentials",
            lambda cwd=None, allow_login=True: {"access_key": "AKIA", "secret_key": "SECRET"},
        )
        monkeypatch.setattr(
            providers, "_bedrock_mantle_client",
            lambda credentials, region, timeout=providers.DEFAULT_HTTP_TIMEOUT: dict(sentinel),
        )
        client = providers._mantle_completion_client(tmp_path)
        assert client["list_models"]() == ["alpha", "beta"]
        assert client["chat_completion"] is sentinel["chat_completion"]

    def test_load_bedrock_model_list_uses_mantle_endpoint(self, monkeypatch, tmp_path):
        requested = {}
        monkeypatch.setattr(providers, "default_region", lambda choice=None: "ap-southeast-2")
        monkeypatch.setattr(
            providers, "load_aws_credentials",
            lambda cwd=None, allow_login=True: {
                "access_key": "AKIDEXAMPLE",
                "secret_key": "wJalrXUtnFEMI/I/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "session_token": "TOKEN",
            },
        )
        monkeypatch.setattr(
            providers, "_bedrock_request_headers",
            lambda credentials, region, method, url, body=b"", headers=None: {
                "Authorization": "AWS4-HMAC-SHA256 test",
                "X-Amz-Date": "20260327T062009Z",
                "X-Amz-Security-Token": "TOKEN",
                **(headers or {}),
            },
        )
        monkeypatch.setattr(
            providers, "llm_session",
            lambda **kwargs: SimpleNamespace(
                request=lambda method, url, **req: requested.update({"method": method, "url": url, **req})
                    or providers.response_adapter(
                        status_code=200,
                        headers={"Content-Type": "application/json"},
                        text='{"data":[{"id":"zai.glm-4.6"},{"id":"moonshotai.kimi-k2-thinking"}]}',
                        content=b'{"data":[{"id":"zai.glm-4.6"},{"id":"moonshotai.kimi-k2-thinking"}]}',
                        url=url, reason_phrase="OK",
                    )
            ),
        )
        assert providers.load_bedrock_model_list(tmp_path) == ["zai.glm-4.6", "moonshotai.kimi-k2-thinking"]
        assert requested["method"] == "GET"
        assert requested["url"] == "https://bedrock-mantle.ap-southeast-2.api.aws/v1/models"
        assert requested["headers"]["Authorization"] == "AWS4-HMAC-SHA256 test"


class TestShimRegistry:
    def test_registry_order_and_client_building(self, monkeypatch, tmp_path):
        calls: list[str] = []
        sentinel = {"chat_completion": lambda *a, **k: None, "list_models": lambda: ["demo"]}

        def ok(name: str):
            def ensure_env(cwd):
                assert cwd == tmp_path
                calls.append(name)
            return ensure_env

        def fail(name: str):
            def ensure_env(cwd):
                assert cwd == tmp_path
                calls.append(name)
                raise RuntimeError(name)
            return ensure_env

        specs = {
            "alpha": {
                "name": "alpha",
                "ensure_env": ok("alpha"),
                "build_client": lambda cwd: sentinel,
                "list_models": lambda cwd: ["demo"],
            },
            "beta": {
                "name": "beta",
                "ensure_env": fail("beta"),
                "build_client": lambda cwd: None,
                "list_models": lambda cwd: [],
            },
            "gamma": {
                "name": "gamma",
                "ensure_env": ok("gamma"),
                "build_client": lambda cwd: None,
                "list_models": lambda cwd: [],
            },
        }
        monkeypatch.setattr(providers, "SHIM_ORDER", ("alpha", "beta", "gamma"), raising=False)
        monkeypatch.setattr(providers, "KNOWN_SHIMS", set(specs), raising=False)
        monkeypatch.setattr(providers, "SHIM_SPECS", specs, raising=False)
        monkeypatch.setattr(providers, "_shim_available", lambda shim: providers._shim_env_error(specs[shim], tmp_path) is None)

        assert providers.detect_available_shims() == ["alpha", "gamma"]
        assert calls == ["alpha", "beta", "gamma"]
        assert providers.ensure_api_env("alpha:model", None, tmp_path) == (True, None)
        assert providers.get_client("alpha", cwd=tmp_path) is sentinel
        assert providers.list_models_for_shim("alpha", cwd=tmp_path) == ["alpha:demo"]
        assert providers.list_models_for_shim("gamma", cwd=tmp_path) == []

    def test_list_models_error_handling(self, monkeypatch, tmp_path):
        calls = []

        specs = {
            "gamma": {
                "name": "gamma",
                "ensure_env": lambda cwd: calls.append(cwd),
                "build_client": lambda cwd: None,
                "list_models": lambda cwd: (_ for _ in ()).throw(RuntimeError("boom")),
            },
        }
        monkeypatch.setattr(providers, "SHIM_ORDER", ("gamma",), raising=False)
        monkeypatch.setattr(providers, "KNOWN_SHIMS", set(specs), raising=False)
        monkeypatch.setattr(providers, "SHIM_SPECS", specs, raising=False)
        monkeypatch.setattr(providers, "_shim_available", lambda shim: True)

        assert providers.list_models_for_shim("gamma", cwd=tmp_path) == []
        with pytest.raises(RuntimeError, match="boom"):
            providers.list_models_for_shim("gamma", cwd=tmp_path, ignore_errors=False)


class TestToolOutputRoundTrip:
    def test_failed_tool_results_keep_failure_metadata(self):
        result = ToolResult(
            ok=False,
            content={"tool": "read", "error_type": "ValueError", "message": "read path does not exist: missing.txt"},
        )
        assert providers._tool_output_value(result) == {
            "ok": False,
            "tool": "read",
            "error_type": "ValueError",
            "message": "read path does not exist: missing.txt",
        }

        openai_tool = providers._openai_chat_message(ToolMessage("call_1", "read", result))
        assert 'ok: false' in openai_tool["content"]
        assert 'read path does not exist: missing.txt' in openai_tool["content"]

        responses_input = providers._responses_input_from_messages([ToolMessage("call_1", "read", result)])
        assert len(responses_input) == 1
        assert responses_input[0]["type"] == "function_call_output"
        assert 'ok: false' in responses_input[0]["output"]
