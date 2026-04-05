"""Tests for providers module: HTTP, auth, shims, message handling."""

from __future__ import annotations

import json
import subprocess
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

    def test_http_sessions_treat_none_timeout_as_default(self):
        llm = providers.llm_session(timeout=None)
        try:
            assert llm.timeout == providers.DEFAULT_HTTP_TIMEOUT
        finally:
            llm.close()


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
            "https://bedrock-mantle.ap-southeast-2.api.aws/v1/responses",
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
                        "content": [{"text": "hello"}],
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
            "hello",
            tool_calls=[ToolCall(id="call_1", name="echo", arguments={"value": "x"})],
        )

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

    def test_raises_clear_error_when_responses_api_is_not_supported(self):
        responses_create = Mock(side_effect=[api_error("Unknown path: /responses")])
        client = providers._responses_client(responses_create, lambda: ["gpt-test"])
        with pytest.raises(RuntimeError, match="does not support the Open Responses / Responses API required by oy"):
            client["chat_completion"]("gpt-test", [])


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
            "_call_responses",
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

    def test_responses_client_retries_malformed_tool_arguments(self, monkeypatch):
        self._fake_retry(monkeypatch)
        retry_notes = []
        responses = iter(
            [
                {
                    "output": [
                        {
                            "type": "function_call",
                            "call_id": "call_1",
                            "name": "echo",
                            "arguments": '{"count":',
                        }
                    ]
                },
                {
                    "output": [
                        {
                            "type": "function_call",
                            "call_id": "call_1",
                            "name": "echo",
                            "arguments": '{"count":2}',
                        }
                    ]
                },
            ]
        )
        monkeypatch.setattr(
            providers,
            "_call_responses",
            lambda *args, **kwargs: next(responses),
        )

        client = providers._responses_client(Mock(), lambda: ["gpt-test"])
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
            "_call_responses",
            lambda *args, **kwargs: attempts.append("call") or {"output": []},
        )

        client = providers._responses_client(Mock(), lambda: ["gpt-test"])
        with pytest.raises(providers.RetryableDecodeError):
            client["chat_completion"]("gpt-test", [])
        assert len(attempts) == providers.MALFORMED_OUTPUT_RETRY_ATTEMPTS


class TestClientFactories:
    def test_responses_from_key_uses_openai_helpers(self, monkeypatch):
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

        responses = providers._responses_from_key("key")

        assert responses == {
            "kind": "responses",
            "create": "create:/responses",
            "list_models": "models:0",
        }
        assert created == [
            ({"api": 0}, "/responses", "Responses API"),
        ]


    def test_client_model_lister_passes_cwd_positionally(self, tmp_path):
        calls = []

        def build_client(cwd):
            calls.append(cwd)
            return {"list_models": lambda: ["demo"]}

        list_models = providers._client_model_lister(build_client)

        assert list_models(tmp_path) == ["demo"]
        assert calls == [tmp_path]

    def test_opencode_client_uses_zen_lookup_and_builder(self, monkeypatch):
        monkeypatch.delenv(providers.OPENCODE_SHARED_ENV_VAR, raising=False)
        monkeypatch.setattr(
            providers,
            "_load_opencode_auth",
            lambda: {"opencode": {"key": "zen-key"}},
        )
        seen = []
        monkeypatch.setattr(
            providers,
            "_responses_from_key",
            lambda api_key, *, base_url=None, **kwargs: (
                seen.append((api_key, base_url, kwargs))
                or {"api_key": api_key, "base_url": base_url, **kwargs}
            ),
        )
        monkeypatch.setattr(
            providers,
            "_opencode_list_models",
            lambda name, provider_id, base_url: [f"{provider_id}-model"],
        )

        assert providers._opencode_api_key("opencode") == "zen-key"
        zen = providers._opencode_zen_client()
        assert zen["list_models"]() == ["opencode-model"]
        assert seen == [("zen-key", providers.OPENCODE_ZEN_URL, {})]

    def test_opencode_api_key_prefers_shared_env(self, monkeypatch):
        monkeypatch.setenv(providers.OPENCODE_SHARED_ENV_VAR, "shared-key")
        monkeypatch.setattr(
            providers,
            "_load_opencode_auth",
            lambda: {"opencode": {"key": "zen-key"}},
        )
        assert providers._opencode_api_key("opencode") == "shared-key"

        monkeypatch.delenv(providers.OPENCODE_SHARED_ENV_VAR, raising=False)
        monkeypatch.setattr(
            providers,
            "_load_opencode_auth",
            lambda: {"opencode": {"key": "zen-only"}},
        )
        assert providers._opencode_api_key("opencode") == "zen-only"

    def test_local_list_models_reads_models_endpoint(self, monkeypatch):
        monkeypatch.setattr(
            providers,
            "_openai",
            lambda *args, **kwargs: {"base_url": kwargs["base_url"]},
        )
        monkeypatch.setattr(
            providers,
            "_model_ids",
            lambda api: ["hf://Qwen/Qwen3.5-9B", "ollama://qwen3.5"],
        )
        assert providers._local_list_models(shim="local-8080") == [
            "hf://Qwen/Qwen3.5-9B",
            "ollama://qwen3.5",
        ]

    def test_require_local_env_checks_models_endpoint(self, monkeypatch):
        calls = []
        monkeypatch.setattr(
            providers,
            "_openai",
            lambda *args, **kwargs: {"base_url": kwargs["base_url"]},
        )
        monkeypatch.setattr(providers, "_model_ids", lambda api: calls.append(api) or ["qwen"])
        providers._require_local_env(shim="local-8080")
        assert calls == [{"base_url": "http://127.0.0.1:8080/v1"}]

    def test_local_base_url_prefers_explicit_env(self, monkeypatch):
        monkeypatch.setenv("OY_LOCAL_9999_URL", "http://127.0.0.1:9999/v1/")
        assert providers._local_base_url("local-9999") == "http://127.0.0.1:9999/v1"

    def test_local_client_uses_external_server(self, monkeypatch):
        monkeypatch.setenv("OY_MODEL", "local-8080:qwen3.5")
        monkeypatch.setattr(providers, "_local_base_url", lambda shim, cwd=None: "http://127.0.0.1:8080/v1")
        calls = []
        monkeypatch.setattr(
            providers,
            "_req_json",
            lambda api, method, path, *, source, json_body=None, data=None, headers=None: (
                calls.append((api, method, path, source, json_body))
                or {"output": [{"type": "message", "role": "assistant", "content": [{"text": "done"}]}]}
            ),
        )
        client = providers._local_client(shim="local-8080")
        assert client["chat_completion"]("qwen3.5", []) == AssistantMessage("done")
        assert calls[0][0]["base_url"] == "http://127.0.0.1:8080/v1"
        assert calls[0][1:] == (
            "POST",
            "/responses",
            "local-8080 responses",
            {"model": "qwen3.5", "input": [], "store": False, "reasoning": {"effort": "high"}},
        )

    def test_opencode_list_models_uses_remote_models_endpoint(self, monkeypatch):
        monkeypatch.setattr(providers, "_opencode_api_key", lambda name: "key")
        monkeypatch.setattr(providers, "_openai", lambda *args, **kwargs: {"api": "ok"})
        monkeypatch.setattr(
            providers,
            "_model_ids",
            lambda api: ["chat-a", "responses-b"],
        )
        assert providers._opencode_list_models(
            "opencode", "opencode", providers.OPENCODE_ZEN_URL
        ) == ["chat-a", "responses-b"]


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

    def test_bedrock_mantle_client_posts_to_responses_api(self, monkeypatch):
        requested = {}
        monkeypatch.setattr(
            providers,
            "_bedrock_request_headers",
            lambda credentials, region, method, url, body=b"", headers=None: {
                "Authorization": "AWS4-HMAC-SHA256 test",
                "X-Amz-Date": "20260327T062009Z",
                **(headers or {}),
            },
        )

        class FakeSession:
            def request(self, method, url, **req):
                requested.update({"method": method, "url": url, **req})
                return providers.response_adapter(
                    status_code=200,
                    headers={"Content-Type": "application/json"},
                    text='{"output":[{"type":"message","role":"assistant","content":[{"text":"done"}]}]}',
                    content=b'{"output":[{"type":"message","role":"assistant","content":[{"text":"done"}]}]}',
                    url=url,
                    reason_phrase="OK",
                )

        monkeypatch.setattr(providers, "llm_session", lambda **kwargs: FakeSession())
        client = providers._bedrock_mantle_client(
            {"access_key": "AKIA", "secret_key": "SECRET"},
            "ap-southeast-2",
        )
        assert client["chat_completion"]("zai.glm-4.6", []) == AssistantMessage("done")
        assert requested["method"] == "POST"
        assert (
            requested["url"]
            == "https://bedrock-mantle.ap-southeast-2.api.aws/v1/responses"
        )
        assert requested["headers"]["Authorization"] == "AWS4-HMAC-SHA256 test"
        assert requested["headers"]["Content-Type"] == "application/json"
        assert b'"model":"zai.glm-4.6"' in requested["data"]
        assert b'"reasoning":{"effort":"high"}' in requested["data"]

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
                "list_models": lambda cwd: ["demo"],
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
        assert providers.list_models_for_shim("gamma", cwd=tmp_path) == ["gamma:demo"]

    def test_dynamic_local_shim_is_valid_and_builds(self, monkeypatch):
        monkeypatch.setattr(providers, "_openai", lambda *args, **kwargs: {"base_url": kwargs["base_url"]})
        monkeypatch.setattr(providers, "_model_ids", lambda api: ["demo"])
        assert providers.validate_shim("local-8080") == "local-8080"
        assert providers.list_models_for_shim("local-8080") == ["local-8080:demo"]

    def test_local_current_model_accepts_bare_model_id_with_colon(self):
        assert (
            providers._local_current_model(
                "local-8080", "unsloth/gemma-4-31B-it-GGUF:UD-Q4_K_XL"
            )
            == "unsloth/gemma-4-31B-it-GGUF:UD-Q4_K_XL"
        )

    def test_local_client_accepts_bare_model_id_with_colon(self, monkeypatch):
        calls = []
        monkeypatch.setattr(providers, "_local_base_url", lambda shim, cwd=None: "http://127.0.0.1:8080/v1")
        monkeypatch.setattr(
            providers,
            "_req_json",
            lambda api, method, path, *, source, json_body=None, data=None, headers=None: (
                calls.append((api, method, path, source, json_body))
                or {"output": [{"type": "message", "role": "assistant", "content": [{"text": "done"}]}]}
            ),
        )
        client = providers._local_client(
            shim="local-8080", model_spec="unsloth/gemma-4-31B-it-GGUF:UD-Q4_K_XL"
        )
        assert client["chat_completion"]("", []) == AssistantMessage("done")
        assert calls[0][3] == "local-8080 responses"
        assert calls[0][4]["model"] == "unsloth/gemma-4-31B-it-GGUF:UD-Q4_K_XL"

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

        with pytest.raises(RuntimeError, match="boom"):
            providers.list_models_for_shim("gamma", cwd=tmp_path)


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

        output_text = providers._tool_output_text(result)
        assert "ok: false" in output_text
        assert "read path does not exist: missing.txt" in output_text

        responses_input = providers._responses_input_from_messages(
            [ToolMessage("call_1", "read", result)]
        )
        assert len(responses_input) == 1
        assert responses_input[0]["type"] == "function_call_output"
        assert "ok: false" in responses_input[0]["output"]
