from __future__ import annotations
import base64
import json
import os
import shutil
import subprocess
import sys
import threading as _threading
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from functools import lru_cache
from http import HTTPStatus
from pathlib import Path
from types import MappingProxyType
from typing import Any, Callable, TypeAlias
from urllib.parse import urljoin

import toons
import urllib3
from tenacity import Retrying, retry_if_exception_type, stop_after_attempt
from tenacity.wait import wait_base
from urllib3.util import Retry

from .aws_sigv4 import sigv4_headers

JSONLike: TypeAlias = dict[str, Any] | list[Any] | str | int | float | bool | None

def normalize_jsonlike(value: Any) -> JSONLike:
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, dict):
        return {str(key): normalize_jsonlike(item) for key, item in value.items()}
    if isinstance(value, (list, tuple, set)):
        return [normalize_jsonlike(item) for item in value]
    return str(value)

def serialize_json(value: Any) -> str:
    normalized = normalize_jsonlike(value)
    return (
        normalized
        if isinstance(normalized, str)
        else json.dumps(normalized, separators=(",", ":"), ensure_ascii=False)
    )

def serialize_toon(value: Any) -> str:
    normalized = normalize_jsonlike(value)
    return normalized if isinstance(normalized, str) else toons.dumps(normalized)

def ToolCall(id: str, name: str, arguments: dict[str, Any] | None = None) -> dict[str, Any]:
    return {"id": id, "name": name, "arguments": dict(arguments or {})}


def ToolResult(*, ok: bool = True, content: JSONLike = None) -> dict[str, Any]:
    return {"ok": ok, "content": content}


def SystemMessage(content: str) -> dict[str, Any]:
    return {"role": "system", "content": content}


def UserMessage(content: str) -> dict[str, Any]:
    return {"role": "user", "content": content}


def AssistantMessage(
    content: str = "",
    tool_calls: list[dict[str, Any]] | None = None,
    thought_signatures: dict[str, str] | None = None,
) -> dict[str, Any]:
    return {
        "role": "assistant",
        "content": content,
        "tool_calls": list(tool_calls or []),
        "thought_signatures": dict(thought_signatures or {}),
    }


def ToolMessage(
    tool_call_id: str,
    name: str = "",
    content: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "role": "tool",
        "tool_call_id": tool_call_id,
        "name": name,
        "content": content or ToolResult(),
    }


ChatMessage: TypeAlias = dict[str, Any]


CompletionClient: TypeAlias = dict[str, Any]


SHIM_OPENAI = "openai"
SHIM_CODEX = "codex"
SHIM_MANTLE = "bedrock-mantle"
SHIM_COPILOT = "copilot"
SHIM_OPENCODE = "opencode"
SHIM_OPENCODE_GO = "opencode-go"
SHIM_ORDER = (
    SHIM_OPENAI,
    SHIM_CODEX,
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
TRANSPORT_ERROR_RETRY_DELAY = 3.0

def _tool_output_value(result: dict[str, Any]) -> JSONLike:
    return normalize_jsonlike(result.get("content"))

def _tool_output_text(result: dict[str, Any]) -> str:
    return serialize_toon(_tool_output_value(result))

def _openai_tool_call(tool_call: dict[str, Any]) -> dict[str, Any]:
    return {
        "id": tool_call["id"],
        "type": "function",
        "function": {
            "name": tool_call["name"],
            "arguments": serialize_json(tool_call["arguments"]),
        },
    }

def _openai_chat_message(message: ChatMessage) -> dict[str, Any]:
    role = message.get("role")
    if role == "system":
        return {"role": "system", "content": message["content"]}
    if role == "user":
        return {"role": "user", "content": message["content"]}
    if role == "assistant":
        payload = {"role": "assistant", "content": message["content"]}
        if message["tool_calls"]:
            payload["tool_calls"] = [
                _openai_tool_call(tool_call) for tool_call in message["tool_calls"]
            ]
        if message["thought_signatures"]:
            payload["thought_signatures"] = message["thought_signatures"]
        return payload
    if role == "tool":
        return {
            "role": "tool",
            "tool_call_id": message["tool_call_id"],
            "name": message["name"],
            "content": _tool_output_text(message["content"]),
        }
    raise TypeError(f"Unsupported message role: {role!r}")

# ---------------------------------------------------------------------------
# Local persistence and shell/runtime helpers
# ---------------------------------------------------------------------------

def load_json(path, default):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return default

def _ensure_private_dir(path: Path) -> Path:
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.chmod(0o700)
    return path

def save_json(path, data):
    try:
        _ensure_private_dir(path.parent)
        path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        path.chmod(0o600)
        return True
    except OSError:
        return False

def unique_strings(values):
    return list(dict.fromkeys(value for value in values if value))

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
        with tool_session(timeout=SHORT_HTTP_TIMEOUT) as client:
            response = client.request(
                "POST",
                url,
                data=data,
                headers={"Content-Type": "application/x-www-form-urlencoded"},
            )
        response_raise_for_status(response)
        payload = response_json(response)
    except HTTPError as exc:
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

def which(command, path=None):
    return shutil.which(command, path=path)

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

DEFAULT_HTTP_TIMEOUT = 300.0
SHORT_HTTP_TIMEOUT = 30.0
DEFAULT_WEBFETCH_TIMEOUT_SECONDS = 60


class HTTPError(RuntimeError):
    def __init__(self, message: str, *, response: "ResponseAdapter | None" = None):
        self.response = response
        super().__init__(message)


class TransportError(RuntimeError):
    pass


class TimeoutException(TransportError):
    pass


class APIConnectionError(TransportError):
    pass


class APITimeoutError(TimeoutException):
    pass


class APIStatusError(HTTPError):
    def __init__(self, message: str, *, response: "ResponseAdapter", body: Any = None):
        self.body = body
        super().__init__(message, response=response)


class AuthenticationError(APIStatusError):
    pass


class PermissionDeniedError(APIStatusError):
    pass


class RateLimitError(APIStatusError):
    pass


class BadRequestError(APIStatusError):
    pass


ResponseAdapter: TypeAlias = dict[str, Any]


def _normalize_headers(headers: Any) -> dict[str, str]:
    if headers is None:
        return {}
    if hasattr(headers, "items"):
        items = headers.items()
    else:
        items = dict(headers).items()
    return {str(key).lower(): str(value) for key, value in items}


def adapt_response(response: Any) -> ResponseAdapter:
    is_mapping = isinstance(response, dict)
    status_code = int((response.get("status_code") if is_mapping else getattr(response, "status_code", 0)) or 0)
    reason_phrase = str(
        (
            response.get("reason_phrase", "")
            if is_mapping
            else getattr(response, "reason", "") or _status_reason(status_code)
        )
        or ""
    )
    return response_adapter(
        status_code=status_code,
        headers=response.get("headers") if is_mapping else getattr(response, "headers", None),
        text=str((response.get("text", "") if is_mapping else getattr(response, "text", "")) or ""),
        content=bytes((response.get("content", b"") if is_mapping else getattr(response, "content", b"")) or b""),
        url=str((response.get("url", "") if is_mapping else getattr(response, "url", "")) or ""),
        reason_phrase=reason_phrase,
        http_version=_http_version_name(
            response.get("http_version") if is_mapping else getattr(response, "http_version", None)
        ),
    )


def response_adapter(
    *,
    status_code: int,
    headers: Any,
    text: str,
    content: bytes,
    url: str,
    reason_phrase: str = "",
    http_version: str = "HTTP/1.1",
) -> ResponseAdapter:
    return {
        "status_code": status_code,
        "headers": _normalize_headers(headers),
        "text": text,
        "content": content,
        "url": url,
        "reason_phrase": reason_phrase,
        "http_version": http_version,
    }


def response_is_success(response: ResponseAdapter) -> bool:
    return 200 <= response["status_code"] < 300


def response_json(response: ResponseAdapter) -> Any:
    return json.loads(response["text"])


def response_raise_for_status(response: ResponseAdapter) -> None:
    if response["status_code"] < 400:
        return
    raise _status_error_from_response(response)


def _encode_json_body(value: Any) -> bytes:
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def _coerce_form_fields(value: dict[Any, Any]) -> dict[str, str]:
    return {str(key): str(item) for key, item in value.items()}



def _encode_request_data(value: Any) -> bytes | None:
    if value is None:
        return None
    if isinstance(value, bytes):
        return value
    if isinstance(value, bytearray):
        return bytes(value)
    if isinstance(value, str):
        return value.encode("utf-8")
    raise TypeError(f"Unsupported request body type: {type(value).__name__}")


def _decode_response_text(content: bytes, headers: Any) -> str:
    charset = None
    if hasattr(headers, "get_content_charset"):
        charset = headers.get_content_charset()
    if not charset and hasattr(headers, "get"):
        content_type = headers.get("Content-Type") or headers.get("content-type")
        if isinstance(content_type, str):
            parts = [part.strip() for part in content_type.split(";")]
            for part in parts[1:]:
                if part.lower().startswith("charset="):
                    charset = part.split("=", 1)[1].strip().strip('"') or None
                    break
    for encoding in [charset, "utf-8", "latin-1"]:
        if not encoding:
            continue
        try:
            return content.decode(encoding)
        except UnicodeDecodeError:
            continue
    return content.decode("utf-8", errors="replace")


def _http_request_timeout(value: float) -> urllib3.Timeout:
    return urllib3.Timeout(total=float(value))



def _http_request_retries(follow_redirects: bool) -> Retry | bool:
    if not follow_redirects:
        return False
    return Retry(
        total=None,
        connect=0,
        read=0,
        redirect=10,
        status=0,
        other=0,
        raise_on_redirect=False,
    )



def _transport_error_reason(exc: BaseException) -> str:
    if isinstance(exc, urllib3.exceptions.MaxRetryError) and exc.reason is not None:
        return _transport_error_reason(exc.reason)
    return str(exc)



def _transport_error_is_timeout(exc: BaseException) -> bool:
    if isinstance(exc, urllib3.exceptions.MaxRetryError) and exc.reason is not None:
        return _transport_error_is_timeout(exc.reason)
    if isinstance(exc, urllib3.exceptions.ReadTimeoutError):
        return True
    return isinstance(exc, urllib3.exceptions.TimeoutError) and not isinstance(
        exc, urllib3.exceptions.NewConnectionError
    )



def _response_url_from_raw(response: Any, request_url: str) -> str:
    raw_url = getattr(response, "url", None)
    if not raw_url and hasattr(response, "geturl"):
        raw_url = response.geturl()
    return urljoin(request_url, str(raw_url or "")) or request_url


class HTTPClient:
    def __init__(
        self,
        *,
        timeout: float = DEFAULT_HTTP_TIMEOUT,
        follow_redirects: bool = False,
    ) -> None:
        self.timeout = float(timeout)
        self.follow_redirects = bool(follow_redirects)
        self._pool = urllib3.PoolManager()

    def __enter__(self) -> "HTTPClient":
        return self

    def __exit__(self, exc_type, exc, tb) -> bool:
        self.close()
        return False

    def close(self) -> None:
        self._pool.clear()

    def get(self, url: str, **kwargs: Any) -> ResponseAdapter:
        return self.request("GET", url, **kwargs)

    def post(self, url: str, **kwargs: Any) -> ResponseAdapter:
        return self.request("POST", url, **kwargs)

    def request(
        self,
        method: str,
        url: str,
        *,
        json: Any = None,
        data: Any = None,
        headers: dict[str, str] | None = None,
        timeout: float | None = None,
    ) -> ResponseAdapter:
        if json is not None and data is not None:
            raise TypeError("request accepts either json or data, not both")
        method = method.upper()
        request_kwargs: dict[str, Any] = {
            "headers": {str(key): str(value) for key, value in (headers or {}).items()},
            "timeout": _http_request_timeout(self.timeout if timeout is None else float(timeout)),
            "redirect": self.follow_redirects,
            "retries": _http_request_retries(self.follow_redirects),
            "preload_content": True,
        }
        if json is not None:
            request_kwargs["json"] = json
        elif isinstance(data, dict):
            request_kwargs["fields"] = _coerce_form_fields(data)
            if method not in {"DELETE", "GET", "HEAD", "OPTIONS"}:
                request_kwargs["encode_multipart"] = False
        elif data is not None:
            request_kwargs["body"] = _encode_request_data(data)
        try:
            raw = self._pool.request(method, url, **request_kwargs)
        except urllib3.exceptions.HTTPError as exc:
            reason = _transport_error_reason(exc)
            if _transport_error_is_timeout(exc):
                raise APITimeoutError(reason) from exc
            raise APIConnectionError(reason) from exc
        content = bytes(getattr(raw, "data", b"") or b"")
        status_code = int(getattr(raw, "status", 0) or 0)
        response_headers = getattr(raw, "headers", None)
        return response_adapter(
            status_code=status_code,
            headers=response_headers,
            text=_decode_response_text(content, response_headers),
            content=content,
            url=_response_url_from_raw(raw, url),
            reason_phrase=str(getattr(raw, "reason", None) or _status_reason(status_code)),
            http_version=_http_version_name(
                getattr(raw, "version_string", None) or getattr(raw, "version", None)
            ),
        )


OpenAI: TypeAlias = dict[str, Any]



def llm_session(**kw):
    kw.setdefault("timeout", DEFAULT_HTTP_TIMEOUT)
    kw.setdefault("follow_redirects", False)
    return http_client(**kw)



def tool_session(**kw):
    kw.setdefault("timeout", DEFAULT_WEBFETCH_TIMEOUT_SECONDS)
    kw.setdefault("follow_redirects", False)
    return http_client(**kw)



def _openai(
    api_key: str,
    *,
    base_url: str = "https://api.openai.com/v1",
    max_retries: int = 3,
    timeout: float = DEFAULT_HTTP_TIMEOUT,
    headers: dict[str, str] | None = None,
    follow_redirects: bool = False,
    http: HTTPClient | None = None,
) -> OpenAI:
    _ = max_retries
    return {
        "api_key": api_key,
        "base_url": base_url,
        "http": http or llm_session(timeout=timeout, follow_redirects=follow_redirects),
        "headers": dict(headers or {}),
    }


def _headers(api: OpenAI, headers: dict[str, str] | None = None) -> dict[str, str]:
    merged = {
        "Authorization": f"Bearer {api['api_key']}",
        "Content-Type": "application/json",
        **api["headers"],
    }
    if headers:
        merged.update(headers)
    return merged


def _bedrock_request_headers(
    credentials: dict[str, str],
    region: str,
    method: str,
    url: str,
    *,
    body: bytes = b"",
    headers: dict[str, str] | None = None,
) -> dict[str, str]:
    return sigv4_headers(
        credentials,
        region,
        "bedrock-mantle",
        method,
        url,
        body=body,
        headers=headers,
    )


def _req(
    api: OpenAI,
    method: str,
    path: str,
    *,
    json_body: dict[str, Any] | None = None,
    data: dict[str, str] | None = None,
    headers: dict[str, str] | None = None,
) -> ResponseAdapter:
    url = (
        path
        if path.startswith(("http://", "https://"))
        else f"{api['base_url'].rstrip('/')}/{path.lstrip('/')}"
    )
    response = api["http"].request(
        method,
        url,
        json=json_body,
        data=data,
        headers=_headers(api, headers),
    )
    response_raise_for_status(response)
    return response


def _req_json(
    api: OpenAI,
    method: str,
    path: str,
    *,
    source: str,
    json_body: dict[str, Any] | None = None,
    data: dict[str, str] | None = None,
    headers: dict[str, str] | None = None,
) -> dict[str, Any]:
    return _json_dict(
        _req(api, method, path, json_body=json_body, data=data, headers=headers),
        source,
    )


def _model_ids(api: OpenAI) -> list[str]:
    data = _req_json(api, "GET", "/models", source="Models API")
    return sorted(
        model_id
        for model_id in _extract_model_ids(data.get("data"), "id")
        if not model_id.startswith("text-embedding")
    )



def _status_reason(status_code: int) -> str:
    try:
        return HTTPStatus(status_code).phrase
    except ValueError:
        return ""


def _http_version_name(value: Any) -> str:
    if value in (None, ""):
        return "HTTP/1.1"
    if isinstance(value, str):
        upper = value.upper()
        return upper if upper.startswith("HTTP/") else f"HTTP/{value}"
    if value == 3:
        return "HTTP/3"
    if value == 2:
        return "HTTP/2"
    if value in (11, 1.1, 1):
        return "HTTP/1.1"
    if value == 10:
        return "HTTP/1.0"
    return f"HTTP/{value}"



def _json_dict(response: ResponseAdapter, source: str) -> dict[str, Any]:
    try:
        payload = response_json(response)
    except Exception as exc:
        raise RuntimeError(f"{source}: invalid JSON response") from exc
    if not isinstance(payload, dict):
        raise RuntimeError(f"{source}: invalid JSON response")
    return payload


def _status_error_from_response(response: ResponseAdapter) -> APIStatusError:
    message = _response_error_message(response) or response["reason_phrase"] or f"HTTP {response['status_code']}"
    cls: type[APIStatusError]
    if response["status_code"] == 400:
        cls = BadRequestError
    elif response["status_code"] == 401:
        cls = AuthenticationError
    elif response["status_code"] == 403:
        cls = PermissionDeniedError
    elif response["status_code"] == 429:
        cls = RateLimitError
    else:
        cls = APIStatusError
    return cls(message, response=response)


def http_client(**kw):
    timeout = float(kw.pop("timeout", DEFAULT_HTTP_TIMEOUT))
    follow_redirects = bool(kw.pop("follow_redirects", kw.pop("allow_redirects", False)))
    if kw:
        raise TypeError(f"Unsupported http_client kwargs: {', '.join(sorted(kw))}")
    return HTTPClient(timeout=timeout, follow_redirects=follow_redirects)


def bedrock_base_url(region: str) -> str:
    return f"https://bedrock-mantle.{region}.api.aws/v1"


def load_bedrock_model_list(cwd: Path | None = None, region: str | None = None) -> list[str]:
    current = default_region(region)
    url = f"{bedrock_base_url(current).rstrip('/')}/models"
    response = llm_session(timeout=SHORT_HTTP_TIMEOUT, follow_redirects=False).request(
        "GET",
        url,
        headers=_bedrock_request_headers(load_aws_credentials(cwd), current, "GET", url),
    )
    response_raise_for_status(response)
    return _extract_model_ids(response_json(response).get("data"), "id")

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
        or "ap-southeast-2"
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
    def __init__(self, response: ResponseAdapter):
        self.response = response
        super().__init__(f"retryable HTTP {response['status_code']}")

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

def _response_json(response: ResponseAdapter) -> dict[str, Any] | None:
    try:
        payload = response_json(response)
    except Exception:
        return None
    return payload if isinstance(payload, dict) else None

class WaitForRetryableResponse(wait_base):
    def __init__(
        self,
        *,
        initial: float = DEFAULT_RETRY_INITIAL_DELAY_SECONDS,
        maximum: float = DEFAULT_RETRY_MAX_DELAY_SECONDS,
    ):
        self.initial = initial
        self.maximum = maximum

    def __call__(self, retry_state) -> float:
        attempt = max(retry_state.attempt_number, 1)
        base = min(self.maximum, self.initial * (2 ** max(attempt - 1, 0)))
        exc = retry_state.outcome.exception() if retry_state.outcome else None
        if isinstance(exc, RetryableHttpError):
            retry_after_seconds = _parse_retry_after_seconds(
                exc.response["headers"].get("retry-after")
            )
            return min(self.maximum, max(base, retry_after_seconds or 0.0))
        # Transport errors (including timeouts): use a short fixed floor so we
        # don't hammer the endpoint, but also don't wait as long as rate-limit
        # back-off since the server may just have been slow.
        if isinstance(exc, (TransportError, APIConnectionError, APITimeoutError)):
            return max(TRANSPORT_ERROR_RETRY_DELAY, min(base, self.maximum))
        return base

def _retry_error_context(exc: BaseException | None) -> str | None:
    if isinstance(exc, RetryableHttpError):
        msg = _response_error_message(exc.response)
        return msg or f"HTTP {exc.response['status_code']}"
    if isinstance(exc, APIStatusError):
        msg = _response_error_message(exc.response)
        return msg or f"HTTP {exc.response['status_code']}"
    if isinstance(exc, APITimeoutError):
        return f"timeout ({type(exc).__name__})"
    if isinstance(exc, APIConnectionError):
        return f"transport error ({type(exc).__name__}): {exc}"
    if isinstance(exc, TimeoutException):
        return f"timeout ({type(exc).__name__})"
    if isinstance(exc, TransportError):
        return f"transport error ({type(exc).__name__}): {exc}"
    if exc is not None:
        return str(exc)
    return None

def _call_with_retry(
    call,
    *,
    max_attempts: int = DEFAULT_RETRY_MAX_ATTEMPTS,
    on_retry=None,
):
    for attempt in Retrying(
        stop=stop_after_attempt(max_attempts),
        wait=WaitForRetryableResponse(maximum=DEFAULT_RETRY_MAX_DELAY_SECONDS),
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
                return call()
            except APIStatusError as exc:
                if _is_retryable_status(exc.response["status_code"]):
                    raise RetryableHttpError(exc.response) from exc
                raise
    raise RuntimeError("SDK retry loop exited unexpectedly")

def _response_error_message(response: ResponseAdapter) -> str:
    payload = _response_json(response)
    if isinstance(payload, dict):
        error = payload.get("error")
        if isinstance(error, dict) and isinstance(error.get("message"), str):
            return error["message"]
        top_msg = payload.get("message")
        if isinstance(top_msg, str) and top_msg:
            error_type = payload.get("__type") or payload.get("code") or ""
            return f"{error_type}: {top_msg}" if error_type else top_msg
    return response["text"]

def _responses_instructions(messages: list[ChatMessage]) -> str | None:
    parts = [msg["content"] for msg in messages if msg.get("role") == "system"]
    joined = "\n\n".join(part for part in parts if part)
    return joined or None

def _responses_input_from_messages(messages: list[ChatMessage]) -> list[dict[str, Any]]:
    items: list[dict[str, Any]] = []
    for msg in messages:
        role = msg.get("role")
        if role == "system":
            continue
        if role == "user":
            items.append({"type": "message", "role": "user", "content": msg["content"]})
            continue
        if role == "assistant":
            if msg["content"]:
                items.append({"type": "message", "role": "assistant", "content": msg["content"]})
            items.extend(
                {
                    "type": "function_call",
                    "call_id": call["id"],
                    "name": call["name"],
                    "arguments": serialize_json(call["arguments"]),
                    "status": "completed",
                }
                for call in msg["tool_calls"]
            )
            continue
        if role == "tool":
            items.append(
                {
                    "type": "function_call_output",
                    "call_id": msg["tool_call_id"],
                    "output": _tool_output_text(msg["content"]),
                }
            )
    return items

def _responses_tools(tools: list[dict[str, Any]] | None) -> list[dict[str, Any]] | None:
    result = [
        {
            "type": "function",
            "name": tool["name"],
            "description": tool["description"],
            "parameters": tool.get("parameters") or {"type": "object"},
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
        parsed = json.loads(candidate)
        parsed = json.loads(parsed) if isinstance(parsed, str) else parsed
        if not isinstance(parsed, dict):
            raise RuntimeError("Tool arguments must decode to a JSON object")
        return parsed

    try:
        return decode(arguments)
    except (json.JSONDecodeError, RuntimeError) as exc:
        # Some providers duplicate a JSON blob — the first copy is often
        # truncated (missing close brace) and the second is the valid one.
        # Scan near the midpoint for `{` and try decoding from there.
        mid = len(arguments) // 2
        for i in range(max(0, mid - 40), min(len(arguments), mid + 40)):
            if arguments[i] == "{":
                try:
                    return decode(arguments[i:])
                except (json.JSONDecodeError, RuntimeError):
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
    if exc.response["status_code"] != 400:
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

def _call_with_reasoning_fallback(
    api_kind: str,
    model: str,
    payload: dict[str, Any],
    create,
    *,
    on_retry=None,
):
    if not _should_send_reasoning(api_kind, model):
        payload = _drop_reasoning_arg(payload)
    try:
        return _call_with_retry(lambda: create(payload), on_retry=on_retry)
    except APIStatusError as exc:
        if not _is_reasoning_unsupported_error(exc):
            raise
        _mark_reasoning_unsupported(api_kind, model)
        return _call_with_retry(
            lambda: create(_drop_reasoning_arg(payload)),
            on_retry=on_retry,
        )


def _responses_payload(
    model: str,
    messages: list[ChatMessage],
    tools: list[dict[str, Any]] | None,
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
    tool_calls: list[dict[str, Any]] = []
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

def _http_error_message(prefix: str, response: ResponseAdapter) -> str:
    try:
        data = response_json(response)
    except ValueError:
        body = response["text"].strip()
        body = body[:200] if body else ""
        return (
            f"{prefix} error {response['status_code']}: {body or response['reason_phrase']}"
        )
    detail = data.get("error") or data.get("detail") if isinstance(data, dict) else data
    if isinstance(detail, dict):
        message = detail.get("message") or detail.get("code") or json.dumps(detail)
    elif isinstance(detail, str):
        message = detail
    else:
        message = json.dumps(detail, ensure_ascii=True)
    return f"{prefix} error {response['status_code']}: {message}"

def _list_models(
    list_models: Callable[[], list[str]],
    *,
    fallback: Callable[[], list[str]] | None = None,
    default: list[str] | None = None,
) -> list[str]:
    try:
        return list_models()
    except Exception:
        if fallback:
            cached = fallback()
            if cached:
                return cached
        if default is not None:
            return default
        raise


def _responses_client(
    create: Callable[[dict[str, Any]], dict[str, Any]],
    list_models: Callable[[], list[str]],
    *,
    fallback: Callable[[], list[str]] | None = load_codex_model_list,
    default: list[str] | None = None,
) -> CompletionClient:
    def chat_completion(
        model: str,
        messages: list[ChatMessage],
        tools: list[dict[str, Any]] | None = None,
        tool_choice: str = "auto",
        on_retry=None,
    ) -> AssistantMessage:
        payload = _responses_payload(model, messages, tools, tool_choice)
        response = _call_with_reasoning_fallback(
            "responses", model, payload, create, on_retry=on_retry
        )
        return _decode_responses_output(response)

    return {
        "chat_completion": chat_completion,
        "list_models": lambda: _list_models(
            list_models, fallback=fallback, default=default
        ),
    }

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
        "reasoning",
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

def _chat_completion_tool_call(tool_call: Any) -> dict[str, Any] | None:
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
    data = _message_like_dict(response)
    choices = data.get("choices") if data else getattr(response, "choices", None)
    message = (
        _merged_chat_completion_message(choices)
        if isinstance(choices, list) and len(choices) > 1
        else _chat_completion_message_dict(
            (choices[0] or {}).get("message") if isinstance(choices[0], dict) else getattr(choices[0], "message", None)
        )
        if isinstance(choices, list) and choices
        else {}
    )
    content = message.get("content") if isinstance(message.get("content"), str) else ""
    if not content and isinstance(message.get("reasoning"), str):
        content = message["reasoning"]
    return AssistantMessage(
        content=content,
        tool_calls=[
            call
            for tool_call in message.get("tool_calls") or []
            if (call := _chat_completion_tool_call(tool_call)) is not None
        ],
    )

def _chat_client(
    create: Callable[[dict[str, Any]], dict[str, Any]],
    list_models: Callable[[], list[str]],
    *,
    tools_map: Callable[[list[dict[str, Any]]], list[dict[str, Any]]],
) -> CompletionClient:
    def chat_completion(
        model: str,
        messages: list[ChatMessage],
        tools: list[dict[str, Any]] | None = None,
        tool_choice: str = "auto",
        on_retry=None,
    ) -> AssistantMessage:
        payload: dict[str, Any] = {
            "model": model,
            "messages": [_openai_chat_message(message) for message in messages],
            "reasoning_effort": "high",
        }
        if tools:
            payload["tools"] = tools_map(tools)
            payload["tool_choice"] = tool_choice
        response = _call_with_reasoning_fallback(
            "chat_completions", model, payload, create, on_retry=on_retry
        )
        return _chat_completion_to_assistant_message(response)

    return {
        "chat_completion": chat_completion,
        "list_models": lambda: _list_models(list_models),
    }

def _tool_specs_to_openai(tools: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [
        {
            "type": "function",
            "function": {
                "name": tool["name"],
                "description": tool["description"],
                "parameters": tool["parameters"],
            },
        }
        for tool in tools
    ]

def _codex_chatgpt_client() -> CompletionClient:
    api = _openai(
        "",
        base_url="https://chatgpt.com/backend-api/codex",
        timeout=DEFAULT_HTTP_TIMEOUT,
    )

    def chat_completion(
        model: str,
        messages: list[ChatMessage],
        tools: list[dict[str, Any]] | None = None,
        tool_choice: str = "auto",
        on_retry=None,
    ) -> AssistantMessage:
        _ = on_retry
        payload = _responses_payload(model, messages, tools, tool_choice)
        session = get_codex_chatgpt_session()
        headers = {
            "Authorization": f"Bearer {session['access_token']}",
            "ChatGPT-Account-Id": session["account_id"],
        }
        for attempt in range(2):
            try:
                data = _req_json(
                    api,
                    "POST",
                    CODEX_CHATGPT_RESPONSES_URL,
                    source="Codex ChatGPT",
                    json_body=payload,
                    headers=headers,
                )
            except AuthenticationError:
                if attempt == 0:
                    session = get_codex_chatgpt_session(force_refresh=True)
                    headers = {
                        "Authorization": f"Bearer {session['access_token']}",
                        "ChatGPT-Account-Id": session["account_id"],
                    }
                    continue
                raise RuntimeError("Codex ChatGPT authentication failed after token refresh")
            except APIStatusError as exc:
                raise RuntimeError(_http_error_message("Codex ChatGPT", exc.response)) from exc
            except HTTPError as exc:
                raise RuntimeError(f"Codex ChatGPT request failed: {exc}") from exc
            return _decode_responses_output(data)
        raise RuntimeError("Codex ChatGPT authentication failed after token refresh")

    return {
        "chat_completion": chat_completion,
        "list_models": lambda: load_codex_model_list() or [CODEX_DEFAULT_MODEL],
    }

KNOWN_SHIMS = set(SHIM_ORDER)
_COPILOT_BASE_URL = os.environ.get(
    "COPILOT_BASE_URL", "https://api.githubcopilot.com"
)
_COPILOT_INTEGRATION_ID = "copilot-developer-cli"
_COPILOT_EDITOR_VERSION = "copilot-developer-cli/1.0.6"

def _responses_from_key(
    api_key: str,
    *,
    base_url: str | None = None,
    max_retries: int = 3,
    timeout: Any = None,
    fallback: Callable[[], list[str]] | None = None,
    default: list[str] | None = None,
) -> CompletionClient:
    api = _openai(
        api_key,
        base_url=base_url,
        max_retries=max_retries,
        timeout=timeout,
    )
    return _responses_client(
        lambda payload: _req_json(
            api,
            "POST",
            "/responses",
            source="Responses API",
            json_body=payload,
        ),
        lambda: _model_ids(api),
        fallback=fallback,
        default=default,
    )


def _chat_from_key(
    api_key: str,
    *,
    base_url: str | None = None,
    max_retries: int = 3,
    timeout: Any = None,
    tools_map: Callable[[list[dict[str, Any]]], list[dict[str, Any]]] | None = _tool_specs_to_openai,
) -> CompletionClient:
    api = _openai(
        api_key,
        base_url=base_url,
        max_retries=max_retries,
        timeout=timeout,
    )
    return _chat_client(
        lambda payload: _req_json(
            api,
            "POST",
            "/chat/completions",
            source="Chat Completions API",
            json_body=payload,
        ),
        lambda: _model_ids(api),
        tools_map=tools_map,
    )

def _require_openai_env(_cwd: Path | None = None) -> None:
    _require_string(get_openai_api_key(), "OPENAI_API_KEY is not set")

def _openai_client(
    cwd: Path | None = None,
    *,
    max_retries: int = 3,
) -> CompletionClient:
    _ = cwd
    return _responses_from_key(
        _require_string(get_openai_api_key(), "No OpenAI credentials found"),
        base_url=os.environ.get("OPENAI_BASE_URL"),
        max_retries=max_retries,
    )

def _require_codex_env(_cwd: Path | None = None) -> None:
    load_codex_session()

def _codex_client(cwd: Path | None = None) -> CompletionClient:
    _ = cwd
    if api_key := get_codex_api_key():
        return _responses_from_key(
            api_key,
            fallback=load_codex_model_list,
            default=[CODEX_DEFAULT_MODEL],
        )
    return _codex_chatgpt_client()

def _require_aws_env(cwd: Path | None = None) -> None:
    load_aws_credentials(cwd, allow_login=False)

def _bedrock_mantle_client(
    credentials: dict[str, str],
    region: str,
    *,
    timeout: float = DEFAULT_HTTP_TIMEOUT,
) -> CompletionClient:
    api = {
        "credentials": credentials,
        "region": region,
        "base_url": bedrock_base_url(region),
        "http": llm_session(timeout=timeout, follow_redirects=False),
    }

    def create(payload: dict[str, Any]) -> dict[str, Any]:
        body = _encode_json_body(payload)
        url = f"{api['base_url'].rstrip('/')}/chat/completions"
        response = api["http"].request(
            "POST",
            url,
            data=body,
            headers=_bedrock_request_headers(
                api["credentials"],
                api["region"],
                "POST",
                url,
                body=body,
                headers={"Content-Type": "application/json"},
            ),
        )
        response_raise_for_status(response)
        return _json_dict(response, "Chat Completions API")

    return _chat_client(
        create,
        lambda: load_bedrock_model_list(None, region),
        tools_map=_tool_specs_to_openai,
    )



def _mantle_completion_client(
    cwd: Path | None = None,
    *,
    region: str | None = None,
) -> CompletionClient:
    current = default_region(region)
    return _bedrock_mantle_client(load_aws_credentials(cwd), current)

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

def _require_copilot_env(_cwd: Path | None = None) -> None:
    _require_string(
        _get_github_token(),
        "No GitHub token found (set GH_TOKEN, GITHUB_TOKEN, or run `gh auth login`)",
    )

def _fetch_copilot_models_raw(token: str) -> list[dict[str, Any]]:
    api = _openai(
        token,
        base_url=_COPILOT_BASE_URL,
        timeout=SHORT_HTTP_TIMEOUT,
        headers=_copilot_default_headers(),
    )
    data = _req_json(api, "GET", "/models", source="Copilot models")
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

def _copilot_completion_client(cwd: Path | None = None) -> CompletionClient:
    _ = cwd
    token = _require_string(_get_github_token(), "No GitHub token found")
    client = _openai(
        token,
        base_url=_COPILOT_BASE_URL,
        max_retries=0,
        headers=_copilot_default_headers(),
    )

    try:
        _, responses_models = _classify_copilot_models(token)
    except Exception:
        responses_models = set()

    responses_inner = _responses_client(
        lambda payload: _req_json(
            client,
            "POST",
            "/responses",
            source="Responses API",
            json_body=payload,
        ),
        lambda: _model_ids(client),
        fallback=None,
        default=None,
    )
    chat_inner = _chat_client(
        lambda payload: _req_json(
            client,
            "POST",
            "/chat/completions",
            source="Chat Completions API",
            json_body=payload,
        ),
        lambda: _model_ids(client),
        tools_map=_tool_specs_to_openai,
    )

    def chat_completion(
        model: str,
        messages: list[ChatMessage],
        tools: list[dict[str, Any]] | None = None,
        tool_choice: str = "auto",
        on_retry=None,
    ) -> AssistantMessage:
        inner = responses_inner if model in responses_models else chat_inner
        return inner["chat_completion"](
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
            return _list_models(lambda: _model_ids(client), fallback=None, default=[])


    return {"chat_completion": chat_completion, "list_models": list_models}

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

def _opencode_client(api_key: str, *, base_url: str) -> CompletionClient:
    return _chat_from_key(api_key, base_url=base_url)

def _opencode_zen_client(cwd: Path | None = None) -> CompletionClient:
    _ = cwd
    return _opencode_client(
        _require_string(get_opencode_zen_api_key(), "No OpenCode Zen credentials found"),
        base_url=OPENCODE_ZEN_URL,
    )

def _opencode_go_client(cwd: Path | None = None) -> CompletionClient:
    _ = cwd
    return _opencode_client(
        _require_string(get_opencode_go_api_key(), "No OpenCode Go credentials found"),
        base_url=OPENCODE_GO_URL,
    )

ShimEnvChecker: TypeAlias = Callable[[Path | None], None]
ShimClientBuilder: TypeAlias = Callable[..., CompletionClient]
ShimModelLister: TypeAlias = Callable[[Path | None], list[str]]

ShimSpec: TypeAlias = dict[str, Any]


def _client_model_lister(
    build_client: ShimClientBuilder,
    /,
    **kwargs: Any,
) -> ShimModelLister:
    def list_models(cwd: Path | None = None) -> list[str]:
        return build_client(cwd=cwd, **kwargs)["list_models"]()

    return list_models

def _make_shim_spec(
    name: str,
    *,
    ensure_env: ShimEnvChecker,
    build_client: ShimClientBuilder,
    list_models: ShimModelLister | None = None,
) -> ShimSpec:
    return {
        "name": name,
        "ensure_env": ensure_env,
        "build_client": build_client,
        "list_models": list_models or _client_model_lister(build_client),
    }

SHIM_SPECS: dict[str, ShimSpec] = {
    SHIM_OPENAI: _make_shim_spec(
        SHIM_OPENAI,
        ensure_env=_require_openai_env,
        build_client=_openai_client,
        list_models=_client_model_lister(_openai_client, max_retries=0),
    ),
    SHIM_CODEX: _make_shim_spec(
        SHIM_CODEX,
        ensure_env=_require_codex_env,
        build_client=_codex_client,
    ),
    SHIM_MANTLE: _make_shim_spec(
        SHIM_MANTLE,
        ensure_env=_require_aws_env,
        build_client=_mantle_completion_client,
        list_models=load_bedrock_model_list,
    ),
    SHIM_COPILOT: _make_shim_spec(
        SHIM_COPILOT,
        ensure_env=_require_copilot_env,
        build_client=_copilot_completion_client,
    ),
    SHIM_OPENCODE: _make_shim_spec(
        SHIM_OPENCODE,
        ensure_env=_require_opencode_zen_env,
        build_client=_opencode_zen_client,
    ),
    SHIM_OPENCODE_GO: _make_shim_spec(
        SHIM_OPENCODE_GO,
        ensure_env=_require_opencode_go_env,
        build_client=_opencode_go_client,
    ),
}

def _shim_spec(shim: str) -> ShimSpec:
    return SHIM_SPECS[validate_shim(shim)]

def _shim_env_error(spec: ShimSpec, cwd: Path | None) -> str | None:
    try:
        spec["ensure_env"](cwd)
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
    return shims[0] if shims else SHIM_OPENAI

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
    error = _shim_env_error(spec, cwd)
    return error is None, error

_MISSING_API_CREDENTIALS_MESSAGE = (
    "Missing API credentials.\n\n"
    "- set `OPENAI_API_KEY`, or\n"
    "- sign in with Codex CLI (`codex login`), or\n"
    "- authenticate GitHub CLI for Copilot (`gh auth login`), or\n"
    "- authenticate with OpenCode (`opencode auth`), or\n"
    "- for Bedrock Mantle: configure AWS CLI credentials / SSO and set `AWS_REGION` (or `AWS_DEFAULT_REGION`) for the target region"
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

def get_client(shim: str, cwd: Path | None = None) -> CompletionClient:
    return _shim_spec(shim)["build_client"](cwd)

def list_models_for_shim(
    shim: str,
    cwd: Path | None = None,
    *,
    ignore_errors: bool = True,
) -> list[str]:
    try:
        raw = _shim_spec(shim)["list_models"](cwd)
        return [join_model_spec(shim, model) for model in raw]
    except Exception:
        if ignore_errors:
            return []
        raise

__all__ = [
    "APIStatusError",
    "AuthenticationError",
    "BadRequestError",
    "AssistantMessage",
    "ChatMessage",
    "CompletionClient",
    "JSONLike",
    "ShimSpec",
    "SystemMessage",
    "ToolCall",
    "ToolMessage",
    "ToolResult",
    "UserMessage",
    "command_env",
    "detect_available_shims",
    "ensure_api_env",
    "get_client",
    "join_model_spec",
    "llm_session",
    "list_models_for_shim",
    "load_json",
    "normalize_jsonlike",
    "PermissionDeniedError",
    "RateLimitError",
    "require_api_env",
    "resolve_shim",
    "run_cmd",
    "save_json",
    "serialize_json",
    "serialize_toon",
    "split_model_spec",
    "tool_session",
    "validate_shim",
    "which",
]
