"""Tests for runtime module: config, display, prompts."""

from __future__ import annotations

from types import SimpleNamespace

import pytest

from oy_cli import runtime as rt
from tests.conftest import patch_runtime


class TestSessionText:
    def test_guidance_mentions_exclude_and_todo_requirements(self):
        rt.load_session_text.cache_clear()
        assert "Never guess" in rt.base_system_prompt()
        assert "Follow the user's output constraints exactly" in rt.base_system_prompt()
        assert "Use `webfetch` freely" in rt.base_system_prompt()
        assert rt.active_system_prompt(True).startswith(rt.BASE_SYSTEM_PROMPT)
        assert rt.active_system_prompt(False).startswith(rt.BASE_SYSTEM_PROMPT)
        assert "no-write rather than no-network" in rt.ask_system_prompt("sys")
        audit_prompt = rt.audit_system_prompt()
        assert "Renovate lookup report command" in audit_prompt
        assert "pnpm dlx --allow-build=re2 renovate" in audit_prompt
        assert "npm exec --yes --package renovate -- renovate" in audit_prompt
        assert "--dry-run=lookup" in audit_prompt
        assert "--report-path=renovate-report.json" in audit_prompt
        assert "throwaway local artifact" in audit_prompt
        assert "delete it or leave it untracked" in audit_prompt
        assert "`jq` when available or Python otherwise" in audit_prompt
        for name in ("list", "search", "replace", "sloc"):
            assert "exclude" in rt.tool_description(name)
        assert "public web research" in rt.tool_description("webfetch")
        assert (
            "Every item must include string `id`, string `task`, and a valid `status`"
            in rt.tool_description("todo")
        )


class TestModelConfig:
    def test_round_trip(self, tmp_path, monkeypatch):
        monkeypatch.setenv("OY_CONFIG", str(tmp_path / "config.json"))
        monkeypatch.delenv("OY_MODEL", raising=False)
        monkeypatch.delenv("OY_SHIM", raising=False)

        assert rt.save_model_config("openai:gpt-test") == {
            "model": "gpt-test",
            "shim": "openai",
        }
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

    def test_unknown_saved_shim_falls_back_to_picker(self, tmp_path, monkeypatch):
        monkeypatch.setenv("OY_CONFIG", str(tmp_path / "config.json"))
        monkeypatch.delenv("OY_MODEL", raising=False)
        monkeypatch.delenv("OY_SHIM", raising=False)
        (tmp_path / "config.json").write_text(
            '{"model":"gpt-stale","shim":"other-shim"}', encoding="utf-8"
        )
        monkeypatch.setattr(rt, "_pick_model", lambda: "local-8080:qwen3.5")
        assert rt.load_model_config() == {"model": None, "shim": None}
        assert rt._model(None) == "local-8080:qwen3.5"

    def test_unknown_env_shim_falls_back_to_bare_env_model(self, tmp_path, monkeypatch):
        monkeypatch.setenv("OY_CONFIG", str(tmp_path / "config.json"))
        monkeypatch.setenv("OY_SHIM", "other-shim")
        monkeypatch.setenv("OY_MODEL", "gpt-live")
        assert rt._model(None) == "gpt-live"

    def test_env_model_with_colon_keeps_configured_local_shim(self, tmp_path, monkeypatch):
        monkeypatch.setenv("OY_CONFIG", str(tmp_path / "config.json"))
        monkeypatch.setenv("OY_SHIM", "local-8080")
        monkeypatch.setenv("OY_MODEL", "unsloth/gemma-4-31B-it-GGUF:UD-Q4_K_XL")
        assert rt._model(None) == "unsloth/gemma-4-31B-it-GGUF:UD-Q4_K_XL"


class TestDurationEnvHelpers:
    @pytest.mark.parametrize(
        ("name", "call"),
        [
            ("OY_UNATTENDED_LIMIT", rt.unattended_limit_seconds),
            ("OY_RALPH_LIMIT", rt.ralph_limit_seconds),
        ],
    )
    def test_invalid_env_vars_raise_system_exit(self, monkeypatch, name, call):
        monkeypatch.setenv(name, "bad")
        with pytest.raises(SystemExit):
            call()

    def test_unattended_limit_prefers_new_name(self, monkeypatch):
        monkeypatch.setenv("OY_UNATTENDED_LIMIT", "30m")
        assert rt.unattended_limit_seconds() == 1800

    def test_unattended_limit_default_is_one_hour(self, monkeypatch):
        monkeypatch.delenv("OY_UNATTENDED_LIMIT", raising=False)
        assert rt.unattended_limit_seconds() == rt.DEFAULT_UNATTENDED_LIMIT_SECONDS


class TestModelListRendering:
    def test_render_model_list_mentions_local_defaults(self, monkeypatch):
        printed = []
        patch_runtime(
            monkeypatch,
            _print=lambda *a, **k: printed.append(k.get("value", a[1] if len(a) > 1 else a[0] if a else "")),
        )
        rt.render_model_list(["openai:gpt-test"], title="## Available Models", err=True)
        assert "local-8080" in printed[-1]
        assert "http://127.0.0.1:11434/v1" in printed[-1]

class TestDisplayHelpers:
    def test_render_search_preview_text_uses_text_color(self):
        rendered = rt._render_search_preview_text("pre ⟦match⟧ post")
        assert rendered.plain == "pre match post"
        spans = [
            span
            for span in rendered.spans
            if rendered.plain[span.start : span.end] == "match"
        ]
        assert spans
        assert all(span.style == "bold yellow" for span in spans)
        assert all(" on " not in span.style for span in spans)

    def test_render_preview_text_highlights_structured_output(self):
        rendered = rt._render_preview_text(
            "\n".join(
                [
                    "path: src/demo.py",
                    'stdout: "line1\nline2"',
                    "text.python: print('ok')",
                    "items[2]{id,task,status}:",
                    '  "1",ship it,done',
                    "src/demo.py:12:7:print(⟦'ok'⟧)",
                    "skip: file.txt — archive",
                    "change: file.txt — 3 replacements",
                    "... [4 more matches omitted]",
                ]
            )
        )
        assert "path:" in rendered
        assert "line1" in rendered and "line2" in rendered
        assert "text.python:" in rendered and "print('ok')" in rendered
        assert "skip" in rendered and "file.txt" in rendered
        assert "change" in rendered and "3" in rendered and "replacement" in rendered

    def test_show_truncates_preview(self, monkeypatch):
        rendered: list[str] = []
        patch_runtime(
            monkeypatch,
            print_console=lambda console, *values, **kwargs: rendered.extend(
                map(str, values)
            ),
        )
        rt.show("\n".join(f"line {i}" for i in range(40)))
        assert rendered
        assert "line 0" in rendered[-1]
        assert "line 39" in rendered[-1]
        assert "... [20 lines hidden]" in rendered[-1]
        assert "... [40 lines total]" in rendered[-1]


class TestListModels:
    def test_dedupes_models_from_multiple_shims(self, monkeypatch, tmp_path):
        monkeypatch.setattr(rt, "detect_available_shims", lambda: ["alpha", "beta"])
        monkeypatch.setattr(
            rt,
            "list_models_for_shim",
            lambda shim, cwd=None: ["shared:model", f"{shim}:only"],
        )
        monkeypatch.setattr(rt, "Path", SimpleNamespace(cwd=lambda: tmp_path))
        assert rt.list_all_model_ids() == ["shared:model", "alpha:only", "beta:only"]

    def test_warns_and_keeps_other_shims(self, monkeypatch, tmp_path):
        printed: list[tuple[str, str, bool]] = []
        monkeypatch.setattr(rt, "detect_available_shims", lambda: ["alpha", "beta"])

        def fake_list_models_for_shim(shim, cwd=None):
            assert cwd == tmp_path
            if shim == "alpha":
                return ["alpha:demo"]
            raise RuntimeError("boom\nsecond line")

        monkeypatch.setattr(rt, "list_models_for_shim", fake_list_models_for_shim)
        monkeypatch.setattr(
            rt,
            "_print",
            lambda kind="md", value="", err=False, extra=None: printed.append(
                (kind, value, err)
            ),
        )
        monkeypatch.setattr(rt, "Path", SimpleNamespace(cwd=lambda: tmp_path))

        with pytest.raises(RuntimeError, match="Could not load models from `beta`: boom"):
            rt.list_all_model_ids()
        assert printed == [
            ("status", "Loading models from `alpha`.", True),
            ("status", "Loading models from `beta`.", True),
        ]
