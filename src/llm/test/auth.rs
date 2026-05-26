use super::*;

use crate::ENV_LOCK;

struct EnvGuard {
    saved: Option<String>,
}

impl EnvGuard {
    fn set_allow_insecure_local(value: Option<&str>) -> Self {
        let saved = env::var(ALLOW_INSECURE_LOCAL_PROVIDER_HTTP_ENV).ok();
        match value {
            Some(value) => unsafe { env::set_var(ALLOW_INSECURE_LOCAL_PROVIDER_HTTP_ENV, value) },
            None => unsafe { env::remove_var(ALLOW_INSECURE_LOCAL_PROVIDER_HTTP_ENV) },
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.saved.take() {
            Some(value) => unsafe { env::set_var(ALLOW_INSECURE_LOCAL_PROVIDER_HTTP_ENV, value) },
            None => unsafe { env::remove_var(ALLOW_INSECURE_LOCAL_PROVIDER_HTTP_ENV) },
        }
    }
}

#[test]
fn credential_transport_allows_https() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set_allow_insecure_local(None);

    ensure_credential_transport("https://api.openai.com/v1/responses").unwrap();
}

#[test]
fn credential_transport_rejects_http_without_opt_in() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set_allow_insecure_local(None);

    let err =
        ensure_credential_transport("http://127.0.0.1:11434/v1/chat/completions").unwrap_err();

    assert!(
        err.to_string()
            .contains("refusing to attach provider credentials over HTTP")
    );
}

#[test]
fn credential_transport_allows_local_http_with_opt_in() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set_allow_insecure_local(Some("1"));

    ensure_credential_transport("http://localhost:11434/v1/chat/completions").unwrap();
    ensure_credential_transport("http://10.0.0.5/v1/chat/completions").unwrap();
    ensure_credential_transport("http://[fd00::1]/v1/chat/completions").unwrap();
}

#[test]
fn credential_transport_rejects_public_http_with_opt_in() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvGuard::set_allow_insecure_local(Some("1"));

    let err = ensure_credential_transport("http://api.openai.com/v1/responses").unwrap_err();

    assert!(
        err.to_string().contains(
            "OY_ALLOW_INSECURE_LOCAL_PROVIDER_HTTP only permits HTTP provider credentials"
        )
    );
}

#[test]
fn sigv4_signing_key_matches_aws_documented_example() {
    let key = signing_key(
        "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        "20150830",
        "us-east-1",
        "iam",
    );

    assert_eq!(
        hex_bytes(&key),
        "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
    );
}

#[test]
fn test_sigv4_headers_with_custom_port() {
    let credentials = AwsCredentials {
        access_key_id: "test_key".to_string(),
        secret_access_key: "test_secret".to_string(),
        session_token: None,
        region: "us-east-1".to_string(),
    };

    // Signature with custom port should be different from signature without port or with standard port
    let headers_with_custom_port =
        sigv4_headers("http://localhost:8000/v1/foo", "{}", &credentials).unwrap();
    let auth_with_custom_port = headers_with_custom_port
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();

    let headers_without_port =
        sigv4_headers("http://localhost/v1/foo", "{}", &credentials).unwrap();
    let auth_without_port = headers_without_port
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();

    let headers_with_standard_port =
        sigv4_headers("http://localhost:80/v1/foo", "{}", &credentials).unwrap();
    let auth_with_standard_port = headers_with_standard_port
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();

    assert_ne!(auth_with_custom_port, auth_without_port);
    assert_eq!(auth_without_port, auth_with_standard_port);
}

#[test]
fn test_recursive_composite_auth() {
    let client = reqwest::Client::new();
    let builder = client.post("https://api.openai.com/v1/chat/completions");
    let auth = RouteAuth::Composite(vec![
        RouteAuth::ApiKey("secret_key_1".to_string()),
        RouteAuth::Composite(vec![RouteAuth::Header {
            name: "X-Custom".to_string(),
            value: "value_2".to_string(),
        }]),
    ]);

    let builder_with_headers = apply_json_headers(
        builder,
        &auth,
        "https://api.openai.com/v1/chat/completions",
        "{}",
    )
    .unwrap();

    let request = builder_with_headers.build().unwrap();
    let headers = request.headers();

    assert_eq!(headers.get("authorization").unwrap(), "Bearer secret_key_1");
    assert_eq!(headers.get("x-custom").unwrap(), "value_2");
}
