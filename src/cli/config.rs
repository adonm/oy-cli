mod env_config;
mod mode;
mod model_config;
mod paths;
mod prompt;
mod sessions;

pub use env_config::{
    ContextConfig, can_prompt, context_config_for_model, max_bash_cmd_bytes, max_tool_rounds,
    non_interactive,
};
pub use mode::{SafetyMode, policy_risk_label, tool_policy};
pub use model_config::{
    canonical_model_spec, canonical_provider, clear_recent_models, load_model_config,
    recent_models, save_model_config, saved_model_config_from_selection, split_model_spec,
};
pub use paths::{
    config_dir_path, config_root, create_private_dir_all, oy_root, resolve_workspace_output_path,
    sessions_dir, write_private_file, write_workspace_file,
};
pub use prompt::{ask_system_prompt, session_text_value, system_prompt};
pub use sessions::{SessionFile, load_session_file, resolve_saved_session, save_session_file};

#[cfg(test)]
use env_config::parse_tool_round_limit;
#[cfg(test)]
use model_config::updated_recent_models;
#[cfg(test)]
use paths::DEFAULT_CONFIG_DIR_NAME;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::FileAccess;
    use std::{
        env, fs,
        path::{Path, PathBuf},
        sync::Mutex,
    };

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn mode_policy_and_risk_labels_are_centralized() {
        let plan = tool_policy(SafetyMode::Plan);
        assert_eq!(SafetyMode::parse("ask").unwrap().name(), "default");
        assert_eq!(SafetyMode::parse("read_only").unwrap().name(), "plan");
        assert_eq!(SafetyMode::parse("edit").unwrap().name(), "accept-edits");
        assert_eq!(SafetyMode::parse("yolo").unwrap().name(), "auto-approve");
        assert_eq!(plan.files, FileAccess::ReadOnly);
        assert_eq!(
            policy_risk_label(&plan),
            "read-only: no file edits or shell"
        );
        assert_eq!(
            policy_risk_label(&tool_policy(SafetyMode::AutoEdits)),
            "medium: auto edits"
        );
        assert_eq!(
            policy_risk_label(&tool_policy(SafetyMode::AutoAll)),
            "high: auto shell"
        );
    }

    #[test]
    fn output_paths_stay_in_workspace() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_workspace_output_path(dir.path(), Path::new("notes/out.md")).is_ok());
        assert!(resolve_workspace_output_path(dir.path(), Path::new("../out.md")).is_err());
        assert!(resolve_workspace_output_path(dir.path(), Path::new("/tmp/out.md")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn output_paths_reject_symlink_ancestor_escapes() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), dir.path().join("reports")).unwrap();
        let err =
            resolve_workspace_output_path(dir.path(), Path::new("reports/new/out.md")).unwrap_err();
        assert!(err.to_string().contains("symlink ancestor"));
    }

    #[cfg(unix)]
    #[test]
    fn output_paths_reject_symlink_destinations() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.md");
        fs::write(&target, "safe").unwrap();
        symlink(&target, dir.path().join("link.md")).unwrap();
        let err = resolve_workspace_output_path(dir.path(), Path::new("link.md")).unwrap_err();
        assert!(err.to_string().contains("refusing to write symlink"));
    }

    #[test]
    fn default_config_dir_name_is_rust_specific() {
        assert_eq!(DEFAULT_CONFIG_DIR_NAME, "oy-rust");
    }

    #[test]
    fn saved_model_config_canonicalizes_provider_specs() {
        let saved = saved_model_config_from_selection("copilot::gpt-5.5");
        assert_eq!(saved.model.as_deref(), Some("github-copilot/gpt-5.5"));
        assert!(saved.recent_models.is_empty());

        let saved = saved_model_config_from_selection("openai::gpt-5.5");
        assert_eq!(saved.model.as_deref(), Some("openai/gpt-5.5"));
    }

    #[test]
    fn recent_models_are_deduped_most_recent_first_and_limited() {
        let previous = vec![
            "gpt-a".to_string(),
            "gpt-b".to_string(),
            "gpt-c".to_string(),
            "gpt-d".to_string(),
            "gpt-e".to_string(),
        ];
        assert_eq!(
            updated_recent_models(&previous, " gpt-c "),
            vec!["gpt-c", "gpt-a", "gpt-b", "gpt-d", "gpt-e"]
        );
        assert_eq!(
            updated_recent_models(&previous, "gpt-f"),
            vec!["gpt-f", "gpt-a", "gpt-b", "gpt-c", "gpt-d"]
        );
    }

    #[test]
    fn save_and_clear_model_config_persist_recent_models() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.json");
        unsafe { env::set_var("OY_CONFIG", &config) };

        save_model_config("gpt-a").unwrap();
        save_model_config("gpt-b").unwrap();
        save_model_config("gpt-a").unwrap();
        assert_eq!(recent_models().unwrap(), vec!["gpt-a", "gpt-b"]);

        clear_recent_models().unwrap();
        let saved = load_model_config().unwrap();
        assert_eq!(saved.model.as_deref(), Some("gpt-a"));
        assert!(saved.recent_models.is_empty());

        unsafe { env::remove_var("OY_CONFIG") };
    }

    #[test]
    fn split_model_spec_supports_double_colon() {
        assert_eq!(
            split_model_spec("copilot::gpt-4.1-mini"),
            (Some("copilot"), "gpt-4.1-mini")
        );
    }

    #[test]
    fn split_model_spec_leaves_plain_models_untouched() {
        assert_eq!(split_model_spec("gpt-5.4-mini"), (None, "gpt-5.4-mini"));
    }

    #[test]
    fn session_text_loads_base_prompt() {
        let prompt = session_text_value("system", "base").unwrap();
        assert!(prompt.contains("You are oy"));
        assert!(prompt.contains("Do not retry the same call unchanged"));
    }

    #[test]
    fn session_file_save_stores_mode() {
        let file = SessionFile {
            model: "gpt-test".into(),
            saved_at: "2026-01-01T00:00:00".into(),
            workspace_root: PathBuf::from("/workspace"),
            mode: Some(SafetyMode::Default),
            transcript: serde_json::json!({"messages": []}),
            todos: Vec::new(),
        };
        let raw = serde_json::to_value(&file).unwrap();
        assert_eq!(raw["mode"], "default");
        assert!(raw.get("agent").is_none());
    }

    #[test]
    fn tool_round_limit_supports_high_and_unlimited_values() {
        assert_eq!(parse_tool_round_limit(None, 512), 512);
        assert_eq!(parse_tool_round_limit(Some("2048"), 512), 2048);
        assert!(parse_tool_round_limit(Some("0"), 512) > 1_000_000);
        assert!(parse_tool_round_limit(Some("unlimited"), 512) > 1_000_000);
        assert_eq!(parse_tool_round_limit(Some("bad"), 512), 512);
    }
}
