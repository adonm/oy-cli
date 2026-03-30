from __future__ import annotations

import hashlib
import hmac
from datetime import datetime, timezone
from urllib.parse import quote, urlsplit


type AwsCredentials = dict[str, str | None]


def _utc_now() -> datetime:
    return datetime.now(timezone.utc)


def _uri_encode(value: str) -> str:
    return quote(value, safe="-_.~")


def _normalize_path(path: str) -> str:
    return quote(path or "/", safe="/~")


def _canonical_query_string(query: str) -> str:
    if not query:
        return ""
    pairs = [part.partition("=") for part in query.split("&")]
    return "&".join(
        f"{key}={value}"
        for key, _, value in sorted(pairs, key=lambda pair: (pair[0], pair[2]))
    )


def _credential_scope(datestamp: str, region: str, service: str) -> str:
    return f"{datestamp}/{region}/{service}/aws4_request"


def _sign(key: bytes, message: str) -> bytes:
    return hmac.new(key, message.encode("utf-8"), hashlib.sha256).digest()


def _signature(
    secret_key: str,
    datestamp: str,
    region: str,
    service: str,
    string_to_sign: str,
) -> str:
    k_date = _sign(f"AWS4{secret_key}".encode("utf-8"), datestamp)
    k_region = _sign(k_date, region)
    k_service = _sign(k_region, service)
    k_signing = _sign(k_service, "aws4_request")
    return hmac.new(
        k_signing, string_to_sign.encode("utf-8"), hashlib.sha256
    ).hexdigest()


def sigv4_headers(
    credentials: AwsCredentials,
    region: str,
    service: str,
    method: str,
    url: str,
    *,
    body: bytes = b"",
    headers: dict[str, str] | None = None,
    now: datetime | None = None,
) -> dict[str, str]:
    now = (now or _utc_now()).astimezone(timezone.utc)
    timestamp = now.strftime("%Y%m%dT%H%M%SZ")
    datestamp = timestamp[:8]
    parsed = urlsplit(url)
    canonical_headers = {
        "host": parsed.netloc,
        "x-amz-date": timestamp,
    }
    for key, value in (headers or {}).items():
        canonical_headers[str(key).lower()] = str(value).strip()
    if credentials["session_token"]:
        canonical_headers["x-amz-security-token"] = str(credentials["session_token"])
    signed_headers = ";".join(sorted(canonical_headers))
    canonical_request = "\n".join(
        [
            method.upper(),
            _normalize_path(parsed.path),
            _canonical_query_string(parsed.query),
            "".join(
                f"{key}:{canonical_headers[key]}\n" for key in sorted(canonical_headers)
            ),
            signed_headers,
            hashlib.sha256(body).hexdigest(),
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
    auth = (
        "AWS4-HMAC-SHA256 "
        f"Credential={credentials['access_key']}/{_credential_scope(datestamp, region, service)}, "
        f"SignedHeaders={signed_headers}, "
        f"Signature={_signature(str(credentials['secret_key']), datestamp, region, service, string_to_sign)}"
    )
    return {
        **(headers or {}),
        "Host": parsed.netloc,
        "X-Amz-Date": timestamp,
        **(
            {"X-Amz-Security-Token": str(credentials["session_token"])}
            if credentials["session_token"]
            else {}
        ),
        "Authorization": auth,
    }
