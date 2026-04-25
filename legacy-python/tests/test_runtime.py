"""Tests for runtime module: config, display, prompts."""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace
import tomllib

import pytest

from oy_cli import runtime as rt
from tests.conftest import patch_runtime


class TestSessionText:
    def test_guidance_mentions_exclude_and_todo_requirements(self):
        rt.load_session_text.cache_clear()
        assert "Inspect files, code, commands, and repo state first" in rt.base_system_prompt()
        assert "Follow the user's output constraints exactly" in rt.base_system_prompt()
        assert "Use `webfetch` freely" in rt.base_system_prompt()
        assert rt.active_system_prompt(True).startswith(rt.BASE_SYSTEM_PROMPT)
        assert rt.active_system_prompt(False).startswith(rt.BASE_SYSTEM_PROMPT)
        assert rt.audit_settings()["phase2_workers"] == max((__import__("os").cpu_count() or 1) * 4, 1)
        assert rt.audit_settings()["phase2_launch_delay_seconds"] == 10
        assert "Stay no-write: leave files unchanged, skip `bash`, and keep `webfetch` available" in rt.ask_system_prompt("sys")
        audit_prompt = rt.audit_system_prompt()
        logic_audit_prompt = rt.logic_audit_system_prompt()
        phase1_prompt = rt.audit_system_prompt(phase="phase1")
        phase2_prompt = rt.audit_system_prompt(phase="phase2")
        phase3_prompt = rt.audit_system_prompt(phase="phase3")
        assert ".tmp/renovate-*.json" not in audit_prompt
        assert "LOGIC-FOCUSED audit mode" in logic_audit_prompt
        assert "phase1 should skip docs and lockfiles from the backlog" in logic_audit_prompt
        assert "comments and docstrings stripped where possible" in logic_audit_prompt
        assert "session dir" in audit_prompt
        assert rt.session_text("audit", "report_title") == "# Audit Issues"
        assert rt.session_text("audit", "inbox_title") == "Inbox"
        assert "setup only" in phase1_prompt
        assert "review plan and chunking are handled before you start" in phase1_prompt
        assert "normalized into the inbox layout" in phase1_prompt
        assert ".tmp/renovate-*.json" in phase1_prompt
        assert "oy renovate-local" in phase1_prompt
        assert "review only the provided chunk" in phase2_prompt
        assert "use `search` for repo lookups" in phase2_prompt
        assert "AUDIT PHASE1" in phase1_prompt
        assert "AUDIT PHASE2" in phase2_prompt
        assert "AUDIT PHASE3" in phase3_prompt
        assert "use `inbox_append` to add candidate findings" in phase2_prompt
        assert "Each chunk must leave a durable inbox update in `ISSUES.md`" in phase2_prompt
        assert "do not rewrite, merge, dedupe, or reorder old findings" in phase2_prompt
        assert "final rewrite" in phase3_prompt
        assert "rewriting `ISSUES.md` is expected" in phase3_prompt
        assert "durable inbox update in `ISSUES.md`" in phase2_prompt
        assert "Consume the inbox and rewrite `ISSUES.md` into the final report" in phase3_prompt
        assert "rewriting `ISSUES.md` is expected" in phase3_prompt
        assert "oy renovate-local" not in audit_prompt
        assert "Skip cleanly when no report is present" not in audit_prompt
        assert "Skip cleanly when no report is present" in phase1_prompt
        assert "Use the audit prompt's built-in ASVS, MASVS, and grugbrain summary" in audit_prompt
        reference = rt.session_text("audit", "reference_suffix")
        assert "OWASP ASVS quick map for web, API, and backend repos" in reference
        assert "OWASP MASVS quick map for mobile repos only" in reference
        assert "Grugbrain context and complexity filter" in reference
        assert "v5.0.0-<chapter.section.requirement>" in reference
        assert "`V1` Encoding and Sanitization" in reference
        assert "`V8` Authorization" in reference
        assert "`MASVS-STORAGE`" in reference
        assert "`MASVS-AUTH`" in reference
        assert "`MASWE`" in reference
        assert "https://owasp.org/www-project-application-security-verification-standard/" in reference
        assert "https://mas.owasp.org/MASVS/" in reference
        assert "https://grugbrain.dev/" in reference
        assert "How to use this block during audit" in reference
        assert "OWASP ASVS context" in reference
        assert "ASVS citation hints by finding type" in reference
        assert "OWASP MAS ecosystem context for mobile repos" in reference
        assert "Grugbrain context and complexity filter" in reference
        assert "Practical audit heuristics combining both lenses" in reference
        assert "small sharp tools" in reference
        assert "reproduce bug first" in reference
        assert "small focused unit tests around invariants" in reference
        assert "thin layer of high-value end-to-end coverage" in reference
        assert "authz boundaries" in reference
        assert "Grugbrain has no formal section IDs" in reference
        assert "`complexity very bad`" in reference
        assert "`testing security boundaries`" in reference
        assert "renovate-report.json" not in audit_prompt
        assert "pnpm dlx --allow-build=re2 renovate" not in audit_prompt
        assert "npm exec --yes --package renovate -- renovate" not in audit_prompt
        for name in ("list", "search", "replace", "sloc"):
            assert "exclude" in rt.tool_description(name)
        assert "public web research" in rt.tool_description("webfetch")
        assert (
            "Every item must include string `id`, string `task`, and a valid `status`"
            in rt.tool_description("todo")
        )

    def test_audit_settings_use_more_of_large_context_budget(self):
        tuned = rt.audit_settings(context_tokens=128_000)
        assert tuned["review_chunk_target_tokens"] == 64_000
        assert tuned["review_chunk_max_files"] == 64

    def test_audit_settings_scale_with_context_budget(self):
        small = rt.audit_settings(context_tokens=32_000)
        large = rt.audit_settings(context_tokens=262_144)
        assert small["review_chunk_target_tokens"] == 64_000
        assert large["review_chunk_target_tokens"] == 64_000
        assert small["review_chunk_max_files"] == 64
        assert large["review_chunk_max_files"] == 64
        assert small["report_context_limit"] == 64
        assert large["report_context_limit"] == 64

    def test_session_context_carries_max_context_tokens(self, tmp_path):
        session = rt.session_context(
            workspace=tmp_path,
            model="openai:gpt-test",
            interactive=False,
            system_prompt="sys",
            max_context_tokens=77777,
        )
        assert session["max_context_tokens"] == 77777


class TestPackagingMetadata:
    def test_project_scripts_expose_oy_and_oy_cli(self):
        data = tomllib.loads(Path("pyproject.toml").read_text(encoding="utf-8"))
        assert data["project"]["scripts"] == {
            "oy": "oy_cli.cli:main",
            "oy-cli": "oy_cli.cli:main",
        }


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
        assert "audit_todo" not in rt.active_tool_registry(False)
        assert set(rt.read_only_tool_registry()) == rt._READ_ONLY_TOOLS

    def test_agent_profiles_map_to_expected_modes(self, tmp_path, monkeypatch):
        monkeypatch.setenv("OY_CONFIG", str(tmp_path / "config.json"))
        assert rt.list_agent_profiles() == [
            "default",
            "plan",
            "accept-edits",
            "auto-approve",
        ]
        assert rt.normalize_agent_profile("accept_edits") == "accept-edits"
        assert rt.agent_profile("plan")["tool_mode"] == "read_only"
        assert rt.agent_profile("accept-edits")["auto_approve_edits"] is True
        assert rt.agent_profile("auto-approve")["yolo"] is True
        assert set(rt.active_tool_registry(True, agent="plan")) == rt._READ_ONLY_TOOLS

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
                    "path: src/demo.py",
                    "match: 12:7:python:print(⟦\'ok\'⟧)",
                    "skip: file.txt — archive",
                    "change: file.txt — 3 replacements",
                    "... [4 more matches omitted]",
                ]
            )
        )
        assert "path:" in rendered
        assert "line1" in rendered and "line2" in rendered
        assert "text.python:" in rendered and "print('ok')" in rendered
        assert "src/demo.py" in rendered and "12" in rendered and "7" in rendered
        assert "match" in rendered and "python" in rendered
        assert "skip" in rendered and "file.txt" in rendered
        assert "change" in rendered and "3" in rendered and "replacement" in rendered


    def test_render_preview_text_renders_raw_multiline_read_output_like_bat(self):
        rendered = rt._render_preview_text(
            "\n".join(
                [
                    "path: dir/demo.py",
                    "lines: 5-7 of 20",
                    "text.python: print('a')",
                    '  print("b")',
                    "  print(3)",
                ]
            )
        )
        assert "dir/demo.py" in rendered
        assert "[demo.py]" in rendered and "[python]" in rendered
        assert "5-7 of 20" in rendered
        assert "5 print('a')" in rendered
        assert '6 print("b")' in rendered
        assert "7 print(3)" in rendered

    def test_show_truncates_preview(self, monkeypatch):
        rendered: list[str] = []
        patch_runtime(
            monkeypatch,
            print_console=lambda _console, *values, **_kwargs: rendered.extend(
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
        def fake_list_models_for_shim(shim, cwd=None):
            assert cwd == tmp_path
            return ["shared:model", f"{shim}:only"]

        monkeypatch.setattr(rt, "list_models_for_shim", fake_list_models_for_shim)
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
            lambda kind="md", value="", err=False, _extra=None: printed.append(
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
