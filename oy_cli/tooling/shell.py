from __future__ import annotations

from typing import Any

from .. import runtime as rt
from .core import BashArgs, tool
from .output import (
    _parse_bash_json_output,
    _summarize_json_output,
    _summarize_text_output,
    _tool_content_payload,
)

def _merge_bash_streams(stdout: str, stderr: str) -> str:
    stdout = stdout.rstrip()
    stderr = stderr.rstrip()
    if stdout and stderr:
        return f"[stdout]\n{stdout}\n\n[stderr]\n{stderr}"
    if stdout:
        return stdout
    if stderr:
        return f"[stderr]\n{stderr}"
    return ""

def _bash_payload(command: str, result) -> dict[str, Any]:
    parsed = _parse_bash_json_output(result.stdout, result.stderr)
    if parsed is not None:
        content, truncated, content_format = _summarize_json_output(parsed)
    else:
        content, truncated = _summarize_text_output(
            _merge_bash_streams(result.stdout, result.stderr)
        )
        content_format = "text"
    return _tool_content_payload(
        command=command,
        exit_code=result.returncode,
        ok=result.returncode == 0,
        content=content,
        content_format=content_format,
        truncated=truncated,
    )

def _render_bash_preview(command: str, result, payload: dict[str, Any]) -> str:
    if payload.get("content_format") != "toon":
        return rt._fmt("bash", command, (result.stdout, result.returncode, result.stderr))

    toon_text = payload.get("content") or result.stdout
    blocks = [
        rt._fmt("block", f"$ {command}", "bash"),
        rt._fmt("block", toon_text, "text"),
    ]
    if result.returncode:
        blocks.append(rt._fmt("status", f"exit {result.returncode}"))
    if result.stderr.strip():
        blocks.extend(["**stderr**", rt._fmt("block", result.stderr.rstrip(), "text")])
    return "\n\n".join(blocks)

@tool(BashArgs)
def tool_bash(state: Any, command: str, timeout_seconds: int = 120):
    if len(command.encode("utf-8", errors="replace")) > rt.MAX_BASH_CMD_BYTES:
        raise ValueError(
            f"command too large ({len(command)} chars); limit is {rt.MAX_BASH_CMD_BYTES} bytes"
        )
    rt.note_tool(
        state,
        "bash",
        _defaults={"timeout": 120},
        command=command,
        timeout=timeout_seconds,
    )
    env = rt.require_command_env(state.root)
    bash_path = rt.which("bash", env.get("PATH"))
    if not bash_path:
        raise ValueError("bash is not installed or not on PATH")
    result = rt.run_cmd(
        [bash_path, "-c", command],
        cwd=state.root,
        env=env,
        timeout=timeout_seconds,
    )
    payload = _bash_payload(command, result)
    rt.show(_render_bash_preview(command, result, payload))
    return payload

__all__ = ["_bash_payload", "_render_bash_preview", "tool_bash"]
