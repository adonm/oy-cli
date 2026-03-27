from __future__ import annotations

import json
import zipfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from oy_cli import agent, cli, providers, runtime as rt, tools
from oy_cli.providers import AssistantMessage, SystemMessage, ToolCall, ToolMessage, ToolResult, UserMessage


@pytest.fixture(autouse=True)
def _reset_provider_state():
    providers._REASONING_SUPPORT_CACHE.clear()
    providers.command_env.cache_clear()
    yield
    providers._REASONING_SUPPORT_CACHE.clear()
    providers.command_env.cache_clear()


def make_state(
    root: Path,
    *,
    interactive: bool = False,
    yolo: bool = False,
    registry: dict[str, dict[str, object]] | None = None,
):
    return agent.agent_state(
        root=root,
        tool_registry=tools.TOOL_REGISTRY if registry is None else registry,
        unattended_timeout_seconds=3600,
        unattended_deadline=float("inf"),
        interactive=interactive,
        approve_all_mutating_tools=yolo,
        yolo=yolo,
    )


def raw_response(**overrides):
    data = {
        "status_code": 200,
        "headers": {"Content-Type": "text/plain"},
        "text": "hello world",
        "content": b"hello world",
        "url": "https://example.com",
        "reason": "OK",
        "http_version": 2,
    }
    data.update(overrides)
    return SimpleNamespace(**data)


def api_error(message: str, *, status_code: int = 400):
    return providers.APIStatusError(
        message,
        response=providers.response_adapter(
            status_code=status_code,
            headers={},
            text=json.dumps({"error": {"message": message}}),
            content=b"",
            url="https://example.com",
            reason_phrase="Bad Request",
        ),
        body=None,
    )


def tool_handler(name: str, fn, *, mutating: bool = False):
    return {
        name: {
            "name": name,
            "fn": fn,
            "description": name,
            "parameters": {"type": "object"},
            "mutating": mutating,
        }
    }


def test_tool_specs_use_closed_object_schemas():
    specs = {tool["name"]: tool for tool in tools.tool_specs()}
    assert specs["todo"]["parameters"]["additionalProperties"] is False
    assert specs["todo"]["parameters"]["properties"]["todos"]["items"]["additionalProperties"] is False


class DummyHttpClient:
    def __init__(self, response=None, error=None, **kwargs):
        self.response = response
        self.error = error
        self.kwargs = kwargs
        self.called = None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False

    def request(self, method, url, headers=None):
        self.called = (method, url, headers)
        if self.error:
            raise self.error
        return self.response


def test_adapt_response_and_webfetch_accept_response_objects(monkeypatch, tmp_path):
    response = raw_response()
    adapted = providers.adapt_response(response)
    assert adapted["status_code"] == 200
    assert adapted["headers"]["content-type"] == "text/plain"
    assert adapted["http_version"] == "HTTP/2"

    created: list[DummyHttpClient] = []

    def fake_http_client(**kwargs):
        client = DummyHttpClient(response=response, **kwargs)
        created.append(client)
        return client

    shown: list[str] = []
    monkeypatch.setattr(tools, "_validate_url_safe", lambda url: url)
    monkeypatch.setattr(rt, "tool_session", fake_http_client)
    monkeypatch.setattr(rt, "show", shown.append)

    payload = tools.tool_webfetch(
        make_state(tmp_path),
        url="https://example.com",
        headers={"Accept": "text/plain"},
        follow_redirects=True,
        timeout_seconds=9,
    )

    assert created[0].kwargs == {"timeout": 9, "follow_redirects": True}
    assert created[0].called == ("GET", "https://example.com", {"Accept": "text/plain"})
    assert payload["status_code"] == 200
    assert payload["http_version"] == "HTTP/2"
    assert payload["headers"]["Content-Type"] == "text/plain"
    assert payload["text"] == "hello world"
    assert payload["format"] == "text"
    assert any("hello world" in text for text in shown)


def test_default_http_sessions_use_expected_defaults():
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


def test_reasoning_fallback_drops_unsupported_reasoning_after_first_rejection():
    responses_create = Mock(
        side_effect=[api_error("Unsupported parameter: reasoning"), {"output": []}]
    )
    responses_client = providers._responses_client(responses_create, lambda: ["gpt-test"])
    assert responses_client["chat_completion"]("gpt-test", [])["content"] == ""
    assert responses_create.call_args_list[0].args[0]["reasoning"] == {"effort": "high"}
    assert "reasoning" not in responses_create.call_args_list[1].args[0]

    responses_cached = Mock(return_value={"output": []})
    responses_client = providers._responses_client(responses_cached, lambda: ["gpt-test"])
    responses_client["chat_completion"]("gpt-test", [])
    assert "reasoning" not in responses_cached.call_args.args[0]

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

    chat_cached = Mock(return_value=final)
    chat_client = providers._chat_client(chat_cached, lambda: ["gpt-test"], tools_map=lambda _: [])
    chat_client["chat_completion"]("gpt-test", [])
    assert "reasoning_effort" not in chat_cached.call_args.args[0]


def test_sigv4_headers_sign_bedrock_mantle_requests():
    headers = providers._bedrock_request_headers(
        {
            "access_key": "AKIDEXAMPLE",
            "secret_key": "wJalrXUtnFEMI/I/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "session_token": "TOKEN",
        },
        "us-east-1",
        "POST",
        "https://bedrock-mantle.us-east-1.api.aws/v1/chat/completions",
        body=b'{"model":"zai.glm-4.6"}',
        headers={"Content-Type": "application/json"},
    )

    assert headers["Content-Type"] == "application/json"
    assert headers["Host"] == "bedrock-mantle.us-east-1.api.aws"
    assert headers["X-Amz-Security-Token"] == "TOKEN"
    assert "Credential=AKIDEXAMPLE/" in headers["Authorization"]
    assert "/us-east-1/bedrock-mantle/aws4_request" in headers["Authorization"]
    assert "SignedHeaders=content-type;host;x-amz-date;x-amz-security-token" in headers["Authorization"]



def test_provider_decoding_helpers_cover_json_and_message_shapes():
    assert providers._decode_tool_call_arguments(json.dumps('{"count":2}')) == {"count": 2}
    assert providers._decode_tool_call_arguments('{"ok":true}{"ok":true}') == {"ok": True}

    decoded = providers._decode_responses_output(
        {
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
        }
    )
    assert decoded == AssistantMessage(
        "hello\n\nnope",
        tool_calls=[ToolCall(id="call_1", name="echo", arguments={"value": "x"})],
    )

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


def test_mantle_completion_client_uses_sigv4_client_in_aws_credentials_mode(monkeypatch, tmp_path):
    sentinel = {"chat_completion": lambda *a, **k: None, "list_models": lambda: ["alpha", "beta"]}
    monkeypatch.setattr(providers, "default_region", lambda choice=None: "us-east-1")
    monkeypatch.setattr(
        providers,
        "load_aws_credentials",
        lambda cwd=None, allow_login=True: {"access_key": "AKIA", "secret_key": "SECRET"},
    )
    monkeypatch.setattr(
        providers,
        "_bedrock_mantle_client",
        lambda credentials, region, timeout=providers.DEFAULT_HTTP_TIMEOUT: dict(sentinel),
    )

    client = providers._mantle_completion_client(tmp_path)
    assert client["list_models"]() == ["alpha", "beta"]
    assert client["chat_completion"] is sentinel["chat_completion"]


def test_load_bedrock_model_list_uses_mantle_models_endpoint(monkeypatch, tmp_path):
    requested = {}

    monkeypatch.setattr(providers, "default_region", lambda choice=None: "us-east-1")
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
    monkeypatch.setattr(
        providers,
        "llm_session",
        lambda **kwargs: SimpleNamespace(
            request=lambda method, url, **req: requested.update({"method": method, "url": url, **req}) or providers.response_adapter(
                status_code=200,
                headers={"Content-Type": "application/json"},
                text='{"data":[{"id":"zai.glm-4.6"},{"id":"moonshotai.kimi-k2-thinking"}]}',
                content=b'{"data":[{"id":"zai.glm-4.6"},{"id":"moonshotai.kimi-k2-thinking"}]}',
                url=url,
                reason_phrase="OK",
            )
        ),
    )

    assert providers.load_bedrock_model_list(tmp_path) == ["zai.glm-4.6", "moonshotai.kimi-k2-thinking"]
    assert requested["method"] == "GET"
    assert requested["url"] == "https://bedrock-mantle.us-east-1.api.aws/v1/models"
    assert requested["headers"]["Authorization"] == "AWS4-HMAC-SHA256 test"
    assert requested["headers"]["X-Amz-Security-Token"] == "TOKEN"


def test_shim_registry_helpers_follow_registry_order_and_build_clients(monkeypatch, tmp_path):
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

    specs["gamma"]["list_models"] = lambda cwd: (_ for _ in ()).throw(RuntimeError("boom"))
    assert providers.list_models_for_shim("gamma", cwd=tmp_path) == []
    with pytest.raises(RuntimeError, match="boom"):
        providers.list_models_for_shim("gamma", cwd=tmp_path, ignore_errors=False)


def test_transcript_lifecycle_and_prepared_messages_pack_and_truncate(monkeypatch):
    tx = agent.transcript_with_system_prompt("sys")
    agent.add_user(tx, "hello")
    agent.clear_transcript(tx, "next")
    assert tx["messages"] == [SystemMessage("next")]
    assert agent.undo_last_turn(tx) is False

    monkeypatch.setattr(agent, "count_tokens", lambda text: len(text))

    truncated = agent.prepared_messages(
        agent.transcript(
            messages=[
                SystemMessage("sys"),
                UserMessage("abcdef"),
                UserMessage("ghij"),
                UserMessage("kl"),
            ],
            max_context_tokens=18,
            max_message_tokens=100,
        )
    )
    assert truncated[0] == SystemMessage("sys")
    assert truncated[1]["role"] == "user"
    assert "earlier messages omitted" in truncated[1]["content"]
    assert truncated[-1] == UserMessage("kl")

    monkeypatch.setattr(agent, "_packed_history_note", lambda messages: SystemMessage("packed"))
    packed = agent.prepared_messages(
        agent.transcript(
            messages=[
                SystemMessage("sys"),
                UserMessage("abcdef"),
                UserMessage("ghij"),
                UserMessage("mnop"),
                UserMessage("kl"),
            ],
            max_context_tokens=80,
            max_message_tokens=100,
        ),
        model="gpt-4o",
    )
    assert packed == [SystemMessage("sys"), SystemMessage("packed"), UserMessage("mnop"), UserMessage("kl")]

    kept_as_unit = agent.prepared_messages(
        agent.transcript(
            messages=[
                SystemMessage("sys"),
                UserMessage("earlier"),
                AssistantMessage("", tool_calls=[ToolCall(id="call_1", name="bash", arguments={})]),
                ToolMessage("call_1", "bash", ToolResult(ok=True, content="tool output")),
                UserMessage("tail"),
            ],
            max_context_tokens=23,
            max_message_tokens=100,
        )
    )
    assert kept_as_unit == [
        SystemMessage("sys"),
        UserMessage("... [3 earlier messages omitted to fit context limit]"),
        UserMessage("tail"),
    ]


def test_list_all_model_ids_warns_and_keeps_other_shims(monkeypatch, tmp_path):
    printed: list[tuple[str, str, bool]] = []
    warned: list[str] = []

    monkeypatch.setattr(rt, "detect_available_shims", lambda: ["alpha", "beta"])

    def fake_list_models_for_shim(shim, cwd=None, *, ignore_errors=True):
        assert cwd == tmp_path
        if shim == "alpha":
            return ["alpha:demo"]
        raise RuntimeError("boom\nsecond line")

    monkeypatch.setattr(rt, "list_models_for_shim", fake_list_models_for_shim)
    monkeypatch.setattr(rt, "_print", lambda kind="md", value="", err=False, extra=None: printed.append((kind, value, err)))
    monkeypatch.setattr(rt, "_warn", warned.append)
    monkeypatch.setattr(rt, "Path", SimpleNamespace(cwd=lambda: tmp_path))

    assert rt.list_all_model_ids() == ["alpha:demo"]
    assert printed == [
        ("status", "Loading models from `alpha`.", True),
        ("status", "Loading models from `beta`.", True),
    ]
    assert warned == ["Could not load models from `beta`: boom"]



def test_runtime_model_config_and_tool_registries_round_trip(tmp_path, monkeypatch):
    monkeypatch.setenv("OY_CONFIG", str(tmp_path / "config.json"))
    monkeypatch.delenv("OY_MODEL", raising=False)
    monkeypatch.delenv("OY_SHIM", raising=False)

    assert rt.save_model_config("openai:gpt-test") == {"model": "gpt-test", "shim": "openai"}
    assert rt.load_model_config() == {"model": "gpt-test", "shim": "openai"}
    assert rt._model(None) == "openai:gpt-test"

    monkeypatch.setenv("OY_SHIM", "copilot")
    monkeypatch.setenv("OY_MODEL", "gpt-live")
    monkeypatch.setenv("OY_YOLO", "yes")
    assert rt._model(None) == "copilot:gpt-live"
    assert rt.yolo_enabled() is True
    assert "ask" not in rt.active_tool_registry(False)
    assert set(rt.read_only_tool_registry()) == rt._READ_ONLY_TOOLS


def test_invoke_tool_validates_arguments_and_respects_approval_modes(monkeypatch, tmp_path):
    calls: list[str] = []

    def mutating(state, text: str):
        calls.append(text)
        return f"done:{text}"

    registry = tool_handler("mutating", mutating, mutating=True)
    monkeypatch.setattr(rt, "_note", lambda *a, **k: None)

    denied = make_state(tmp_path, interactive=True, registry=registry)
    monkeypatch.setattr(rt, "select", lambda *a, **k: "deny")
    result = tools.invoke_tool(registry, denied, "mutating", {"text": "nope"})
    assert result["ok"] is False
    assert result["content"]["error_type"] == "PermissionError"
    assert calls == []

    approved = make_state(tmp_path, interactive=True, registry=registry)
    monkeypatch.setattr(rt, "select", lambda *a, **k: "all")
    first = tools.invoke_tool(registry, approved, "mutating", {"text": "first"})
    second = tools.invoke_tool(registry, approved, "mutating", {"text": "second"})
    assert first["ok"] is True and second["ok"] is True
    assert approved["approve_all_mutating_tools"] is True
    assert calls == ["first", "second"]

    invalid = tools.invoke_tool(registry, make_state(tmp_path, registry=registry), "mutating", {})
    assert invalid["ok"] is False
    assert invalid["content"]["error_type"] == "ValidationError"


def test_ask_and_todo_tools_update_state_and_use_prompt_helpers(monkeypatch, tmp_path):
    state = make_state(tmp_path)
    shown: list[str] = []
    monkeypatch.setattr(rt, "require_prompt", lambda *a, **k: None)
    monkeypatch.setattr(rt, "ask", lambda *a, **k: " free ")
    monkeypatch.setattr(rt, "select", lambda *a, **k: "beta")
    monkeypatch.setattr(rt, "note_tool", lambda *a, **k: None)
    monkeypatch.setattr(rt, "show", shown.append)

    assert tools.tool_ask(state, "Question?") == "free"
    assert tools.tool_ask(state, "Choose", ["alpha", "beta"]) == "beta"
    rendered = tools.tool_todo(
        state,
        [{"id": "t1", "task": "ship it", "status": "in_progress"}],
    )
    assert state["todos"] == [{"id": "t1", "task": "ship it", "status": "in_progress"}]
    assert rendered == {"items": [{"id": "t1", "task": "ship it", "status": "in_progress"}], "count": 1}
    assert "count: 1" in shown[-1]
    with pytest.raises(tools.ValidationError):
        tools.tool_todo(state, [{"id": "t2", "task": "bad", "status": "wat"}])


def test_bash_tool_summarizes_json_and_text(monkeypatch, tmp_path):
    state = make_state(tmp_path)
    shown: list[str] = []
    results = iter(
        [
            SimpleNamespace(returncode=0, stdout='{"ok":true,"items":[1,2]}', stderr=""),
            SimpleNamespace(returncode=1, stdout="line1\nline2", stderr="boom"),
        ]
    )
    monkeypatch.setattr(rt, "note_tool", lambda *a, **k: None)
    monkeypatch.setattr(rt, "require_command_env", lambda root: {"PATH": ""})
    monkeypatch.setattr(rt, "which", lambda name, path=None: "/bin/bash")
    monkeypatch.setattr(rt, "run_cmd", lambda *a, **k: next(results))
    monkeypatch.setattr(rt, "show", shown.append)

    json_payload = tools.tool_bash(state, command="echo json")
    text_payload = tools.tool_bash(state, command="echo text")

    assert json_payload["returncode"] == 0
    assert '"ok":true' in json_payload["stdout"]
    assert json_payload["stderr"] == ""
    assert text_payload["returncode"] == 1
    assert text_payload["stdout"] == "line1\nline2"
    assert text_payload["stderr"] == "boom"
    assert any("returncode: 0" in text or "returncode: 1" in text for text in shown)


def test_file_tools_cover_listing_reading_search_replace_and_sloc(tmp_path, monkeypatch):
    (tmp_path / ".gitignore").write_text("ignored.txt\n", encoding="utf-8")
    (tmp_path / "a.txt").write_text("alpha\nbeta\n", encoding="utf-8")
    (tmp_path / "ignored.txt").write_text("alpha should stay hidden\n", encoding="utf-8")
    nested = tmp_path / "dir"
    nested.mkdir()
    (nested / "b.py").write_text("print('hello')\n", encoding="utf-8")
    with zipfile.ZipFile(tmp_path / "archive.zip", "w") as archive:
        archive.writestr("notes.txt", "no match here")

    state = make_state(tmp_path)
    monkeypatch.setattr(rt, "show", lambda *a, **k: None)
    monkeypatch.setattr(rt, "note_tool", lambda *a, **k: None)

    list_payload = tools.tool_list(state, "*")
    assert "a.txt" in list_payload["items"]
    assert "dir/" in list_payload["items"]
    assert list_payload["count"] >= 2
    read_payload = tools.tool_read(state, "a.txt", offset=2, limit=1)
    assert read_payload["text"] == "2: beta"
    assert read_payload["line_count"] == 2
    with pytest.raises(ValueError):
        tools.tool_read(state, "dir")

    search_payload = tools.tool_search(state, "alpha|print", ".", limit=20)
    found = {match["path"] for match in search_payload["matches"]}
    assert {"a.txt", "dir/b.py"} <= found
    assert "ignored.txt" not in found

    replace_payload = tools.tool_replace(state, "alpha", "ALPHA", ".", limit=20)
    assert replace_payload["changed_file_count"] == 1
    assert replace_payload["replacement_count"] == 1
    assert any(item["reason"] == "archive" for item in replace_payload.get("skipped", []))
    assert (tmp_path / "a.txt").read_text(encoding="utf-8").startswith("ALPHA")
    assert "alpha" in (tmp_path / "ignored.txt").read_text(encoding="utf-8")

    sloc_payload = tools.tool_sloc(state, ".", limit=20)
    assert sloc_payload["total_code_count"] > 0
    assert any(language["language"] == "Python" for language in sloc_payload["languages"])


def test_webfetch_validation_markdown_redaction_and_error_payloads(monkeypatch, tmp_path):
    with pytest.raises(ValueError):
        tools._validate_url_safe("file:///etc/passwd")
    with pytest.raises(ValueError):
        tools._validate_url_safe("http://localhost/secret")
    with pytest.raises(ValueError):
        tools._validate_url_safe("http://127.0.0.1/secret")
    with pytest.raises(ValueError):
        tools._validate_url_safe("http://169.254.169.254/latest/meta-data/")

    state = make_state(tmp_path)
    shown: list[str] = []
    html = "<html><body><h1>Title</h1>" + "".join(
        f"<p>Paragraph {i} with <a href='/doc/{i}'>link {i}</a>.</p>" for i in range(20)
    ) + "</body></html>"
    response = providers.response_adapter(
        status_code=200,
        headers={
            "Content-Type": "text/html; charset=utf-8",
            "Location": "https://secret.example/next",
            "Set-Cookie": "session=secret",
        },
        text=html,
        content=html.encode("utf-8"),
        url="https://example.com/page",
        reason_phrase="OK",
    )

    monkeypatch.setattr(tools, "_validate_url_safe", lambda url: url)
    monkeypatch.setattr(rt, "BUDGETS", rt.runtime_budgets(
        message_tokens=64,
        tool_output_tokens=40,
        tool_tail_tokens=10,
        default_line_limit=20,
    ))
    monkeypatch.setattr(rt, "show", shown.append)
    monkeypatch.setattr(rt, "tool_session", lambda **kw: DummyHttpClient(response=response, **kw))

    payload = tools.tool_webfetch(state, url="https://example.com/page")
    assert payload["format"] == "markdown"
    assert "Title\n=====" in payload["text"]
    assert payload["headers"]["Location"] == "<redacted>"
    assert payload["headers"]["Set-Cookie"] == "<redacted>"

    monkeypatch.setattr(
        rt,
        "tool_session",
        lambda **kw: DummyHttpClient(error=providers.TransportError("boom"), **kw),
    )
    error_payload = tools.tool_webfetch(state, url="https://example.com/page")
    assert error_payload == {
        "method": "GET",
        "url": "https://example.com/page",
        "ok": False,
        "error_type": "TransportError",
        "message": "boom",
    }
    assert any("boom" in text for text in shown)

    with pytest.raises(ValueError):
        tools.tool_webfetch(state, url="https://example.com/page", method="POST")
    with pytest.raises(ValueError):
        tools.tool_webfetch(
            state,
            url="https://example.com/page",
            headers={"Authorization": "secret"},
        )


def test_cli_load_transcript_and_chat_commands_use_dict_transcripts(tmp_path, monkeypatch):
    monkeypatch.setattr(cli, "_SESSIONS_DIR", tmp_path)
    monkeypatch.setattr(rt, "_note", lambda *a, **k: None)
    monkeypatch.setattr(rt, "_print", lambda *a, **k: None)

    saved = {
        "model": "openai:gpt-test",
        "saved_at": "2026-03-25T12:34:56",
        "transcript": cli._transcript_data(
            agent.transcript(messages=[SystemMessage("old"), UserMessage("hello")])
        ),
    }
    (tmp_path / "saved.json").write_text(json.dumps(saved), encoding="utf-8")

    loaded, model = cli._handle_load(
        "saved",
        agent.transcript_with_system_prompt("sys"),
        "openai:gpt-old",
        "new system",
    )
    assert model == "openai:gpt-test"
    assert loaded["messages"] == [SystemMessage("new system"), UserMessage("hello")]
    assert cli._chat_command("/yolo", loaded, "new system", model) == ("yolo",)
    assert cli._chat_command("/clear", loaded, "new system", model) is True
    assert loaded["messages"] == [SystemMessage("new system")]


def test_chat_rolls_back_with_checkpoint_helper_on_agent_error(tmp_path, monkeypatch):
    inputs = iter(["hello", "quit"])
    rollback_calls = []
    errors = []

    monkeypatch.setattr(cli, "_create_prompt_session", lambda: object())
    monkeypatch.setattr(
        cli,
        "resolve_session",
        lambda **kwargs: {
            "workspace": tmp_path,
            "model": "openai:gpt-test",
            "interactive": True,
            "system_prompt": "sys",
            "system_file": None,
            "yolo": False,
        },
    )
    monkeypatch.setattr(cli, "_print_session_intro", lambda *a, **k: None)
    monkeypatch.setattr(cli, "_set_terminal_title", lambda *a, **k: None)
    monkeypatch.setattr(cli, "_read_input", lambda *a, **k: next(inputs))
    monkeypatch.setattr(cli, "checkpoint", lambda tx: 7)
    monkeypatch.setattr(cli, "rollback", lambda tx, point: rollback_calls.append(point))
    monkeypatch.setattr(cli, "run_agent", lambda *a, **k: (_ for _ in ()).throw(RuntimeError("boom")))
    monkeypatch.setattr(rt, "print_console", lambda *a, **k: None)
    monkeypatch.setattr(rt, "rule_console", lambda *a, **k: None)
    monkeypatch.setattr(rt, "_note", lambda *a, **k: None)
    monkeypatch.setattr(rt, "_error", errors.append)

    assert cli.chat() == 0
    assert rollback_calls == [7]
    assert errors == ["Agent error: boom"]



def test_cli_main_wraps_bare_args(monkeypatch):
    seen = {}

    def fake_run(functions, *, argv, **kwargs):
        seen["argv"] = argv
        return 0

    monkeypatch.setattr(cli.defopt, "run", fake_run)
    assert cli.main(["fix", "tests"]) == 0
    assert seen["argv"] == ["run", "fix", "tests"]


def test_cli_main_rejects_top_level_yolo(monkeypatch):
    monkeypatch.delenv("OY_YOLO", raising=False)

    with pytest.raises(SystemExit):
        cli.main(["--yolo", "fix", "tests"])

    assert rt.yolo_enabled() is False


def test_run_turn_executes_tool_calls_until_final_answer(monkeypatch, tmp_path):
    def echo(state, text: str):
        return f"{state['root'].name}:{text}"

    registry = tool_handler("echo", echo)
    transcript = agent.transcript_with_system_prompt("sys")
    agent.add_user(transcript, "hello")
    responses = iter(
        [
            AssistantMessage("", tool_calls=[ToolCall(id="call_1", name="echo", arguments={"text": "hi"})]),
            AssistantMessage("done"),
        ]
    )
    printed: list[str] = []
    monkeypatch.setattr(rt, "_print", lambda *a, value="", **k: printed.append(value))
    monkeypatch.setattr(rt, "_debug_log", lambda *a, **k: None)
    monkeypatch.setattr(rt, "_note", lambda *a, **k: None)

    client = {
        "chat_completion": lambda **kwargs: next(responses),
    }
    code, content = agent.run_turn(
        client,
        transcript,
        make_state(tmp_path, registry=registry),
        "openai:gpt-test",
        tools.tool_specs(registry),
    )

    assert (code, content) == (0, "done")
    assert printed == ["done"]
    assert transcript["messages"][2] == AssistantMessage(
        "", tool_calls=[ToolCall(id="call_1", name="echo", arguments={"text": "hi"})]
    )
    assert transcript["messages"][3] == ToolMessage(
        "call_1",
        "echo",
        ToolResult(content=f"{tmp_path.name}:hi"),
    )
