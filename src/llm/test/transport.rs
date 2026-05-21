use super::*;

#[test]
fn provider_error_message_prefers_openai_error_message() {
    let text = serde_json::json!({"error": {"message": "bad request detail"}}).to_string();

    assert_eq!(
        provider_error_message(StatusCode::BAD_REQUEST, &text),
        "bad request detail"
    );
}
