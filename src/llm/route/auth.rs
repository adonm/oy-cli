use anyhow::{Context, Result, bail};
use chrono::{Datelike, Timelike, Utc};
use hmac::{Hmac, Mac};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use sha2::{Digest, Sha256};
use std::{env, net::IpAddr};

use crate::llm::{AwsCredentials, RouteAuth};

const ALLOW_INSECURE_LOCAL_PROVIDER_HTTP_ENV: &str = "OY_ALLOW_INSECURE_LOCAL_PROVIDER_HTTP";

pub(crate) fn apply_json_headers(
    builder: reqwest::RequestBuilder,
    auth: &RouteAuth,
    endpoint: &str,
    body: &str,
) -> Result<reqwest::RequestBuilder> {
    ensure_credential_transport(endpoint)?;
    match auth {
        RouteAuth::ApiKey(api_key) => Ok(builder
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .bearer_auth(api_key)),
        RouteAuth::Header { name, value } => Ok(builder
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .header(
                HeaderName::from_bytes(name.as_bytes()).context("invalid auth header name")?,
                HeaderValue::from_str(value).context("invalid auth header value")?,
            )),
        RouteAuth::Headers(headers) => apply_header_pairs(
            builder.header(CONTENT_TYPE, HeaderValue::from_static("application/json")),
            headers,
        ),
        RouteAuth::Composite(auths) => {
            let mut builder =
                builder.header(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            for auth in auths {
                builder = match auth {
                    RouteAuth::ApiKey(api_key) => builder.bearer_auth(api_key),
                    RouteAuth::Header { name, value } => builder.header(
                        HeaderName::from_bytes(name.as_bytes())
                            .context("invalid auth header name")?,
                        HeaderValue::from_str(value).context("invalid auth header value")?,
                    ),
                    RouteAuth::Headers(headers) => apply_header_pairs(builder, headers)?,
                    RouteAuth::Composite(_) => apply_json_headers(builder, auth, endpoint, body)?,
                    RouteAuth::AwsSigV4(credentials) => {
                        builder.headers(sigv4_headers(endpoint, body, credentials)?)
                    }
                };
            }
            Ok(builder)
        }
        RouteAuth::AwsSigV4(credentials) => {
            Ok(builder.headers(sigv4_headers(endpoint, body, credentials)?))
        }
    }
}

fn ensure_credential_transport(endpoint: &str) -> Result<()> {
    let url = reqwest::Url::parse(endpoint).context("failed to parse native LLM endpoint")?;
    match url.scheme() {
        "https" => Ok(()),
        "http" => {
            if !insecure_local_provider_http_opt_in() {
                bail!(
                    "refusing to attach provider credentials over HTTP; use HTTPS for provider base URLs or set {ALLOW_INSECURE_LOCAL_PROVIDER_HTTP_ENV}=1 for loopback/private local development endpoints"
                );
            }
            let host = url
                .host_str()
                .context("native LLM HTTP endpoint has no host")?;
            if !is_loopback_or_private_host(host) {
                bail!(
                    "{ALLOW_INSECURE_LOCAL_PROVIDER_HTTP_ENV} only permits HTTP provider credentials to loopback or private IP endpoints"
                );
            }
            eprintln!(
                "warning: sending native LLM credentials over HTTP to local endpoint {host} because {ALLOW_INSECURE_LOCAL_PROVIDER_HTTP_ENV} is set"
            );
            Ok(())
        }
        scheme => bail!(
            "native LLM endpoint must use HTTPS, or HTTP with {ALLOW_INSECURE_LOCAL_PROVIDER_HTTP_ENV}=1 for local development; got `{scheme}`"
        ),
    }
}

fn insecure_local_provider_http_opt_in() -> bool {
    env::var(ALLOW_INSECURE_LOCAL_PROVIDER_HTTP_ENV)
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
}

fn is_loopback_or_private_host(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if host
        .to_ascii_lowercase()
        .strip_suffix(".localhost")
        .is_some_and(|prefix| !prefix.is_empty())
    {
        return true;
    }
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .is_ok_and(is_loopback_or_private_ip)
}

fn is_loopback_or_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
    }
}

fn apply_header_pairs(
    mut builder: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> Result<reqwest::RequestBuilder> {
    for (name, value) in headers {
        builder = builder.header(
            HeaderName::from_bytes(name.as_bytes()).context("invalid auth header name")?,
            HeaderValue::from_str(value).context("invalid auth header value")?,
        );
    }
    Ok(builder)
}

fn sigv4_headers(endpoint: &str, body: &str, credentials: &AwsCredentials) -> Result<HeaderMap> {
    let url =
        reqwest::Url::parse(endpoint).context("failed to parse Bedrock endpoint for SigV4")?;
    let host = url.host_str().context("Bedrock endpoint has no host")?;
    let path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    let query = url.query().unwrap_or_default();
    let now = Utc::now();
    let amz_date = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let date = &amz_date[..8];
    let payload_hash = hex_sha256(body.as_bytes());
    let mut canonical_headers = format!(
        "content-type:application/json\nhost:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n"
    );
    let mut signed_headers = "content-type;host;x-amz-content-sha256;x-amz-date".to_string();
    if let Some(token) = credentials.session_token.as_deref() {
        canonical_headers.push_str(&format!("x-amz-security-token:{token}\n"));
        signed_headers.push_str(";x-amz-security-token");
    }
    let canonical_request =
        format!("POST\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
    let scope = format!("{date}/{}/bedrock/aws4_request", credentials.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex_sha256(canonical_request.as_bytes())
    );
    let signing_key = signing_key(
        &credentials.secret_access_key,
        date,
        &credentials.region,
        "bedrock",
    );
    let signature = hex_hmac(&signing_key, string_to_sign.as_bytes());
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key_id
    );

    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert(
        "x-amz-content-sha256",
        HeaderValue::from_str(&payload_hash).context("invalid SigV4 payload hash header")?,
    );
    headers.insert(
        "x-amz-date",
        HeaderValue::from_str(&amz_date).context("invalid SigV4 date header")?,
    );
    headers.insert(
        "authorization",
        HeaderValue::from_str(&authorization).context("invalid SigV4 authorization header")?,
    );
    if let Some(token) = credentials.session_token.as_deref() {
        headers.insert(
            HeaderName::from_static("x-amz-security-token"),
            HeaderValue::from_str(token).context("invalid SigV4 security token header")?,
        );
    }
    Ok(headers)
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let date_key = hmac_bytes(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let region_key = hmac_bytes(&date_key, region.as_bytes());
    let service_key = hmac_bytes(&region_key, service.as_bytes());
    hmac_bytes(&service_key, b"aws4_request")
}

fn hmac_bytes(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex_hmac(key: &[u8], data: &[u8]) -> String {
    hex_bytes(&hmac_bytes(key, data))
}

fn hex_sha256(data: &[u8]) -> String {
    hex_bytes(&Sha256::digest(data))
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
#[path = "../test/auth.rs"]
mod tests;
