//! Minimal reqwest-backed webfetch tool.

use std::net::IpAddr;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;
use reqwest::header::{CONTENT_TYPE, COOKIE, USER_AGENT};
use serde::Serialize;
use serde_json::Value;
use url::Url;

use super::args::{ReturnFormat, WebfetchArgs};
use super::{NetworkAccess, ToolContext};

#[derive(Debug, Serialize)]
pub(super) struct WebfetchOutput {
    pub url: String,
    pub status_code: u16,
    pub content: String,
    pub links: Vec<String>,
}

pub(super) async fn tool_webfetch(ctx: &ToolContext, args: WebfetchArgs) -> Result<Value> {
    if ctx.policy().network != NetworkAccess::Enabled {
        bail!("tool denied by policy: webfetch");
    }

    let url = validate_public_url(&args.url).await?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build webfetch HTTP client")?;
    let mut request = client.get(url.clone());
    if let Some(user_agent) = args
        .user_agent
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        request = request.header(USER_AGENT, user_agent.trim());
    }
    if let Some(cookie) = args
        .cookie
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        request = request.header(COOKIE, cookie.trim());
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("failed to fetch {}", url.as_str()))?;
    let status_code = response.status().as_u16();
    let response_url = response.url().clone();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let raw = response.text().await.with_context(|| {
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
    })?)
}

async fn validate_public_url(input: &str) -> Result<Url> {
    let url = Url::parse(&normalize_scrape_url(input)).context("Invalid URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("webfetch only supports http(s) URLs");
    }
    let host = url.host_str().context("URL must include a host")?;
    validate_public_host(host)?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        validate_public_ip(ip)?;
        return Ok(url);
    }

    let port = url
        .port_or_known_default()
        .context("URL must include a valid port")?;
    let mut resolved_any = false;
    for addr in tokio::net::lookup_host((host, port))
        .await
        .with_context(|| format!("failed to resolve {host}"))?
    {
        resolved_any = true;
        validate_public_ip(addr.ip())?;
    }
    if !resolved_any {
        bail!("failed to resolve {host}");
    }
    Ok(url)
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
    ip_rfc::global(&ip)
        && !ip.is_multicast()
        && !matches!(ip, IpAddr::V6(ip) if (ip.segments()[0] & 0xffc0) == 0xfec0)
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
}
