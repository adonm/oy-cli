from __future__ import annotations

import base64
import hashlib
import hmac
import json
import os
import random
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from functools import lru_cache
from pathlib import Path
from typing import Any
from urllib.parse import quote
import httpx
import httpx_aws_auth
from openai import AsyncOpenAI, OpenAI
from tenacity import AsyncRetrying, retry_if_exception_type, stop_after_attempt
from tenacity.wait import wait_base


SHIM_OPENAI = "openai"
SHIM_CODEX = "codex"
SHIM_GEMINI = "gemini"
SHIM_BEDROCK = "bedrock"
SHIM_MANTLE = "bedrock-mantle"
SHIM_CLAUDE = "claude"
KNOWN_SHIMS = {
    SHIM_OPENAI,
    SHIM_CODEX,
    SHIM_GEMINI,
    SHIM_BEDROCK,
    SHIM_MANTLE,
    SHIM_CLAUDE,
}

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
GEMINI_OAUTH_CLIENT_ID = "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com"
GEMINI_OAUTH_CLIENT_SECRET = "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"
CLAUDE_API_URL = "https://api.anthropic.com"
CLAUDE_TOKEN_URL = "https://platform.claude.com/v1/oauth/token"
CLAUDE_OAUTH_CLIENT_ID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
ANTHROPIC_VERSION = "2023-06-01"
DEFAULT_RETRY_MAX_ATTEMPTS = 10
DEFAULT_RETRY_INITIAL_DELAY_SECONDS = 5.0
DEFAULT_RETRY_MAX_DELAY_SECONDS = 30.0


def load_json(path: Path, default: Any) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return default


def save_json(path: Path, data: Any) -> bool:
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return True
    except OSError:
        return False


def split_path(value: str | None) -> list[str]:
    return [entry for entry in (value or "").split(os.pathsep) if entry]


def merge_paths(*groups: list[str]) -> str:
    merged: list[str] = []
    seen: set[str] = set()
    for group in groups:
        for entry in group:
            key = os.path.normcase(os.path.normpath(entry))
            if entry and key not in seen:
                seen.add(key)
                merged.append(entry)
    return os.pathsep.join(merged)


def unique_strings(values: list[str]) -> list[str]:
    seen: set[str] = set()
    result: list[str] = []
    for value in values:
        if value and value not in seen:
            seen.add(value)
            result.append(value)
    return result


def expiry_ms(expires_in_seconds: Any, *, skew_seconds: int = 60) -> int:
    import time

    try:
        seconds = float(expires_in_seconds)
    except (TypeError, ValueError):
        seconds = 3600.0
    return int((time.time() + seconds - skew_seconds) * 1000)


def which(tool: str, path_value: str | None = None) -> str | None:
    return shutil.which(tool, path=path_value)


def run_cmd(
    command: list[str],
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    timeout: int = 120,
    stdin_text: str | None = None,
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            command,
            cwd=cwd,
            env=env,
            input=stdin_text,
            text=True,
            capture_output=True,
            timeout=max(timeout, 1),
        )
    except subprocess.TimeoutExpired as exc:
        raise ValueError(f"command timed out after {timeout} seconds") from exc



def command_env(cwd: Path | None = None) -> dict[str, str]:
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


def http_client(**kwargs: Any) -> httpx.Client:
    return httpx.Client(follow_redirects=True, **kwargs)

def async_http_client(**kwargs: Any) -> httpx.AsyncClient:
    return httpx.AsyncClient(follow_redirects=True, **kwargs)


def _sigv4_sign(key: bytes, msg: str) -> bytes:
    return hmac.new(key, msg.encode("utf-8"), hashlib.sha256).digest()


def _sigv4_key(secret_key: str, date_stamp: str, region: str, service: str) -> bytes:
    key = _sigv4_sign(("AWS4" + secret_key).encode("utf-8"), date_stamp)
    for part in (region, service, "aws4_request"):
        key = _sigv4_sign(key, part)
    return key


def bedrock_base_url(region: str) -> str:
    return f"https://bedrock-mantle.{region}.api.aws/v1"


def make_bedrock_token(region: str, cwd: Path | None = None, expires: int = 43200) -> str:
    """Generate a Bedrock bearer token using AWS SigV4 signing.

    Takes AWS credentials and produces a token suitable for the Bedrock Mantle API.
    Token is valid for `expires` seconds (default 12 hours).
    """
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


def resolve_tool_path(tool: str, cwd: Path | None = None) -> str | None:
    env = command_env(cwd)
    return which(tool, env.get("PATH")) or which(tool)


def aws_cli(parts: list[str], cwd: Path | None = None, timeout: int = 10):
    env = command_env(cwd)
    if not (aws := which("aws", env.get("PATH"))):
        raise RuntimeError("AWS CLI is not installed or not on PATH")
    return run_cmd([aws, *parts], cwd=cwd, env=env, timeout=timeout)


def run_aws_sso_login(cwd: Path | None = None) -> None:
    env = command_env(cwd)
    if not (aws := which("aws", env.get("PATH"))):
        raise RuntimeError("AWS CLI is not installed or not on PATH")
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise RuntimeError(
            "AWS SSO session is stale. Run `aws sso login --use-device-code --no-browser` and retry."
        )
    result = run_cmd(
        [
            aws,
            "sso",
            "login",
            "--use-device-code",
            "--no-browser",
            "--no-cli-pager",
        ],
        cwd=cwd,
        env=env,
        timeout=300,
    )
    if result.returncode:
        raise RuntimeError(f"AWS SSO login failed with exit code {result.returncode}")


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
    return current_region(None)


def validate_shim(shim: str) -> str:
    if shim not in KNOWN_SHIMS:
        raise RuntimeError(
            f"Unknown shim value: `{shim}`. Use one of: {', '.join(sorted(KNOWN_SHIMS))}"
        )
    return shim


def ensure_api_env(
    model_spec: str | None = None,
    configured_shim: str | None = None,
    cwd: Path | None = None,
) -> tuple[bool, str | None]:
    shim = validate_shim(resolve_shim(model_spec, configured_shim))
    if shim == SHIM_OPENAI:
        if get_openai_api_key():
            return True, None
        return False, "OPENAI_API_KEY is not set"
    if shim == SHIM_CODEX:
        if has_codex_credentials():
            return True, None
        return False, "Codex CLI credentials were not found in ~/.codex/auth.json"
    if shim == SHIM_GEMINI:
        try:
            load_gemini_oauth_creds()
            return True, None
        except RuntimeError as exc:
            return False, str(exc)
    if shim == SHIM_CLAUDE:
        try:
            load_claude_auth_status()
            return True, None
        except RuntimeError as exc:
            return False, str(exc)
    try:
        current_region(None)
        if shim == SHIM_BEDROCK:
            load_aws_credentials(cwd, allow_login=False)
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
    path = root / "node_modules" / "@google" / "gemini-cli-core" / "dist" / "src" / relative
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        raise RuntimeError(f"Cannot read Gemini CLI file {path}: {exc}") from exc


@lru_cache(maxsize=1)
def load_gemini_oauth_client() -> tuple[str, str]:
    client_id = os.environ.get("GEMINI_OAUTH_CLIENT_ID") or GEMINI_OAUTH_CLIENT_ID
    client_secret = os.environ.get("GEMINI_OAUTH_CLIENT_SECRET") or GEMINI_OAUTH_CLIENT_SECRET
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


def gemini_creds_path() -> Path:
    return GEMINI_CREDS_PATH


def load_gemini_oauth_creds() -> dict[str, Any]:
    path = gemini_creds_path()
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"Cannot read Gemini OAuth credentials: {exc}") from exc
    if not isinstance(data.get("refresh_token"), str):
        raise RuntimeError("Gemini OAuth credentials missing refresh_token")
    return data


def refresh_gemini_token(refresh_token: str) -> str:
    client_id, client_secret = load_gemini_oauth_client()
    payload = {
        "client_id": client_id,
        "client_secret": client_secret,
        "refresh_token": refresh_token,
        "grant_type": "refresh_token",
    }
    try:
        with http_client(timeout=15) as client:
            resp = client.post(
                "https://oauth2.googleapis.com/token",
                data=payload,
                headers={"Content-Type": "application/x-www-form-urlencoded"},
            )
            resp.raise_for_status()
            data = resp.json()
    except httpx.HTTPError as exc:
        raise RuntimeError(f"Gemini token refresh failed: {exc}") from exc
    if not isinstance(data.get("access_token"), str):
        raise RuntimeError("Gemini token refresh did not return an access_token")
    existing = load_json(gemini_creds_path(), {})
    if isinstance(existing, dict):
        existing.update(
            {
                "access_token": data["access_token"],
                "expiry_date": expiry_ms(data.get("expires_in", 3600)),
            }
        )
        save_json(gemini_creds_path(), existing)
    return data["access_token"]


def get_gemini_access_token() -> str:
    import time

    creds = load_gemini_oauth_creds()
    expires_at_ms = creds.get("expiry_date", 0)
    access_token = creds.get("access_token")
    if (
        isinstance(access_token, str)
        and access_token
        and isinstance(expires_at_ms, (int, float))
        and expires_at_ms > time.time() * 1000
    ):
        return access_token
    return refresh_gemini_token(creds["refresh_token"])


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


def save_codex_auth(data: dict[str, Any]) -> None:
    save_json(CODEX_AUTH_PATH, data)


def get_openai_api_key() -> str | None:
    return os.environ.get("OPENAI_API_KEY")


def has_openai_credentials() -> bool:
    return bool(os.environ.get("OPENAI_API_KEY"))


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
    try:
        with http_client(timeout=15) as client:
            resp = client.post(
                CODEX_OAUTH_TOKEN_URL,
                data={
                    "grant_type": "refresh_token",
                    "refresh_token": refresh_token,
                    "client_id": CODEX_OAUTH_CLIENT_ID,
                },
                headers={"Content-Type": "application/x-www-form-urlencoded"},
            )
            resp.raise_for_status()
            data = resp.json()
    except httpx.HTTPError as exc:
        raise RuntimeError(f"Codex token refresh failed: {exc}") from exc
    if not isinstance(data.get("access_token"), str):
        raise RuntimeError("Codex token refresh did not return an access_token")
    auth = load_codex_auth()
    tokens = auth.get("tokens")
    if not isinstance(tokens, dict):
        tokens = {}
    tokens["access_token"] = data["access_token"]
    if isinstance(data.get("refresh_token"), str) and data.get("refresh_token"):
        tokens["refresh_token"] = data["refresh_token"]
    if isinstance(data.get("id_token"), str) and data.get("id_token"):
        tokens["id_token"] = data["id_token"]
    auth["tokens"] = tokens
    auth["last_refresh"] = datetime.now(timezone.utc).isoformat()
    save_codex_auth(auth)
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
    if force_refresh or not access_token or (expiry is not None and expiry <= datetime.now(timezone.utc).timestamp() + 60):
        refreshed = refresh_codex_chatgpt_session(refresh_token)
        tokens = _codex_tokens(refreshed)
        access_token = tokens.get("access_token")
        account_id = tokens.get("account_id")
    if not access_token or not account_id:
        raise RuntimeError("Codex ChatGPT session is missing access token or account ID")
    return {
        "access_token": access_token,
        "refresh_token": refresh_token,
        "account_id": account_id,
    }


def has_codex_credentials() -> bool:
    try:
        load_codex_session()
    except RuntimeError:
        return False
    return True


def load_codex_model_list() -> list[str]:
    data = load_json(CODEX_MODELS_CACHE_PATH, {})
    items = data.get("models") if isinstance(data, dict) else None
    if not isinstance(items, list):
        return []
    models: list[str] = []
    for item in items:
        if not isinstance(item, dict):
            continue
        model_id = next(
            (
                item.get(key)
                for key in ("id", "name", "slug", "model", "model_id")
                if isinstance(item.get(key), str) and item.get(key)
            ),
            None,
        )
        if model_id:
            models.append(model_id)
    return unique_strings(models)


def load_claude_auth_status() -> dict[str, Any]:
    """Load Claude authentication status from the credentials file."""
    creds = load_json(CLAUDE_CREDS_PATH, {})
    oauth = creds.get("claudeAiOauth", {})
    if not isinstance(oauth, dict) or not isinstance(oauth.get("accessToken"), str):
        raise RuntimeError(
            "Claude Code is not logged in. Run `claude` to authenticate."
        )
    return {"loggedIn": True, "oauth": oauth}


def has_claude_credentials() -> bool:
    try:
        load_claude_auth_status()
    except RuntimeError:
        return False
    return True


def get_claude_access_token() -> str:
    """Return a valid Claude access token, refreshing if expired."""
    import time

    creds = load_json(CLAUDE_CREDS_PATH, {})
    oauth = creds.get("claudeAiOauth", {})
    if not isinstance(oauth, dict):
        raise RuntimeError("Claude credentials not found. Run `claude` to authenticate.")
    access_token = oauth.get("accessToken")
    if not isinstance(access_token, str) or not access_token:
        raise RuntimeError("Claude credentials missing accessToken. Run `claude` to authenticate.")
    expires_at_ms = oauth.get("expiresAt", 0)
    if isinstance(expires_at_ms, (int, float)) and expires_at_ms > time.time() * 1000 + 30_000:
        return access_token
    refresh_token = oauth.get("refreshToken")
    if not isinstance(refresh_token, str) or not refresh_token:
        raise RuntimeError(
            "Claude session expired and no refresh token available. Run `claude` to re-authenticate."
        )
    return _refresh_claude_token(refresh_token)


def _refresh_claude_token(refresh_token: str) -> str:
    """Refresh the Claude OAuth access token and persist updated credentials."""
    import time

    try:
        with http_client(timeout=15) as client:
            resp = client.post(
                CLAUDE_TOKEN_URL,
                data={
                    "grant_type": "refresh_token",
                    "refresh_token": refresh_token,
                    "client_id": CLAUDE_OAUTH_CLIENT_ID,
                },
                headers={"Content-Type": "application/x-www-form-urlencoded"},
            )
            resp.raise_for_status()
            data = resp.json()
    except httpx.HTTPError as exc:
        raise RuntimeError(f"Claude token refresh failed: {exc}") from exc
    if not isinstance(data.get("access_token"), str):
        raise RuntimeError("Claude token refresh did not return an access_token")
    existing = load_json(CLAUDE_CREDS_PATH, {})
    if isinstance(existing, dict) and isinstance(existing.get("claudeAiOauth"), dict):
        existing["claudeAiOauth"]["accessToken"] = data["access_token"]
        existing["claudeAiOauth"]["expiresAt"] = expiry_ms(data.get("expires_in", 3600))
        if isinstance(data.get("refresh_token"), str):
            existing["claudeAiOauth"]["refreshToken"] = data["refresh_token"]
        save_json(CLAUDE_CREDS_PATH, existing)
    return data["access_token"]


def get_codex_openai_clients() -> tuple[AsyncOpenAI, OpenAI]:
    """Return AsyncOpenAI and OpenAI clients using credentials from ~/.codex/auth.json."""
    api_key = get_codex_api_key()
    if not api_key:
        raise RuntimeError("No Codex API key found in ~/.codex/auth.json")
    return (
        AsyncOpenAI(api_key=api_key, max_retries=3),
        OpenAI(api_key=api_key, max_retries=3),
    )


def split_model_spec(spec: str) -> tuple[str | None, str]:
    if ":" in spec:
        shim, _, model = spec.partition(":")
        if shim in KNOWN_SHIMS:
            return shim, model
    return None, spec


def join_model_spec(shim: str, model: str) -> str:
    return f"{shim}:{model}"


class CompletionClient:
    async def chat_completion(
        self,
        model: str,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        tool_choice: str = "auto",
    ) -> dict[str, Any]:
        raise NotImplementedError

    def list_models(self) -> list[str]:
        raise NotImplementedError


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
    ):
        self.initial = initial
        self.maximum = maximum

    def __call__(self, retry_state) -> float:
        attempt = max(retry_state.attempt_number, 1)
        base = min(self.maximum, self.initial * (2 ** max(attempt - 1, 0)))
        exc = retry_state.outcome.exception() if retry_state.outcome else None
        if isinstance(exc, RetryableHttpError):
            retry_after_seconds = _parse_retry_after_seconds(
                exc.response.headers.get("retry-after")
            )
            google_delay = _google_retry_delay_seconds(exc.response)
            chosen = max(
                base,
                retry_after_seconds or 0.0,
                google_delay or 0.0,
            )
            return min(self.maximum, chosen)
        return base


async def _send_with_retry(
    send,
    *,
    max_attempts: int = DEFAULT_RETRY_MAX_ATTEMPTS,
    should_retry_response=None,
) -> httpx.Response:
    async for attempt in AsyncRetrying(
        stop=stop_after_attempt(max_attempts),
        wait=WaitForRetryableResponse(),
        retry=retry_if_exception_type((httpx.TransportError, RetryableHttpError)),
        reraise=True,
    ):
        with attempt:
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


def _response_error_message(response: httpx.Response) -> str:
    payload = _response_json(response)
    if isinstance(payload, dict):
        error = payload.get("error")
        if isinstance(error, dict) and isinstance(error.get("message"), str):
            return error["message"]
    return response.text


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


def _stringify_message_content(content: Any) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts: list[str] = []
        for item in content:
            if isinstance(item, dict):
                if isinstance(item.get("text"), str):
                    parts.append(item["text"])
                    continue
                if isinstance(item.get("refusal"), str):
                    parts.append(item["refusal"])
                    continue
            parts.append(json.dumps(item, ensure_ascii=True))
        return "\n".join(part for part in parts if part)
    if content is None:
        return ""
    return json.dumps(content, ensure_ascii=True)


def _responses_instructions(messages: list[dict[str, Any]]) -> str | None:
    parts = [
        _stringify_message_content(msg.get("content"))
        for msg in messages
        if msg.get("role") in {"system", "developer"}
    ]
    joined = "\n\n".join(part for part in parts if part)
    return joined or None


def _content_message(role: str, content: Any) -> dict[str, Any]:
    return {
        "type": "message",
        "role": role,
        "content": _stringify_message_content(content),
    }


def _function_call_item(call_id: str, name: str, arguments: Any, *, status: str | None = None) -> dict[str, Any]:
    item = {
        "type": "function_call",
        "call_id": call_id,
        "name": name,
        "arguments": _json_arguments(arguments),
    }
    if status:
        item["status"] = status
    return item


def _function_output_item(call_id: str, content: Any) -> dict[str, Any]:
    return {
        "type": "function_call_output",
        "call_id": call_id,
        "output": _stringify_message_content(content),
    }


def _responses_input_from_messages(messages: list[dict[str, Any]]) -> list[dict[str, Any]]:
    items: list[dict[str, Any]] = []
    for msg in messages:
        role = msg.get("role")
        match role:
            case "system" | "developer":
                continue
            case "user":
                items.append(_content_message(role, msg.get("content")))
            case "assistant":
                if text := _stringify_message_content(msg.get("content")):
                    items.append(_content_message("assistant", text))
                for call in msg.get("tool_calls") or []:
                    function = call.get("function")
                    call_id = call.get("id")
                    if isinstance(function, dict) and isinstance(call_id, str) and call_id:
                        items.append(
                            _function_call_item(
                                call_id,
                                function.get("name") or "",
                                function.get("arguments"),
                                status="completed",
                            )
                        )
            case "tool":
                if isinstance(msg.get("tool_call_id"), str) and msg["tool_call_id"]:
                    items.append(_function_output_item(msg["tool_call_id"], msg.get("content")))
    return items


def _iter_function_tools(tools: list[dict[str, Any]] | None):
    for tool in tools or []:
        if tool.get("type") != "function":
            continue
        function = tool.get("function")
        if not isinstance(function, dict):
            continue
        name = function.get("name")
        if not isinstance(name, str) or not name:
            continue
        yield function


def _responses_tools(tools: list[dict[str, Any]] | None) -> list[dict[str, Any]] | None:
    result = [
        {
            "type": "function",
            "name": function["name"],
            "description": function.get("description"),
            "parameters": function.get("parameters") or {"type": "object"},
            "strict": False,
        }
        for function in _iter_function_tools(tools)
    ]
    return result or None


def _json_arguments(arguments: Any) -> str:
    return arguments if isinstance(arguments, str) else json.dumps(arguments or {}, ensure_ascii=True)


def _openai_tool_call(call_id: str, name: str, arguments: Any) -> dict[str, Any]:
    return {
        "id": call_id,
        "type": "function",
        "function": {"name": name, "arguments": _json_arguments(arguments)},
    }


def _text_parts(content: Any) -> list[str]:
    if isinstance(content, str):
        return [content] if content else []
    if not isinstance(content, list):
        return []
    return [
        item["text"]
        for item in content
        if isinstance(item, dict) and item.get("type") == "text" and item.get("text")
    ]


def _responses_payload(
    model: str,
    messages: list[dict[str, Any]],
    tools: list[dict[str, Any]] | None,
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


def _responses_output_to_message(response: Any) -> dict[str, Any]:
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
                if isinstance(part.get("text"), str):
                    content_parts.append(part["text"])
                elif isinstance(part.get("refusal"), str):
                    content_parts.append(part["refusal"])
        elif item_type == "function_call":
            call_id = item.get("call_id") or item.get("id")
            if not isinstance(call_id, str) or not call_id:
                continue
            arguments = item.get("arguments")
            if not isinstance(arguments, str):
                arguments = json.dumps(arguments or {}, ensure_ascii=True)
            tool_calls.append(_openai_tool_call(call_id, item.get("name") or "", arguments))
    if not content_parts and isinstance(data.get("output_text"), str):
        content_parts.append(data["output_text"])
    message: dict[str, Any] = {
        "role": "assistant",
        "content": "\n\n".join(part for part in content_parts if part),
    }
    if tool_calls:
        message["tool_calls"] = tool_calls
    return message


def _http_error_message(prefix: str, response: httpx.Response) -> str:
    try:
        data = response.json()
    except ValueError:
        body = response.text.strip()
        body = body[:200] if body else ""
        return f"{prefix} error {response.status_code}: {body or response.reason_phrase}"
    detail = (
        data.get("error") or data.get("detail")
        if isinstance(data, dict)
        else data
    )
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


class OpenAIApiShim(CompletionClient):
    def __init__(self, async_client: AsyncOpenAI, sync_client: OpenAI):
        self.async_client = async_client
        self.sync_client = sync_client

    async def chat_completion(
        self,
        model: str,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        tool_choice: str = "auto",
    ) -> dict[str, Any]:
        response = await self.async_client.responses.create(
            **_responses_payload(model, messages, tools, tool_choice)
        )
        return _responses_output_to_message(response)

    def list_models(self) -> list[str]:
        try:
            return sorted(model.id for model in list(self.sync_client.models.list()))
        except Exception:
            cached = load_codex_model_list()
            if cached:
                return cached
            raise


class CodexApiShim(OpenAIApiShim):
    """Codex shim using OpenAI API directly with credentials from ~/.codex/auth.json."""

    def list_models(self) -> list[str]:
        try:
            return sorted(model.id for model in list(self.sync_client.models.list()))
        except Exception:
            cached = load_codex_model_list()
            return cached or [CODEX_DEFAULT_MODEL]


class CodexChatGPTShim(CompletionClient):
    """Codex shim using the ChatGPT-backed Codex responses endpoint."""

    def list_models(self) -> list[str]:
        cached = load_codex_model_list()
        return cached or [CODEX_DEFAULT_MODEL]

    async def chat_completion(
        self,
        model: str,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        tool_choice: str = "auto",
    ) -> dict[str, Any]:
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
            return _responses_output_to_message(data)
        raise RuntimeError("Codex ChatGPT authentication failed after token refresh")


class BedrockConverseShim(CompletionClient):
    def __init__(self, region: str, credentials: dict[str, str]):
        self.region = region
        self.base_url = f"https://bedrock-runtime.{region}.amazonaws.com"
        aws_credentials = httpx_aws_auth.AwsCredentials(
            access_key=credentials["access_key"],
            secret_key=credentials["secret_key"],
            session_token=credentials.get("session_token"),
        )
        self._auth = httpx_aws_auth.AwsSigV4Auth(
            credentials=aws_credentials,
            service="bedrock",
            region=region,
        )
        self._list_auth = httpx_aws_auth.AwsSigV4Auth(
            credentials=aws_credentials,
            service="bedrock",
            region=region,
        )

    async def chat_completion(
        self,
        model: str,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        tool_choice: str = "auto",
    ) -> dict[str, Any]:
        system_prompts = []
        bedrock_messages = []
        for msg in messages:
            role = msg["role"]
            content = msg.get("content") or ""
            if role == "system":
                system_prompts.append({"text": str(content)})
                continue

            blocks = []
            if role == "tool":
                result_block = {
                    "toolResult": {
                        "toolUseId": msg["tool_call_id"],
                        "content": [{"text": str(content)}],
                    }
                }
                if bedrock_messages and bedrock_messages[-1]["role"] == "user":
                    bedrock_messages[-1]["content"].append(result_block)
                    continue
                role = "user"
                blocks.append(result_block)
            else:
                if isinstance(content, str) and content:
                    blocks.append({"text": content})
                elif isinstance(content, list):
                    for item in content:
                        if item.get("type") == "text" and item.get("text"):
                            blocks.append({"text": item["text"]})
                if role == "assistant" and "tool_calls" in msg:
                    for call in msg["tool_calls"]:
                        blocks.append(
                            {
                                "toolUse": {
                                    "toolUseId": call["id"],
                                    "name": call["function"]["name"],
                                    "input": json.loads(call["function"]["arguments"]),
                                }
                            }
                        )
                if not blocks:
                    continue
            bedrock_messages.append({"role": role, "content": blocks})

        payload: dict[str, Any] = {"messages": bedrock_messages}
        if system_prompts:
            payload["system"] = system_prompts
        if tools:
            payload["toolConfig"] = {
                "tools": [
                    {
                        "toolSpec": {
                            "name": tool["function"]["name"],
                            "description": tool["function"]["description"],
                            "inputSchema": {"json": tool["function"]["parameters"]},
                        }
                    }
                    for tool in tools
                ],
                "toolChoice": {"auto": {}} if tool_choice == "auto" else {"any": {}},
            }

        # Let httpx preserve the raw model ID so SigV4 signs the same path Bedrock sees.
        url = f"{self.base_url}/model/{model}/converse"
        body = json.dumps(payload)
        async with async_http_client(auth=self._auth) as client:
            response = await client.post(
                url,
                content=body,
                headers={"content-type": "application/json"},
                timeout=120,
            )
            if response.is_error:
                error_body = response.text
                try:
                    error_json = response.json()
                    message = error_json.get("message", error_body)
                except Exception:
                    message = error_body
                raise RuntimeError(f"Bedrock error ({response.status_code}): {message}")

            data = response.json()
            output = data["output"]["message"]
            content = ""
            tool_calls = []
            for block in output["content"]:
                if "text" in block:
                    content += block["text"]
                if "toolUse" in block:
                    tool = block["toolUse"]
                    tool_calls.append(
                        {
                            "id": tool["toolUseId"],
                            "type": "function",
                            "function": {
                                "name": tool["name"],
                                "arguments": json.dumps(tool["input"]),
                            },
                        }
                    )

        result: dict[str, Any] = {"role": output["role"], "content": content}
        if tool_calls:
            result["tool_calls"] = tool_calls
        return result

    def list_models(self) -> list[str]:
        all_ids: list[str] = []
        with http_client(auth=self._list_auth, timeout=30) as client:
            all_ids.extend(self._list_foundation_models(client))
            all_ids.extend(self._list_inference_profiles(client))
        return sorted(unique_strings(all_ids))

    def _list_foundation_models(self, client: httpx.Client) -> list[str]:
        response = client.get(
            f"https://bedrock.{self.region}.amazonaws.com/foundation-models"
        )
        if response.is_error:
            return []
        try:
            data = response.json()
        except ValueError:
            return []
        return [
            model["modelId"]
            for model in data.get("modelSummaries", [])
            if isinstance(model, dict)
            and isinstance(model.get("modelId"), str)
            and "TEXT" in model.get("outputModalities", [])
            and model["modelId"].startswith(("global.", "us."))
        ]

    def _list_inference_profiles(self, client: httpx.Client) -> list[str]:
        response = client.get(
            f"https://bedrock.{self.region}.amazonaws.com/inference-profiles"
        )
        if response.is_error:
            return []
        try:
            data = response.json()
        except ValueError:
            return []
        return [
            profile["inferenceProfileId"]
            for profile in data.get("inferenceProfileSummaries", [])
            if isinstance(profile, dict)
            and isinstance(profile.get("inferenceProfileId"), str)
            and profile["inferenceProfileId"].startswith(("global.", "us."))
        ]


class BedrockMantleShim(OpenAIApiShim):
    """Bedrock Mantle shim: OpenAI-compatible endpoint backed by AWS SigV4 bearer token."""

    def __init__(self, region: str, cwd: Path | None = None):
        token = make_bedrock_token(region, cwd)
        base_url = bedrock_base_url(region)
        super().__init__(
            AsyncOpenAI(api_key=token, base_url=base_url, max_retries=3),
            OpenAI(api_key=token, base_url=base_url, max_retries=3),
        )


class GeminiCodeAssistShim(CompletionClient):
    def __init__(self, project_id: str, access_token: str):
        self.project_id = project_id
        self.access_token = access_token
        self.base_url = f"{GEMINI_CODE_ASSIST_ENDPOINT}/{GEMINI_CODE_ASSIST_VERSION}"

    def _auth_headers(self) -> dict[str, str]:
        return _bearer_headers(self.access_token, **{"Content-Type": "application/json"})

    def _openai_messages_to_vertex(
        self, messages: list[dict[str, Any]]
    ) -> tuple[list[dict[str, Any]], dict[str, Any] | None]:
        system_parts: list[dict[str, Any]] = []
        contents: list[dict[str, Any]] = []
        for msg in messages:
            role = msg["role"]
            content = msg.get("content") or ""
            if role == "system":
                system_parts.append({"text": str(content)})
                continue
            if role == "tool":
                result_part = {
                    "functionResponse": {
                        "name": msg.get("name", "tool"),
                        "response": {"output": str(content)},
                    }
                }
                if contents and contents[-1]["role"] == "user":
                    contents[-1]["parts"].append(result_part)
                else:
                    contents.append({"role": "user", "parts": [result_part]})
                continue

            vertex_role = "model" if role == "assistant" else "user"
            parts: list[dict[str, Any]] = []
            parts.extend({"text": text} for text in _text_parts(content))

            if role == "assistant" and "tool_calls" in msg:
                signatures = msg.get("_thought_signatures", {})
                for call in msg["tool_calls"]:
                    part: dict[str, Any] = {
                        "functionCall": {
                            "name": call["function"]["name"],
                            "args": json.loads(call["function"]["arguments"]),
                        }
                    }
                    call_id = call.get("id", "")
                    if call_id in signatures:
                        part["thoughtSignature"] = signatures[call_id]
                    parts.append(part)

            if parts:
                contents.append({"role": vertex_role, "parts": parts})
        return contents, {"parts": system_parts} if system_parts else None

    def _openai_tools_to_vertex(
        self, tools: list[dict[str, Any]]
    ) -> list[dict[str, Any]] | None:
        if not tools:
            return None
        decls = [
            {
                "name": function["name"],
                "description": function.get("description", ""),
                "parameters": function.get("parameters", {}),
            }
            for function in _iter_function_tools(tools)
        ]
        return [{"functionDeclarations": decls}] if decls else None

    def _vertex_response_to_openai(self, response: dict[str, Any]) -> dict[str, Any]:
        inner = response.get("response", {})
        candidates = inner.get("candidates", [])
        if not candidates:
            return {"role": "assistant", "content": ""}

        content_text = ""
        tool_calls: list[dict[str, Any]] = []
        thought_signatures: dict[str, str] = {}
        for index, part in enumerate(candidates[0].get("content", {}).get("parts", [])):
            if "text" in part:
                content_text += part["text"]
            elif "functionCall" in part:
                function_call = part["functionCall"]
                call_id = f"call_{index}"
                tool_calls.append(
                    _openai_tool_call(
                        call_id, function_call["name"], function_call.get("args", {})
                    )
                )
                if "thoughtSignature" in part:
                    thought_signatures[call_id] = part["thoughtSignature"]

        result: dict[str, Any] = {"role": "assistant", "content": content_text}
        if tool_calls:
            result["tool_calls"] = tool_calls
        if thought_signatures:
            result["_thought_signatures"] = thought_signatures
        return result

    async def chat_completion(
        self,
        model: str,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        tool_choice: str = "auto",
    ) -> dict[str, Any]:
        contents, system_instruction = self._openai_messages_to_vertex(messages)
        vertex_tools = self._openai_tools_to_vertex(tools or [])

        request_body: dict[str, Any] = {"contents": contents}
        if system_instruction:
            request_body["systemInstruction"] = system_instruction
        if vertex_tools:
            request_body["tools"] = vertex_tools
            request_body["toolConfig"] = {
                "functionCallingConfig": {
                    "mode": "AUTO" if tool_choice == "auto" else "ANY"
                }
            }

        payload = {
            "model": model,
            "project": self.project_id,
            "request": request_body,
        }
        url = f"{self.base_url}:generateContent"
        async with async_http_client(timeout=120) as client:
            async def send_request() -> httpx.Response:
                resp = await client.post(url, json=payload, headers=self._auth_headers())
                if resp.status_code == 401:
                    self.access_token = refresh_gemini_token(
                        load_gemini_oauth_creds()["refresh_token"]
                    )
                    resp = await client.post(url, json=payload, headers=self._auth_headers())
                return resp

            resp = await _send_with_retry(
                send_request,
                should_retry_response=_should_retry_google_response,
            )
            if resp.is_error:
                raise RuntimeError(
                    f"Gemini Code Assist error ({resp.status_code}): {_response_error_message(resp)}"
                )
            return self._vertex_response_to_openai(resp.json())

    def list_models(self) -> list[str]:
        return load_gemini_model_list()


class ClaudeApiShim(CompletionClient):
    """Claude shim using the Anthropic API directly with OAuth credentials."""

    def __init__(self, access_token: str):
        self.access_token = access_token

    def _auth_headers(self) -> dict[str, str]:
        return _bearer_headers(
            self.access_token,
            **{
                "anthropic-version": ANTHROPIC_VERSION,
                "anthropic-beta": "oauth-2025-04-20",
                "content-type": "application/json",
            },
        )

    def _openai_messages_to_anthropic(
        self, messages: list[dict[str, Any]]
    ) -> tuple[str, list[dict[str, Any]]]:
        system_parts: list[str] = []
        anthropic_messages: list[dict[str, Any]] = []
        for msg in messages:
            role = msg["role"]
            content = msg.get("content") or ""
            if role == "system":
                system_parts.append(str(content))
                continue
            if role == "tool":
                result_block: dict[str, Any] = {
                    "type": "tool_result",
                    "tool_use_id": msg["tool_call_id"],
                    "content": str(content),
                }
                if anthropic_messages and anthropic_messages[-1]["role"] == "user":
                    existing = anthropic_messages[-1]["content"]
                    if isinstance(existing, list):
                        existing.append(result_block)
                    else:
                        anthropic_messages[-1]["content"] = [
                            {"type": "text", "text": str(existing)},
                            result_block,
                        ]
                    continue
                anthropic_messages.append({"role": "user", "content": [result_block]})
                continue
            if role == "assistant":
                blocks: list[dict[str, Any]] = []
                blocks.extend({"type": "text", "text": text} for text in _text_parts(content))
                for call in msg.get("tool_calls", []):
                    blocks.append(
                        {
                            "type": "tool_use",
                            "id": call["id"],
                            "name": call["function"]["name"],
                            "input": json.loads(call["function"]["arguments"]),
                        }
                    )
                if blocks:
                    anthropic_messages.append({"role": "assistant", "content": blocks})
                continue
            # user role
            if isinstance(content, list):
                blocks = [
                    {"type": "text", "text": item.get("text", "")}
                    for item in content
                    if item.get("type") == "text"
                ]
                anthropic_messages.append({"role": "user", "content": blocks or str(content)})
            else:
                anthropic_messages.append({"role": "user", "content": str(content)})
        return "\n\n".join(system_parts), anthropic_messages

    def _openai_tools_to_anthropic(
        self, tools: list[dict[str, Any]]
    ) -> list[dict[str, Any]]:
        return [
            {
                "name": function["name"],
                "description": function.get("description", ""),
                "input_schema": function.get(
                    "parameters", {"type": "object", "properties": {}}
                ),
            }
            for function in _iter_function_tools(tools)
        ]

    def _anthropic_response_to_openai(self, data: dict[str, Any]) -> dict[str, Any]:
        content_text = ""
        tool_calls: list[dict[str, Any]] = []
        for block in data.get("content", []):
            if block.get("type") == "text":
                content_text += block["text"]
            elif block.get("type") == "tool_use":
                tool_calls.append(
                    _openai_tool_call(
                        block["id"], block["name"], block.get("input", {})
                    )
                )
        result: dict[str, Any] = {"role": "assistant", "content": content_text}
        if tool_calls:
            result["tool_calls"] = tool_calls
        return result

    async def chat_completion(
        self,
        model: str,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]] | None = None,
        tool_choice: str = "auto",
    ) -> dict[str, Any]:
        system, anthropic_messages = self._openai_messages_to_anthropic(messages)
        body: dict[str, Any] = {
            "model": model,
            "max_tokens": 8096,
            "messages": anthropic_messages,
        }
        if system:
            body["system"] = system
        if tools:
            body["tools"] = self._openai_tools_to_anthropic(tools)
            body["tool_choice"] = {"type": "auto"} if tool_choice == "auto" else {"type": "any"}

        url = f"{CLAUDE_API_URL}/v1/messages"
        async with async_http_client(timeout=120) as client:
            async def send_request() -> httpx.Response:
                resp = await client.post(url, json=body, headers=self._auth_headers())
                if resp.status_code == 401:
                    self.access_token = get_claude_access_token()
                    resp = await client.post(url, json=body, headers=self._auth_headers())
                return resp

            resp = await _send_with_retry(send_request)
            if resp.is_error:
                message = _response_error_message(resp)
                detail = (
                    _anthropic_rate_limit_detail(resp)
                    if resp.status_code == 429
                    else ""
                )
                raise RuntimeError(
                    f"Anthropic API error ({resp.status_code}): {message}{detail}"
                )
            return self._anthropic_response_to_openai(resp.json())

    def list_models(self) -> list[str]:
        return _fetch_claude_models(self.access_token)


def _bearer_headers(token: str, **headers: str) -> dict[str, str]:
    return {"Authorization": f"Bearer {token}", **headers}



def _fetch_claude_models(access_token: str) -> list[str]:
    """Query the Anthropic API for available models."""
    headers = _bearer_headers(
        access_token,
        **{
            "anthropic-version": ANTHROPIC_VERSION,
            "anthropic-beta": "oauth-2025-04-20",
        },
    )
    models: list[str] = []
    params: dict[str, Any] = {"limit": 1000}
    with http_client(timeout=15) as client:
        while True:
            resp = client.get(f"{CLAUDE_API_URL}/v1/models", headers=headers, params=params)
            resp.raise_for_status()
            data = resp.json()
            models.extend(m["id"] for m in data.get("data", []))
            if not data.get("has_more"):
                break
            params["after_id"] = data["last_id"]
    return models


def _has_gemini_credentials() -> bool:
    try:
        load_gemini_oauth_creds()
    except RuntimeError:
        return False
    return True



def _has_bedrock_credentials() -> bool:
    try:
        load_aws_credentials(allow_login=False)
    except Exception:
        return False
    return True



def detect_available_shims() -> list[str]:
    available = []
    if has_openai_credentials():
        available.append(SHIM_OPENAI)
    if has_codex_credentials():
        available.append(SHIM_CODEX)
    if _has_gemini_credentials():
        available.append(SHIM_GEMINI)
    if has_claude_credentials():
        available.append(SHIM_CLAUDE)
    if _has_bedrock_credentials():
        available.extend([SHIM_BEDROCK, SHIM_MANTLE])
    return available


def _first_available_shim() -> str:
    if has_openai_credentials():
        return SHIM_OPENAI
    if has_codex_credentials():
        return SHIM_CODEX
    if _has_gemini_credentials():
        return SHIM_GEMINI
    if has_claude_credentials():
        return SHIM_CLAUDE
    return SHIM_BEDROCK


def resolve_shim(
    model_spec: str | None = None, configured_shim: str | None = None
) -> str:
    if env_shim := os.environ.get("OY_SHIM"):
        return env_shim
    if model_spec:
        prefix, _ = split_model_spec(model_spec)
        if prefix:
            return prefix
    return configured_shim or _first_available_shim()


def _build_openai_shim(*, max_retries: int) -> OpenAIApiShim:
    api_key = get_openai_api_key()
    if not api_key:
        raise RuntimeError("No OpenAI credentials found")
    base_url = os.environ.get("OPENAI_BASE_URL")
    return OpenAIApiShim(
        AsyncOpenAI(api_key=api_key, base_url=base_url, max_retries=max_retries),
        OpenAI(api_key=api_key, base_url=base_url, max_retries=max_retries),
    )


def _build_codex_shim() -> CompletionClient:
    if get_codex_api_key():
        async_client, sync_client = get_codex_openai_clients()
        return CodexApiShim(async_client, sync_client)
    return CodexChatGPTShim()


def get_client(
    shim: str,
    model_spec: str | None = None,
    region: str | None = None,
    cwd: Path | None = None,
) -> CompletionClient:
    _ = model_spec
    if shim == SHIM_GEMINI:
        return GeminiCodeAssistShim(resolve_gemini_project(), get_gemini_access_token())
    if shim == SHIM_BEDROCK:
        return BedrockConverseShim(current_region(region), load_aws_credentials(cwd))
    if shim == SHIM_MANTLE:
        return BedrockMantleShim(current_region(region), cwd)
    if shim == SHIM_CLAUDE:
        return ClaudeApiShim(get_claude_access_token())
    if shim == SHIM_CODEX:
        return _build_codex_shim()
    return _build_openai_shim(max_retries=3)


def list_models_for_shim(
    shim: str, region: str | None = None, cwd: Path | None = None
) -> list[str]:
    try:
        if shim == SHIM_OPENAI:
            raw = _build_openai_shim(max_retries=0).list_models() if get_openai_api_key() else []
        elif shim == SHIM_CODEX:
            raw = _build_codex_shim().list_models()
        elif shim == SHIM_GEMINI:
            load_gemini_oauth_creds()
            raw = load_gemini_model_list()
        elif shim == SHIM_CLAUDE:
            load_claude_auth_status()
            raw = _fetch_claude_models(get_claude_access_token())
        elif shim == SHIM_BEDROCK:
            raw = BedrockConverseShim(
                current_region(region), load_aws_credentials(cwd, allow_login=False)
            ).list_models()
        elif shim == SHIM_MANTLE:
            raw = BedrockMantleShim(current_region(region), cwd).list_models()
        else:
            return []
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
