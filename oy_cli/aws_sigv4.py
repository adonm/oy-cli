from __future__ import annotations

import base64
import hashlib
import hmac
from dataclasses import dataclass
from datetime import datetime, timezone
from urllib.parse import parse_qsl, quote, urlsplit, urlunsplit

_BEDROCK_BEARER_TOKEN_URL = (
    "https://bedrock.amazonaws.com/?Action=CallWithBearerToken&Version=1"
)
_EMPTY_SHA256_HASH = hashlib.sha256(b"").hexdigest()


@dataclass(frozen=True, slots=True)
class AwsCredentials:
    access_key: str
    secret_key: str
    session_token: str | None = None


def _utc_now() -> datetime:
    return datetime.now(timezone.utc)


def _uri_encode(value: str) -> str:
    return quote(value, safe="-_.~")


def _normalize_path(path: str) -> str:
    return quote(path or "/", safe="/~")


def _encode_query_pairs(pairs: list[tuple[str, str]]) -> str:
    return "&".join(
        f"{_uri_encode(key)}={_uri_encode(value)}" for key, value in pairs
    )


def _canonical_query_string(query: str) -> str:
    if not query:
        return ""
    pairs = [part.partition("=") for part in query.split("&")]
    return "&".join(
        f"{key}={value}" for key, _, value in sorted(pairs, key=lambda pair: (pair[0], pair[2]))
    )


def _credential_scope(datestamp: str, region: str, service: str) -> str:
    return f"{datestamp}/{region}/{service}/aws4_request"


def _sign(key: bytes, message: str) -> bytes:
    return hmac.new(key, message.encode("utf-8"), hashlib.sha256).digest()


def _signature(secret_key: str, datestamp: str, region: str, service: str, string_to_sign: str) -> str:
    k_date = _sign(f"AWS4{secret_key}".encode("utf-8"), datestamp)
    k_region = _sign(k_date, region)
    k_service = _sign(k_region, service)
    k_signing = _sign(k_service, "aws4_request")
    return hmac.new(
        k_signing, string_to_sign.encode("utf-8"), hashlib.sha256
    ).hexdigest()


def bedrock_bearer_token(
    credentials: AwsCredentials,
    region: str,
    *,
    expires: int = 43200,
    now: datetime | None = None,
) -> str:
    now = (now or _utc_now()).astimezone(timezone.utc)
    timestamp = now.strftime("%Y%m%dT%H%M%SZ")
    datestamp = timestamp[:8]
    service = "bedrock"
    url = urlsplit(_BEDROCK_BEARER_TOKEN_URL)
    operation_params = parse_qsl(url.query, keep_blank_values=True)
    auth_params = [
        ("X-Amz-Algorithm", "AWS4-HMAC-SHA256"),
        (
            "X-Amz-Credential",
            f"{credentials.access_key}/{_credential_scope(datestamp, region, service)}",
        ),
        ("X-Amz-Date", timestamp),
        ("X-Amz-Expires", str(expires)),
        ("X-Amz-SignedHeaders", "host"),
    ]
    if credentials.session_token:
        auth_params.append(("X-Amz-Security-Token", credentials.session_token))
    query = "&".join(
        part for part in (_encode_query_pairs(operation_params), _encode_query_pairs(auth_params)) if part
    )
    canonical_request = "\n".join(
        [
            "POST",
            _normalize_path(url.path),
            _canonical_query_string(query),
            f"host:{url.netloc}\n",
            "host",
            _EMPTY_SHA256_HASH,
        ]
    )
    string_to_sign = "\n".join(
        [
            "AWS4-HMAC-SHA256",
            timestamp,
            _credential_scope(datestamp, region, service),
            hashlib.sha256(canonical_request.encode("utf-8")).hexdigest(),
        ]
    )
    signature = _signature(
        credentials.secret_key, datestamp, region, service, string_to_sign
    )
    signed_url = urlunsplit(
        (
            url.scheme,
            url.netloc,
            url.path,
            f"{query}&X-Amz-Signature={signature}",
            url.fragment,
        )
    )
    raw = signed_url.removeprefix("https://")
    return f"bedrock-api-key-{base64.b64encode(raw.encode()).decode()}"
