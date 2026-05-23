//! Minimal reqwest-backed webfetch tool.

use std::net::{IpAddr, SocketAddr};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt as _;
use regex::Regex;
use reqwest::header::{CONTENT_TYPE, COOKIE, USER_AGENT};
use serde::Serialize;
use serde_json::Value;
use url::Url;

use super::args::{ReturnFormat, WebfetchArgs};
use super::{NetworkAccess, ToolContext};

const WEBFETCH_BODY_LIMIT_BYTES: usize = 5 * 1024 * 1024;
const WEBFETCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WEBFETCH_READ_TIMEOUT: Duration = Duration::from_secs(10);
const WEBFETCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize)]
pub(super) struct WebfetchOutput {
    pub url: String,
    pub status_code: u16,
    pub content: String,
    pub links: Vec<String>,
    pub truncated: bool,
}

pub(super) async fn tool_webfetch(ctx: &ToolContext, args: WebfetchArgs) -> Result<Value> {
    if ctx.policy().network != NetworkAccess::Enabled {
        bail!("tool denied by policy: webfetch");
    }

    let client = PublicWebfetchClient::new(&args.url).await?;
    let response = client
        .fetch(args.user_agent.as_deref(), args.cookie.as_deref())
        .await?;
    let status_code = response.status().as_u16();
    let response_url = response.url().clone();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let (raw, truncated) = read_capped_response_body(response, WEBFETCH_BODY_LIMIT_BYTES)
        .await
        .with_context(|| {
            format!(
                "failed to read response body from {}",
                response_url.as_str()
            )
        })?;
    let links = extract_links(&raw, &content_type, &response_url);
    let content = transform_scraped_content(&raw, &content_type, args.return_format);

    Ok(serde_json::to_value(WebfetchOutput {
        url: response_url.to_string(),
        status_code,
        content,
        links,
        truncated,
    })?)
}

struct PublicWebfetchClient {
    url: Url,
    client: reqwest::Client,
}

impl PublicWebfetchClient {
    async fn new(input: &str) -> Result<Self> {
        let target = PublicWebfetchTarget::resolve(input).await?;
        Self::from_target(target)
    }

    fn from_target(target: PublicWebfetchTarget) -> Result<Self> {
        let custom_policy = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("too many redirects");
            }

            let (scheme, host, port) = {
                let url = attempt.url();
                let scheme = url.scheme().to_string();
                let host = match url.host_str() {
                    Some(h) => h.to_string(),
                    None => return attempt.error("redirect URL must include a host"),
                };
                let port = url.port_or_known_default();
                (scheme, host, port)
            };

            if !matches!(scheme.as_str(), "http" | "https") {
                return attempt.error("webfetch only supports http(s) redirects");
            }

            if let Err(err) = validate_public_host(&host) {
                return attempt.error(err.to_string());
            }

            if let Ok(ip) = host.parse::<IpAddr>() {
                if let Err(err) = validate_public_ip(ip) {
                    return attempt.error(err.to_string());
                }
            } else {
                let port = match port {
                    Some(p) => p,
                    None => return attempt.error("redirect URL must include a valid port"),
                };

                use std::net::ToSocketAddrs;
                match (&host as &str, port).to_socket_addrs() {
                    Ok(addrs) => {
                        let mut resolved_any = false;
                        for addr in addrs {
                            resolved_any = true;
                            if let Err(err) = validate_public_ip(addr.ip()) {
                                return attempt.error(err.to_string());
                            }
                        }
                        if !resolved_any {
                            return attempt.error(format!("failed to resolve {host}"));
                        }
                    }
                    Err(err) => {
                        return attempt.error(format!("failed to resolve {host}: {err}"));
                    }
                }
            }

            attempt.follow()
        });
        let mut builder = reqwest::Client::builder()
            .redirect(custom_policy)
            .connect_timeout(WEBFETCH_CONNECT_TIMEOUT)
            .read_timeout(WEBFETCH_READ_TIMEOUT)
            .timeout(WEBFETCH_REQUEST_TIMEOUT);
        if let Some((host, addrs)) = target.pinned_dns_override() {
            builder = builder.resolve_to_addrs(host, addrs);
        }
        let client = builder
            .build()
            .context("failed to build webfetch HTTP client")?;

        Ok(Self {
            url: target.url,
            client,
        })
    }

    async fn fetch(
        &self,
        user_agent: Option<&str>,
        cookie: Option<&str>,
    ) -> Result<reqwest::Response> {
        let mut request = self.client.get(self.url.clone());
        if let Some(user_agent) = user_agent.filter(|value| !value.trim().is_empty()) {
            request = request.header(USER_AGENT, user_agent.trim());
        }
        if let Some(cookie) = cookie.filter(|value| !value.trim().is_empty()) {
            request = request.header(COOKIE, cookie.trim());
        }

        request
            .send()
            .await
            .with_context(|| format!("failed to fetch {}", self.url.as_str()))
    }
}

struct PublicWebfetchTarget {
    url: Url,
    host: String,
    resolved_addrs: Vec<SocketAddr>,
}

impl PublicWebfetchTarget {
    async fn resolve(input: &str) -> Result<Self> {
        let url = Url::parse(&normalize_scrape_url(input)).context("Invalid URL")?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("webfetch only supports http(s) URLs");
        }
        let host = url
            .host_str()
            .context("URL must include a host")?
            .to_string();
        validate_public_host(&host)?;
        if let Ok(ip) = host.parse::<IpAddr>() {
            validate_public_ip(ip)?;
            return Ok(Self {
                url,
                host,
                resolved_addrs: Vec::new(),
            });
        }

        let port = url
            .port_or_known_default()
            .context("URL must include a valid port")?;
        let resolved_addrs = resolve_public_addrs(&host, port).await?;
        Ok(Self {
            url,
            host,
            resolved_addrs,
        })
    }

    fn pinned_dns_override(&self) -> Option<(&str, &[SocketAddr])> {
        if self.resolved_addrs.is_empty() {
            None
        } else {
            Some((&self.host, &self.resolved_addrs))
        }
    }
}

async fn read_capped_response_body(
    response: reqwest::Response,
    limit: usize,
) -> Result<(String, bool)> {
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or(limit as u64)
            .min(limit as u64) as usize,
    );
    let mut truncated = response
        .content_length()
        .is_some_and(|length| length > limit as u64);
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let remaining = limit.saturating_sub(body.len());
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }

    Ok((String::from_utf8_lossy(&body).to_string(), truncated))
}

async fn resolve_public_addrs(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let mut resolved_addrs = Vec::new();
    let mut resolved_any = false;
    for addr in tokio::net::lookup_host((host, port))
        .await
        .with_context(|| format!("failed to resolve {host}"))?
    {
        resolved_any = true;
        validate_public_ip(addr.ip())?;
        resolved_addrs.push(addr);
    }
    if !resolved_any {
        bail!("failed to resolve {host}");
    }
    Ok(resolved_addrs)
}

fn validate_public_host(host: &str) -> Result<()> {
    let host = host.trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        bail!("webfetch blocks localhost targets");
    }
    Ok(())
}

fn validate_public_ip(ip: IpAddr) -> Result<()> {
    if is_public_ip(ip) {
        Ok(())
    } else {
        bail!("webfetch blocks localhost and private IP targets");
    }
}

fn is_public_ip(ip: IpAddr) -> bool {
    let normalized_ip = match ip {
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4() {
                IpAddr::V4(v4)
            } else {
                IpAddr::V6(v6)
            }
        }
        IpAddr::V4(v4) => IpAddr::V4(v4),
    };
    ip_rfc::global(&normalized_ip)
        && !normalized_ip.is_multicast()
        && !matches!(normalized_ip, IpAddr::V6(ip) if (ip.segments()[0] & 0xffc0) == 0xfec0)
}

fn normalize_scrape_url(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.starts_with("http") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

fn is_html_content(content_type: &str, content: &str) -> bool {
    content_type.to_ascii_lowercase().contains("html")
        || content.trim_start().starts_with("<!DOCTYPE html")
        || content.trim_start().starts_with("<html")
}

static HREF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<a\b[^>]*\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'<>`]+))"#)
        .expect("valid href regex")
});

fn extract_links(content: &str, content_type: &str, base_url: &Url) -> Vec<String> {
    if !is_html_content(content_type, content) {
        return Vec::new();
    }
    HREF_RE
        .captures_iter(content)
        .filter_map(|captures| {
            captures
                .get(1)
                .or_else(|| captures.get(2))
                .or_else(|| captures.get(3))
                .map(|value| value.as_str().trim())
        })
        .filter(|href| !href.is_empty())
        .filter_map(|href| base_url.join(href).ok())
        .map(|url| url.to_string())
        .collect()
}

fn transform_scraped_content(
    content: &str,
    content_type: &str,
    return_format: ReturnFormat,
) -> String {
    match return_format {
        ReturnFormat::Raw => content.to_string(),
        ReturnFormat::Markdown => {
            if is_html_content(content_type, content) {
                html2md::parse_html(content)
            } else {
                content.to_string()
            }
        }
        ReturnFormat::Text => html_to_text(content, content_type),
        ReturnFormat::Xml => format!(
            "<page><content><![CDATA[{}]]></content></page>",
            content.replace("]]>", "]]]]><![CDATA[>")
        ),
    }
}

fn html_to_text(content: &str, content_type: &str) -> String {
    if is_html_content(content_type, content) {
        html2md::parse_html(content)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        content.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ip_filter_blocks_local_and_private_ranges() {
        for ip in [
            "0.0.0.0".parse().unwrap(),
            "10.0.0.1".parse().unwrap(),
            "100.64.0.1".parse().unwrap(),
            "127.0.0.1".parse().unwrap(),
            "169.254.1.1".parse().unwrap(),
            "172.16.0.1".parse().unwrap(),
            "192.0.0.1".parse().unwrap(),
            "192.0.2.1".parse().unwrap(),
            "192.168.0.1".parse().unwrap(),
            "198.18.0.1".parse().unwrap(),
            "198.51.100.1".parse().unwrap(),
            "203.0.113.1".parse().unwrap(),
            "224.0.0.1".parse().unwrap(),
            "240.0.0.1".parse().unwrap(),
            "::".parse().unwrap(),
            "::1".parse().unwrap(),
            "2001:db8::1".parse().unwrap(),
            "fc00::1".parse().unwrap(),
            "fe80::1".parse().unwrap(),
            "fec0::1".parse().unwrap(),
            "ff0e::1".parse().unwrap(),
            "::ffff:127.0.0.1".parse().unwrap(),
            "::ffff:10.0.0.1".parse().unwrap(),
            "::ffff:169.254.169.254".parse().unwrap(),
            "::ffff:192.168.1.1".parse().unwrap(),
        ] {
            assert!(!is_public_ip(ip), "{ip} should be blocked");
        }

        for ip in [
            "93.184.216.34".parse().unwrap(),
            "192.0.0.9".parse().unwrap(),
            "192.0.0.10".parse().unwrap(),
            "2606:2800:220:1:248:1893:25c8:1946".parse().unwrap(),
        ] {
            assert!(is_public_ip(ip), "{ip} should be allowed");
        }
    }

    #[test]
    fn localhost_hostnames_are_blocked_before_resolution() {
        assert!(validate_public_host("localhost").is_err());
        assert!(validate_public_host("api.localhost").is_err());
        assert!(validate_public_host("example.com").is_ok());
    }

    #[test]
    fn extracts_absolute_and_relative_links_from_html() {
        let base = Url::parse("https://example.com/docs/page.html").unwrap();
        let links = extract_links(
            r#"<html><a href="/root">root</a><a href='next.html'>next</a><a href=https://other.test/>other</a></html>"#,
            "text/html; charset=utf-8",
            &base,
        );
        assert_eq!(
            links,
            vec![
                "https://example.com/root".to_string(),
                "https://example.com/docs/next.html".to_string(),
                "https://other.test/".to_string(),
            ]
        );
    }

    #[test]
    fn text_content_has_no_links() {
        let base = Url::parse("https://example.com/").unwrap();
        assert!(extract_links("<a href='/x'>x</a>", "text/plain", &base).is_empty());
    }

    #[tokio::test]
    async fn response_body_reader_caps_streamed_bytes() {
        let response = test_response("abcdef", None).await;
        let (body, truncated) = read_capped_response_body(response, 3).await.unwrap();

        assert_eq!(body, "abc");
        assert!(truncated);
    }

    #[tokio::test]
    async fn response_body_reader_marks_large_content_length_truncated() {
        let response = test_response("abcdef", Some(6)).await;
        let (body, truncated) = read_capped_response_body(response, 3).await.unwrap();

        assert_eq!(body, "abc");
        assert!(truncated);
    }

    #[tokio::test]
    async fn public_webfetch_client_uses_pinned_addrs_and_preserves_host() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let pinned_addr = listener.local_addr().unwrap();
        let url = Url::parse(&format!(
            "http://example.test:{}/docs?q=1",
            pinned_addr.port()
        ))
        .unwrap();
        let client = PublicWebfetchClient::from_target(PublicWebfetchTarget {
            url,
            host: "example.test".to_string(),
            resolved_addrs: vec![pinned_addr],
        })
        .unwrap();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
            String::from_utf8(request).unwrap()
        });

        let response = client.fetch(None, None).await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let request = server.await.unwrap();
        assert!(request.starts_with("GET /docs?q=1 HTTP/1.1\r\n"));
        assert!(request.lines().any(|line| {
            line.eq_ignore_ascii_case(&format!("host: example.test:{}", pinned_addr.port()))
        }));
    }

    async fn test_response(body: &str, content_length: Option<usize>) -> reqwest::Response {
        use tokio::io::AsyncWriteExt as _;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let body = body.to_string();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let content_length = content_length
                .map(|length| format!("Content-Length: {length}\r\n"))
                .unwrap_or_default();
            socket
                .write_all(
                    format!("HTTP/1.1 200 OK\r\n{content_length}Connection: close\r\n\r\n{body}")
                        .as_bytes(),
                )
                .await
                .unwrap();
        });

        reqwest::Client::new().get(url).send().await.unwrap()
    }

    #[tokio::test]
    async fn public_webfetch_client_allows_safe_redirects_but_blocks_private_targets() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let pinned_addr = listener.local_addr().unwrap();

        let url = Url::parse(&format!("http://example.test:{}/start", pinned_addr.port())).unwrap();

        let client = PublicWebfetchClient::from_target(PublicWebfetchTarget {
            url,
            host: "example.test".to_string(),
            resolved_addrs: vec![pinned_addr],
        })
        .unwrap();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            socket
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:12345/secret\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });

        let response = client.fetch(None, None).await;
        assert!(response.is_err());
        let err = response.err().unwrap();
        let err_debug = format!("{:?}", err);
        assert!(
            err_debug.contains("webfetch blocks localhost and private IP targets")
                || err_debug.contains("redirect"),
            "unexpected error message: {}",
            err_debug
        );

        server.await.unwrap();
    }
}
