from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
from typing import Any, Callable

import providers as _providers
from providers import (
    AssistantMessage,
    ChatMessage,
    CompletionClient,
    ShimSpec,
    SystemMessage,
    ToolCall,
    ToolMessage,
    ToolResult,
    ToolSpec,
    UserMessage,
)

APIStatusError = _providers.APIStatusError

SHIM_OPENAI = _providers.SHIM_OPENAI
SHIM_CODEX = _providers.SHIM_CODEX
SHIM_GEMINI = _providers.SHIM_GEMINI
SHIM_BEDROCK = _providers.SHIM_BEDROCK
SHIM_MANTLE = _providers.SHIM_MANTLE
SHIM_CLAUDE = _providers.SHIM_CLAUDE
SHIM_COPILOT = _providers.SHIM_COPILOT
SHIM_ORDER = _providers.SHIM_ORDER
KNOWN_SHIMS = set(SHIM_ORDER)


# ---------------------------------------------------------------------------
# Public protocol and shared helper boundary
# ---------------------------------------------------------------------------


def load_json(path, default):
    return _providers.load_json(path, default)


def save_json(path, data):
    return _providers.save_json(path, data)


def run_cmd(cmd, **kwargs):
    return _providers.run_cmd(cmd, **kwargs)


def which(tool, path=None):
    return _providers.which(tool, path)


def command_env(cwd=None):
    env = os.environ.copy()
    if not which("mise", env.get("PATH")):
        raise RuntimeError(
            "`mise` is required; install and activate `mise` before running `oy`."
        )
    return _providers.MappingProxyType(env)


def _clear_command_env_cache() -> None:
    cache_clear = getattr(_providers.command_env, "cache_clear", None)
    if callable(cache_clear):
        cache_clear()


command_env.cache_clear = _clear_command_env_cache


def default_region() -> str:
    return _providers.default_region()


def join_model_spec(shim: str, model: str) -> str:
    return _providers.join_model_spec(shim, model)


def split_model_spec(spec: str) -> tuple[str | None, str]:
    return _providers.split_model_spec(spec)


# ---------------------------------------------------------------------------
# Public shim registry and runtime selection
# ---------------------------------------------------------------------------


def validate_shim(shim: str) -> str:
    """Raise RuntimeError if *shim* is not a known backend name."""
    if shim not in KNOWN_SHIMS:
        raise RuntimeError(
            f"Unknown shim value: `{shim}`. Use one of: {', '.join(SHIM_ORDER)}"
        )
    return shim


SHIM_SPECS: dict[str, ShimSpec] = {
    SHIM_OPENAI: ShimSpec(
        name=SHIM_OPENAI,
        ensure_env=_providers._static_env_checker(_providers._require_openai_env),
        build_client=_providers._static_client_builder(_providers._openai_client),
        list_models=_providers._static_model_lister(
            lambda: _providers._openai_client(max_retries=0).list_models()
        ),
    ),
    SHIM_CODEX: ShimSpec(
        name=SHIM_CODEX,
        ensure_env=_providers._static_env_checker(_providers._require_codex_env),
        build_client=_providers._static_client_builder(_providers._codex_client),
        list_models=_providers._client_model_lister(
            _providers._static_client_builder(_providers._codex_client)
        ),
    ),
    SHIM_GEMINI: ShimSpec(
        name=SHIM_GEMINI,
        ensure_env=_providers._static_env_checker(_providers._require_gemini_env),
        build_client=_providers._static_client_builder(_providers._gemini_completion_client),
        list_models=_providers._static_model_lister(_providers.load_gemini_model_list),
    ),
    SHIM_BEDROCK: ShimSpec(
        name=SHIM_BEDROCK,
        ensure_env=_providers._require_boto3_aws_env,
        build_client=_providers._region_client_builder(_providers._bedrock_completion_client),
        list_models=_providers._client_model_lister(
            _providers._region_client_builder(_providers._bedrock_completion_client)
        ),
    ),
    SHIM_MANTLE: ShimSpec(
        name=SHIM_MANTLE,
        ensure_env=_providers._require_aws_env,
        build_client=_providers._mantle_completion_client,
        list_models=_providers._client_model_lister(_providers._mantle_completion_client),
    ),
    SHIM_CLAUDE: ShimSpec(
        name=SHIM_CLAUDE,
        ensure_env=_providers._static_env_checker(_providers._require_claude_env),
        build_client=_providers._static_client_builder(_providers._claude_client_from_auth),
        list_models=_providers._static_model_lister(_providers._claude_model_list),
    ),
    SHIM_COPILOT: ShimSpec(
        name=SHIM_COPILOT,
        ensure_env=_providers._static_env_checker(_providers._require_copilot_env),
        build_client=_providers._static_client_builder(_providers._copilot_completion_client),
        list_models=_providers._client_model_lister(
            _providers._static_client_builder(_providers._copilot_completion_client)
        ),
    ),
}


def _shim_spec(shim: str) -> ShimSpec:
    return SHIM_SPECS[validate_shim(shim)]


def _shim_available(shim: str) -> bool:
    try:
        _shim_spec(shim).ensure_env(None)
        return True
    except Exception:
        return False


def detect_available_shims() -> list[str]:
    """Probe each known shim and return the names of those with valid credentials."""
    return [shim for shim in SHIM_ORDER if _shim_available(shim)]


def resolve_shim(
    model_spec: str | None = None, configured_shim: str | None = None
) -> str:
    """Determine which backend to use from model spec, config, or auto-detection."""
    if env_shim := os.environ.get("OY_SHIM"):
        return env_shim
    if model_spec:
        prefix, _ = split_model_spec(model_spec)
        if prefix:
            return prefix
    if configured_shim:
        return configured_shim
    shims = detect_available_shims()
    return shims[0] if shims else SHIM_MANTLE


def ensure_api_env(
    model_spec: str | None = None,
    configured_shim: str | None = None,
    cwd: Path | None = None,
) -> tuple[bool, str | None]:
    spec = _shim_spec(resolve_shim(model_spec, configured_shim))
    try:
        spec.ensure_env(cwd)
        return True, None
    except Exception as exc:
        return False, str(exc)


def require_api_env(
    model_spec: str | None = None,
    configured_shim: str | None = None,
    cwd: Path | None = None,
) -> str:
    ok, error = ensure_api_env(model_spec, configured_shim, cwd)
    if ok:
        return validate_shim(resolve_shim(model_spec, configured_shim))
    message = (
        "Missing API credentials.\n\n"
        "- set `OPENAI_API_KEY`, or\n"
        "- sign in with Codex CLI (`codex login`), or\n"
        "- install Gemini CLI and run `gemini` once to authenticate, or\n"
        "- sign in with Claude Code (`claude auth login`), or\n"
        "- configure AWS CLI for Bedrock"
    )
    if error:
        message += f"\n- error: {error}"
    raise RuntimeError(message)


def get_client(
    shim: str,
    model_spec: str | None = None,
    region: str | None = None,
    cwd: Path | None = None,
) -> CompletionClient:
    _ = model_spec
    return _shim_spec(shim).build_client(region, cwd)


def list_models_for_shim(
    shim: str, region: str | None = None, cwd: Path | None = None
) -> list[str]:
    try:
        raw = _shim_spec(shim).list_models(region, cwd)
        return [join_model_spec(shim, model) for model in raw]
    except Exception:
        return []


def list_all_model_ids(region: str | None = None, cwd: Path | None = None) -> list[str]:
    models: list[str] = []
    for shim in detect_available_shims():
        models.extend(list_models_for_shim(shim, region=region, cwd=cwd))
    return models


# ---------------------------------------------------------------------------
# Narrow bridge consumed by oy_cli
# ---------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class ShimBridge:
    load_json: Callable[[Path, Any], Any]
    save_json: Callable[[Path, Any], bool]
    run_cmd: Callable[..., Any]
    which: Callable[[str, str | None], str | None]
    command_env: Callable[[Path | None], Any]
    default_region: Callable[[], str]
    detect_available_shims: Callable[[], list[str]]
    ensure_api_env: Callable[[str | None, str | None, Path | None], tuple[bool, str | None]]
    require_api_env: Callable[[str | None, str | None, Path | None], str]
    build_client: Callable[..., CompletionClient]
    list_models_for_shim: Callable[[str, str | None, Path | None], list[str]]
    resolve_shim: Callable[[str | None, str | None], str]
    validate_shim: Callable[[str], str]
    join_model_spec: Callable[[str, str], str]
    split_model_spec: Callable[[str], tuple[str | None, str]]


SHIMS = ShimBridge(
    load_json=load_json,
    save_json=save_json,
    run_cmd=run_cmd,
    which=which,
    command_env=command_env,
    default_region=default_region,
    detect_available_shims=detect_available_shims,
    ensure_api_env=ensure_api_env,
    require_api_env=require_api_env,
    build_client=get_client,
    list_models_for_shim=list_models_for_shim,
    resolve_shim=resolve_shim,
    validate_shim=validate_shim,
    join_model_spec=join_model_spec,
    split_model_spec=split_model_spec,
)


__all__ = [
    "APIStatusError",
    "AssistantMessage",
    "ChatMessage",
    "CompletionClient",
    "SHIMS",
    "ShimBridge",
    "ShimSpec",
    "SystemMessage",
    "ToolCall",
    "ToolMessage",
    "ToolResult",
    "ToolSpec",
    "UserMessage",
    "command_env",
    "default_region",
    "detect_available_shims",
    "ensure_api_env",
    "get_client",
    "join_model_spec",
    "list_all_model_ids",
    "list_models_for_shim",
    "load_json",
    "require_api_env",
    "resolve_shim",
    "run_cmd",
    "save_json",
    "split_model_spec",
    "validate_shim",
    "which",
]
