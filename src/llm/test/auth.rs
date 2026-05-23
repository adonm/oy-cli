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
