"""Tests for tools module: file ops, bash, webfetch, search/replace/sloc."""
from __future__ import annotations

import zipfile
from types import SimpleNamespace

import pytest

from oy_cli import providers, runtime as rt, tools
from tests.conftest import DummyHttpClient, make_state, tool_handler


class TestToolSchemas:
    def test_closed_object_schemas(self):
        specs = {tool["name"]: tool for tool in tools.tool_specs()}
        assert specs["todo"]["parameters"]["additionalProperties"] is False
        assert specs["todo"]["parameters"]["properties"]["todos"]["items"]["additionalProperties"] is False
        assert "exclude" in specs["list"]["parameters"]["properties"]
        assert "exclude" in specs["search"]["parameters"]["properties"]
        assert "exclude" in specs["replace"]["parameters"]["properties"]
        assert "exclude" in specs["sloc"]["parameters"]["properties"]


class TestToolApproval:
    def test_validates_arguments_and_respects_approval_modes(self, monkeypatch, tmp_path):
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


class TestAskTodoTools:
    def test_ask_and_todo_update_state(self, monkeypatch, tmp_path):
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
            state, [{"id": "t1", "task": "ship it", "status": "in_progress"}],
        )
        assert state["todos"] == [{"id": "t1", "task": "ship it", "status": "in_progress"}]
        assert rendered == {"items": [{"id": "t1", "task": "ship it", "status": "in_progress"}], "count": 1}
        assert shown[-1] == "count: 1\ntodos:\n  [~] t1: ship it"

        with pytest.raises(tools.ValidationError):
            tools.tool_todo(state, [{"id": "t2", "task": "bad", "status": "wat"}])


class TestBashTool:
    def test_summarizes_json_and_text(self, monkeypatch, tmp_path):
        state = make_state(tmp_path)
        shown: list[str] = []
        results = iter([
            SimpleNamespace(returncode=0, stdout='{"ok":true,"items":[1,2]}', stderr=""),
            SimpleNamespace(returncode=1, stdout="line1\nline2", stderr="boom"),
        ])
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
        assert any("$ echo json" in text and "exit: 0" in text for text in shown)
        assert any("stdout:" in text and '"ok":true' in text for text in shown)


class TestFileTools:
    def test_list_read_search_replace_sloc(self, tmp_path, monkeypatch):
        (tmp_path / ".gitignore").write_text("ignored.txt\n", encoding="utf-8")
        (tmp_path / "a.txt").write_text("alpha\nbeta\n", encoding="utf-8")
        (tmp_path / "ignored.txt").write_text("alpha should stay hidden\n", encoding="utf-8")
        nested = tmp_path / "dir"
        nested.mkdir()
        (nested / "b.py").write_text("print('hello')\n", encoding="utf-8")
        (tmp_path / "docs").mkdir()
        (tmp_path / "docs" / "banana.txt").write_text("banana hidden\n", encoding="utf-8")
        with zipfile.ZipFile(tmp_path / "archive.zip", "w") as archive:
            archive.writestr("notes.txt", "hello from archive")

        state = make_state(tmp_path)
        monkeypatch.setattr(rt, "show", lambda *a, **k: None)
        monkeypatch.setattr(rt, "note_tool", lambda *a, **k: None)

        # List
        list_payload = tools.tool_list(state, "*")
        assert "a.txt" in list_payload["items"]
        assert "dir/" in list_payload["items"]
        assert list_payload["count"] >= 2

        # List with exclude
        list_excluded = tools.tool_list(state, "*", exclude=["dir/**", "archive.zip"])
        assert list_excluded["exclude"] == ["dir/**", "archive.zip"]
        assert "dir/" not in list_excluded["items"]
        assert "archive.zip" not in list_excluded["items"]

        shown: list[str] = []
        monkeypatch.setattr(rt, "show", shown.append)

        # Read
        read_payload = tools.tool_read(state, "a.txt", offset=2, limit=1)
        assert read_payload["text"] == "beta"
        assert read_payload["line_count"] == 2
        assert shown[-1] == "path: a.txt\nlines: 2-2 of 2\ntext: beta"

        shown.clear()
        python_read = tools.tool_read(state, "dir/b.py", offset=1, limit=1)
        assert python_read["text"] == "print('hello')"
        assert shown[-1] == "path: dir/b.py\nlines: 1-1 of 1\ntext.python: print('hello')"

        shown.clear()
        beyond_end = tools.tool_read(state, "a.txt", offset=5, limit=1)
        assert beyond_end["text"] == ""
        assert beyond_end["line_count"] == 2
        assert beyond_end["truncated"] is False
        assert shown[-1] == "path: a.txt\nlines: 5-4 of 2\n<empty file>"

        with pytest.raises(ValueError):
            tools.tool_read(state, "dir")

        # Search
        search_payload = tools.tool_search(state, "alpha|hello", ".", limit=20)
        found = {match["path"]: match for match in search_payload["matches"]}
        assert {"a.txt", "dir/b.py"} <= set(found)
        assert "ignored.txt" not in found

        # Search with exclude
        excluded_payload = tools.tool_search(state, "alpha|hello", ".", exclude=["dir/**", "*.zip"], limit=20)
        excluded_found = {match["path"]: match for match in excluded_payload["matches"]}
        assert excluded_payload["exclude"] == ["dir/**", "*.zip"]
        assert "a.txt" in excluded_found
        assert "dir/b.py" not in excluded_found

        # Fuzzy search
        (tmp_path / "fuzzy.txt").write_text("hallo\ncat and dog\n", encoding="utf-8")
        shown.clear()
        fuzzy_payload = tools.tool_search(state, "hello", ".", fuzzy="s<=1", limit=20)
        fuzzy_found = {match["path"]: match for match in fuzzy_payload["matches"]}
        assert fuzzy_found["fuzzy.txt"]["column"] == 1
        assert fuzzy_found["fuzzy.txt"]["text"] == "hallo"

        # Replace
        replace_payload = tools.tool_replace(state, "alpha", "ALPHA", ".", limit=20)
        assert replace_payload["changed_file_count"] == 1

        (tmp_path / "skip.txt").write_text("alpha skip\n", encoding="utf-8")
        replace_excluded = tools.tool_replace(state, "alpha", "OMEGA", ".", exclude=["skip.txt"], limit=20)
        assert replace_excluded["changed_file_count"] == 0
        assert replace_excluded["exclude"] == ["skip.txt"]

        # Sloc
        sloc_payload = tools.tool_sloc(state, ".", limit=20)
        assert sloc_payload["total_code_count"] > 0
        assert any(lang["language"] == "Python" for lang in sloc_payload["languages"])
        assert sloc_payload["top_file_count"] == 5

    def test_sloc_top_20_default(self, tmp_path, monkeypatch):
        for i in range(25):
            source = "\n".join(f"v_{i}_{line} = {line}" for line in range(i + 1)) + "\n"
            (tmp_path / f"file_{i:02d}.py").write_text(source, encoding="utf-8")

        state = make_state(tmp_path)
        monkeypatch.setattr(rt, "show", lambda *a, **k: None)
        monkeypatch.setattr(rt, "note_tool", lambda *a, **k: None)

        sloc_payload = tools.tool_sloc(state, ".", limit=3)
        assert sloc_payload["top_file_count"] == 25
        assert len(sloc_payload["top_files"]) == 20
        assert sloc_payload["truncated"] is True


class TestWebfetch:
    def test_url_validation_blocks_private_ips(self):
        with pytest.raises(ValueError):
            tools._validate_url_safe("file:///etc/passwd")
        with pytest.raises(ValueError):
            tools._validate_url_safe("http://localhost/secret")
        with pytest.raises(ValueError):
            tools._validate_url_safe("http://127.0.0.1/secret")
        with pytest.raises(ValueError):
            tools._validate_url_safe("http://169.254.169.254/latest/meta-data/")

    def test_webfetch_html_to_markdown_and_redaction(self, monkeypatch, tmp_path):
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
            message_tokens=64, tool_output_tokens=40, tool_tail_tokens=10, default_line_limit=20,
        ))
        monkeypatch.setattr(rt, "show", shown.append)
        monkeypatch.setattr(rt, "tool_session", lambda **kw: DummyHttpClient(response=response, **kw))

        payload = tools.tool_webfetch(state, url="https://example.com/page")
        assert payload["format"] == "markdown"
        assert "Title\n=====" in payload["text"]
        assert payload["headers"]["Location"] == "<redacted>"
        assert payload["headers"]["Set-Cookie"] == "<redacted>"

    def test_webfetch_error_payload(self, monkeypatch, tmp_path):
        state = make_state(tmp_path)
        shown: list[str] = []

        monkeypatch.setattr(tools, "_validate_url_safe", lambda url: url)
        monkeypatch.setattr(rt, "show", shown.append)
        monkeypatch.setattr(rt, "tool_session", lambda **kw: DummyHttpClient(error=providers.TransportError("boom"), **kw))

        error_payload = tools.tool_webfetch(state, url="https://example.com/page")
        assert error_payload == {
            "method": "GET",
            "url": "https://example.com/page",
            "ok": False,
            "error_type": "TransportError",
            "message": "boom",
        }

    def test_webfetch_restricted_methods_and_headers(self, monkeypatch, tmp_path):
        state = make_state(tmp_path)
        monkeypatch.setattr(tools, "_validate_url_safe", lambda url: url)
        monkeypatch.setattr(rt, "tool_session", lambda **kw: DummyHttpClient())

        with pytest.raises(ValueError):
            tools.tool_webfetch(state, url="https://example.com/page", method="POST")
        with pytest.raises(ValueError):
            tools.tool_webfetch(state, url="https://example.com/page", headers={"Authorization": "secret"})
