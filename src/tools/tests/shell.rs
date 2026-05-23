use super::*;

#[tokio::test]
async fn non_interactive_default_denies_bash() {
    let (_dir, ctx) = test_context(ToolPolicy::with_write(Approval::Ask, Approval::Ask), false);
    let err = tool_bash(
        &ctx,
        BashArgs {
            command: "echo nope".into(),
            timeout_seconds: 1,
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("requires interactive approval"));
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bash_returns_full_output_and_bounded_preview() {
    let (_dir, ctx) = test_context(auto_policy(), false);
    let value = tool_bash(
        &ctx,
        BashArgs {
            command: "python3 - <<'PY'\nprint('x' * 13000)\nPY".into(),
            timeout_seconds: 5,
        },
    )
    .await
    .unwrap();

    assert_eq!(value["returncode"], 0);
    assert!(value["stdout"].as_str().unwrap().len() > 12_000);
    assert!(
        value["stdout_preview"].as_str().unwrap().len() < value["stdout"].as_str().unwrap().len()
    );
    assert_eq!(value["stdout_truncated"], true);
    assert_eq!(value["stdout_capped"], false);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bash_accepts_large_commands() {
    let (_dir, ctx) = test_context(auto_policy(), false);
    let mut command = String::from("cat\n");
    for _ in 0..25_000 {
        command.push_str("# padding to exceed argv limits\n");
    }
    command.push_str("printf 'large-ok'\n");

    let value = tool_bash(
        &ctx,
        BashArgs {
            command,
            timeout_seconds: 5,
        },
    )
    .await
    .unwrap();

    assert_eq!(value["returncode"], 0);
    assert_eq!(value["stdout"], "large-ok");
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bash_output_preserves_terminal_sequences_raw() {
    let (_dir, ctx) = test_context(auto_policy(), false);
    let value = tool_bash(
        &ctx,
        BashArgs {
            command: "printf '\\033[31mred\\033(B\\033[m\\a\\b\\v\\f\\016\\017\\n'".into(),
            timeout_seconds: 5,
        },
    )
    .await
    .unwrap();

    let stdout = value["stdout"].as_str().unwrap();
    // Raw terminal sequences are preserved for bat to handle
    assert!(stdout.contains('\x1b'));
    assert!(stdout.contains('\x07'));
    assert_eq!(stdout, "\x1b[31mred\x1b(B\x1b[m\x07\x08\x0b\x0c\x0e\x0f\n");
    assert_eq!(value["stdout_preview"], stdout);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bash_filters_credential_like_environment_variables() {
    let _guard = crate::ENV_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let old_secret = std::env::var("OY_TEST_SECRET_TOKEN").ok();
    let old_public = std::env::var("OY_TEST_PUBLIC_VALUE").ok();
    unsafe {
        std::env::set_var("OY_TEST_SECRET_TOKEN", "do-not-leak");
        std::env::set_var("OY_TEST_PUBLIC_VALUE", "visible");
    }

    let (_dir, ctx) = test_context(auto_policy(), false);
    let value = tool_bash(
        &ctx,
        BashArgs {
            command:
                "printf '%s:%s' \"${OY_TEST_SECRET_TOKEN-unset}\" \"${OY_TEST_PUBLIC_VALUE-unset}\""
                    .into(),
            timeout_seconds: 5,
        },
    )
    .await
    .unwrap();

    match old_secret {
        Some(value) => unsafe { std::env::set_var("OY_TEST_SECRET_TOKEN", value) },
        None => unsafe { std::env::remove_var("OY_TEST_SECRET_TOKEN") },
    }
    match old_public {
        Some(value) => unsafe { std::env::set_var("OY_TEST_PUBLIC_VALUE", value) },
        None => unsafe { std::env::remove_var("OY_TEST_PUBLIC_VALUE") },
    }

    assert_eq!(value["returncode"], 0);
    assert_eq!(value["stdout"].as_str().unwrap(), "unset:visible");
}
