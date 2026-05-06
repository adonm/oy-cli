use anyhow::{Context, Result, bail};
use futures_util::StreamExt as _;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::net::lookup_host;
use url::Url;

use super::args::{HeaderPolicy, RedirectPolicy, WebfetchArgs};
use super::{
    MAX_WEBFETCH_BYTES, MAX_WEBFETCH_TIMEOUT_SECONDS, NetworkAccess, ToolContext, WEBFETCH_ACCEPT,
    WEBFETCH_USER_AGENT,
};

#[derive(Debug, Serialize)]
pub(super) struct WebfetchOutput {
    pub method: String,
    pub url: String,
    pub status_code: u16,
    pub reason_phrase: &'static str,
    pub http_version: String,
    pub headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<&'static str>,
    #[serde(skip_serializing_if = "is_false")]
    pub binary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_bytes: Option<usize>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_capped: Option<bool>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub(super) async fn tool_webfetch(ctx: &ToolContext, args: WebfetchArgs) -> Result<Value> {
    if ctx.policy.network != NetworkAccess::Enabled {
        bail!("tool denied by policy: webfetch");
    }
    let method = args.method.to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS") {
        bail!("Only GET/HEAD/OPTIONS are allowed, got {method}");
    }
    let url = validate_public_url(&args.url).await?;
    let resolved = public_socket_addrs(&url).await?;
    let headers = validated_webfetch_headers(&args.headers)?;
    let client = webfetch_client(&url, &resolved, args.timeout_seconds)?;
    let mut request = client.request(method.parse()?, url.clone());
    for (key, value) in &headers {
        request = request.header(key, value);
    }
    let mut response = request.send().await?;
    if args.redirects == RedirectPolicy::Follow {
        response = follow_public_redirects(response, &method, &headers).await?;
    }
    let status = response.status();
    let version = response.version();
    let final_url = response.url().to_string();
    if final_url != url.as_str() {
        validate_public_url(&final_url).await?;
    }
    let headers = response.headers().clone();
    let content_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let header_map = headers
        .iter()
        .map(|(k, v)| {
            let key = k.as_str().to_string();
            let value = if matches!(
                key.to_ascii_lowercase().as_str(),
                "set-cookie" | "www-authenticate" | "proxy-authenticate" | "location"
            ) {
                "<redacted>".to_string()
            } else {
                v.to_str().unwrap_or("").to_string()
            };
            (key, value)
        })
        .collect::<BTreeMap<_, _>>();

    let (body, body_capped) = read_limited_response(response, MAX_WEBFETCH_BYTES).await?;
    if is_text_content_type(&content_type) {
        let text = String::from_utf8_lossy(&body).to_string();
        let normalized = if content_type.contains("text/html")
            || text.trim_start().starts_with("<!DOCTYPE html")
            || text.trim_start().starts_with("<html")
        {
            html2md::parse_html(&text)
        } else {
            text
        };
        let (text_preview, preview_truncated) = crate::ui::head_tail(&normalized, 12_000);
        let truncated = preview_truncated || body_capped;
        return Ok(serde_json::to_value(WebfetchOutput {
            method,
            url: final_url,
            status_code: status.as_u16(),
            reason_phrase: reason_phrase(status),
            http_version: format!("{:?}", version),
            headers: header_map,
            text: Some(normalized),
            text_preview: Some(text_preview),
            format: Some(if content_type.contains("html") {
                "markdown"
            } else {
                "text"
            }),
            binary: false,
            content_bytes: None,
            truncated,
            body_capped: Some(body_capped),
        })?);
    }

    Ok(serde_json::to_value(WebfetchOutput {
        method,
        url: final_url,
        status_code: status.as_u16(),
        reason_phrase: reason_phrase(status),
        http_version: format!("{:?}", version),
        headers: header_map,
        text: None,
        text_preview: None,
        format: None,
        binary: true,
        content_bytes: Some(body.len()),
        truncated: body_capped,
        body_capped: None,
    })?)
}
async fn read_limited_response(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool)> {
    let mut stream = response.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let remaining = max_bytes.saturating_sub(out.len());
        if chunk.len() > remaining {
            out.extend_from_slice(&chunk[..remaining]);
            return Ok((out, true));
        }
        out.extend_from_slice(&chunk);
        if out.len() >= max_bytes {
            return Ok((out, true));
        }
    }
    Ok((out, false))
}

// === Public network boundary ===
async fn follow_public_redirects(
    mut response: reqwest::Response,
    method: &str,
    headers: &BTreeMap<String, String>,
) -> Result<reqwest::Response> {
    for _ in 0..10 {
        if !response.status().is_redirection() {
            return Ok(response);
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .context("redirect missing valid Location header")?;
        let next_url = response.url().join(location)?;
        validate_public_url_parts(&next_url)?;
        let resolved = public_socket_addrs(&next_url).await?;
        let client = webfetch_client(&next_url, &resolved, MAX_WEBFETCH_TIMEOUT_SECONDS)?;
        let mut request = client.request(method.parse()?, next_url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        response = request.send().await?;
    }
    bail!("too many redirects")
}

pub(super) fn validated_webfetch_headers(
    headers: &HeaderPolicy,
) -> Result<BTreeMap<String, String>> {
    let mut validated = BTreeMap::from([
        (ACCEPT.as_str().to_string(), WEBFETCH_ACCEPT.to_string()),
        (
            USER_AGENT.as_str().to_string(),
            WEBFETCH_USER_AGENT.to_string(),
        ),
    ]);
    for (key, value) in &headers.values {
        let lower = key.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "authorization"
                | "cookie"
                | "host"
                | "proxy-authorization"
                | "x-forwarded-for"
                | "x-real-ip"
        ) {
            bail!("Header {key:?} is not allowed in webfetch requests");
        }
        if value.contains('\r') || value.contains('\n') {
            bail!("Header value for {key:?} contains invalid CRLF characters");
        }
        validated.retain(|existing, _| !existing.eq_ignore_ascii_case(key));
        validated.insert(key.clone(), value.clone());
    }
    Ok(validated)
}

fn webfetch_client(
    url: &Url,
    resolved: &[SocketAddr],
    timeout_seconds: u64,
) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(
            timeout_seconds.clamp(1, MAX_WEBFETCH_TIMEOUT_SECONDS),
        ))
        .resolve_to_addrs(url.host_str().context("missing hostname")?, resolved)
        .build()?)
}

async fn validate_public_url(input: &str) -> Result<Url> {
    let url = Url::parse(input).with_context(|| format!("invalid URL: {input}"))?;
    validate_public_url_parts(&url)?;
    let _ = public_socket_addrs(&url).await?;
    Ok(url)
}

fn validate_public_url_parts(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("Only http/https URLs are allowed, got {:?}", url.scheme());
    }
    let host = url.host_str().context("missing hostname")?;
    let lower = host.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "localhost" | "localhost.localdomain" | "ip6-localhost" | "ip6-loopback"
    ) {
        bail!("Local addresses are not allowed: {host}");
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        ensure_public_ip(ip)?;
    }
    Ok(())
}

async fn public_socket_addrs(url: &Url) -> Result<Vec<SocketAddr>> {
    validate_public_url_parts(url)?;
    let host = url.host_str().context("missing hostname")?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs = lookup_host((host, port)).await?.collect::<Vec<_>>();
    if addrs.is_empty() {
        bail!("URL host resolved to no addresses: {host}");
    }
    for addr in &addrs {
        ensure_public_ip(addr.ip())?;
    }
    Ok(addrs)
}

fn ensure_public_ip(ip: IpAddr) -> Result<()> {
    if is_public_ip(ip) {
        Ok(())
    } else {
        bail!("URL resolves to non-public address ({ip})")
    }
}

pub(super) fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(is_public_ipv4)
            .unwrap_or_else(|| is_public_ipv6(ip)),
    }
}

fn is_public_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || octets[0] == 0
        || octets[0] >= 240
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 198 && (18..=19).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)) // 192.0.0.0/24 – IETF Protocol Assignments
}

fn is_public_ipv6(ip: std::net::Ipv6Addr) -> bool {
    let segments = ip.segments();
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_multicast()
        || (segments[0] & 0xffc0) == 0xfec0)
}

fn is_text_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("javascript")
        || content_type.contains("svg")
        || content_type.is_empty()
}

fn reason_phrase(status: StatusCode) -> &'static str {
    status.canonical_reason().unwrap_or("")
}
