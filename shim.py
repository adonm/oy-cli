from __future__ import annotations
import base64
import hashlib
import hmac
import json
import os
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from functools import lru_cache
from pathlib import Path
from typing import Any, Awaitable, Callable, TypeAlias
from urllib.parse import quote
import httpx
import httpx_aws_auth
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


SHIM_OPENAI = "openai"
SHIM_CODEX = "codex"
SHIM_GEMINI = "gemini"
SHIM_BEDROCK = "bedrock"
SHIM_MANTLE = "bedrock-mantle"
SHIM_CLAUDE = "claude"
SHIM_ORDER = (
    SHIM_OPENAI,
    SHIM_CODEX,
    SHIM_GEMINI,
    SHIM_BEDROCK,
    SHIM_MANTLE,
    SHIM_CLAUDE,
)
KNOWN_SHIMS = set(SHIM_ORDER)

DEFAULT_REGION = (
    os.environ.get("AWS_REGION") or os.environ.get("AWS_DEFAULT_REGION") or "us-east-1"
)
SSO_MARKERS = (
    "error loading sso token",
    "the sso session associated with this profile has expired",
    "the sso session has expired or is otherwise invalid",
    "to refresh this sso session run aws sso login",
)
CODEX_AUTH_PATH = Path.home() / ".codex" / "auth.json"
CODEX_MODELS_CACHE_PATH = Path.home() / ".codex" / "models_cache.json"
GEMINI_CREDS_PATH = Path.home() / ".gemini" / "oauth_creds.json"
CLAUDE_CREDS_PATH = Path.home() / ".claude" / ".credentials.json"
CODEX_DEFAULT_MODEL = "gpt-5-codex"
CODEX_CHATGPT_RESPONSES_URL = "https://chatgpt.com/backend-api/codex/responses"
CODEX_OAUTH_TOKEN_URL = "https://auth.openai.com/oauth/token"
CODEX_OAUTH_CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann"
GEMINI_CODE_ASSIST_ENDPOINT = "https://cloudcode-pa.googleapis.com"
GEMINI_CODE_ASSIST_VERSION = "v1internal"
GEMINI_OAUTH_CLIENT_ID = (
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com"
)
GEMINI_OAUTH_CLIENT_SECRET = "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"
CLAUDE_API_URL = "https://api.anthropic.com"
CLAUDE_TOKEN_URL = "https://platform.claude.com/v1/oauth/token"
CLAUDE_OAUTH_CLIENT_ID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
ANTHROPIC_VERSION = "2023-06-01"
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

JSONLike: TypeAlias = dict[str, Any] | list[Any] | str | int | float | bool | None
JSONDict: TypeAlias = dict[str, Any]
ProviderItem: TypeAlias = JSONDict | str


class ToolCall(msgspec.Struct, omit_defaults=True):
    id: str
    name: str
    arguments: dict[str, Any] = msgspec.field(default_factory=dict)


class ToolResult(msgspec.Struct, omit_defaults=True):
    ok: bool = True
    content: JSONLike = None


class ToolSpec(msgspec.Struct, omit_defaults=True):
    name: str
    description: str
    parameters: dict[str, Any]


class SystemMessage(msgspec.Struct, tag="system", tag_field="role", omit_defaults=True):
    content: str


class UserMessage(msgspec.Struct, tag="user", tag_field="role", omit_defaults=True):
    content: str


class AssistantMessage(
    msgspec.Struct, tag="assistant", tag_field="role", omit_defaults=True
):
    content: str = ""
    tool_calls: list[ToolCall] = msgspec.field(default_factory=list)
    thought_signatures: dict[str, str] = msgspec.field(default_factory=dict)


class ToolMessage(msgspec.Struct, tag="tool", tag_field="role", omit_defaults=True):
    tool_call_id: str
    name: str = ""
    content: ToolResult = msgspec.field(default_factory=ToolResult)


ChatMessage: TypeAlias = SystemMessage | UserMessage | AssistantMessage | ToolMessage


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
HttpRequestBuilder: TypeAlias = Callable[
    [str, list[ChatMessage], list[ToolSpec] | None, str], "HttpRequest"
]
HttpResponseDecoder: TypeAlias = Callable[[JSONDict], AssistantMessage]
HttpErrorFormatter: TypeAlias = Callable[[httpx.Response], str]
HttpRetryDecider: TypeAlias = Callable[[httpx.Response], bool]


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


@dataclass(frozen=True, slots=True)
class HttpRequest:
    url: str
    json_body: Any = None
    content_body: str | None = None
    headers: dict[str, str] | None = None


@dataclass(frozen=True, slots=True)
class HttpChatEndpoint:
    make_client: Callable[[], httpx.AsyncClient]
    build_request: HttpRequestBuilder
    decode: HttpResponseDecoder
    error: HttpErrorFormatter
    timeout: Any
    should_retry_response: HttpRetryDecider | None = None
    refresh_auth: Callable[[], None] | None = None
    bedrock: bool = False


def _normalize_jsonlike(value: Any) -> JSONLike:
    return (
        value
        if isinstance(value, (dict, list, str, int, float, bool)) or value is None
        else str(value)
    )


def _json_text(value: Any) -> str:
    return (
        value
        if isinstance(value, str)
        else msgspec.json.encode(value).decode("utf-8")
    )


def _tool_output_value(result: ToolResult) -> JSONLike:
    return _normalize_jsonlike(result.content)


def _tool_output_text(result: ToolResult) -> str:
    return _json_text(_tool_output_value(result))


def _assistant_blocks(message: AssistantMessage) -> list[ContentBlock]:
    blocks: list[ContentBlock] = [TextBlock(message.content)] if message.content else []
    blocks.extend(
        ToolUseBlock(
            id=call.id,
            name=call.name,
            arguments=call.arguments,
            thought_signature=message.thought_signatures.get(call.id, ""),
        )
        for call in message.tool_calls
    )
    return blocks


def _tool_message_block(message: ToolMessage) -> ToolResultBlock:
    return ToolResultBlock(id=message.tool_call_id, name=message.name, result=message.content)


def _assistant_from_blocks(blocks: list[ContentBlock]) -> AssistantMessage:
    content = ""
    tool_calls: list[ToolCall] = []
    thought_signatures: dict[str, str] = {}
    for block in blocks:
        match block:
            case TextBlock():
                content += block.text
            case ToolUseBlock():
                tool_calls.append(
                    ToolCall(id=block.id, name=block.name, arguments=block.arguments)
                )
                if block.thought_signature:
                    thought_signatures[block.id] = block.thought_signature
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
            blocks: list[ContentBlock] = [TextBlock(message.content)] if message.content else []
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
        if text := text_of(item):
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
            "arguments": _json_text(tool_call.arguments),
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
            return payload
        case ToolMessage():
            return {
                "role": "tool",
                "tool_call_id": message.tool_call_id,
                "name": message.name,
                "content": _tool_output_text(message.content),
            }
    raise TypeError(f"Unsupported message type: {type(message).__name__}")


def load_json(p, d):
    try:
        return json.loads(p.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return d


def save_json(p, d):
    try:
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(json.dumps(d, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return True
    except OSError:
        return False


def split_path(v):
    return [e for e in (v or "").split(os.pathsep) if e]


def merge_paths(*groups):
    merged, seen = [], set()
    for g in groups:
        for e in g:
            k = os.path.normcase(os.path.normpath(e))
            if e and k not in seen:
                seen.add(k)
                merged.append(e)
    return os.pathsep.join(merged)


def unique_strings(v):
    return list(dict.fromkeys(x for x in v if x))


def _read_json_dict(path: Path, error_prefix: str) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"{error_prefix}: {exc}") from exc
    if not isinstance(data, dict):
        raise RuntimeError(f"{error_prefix}: expected a JSON object")
    return data


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


def _valid_access_token(
    access_token: Any, expires_at_ms: Any, *, skew_ms: int = 0
) -> str | None:
    if (
        isinstance(access_token, str)
        and access_token
        and isinstance(expires_at_ms, (int, float))
        and expires_at_ms > time.time() * 1000 + skew_ms
    ):
        return access_token
    return None


def _persist_json_dict(path: Path, update: Callable[[dict[str, Any]], None]) -> None:
    existing = load_json(path, {})
    if isinstance(existing, dict):
        update(existing)
        save_json(path, existing)


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
        _first_nonempty_string(item, *keys)
        for item in items
        if isinstance(item, dict)
    )


def _fetch_json_ids(
    client: httpx.Client,
    url: str,
    *,
    items_key: str,
    id_key: str,
    headers: dict[str, str] | None = None,
    params: dict[str, Any] | None = None,
    has_more_key: str | None = None,
    cursor_key: str | None = None,
    cursor_param: str | None = None,
    item_filter: Callable[[dict[str, Any]], bool] | None = None,
    raise_errors: bool = False,
) -> list[str]:
    request_params = dict(params or {})
    ids: list[str] = []
    while True:
        response = client.get(url, headers=headers, params=request_params or None)
        if response.is_error:
            if raise_errors:
                response.raise_for_status()
            return []
        try:
            data = response.json()
        except ValueError:
            if raise_errors:
                raise
            return []
        if not isinstance(data, dict):
            if raise_errors:
                raise RuntimeError(f"Invalid JSON response from {url}")
            return []
        ids.extend(
            item[id_key]
            for item in data.get(items_key, [])
            if isinstance(item, dict)
            and isinstance(item.get(id_key), str)
            and (item_filter(item) if item_filter else True)
        )
        if not has_more_key or not data.get(has_more_key):
            return ids
        cursor = data.get(cursor_key or "")
        if not isinstance(cursor, str) or not cursor:
            return ids
        request_params[cursor_param or cursor_key or "cursor"] = cursor


def expiry_ms(s, *, skew=60):
    try:
        return int((time.time() + float(s) - skew) * 1000)
    except (TypeError, ValueError):
        return int((time.time() + 3600.0 - skew) * 1000)


def which(t, p=None):
    return shutil.which(t, path=p)


def run_cmd(cmd, cwd=None, env=None, timeout=120, stdin_text=None):
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


@lru_cache(maxsize=8)
def command_env(cwd=None):
    env = os.environ.copy()
    if brew := which("brew", env.get("PATH")):
        prefix = Path(brew).parent.parent
        env["HOMEBREW_PREFIX"] = str(prefix)
        env["PATH"] = merge_paths(
            [str(prefix / "bin"), str(prefix / "sbin")], split_path(env.get("PATH"))
        )
    if not (mise := which("mise", env.get("PATH"))):
        return env
    try:
        result = run_cmd(
            [mise, "env", "--json"],
            cwd=cwd if cwd and cwd.is_dir() else None,
            env=env,
            timeout=5,
        )
        data = json.loads(result.stdout) if result.returncode == 0 else {}
    except (OSError, ValueError, json.JSONDecodeError):
        return env
    if not isinstance(data, dict):
        return env
    merged = env.copy()
    for key, value in data.items():
        if not isinstance(value, str):
            continue
        merged[key] = (
            merge_paths(split_path(value), split_path(env.get("PATH")))
            if key == "PATH"
            else value
        )
    return merged


def http_client(**kw):
    return httpx.Client(follow_redirects=True, **kw)


def async_http_client(**kw):
    return httpx.AsyncClient(follow_redirects=True, **kw)


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


def resolve_tool_path(t, cwd=None):
    return which(t, command_env(cwd).get("PATH")) or which(t)


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


def current_region(choice: str | None = None) -> str:
    return (
        choice
        or os.environ.get("AWS_REGION")
        or os.environ.get("AWS_DEFAULT_REGION")
        or DEFAULT_REGION
    )


def default_region() -> str:
    return current_region()


def validate_shim(shim: str) -> str:
    if shim not in KNOWN_SHIMS:
        raise RuntimeError(
            f"Unknown shim value: `{shim}`. Use one of: {', '.join(SHIM_ORDER)}"
        )
    return shim


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


@lru_cache(maxsize=1)
def gemini_cli_package_root() -> Path:
    exe = resolve_tool_path("gemini")
    if not exe:
        raise RuntimeError(
            "Gemini CLI is not installed or not on PATH. "
            "Install it with `npm i -g @google/gemini-cli` and run `gemini` once to authenticate."
        )
    real = Path(exe).resolve()
    root = real.parent.parent
    core = root / "node_modules" / "@google" / "gemini-cli-core"
    if not core.is_dir():
        raise RuntimeError(
            f"Gemini CLI package structure not recognised at {root}. "
            "Re-install with `npm i -g @google/gemini-cli`."
        )
    return root


def _read_gemini_cli_file(relative: str) -> str:
    root = gemini_cli_package_root()
    path = (
        root
        / "node_modules"
        / "@google"
        / "gemini-cli-core"
        / "dist"
        / "src"
        / relative
    )
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        raise RuntimeError(f"Cannot read Gemini CLI file {path}: {exc}") from exc


@lru_cache(maxsize=1)
def load_gemini_oauth_client() -> tuple[str, str]:
    client_id = os.environ.get("GEMINI_OAUTH_CLIENT_ID") or GEMINI_OAUTH_CLIENT_ID
    client_secret = (
        os.environ.get("GEMINI_OAUTH_CLIENT_SECRET") or GEMINI_OAUTH_CLIENT_SECRET
    )
    return client_id, client_secret


@lru_cache(maxsize=1)
def load_gemini_model_list() -> list[str]:
    text = _read_gemini_cli_file("config/models.js")
    result = unique_strings(re.findall(r"=\s*'(gemini-[^']+)'", text))
    if not result:
        raise RuntimeError(
            "Could not parse model list from Gemini CLI. "
            "Re-install with `npm i -g @google/gemini-cli`."
        )
    return result


def load_gemini_oauth_creds() -> dict[str, Any]:
    data = _read_json_dict(GEMINI_CREDS_PATH, "Cannot read Gemini OAuth credentials")
    _require_string(
        data.get("refresh_token"), "Gemini OAuth credentials missing refresh_token"
    )
    return data


def refresh_gemini_token(refresh_token: str) -> str:
    client_id, client_secret = load_gemini_oauth_client()
    data = _post_form_json(
        "https://oauth2.googleapis.com/token",
        {
            "client_id": client_id,
            "client_secret": client_secret,
            "refresh_token": refresh_token,
            "grant_type": "refresh_token",
        },
        error_prefix="Gemini token refresh failed",
    )
    access_token = _require_string(
        data.get("access_token"),
        "Gemini token refresh did not return an access_token",
    )
    _persist_json_dict(
        GEMINI_CREDS_PATH,
        lambda existing: existing.update(
            {
                "access_token": access_token,
                "expiry_date": expiry_ms(data.get("expires_in", 3600)),
            }
        ),
    )
    return access_token


def get_gemini_access_token() -> str:
    creds = load_gemini_oauth_creds()
    if access_token := _valid_access_token(
        creds.get("access_token"), creds.get("expiry_date", 0)
    ):
        return access_token
    return refresh_gemini_token(
        _require_string(
            creds.get("refresh_token"), "Gemini OAuth credentials missing refresh_token"
        )
    )


def resolve_gemini_project() -> str:
    token = get_gemini_access_token()
    url = f"{GEMINI_CODE_ASSIST_ENDPOINT}/{GEMINI_CODE_ASSIST_VERSION}:loadCodeAssist"
    try:
        with http_client(timeout=20) as client:
            resp = client.post(
                url,
                json={},
                headers={
                    "Authorization": f"Bearer {token}",
                    "Content-Type": "application/json",
                },
            )
            resp.raise_for_status()
            data = resp.json()
    except httpx.HTTPError as exc:
        raise RuntimeError(f"Gemini loadCodeAssist failed: {exc}") from exc
    project_id = data.get("cloudaicompanionProject")
    if not isinstance(project_id, str) or not project_id:
        raise RuntimeError("Gemini loadCodeAssist did not return a project ID")
    return project_id


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
    auth.update({"tokens": tokens, "last_refresh": datetime.now(timezone.utc).isoformat()})
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


def load_claude_auth_status() -> dict[str, Any]:
    oauth = _read_json_dict(CLAUDE_CREDS_PATH, "Cannot read Claude credentials").get(
        "claudeAiOauth"
    )
    if not isinstance(oauth, dict) or not _first_nonempty_string(oauth, "accessToken"):
        raise RuntimeError(
            "Claude Code is not logged in. Run `claude` to authenticate."
        )
    return {"loggedIn": True, "oauth": oauth}


def get_claude_access_token() -> str:
    oauth = _read_json_dict(CLAUDE_CREDS_PATH, "Cannot read Claude credentials").get(
        "claudeAiOauth"
    )
    if not isinstance(oauth, dict):
        raise RuntimeError(
            "Claude credentials not found. Run `claude` to authenticate."
        )
    access_token = _first_nonempty_string(oauth, "accessToken")
    if not access_token:
        raise RuntimeError(
            "Claude credentials missing accessToken. Run `claude` to authenticate."
        )
    if _valid_access_token(access_token, oauth.get("expiresAt", 0), skew_ms=30_000):
        return access_token
    refresh_token = _first_nonempty_string(oauth, "refreshToken")
    if not refresh_token:
        raise RuntimeError(
            "Claude session expired and no refresh token available. Run `claude` to re-authenticate."
        )
    return _refresh_claude_token(refresh_token)


def _refresh_claude_token(refresh_token: str) -> str:
    data = _post_form_json(
        CLAUDE_TOKEN_URL,
        {
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLAUDE_OAUTH_CLIENT_ID,
        },
        error_prefix="Claude token refresh failed",
    )
    access_token = _require_string(
        data.get("access_token"),
        "Claude token refresh did not return an access_token",
    )

    def update(existing: dict[str, Any]) -> None:
        oauth = existing.get("claudeAiOauth")
        if not isinstance(oauth, dict):
            return
        oauth["accessToken"] = access_token
        oauth["expiresAt"] = expiry_ms(data.get("expires_in", 3600))
        if token := _first_nonempty_string(data, "refresh_token"):
            oauth["refreshToken"] = token

    _persist_json_dict(CLAUDE_CREDS_PATH, update)
    return access_token


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
    return AsyncOpenAI(**kwargs), OpenAI(**kwargs)


def split_model_spec(spec: str) -> tuple[str | None, str]:
    if ":" in spec:
        shim, _, model = spec.partition(":")
        if shim in KNOWN_SHIMS:
            return shim, model
    return None, spec


def join_model_spec(shim: str, model: str) -> str:
    return f"{shim}:{model}"


@dataclass(frozen=True, slots=True)
class CompletionClient:
    chat_completion: Callable[
        [str, list[ChatMessage], list[ToolSpec] | None, str, Any],
        Awaitable[AssistantMessage],
    ]
    list_models: Callable[[], list[str]]


ShimEnvChecker: TypeAlias = Callable[[Path | None], None]
ShimClientBuilder: TypeAlias = Callable[[str | None, Path | None], CompletionClient]
ShimModelLister: TypeAlias = Callable[[str | None, Path | None], list[str]]


@dataclass(frozen=True, slots=True)
class ShimSpec:
    name: str
    ensure_env: ShimEnvChecker
    build_client: ShimClientBuilder
    list_models: ShimModelLister


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


def _parse_duration_seconds(value: Any) -> float | None:
    if not isinstance(value, str):
        return None
    if value.endswith("ms"):
        try:
            return float(value[:-2]) / 1000.0
        except ValueError:
            return None
    if value.endswith("s"):
        try:
            return float(value[:-1])
        except ValueError:
            return None
    return None


def _response_json(response: httpx.Response) -> dict[str, Any] | None:
    try:
        payload = response.json()
    except Exception:
        return None
    return payload if isinstance(payload, dict) else None


def _google_error_payload(response: httpx.Response) -> dict[str, Any] | None:
    payload = _response_json(response)
    if not payload:
        return None
    error = payload.get("error")
    return error if isinstance(error, dict) else None


def _should_retry_google_response(response: httpx.Response) -> bool:
    if not _is_retryable_status(response.status_code):
        return False
    error = _google_error_payload(response)
    if not error:
        return True
    details = error.get("details")
    if not isinstance(details, list):
        return True
    for detail in details:
        if not isinstance(detail, dict):
            continue
        if detail.get("@type") == "type.googleapis.com/google.rpc.QuotaFailure":
            violations = detail.get("violations")
            if not isinstance(violations, list):
                continue
            for violation in violations:
                if not isinstance(violation, dict):
                    continue
                quota_id = str(violation.get("quotaId") or "")
                if "PerDay" in quota_id or "Daily" in quota_id:
                    return False
        if detail.get("@type") == "type.googleapis.com/google.rpc.ErrorInfo":
            reason = str(detail.get("reason") or "")
            if reason in {"QUOTA_EXHAUSTED", "INSUFFICIENT_G1_CREDITS_BALANCE"}:
                return False
    return True


def _google_retry_delay_seconds(response: httpx.Response) -> float | None:
    error = _google_error_payload(response)
    if error:
        details = error.get("details")
        if isinstance(details, list):
            for detail in details:
                if (
                    isinstance(detail, dict)
                    and detail.get("@type")
                    == "type.googleapis.com/google.rpc.RetryInfo"
                ):
                    retry_delay = _parse_duration_seconds(detail.get("retryDelay"))
                    if retry_delay is not None:
                        return retry_delay
            for detail in details:
                if not isinstance(detail, dict):
                    continue
                if detail.get("@type") == "type.googleapis.com/google.rpc.QuotaFailure":
                    violations = detail.get("violations")
                    if not isinstance(violations, list):
                        continue
                    for violation in violations:
                        if not isinstance(violation, dict):
                            continue
                        quota_id = str(violation.get("quotaId") or "")
                        if "PerMinute" in quota_id:
                            return 60.0
                if detail.get("@type") == "type.googleapis.com/google.rpc.ErrorInfo":
                    metadata = detail.get("metadata")
                    quota_limit = (
                        str(metadata.get("quota_limit") or "")
                        if isinstance(metadata, dict)
                        else ""
                    )
                    if "PerMinute" in quota_limit:
                        return 60.0
                    if str(detail.get("reason") or "") == "RATE_LIMIT_EXCEEDED":
                        return 10.0
        message = error.get("message")
        if isinstance(message, str):
            match = re.search(r"Please retry in ([0-9.]+(?:ms|s))", message)
            if match:
                return _parse_duration_seconds(match.group(1))
    return None


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
            google_delay = _google_retry_delay_seconds(exc.response)
            # Bedrock encodes retryAfter in the JSON body, not the HTTP header.
            bedrock_delay = (
                _bedrock_retry_delay_seconds(exc.response) if self.bedrock else None
            )
            chosen = max(
                base,
                retry_after_seconds or 0.0,
                google_delay or 0.0,
                bedrock_delay or 0.0,
            )
            return min(self.maximum, chosen)
        # Transport errors (including timeouts): use a short fixed floor so we
        # don't hammer the endpoint, but also don't wait as long as rate-limit
        # back-off since the server may just have been slow.
        if isinstance(
            exc, (httpx.TransportError, APIConnectionError, APITimeoutError)
        ):
            return max(TRANSPORT_ERROR_RETRY_DELAY, min(base, self.maximum))
        return base


async def _send_with_retry(
    send,
    *,
    max_attempts: int = DEFAULT_RETRY_MAX_ATTEMPTS,
    should_retry_response=None,
    on_retry=None,
    bedrock: bool = False,
) -> httpx.Response:
    maximum = (
        BEDROCK_RETRY_MAX_DELAY_SECONDS if bedrock else DEFAULT_RETRY_MAX_DELAY_SECONDS
    )
    async for attempt in AsyncRetrying(
        stop=stop_after_attempt(max_attempts),
        wait=WaitForRetryableResponse(maximum=maximum, bedrock=bedrock),
        retry=retry_if_exception_type((httpx.TransportError, RetryableHttpError)),
        reraise=True,
    ):
        with attempt:
            if on_retry and attempt.retry_state.attempt_number > 1:
                exc = (
                    attempt.retry_state.outcome.exception()
                    if attempt.retry_state.outcome
                    else None
                )
                if isinstance(exc, RetryableHttpError):
                    error_ctx = _response_error_message(exc.response)
                elif isinstance(exc, httpx.TimeoutException):
                    error_ctx = f"timeout ({type(exc).__name__})"
                elif isinstance(exc, httpx.TransportError):
                    error_ctx = f"transport error ({type(exc).__name__}): {exc}"
                elif exc is not None:
                    error_ctx = str(exc)
                else:
                    error_ctx = None
                on_retry(attempt.retry_state.attempt_number, max_attempts, error_ctx)
            response = await send()
            retryable = (
                should_retry_response(response)
                if should_retry_response is not None
                else _is_retryable_status(response.status_code)
            )
            if retryable:
                raise RetryableHttpError(response)
            return response
    raise RuntimeError("request retry loop exited unexpectedly")


def _retry_error_context(exc: BaseException | None) -> str | None:
    if isinstance(exc, RetryableHttpError):
        return _response_error_message(exc.response)
    if isinstance(exc, APIStatusError):
        return _response_error_message(exc.response)
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


def _anthropic_rate_limit_detail(response: httpx.Response) -> str:
    headers = [
        "retry-after",
        "anthropic-ratelimit-requests-limit",
        "anthropic-ratelimit-requests-remaining",
        "anthropic-ratelimit-requests-reset",
        "anthropic-ratelimit-input-tokens-reset",
        "anthropic-ratelimit-tokens-limit",
        "anthropic-ratelimit-tokens-remaining",
        "anthropic-ratelimit-tokens-reset",
        "anthropic-ratelimit-input-tokens-limit",
        "anthropic-ratelimit-input-tokens-remaining",
        "anthropic-ratelimit-output-tokens-limit",
        "anthropic-ratelimit-output-tokens-remaining",
        "anthropic-ratelimit-output-tokens-reset",
    ]
    parts = [
        f"{header}={value}"
        for header in headers
        if (value := response.headers.get(header))
    ]
    return f" [{', '.join(parts)}]" if parts else ""


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
                        "arguments": _json_text(call.arguments),
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
        # Some providers duplicate a JSON blob instead of returning one object.
        # Scan near the midpoint for the start of a second object and salvage that.
        mid = len(arguments) // 2
        for i in range(max(0, mid - 15), min(len(arguments), mid + 15)):
            if arguments[i] == "{":
                try:
                    return decode(arguments[i:])
                except (msgspec.DecodeError, RuntimeError):
                    pass
        raise RuntimeError(f"Could not parse tool arguments JSON: {exc}") from exc


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
    instructions = _responses_instructions(messages)
    if instructions:
        payload["instructions"] = instructions
    response_tools = _responses_tools(tools)
    if response_tools:
        payload["tools"] = response_tools
        payload["tool_choice"] = tool_choice
        payload["parallel_tool_calls"] = False
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
                if isinstance(part.get("text"), str):
                    content_parts.append(part["text"])
                elif isinstance(part.get("refusal"), str):
                    content_parts.append(part["refusal"])
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
    if not content_parts and isinstance(data.get("output_text"), str):
        content_parts.append(data["output_text"])
    return AssistantMessage(
        content="\n\n".join(part for part in content_parts if part),
        tool_calls=tool_calls,
    )


parse_tool_call_arguments = _decode_tool_call_arguments
_responses_output_to_message = _decode_responses_output


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


def _bearer_headers(token: str, **headers: str) -> dict[str, str]:
    return {"Authorization": f"Bearer {token}", **headers}


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
            "content": [{"text": _tool_output_text(block.result).strip() or "(no output)"}],
            "status": "error" if not block.result.ok else "success",
        }
    },
)

VERTEX_CODEC = ProviderCodec(
    user_role="user",
    assistant_role="model",
    content_key="parts",
    system_item=lambda text: {"text": text},
    finalize_system=lambda parts: {"parts": parts} if parts else None,
    encode_text=lambda text: {"text": text},
    encode_tool_use=lambda block: (
        {
            "functionCall": {"name": block.name, "args": block.arguments},
            **({"thoughtSignature": block.thought_signature} if block.thought_signature else {}),
        }
    ),
    encode_tool_result=lambda block: {
        "functionResponse": {
            "name": block.name or "tool",
            "response": (
                {"error": payload}
                if not block.result.ok
                and isinstance(
                    payload := (
                        block.result.content
                        if isinstance(block.result.content, dict)
                        else {"output": block.result.content}
                    ),
                    dict,
                )
                and "error" not in payload
                else (
                    block.result.content
                    if isinstance(block.result.content, dict)
                    else {"output": block.result.content}
                )
            ),
        }
    },
)

ANTHROPIC_CODEC = ProviderCodec(
    user_role="user",
    assistant_role="assistant",
    content_key="content",
    system_item=lambda text: text,
    finalize_system=lambda parts: "\n\n".join(parts) if parts else "",
    encode_text=lambda text: {"type": "text", "text": text},
    encode_tool_use=lambda block: {
        "type": "tool_use",
        "id": block.id,
        "name": block.name,
        "input": block.arguments,
    },
    encode_tool_result=lambda block: {
        "type": "tool_result",
        "tool_use_id": block.id,
        "content": _tool_output_text(block.result),
        **({"is_error": True} if not block.result.ok else {}),
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


def _http_completion_client(
    endpoint: HttpChatEndpoint, list_models: Callable[[], list[str]]
) -> CompletionClient:
    async def chat_completion(
        model: str,
        messages: list[ChatMessage],
        tools: list[ToolSpec] | None = None,
        tool_choice: str = "auto",
        on_retry=None,
    ) -> AssistantMessage:
        return await _run_http_chat_endpoint(
            endpoint, model, messages, tools, tool_choice, on_retry
        )

    return CompletionClient(chat_completion=chat_completion, list_models=list_models)


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

        async def create_response():
            return await client.responses.create(
                **_responses_payload(model, messages, tools, tool_choice)
            )

        response = await _call_with_retry(
            create_response, on_retry=on_retry, bedrock=bedrock
        )
        return _decode_responses_output(response)

    return CompletionClient(
        chat_completion=chat_completion,
        list_models=lambda: _sync_model_ids(
            sync_client, fallback=fallback_models, default=default_models
        ),
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
        }
        if tools:
            kwargs["tools"] = tools_map(tools)
            kwargs["tool_choice"] = tool_choice

        async def create_response():
            return await client.chat.completions.create(**kwargs)

        response = await _call_with_retry(
            create_response, on_retry=on_retry, bedrock=bedrock
        )
        choice = response.choices[0]
        msg = choice.message
        return AssistantMessage(
            content=msg.content or "",
            tool_calls=[
                ToolCall(
                    id=tc.id,
                    name=tc.function.name,
                    arguments=_decode_tool_call_arguments(tc.function.arguments),
                )
                for tc in msg.tool_calls or []
            ],
        )

    return CompletionClient(
        chat_completion=chat_completion,
        list_models=lambda: _sync_model_ids(sync_client, fallback=None),
    )


async def _run_http_chat_endpoint(
    endpoint: HttpChatEndpoint,
    model: str,
    messages: list[ChatMessage],
    tools: list[ToolSpec] | None = None,
    tool_choice: str = "auto",
    on_retry=None,
) -> AssistantMessage:
    request = endpoint.build_request(model, messages, tools, tool_choice)
    refreshed = False

    async with endpoint.make_client() as client:

        async def send_request() -> httpx.Response:
            nonlocal request, refreshed
            response = await client.post(
                request.url,
                json=request.json_body,
                content=request.content_body,
                headers=request.headers,
                timeout=endpoint.timeout,
            )
            if (
                response.status_code == 401
                and endpoint.refresh_auth is not None
                and not refreshed
            ):
                endpoint.refresh_auth()
                refreshed = True
                request = endpoint.build_request(model, messages, tools, tool_choice)
                response = await client.post(
                    request.url,
                    json=request.json_body,
                    content=request.content_body,
                    headers=request.headers,
                    timeout=endpoint.timeout,
                )
            return response

        response = await _send_with_retry(
            send_request,
            should_retry_response=endpoint.should_retry_response,
            on_retry=on_retry,
            bedrock=endpoint.bedrock,
        )
    if response.is_error:
        raise RuntimeError(endpoint.error(response))
    return endpoint.decode(response.json())


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
        text_of=lambda item: item.get("text") if isinstance(item.get("text"), str) else None,
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


def _bedrock_model_ids(client: httpx.Client, region: str) -> list[str]:
    base_url = f"https://bedrock.{region}.amazonaws.com"
    return [
        *_fetch_json_ids(
            client,
            f"{base_url}/foundation-models",
            items_key="modelSummaries",
            id_key="modelId",
            item_filter=lambda item: (
                "TEXT" in item.get("outputModalities", [])
                and item["modelId"].startswith(("global.", "us."))
            ),
        ),
        *_fetch_json_ids(
            client,
            f"{base_url}/inference-profiles",
            items_key="inferenceProfileSummaries",
            id_key="inferenceProfileId",
            item_filter=lambda item: item["inferenceProfileId"].startswith(
                ("global.", "us.")
            ),
        ),
    ]


def _bedrock_converse_client(region: str, credentials: dict[str, str]) -> CompletionClient:
    base_url = f"https://bedrock-runtime.{region}.amazonaws.com"
    aws_credentials = httpx_aws_auth.AwsCredentials(
        access_key=credentials["access_key"],
        secret_key=credentials["secret_key"],
        session_token=credentials.get("session_token"),
    )
    auth = httpx_aws_auth.AwsSigV4Auth(
        credentials=aws_credentials, service="bedrock", region=region
    )

    def make_request(
        model: str,
        messages: list[ChatMessage],
        tools: list[ToolSpec] | None,
        tool_choice: str,
    ) -> HttpRequest:
        bedrock_messages, system_prompts = _encode_provider_messages(
            messages, BEDROCK_CODEC
        )
        payload: dict[str, Any] = {
            "messages": bedrock_messages,
            "inferenceConfig": {
                "maxTokens": int(os.environ.get("OY_BEDROCK_MAX_OUTPUT_TOKENS", "4096"))
            },
        }
        if system_prompts:
            payload["system"] = system_prompts
        if tools and (tool_config := _bedrock_tools(tools, tool_choice)):
            payload["toolConfig"] = tool_config
        return HttpRequest(
            url=f"{base_url}/model/{model}/converse",
            content_body=json.dumps(payload),
            headers={"content-type": "application/json"},
        )

    endpoint = HttpChatEndpoint(
        make_client=lambda: async_http_client(auth=auth),
        build_request=make_request,
        decode=lambda data: _assistant_from_blocks(_bedrock_output_blocks(data)),
        error=lambda resp: _http_error_message("Bedrock", resp),
        timeout=BEDROCK_TIMEOUT,
        bedrock=True,
    )

    def list_models() -> list[str]:
        with http_client(auth=auth, timeout=30) as client:
            return sorted(unique_strings(_bedrock_model_ids(client, region)))

    return _http_completion_client(
        endpoint,
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


def _vertex_output_blocks(response: dict[str, Any]) -> list[ContentBlock]:
    candidates = response.get("response", {}).get("candidates", [])
    if not candidates:
        return []
    return _extract_blocks(
        candidates[0].get("content", {}).get("parts", []),
        text_of=lambda item: item.get("text") if isinstance(item.get("text"), str) else None,
        tool_of=lambda item, index: (
            ToolUseBlock(
                id=f"call_{index}",
                name=item["functionCall"]["name"],
                arguments=item["functionCall"].get("args", {}),
                thought_signature=item.get("thoughtSignature", ""),
            )
            if "functionCall" in item
            else None
        ),
    )


def _gemini_client(project_id: str, initial_access_token: str) -> CompletionClient:
    access_token = initial_access_token
    base_url = f"{GEMINI_CODE_ASSIST_ENDPOINT}/{GEMINI_CODE_ASSIST_VERSION}"

    def auth_headers() -> dict[str, str]:
        return _bearer_headers(access_token, **{"Content-Type": "application/json"})

    def refresh_auth() -> None:
        nonlocal access_token
        access_token = refresh_gemini_token(load_gemini_oauth_creds()["refresh_token"])

    def make_request(
        model: str,
        messages: list[ChatMessage],
        tools: list[ToolSpec] | None,
        tool_choice: str,
    ) -> HttpRequest:
        contents, system_instruction = _encode_provider_messages(messages, VERTEX_CODEC)
        request_body: dict[str, Any] = {"contents": contents}
        if system_instruction:
            request_body["systemInstruction"] = system_instruction
        if tools:
            request_body["tools"] = [
                {
                    "functionDeclarations": [
                        {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        }
                        for tool in tools
                    ]
                }
            ]
            request_body["toolConfig"] = {
                "functionCallingConfig": {
                    "mode": "AUTO" if tool_choice == "auto" else "ANY"
                }
            }
        return HttpRequest(
            url=f"{base_url}:generateContent",
            json_body={"model": model, "project": project_id, "request": request_body},
            headers=auth_headers(),
        )

    endpoint = HttpChatEndpoint(
        make_client=lambda: async_http_client(timeout=120),
        build_request=make_request,
        decode=lambda data: _assistant_from_blocks(_vertex_output_blocks(data)),
        error=lambda resp: (
            f"Gemini Code Assist error ({resp.status_code}): {_response_error_message(resp)}"
        ),
        timeout=120,
        should_retry_response=_should_retry_google_response,
        refresh_auth=refresh_auth,
    )
    return _http_completion_client(endpoint, list_models=load_gemini_model_list)


def _anthropic_output_blocks(data: dict[str, Any]) -> list[ContentBlock]:
    return _extract_blocks(
        data.get("content", []),
        text_of=lambda item: item["text"] if item.get("type") == "text" else None,
        tool_of=lambda item, _: (
            ToolUseBlock(
                id=item["id"],
                name=item["name"],
                arguments=item.get("input", {}),
            )
            if item.get("type") == "tool_use"
            else None
        ),
    )


def _claude_client(initial_access_token: str) -> CompletionClient:
    access_token = initial_access_token

    def auth_headers() -> dict[str, str]:
        return _bearer_headers(
            access_token,
            **{
                "anthropic-version": ANTHROPIC_VERSION,
                "anthropic-beta": "oauth-2025-04-20",
                "content-type": "application/json",
            },
        )

    def refresh_auth() -> None:
        nonlocal access_token
        access_token = get_claude_access_token()

    def make_request(
        model: str,
        messages: list[ChatMessage],
        tools: list[ToolSpec] | None,
        tool_choice: str,
    ) -> HttpRequest:
        anthropic_payload, system = _encode_provider_messages(messages, ANTHROPIC_CODEC)
        body: dict[str, Any] = {
            "model": model,
            "max_tokens": 8096,
            "messages": anthropic_payload,
        }
        if system:
            body["system"] = system
        if tools:
            body["tools"] = [
                {
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters,
                }
                for tool in tools
            ]
            body["tool_choice"] = (
                {"type": "auto"} if tool_choice == "auto" else {"type": "any"}
            )
        return HttpRequest(
            url=f"{CLAUDE_API_URL}/v1/messages",
            json_body=body,
            headers=auth_headers(),
        )

    endpoint = HttpChatEndpoint(
        make_client=lambda: async_http_client(timeout=120),
        build_request=make_request,
        decode=lambda data: _assistant_from_blocks(_anthropic_output_blocks(data)),
        error=lambda resp: (
            f"Anthropic API error ({resp.status_code}): "
            f"{_response_error_message(resp)}"
            f"{_anthropic_rate_limit_detail(resp) if resp.status_code == 429 else ''}"
        ),
        timeout=120,
        refresh_auth=refresh_auth,
    )
    return _http_completion_client(
        endpoint, list_models=lambda: _fetch_claude_models(access_token)
    )


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


def _fetch_claude_models(access_token: str) -> list[str]:
    with http_client(timeout=15) as client:
        return _fetch_json_ids(
            client,
            f"{CLAUDE_API_URL}/v1/models",
            headers=_bearer_headers(
                access_token,
                **{
                    "anthropic-version": ANTHROPIC_VERSION,
                    "anthropic-beta": "oauth-2025-04-20",
                },
            ),
            params={"limit": 1000},
            items_key="data",
            id_key="id",
            has_more_key="has_more",
            cursor_key="last_id",
            cursor_param="after_id",
            raise_errors=True,
        )


def _try_bool(fn) -> bool:
    try:
        fn()
        return True
    except Exception:
        return False


def _require_openai_env(_: Path | None = None) -> None:
    _require_string(get_openai_api_key(), "OPENAI_API_KEY is not set")


def _require_codex_env(_: Path | None = None) -> None:
    load_codex_session()


def _require_gemini_env(_: Path | None = None) -> None:
    load_gemini_oauth_creds()


def _require_claude_env(_: Path | None = None) -> None:
    load_claude_auth_status()


def _require_aws_env(cwd: Path | None = None) -> None:
    current_region(None)
    load_aws_credentials(cwd, allow_login=False)


def _build_openai_client(
    region: str | None = None, cwd: Path | None = None, *, max_retries: int = 3
) -> CompletionClient:
    _ = region, cwd
    return _openai_responses_client(
        *_openai_pair(
            _require_string(get_openai_api_key(), "No OpenAI credentials found"),
            base_url=os.environ.get("OPENAI_BASE_URL"),
            max_retries=max_retries,
        )
    )


def _build_codex_client(
    region: str | None = None, cwd: Path | None = None
) -> CompletionClient:
    _ = region, cwd
    if api_key := get_codex_api_key():
        return _openai_responses_client(
            *_openai_pair(api_key),
            fallback_models=load_codex_model_list,
            default_models=[CODEX_DEFAULT_MODEL],
        )
    return _codex_chatgpt_client()


def _build_gemini_client(
    region: str | None = None, cwd: Path | None = None
) -> CompletionClient:
    _ = region, cwd
    return _gemini_client(resolve_gemini_project(), get_gemini_access_token())


def _build_bedrock_client(
    region: str | None = None, cwd: Path | None = None
) -> CompletionClient:
    return _bedrock_converse_client(current_region(region), load_aws_credentials(cwd))


def _build_mantle_client(
    region: str | None = None, cwd: Path | None = None
) -> CompletionClient:
    current = current_region(region)
    return _openai_chat_completions_client(
        *_openai_pair(
            make_bedrock_token(current, cwd),
            base_url=bedrock_base_url(current),
            max_retries=0,
            timeout=BEDROCK_TIMEOUT.read,
        ),
        tools_map=_tool_specs_to_openai,
        bedrock=True,
    )


def _build_claude_shim(
    region: str | None = None, cwd: Path | None = None
) -> CompletionClient:
    _ = region, cwd
    return _claude_client(get_claude_access_token())


def _list_openai_models(
    region: str | None = None, cwd: Path | None = None
) -> list[str]:
    return _build_openai_client(region, cwd, max_retries=0).list_models()


def _list_gemini_models(
    region: str | None = None, cwd: Path | None = None
) -> list[str]:
    _require_gemini_env(cwd)
    return load_gemini_model_list()


def _list_codex_models(
    region: str | None = None, cwd: Path | None = None
) -> list[str]:
    return _build_codex_client(region, cwd).list_models()


def _list_claude_models(
    region: str | None = None, cwd: Path | None = None
) -> list[str]:
    _ = region, cwd
    return _fetch_claude_models(get_claude_access_token())


def _list_bedrock_models(
    region: str | None = None, cwd: Path | None = None
) -> list[str]:
    return _bedrock_converse_client(
        current_region(region), load_aws_credentials(cwd, allow_login=False)
    ).list_models()


def _list_mantle_models(
    region: str | None = None, cwd: Path | None = None
) -> list[str]:
    return _build_mantle_client(region, cwd).list_models()


SHIM_SPECS: dict[str, ShimSpec] = {
    SHIM_OPENAI: ShimSpec(
        name=SHIM_OPENAI,
        ensure_env=_require_openai_env,
        build_client=_build_openai_client,
        list_models=_list_openai_models,
    ),
    SHIM_CODEX: ShimSpec(
        name=SHIM_CODEX,
        ensure_env=_require_codex_env,
        build_client=_build_codex_client,
        list_models=_list_codex_models,
    ),
    SHIM_GEMINI: ShimSpec(
        name=SHIM_GEMINI,
        ensure_env=_require_gemini_env,
        build_client=_build_gemini_client,
        list_models=_list_gemini_models,
    ),
    SHIM_BEDROCK: ShimSpec(
        name=SHIM_BEDROCK,
        ensure_env=_require_aws_env,
        build_client=_build_bedrock_client,
        list_models=_list_bedrock_models,
    ),
    SHIM_MANTLE: ShimSpec(
        name=SHIM_MANTLE,
        ensure_env=_require_aws_env,
        build_client=_build_mantle_client,
        list_models=_list_mantle_models,
    ),
    SHIM_CLAUDE: ShimSpec(
        name=SHIM_CLAUDE,
        ensure_env=_require_claude_env,
        build_client=_build_claude_shim,
        list_models=_list_claude_models,
    ),
}


def _shim_spec(shim: str) -> ShimSpec:
    return SHIM_SPECS[validate_shim(shim)]


def detect_available_shims() -> list[str]:
    return [
        shim
        for shim in SHIM_ORDER
        if _try_bool(lambda spec=_shim_spec(shim): spec.ensure_env(None))
    ]


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


def list_model_ids(
    shim: str, region: str | None = None, cwd: Path | None = None
) -> list[str]:
    return get_client(shim, region=region, cwd=cwd).list_models()
