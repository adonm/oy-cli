"""Tests for runtime module: config, display, prompts."""
from __future__ import annotations

from types import SimpleNamespace

import pytest

from oy_cli import runtime as rt


class TestSessionText:
    def test_guidance_mentions_exclude_and_todo_requirements(self):
        rt.load_session_text.cache_clear()
        assert "Never guess" in rt.base_system_prompt()
        assert "`webfetch` freely" in rt.base_system_prompt()
        assert "exclude" in rt.tool_description("list")
        assert "exclude" in rt.tool_description("search")
        assert "exclude" in rt.tool_description("replace")
        assert "exclude" in rt.tool_description("sloc")
        assert "broad browsing" in rt.tool_description("webfetch")
        assert "Every item must include string `id` and string `task`" in rt.tool_description("todo")


class TestModelConfig:
    def test_round_trip(self, tmp_path, monkeypatch):
        monkeypatch.setenv("OY_CONFIG", str(tmp_path / "config.json"))
        monkeypatch.delenv("OY_MODEL", raising=False)
        monkeypatch.delenv("OY_SHIM", raising=False)

        assert rt.save_model_config("openai:gpt-test") == {"model": "gpt-test", "shim": "openai"}
        assert rt.load_model_config() == {"model": "gpt-test", "shim": "openai"}
        assert rt._model(None) == "openai:gpt-test"

    def test_env_vars_override(self, tmp_path, monkeypatch):
        monkeypatch.setenv("OY_CONFIG", str(tmp_path / "config.json"))
        monkeypatch.setenv("OY_SHIM", "copilot")
        monkeypatch.setenv("OY_MODEL", "gpt-live")
        monkeypatch.setenv("OY_YOLO", "yes")
        assert rt._model(None) == "copilot:gpt-live"
        assert rt.yolo_enabled() is True
        assert "ask" not in rt.active_tool_registry(False)
        assert set(rt.read_only_tool_registry()) == rt._READ_ONLY_TOOLS


class TestDurationEnvHelpers:
    def test_unattended_limit_prefers_new_name(self, monkeypatch):
        monkeypatch.setenv("OY_UNATTENDED_LIMIT", "30m")
        assert rt.unattended_limit_seconds() == 1800

    def test_unattended_limit_rejects_invalid_value(self, monkeypatch):
        monkeypatch.setenv("OY_UNATTENDED_LIMIT", "bad")
        with pytest.raises(SystemExit):
            rt.unattended_limit_seconds()

    def test_unattended_limit_default_is_one_hour(self, monkeypatch):
        monkeypatch.delenv("OY_UNATTENDED_LIMIT", raising=False)
        assert rt.unattended_limit_seconds() == rt.DEFAULT_UNATTENDED_LIMIT_SECONDS


class TestDisplayHelpers:
    def test_render_search_preview_text_uses_text_color(self):
        rendered = rt._render_search_preview_text("pre ⟦match⟧ post")
        assert rendered.plain == "pre match post"
        spans = [span for span in rendered.spans if rendered.plain[span.start:span.end] == "match"]
        assert spans
        assert all(span.style == "bold yellow" for span in spans)
        assert all(" on " not in span.style for span in spans)

    def test_render_preview_text_highlights_structured_output(self):
        rendered = rt._render_preview_text(
            "\n".join([
                "path: src/demo.py",
                'stdout: "line1\nline2"',
                "text.python: print('ok')",
                "items[2]{id,task,status}:",
                '  "1",ship it,done',
                "src/demo.py:12:7:print(⟦'ok'⟧)",
                "skip: file.txt — archive",
                "change: file.txt — 3 replacements",
                "... [4 more matches omitted]",
            ])
        )
        assert "path:" in rendered
        assert "line1" in rendered and "line2" in rendered
        assert "text.python:" in rendered and "print('ok')" in rendered
        assert "skip" in rendered and "file.txt" in rendered
        assert "change" in rendered and "3" in rendered and "replacement" in rendered

    def test_show_truncates_preview(self, monkeypatch):
        rendered: list[str] = []
        monkeypatch.setattr(rt, "print_console", lambda console, *values, **kwargs: rendered.extend(map(str, values)))
        rt.show("\n".join(f"line {i}" for i in range(40)))
        assert rendered
        assert "line 0" in rendered[-1]
        assert "line 39" in rendered[-1]
        assert "... [20 lines hidden]" in rendered[-1]
        assert "... [40 lines total]" in rendered[-1]


class TestListModels:
    def test_warns_and_keeps_other_shims(self, monkeypatch, tmp_path):
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
