use anyhow::{Result, bail};

pub(crate) fn render_with_query(
    base_url: Option<&str>,
    default_base_url: &str,
    path: &str,
    query_params: Option<&[(String, String)]>,
) -> Result<String> {
    let base_url = base_url
        .unwrap_or(default_base_url)
        .trim()
        .trim_end_matches('/');
    if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
        bail!("native OpenAI base URL must be http or https");
    }
    let mut url = format!("{base_url}/{}", path.trim_start_matches('/'));
    if let Some(query_params) = query_params.filter(|items| !items.is_empty()) {
        let query = query_params
            .iter()
            .flat_map(|(key, value)| {
                [
                    url::form_urlencoded::byte_serialize(key.as_bytes()).collect::<String>(),
                    "=".to_string(),
                    url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>(),
                    "&".to_string(),
                ]
            })
            .collect::<String>();
        url.push('?');
        url.push_str(query.trim_end_matches('&'));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_rejects_invalid_native_base_url() {
        let err = render_with_query(
            Some("file:///tmp/socket"),
            "https://api.openai.com/v1",
            "responses",
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("must be http or https"));
    }

    #[test]
    fn endpoint_trims_base_and_path_slashes() {
        assert_eq!(
            render_with_query(
                Some("https://example.test/v1/"),
                "https://unused",
                "/chat/completions",
                None,
            )
            .unwrap(),
            "https://example.test/v1/chat/completions"
        );
    }

    #[test]
    fn endpoint_appends_query_params() {
        assert_eq!(
            render_with_query(
                Some("https://example.test/openai/v1"),
                "https://unused",
                "responses",
                Some(&[("api-version".to_string(), "2025-01-01".to_string())]),
            )
            .unwrap(),
            "https://example.test/openai/v1/responses?api-version=2025-01-01"
        );
    }
}
