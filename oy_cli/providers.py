from __future__ import annotations
import base64
import hashlib
import hmac
import json
import os
import shutil
import subprocess
import sys
import threading as _threading
from dataclasses import dataclass
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from functools import lru_cache
from types import MappingProxyType
from pathlib import Path
from typing import Any, Callable, TypeAlias
from urllib.parse import quote
import httpx
import msgspec
from openai import (
    APIConnectionError,
    APIStatusError,
    APITimeoutError,
    AsyncOpenAI,
    OpenAI,
)
from tenacity import AsyncRetrying, retry_if_exception_type, stop_after_attempt
from tenacity.wait import wait_base

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
from .serialization import JSONLike, normalize_jsonlike, serialize_json, serialize_toon

SHIM_OPENAI = "openai"
SHIM_CODEX = "codex"
SHIM_BEDROCK = "bedrock"
SHIM_MANTLE = "bedrock-mantle"
SHIM_COPILOT = "copilot"
SHIM_OPENCODE = "opencode"
SHIM_OPENCODE_GO = "opencode-go"
SHIM_ORDER = (
    SHIM_OPENAI,
    SHIM_CODEX,
    SHIM_BEDROCK,
    SHIM_MANTLE,
    SHIM_COPILOT,
    SHIM_OPENCODE,
    SHIM_OPENCODE_GO,
)

SSO_MARKERS = (
    "error loading sso token",
    "the sso session associated with this profile has expired",
    "the sso session has expired or is otherwise invalid",
    "to refresh this sso session run aws sso login",
)
CODEX_AUTH_PATH = Path.home() / ".codex" / "auth.json"
CODEX_MODELS_CACHE_PATH = Path.home() / ".codex" / "models_cache.json"
OPENCODE_AUTH_PATH = Path.home() / ".local" / "share" / "opencode" / "auth.json"
OPENCODE_ZEN_URL = "https://opencode.ai/zen/v1"
OPENCODE_GO_URL = "https://opencode.ai/zen/go/v1"
CODEX_DEFAULT_MODEL = "gpt-5-codex"
CODEX_CHATGPT_RESPONSES_URL = "https://chatgpt.com/backend-api/codex/responses"
CODEX_OAUTH_TOKEN_URL = "https://auth.openai.com/oauth/token"
# --- Public OAuth2 "installed app" credentials ---
# These are NOT confidential server secrets. OpenAI embeds client IDs in
# CLI binaries by design (RFC 8252 §8.5). Safe to publish in source code.
# Override via env: CODEX_OAUTH_CLIENT_ID
CODEX_OAUTH_CLIENT_ID = (
    os.environ.get("CODEX_OAUTH_CLIENT_ID") or "app_EMoamEEZ73f0CkXaXp7hrann"
)
DEFAULT_RETRY_MAX_ATTEMPTS = 10
DEFAULT_RETRY_INITIAL_DELAY_SECONDS = 5.0
DEFAULT_RETRY_MAX_DELAY_SECONDS = 30.0
BEDROCK_RETRY_MAX_DELAY_SECONDS = 90.0
BEDROCK_TIMEOUT = httpx.Timeout(
    connect=10.0,
    read=float(os.environ.get("OY_BEDROCK_READ_TIMEOUT", "120")),
    write=10.0,
    pool=10.0,
)
TRANSPORT_ERROR_RETRY_DELAY = 3.0

JSONDict: TypeAlias = dict[str, Any]
ProviderItem: TypeAlias = JSONDict | str

class TextBlock(msgspec.Struct, omit_defaults=True):
    text: str

class ToolUseBlock(msgspec.Struct, omit_defaults=True):
    id: str
    name: str
    arguments: dict[str, Any] = msgspec.field(default_factory=dict)
    thought_signature: str = ""

class ToolResultBlock(msgspec.Struct, omit_defaults=True):
    id: str
    name: str = ""
    result: ToolResult = msgspec.field(default_factory=ToolResult)

ContentBlock: TypeAlias = TextBlock | ToolUseBlock | ToolResultBlock
ProviderSystem: TypeAlias = ProviderItem | list[ProviderItem] | None
ProviderTextEncoder: TypeAlias = Callable[[str], ProviderItem]
ProviderToolUseEncoder: TypeAlias = Callable[[ToolUseBlock], ProviderItem]
ProviderToolResultEncoder: TypeAlias = Callable[[ToolResultBlock], ProviderItem]
ProviderSystemItem: TypeAlias = Callable[[str], ProviderItem]
ProviderSystemFinalizer: TypeAlias = Callable[[list[ProviderItem]], ProviderSystem]

@dataclass(frozen=True, slots=True)
class ProviderCodec:
    user_role: str
    assistant_role: str
    content_key: str
    system_item: ProviderSystemItem
    finalize_system: ProviderSystemFinalizer
    encode_text: ProviderTextEncoder
    encode_tool_use: ProviderToolUseEncoder
    encode_tool_result: ProviderToolResultEncoder
    merge_tool_results: bool = True

def _tool_output_value(result: ToolResult) -> JSONLike:
    return normalize_jsonlike(result.content)

def _tool_output_text(result: ToolResult) -> str:
    return serialize_toon(_tool_output_value(result))

def _assistant_blocks(message: AssistantMessage) -> list[ContentBlock]:
    blocks: list[ContentBlock] = [TextBlock(message.content)] if message.content else []
    fallback_signature = next(iter(message.thought_signatures.values()), "")
    blocks.extend(
        ToolUseBlock(
            id=call.id,
            name=call.name,
            arguments=call.arguments,
            thought_signature=message.thought_signatures.get(call.id, fallback_signature),
        )
        for call in message.tool_calls
    )
    return blocks

def _tool_message_block(message: ToolMessage) -> ToolResultBlock:
    return ToolResultBlock(
        id=message.tool_call_id, name=message.name, result=message.content
    )

def _assistant_from_blocks(blocks: list[ContentBlock]) -> AssistantMessage:
    content_parts: list[str] = []
    tool_calls: list[ToolCall] = []
    thought_signatures: dict[str, str] = {}
    for block in blocks:
        match block:
            case TextBlock():
                if not _is_blank_chat_value(block.text):
                    content_parts.append(block.text)
            case ToolUseBlock():
                tool_calls.append(
                    ToolCall(id=block.id, name=block.name, arguments=block.arguments)
                )
                if block.thought_signature:
                    thought_signatures[block.id] = block.thought_signature
    content = "".join(content_parts)
    if _is_blank_chat_value(content):
        content = ""
    return AssistantMessage(
        content=content,
        tool_calls=tool_calls,
        thought_signatures=thought_signatures,
    )

def _encode_provider_block(block: ContentBlock, codec: ProviderCodec):
    match block:
        case TextBlock():
            return codec.encode_text(block.text)
        case ToolUseBlock():
            return codec.encode_tool_use(block)
        case ToolResultBlock():
            return codec.encode_tool_result(block)
    raise TypeError(f"Unsupported content block: {type(block).__name__}")

def _encode_provider_messages(
    messages: list[ChatMessage], codec: ProviderCodec
) -> tuple[list[dict[str, Any]], Any]:
    system_parts: list[Any] = []
    encoded: list[dict[str, Any]] = []
    for message in messages:
        if isinstance(message, SystemMessage):
            if message.content:
                system_parts.append(codec.system_item(message.content))
            continue
        if isinstance(message, UserMessage):
            role = codec.user_role
            blocks: list[ContentBlock] = (
                [TextBlock(message.content)] if message.content else []
            )
        elif isinstance(message, AssistantMessage):
            role = codec.assistant_role
            blocks = _assistant_blocks(message)
        else:
            role = codec.user_role
            blocks = [_tool_message_block(message)]
        items = [_encode_provider_block(block, codec) for block in blocks]
        if not items:
            continue
        if (
            isinstance(message, ToolMessage)
            and codec.merge_tool_results
            and encoded
            and encoded[-1]["role"] == codec.user_role
        ):
            encoded[-1][codec.content_key].extend(items)
        else:
            encoded.append({"role": role, codec.content_key: items})
    return encoded, codec.finalize_system(system_parts)

def _extract_blocks(
    items: list[Any],
    *,
    text_of: Callable[[Any], str | None],
    tool_of: Callable[[Any, int], ToolUseBlock | None],
) -> list[ContentBlock]:
    blocks: list[ContentBlock] = []
    for index, item in enumerate(items):
        text = text_of(item)
        if isinstance(text, str) and not _is_blank_chat_value(text):
            blocks.append(TextBlock(text))
        if tool := tool_of(item, index):
            blocks.append(tool)
    return blocks

def _openai_tool_call(tool_call: ToolCall) -> dict[str, Any]:
    return {
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.name,
            "arguments": serialize_json(tool_call.arguments),
        },
    }

def _openai_chat_message(message: ChatMessage) -> dict[str, Any]:
    match message:
        case SystemMessage():
            return {"role": "system", "content": message.content}
        case UserMessage():
            return {"role": "user", "content": message.content}
        case AssistantMessage():
            payload = {"role": "assistant", "content": message.content}
            if message.tool_calls:
                payload["tool_calls"] = [
                    _openai_tool_call(tool_call) for tool_call in message.tool_calls
                ]
            if message.thought_signatures:
                payload["thought_signatures"] = message.thought_signatures
            return payload
        case ToolMessage():
            return {
                "role": "tool",
                "tool_call_id": message.tool_call_id,
                "name": message.name,
                "content": _tool_output_text(message.content),
            }
    raise TypeError(f"Unsupported message type: {type(message).__name__}")

# ---------------------------------------------------------------------------
# Local persistence and shell/runtime helpers
# ---------------------------------------------------------------------------

def load_json(p, d):
    try:
        return json.loads(p.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return d

def _ensure_private_dir(path: Path) -> None:
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.chmod(0o700)

def save_json(p, d):
    try:
        _ensure_private_dir(p.parent)
        p.write_text(json.dumps(d, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        p.chmod(0o600)
        return True
    except OSError:
        return False

def unique_strings(v):
    return list(dict.fromkeys(x for x in v if x))

def _first_nonempty_string(data: dict[str, Any], *keys: str) -> str | None:
    for key in keys:
        value = data.get(key)
        if isinstance(value, str) and value:
            return value
    return None

def _require_string(value: Any, error: str) -> str:
    if not isinstance(value, str) or not value:
        raise RuntimeError(error)
    return value

def _post_form_json(
    url: str, data: dict[str, str], *, error_prefix: str
) -> dict[str, Any]:
    try:
        with http_client(timeout=15) as client:
            resp = client.post(
                url,
                data=data,
                headers={"Content-Type": "application/x-www-form-urlencoded"},
            )
            resp.raise_for_status()
            payload = resp.json()
    except httpx.HTTPError as exc:
        raise RuntimeError(f"{error_prefix}: {exc}") from exc
    if not isinstance(payload, dict):
        raise RuntimeError(f"{error_prefix}: invalid JSON response")
    return payload

def _extract_model_ids(items: Any, *keys: str) -> list[str]:
    if not isinstance(items, list):
        return []
    return unique_strings(
        _first_nonempty_string(item, *keys) for item in items if isinstance(item, dict)
    )

def which(t, p=None):
    return shutil.which(t, path=p)

def run_cmd(cmd, cwd=None, env=None, timeout=120, stdin_text=None):
    if not cmd:
        raise ValueError("command must not be empty")
    try:
        return subprocess.run(
            cmd,
            cwd=cwd,
            env=env,
            input=stdin_text,
            text=True,
            capture_output=True,
            timeout=max(timeout, 1),
        )
    except subprocess.TimeoutExpired as e:
        raise ValueError(f"command timed out after {timeout}s") from e

# lru_cache is intentional: the launch environment is expected to be stable
# within a single oy run. If env vars change mid-process (e.g. in tests), the
# cache will be stale.
@lru_cache(maxsize=8)
def command_env(_cwd=None):
    return MappingProxyType(os.environ.copy())

_OPENAI_HTTPX_ONLY_KWARGS = (
    "api_key",
    "max_retries",
    "default_headers",
    "default_query",
    "organization",
    "project",
    "webhook_secret",
    "_strict_response_validation",
)

def _httpx_client_kwargs(kwargs: dict[str, Any]) -> dict[str, Any]:
    httpx_kwargs = dict(kwargs)
    for key in _OPENAI_HTTPX_ONLY_KWARGS:
        httpx_kwargs.pop(key, None)
    return httpx_kwargs

def http_client(**kw):
    httpx_kwargs = _httpx_client_kwargs(kw)
    httpx_kwargs.setdefault("follow_redirects", False)
    return httpx.Client(**httpx_kwargs)

def async_http_client(**kw):
    httpx_kwargs = _httpx_client_kwargs(kw)
    httpx_kwargs.setdefault("follow_redirects", False)
    return httpx.AsyncClient(**httpx_kwargs)

def _sigv4_sign(key: bytes, msg: str) -> bytes:
    return hmac.new(key, msg.encode("utf-8"), hashlib.sha256).digest()

def _sigv4_key(secret_key: str, date_stamp: str, region: str, service: str) -> bytes:
    key = _sigv4_sign(("AWS4" + secret_key).encode("utf-8"), date_stamp)
    for part in (region, service, "aws4_request"):
        key = _sigv4_sign(key, part)
    return key

def bedrock_base_url(region: str) -> str:
    return f"https://bedrock-mantle.{region}.api.aws/v1"

def make_bedrock_token(
    region: str, cwd: Path | None = None, expires: int = 43200
) -> str:
    creds = load_aws_credentials(cwd)
    now = datetime.now(timezone.utc)
    amz_date, date_stamp = now.strftime("%Y%m%dT%H%M%SZ"), now.strftime("%Y%m%d")
    query = [
        ("Action", "CallWithBearerToken"),
        ("X-Amz-Algorithm", "AWS4-HMAC-SHA256"),
        (
            "X-Amz-Credential",
            f"{creds['access_key']}/{date_stamp}/{region}/bedrock/aws4_request",
        ),
        ("X-Amz-Date", amz_date),
        ("X-Amz-Expires", str(expires)),
        ("X-Amz-SignedHeaders", "host"),
    ]
    if token := creds.get("session_token"):
        query.append(("X-Amz-Security-Token", token))
    canonical = "&".join(
        f"{quote(k, safe='-_.~')}={quote(v, safe='-_.~')}" for k, v in sorted(query)
    )
    request = f"POST\n/\n{canonical}\nhost:bedrock.amazonaws.com\n\nhost\n{hashlib.sha256(b'').hexdigest()}"
    scope = f"{date_stamp}/{region}/bedrock/aws4_request"
    string_to_sign = f"AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hashlib.sha256(request.encode()).hexdigest()}"
    signature = hmac.new(
        _sigv4_key(creds["secret_key"], date_stamp, region, "bedrock"),
        string_to_sign.encode(),
        hashlib.sha256,
    ).hexdigest()
    raw = f"bedrock.amazonaws.com/?{canonical}&X-Amz-Signature={signature}&Version=1"
    return f"bedrock-api-key-{base64.b64encode(raw.encode()).decode()}"

def aws_cli(parts, cwd=None, timeout=10):
    env = command_env(cwd)
    if not (aws := which("aws", env.get("PATH"))):
        raise RuntimeError("AWS CLI is not installed or not on PATH")
    return run_cmd([aws, *parts], cwd=cwd, env=env, timeout=timeout)

def run_aws_sso_login(cwd=None):
    env = command_env(cwd)
    if not (aws := which("aws", env.get("PATH"))):
        raise RuntimeError("AWS CLI is not installed or not on PATH")
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise RuntimeError(
            "AWS SSO session is stale. Run `aws sso login --use-device-code --no-browser` and retry."
        )
    r = run_cmd(
        [aws, "sso", "login", "--use-device-code", "--no-browser", "--no-cli-pager"],
        cwd=cwd,
        env=env,
        timeout=300,
    )
    if r.returncode:
        raise RuntimeError(f"AWS SSO login failed with exit code {r.returncode}")

def load_aws_credentials(
    cwd: Path | None = None, allow_login: bool = True
) -> dict[str, str]:
    result = aws_cli(
        ["configure", "export-credentials", "--format", "process", "--no-cli-pager"],
        cwd=cwd,
        timeout=30,
    )
    if result.returncode:
        message = (
            result.stderr.strip()
            or result.stdout.strip()
            or f"AWS CLI exited with status {result.returncode}"
        )
        stale = any(marker in message.lower() for marker in SSO_MARKERS) or (
            "token for" in message.lower() and "does not exist" in message.lower()
        )
        if allow_login and stale:
            run_aws_sso_login(cwd)
            return load_aws_credentials(cwd, False)
        raise RuntimeError(message)
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"Could not parse AWS credentials JSON: {exc}") from exc
    key = payload.get("AccessKeyId")
    secret = payload.get("SecretAccessKey")
    token = payload.get("SessionToken")
    if not isinstance(key, str) or not isinstance(secret, str):
        raise RuntimeError("AWS CLI did not return AccessKeyId/SecretAccessKey")
    creds = {"access_key": key, "secret_key": secret}
    if isinstance(token, str) and token:
        creds["session_token"] = token
    return creds

def default_region(choice: str | None = None) -> str:
    return (
        choice
        or os.environ.get("AWS_REGION")
        or os.environ.get("AWS_DEFAULT_REGION")
        or "us-east-1"
    )

# ---------------------------------------------------------------------------
# Credential loading and model discovery helpers
# ---------------------------------------------------------------------------

def load_codex_auth() -> dict[str, Any]:
    data = load_json(CODEX_AUTH_PATH, {})
    return data if isinstance(data, dict) else {}

def get_openai_api_key() -> str | None:
    return os.environ.get("OPENAI_API_KEY")

def load_codex_session() -> dict[str, Any]:
    auth = load_codex_auth()
    if not auth:
        raise RuntimeError("Codex CLI credentials were not found in ~/.codex/auth.json")
    if isinstance(auth.get("OPENAI_API_KEY"), str) and auth.get("OPENAI_API_KEY"):
        return auth
    tokens = auth.get("tokens")
    if isinstance(tokens, dict) and any(
        isinstance(tokens.get(key), str) and tokens.get(key)
        for key in ("access_token", "refresh_token")
    ):
        return auth
    raise RuntimeError("Codex CLI auth file does not contain a usable session")

def get_codex_api_key() -> str | None:
    auth = load_codex_auth()
    key = auth.get("OPENAI_API_KEY")
    return key if isinstance(key, str) and key else None

def _jwt_expiry_epoch(token: str) -> float | None:
    try:
        parts = token.split(".")
        if len(parts) < 2:
            return None
        payload = parts[1]
        payload += "=" * (-len(payload) % 4)
        data = json.loads(base64.urlsafe_b64decode(payload.encode("ascii")))
    except (OSError, UnicodeDecodeError, ValueError, json.JSONDecodeError):
        return None
    exp = data.get("exp")
    if isinstance(exp, (int, float)):
        return float(exp)
    return None

def _codex_tokens(auth: dict[str, Any]) -> dict[str, str]:
    tokens = auth.get("tokens")
    if not isinstance(tokens, dict):
        raise RuntimeError("Codex CLI auth file does not contain session tokens")
    result: dict[str, str] = {}
    for key in ("access_token", "refresh_token", "id_token", "account_id"):
        value = tokens.get(key)
        if isinstance(value, str) and value:
            result[key] = value
    return result

def refresh_codex_chatgpt_session(refresh_token: str) -> dict[str, Any]:
    data = _post_form_json(
        CODEX_OAUTH_TOKEN_URL,
        {
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CODEX_OAUTH_CLIENT_ID,
        },
        error_prefix="Codex token refresh failed",
    )
    access_token = _require_string(
        data.get("access_token"),
        "Codex token refresh did not return an access_token",
    )
    auth = load_codex_auth()
    tokens = auth.get("tokens") if isinstance(auth.get("tokens"), dict) else {}
    tokens["access_token"] = access_token
    if token := _first_nonempty_string(data, "refresh_token"):
        tokens["refresh_token"] = token
    if token := _first_nonempty_string(data, "id_token"):
        tokens["id_token"] = token
    auth.update(
        {"tokens": tokens, "last_refresh": datetime.now(timezone.utc).isoformat()}
    )
    save_json(CODEX_AUTH_PATH, auth)
    return auth

def get_codex_chatgpt_session(force_refresh: bool = False) -> dict[str, str]:
    auth = load_codex_session()
    tokens = _codex_tokens(auth)
    access_token = tokens.get("access_token")
    refresh_token = tokens.get("refresh_token")
    account_id = tokens.get("account_id")
    if not refresh_token or not account_id:
        raise RuntimeError(
            "Codex CLI auth file does not contain a usable ChatGPT session."
        )
    expiry = _jwt_expiry_epoch(access_token) if access_token else None
    if (
        force_refresh
        or not access_token
        or (
            expiry is not None and expiry <= datetime.now(timezone.utc).timestamp() + 60
        )
    ):
        refreshed = refresh_codex_chatgpt_session(refresh_token)
        tokens = _codex_tokens(refreshed)
        access_token = tokens.get("access_token")
        account_id = tokens.get("account_id")
    if not access_token or not account_id:
        raise RuntimeError(
            "Codex ChatGPT session is missing access token or account ID"
        )
    return {
        "access_token": access_token,
        "refresh_token": refresh_token,
        "account_id": account_id,
    }

def load_codex_model_list() -> list[str]:
    data = load_json(CODEX_MODELS_CACHE_PATH, {})
    return _extract_model_ids(
        data.get("models") if isinstance(data, dict) else None,
        "id",
        "name",
        "slug",
        "model",
        "model_id",
    )

# ---------------------------------------------------------------------------
# Provider client factories and protocol adapters
# ---------------------------------------------------------------------------

def _openai_client_pair(**kwargs: Any) -> tuple[AsyncOpenAI, OpenAI]:
    http_client_kwargs = dict(kwargs)
    http_client_kwargs.pop("max_retries", None)
    async_http = async_http_client(**http_client_kwargs)
    sync_http = http_client(**http_client_kwargs)
    return (
        AsyncOpenAI(http_client=async_http, **kwargs),
        OpenAI(http_client=sync_http, **kwargs),
    )

def _openai_pair(
    api_key: str,
    *,
    base_url: str | None = None,
    max_retries: int = 3,
    timeout: Any = None,
) -> tuple[AsyncOpenAI, OpenAI]:
    kwargs: dict[str, Any] = {"api_key": api_key, "max_retries": max_retries}
    if base_url:
        kwargs["base_url"] = base_url
    if timeout is not None:
        kwargs["timeout"] = timeout
    return _openai_client_pair(**kwargs)

def split_model_spec(spec: str) -> tuple[str | None, str]:
    if ":" in spec:
        shim, _, model = spec.partition(":")
        if shim in set(SHIM_ORDER):
            return shim, model
    return None, spec

def join_model_spec(shim: str, model: str) -> str:
    return f"{shim}:{model}"

# ---------------------------------------------------------------------------
# Retry plumbing
# ---------------------------------------------------------------------------

def _is_retryable_status(status_code: int) -> bool:
    return status_code in {429, 499} or 500 <= status_code < 600

class RetryableHttpError(RuntimeError):
    def __init__(self, response: httpx.Response):
        self.response = response
        super().__init__(f"retryable HTTP {response.status_code}")

def _parse_retry_after_seconds(value: str | None) -> float | None:
    if not value:
        return None
    try:
        seconds = float(value)
    except ValueError:
        try:
            retry_at = parsedate_to_datetime(value)
        except (TypeError, ValueError, IndexError, OverflowError):
            return None
        if retry_at.tzinfo is None:
            retry_at = retry_at.replace(tzinfo=timezone.utc)
        seconds = (retry_at - datetime.now(timezone.utc)).total_seconds()
    return max(0.0, seconds)

def _response_json(response: httpx.Response) -> dict[str, Any] | None:
    try:
        payload = response.json()
    except Exception:
        return None
    return payload if isinstance(payload, dict) else None

class WaitForRetryableResponse(wait_base):
    def __init__(
        self,
        *,
        initial: float = DEFAULT_RETRY_INITIAL_DELAY_SECONDS,
        maximum: float = DEFAULT_RETRY_MAX_DELAY_SECONDS,
        bedrock: bool = False,
    ):
        self.initial = initial
        self.maximum = maximum
        self.bedrock = bedrock

    def __call__(self, retry_state) -> float:
        attempt = max(retry_state.attempt_number, 1)
        base = min(self.maximum, self.initial * (2 ** max(attempt - 1, 0)))
        exc = retry_state.outcome.exception() if retry_state.outcome else None
        if isinstance(exc, RetryableHttpError):
            retry_after_seconds = _parse_retry_after_seconds(
                exc.response.headers.get("retry-after")
            )
            # Bedrock encodes retryAfter in the JSON body, not the HTTP header.
            bedrock_delay = (
                _bedrock_retry_delay_seconds(exc.response) if self.bedrock else None
            )
            chosen = max(
                base,
                retry_after_seconds or 0.0,
                bedrock_delay or 0.0,
            )
            return min(self.maximum, chosen)
        # Transport errors (including timeouts): use a short fixed floor so we
        # don't hammer the endpoint, but also don't wait as long as rate-limit
        # back-off since the server may just have been slow.
        if isinstance(exc, (httpx.TransportError, APIConnectionError, APITimeoutError)):
            return max(TRANSPORT_ERROR_RETRY_DELAY, min(base, self.maximum))
        return base

def _retry_error_context(exc: BaseException | None) -> str | None:
    if isinstance(exc, RetryableHttpError):
        msg = _response_error_message(exc.response)
        return msg or f"HTTP {exc.response.status_code}"
    if isinstance(exc, APIStatusError):
        msg = _response_error_message(exc.response)
        return msg or f"HTTP {exc.response.status_code}"
    if isinstance(exc, APITimeoutError):
        return f"timeout ({type(exc).__name__})"
    if isinstance(exc, APIConnectionError):
        return f"transport error ({type(exc).__name__}): {exc}"
    if isinstance(exc, httpx.TimeoutException):
        return f"timeout ({type(exc).__name__})"
    if isinstance(exc, httpx.TransportError):
        return f"transport error ({type(exc).__name__}): {exc}"
    if exc is not None:
        return str(exc)
    return None

async def _call_with_retry(
    call,
    *,
    max_attempts: int = DEFAULT_RETRY_MAX_ATTEMPTS,
    on_retry=None,
    bedrock: bool = False,
):
    maximum = (
        BEDROCK_RETRY_MAX_DELAY_SECONDS if bedrock else DEFAULT_RETRY_MAX_DELAY_SECONDS
    )
    async for attempt in AsyncRetrying(
        stop=stop_after_attempt(max_attempts),
        wait=WaitForRetryableResponse(maximum=maximum, bedrock=bedrock),
        retry=retry_if_exception_type(
            (APIConnectionError, APITimeoutError, RetryableHttpError)
        ),
        reraise=True,
    ):
        with attempt:
            if on_retry and attempt.retry_state.attempt_number > 1:
                on_retry(
                    attempt.retry_state.attempt_number,
                    max_attempts,
                    _retry_error_context(
                        attempt.retry_state.outcome.exception()
                        if attempt.retry_state.outcome
                        else None
                    ),
                )
            try:
                return await call()
            except APIStatusError as exc:
                if _is_retryable_status(exc.response.status_code):
                    raise RetryableHttpError(exc.response) from exc
                raise
    raise RuntimeError("SDK retry loop exited unexpectedly")

def _response_error_message(response: httpx.Response) -> str:
    payload = _response_json(response)
    if isinstance(payload, dict):
        # OpenAI/Anthropic style: {"error": {"message": "..."}}
        error = payload.get("error")
        if isinstance(error, dict) and isinstance(error.get("message"), str):
            return error["message"]
        # Bedrock style: {"message": "...", "__type": "..."}
        top_msg = payload.get("message")
        if isinstance(top_msg, str) and top_msg:
            error_type = payload.get("__type") or payload.get("code") or ""
            return f"{error_type}: {top_msg}" if error_type else top_msg
    return response.text

def _bedrock_retry_delay_seconds(response: httpx.Response) -> float | None:
    payload = _response_json(response)
    if not isinstance(payload, dict):
        return None
    retry_after = payload.get("retryAfter")
    if isinstance(retry_after, (int, float)) and retry_after > 0:
        return float(retry_after)
    return None

def _responses_instructions(messages: list[ChatMessage]) -> str | None:
    parts = [msg.content for msg in messages if isinstance(msg, SystemMessage)]
    joined = "\n\n".join(part for part in parts if part)
    return joined or None

def _responses_input_from_messages(messages: list[ChatMessage]) -> list[dict[str, Any]]:
    items: list[dict[str, Any]] = []
    for msg in messages:
        match msg:
            case SystemMessage():
                continue
            case UserMessage():
                items.append(
                    {"type": "message", "role": "user", "content": msg.content}
                )
            case AssistantMessage():
                if msg.content:
                    items.append(
                        {
                            "type": "message",
                            "role": "assistant",
                            "content": msg.content,
                        }
                    )
                items.extend(
                    {
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.name,
                        "arguments": serialize_json(call.arguments),
                        "status": "completed",
                    }
                    for call in msg.tool_calls
                )
            case ToolMessage():
                items.append(
                    {
                        "type": "function_call_output",
                        "call_id": msg.tool_call_id,
                        "output": _tool_output_text(msg.content),
                    }
                )
    return items

def _responses_tools(tools: list[ToolSpec] | None) -> list[dict[str, Any]] | None:
    result = [
        {
            "type": "function",
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters or {"type": "object"},
            "strict": False,
        }
        for tool in tools or []
    ]
    return result or None

def _decode_tool_call_arguments(arguments: Any) -> dict[str, Any]:
    # Provider boundaries are loose here: some SDKs hand back dicts, some
    # strings, and some malformed duplicated JSON. Normalize before the rest of
    # the tool loop touches it.
    if isinstance(arguments, dict):
        return arguments
    if arguments in (None, ""):
        return {}
    if not isinstance(arguments, str):
        raise RuntimeError("Tool arguments must be a JSON object or JSON string")

    def decode(candidate: str) -> dict[str, Any]:
        parsed = msgspec.json.decode(candidate)
        parsed = msgspec.json.decode(parsed) if isinstance(parsed, str) else parsed
        if not isinstance(parsed, dict):
            raise RuntimeError("Tool arguments must decode to a JSON object")
        return parsed

    try:
        return decode(arguments)
    except (msgspec.DecodeError, RuntimeError) as exc:
        # Some providers duplicate a JSON blob — the first copy is often
        # truncated (missing close brace) and the second is the valid one.
        # Scan near the midpoint for `{` and try decoding from there.
        mid = len(arguments) // 2
        for i in range(max(0, mid - 40), min(len(arguments), mid + 40)):
            if arguments[i] == "{":
                try:
                    return decode(arguments[i:])
                except (msgspec.DecodeError, RuntimeError):
                    pass
        raise RuntimeError(f"Could not parse tool arguments JSON: {exc}") from exc

def _drop_reasoning_arg(payload: dict[str, Any]) -> dict[str, Any]:
    stripped = dict(payload)
    stripped.pop("reasoning", None)
    stripped.pop("reasoning_effort", None)
    return stripped

# Thread-safe cache for reasoning support per (api_kind, model).
# Background threads (/ask, /audit) may probe this concurrently.
_REASONING_SUPPORT_CACHE: dict[tuple[str, str], bool] = {}
_REASONING_CACHE_LOCK = _threading.Lock()

def _should_send_reasoning(api_kind: str, model: str) -> bool:
    with _REASONING_CACHE_LOCK:
        return _REASONING_SUPPORT_CACHE.get((api_kind, model), True)

def _mark_reasoning_unsupported(api_kind: str, model: str) -> None:
    with _REASONING_CACHE_LOCK:
        _REASONING_SUPPORT_CACHE[(api_kind, model)] = False

def _is_reasoning_unsupported_error(exc: APIStatusError) -> bool:
    if exc.response.status_code != 400:
        return False
    message = (_response_error_message(exc.response) or "").lower()
    return "reasoning" in message and any(
        token in message
        for token in (
            "unsupported",
            "unknown parameter",
            "not allowed",
            "not supported",
            "invalid parameter",
            "extra inputs",
        )
    )

async def _call_with_reasoning_fallback(
    api_kind: str,
    model: str,
    payload: dict[str, Any],
    create,
    *,
    on_retry=None,
    bedrock: bool = False,
):
    if not _should_send_reasoning(api_kind, model):
        payload = _drop_reasoning_arg(payload)
    try:
        return await _call_with_retry(
            lambda: create(payload), on_retry=on_retry, bedrock=bedrock
        )
    except APIStatusError as exc:
        if not _is_reasoning_unsupported_error(exc):
            raise
        _mark_reasoning_unsupported(api_kind, model)
        return await _call_with_retry(
            lambda: create(_drop_reasoning_arg(payload)),
            on_retry=on_retry,
            bedrock=bedrock,
        )

def _responses_payload(
    model: str,
    messages: list[ChatMessage],
    tools: list[ToolSpec] | None,
    tool_choice: str,
) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "model": model,
        "input": _responses_input_from_messages(messages),
        "store": False,
    }
    payload["reasoning"] = {"effort": "high"}
    instructions = _responses_instructions(messages)
    if instructions:
        payload["instructions"] = instructions
    response_tools = _responses_tools(tools)
    if response_tools:
        payload["tools"] = response_tools
        payload["tool_choice"] = tool_choice
        payload["parallel_tool_calls"] = True
    return payload

def _decode_responses_output(response: Any) -> AssistantMessage:
    data = (
        response.model_dump(exclude_none=True)
        if hasattr(response, "model_dump")
        else response
    )
    if not isinstance(data, dict):
        raise RuntimeError("Responses API returned an unexpected payload")
    content_parts: list[str] = []
    tool_calls: list[ToolCall] = []
    for item in data.get("output") or []:
        if not isinstance(item, dict):
            continue
        item_type = item.get("type")
        if item_type == "message" and item.get("role") == "assistant":
            for part in item.get("content") or []:
                if not isinstance(part, dict):
                    continue
                text = part.get("text")
                refusal = part.get("refusal")
                if isinstance(text, str) and not _is_blank_chat_value(text):
                    content_parts.append(text)
                elif isinstance(refusal, str) and not _is_blank_chat_value(refusal):
                    content_parts.append(refusal)
        elif item_type == "function_call":
            call_id = item.get("call_id") or item.get("id")
            if not isinstance(call_id, str) or not call_id:
                continue
            tool_calls.append(
                ToolCall(
                    id=call_id,
                    name=item.get("name") or "",
                    arguments=_decode_tool_call_arguments(item.get("arguments")),
                )
            )
    output_text = data.get("output_text")
    if (
        not content_parts
        and isinstance(output_text, str)
        and not _is_blank_chat_value(output_text)
    ):
        content_parts.append(output_text)
    return AssistantMessage(
        content="\n\n".join(content_parts),
        tool_calls=tool_calls,
    )

def _http_error_message(prefix: str, response: httpx.Response) -> str:
    try:
        data = response.json()
    except ValueError:
        body = response.text.strip()
        body = body[:200] if body else ""
        return (
            f"{prefix} error {response.status_code}: {body or response.reason_phrase}"
        )
    detail = data.get("error") or data.get("detail") if isinstance(data, dict) else data
    if isinstance(detail, dict):
        message = detail.get("message") or detail.get("code") or json.dumps(detail)
    elif isinstance(detail, str):
        message = detail
    else:
        message = json.dumps(detail, ensure_ascii=True)
    return f"{prefix} error {response.status_code}: {message}"

async def _read_sse_completed_response(response: httpx.Response) -> dict[str, Any]:
    event_name = ""
    data_lines: list[str] = []

    async def flush_event() -> dict[str, Any] | None:
        nonlocal event_name, data_lines
        if not data_lines:
            event_name = ""
            return None
        raw = "\n".join(data_lines)
        data_lines = []
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            event_name = ""
            return None
        current_event = event_name
        event_name = ""
        if current_event == "response.completed":
            completed = payload.get("response")
            if isinstance(completed, dict):
                return completed
        return None

    async for line in response.aiter_lines():
        if not line:
            completed = await flush_event()
            if completed is not None:
                return completed
            continue
        if line.startswith("event:"):
            event_name = line[6:].strip()
            continue
        if line.startswith("data:"):
            data_lines.append(line[5:].strip())
    completed = await flush_event()
    if completed is not None:
        return completed
    raise RuntimeError("Codex ChatGPT stream ended before response.completed")

BEDROCK_CODEC = ProviderCodec(
    user_role="user",
    assistant_role="assistant",
    content_key="content",
    system_item=lambda text: {"text": text},
    finalize_system=lambda parts: parts or None,
    encode_text=lambda text: {"text": text},
    encode_tool_use=lambda block: {
        "toolUse": {
            "toolUseId": block.id,
            "name": block.name,
            "input": block.arguments,
        }
    },
    encode_tool_result=lambda block: {
        "toolResult": {
            "toolUseId": block.id,
            "content": [
                {"text": _tool_output_text(block.result).strip() or "(no output)"}
            ],
            "status": "error" if not block.result.ok else "success",
        }
    },
)

def _sync_model_ids(
    sync_client: OpenAI,
    *,
    fallback: Callable[[], list[str]] | None = None,
    default: list[str] | None = None,
) -> list[str]:
    try:
        return sorted(model.id for model in list(sync_client.models.list()))
    except Exception:
        if fallback:
            cached = fallback()
            if cached:
                return cached
        if default is not None:
            return default
        raise

def _openai_responses_client(
    async_client: AsyncOpenAI,
    sync_client: OpenAI,
    *,
    fallback_models: Callable[[], list[str]] | None = load_codex_model_list,
    default_models: list[str] | None = None,
    bedrock: bool = False,
) -> CompletionClient:
    async def chat_completion(
        model: str,
        messages: list[ChatMessage],
        tools: list[ToolSpec] | None = None,
        tool_choice: str = "auto",
        on_retry=None,
    ) -> AssistantMessage:
        client = async_client.with_options(max_retries=0)

        async def create_response(payload: dict[str, Any]):
            return await client.responses.create(**payload)

        payload = _responses_payload(model, messages, tools, tool_choice)
        response = await _call_with_reasoning_fallback(
            "responses", model, payload, create_response,
            on_retry=on_retry, bedrock=bedrock,
        )
        return _decode_responses_output(response)

    return CompletionClient(
        chat_completion=chat_completion,
        list_models=lambda: _sync_model_ids(
            sync_client, fallback=fallback_models, default=default_models
        ),
    )

def _message_like_dict(value: Any) -> dict[str, Any]:
    if isinstance(value, dict):
        return {key: item for key, item in value.items() if item is not None}
    if hasattr(value, "model_dump"):
        data = value.model_dump(exclude_none=True)
        if isinstance(data, dict):
            return data
    return {}

def _chat_completion_message_dict(message: Any) -> dict[str, Any]:
    if data := _message_like_dict(message):
        return data
    result: dict[str, Any] = {}
    for key in (
        "role",
        "content",
        "tool_calls",
        "refusal",
        "reasoning_text",
        "reasoning_opaque",
    ):
        value = getattr(message, key, None)
        if key == "tool_calls":
            if isinstance(value, list):
                result[key] = value
        elif isinstance(value, str):
            result[key] = value
    return result

def _chat_completion_tool_call(tool_call: Any) -> ToolCall | None:
    data = _message_like_dict(tool_call)
    if data:
        call_id = data.get("id")
        function = data.get("function")
    else:
        call_id = getattr(tool_call, "id", None)
        function = getattr(tool_call, "function", None)
    if not isinstance(call_id, str) or not call_id:
        return None
    function_data = _message_like_dict(function)
    if function_data:
        name = function_data.get("name")
        arguments = function_data.get("arguments")
    else:
        name = getattr(function, "name", None)
        arguments = getattr(function, "arguments", None)
    if not isinstance(name, str) or not name:
        return None
    return ToolCall(
        id=call_id,
        name=name,
        arguments=_decode_tool_call_arguments(arguments),
    )

def _is_blank_chat_value(value: Any) -> bool:
    return value is None or value == "" or value == [] or value == {} or (
        isinstance(value, str) and not value.strip()
    )

def _merged_chat_completion_message(choices: list[Any]) -> dict[str, Any]:
    merged: dict[str, Any] = {}
    for choice in choices:
        message = _chat_completion_message_dict(getattr(choice, "message", None))
        if not message:
            continue
        candidate = dict(merged)
        for key, value in message.items():
            if key not in candidate or _is_blank_chat_value(candidate[key]):
                candidate[key] = value
                continue
            if _is_blank_chat_value(value) or candidate[key] == value:
                continue
            return merged or message
        merged = candidate
    return merged

def _chat_completion_to_assistant_message(response: Any) -> AssistantMessage:
    choices = getattr(response, "choices", None)
    message = (
        _merged_chat_completion_message(choices)
        if isinstance(choices, list) and len(choices) > 1
        else _chat_completion_message_dict(getattr(choices[0], "message", None))
        if isinstance(choices, list) and choices
        else {}
    )
    return AssistantMessage(
        content=message.get("content") if isinstance(message.get("content"), str) else "",
        tool_calls=[
            call
            for tool_call in message.get("tool_calls") or []
            if (call := _chat_completion_tool_call(tool_call)) is not None
        ],
    )

def _openai_chat_completions_client(
    async_client: AsyncOpenAI,
    sync_client: OpenAI,
    *,
    tools_map: Callable[[list[ToolSpec]], list[dict[str, Any]]],
    bedrock: bool = False,
) -> CompletionClient:
    async def chat_completion(
        model: str,
        messages: list[ChatMessage],
        tools: list[ToolSpec] | None = None,
        tool_choice: str = "auto",
        on_retry=None,
    ) -> AssistantMessage:
        client = async_client.with_options(max_retries=0)
        kwargs: dict[str, Any] = {
            "model": model,
            "messages": [_openai_chat_message(message) for message in messages],
            "reasoning_effort": "high",
        }
        if tools:
            kwargs["tools"] = tools_map(tools)
            kwargs["tool_choice"] = tool_choice

        async def create_response(payload: dict[str, Any]):
            return await client.chat.completions.create(**payload)

        response = await _call_with_reasoning_fallback(
            "chat_completions", model, kwargs, create_response,
            on_retry=on_retry, bedrock=bedrock,
        )
        return _chat_completion_to_assistant_message(response)

    return CompletionClient(
        chat_completion=chat_completion,
        list_models=lambda: _sync_model_ids(sync_client, fallback=None),
    )

def _bedrock_tools(tools: list[ToolSpec], tool_choice: str) -> dict[str, Any] | None:
    bedrock_tools = [
        {
            "toolSpec": {
                "name": tool.name,
                "description": tool.description,
                "inputSchema": {"json": tool.parameters or {"type": "object"}},
            }
        }
        for tool in tools
    ]
    if not bedrock_tools or tool_choice == "none":
        return None
    return {
        "tools": bedrock_tools,
        "toolChoice": {"any": {}} if tool_choice == "required" else {"auto": {}},
    }

def _bedrock_output_blocks(data: dict[str, Any]) -> list[ContentBlock]:
    return _extract_blocks(
        data["output"]["message"]["content"],
        text_of=lambda item: (
            item.get("text") if isinstance(item.get("text"), str) else None
        ),
        tool_of=lambda item, _: (
            ToolUseBlock(
                id=item["toolUse"]["toolUseId"],
                name=item["toolUse"]["name"],
                arguments=item["toolUse"].get("input", {}),
            )
            if "toolUse" in item
            else None
        ),
    )

def _boto3_bedrock_model_ids(region: str) -> list[str]:
    import boto3

    client = boto3.client("bedrock", region_name=region)
    ids: list[str] = []
    # Foundation models
    try:
        resp = client.list_foundation_models()
        for summary in resp.get("modelSummaries", []):
            model_id = summary.get("modelId", "")
            if "TEXT" in summary.get("outputModalities", []) and model_id.startswith(
                ("global.", "us.")
            ):
                ids.append(model_id)
    except Exception:
        pass
    # Inference profiles
    try:
        resp = client.list_inference_profiles()
        for summary in resp.get("inferenceProfileSummaries", []):
            profile_id = summary.get("inferenceProfileId", "")
            if profile_id.startswith(("global.", "us.")):
                ids.append(profile_id)
    except Exception:
        pass
    return ids

def _bedrock_converse_client(region: str) -> CompletionClient:
    import asyncio
    import boto3
    from botocore.config import Config as BotoConfig

    config = BotoConfig(
        read_timeout=float(os.environ.get("OY_BEDROCK_READ_TIMEOUT", "120")),
        retries={"max_attempts": DEFAULT_RETRY_MAX_ATTEMPTS, "mode": "adaptive"},
    )
    rt_client = boto3.client("bedrock-runtime", region_name=region, config=config)

    async def chat_completion(
        model: str,
        messages: list[ChatMessage],
        tools: list[ToolSpec] | None = None,
        tool_choice: str = "auto",
        on_retry=None,
    ) -> AssistantMessage:
        _ = on_retry
        bedrock_messages, system_prompts = _encode_provider_messages(
            messages, BEDROCK_CODEC
        )
        kwargs: dict[str, Any] = {
            "modelId": model,
            "messages": bedrock_messages,
            "inferenceConfig": {
                "maxTokens": int(os.environ.get("OY_BEDROCK_MAX_OUTPUT_TOKENS", "4096"))
            },
        }
        if system_prompts:
            kwargs["system"] = system_prompts
        if tools and (tool_config := _bedrock_tools(tools, tool_choice)):
            kwargs["toolConfig"] = tool_config
        # boto3 is synchronous; run in executor to avoid blocking the event loop
        loop = asyncio.get_running_loop()
        try:
            data = await loop.run_in_executor(
                None, lambda: rt_client.converse(**kwargs)
            )
        except Exception as exc:
            raise RuntimeError(f"Bedrock converse error: {exc}") from exc
        return _assistant_from_blocks(_bedrock_output_blocks(data))

    def list_models() -> list[str]:
        return sorted(unique_strings(_boto3_bedrock_model_ids(region)))

    return CompletionClient(
        chat_completion=chat_completion,
        list_models=list_models,
    )

def _tool_specs_to_openai(tools: list[ToolSpec]) -> list[dict[str, Any]]:
    return [
        {
            "type": "function",
            "function": {
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            },
        }
        for tool in tools
    ]

def _codex_chatgpt_client() -> CompletionClient:
    async def chat_completion(
        model: str,
        messages: list[ChatMessage],
        tools: list[ToolSpec] | None = None,
        tool_choice: str = "auto",
        on_retry=None,
    ) -> AssistantMessage:
        _ = on_retry
        payload = _responses_payload(model, messages, tools, tool_choice)
        payload["stream"] = True
        session = get_codex_chatgpt_session()
        for attempt in range(2):
            try:
                async with async_http_client(timeout=60) as client:
                    async with client.stream(
                        "POST",
                        CODEX_CHATGPT_RESPONSES_URL,
                        json=payload,
                        headers={
                            "Authorization": f"Bearer {session['access_token']}",
                            "ChatGPT-Account-Id": session["account_id"],
                            "Content-Type": "application/json",
                            "Accept": "text/event-stream",
                        },
                    ) as response:
                        if response.status_code == 401 and attempt == 0:
                            session = get_codex_chatgpt_session(force_refresh=True)
                            continue
                        if response.status_code >= 400:
                            body = await response.aread()
                            buffered = httpx.Response(
                                response.status_code,
                                headers=response.headers,
                                content=body,
                                request=response.request,
                            )
                            raise RuntimeError(
                                _http_error_message("Codex ChatGPT", buffered)
                            )
                        data = await _read_sse_completed_response(response)
            except httpx.HTTPError as exc:
                raise RuntimeError(f"Codex ChatGPT request failed: {exc}") from exc
            return _decode_responses_output(data)
        raise RuntimeError("Codex ChatGPT authentication failed after token refresh")

    return CompletionClient(
        chat_completion=chat_completion,
        list_models=lambda: load_codex_model_list() or [CODEX_DEFAULT_MODEL],
    )

__all__ = [
    "command_env",
    "default_region",
    "join_model_spec",
    "load_json",
    "run_cmd",
    "save_json",
    "split_model_spec",
    "which",
]
