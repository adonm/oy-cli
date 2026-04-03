"""Tests for providers module: HTTP, auth, shims, message handling."""

from __future__ import annotations

import json
from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from oy_cli import providers
from oy_cli.providers import AssistantMessage, ToolCall, ToolMessage, ToolResult
from tests.conftest import api_error, raw_response


class TestJSONHelpers:
    def test_normalize_jsonlike_converts_pathlike(self, tmp_path):
        path = tmp_path / "demo.txt"
        assert providers.normalize_jsonlike({"path": path}) == {"path": str(path)}

    def test_auth_loaders_ignore_non_object_json(self, monkeypatch, tmp_path):
        codex = tmp_path / "codex.json"
        codex.write_text("[]", encoding="utf-8")
        opencode = tmp_path / "opencode.json"
        opencode.write_text("[]", encoding="utf-8")
        monkeypatch.setattr(providers, "CODEX_AUTH_PATH", codex)
        monkeypatch.setattr(providers, "OPENCODE_AUTH_PATH", opencode)
        assert providers.load_codex_auth() == {}
        assert providers._opencode_api_key("opencode") is None
        assert providers._opencode_api_key("opencode-go") is None


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

    def test_http_sessions_allow_timeout_and_redirect_overrides(self):
        llm = providers.llm_session(timeout=1.5, follow_redirects=True)
        tool = providers.tool_session(timeout=2.5, allow_redirects=True)
        try:
            assert llm.timeout == 1.5
            assert llm.follow_redirects is True
            assert tool.timeout == 2.5
            assert tool.follow_redirects is True
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
        assert providers._decode_tool_call_arguments(json.dumps('{"count":2}')) == {
            "count": 2
        }
        assert providers._decode_tool_call_arguments('{"ok":true}{"ok":true}') == {
            "ok": True
        }

    def test_responses_output_decoding(self):
        decoded = providers._decode_responses_output(
            {
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            {"text": "hello"},
                            {"refusal": "nope"},
                            {"text": "   "},
                        ],
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
        assert decoded == AssistantMessage(
            "hello\n\nnope",
            tool_calls=[ToolCall(id="call_1", name="echo", arguments={"value": "x"})],
        )

    def test_chat_completion_to_assistant_message(self):
        chat_message = providers._chat_completion_to_assistant_message(
            SimpleNamespace(
                choices=[
                    SimpleNamespace(
                        message=SimpleNamespace(content="hello", tool_calls=None)
                    ),
                    SimpleNamespace(
                        message=SimpleNamespace(
                            content="hello",
                            tool_calls=[
                                SimpleNamespace(
                                    id="call_2",
                                    function=SimpleNamespace(
                                        name="echo", arguments='{"count":2}'
                                    ),
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
                        message=SimpleNamespace(
                            content="", reasoning="thoughts", tool_calls=None
                        )
                    )
                ]
            )
        )
        assert reasoning_only == AssistantMessage("thoughts")


class TestReasoningFallback:
    def test_drops_unsupported_reasoning_after_first_rejection(self):
        responses_create = Mock(
            side_effect=[
                api_error("Unsupported parameter: reasoning"),
                {
                    "output": [
                        {
                            "type": "message",
                            "role": "assistant",
                            "content": [{"text": "done"}],
                        }
                    ]
                },
            ]
        )
        responses_client = providers._responses_client(
            responses_create, lambda: ["gpt-test"]
        )
        assert responses_client["chat_completion"]("gpt-test", []) == AssistantMessage(
            "done"
        )
        assert responses_create.call_args_list[0].args[0]["reasoning"] == {
            "effort": "high"
        }
        assert "reasoning" not in responses_create.call_args_list[1].args[0]

        # Cached call should not include reasoning
        responses_cached = Mock(
            return_value={
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"text": "cached"}],
                    }
                ]
            }
        )
        responses_client = providers._responses_client(
            responses_cached, lambda: ["gpt-test"]
        )
        assert responses_client["chat_completion"]("gpt-test", []) == AssistantMessage(
            "cached"
        )
        assert "reasoning" not in responses_cached.call_args.args[0]

    def test_drops_reasoning_effort_for_chat_completions(self):
        final = SimpleNamespace(
            choices=[
                SimpleNamespace(
                    message=SimpleNamespace(content="done", tool_calls=None)
                )
            ]
        )
        chat_create = Mock(
            side_effect=[api_error("Unknown parameter: reasoning_effort"), final]
        )
        chat_client = providers._chat_client(
            chat_create, lambda: ["gpt-test"], tools_map=lambda _: []
        )
        assert chat_client["chat_completion"]("gpt-test", []) == AssistantMessage(
            "done"
        )
        assert chat_create.call_args_list[0].args[0]["reasoning_effort"] == "high"
        assert "reasoning_effort" not in chat_create.call_args_list[1].args[0]


class TestMalformedOutputRetry:
    @staticmethod
    def _fake_retry(monkeypatch):
        def fake_call_with_retry(call, *, max_attempts=1, on_retry=None):
            last_exc = None
            for attempt in range(1, max_attempts + 1):
                if on_retry and attempt > 1:
                    on_retry(
                        attempt,
                        max_attempts,
                        providers._retry_error_context(last_exc),
                    )
                try:
                    return call()
                except providers.RetryableDecodeError as exc:
                    last_exc = exc
                    if attempt >= max_attempts:
                        raise
            raise AssertionError("retry loop should have returned or raised")

        monkeypatch.setattr(providers, "_call_with_retry", fake_call_with_retry)

    def test_responses_client_retries_empty_assistant_message(
        self, monkeypatch
    ):
        self._fake_retry(monkeypatch)
        retry_notes = []
        responses = iter(
            [
                {"output": []},
                {
                    "output": [
                        {
                            "type": "message",
                            "role": "assistant",
                            "content": [{"text": "done"}],
                        }
                    ]
                },
            ]
        )
        monkeypatch.setattr(
            providers,
            "_call_with_reasoning_fallback",
            lambda *args, **kwargs: next(responses),
        )

        client = providers._responses_client(Mock(), lambda: ["gpt-test"])
        assert client["chat_completion"](
            "gpt-test",
            [],
            on_retry=lambda *args: retry_notes.append(args),
        ) == AssistantMessage("done")
        assert retry_notes == [
            (
                2,
                providers.MALFORMED_OUTPUT_RETRY_ATTEMPTS,
                "malformed model output: empty assistant message with no tool calls",
            )
        ]

    def test_chat_client_retries_malformed_tool_arguments(self, monkeypatch):
        self._fake_retry(monkeypatch)
        retry_notes = []
        responses = iter(
            [
                SimpleNamespace(
                    choices=[
                        SimpleNamespace(
                            message=SimpleNamespace(
                                content="",
                                tool_calls=[
                                    SimpleNamespace(
                                        id="call_1",
                                        function=SimpleNamespace(
                                            name="echo", arguments='{"count":'
                                        ),
                                    )
                                ],
                            )
                        )
                    ]
                ),
                SimpleNamespace(
                    choices=[
                        SimpleNamespace(
                            message=SimpleNamespace(
                                content="",
                                tool_calls=[
                                    SimpleNamespace(
                                        id="call_1",
                                        function=SimpleNamespace(
                                            name="echo", arguments='{"count":2}'
                                        ),
                                    )
                                ],
                            )
                        )
                    ]
                ),
            ]
        )
        monkeypatch.setattr(
            providers,
            "_call_with_reasoning_fallback",
            lambda *args, **kwargs: next(responses),
        )

        client = providers._chat_client(Mock(), lambda: ["gpt-test"], tools_map=lambda _: [])
        assert client["chat_completion"](
            "gpt-test",
            [],
            on_retry=lambda *args: retry_notes.append(args),
        ) == AssistantMessage(
            "",
            tool_calls=[ToolCall(id="call_1", name="echo", arguments={"count": 2})],
        )
        assert retry_notes and retry_notes[0][0:2] == (
            2,
            providers.MALFORMED_OUTPUT_RETRY_ATTEMPTS,
        )
        assert "Could not parse tool arguments JSON" in retry_notes[0][2]

    def test_responses_client_raises_after_retry_budget_exhausted(
        self, monkeypatch
    ):
        self._fake_retry(monkeypatch)
        attempts = []
        monkeypatch.setattr(
            providers,
            "_call_with_reasoning_fallback",
            lambda *args, **kwargs: attempts.append("call") or {"output": []},
        )

        client = providers._responses_client(Mock(), lambda: ["gpt-test"])
        with pytest.raises(providers.RetryableDecodeError):
            client["chat_completion"]("gpt-test", [])
        assert len(attempts) == providers.MALFORMED_OUTPUT_RETRY_ATTEMPTS


class TestClientFactories:
    def test_responses_and_chat_clients_share_openai_helpers(self, monkeypatch):
        created = []

        def fake_openai(*args, **kwargs):
            return {"api": len(created)}

        def fake_create(api, path, *, source):
            created.append((api, path, source))
            return f"create:{path}"

        monkeypatch.setattr(providers, "_openai", fake_openai)
        monkeypatch.setattr(providers, "_openai_json_create", fake_create)
        monkeypatch.setattr(
            providers, "_openai_model_lister", lambda api: f"models:{api['api']}"
        )
        monkeypatch.setattr(
            providers,
            "_responses_client",
            lambda create, list_models, **kwargs: {
                "kind": "responses",
                "create": create,
                "list_models": list_models,
                **kwargs,
            },
        )
        monkeypatch.setattr(
            providers,
            "_chat_client",
            lambda create, list_models, **kwargs: {
                "kind": "chat",
                "create": create,
                "list_models": list_models,
                **kwargs,
            },
        )

        responses = providers._responses_from_key(
            "key", fallback=list, default=["demo"]
        )
        chat = providers._chat_from_key("key")

        assert responses == {
            "kind": "responses",
            "create": "create:/responses",
            "list_models": "models:0",
            "fallback": list,
            "default": ["demo"],
        }
        assert chat["kind"] == "chat"
        assert chat["create"] == "create:/chat/completions"
        assert chat["list_models"] == "models:1"
        assert created == [
            ({"api": 0}, "/responses", "Responses API"),
            ({"api": 1}, "/chat/completions", "Chat Completions API"),
        ]

    def test_opencode_clients_share_lookup_and_builder(self, monkeypatch):
        monkeypatch.setattr(
            providers,
            "_load_opencode_auth",
            lambda: {"opencode": {"key": "zen-key"}, "opencode-go": {"key": "go-key"}},
        )
        seen = []
        monkeypatch.setattr(
            providers,
            "_chat_from_key",
            lambda api_key, *, base_url=None, **kwargs: (
                seen.append((api_key, base_url, kwargs))
                or {"api_key": api_key, "base_url": base_url}
            ),
        )

        assert providers._opencode_api_key("opencode") == "zen-key"
        assert providers._opencode_api_key("opencode-go") == "go-key"
        assert providers._opencode_zen_client() == {
            "api_key": "zen-key",
            "base_url": providers.OPENCODE_ZEN_URL,
        }
        assert providers._opencode_go_client() == {
            "api_key": "go-key",
            "base_url": providers.OPENCODE_GO_URL,
        }
        assert seen == [
            ("zen-key", providers.OPENCODE_ZEN_URL, {}),
            ("go-key", providers.OPENCODE_GO_URL, {}),
        ]


class TestMantleClient:
    def test_uses_sigv4_client_in_aws_credentials_mode(self, monkeypatch, tmp_path):
        sentinel = {
            "chat_completion": lambda *a, **k: None,
            "list_models": lambda: ["alpha", "beta"],
        }
        monkeypatch.setattr(
            providers, "default_region", lambda choice=None: "ap-southeast-2"
        )
        monkeypatch.setattr(
            providers,
            "load_aws_credentials",
            lambda cwd=None, allow_login=True: {
                "access_key": "AKIA",
                "secret_key": "SECRET",
            },
        )
        monkeypatch.setattr(
            providers,
            "_bedrock_mantle_client",
            lambda credentials, region, timeout=providers.DEFAULT_HTTP_TIMEOUT: dict(
                sentinel
            ),
        )
        client = providers._mantle_completion_client(tmp_path)
        assert client["list_models"]() == ["alpha", "beta"]
        assert client["chat_completion"] is sentinel["chat_completion"]

    def test_load_bedrock_model_list_uses_mantle_endpoint(self, monkeypatch, tmp_path):
        requested = {}
        monkeypatch.setattr(
            providers, "default_region", lambda choice=None: "ap-southeast-2"
        )
        monkeypatch.setattr(
            providers,
            "load_aws_credentials",
            lambda cwd=None, allow_login=True: {
                "access_key": "AKIDEXAMPLE",
                "secret_key": "wJalrXUtnFEMI/I/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "session_token": "TOKEN",
            },
        )
        monkeypatch.setattr(
            providers,
            "_bedrock_request_headers",
            lambda credentials, region, method, url, body=b"", headers=None: {
                "Authorization": "AWS4-HMAC-SHA256 test",
                "X-Amz-Date": "20260327T062009Z",
                "X-Amz-Security-Token": "TOKEN",
                **(headers or {}),
            },
        )

        class FakeSession:
            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc, tb):
                return False

            def request(self, method, url, **req):
                requested.update({"method": method, "url": url, **req})
                return providers.response_adapter(
                    status_code=200,
                    headers={"Content-Type": "application/json"},
                    text='{"data":[{"id":"zai.glm-4.6"},{"id":"moonshotai.kimi-k2-thinking"}]}',
                    content=b'{"data":[{"id":"zai.glm-4.6"},{"id":"moonshotai.kimi-k2-thinking"}]}',
                    url=url,
                    reason_phrase="OK",
                )

        monkeypatch.setattr(providers, "llm_session", lambda **kwargs: FakeSession())
        assert providers.load_bedrock_model_list(tmp_path) == [
            "zai.glm-4.6",
            "moonshotai.kimi-k2-thinking",
        ]
        assert requested["method"] == "GET"
        assert (
            requested["url"]
            == "https://bedrock-mantle.ap-southeast-2.api.aws/v1/models"
        )
        assert requested["headers"]["Authorization"] == "AWS4-HMAC-SHA256 test"


class TestShimRegistry:
    def test_registry_order_and_client_building(self, monkeypatch, tmp_path):
        calls: list[str] = []
        sentinel = {
            "chat_completion": lambda *a, **k: None,
            "list_models": lambda: ["demo"],
        }

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
                "ensure_env": ok("alpha"),
                "build_client": lambda cwd: sentinel,
                "list_models": lambda cwd: ["demo"],
            },
            "beta": {
                "ensure_env": fail("beta"),
                "build_client": lambda cwd: None,
                "list_models": lambda cwd: [],
            },
            "gamma": {
                "ensure_env": ok("gamma"),
                "build_client": lambda cwd: None,
                "list_models": lambda cwd: [],
            },
        }
        monkeypatch.setattr(
            providers, "SHIM_ORDER", ("alpha", "beta", "gamma"), raising=False
        )
        monkeypatch.setattr(providers, "KNOWN_SHIMS", set(specs), raising=False)
        monkeypatch.setattr(providers, "SHIM_SPECS", specs, raising=False)
        monkeypatch.setattr(
            providers,
            "_shim_available",
            lambda shim: providers._shim_env_error(specs[shim], tmp_path) is None,
        )

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
            content={
                "tool": "read",
                "error_type": "ValueError",
                "message": "read path does not exist: missing.txt",
            },
        )
        assert providers._tool_output_value(result) == {
            "ok": False,
            "tool": "read",
            "error_type": "ValueError",
            "message": "read path does not exist: missing.txt",
        }

        openai_tool = providers._openai_chat_message(
            ToolMessage("call_1", "read", result)
        )
        assert "ok: false" in openai_tool["content"]
        assert "read path does not exist: missing.txt" in openai_tool["content"]

        responses_input = providers._responses_input_from_messages(
            [ToolMessage("call_1", "read", result)]
        )
        assert len(responses_input) == 1
        assert responses_input[0]["type"] == "function_call_output"
        assert "ok: false" in responses_input[0]["output"]
