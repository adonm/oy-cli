from __future__ import annotations

from dataclasses import dataclass
import os
import subprocess
from pathlib import Path
from typing import Any, Callable, TypeAlias

# Protocol structs live here so shim/providers can depend on them without a
# larger runtime import cycle.

import httpx

from . import providers as _providers
from .protocol import (
    AssistantMessage,
    ChatMessage,
    CompletionClient,
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

CODEX_DEFAULT_MODEL = _providers.CODEX_DEFAULT_MODEL
OPENCODE_AUTH_PATH = _providers.OPENCODE_AUTH_PATH
OPENCODE_ZEN_URL = _providers.OPENCODE_ZEN_URL
OPENCODE_GO_URL = _providers.OPENCODE_GO_URL
_COPILOT_BASE_URL = os.environ.get(
    "COPILOT_BASE_URL", "https://api.githubcopilot.com"
)
_COPILOT_INTEGRATION_ID = "copilot-developer-cli"
_COPILOT_EDITOR_VERSION = "copilot-developer-cli/1.0.6"

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
# Unified shim implementations
# ---------------------------------------------------------------------------

def _require_string(value: Any, error: str) -> str:
    return _providers._require_string(value, error)

def _list_models_from_client(
    build_client, region: str | None = None, cwd: Path | None = None
) -> list[str]:
    return build_client(region, cwd).list_models()

def _require_openai_env(_cwd: Path | None = None) -> None:
    _require_string(_providers.get_openai_api_key(), "OPENAI_API_KEY is not set")

def _openai_client(
    _region: str | None = None,
    _cwd: Path | None = None,
    *,
    max_retries: int = 3,
) -> CompletionClient:
    return _providers._openai_responses_client(
        *_providers._openai_pair(
            _require_string(
                _providers.get_openai_api_key(), "No OpenAI credentials found"
            ),
            base_url=os.environ.get("OPENAI_BASE_URL"),
            max_retries=max_retries,
        )
    )

def _require_codex_env(_cwd: Path | None = None) -> None:
    _providers.load_codex_session()

def _codex_client(
    _region: str | None = None, _cwd: Path | None = None
) -> CompletionClient:
    if api_key := _providers.get_codex_api_key():
        return _providers._openai_responses_client(
            *_providers._openai_pair(api_key),
            fallback_models=_providers.load_codex_model_list,
            default_models=[CODEX_DEFAULT_MODEL],
        )
    return _providers._codex_chatgpt_client()

def _require_aws_env(cwd: Path | None = None) -> None:
    default_region(None)
    _providers.load_aws_credentials(cwd, allow_login=False)

def _require_boto3_aws_env(_cwd: Path | None = None) -> None:
    import boto3
    from botocore.exceptions import NoCredentialsError, PartialCredentialsError

    session = boto3.Session()
    credentials = session.get_credentials()
    if credentials is None:
        raise RuntimeError(
            "No AWS credentials found. Configure via environment variables, "
            "~/.aws/credentials, IAM role, or AWS SSO."
        )
    try:
        credentials.get_frozen_credentials()
    except (NoCredentialsError, PartialCredentialsError) as exc:
        raise RuntimeError(f"AWS credentials incomplete: {exc}") from exc

def _bedrock_completion_client(
    region: str | None = None, _cwd: Path | None = None
) -> CompletionClient:
    return _providers._bedrock_converse_client(default_region(region))

def _mantle_completion_client(
    region: str | None = None, cwd: Path | None = None
) -> CompletionClient:
    current = default_region(region)
    return _providers._openai_chat_completions_client(
        *_providers._openai_pair(
            _providers.make_bedrock_token(current, cwd),
            base_url=_providers.bedrock_base_url(current),
            max_retries=0,
            timeout=_providers.BEDROCK_TIMEOUT.read,
        ),
        tools_map=_providers._tool_specs_to_openai,
        bedrock=True,
    )

def _get_github_token() -> str | None:
    for var in ("COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"):
        val = os.environ.get(var)
        if isinstance(val, str) and val:
            return val
    gh = which("gh")
    if not gh:
        return None
    try:
        proc = subprocess.run(
            [gh, "auth", "token"],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except Exception:
        return None
    token = proc.stdout.strip()
    return token if proc.returncode == 0 and token else None

def _copilot_default_headers() -> dict[str, str]:
    return {
        "Copilot-Integration-Id": _COPILOT_INTEGRATION_ID,
        "Editor-Version": _COPILOT_EDITOR_VERSION,
    }

def _copilot_openai_pair(token: str):
    return _providers._openai_client_pair(
        api_key=token,
        base_url=_COPILOT_BASE_URL,
        max_retries=0,
        default_headers=_copilot_default_headers(),
    )

def _require_copilot_env(_cwd: Path | None = None) -> None:
    _require_string(
        _get_github_token(),
        "No GitHub token found (set GH_TOKEN, GITHUB_TOKEN, or run `gh auth login`)",
    )

def _fetch_copilot_models_raw(token: str) -> list[dict[str, Any]]:
    response = httpx.get(
        f"{_COPILOT_BASE_URL}/models",
        headers={
            "Authorization": f"Bearer {token}",
            **_copilot_default_headers(),
        },
        timeout=15,
    )
    response.raise_for_status()
    data = response.json()
    return data.get("data", []) if isinstance(data, dict) else []

def _classify_copilot_models(token: str) -> tuple[list[str], set[str]]:
    raw = _fetch_copilot_models_raw(token)
    chat_ids: list[str] = []
    responses_ids: set[str] = set()
    for model in raw:
        model_id = model.get("id")
        if not isinstance(model_id, str):
            continue
        if model.get("capabilities", {}).get("type") == "chat":
            chat_ids.append(model_id)
        if "/responses" in (model.get("supported_endpoints") or []):
            responses_ids.add(model_id)
    return sorted(chat_ids), responses_ids

def _copilot_completion_client(
    _region: str | None = None, _cwd: Path | None = None
) -> CompletionClient:
    token = _require_string(_get_github_token(), "No GitHub token found")
    async_client, sync_client = _copilot_openai_pair(token)

    try:
        _, responses_models = _classify_copilot_models(token)
    except Exception:
        responses_models = set()

    responses_inner = _providers._openai_responses_client(
        async_client,
        sync_client,
        fallback_models=None,
        default_models=None,
    )
    chat_inner = _providers._openai_chat_completions_client(
        async_client,
        sync_client,
        tools_map=_providers._tool_specs_to_openai,
    )

    async def chat_completion(
        model: str,
        messages: list[ChatMessage],
        tools: list[ToolSpec] | None = None,
        tool_choice: str = "auto",
        on_retry=None,
    ) -> AssistantMessage:
        inner = responses_inner if model in responses_models else chat_inner
        return await inner.chat_completion(
            model,
            messages,
            tools,
            tool_choice,
            on_retry,
        )

    def list_models() -> list[str]:
        try:
            chat_ids, _ = _classify_copilot_models(token)
            return chat_ids
        except Exception:
            return sorted(
                model.id
                for model in sync_client.models.list()
                if not model.id.startswith("text-embedding")
            )

    return CompletionClient(chat_completion=chat_completion, list_models=list_models)

def _load_opencode_auth() -> dict[str, Any]:
    data = load_json(OPENCODE_AUTH_PATH, {})
    return data if isinstance(data, dict) else {}

def get_opencode_zen_api_key() -> str | None:
    entry = _load_opencode_auth().get("opencode", {})
    return (entry.get("key") or None) if isinstance(entry, dict) else None

def get_opencode_go_api_key() -> str | None:
    entry = _load_opencode_auth().get("opencode-go", {})
    return (entry.get("key") or None) if isinstance(entry, dict) else None

def _require_opencode_zen_env(_cwd: Path | None = None) -> None:
    _require_string(
        get_opencode_zen_api_key(),
        f"No OpenCode Zen credentials found in {OPENCODE_AUTH_PATH} (run `opencode auth`)",
    )

def _require_opencode_go_env(_cwd: Path | None = None) -> None:
    _require_string(
        get_opencode_go_api_key(),
        f"No OpenCode Go credentials found in {OPENCODE_AUTH_PATH} (run `opencode auth`)",
    )

def _opencode_zen_client(
    _region: str | None = None, _cwd: Path | None = None
) -> CompletionClient:
    key = _require_string(
        get_opencode_zen_api_key(), "No OpenCode Zen credentials found"
    )
    return _providers._openai_chat_completions_client(
        *_providers._openai_pair(key, base_url=OPENCODE_ZEN_URL),
        tools_map=_providers._tool_specs_to_openai,
    )

def _opencode_go_client(
    _region: str | None = None, _cwd: Path | None = None
) -> CompletionClient:
    key = _require_string(
        get_opencode_go_api_key(), "No OpenCode Go credentials found"
    )
    return _providers._openai_chat_completions_client(
        *_providers._openai_pair(key, base_url=OPENCODE_GO_URL),
        tools_map=_providers._tool_specs_to_openai,
    )

ShimEnvChecker: TypeAlias = Callable[[Path | None], None]
ShimClientBuilder: TypeAlias = Callable[..., CompletionClient]
ShimModelLister: TypeAlias = Callable[[str | None, Path | None], list[str]]

@dataclass(frozen=True, slots=True)
class ShimSpec:
    name: str
    ensure_env: ShimEnvChecker
    build_client: ShimClientBuilder
    list_models: ShimModelLister

# ---------------------------------------------------------------------------
# Public shim registry and runtime selection
# ---------------------------------------------------------------------------

def validate_shim(shim: str) -> str:
    if shim not in KNOWN_SHIMS:
        raise RuntimeError(
            f"Unknown shim value: `{shim}`. Use one of: {', '.join(SHIM_ORDER)}"
        )
    return shim

SHIM_SPECS: dict[str, ShimSpec] = {
    SHIM_OPENAI: ShimSpec(
        name=SHIM_OPENAI,
        ensure_env=_require_openai_env,
        build_client=_openai_client,
        list_models=lambda region, cwd: _openai_client(
            region, cwd, max_retries=0
        ).list_models(),
    ),
    SHIM_CODEX: ShimSpec(
        name=SHIM_CODEX,
        ensure_env=_require_codex_env,
        build_client=_codex_client,
        list_models=lambda region, cwd: _list_models_from_client(
            _codex_client, region, cwd
        ),
    ),
    SHIM_BEDROCK: ShimSpec(
        name=SHIM_BEDROCK,
        ensure_env=_require_boto3_aws_env,
        build_client=_bedrock_completion_client,
        list_models=lambda region, cwd: _list_models_from_client(
            _bedrock_completion_client, region, cwd
        ),
    ),
    SHIM_MANTLE: ShimSpec(
        name=SHIM_MANTLE,
        ensure_env=_require_aws_env,
        build_client=_mantle_completion_client,
        list_models=lambda region, cwd: _list_models_from_client(
            _mantle_completion_client, region, cwd
        ),
    ),
    SHIM_COPILOT: ShimSpec(
        name=SHIM_COPILOT,
        ensure_env=_require_copilot_env,
        build_client=_copilot_completion_client,
        list_models=lambda region, cwd: _list_models_from_client(
            _copilot_completion_client, region, cwd
        ),
    ),
    SHIM_OPENCODE: ShimSpec(
        name=SHIM_OPENCODE,
        ensure_env=_require_opencode_zen_env,
        build_client=_opencode_zen_client,
        list_models=lambda region, cwd: _list_models_from_client(
            _opencode_zen_client, region, cwd
        ),
    ),
    SHIM_OPENCODE_GO: ShimSpec(
        name=SHIM_OPENCODE_GO,
        ensure_env=_require_opencode_go_env,
        build_client=_opencode_go_client,
        list_models=lambda region, cwd: _list_models_from_client(
            _opencode_go_client, region, cwd
        ),
    ),
}

def _shim_spec(shim: str) -> ShimSpec:
    return SHIM_SPECS[validate_shim(shim)]

def _shim_env_error(spec: ShimSpec, cwd: Path | None) -> str | None:
    try:
        spec.ensure_env(cwd)
    except Exception as exc:
        return str(exc)
    return None

def _shim_available(shim: str) -> bool:
    return _shim_env_error(_shim_spec(shim), None) is None

def detect_available_shims() -> list[str]:
    return [shim for shim in SHIM_ORDER if _shim_available(shim)]

def resolve_shim(
    model_spec: str | None = None, configured_shim: str | None = None
) -> str:
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
    shim = resolve_shim(model_spec, configured_shim)
    if error := _shim_env_error(_shim_spec(shim), cwd):
        raise RuntimeError(_missing_api_credentials_message(error))
    return shim

def get_client(
    shim: str,
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
