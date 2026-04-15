from __future__ import annotations
import base64
import json
import os
import re
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
    if isinstance(value, os.PathLike):
        return os.fspath(value)
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


def ToolCall(
    id: str, name: str, arguments: dict[str, Any] | None = None
) -> dict[str, Any]:
    return {"id": id, "name": name, "arguments": dict(arguments or {})}


def ToolResult(*, ok: bool = True, content: JSONLike = None) -> dict[str, Any]:
    return {"ok": ok, "content": content}


def _message(role: str, content: Any = "", /, **extra: Any) -> dict[str, Any]:
    return {"role": role, "content": content, **extra}


def SystemMessage(content: str) -> dict[str, Any]:
    return _message("system", content)


def UserMessage(content: str) -> dict[str, Any]:
    return _message("user", content)


def AssistantMessage(
    content: str = "",
    tool_calls: list[dict[str, Any]] | None = None,
    thought_signatures: dict[str, str] | None = None,
) -> dict[str, Any]:
    return _message(
        "assistant",
        content,
        tool_calls=list(tool_calls or []),
        thought_signatures=dict(thought_signatures or {}),
    )


def ToolMessage(
    tool_call_id: str,
    name: str = "",
    content: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return _message(
        "tool",
        content or ToolResult(),
        tool_call_id=tool_call_id,
        name=name,
    )


ChatMessage: TypeAlias = dict[str, Any]


CompletionClient: TypeAlias = dict[str, Any]


SHIM_OPENAI = "openai"
SHIM_CODEX = "codex"
SHIM_MANTLE = "bedrock-mantle"
SHIM_COPILOT = "copilot"
SHIM_OPENCODE = "opencode"
LOCAL_SHIM_PREFIX = "local-"
LOCAL_SHIM_PORTS = (8080, 11434)
SHIM_ORDER = (
    *(f"{LOCAL_SHIM_PREFIX}{port}" for port in LOCAL_SHIM_PORTS),
    SHIM_OPENAI,
    SHIM_CODEX,
    SHIM_MANTLE,
    SHIM_COPILOT,
    SHIM_OPENCODE,
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
OPENCODE_SHARED_ENV_VAR = "OPENCODE_API_KEY"
CODEX_CHATGPT_RESPONSES_URL = "https://chatgpt.com/backend-api/codex/responses"
CODEX_OAUTH_TOKEN_URL = "https://auth.openai.com/oauth/token"
# --- Public OAuth2 "installed app" credentials ---
# These are NOT confidential server secrets. OpenAI embeds client IDs in
# CLI binaries by design (RFC 8252 §8.5). Safe to publish in source code.
# Override via env: CODEX_OAUTH_CLIENT_ID
CODEX_OAUTH_CLIENT_ID = (
    os.environ.get("CODEX_OAUTH_CLIENT_ID") or "app_EMoamEEZ73f0CkXaXp7hrann"
)
DEFAULT_HTTP_TIMEOUT = 120.0
DEFAULT_WEBFETCH_TIMEOUT_SECONDS = 30.0
SHORT_HTTP_TIMEOUT = 15.0
DEFAULT_RETRY_MAX_ATTEMPTS = 10
DEFAULT_RETRY_INITIAL_DELAY_SECONDS = 5.0
DEFAULT_RETRY_MAX_DELAY_SECONDS = 30.0
TRANSPORT_ERROR_RETRY_DELAY = 3.0
MALFORMED_OUTPUT_RETRY_ATTEMPTS = 3
LOCAL_DEFAULT_BASE_URLS = {
    f"{LOCAL_SHIM_PREFIX}8080": "http://127.0.0.1:8080/v1",
    f"{LOCAL_SHIM_PREFIX}11434": "http://127.0.0.1:11434/v1",
}
LOCAL_SHIM_RE = re.compile(rf"^{re.escape(LOCAL_SHIM_PREFIX)}(?P<port>[0-9]+)$")


def _tool_output_value(result: dict[str, Any]) -> JSONLike:
    content = normalize_jsonlike(result.get("content"))
    if result.get("ok", True):
        return content
    if isinstance(content, dict):
        return {"ok": False, **content}
    return {"ok": False, "message": content}


def _tool_output_text(result: dict[str, Any]) -> str:
    return serialize_toon(_tool_output_value(result))


def load_json(path, default):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return default


def load_toon(path, default):
    try:
        return toons.loads(path.read_text(encoding="utf-8"))
    except (OSError, TypeError, ValueError):
        return default


def _load_json_object(path: Path) -> dict[str, Any]:
    data = load_json(path, {})
    return data if isinstance(data, dict) else {}


def _ensure_private_dir(path: Path) -> Path:
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.chmod(0o700)
    return path


def save_json(path, data):
    try:
        _ensure_private_dir(path.parent)
        path.write_text(
            json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        path.chmod(0o600)
        return True
    except OSError:
        return False


def save_toon(path, data):
    try:
        _ensure_private_dir(path.parent)
        payload = toons.dumps(normalize_jsonlike(data), indent=2)
        path.write_text(payload + "\n", encoding="utf-8")
        path.chmod(0o600)
        return True
    except (OSError, TypeError, ValueError):
        return False


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


def _require_json_object(value: Any, error: str) -> dict[str, Any]:
    if not isinstance(value, dict):
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
        return _response_json_object(response, f"{error_prefix}: invalid JSON response")
    except HTTPError as exc:
        raise RuntimeError(f"{error_prefix}: {exc}") from exc


def _extract_model_ids(items: Any, *keys: str) -> list[str]:
    if not isinstance(items, list):
        return []
    return list(
        dict.fromkeys(
            value
            for item in items
            if isinstance(item, dict)
            for value in [_first_nonempty_string(item, *keys)]
            if value
        )
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


class TransportError(RuntimeError): ...


class TimeoutException(TransportError): ...


class APIConnectionError(TransportError): ...


class APITimeoutError(TimeoutException): ...


class APIStatusError(HTTPError):
    def __init__(self, message: str, *, response: "ResponseAdapter", body: Any = None):
        self.body = body
        super().__init__(message, response=response)


class AuthenticationError(APIStatusError): ...


class PermissionDeniedError(APIStatusError): ...


class RateLimitError(APIStatusError): ...


class BadRequestError(APIStatusError): ...


class UnsupportedResponsesAPIError(RuntimeError): ...



ResponseAdapter: TypeAlias = dict[str, Any]


def _normalize_headers(headers: Any) -> dict[str, str]:
    if headers is None:
        return {}
    items = headers.items() if hasattr(headers, "items") else dict(headers).items()
    return {str(key).lower(): str(value) for key, value in items}


def _response_value(
    response: Any, key: str, default: Any = None, *, attr: str | None = None
) -> Any:
    if isinstance(response, dict):
        return response.get(key, default)
    return getattr(response, attr or key, default)


def adapt_response(response: Any) -> ResponseAdapter:
    status_code = int(_response_value(response, "status_code", 0) or 0)
    reason_phrase = str(
        _response_value(response, "reason_phrase", "", attr="reason")
        or _status_reason(status_code)
        or ""
    )
    return response_adapter(
        status_code=status_code,
        headers=_response_value(response, "headers"),
        text=str(_response_value(response, "text", "") or ""),
        content=bytes(_response_value(response, "content", b"") or b""),
        url=str(_response_value(response, "url", "") or ""),
        reason_phrase=reason_phrase,
        http_version=_http_version_name(_response_value(response, "http_version")),
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


def response_json(response: ResponseAdapter) -> Any:
    return json.loads(response["text"])


def _response_json_object(response: ResponseAdapter, error: str) -> dict[str, Any]:
    try:
        return _require_json_object(response_json(response), error)
    except Exception as exc:
        raise RuntimeError(error) from exc


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

    def __exit__(self, _exc_type, _exc, _tb) -> bool:
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
            "timeout": _http_request_timeout(
                self.timeout if timeout is None else float(timeout)
            ),
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
            reason_phrase=str(
                getattr(raw, "reason", None) or _status_reason(status_code)
            ),
            http_version=_http_version_name(
                getattr(raw, "version_string", None) or getattr(raw, "version", None)
            ),
        )


OpenAI: TypeAlias = dict[str, Any]


def llm_session(**kw):
    kw = dict(kw)
    kw.setdefault("timeout", DEFAULT_HTTP_TIMEOUT)
    if "follow_redirects" not in kw and "allow_redirects" not in kw:
        kw["follow_redirects"] = False
    return http_client(**kw)


def tool_session(**kw):
    kw = dict(kw)
    kw.setdefault("timeout", DEFAULT_WEBFETCH_TIMEOUT_SECONDS)
    if "follow_redirects" not in kw and "allow_redirects" not in kw:
        kw["follow_redirects"] = False
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
        "Content-Type": "application/json",
        **api["headers"],
    }
    if api["api_key"]:
        merged["Authorization"] = f"Bearer {api['api_key']}"
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
    response = _req(api, method, path, json_body=json_body, data=data, headers=headers)
    return _response_json_object(response, f"{source}: invalid JSON response")


def _openai_json_create(
    api: OpenAI, path: str, *, source: str
) -> Callable[[dict[str, Any]], dict[str, Any]]:
    return lambda payload: _req_json(
        api,
        "POST",
        path,
        source=source,
        json_body=payload,
    )


def _openai_model_lister(api: OpenAI) -> Callable[[], list[str]]:
    return lambda: _model_ids(api)


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


def _status_error_from_response(response: ResponseAdapter) -> APIStatusError:
    message = (
        _response_error_message(response)
        or response["reason_phrase"]
        or f"HTTP {response['status_code']}"
    )
    cls = {
        400: BadRequestError,
        401: AuthenticationError,
        403: PermissionDeniedError,
        429: RateLimitError,
    }.get(response["status_code"], APIStatusError)
    return cls(message, response=response)


def http_client(**kw):
    timeout = kw.pop("timeout", DEFAULT_HTTP_TIMEOUT)
    timeout = DEFAULT_HTTP_TIMEOUT if timeout is None else float(timeout)
    follow_redirects = bool(
        kw.pop("follow_redirects", kw.pop("allow_redirects", False))
    )
    if kw:
        raise TypeError(f"Unsupported http_client kwargs: {', '.join(sorted(kw))}")
    return HTTPClient(timeout=timeout, follow_redirects=follow_redirects)


def bedrock_base_url(region: str) -> str:
    return f"https://bedrock-mantle.{region}.api.aws/v1"


def load_bedrock_model_list(
    cwd: Path | None = None, region: str | None = None
) -> list[str]:
    current = default_region(region)
    url = f"{bedrock_base_url(current).rstrip('/')}/models"
    with llm_session(timeout=SHORT_HTTP_TIMEOUT, follow_redirects=False) as client:
        response = client.request(
            "GET",
            url,
            headers=_bedrock_request_headers(
                load_aws_credentials(cwd), current, "GET", url
            ),
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
    return _load_json_object(CODEX_AUTH_PATH)


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
    data = _load_json_object(CODEX_MODELS_CACHE_PATH)
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
        if shim in set(SHIM_ORDER) or _is_local_shim(shim):
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


class RetryableDecodeError(RuntimeError):
    pass


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
    if isinstance(exc, RetryableHttpError | APIStatusError):
        msg = _response_error_message(exc.response)
        return msg or f"HTTP {exc.response['status_code']}"
    if isinstance(exc, RetryableDecodeError):
        return str(exc)
    if isinstance(exc, TimeoutException):
        return f"timeout ({type(exc).__name__})"
    if isinstance(exc, TransportError):
        return f"transport error ({type(exc).__name__}): {exc}"
    return None if exc is None else str(exc)


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
            (
                APIConnectionError,
                APITimeoutError,
                RetryableHttpError,
                RetryableDecodeError,
            )
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
    try:
        payload = response_json(response)
    except Exception:
        payload = None
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
                items.append(
                    {"type": "message", "role": "assistant", "content": msg["content"]}
                )
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
    return {key: value for key, value in payload.items() if key != "reasoning"}


def _has_meaningful_assistant_output(message: AssistantMessage) -> bool:
    return bool(message["tool_calls"]) or not _is_blank_chat_value(message["content"])


def _retryable_decode_error(exc: Exception) -> RetryableDecodeError:
    return RetryableDecodeError(f"malformed model output: {exc}")


def _is_blank_chat_value(value: Any) -> bool:
    return (
        value is None
        or value == ""
        or value == []
        or value == {}
        or (isinstance(value, str) and not value.strip())
    )


def _decode_with_retry(call, decode, *, on_retry=None) -> AssistantMessage:
    def run() -> AssistantMessage:
        try:
            message = decode(call())
        except UnsupportedResponsesAPIError:
            raise
        except (json.JSONDecodeError, RuntimeError, TypeError, ValueError) as exc:
            raise _retryable_decode_error(exc) from exc
        if not _has_meaningful_assistant_output(message):
            raise RetryableDecodeError(
                "malformed model output: empty assistant message with no tool calls"
            )
        return message

    return _call_with_retry(
        run,
        max_attempts=MALFORMED_OUTPUT_RETRY_ATTEMPTS,
        on_retry=on_retry,
    )


_REASONING_SUPPORT_CACHE: dict[str, bool] = {}
_REASONING_CACHE_LOCK = _threading.Lock()


def _should_send_reasoning(model: str) -> bool:
    with _REASONING_CACHE_LOCK:
        return _REASONING_SUPPORT_CACHE.get(model, True)


def _mark_reasoning_unsupported(model: str) -> None:
    with _REASONING_CACHE_LOCK:
        _REASONING_SUPPORT_CACHE[model] = False


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


def _is_responses_unsupported_error(exc: APIStatusError) -> bool:
    status = exc.response["status_code"]
    if status not in {400, 404, 405, 415, 422, 501}:
        return False
    message = (_response_error_message(exc.response) or "").lower()
    return any(
        token in message
        for token in (
            "responses",
            "/responses",
            "unsupported api",
            "unsupported endpoint",
            "not found",
            "unknown path",
            "method not allowed",
        )
    )


def _unsupported_responses_api_error(
    model: str, exc: APIStatusError
) -> UnsupportedResponsesAPIError:
    message = _response_error_message(exc.response) or str(exc)
    return UnsupportedResponsesAPIError(
        f"Model {model!r} does not support the Open Responses / Responses API required by oy: {message}"
    )


def _call_responses(
    model: str,
    payload: dict[str, Any],
    create,
    *,
    on_retry=None,
):
    if not _should_send_reasoning(model):
        payload = _drop_reasoning_arg(payload)
    try:
        return _call_with_retry(lambda: create(payload), on_retry=on_retry)
    except APIStatusError as exc:
        if _is_reasoning_unsupported_error(exc):
            _mark_reasoning_unsupported(model)
            return _call_with_retry(
                lambda: create(_drop_reasoning_arg(payload)),
                on_retry=on_retry,
            )
        if _is_responses_unsupported_error(exc):
            raise _unsupported_responses_api_error(model, exc) from exc
        raise


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
        "reasoning": {"effort": "high"},
    }
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
        return f"{prefix} error {response['status_code']}: {body or response['reason_phrase']}"
    detail = data.get("error") or data.get("detail") if isinstance(data, dict) else data
    if isinstance(detail, dict):
        message = detail.get("message") or detail.get("code") or json.dumps(detail)
    elif isinstance(detail, str):
        message = detail
    else:
        message = json.dumps(detail, ensure_ascii=True)
    return f"{prefix} error {response['status_code']}: {message}"


def _chat_completions_messages(messages: list[ChatMessage]) -> list[dict[str, Any]]:
    items: list[dict[str, Any]] = []
    for msg in messages:
        role = msg.get("role")
        if role in {"system", "user"}:
            items.append({"role": role, "content": msg["content"]})
            continue
        if role == "assistant":
            item: dict[str, Any] = {"role": "assistant", "content": msg["content"] or ""}
            if msg["tool_calls"]:
                item["tool_calls"] = [
                    {
                        "id": call["id"],
                        "type": "function",
                        "function": {
                            "name": call["name"],
                            "arguments": serialize_json(call["arguments"]),
                        },
                    }
                    for call in msg["tool_calls"]
                ]
            items.append(item)
            continue
        if role == "tool":
            items.append(
                {
                    "role": "tool",
                    "tool_call_id": msg["tool_call_id"],
                    "content": _tool_output_text(msg["content"]),
                    "name": msg.get("name") or "",
                }
            )
    return items


def _chat_completions_tools(
    tools: list[dict[str, Any]] | None,
) -> list[dict[str, Any]] | None:
    result = [
        {
            "type": "function",
            "function": {
                "name": tool["name"],
                "description": tool["description"],
                "parameters": tool.get("parameters") or {"type": "object"},
            },
        }
        for tool in tools or []
    ]
    return result or None


def _responses_client(
    create: Callable[[dict[str, Any]], dict[str, Any]],
    list_models: Callable[[], list[str]],
) -> CompletionClient:
    def chat_completion(
        model: str,
        messages: list[ChatMessage],
        tools: list[dict[str, Any]] | None = None,
        tool_choice: str = "auto",
        on_retry=None,
    ) -> AssistantMessage:
        payload = _responses_payload(model, messages, tools, tool_choice)
        return _decode_with_retry(
            lambda: _call_responses(model, payload, create, on_retry=on_retry),
            _decode_responses_output,
            on_retry=on_retry,
        )

    return {
        "chat_completion": chat_completion,
        "list_models": list_models,
    }


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
                raise RuntimeError(
                    "Codex ChatGPT authentication failed after token refresh"
                )
            except APIStatusError as exc:
                raise RuntimeError(
                    _http_error_message("Codex ChatGPT", exc.response)
                ) from exc
            except HTTPError as exc:
                raise RuntimeError(f"Codex ChatGPT request failed: {exc}") from exc
            try:
                message = _decode_responses_output(data)
            except (json.JSONDecodeError, RuntimeError, TypeError, ValueError) as exc:
                if attempt == 0:
                    continue
                raise _retryable_decode_error(exc) from exc
            if _has_meaningful_assistant_output(message):
                return message
            if attempt == 0:
                continue
            raise RetryableDecodeError(
                "malformed model output: empty assistant message with no tool calls"
            )
        raise RuntimeError("Codex ChatGPT authentication failed after token refresh")

    return {
        "chat_completion": chat_completion,
        "list_models": load_codex_model_list,
    }


KNOWN_SHIMS = set(SHIM_ORDER)
_COPILOT_BASE_URL = os.environ.get("COPILOT_BASE_URL", "https://api.githubcopilot.com")
_COPILOT_INTEGRATION_ID = "copilot-developer-cli"
_COPILOT_EDITOR_VERSION = "copilot-developer-cli/1.0.6"


def _responses_from_key(
    api_key: str,
    *,
    base_url: str | None = None,
    max_retries: int = 3,
    timeout: Any = None,
) -> CompletionClient:
    api = _openai(
        api_key,
        base_url=base_url,
        max_retries=max_retries,
        timeout=timeout,
    )
    return _responses_client(
        _openai_json_create(api, "/responses", source="Responses API"),
        _openai_model_lister(api),
    )


def _require_openai_env(_cwd: Path | None = None) -> None:
    _require_string(os.environ.get("OPENAI_API_KEY"), "OPENAI_API_KEY is not set")


def _openai_client(
    _cwd: Path | None = None,
    *,
    max_retries: int = 3,
) -> CompletionClient:
    return _responses_from_key(
        _require_string(
            os.environ.get("OPENAI_API_KEY"), "No OpenAI credentials found"
        ),
        base_url=os.environ.get("OPENAI_BASE_URL"),
        max_retries=max_retries,
    )


def _require_codex_env(_cwd: Path | None = None) -> None:
    load_codex_session()


def _codex_client(_cwd: Path | None = None) -> CompletionClient:
    api_key = load_codex_auth().get("OPENAI_API_KEY")
    if isinstance(api_key, str) and api_key:
        return _responses_from_key(api_key)
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
        url = f"{api['base_url'].rstrip('/')}/responses"
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
        return _response_json_object(response, "Responses API: invalid JSON response")

    return _responses_client(create, lambda: load_bedrock_model_list(None, region))


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


def _require_copilot_env(_cwd: Path | None = None) -> None:
    _require_string(
        _get_github_token(),
        "No GitHub token found (set GH_TOKEN, GITHUB_TOKEN, or run `gh auth login`)",
    )


def _copilot_completion_client(_cwd: Path | None = None) -> CompletionClient:
    token = _require_string(_get_github_token(), "No GitHub token found")
    return _responses_from_key(
        token,
        base_url=_COPILOT_BASE_URL,
        max_retries=0,
    )


def _load_opencode_auth() -> dict[str, Any]:
    return _load_json_object(OPENCODE_AUTH_PATH)


def _opencode_api_key(name: str) -> str | None:
    if env_key := _first_nonempty_string(os.environ, OPENCODE_SHARED_ENV_VAR):
        return env_key
    auth = _load_opencode_auth()
    for candidate in dict.fromkeys((name, SHIM_OPENCODE)):
        entry = auth.get(candidate)
        if isinstance(entry, dict):
            if api_key := _first_nonempty_string(entry, "key"):
                return api_key
    return None


def _require_opencode_env(name: str, label: str) -> None:
    _require_string(
        _opencode_api_key(name),
        f"No OpenCode {label} credentials found in {OPENCODE_AUTH_PATH} or ${OPENCODE_SHARED_ENV_VAR}",
    )


def _require_opencode_zen_env(_cwd: Path | None = None) -> None:
    _require_opencode_env("opencode", "Zen")


def _opencode_list_models(name: str, provider_id: str, base_url: str) -> list[str]:
    _ = provider_id, base_url
    api = _openai(
        _require_string(_opencode_api_key(name), "No OpenCode credentials found"),
        base_url=OPENCODE_ZEN_URL,
        max_retries=0,
        timeout=SHORT_HTTP_TIMEOUT,
    )
    return _model_ids(api)


def _opencode_client(
    name: str,
    label: str,
    provider_id: str,
    base_url: str,
) -> CompletionClient:
    _ = provider_id
    client = _responses_from_key(
        _require_string(
            _opencode_api_key(name), f"No OpenCode {label} credentials found"
        ),
        base_url=base_url,
    )
    client["list_models"] = lambda: _opencode_list_models(name, provider_id, base_url)
    return client


def _opencode_zen_client(_cwd: Path | None = None) -> CompletionClient:
    return _opencode_client("opencode", "Zen", SHIM_OPENCODE, OPENCODE_ZEN_URL)


def _local_shim_port(shim: str) -> int:
    match = LOCAL_SHIM_RE.fullmatch(shim)
    if not match:
        raise RuntimeError(f"Unknown shim value: `{shim}`. Use one of: {', '.join(SHIM_ORDER)}")
    return int(match.group("port"))


def _is_local_shim(shim: str | None) -> bool:
    return isinstance(shim, str) and LOCAL_SHIM_RE.fullmatch(shim) is not None


def _local_base_url(shim: str, cwd: Path | None = None) -> str:
    _ = cwd
    port = _local_shim_port(shim)
    env_name = f"OY_LOCAL_{port}_URL"
    value = os.environ.get(env_name)
    if isinstance(value, str) and value.strip():
        return value.rstrip("/")
    return LOCAL_DEFAULT_BASE_URLS.get(shim, f"http://127.0.0.1:{port}/v1")


def _local_api(shim: str, cwd: Path | None = None) -> OpenAI:
    return _openai(
        os.environ.get("LOCAL_API_KEY") or os.environ.get("OPENAI_API_KEY") or "",
        base_url=_local_base_url(shim, cwd),
        max_retries=0,
        timeout=DEFAULT_HTTP_TIMEOUT,
    )


def _require_local_env(cwd: Path | None = None, *, shim: str) -> None:
    api = _openai(
        os.environ.get("LOCAL_API_KEY") or os.environ.get("OPENAI_API_KEY") or "",
        base_url=_local_base_url(shim, cwd),
        max_retries=0,
        timeout=SHORT_HTTP_TIMEOUT,
    )
    _model_ids(api)


def _local_list_models(cwd: Path | None = None, *, shim: str) -> list[str]:
    return _model_ids(
        _openai(
            os.environ.get("LOCAL_API_KEY") or os.environ.get("OPENAI_API_KEY") or "",
            base_url=_local_base_url(shim, cwd),
            max_retries=0,
            timeout=SHORT_HTTP_TIMEOUT,
        )
    )


def _local_current_model(
    shim: str, model_spec: str | None = None, env: dict[str, str] | None = None
) -> str:
    data = env or os.environ
    spec = model_spec or _first_nonempty_string(data, "OY_MODEL")
    if not spec:
        raise RuntimeError(f"{shim} model is not configured")
    prefix, model = split_model_spec(spec)
    if prefix == shim and model:
        return model
    if model_spec and prefix != shim:
        return model_spec
    raise RuntimeError(f"{shim} model is not configured")


def _local_client(
    cwd: Path | None = None, *, model_spec: str | None = None, shim: str
) -> CompletionClient:
    configured_model = _local_current_model(shim, model_spec) if model_spec else None

    def create(payload: dict[str, Any]) -> dict[str, Any]:
        model = str(payload.get("model") or configured_model or _local_current_model(shim))
        api = _local_api(shim, cwd)
        request_payload = dict(payload)
        request_payload["model"] = model
        return _req_json(
            api,
            "POST",
            "/responses",
            source=f"{shim} responses",
            json_body=request_payload,
        )

    return _responses_client(create, lambda: _local_list_models(cwd, shim=shim))


ShimSpec: TypeAlias = dict[str, Any]


def _client_model_lister(
    build_client: Callable[..., CompletionClient], /, **kwargs: Any
):
    def list_models(cwd: Path | None = None) -> list[str]:
        return build_client(cwd, **kwargs)["list_models"]()

    return list_models


def _make_shim_spec(
    *,
    ensure_env: Callable[[Path | None], None],
    build_client: Callable[..., CompletionClient],
    list_models: Callable[[Path | None], list[str]] | None = None,
) -> ShimSpec:
    return {
        "ensure_env": ensure_env,
        "build_client": build_client,
        "list_models": list_models or _client_model_lister(build_client),
    }


SHIM_SPECS: dict[str, ShimSpec] = {
    SHIM_OPENAI: _make_shim_spec(
        ensure_env=_require_openai_env,
        build_client=_openai_client,
        list_models=_client_model_lister(_openai_client, max_retries=0),
    ),
    SHIM_CODEX: _make_shim_spec(
        ensure_env=_require_codex_env,
        build_client=_codex_client,
    ),
    SHIM_MANTLE: _make_shim_spec(
        ensure_env=_require_aws_env,
        build_client=_mantle_completion_client,
        list_models=load_bedrock_model_list,
    ),
    SHIM_COPILOT: _make_shim_spec(
        ensure_env=_require_copilot_env,
        build_client=_copilot_completion_client,
    ),
    SHIM_OPENCODE: _make_shim_spec(
        ensure_env=_require_opencode_zen_env,
        build_client=_opencode_zen_client,
    ),
}


def _local_shim_spec(shim: str) -> ShimSpec:
    validate_shim(shim)
    return _make_shim_spec(
        ensure_env=lambda cwd: _require_local_env(cwd, shim=shim),
        build_client=lambda cwd, *, model_spec=None: _local_client(
            cwd, model_spec=model_spec, shim=shim
        ),
        list_models=lambda cwd: _local_list_models(cwd, shim=shim),
    )


def _shim_spec(shim: str) -> ShimSpec:
    value = validate_shim(shim)
    return _local_shim_spec(value) if _is_local_shim(value) else SHIM_SPECS[value]


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
        if shim := _validated_or_none(env_shim):
            return shim
    if model_spec:
        prefix, _ = split_model_spec(model_spec)
        if prefix:
            return prefix
    if shim := _validated_or_none(configured_shim):
        return shim
    shims = detect_available_shims()
    return shims[0] if shims else SHIM_OPENAI


def validate_shim(shim: str) -> str:
    if shim in KNOWN_SHIMS or _is_local_shim(shim):
        return shim
    raise RuntimeError(
        f"Unknown shim value: `{shim}`. Use one of: {', '.join(SHIM_ORDER)} or {LOCAL_SHIM_PREFIX}<port>"
    )


def _validated_or_none(shim: str | None) -> str | None:
    if not isinstance(shim, str) or not shim:
        return None
    try:
        return validate_shim(shim)
    except RuntimeError:
        return None


def ensure_api_env(
    model_spec: str | None = None,
    configured_shim: str | None = None,
    cwd: Path | None = None,
) -> tuple[bool, str | None]:
    spec = _shim_spec(resolve_shim(model_spec, configured_shim))
    error = _shim_env_error(spec, cwd)
    return error is None, error


_MISSING_API_CREDENTIALS_MESSAGE = (
    "Missing API credentials. oy targets providers that support the Open Responses / OpenAI Responses API.\n\n"
    "- use a local OpenAI-compatible server on `127.0.0.1:8080` or `127.0.0.1:11434`, or\n"
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
        if _is_local_shim(shim):
            raise RuntimeError(error)
        raise RuntimeError(_missing_api_credentials_message(error))
    return shim


def get_client(
    shim: str, cwd: Path | None = None, model_spec: str | None = None
) -> CompletionClient:
    build_client = _shim_spec(shim)["build_client"]
    return build_client(cwd, model_spec=model_spec) if _is_local_shim(shim) else build_client(cwd)


def list_models_for_shim(shim: str, cwd: Path | None = None) -> list[str]:
    raw = _shim_spec(shim)["list_models"](cwd)
    return [join_model_spec(shim, model) for model in raw]


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
    "load_toon",
    "normalize_jsonlike",
    "PermissionDeniedError",
    "RateLimitError",
    "require_api_env",
    "resolve_shim",
    "run_cmd",
    "save_json",
    "save_toon",
    "serialize_json",
    "serialize_toon",
    "split_model_spec",
    "tool_session",
    "validate_shim",
    "which",
]
