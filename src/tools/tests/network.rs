use super::*;

#[test]
fn webfetch_defaults_to_markdown() {
    let args: WebfetchArgs = serde_json::from_value(json!({
        "url": "https://docs.aws.amazon.com/AmazonS3/latest/userguide/s3-files-mounting-eks.md"
    }))
    .unwrap();
    assert_eq!(args.return_format, ReturnFormat::Markdown);
}

#[tokio::test]
async fn webfetch_checks_network_policy_at_sink() {
    let (_dir, ctx) = test_context(
        ToolPolicy {
            files: FileAccess::ReadOnly,
            shell: Approval::Deny,
            network: NetworkAccess::Disabled,
        },
        false,
    );
    let err = tool_webfetch(
        &ctx,
        WebfetchArgs {
            url: "https://example.com".into(),
            return_format: ReturnFormat::Markdown,
            user_agent: None,
            cookie: None,
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("tool denied by policy"));
}
