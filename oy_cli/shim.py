from __future__ import annotations

from pathlib import Path
import os
from typing import Callable

from . import providers as _providers
from .providers import (
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


SHIM_OPENAI = _providers.SHIM_OPENAI
SHIM_CODEX = _providers.SHIM_CODEX
SHIM_BEDROCK = _providers.SHIM_BEDROCK
SHIM_MANTLE = _providers.SHIM_MANTLE
SHIM_COPILOT = _providers.SHIM_COPILOT
SHIM_OPENCODE = _providers.SHIM_OPENCODE
SHIM_OPENCODE_GO = _providers.SHIM_OPENCODE_GO
SHIM_ORDER = _providers.SHIM_ORDER
KNOWN_SHIMS = set(SHIM_ORDER)


# ---------------------------------------------------------------------------
# Public protocol and shared helper boundary
# ---------------------------------------------------------------------------


# Keep these names in shim.py so callers and tests can patch the shim boundary
# without importing providers directly.
load_json = _providers.load_json
save_json = _providers.save_json
run_cmd = _providers.run_cmd
which = _providers.which


command_env = _providers.command_env


default_region = _providers.default_region
join_model_spec = _providers.join_model_spec
split_model_spec = _providers.split_model_spec


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


def _static_shim(
    name: str,
    *,
    ensure_env: Callable[[], None],
    build_client: Callable[[], CompletionClient],
    list_models: Callable[[], list[str]] | None = None,
) -> ShimSpec:
    client_builder = _providers._static_client_builder(build_client)
    return ShimSpec(
        name=name,
        ensure_env=_providers._static_env_checker(ensure_env),
        build_client=client_builder,
        list_models=(
            _providers._static_model_lister(list_models)
            if list_models is not None
            else _providers._client_model_lister(client_builder)
        ),
    )


def _region_shim(
    name: str,
    *,
    ensure_env: Callable[[Path | None], None],
    build_client: Callable[[str | None], CompletionClient],
) -> ShimSpec:
    client_builder = _providers._region_client_builder(build_client)
    return ShimSpec(
        name=name,
        ensure_env=ensure_env,
        build_client=client_builder,
        list_models=_providers._client_model_lister(client_builder),
    )


def _runtime_shim(
    name: str,
    *,
    ensure_env: Callable[[Path | None], None],
    build_client: Callable[[str | None, Path | None], CompletionClient],
) -> ShimSpec:
    return ShimSpec(
        name=name,
        ensure_env=ensure_env,
        build_client=build_client,
        list_models=_providers._client_model_lister(build_client),
    )


SHIM_SPECS: dict[str, ShimSpec] = {
    SHIM_OPENAI: _static_shim(
        SHIM_OPENAI,
        ensure_env=_providers._require_openai_env,
        build_client=_providers._openai_client,
        list_models=lambda: _providers._openai_client(max_retries=0).list_models(),
    ),
    SHIM_CODEX: _static_shim(
        SHIM_CODEX,
        ensure_env=_providers._require_codex_env,
        build_client=_providers._codex_client,
    ),
    SHIM_BEDROCK: _region_shim(
        SHIM_BEDROCK,
        ensure_env=_providers._require_boto3_aws_env,
        build_client=_providers._bedrock_completion_client,
    ),
    SHIM_MANTLE: _runtime_shim(
        SHIM_MANTLE,
        ensure_env=_providers._require_aws_env,
        build_client=_providers._mantle_completion_client,
    ),
    SHIM_COPILOT: _static_shim(
        SHIM_COPILOT,
        ensure_env=_providers._require_copilot_env,
        build_client=_providers._copilot_completion_client,
    ),
    SHIM_OPENCODE: _static_shim(
        SHIM_OPENCODE,
        ensure_env=_providers._require_opencode_zen_env,
        build_client=_providers._opencode_zen_client,
    ),
    SHIM_OPENCODE_GO: _static_shim(
        SHIM_OPENCODE_GO,
        ensure_env=_providers._require_opencode_go_env,
        build_client=_providers._opencode_go_client,
    ),
}


def _shim_spec(shim: str) -> ShimSpec:
    return SHIM_SPECS[validate_shim(shim)]


def _resolved_shim_spec(
    model_spec: str | None = None, configured_shim: str | None = None
) -> tuple[str, ShimSpec]:
    shim = resolve_shim(model_spec, configured_shim)
    return shim, _shim_spec(shim)


def _shim_env_error(spec: ShimSpec, cwd: Path | None) -> str | None:
    try:
        spec.ensure_env(cwd)
    except Exception as exc:
        return str(exc)
    return None


def _shim_available(shim: str) -> bool:
    return _shim_env_error(_shim_spec(shim), None) is None


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
    _, spec = _resolved_shim_spec(model_spec, configured_shim)
    error = _shim_env_error(spec, cwd)
    return error is None, error


_MISSING_API_CREDENTIALS_MESSAGE = (
    "Missing API credentials.\n\n"
    "- set `OPENAI_API_KEY`, or\n"
    "- sign in with Codex CLI (`codex login`), or\n"
        "- configure AWS CLI for Bedrock, or\n"
    "- authenticate with OpenCode (`opencode auth`)"
)


def _missing_api_credentials_message(error: str | None) -> str:
    return (
        _MISSING_API_CREDENTIALS_MESSAGE
        if not error
        else f"{_MISSING_API_CREDENTIALS_MESSAGE}\n- error: {error}"
    )


def require_api_env(
    model_spec: str | None = None,
    configured_shim: str | None = None,
    cwd: Path | None = None,
) -> str:
    shim, spec = _resolved_shim_spec(model_spec, configured_shim)
    if error := _shim_env_error(spec, cwd):
        raise RuntimeError(_missing_api_credentials_message(error))
    return shim


def get_client(
    shim: str,
    model_spec: str | None = None,
    region: str | None = None,
    cwd: Path | None = None,
) -> CompletionClient:
    return _shim_spec(shim).build_client(region, cwd)


def list_models_for_shim(
    shim: str, region: str | None = None, cwd: Path | None = None
) -> list[str]:
    try:
        raw = _shim_spec(shim).list_models(region, cwd)
        return [join_model_spec(shim, model) for model in raw]
    except Exception:
        return []




__all__ = [
    "AssistantMessage",
    "ChatMessage",
    "CompletionClient",
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
