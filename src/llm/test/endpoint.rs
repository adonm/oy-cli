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
