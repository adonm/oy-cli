from __future__ import annotations

import ipaddress
import socket
from typing import Any
from urllib.parse import urlparse

import httpx
import msgspec
from markdownify import markdownify

from .. import runtime as rt
from ..serialization import serialize_toon
from .core import WebfetchArgs, WebfetchOptions, tool
from .output import _summarize_text_output, _tool_content_payload

def _validate_url_safe(url: str) -> str:
    parsed = urlparse(url)
    if parsed.scheme not in ("http", "https"):
        raise ValueError(f"Only http/https URLs are allowed, got: {parsed.scheme!r}")
    hostname = parsed.hostname
    if not hostname:
        raise ValueError(f"No hostname in URL: {url!r}")
    local_hosts = {
        "localhost",
        "localhost.localdomain",
        "ip6-localhost",
        "ip6-loopback",
    }
    if hostname.lower() in local_hosts:
        raise ValueError(f"Local addresses are not allowed: {hostname!r}")
    try:
        addrinfos = socket.getaddrinfo(
            hostname, parsed.port or (443 if parsed.scheme == "https" else 80)
        )
    except socket.gaierror as exc:
        raise ValueError(f"Cannot resolve hostname {hostname!r}: {exc}") from exc
    for _family, _type, _proto, _canonname, sockaddr in addrinfos:
        ip = ipaddress.ip_address(sockaddr[0])
        if ip.is_private or ip.is_reserved or ip.is_loopback or ip.is_link_local:
            raise ValueError(
                f"URL resolves to non-public address ({ip}); "
                "private/reserved/loopback/link-local addresses are blocked"
            )
    return url

_WEBFETCH_ALLOWED_METHODS = {"GET", "HEAD", "OPTIONS"}
_WEBFETCH_BLOCKED_HEADERS = frozenset(
    {
        "authorization",
        "cookie",
        "host",
        "proxy-authorization",
        "x-forwarded-for",
        "x-real-ip",
    }
)
_WEBFETCH_REDACTED_RESPONSE_HEADERS = frozenset(
    {"set-cookie", "www-authenticate", "proxy-authenticate", "location"}
)
_WEBFETCH_HTML_CONTENT_TYPES = ("text/html", "application/xhtml+xml")

def _sanitize_webfetch_headers(headers: dict[str, str] | None) -> dict[str, str]:
    if not headers:
        return {}
    clean: dict[str, str] = {}
    for key, value in headers.items():
        key_str, val_str = str(key), str(value)
        if key_str.lower() in _WEBFETCH_BLOCKED_HEADERS:
            raise ValueError(f"Header {key_str!r} is not allowed in webfetch requests")
        if "\r" in val_str or "\n" in val_str:
            raise ValueError(
                f"Header value for {key_str!r} contains invalid CRLF characters"
            )
        clean[key_str] = val_str
    return clean

def _webfetch_is_html_response(response: httpx.Response, text: str) -> bool:
    content_type = (
        response.headers.get("content-type", "").split(";", 1)[0].strip().lower()
    )
    if content_type in _WEBFETCH_HTML_CONTENT_TYPES:
        return True
    return text.lstrip().lower().startswith(("<!doctype html", "<html"))

def _html_to_markdown(text: str) -> str:
    return markdownify(text)

def _webfetch_summarize_response_body(
    response: httpx.Response,
) -> tuple[str, bool, str]:
    text = response.text
    content_format = "text"
    if (
        _webfetch_is_html_response(response, text)
        and rt.count_tokens(text) > rt.BUDGETS.tool_output_tokens
    ):
        markdown = _html_to_markdown(text)
        if markdown:
            text = markdown
            content_format = "markdown"
    summarized, truncated = _summarize_text_output(text)
    return summarized, truncated, content_format

def _webfetch_response_headers(response: httpx.Response) -> dict[str, str]:
    def display_name(name: str) -> str:
        return "-".join(part.capitalize() for part in name.split("-"))

    return {
        display_name(key): (
            "<redacted>"
            if key.lower() in _WEBFETCH_REDACTED_RESPONSE_HEADERS
            else response.headers[key]
        )
        for key in response.headers.keys()
    }

def _webfetch_structured_text(payload: dict[str, Any]) -> str:
    return serialize_toon(payload)

def _webfetch_response_text(response: httpx.Response, text: str | None = None) -> str:
    version = response.http_version or "HTTP/1.1"
    header_lines = [
        f"{key}: {value}" for key, value in _webfetch_response_headers(response).items()
    ]
    parts = [f"{version} {response.status_code} {response.reason_phrase}".rstrip()]
    if header_lines:
        parts.append("\n".join(header_lines))
    if text is None:
        text = response.text
    if text:
        parts.append(text)
    return "\n\n".join(parts)

def _webfetch_payload(
    response: httpx.Response,
    *,
    method: str,
    text: str,
    truncated: bool,
    content_format: str = "text",
) -> dict[str, Any]:
    return _tool_content_payload(
        method=method,
        url=str(response.url),
        ok=response.is_success,
        status_code=response.status_code,
        reason_phrase=response.reason_phrase,
        http_version=response.http_version or "HTTP/1.1",
        headers=_webfetch_response_headers(response),
        content=text,
        content_format=content_format,
        truncated=truncated,
    )

def _webfetch_error_payload(
    url: str, *, method: str, exc: httpx.HTTPError
) -> dict[str, Any]:
    return {
        "method": method,
        "url": url,
        "ok": False,
        "error_type": type(exc).__name__,
        "message": str(exc),
    }

@tool(WebfetchArgs)
def tool_webfetch(
    state: Any,
    url: str,
    method: str = "GET",
    headers: dict[str, str] | None = None,
    options: dict[str, Any] | WebfetchOptions | None = None,
):
    method = method.upper()
    if method not in _WEBFETCH_ALLOWED_METHODS:
        raise ValueError(
            f"Only {', '.join(sorted(_WEBFETCH_ALLOWED_METHODS))} methods are allowed, got: {method!r}"
        )
    _validate_url_safe(url)
    headers = _sanitize_webfetch_headers(headers)
    options = msgspec.convert(options or {}, WebfetchOptions)
    rt.note_tool(
        state,
        "webfetch",
        _defaults={
            "method": "GET",
            "headers": {},
            "follow_redirects": False,
            "timeout_seconds": 30,
        },
        url=url,
        method=method,
        headers=headers,
        follow_redirects=options.follow_redirects,
        timeout_seconds=options.timeout_seconds,
    )
    try:
        with rt.http_client(
            timeout=options.timeout_seconds,
            follow_redirects=options.follow_redirects,
        ) as client:
            response = client.request(method, url, headers=headers)
    except httpx.HTTPError as exc:
        payload = _webfetch_error_payload(url, method=method, exc=exc)
        rt.show(f"[error] {type(exc).__name__}: {exc}")
        return payload
    text, truncated, content_format = _webfetch_summarize_response_body(response)
    payload = _webfetch_payload(
        response,
        method=method,
        text=text,
        truncated=truncated,
        content_format=content_format,
    )
    rt.show(_webfetch_structured_text(payload))
    return payload

__all__ = [
    "_WEBFETCH_ALLOWED_METHODS",
    "_html_to_markdown",
    "_validate_url_safe",
    "_webfetch_payload",
    "_webfetch_response_headers",
    "_webfetch_response_text",
    "_webfetch_structured_text",
    "tool_webfetch",
]
