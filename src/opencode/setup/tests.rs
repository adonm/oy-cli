use super::*;
use super::{
    backup::{TEST_BACKUP_STATE_DIR, backup_state_dir, copy_path},
    config_file::{format_json, remove_oy_config_entries, update_config},
};
use serde_json::Value;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &Path) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(value) = &self.previous {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

struct BackupStateGuard;

impl BackupStateGuard {
    fn set(path: PathBuf) -> Self {
        TEST_BACKUP_STATE_DIR.with(|state| {
            assert!(state.replace(Some(path)).is_none());
        });
        Self
    }
}

impl Drop for BackupStateGuard {
    fn drop(&mut self) {
        TEST_BACKUP_STATE_DIR.with(|state| {
            state.replace(None);
        });
    }
}

fn backup_dirs(_config_dir: &Path) -> Vec<PathBuf> {
    let base = backup_state_dir().unwrap().join("oy/backups");
    let mut backups = fs::read_dir(base)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    backups.sort();
    backups
}

#[test]
fn setup_defaults_to_global_opencode_config() {
    let _lock = ENV_LOCK.lock().unwrap();
    let config_home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _xdg = EnvGuard::set("XDG_CONFIG_HOME", config_home.path());
    let _backup_state = BackupStateGuard::set(config_home.path().join("state"));
    let _root = EnvGuard::set("OY_ROOT", workspace.path());
    let _host = EnvGuard::set("OY_OPENCODE", &workspace.path().join("missing-opencode"));

    setup_command(false, false, false).unwrap();

    let global = config_home.path().join("opencode");
    assert!(global.join("opencode.json").exists());
    assert!(!global.join("agents/oy.md").exists());
    assert!(!global.join("agents/oy-plan.md").exists());
    assert!(config_has_all_oy_entries(&global.join("opencode.json")));
    assert!(!workspace.path().join(".opencode/opencode.json").exists());
}

#[test]
fn workspace_setup_is_explicit() {
    let _lock = ENV_LOCK.lock().unwrap();
    let config_home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _xdg = EnvGuard::set("XDG_CONFIG_HOME", config_home.path());
    let _backup_state = BackupStateGuard::set(config_home.path().join("state"));
    let _root = EnvGuard::set("OY_ROOT", workspace.path());
    let _host = EnvGuard::set("OY_OPENCODE", &workspace.path().join("missing-opencode"));

    setup_command(true, false, false).unwrap();

    assert!(workspace.path().join(".opencode/opencode.json").exists());
    assert!(!workspace.path().join(".opencode/agents/oy.md").exists());
    assert!(config_has_all_oy_entries(
        &workspace.path().join(".opencode/opencode.json")
    ));
    assert!(!config_home.path().join("opencode/opencode.json").exists());
}

#[test]
fn setup_dry_run_does_not_write_files() {
    let _lock = ENV_LOCK.lock().unwrap();
    let config_home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _xdg = EnvGuard::set("XDG_CONFIG_HOME", config_home.path());
    let _backup_state = BackupStateGuard::set(config_home.path().join("state"));
    let _root = EnvGuard::set("OY_ROOT", workspace.path());
    let _host = EnvGuard::set("OY_OPENCODE", &workspace.path().join("missing-opencode"));

    setup_command(false, true, false).unwrap();

    assert!(!config_home.path().join("opencode/opencode.json").exists());
    assert!(!config_home.path().join("opencode/agents/oy.md").exists());
}

#[test]
fn setup_preserves_user_config_and_merges_oy_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    fs::write(
        &path,
        r#"{
  "$schema": "https://opencode.ai/config.json",
  "model": "test/model",
  "command": { "keep": { "template": "keep me" } },
  "mcp": { "other": { "type": "local", "command": ["other"] } }
}
"#,
    )
    .unwrap();

    update_config(&path).unwrap();

    let updated: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(updated["model"], "test/model");
    assert_eq!(updated["command"]["keep"]["template"], "keep me");
    assert_eq!(updated["plugins"], json!([opencode_plugin_spec()]));
    assert_eq!(updated["mcp"]["other"]["command"][0], "other");
    assert!(updated.pointer("/mcp/servers/oy").is_none());
    assert!(updated.pointer("/mcp/oy").is_none());
    assert!(updated.get("tool_output").is_none());
    assert!(updated.get("default_agent").is_none());
}

#[test]
fn setup_is_idempotent_after_namespace_cleanup() {
    let _lock = ENV_LOCK.lock().unwrap();
    let config_home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _xdg = EnvGuard::set("XDG_CONFIG_HOME", config_home.path());
    let _backup_state = BackupStateGuard::set(config_home.path().join("state"));
    let _root = EnvGuard::set("OY_ROOT", workspace.path());
    let _host = EnvGuard::set("OY_OPENCODE", &workspace.path().join("missing-opencode"));
    let dir = config_home.path().join("opencode");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("opencode.json"),
        r#"{
  "$schema": "https://opencode.ai/config.json",
  "model": "test/model",
  "command": { "keep": { "template": "keep me" } },
  "mcp": { "other": { "type": "local", "command": ["other"] } },
  "tool_output": { "extra_user_key": true }
}
"#,
    )
    .unwrap();

    setup_command(false, false, false).unwrap();
    let owned_paths = [dir.join("opencode.json")];
    let first = owned_paths
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();

    setup_command(false, false, false).unwrap();
    let second = owned_paths
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(second, first);
    let updated: Value = serde_json::from_slice(second.last().expect("config bytes")).unwrap();
    assert_eq!(updated["model"], "test/model");
    assert_eq!(updated["command"]["keep"]["template"], "keep me");
    assert_eq!(updated["mcp"]["other"]["command"][0], "other");
    assert_eq!(updated["tool_output"]["extra_user_key"], true);
    assert_eq!(updated["plugins"], json!([opencode_plugin_spec()]));
    assert_eq!(backup_dirs(&dir).len(), 1);
}

#[test]
fn setup_moves_modified_oy_files_to_backup() {
    let _lock = ENV_LOCK.lock().unwrap();
    let config_home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _xdg = EnvGuard::set("XDG_CONFIG_HOME", config_home.path());
    let _backup_state = BackupStateGuard::set(config_home.path().join("state"));
    let _root = EnvGuard::set("OY_ROOT", workspace.path());
    let _host = EnvGuard::set("OY_OPENCODE", &workspace.path().join("missing-opencode"));
    let agent = config_home.path().join("opencode/agents/oy.md");
    fs::create_dir_all(agent.parent().unwrap()).unwrap();
    fs::write(&agent, "user-owned agent\n").unwrap();

    setup_command(false, false, false).unwrap();

    assert!(!agent.exists());
    let backups = backup_dirs(&config_home.path().join("opencode"));
    assert_eq!(backups.len(), 1);
    assert_eq!(
        fs::read_to_string(backups[0].join("agents/oy.md")).unwrap(),
        "user-owned agent\n"
    );
    assert!(config_has_all_oy_entries(
        &config_home.path().join("opencode/opencode.json")
    ));
}

#[test]
fn setup_preserves_generic_tool_output_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    fs::write(
        &path,
        r#"{
  "$schema": "https://opencode.ai/config.json",
  "tool_output": { "max_bytes": 262144, "max_lines": 20000, "extra_user_key": true }
}
"#,
    )
    .unwrap();

    update_config(&path).unwrap();

    let updated: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(updated["tool_output"]["max_bytes"], 262_144);
    assert_eq!(updated["tool_output"]["max_lines"], 20_000);
    assert_eq!(updated["tool_output"]["extra_user_key"], true);
}

#[test]
fn setup_accepts_opencode_jsonc() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    fs::write(
        &path,
        r#"{
  // opencode allows comments and trailing commas.
  "$schema": "https://opencode.ai/config.json",
  "model": "test/model",
  "command": {
    "keep": { "template": "https://example.com//not-a-comment" },
  },
}
"#,
    )
    .unwrap();

    update_config(&path).unwrap();

    let updated: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(updated["model"], "test/model");
    assert_eq!(
        updated["command"]["keep"]["template"],
        "https://example.com//not-a-comment"
    );
    assert!(updated.pointer("/mcp/servers/oy").is_none());
}

#[test]
fn setup_cleans_oy_entries_from_both_config_files() {
    let _lock = ENV_LOCK.lock().unwrap();
    let config_home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _xdg = EnvGuard::set("XDG_CONFIG_HOME", config_home.path());
    let _backup_state = BackupStateGuard::set(config_home.path().join("state"));
    let _root = EnvGuard::set("OY_ROOT", workspace.path());
    let _host = EnvGuard::set("OY_OPENCODE", &workspace.path().join("missing-opencode"));
    let dir = config_home.path().join("opencode");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("opencode.json"),
        r#"{ "model": "lower", "commands": { "oy-modified": { "custom": true } } }"#,
    )
    .unwrap();
    fs::write(dir.join("opencode.jsonc"), r#"{ "model": "upper" }"#).unwrap();

    setup_command(false, false, false).unwrap();

    let lower: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("opencode.json")).unwrap()).unwrap();
    let upper: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("opencode.jsonc")).unwrap()).unwrap();
    assert_eq!(lower["model"], "lower");
    assert!(lower.get("commands").is_none());
    assert!(lower.get("plugins").is_none());
    assert_eq!(upper["model"], "upper");
    assert_eq!(upper["plugins"], json!([opencode_plugin_spec()]));
    let backups = backup_dirs(&dir);
    assert_eq!(backups.len(), 1);
    assert!(backups[0].join("opencode.json").exists());
    assert!(backups[0].join("opencode.jsonc").exists());
}

#[test]
fn setup_accepts_native_v2_config_and_preserves_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    fs::write(
        &path,
        r#"{
  "commands": { "keep": { "template": "keep me" } },
  "mcp": { "servers": {} }
}
"#,
    )
    .unwrap();

    update_config(&path).unwrap();
    let updated: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(updated["commands"]["keep"]["template"], "keep me");
    assert!(updated["commands"].get("oy-audit").is_none());
    assert_eq!(updated["plugins"], json!([opencode_plugin_spec()]));
    assert!(updated.pointer("/mcp/servers/oy").is_none());
}

#[test]
fn setup_leaves_unrelated_legacy_fields_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    fs::write(
        &path,
        r#"{ "permission": { "edit": "ask" }, "experimental": { "mcp_timeout": 30000 } }"#,
    )
    .unwrap();

    update_config(&path).unwrap();

    let updated: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(updated["permission"]["edit"], "ask");
    assert_eq!(updated["experimental"]["mcp_timeout"], 30_000);
}

#[test]
fn explicit_setup_backs_up_all_oy_named_agents() {
    let _lock = ENV_LOCK.lock().unwrap();
    let config_home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _xdg = EnvGuard::set("XDG_CONFIG_HOME", config_home.path());
    let _backup_state = BackupStateGuard::set(config_home.path().join("state"));
    let _root = EnvGuard::set("OY_ROOT", workspace.path());
    let _host = EnvGuard::set("OY_OPENCODE", &workspace.path().join("missing-opencode"));

    setup_command(true, false, false).unwrap();
    let agent = workspace.path().join(".opencode/agents/oy.md");
    fs::create_dir_all(agent.parent().unwrap()).unwrap();
    fs::write(&agent, OY_AGENT).unwrap();
    let reviewer = workspace.path().join(".opencode/agents/oy-reviewer.md");
    fs::write(
        &reviewer,
        "<!-- Generated by oy setup -->\nold generated reviewer\n",
    )
    .unwrap();

    setup_opencode(SetupScope::Workspace, false, false).unwrap();

    assert!(!reviewer.exists());
    assert!(!agent.exists());
    let backups = backup_dirs(&workspace.path().join(".opencode"));
    assert_eq!(backups.len(), 1);
    assert_eq!(
        fs::read_to_string(backups[0].join("agents/oy.md")).unwrap(),
        OY_AGENT
    );
    assert!(backups[0].join("agents/oy-reviewer.md").exists());
    assert!(config_has_all_oy_entries(
        &workspace.path().join(".opencode/opencode.json")
    ));
    assert!(!config_home.path().join("opencode/opencode.json").exists());
}

#[test]
fn setup_migrates_direct_assets_and_commands_to_versioned_plugin() {
    let _lock = ENV_LOCK.lock().unwrap();
    let config_home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _xdg = EnvGuard::set("XDG_CONFIG_HOME", config_home.path());
    let _backup_state = BackupStateGuard::set(config_home.path().join("state"));
    let _root = EnvGuard::set("OY_ROOT", workspace.path());
    let _host = EnvGuard::set("OY_OPENCODE", &workspace.path().join("missing-opencode"));
    let dir = config_home.path().join("opencode");
    fs::create_dir_all(&dir).unwrap();

    fs::write(
        dir.join("opencode.json"),
        format_json(&json!({
            "model": "test/model",
            "commands": {
                "oy-audit": { "modified": true },
                "oy-custom": { "template": "old custom command" }
            },
            "plugins": ["keep-plugin", "@oy-cli/opencode@0.13.0"]
        }))
        .unwrap(),
    )
    .unwrap();
    let agent = dir.join("agents/oy-local.md");
    let skill = dir.join("skills/oy-custom/SKILL.md");
    let unrelated = dir.join("agents/keep.md");
    fs::create_dir_all(agent.parent().unwrap()).unwrap();
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(&agent, "locally modified agent\n").unwrap();
    fs::write(&skill, "locally modified skill\n").unwrap();
    fs::write(&unrelated, "keep\n").unwrap();

    setup_command(false, false, false).unwrap();

    let updated: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("opencode.json")).unwrap()).unwrap();
    assert_eq!(updated["model"], "test/model");
    assert!(updated.get("commands").is_none());
    assert_eq!(
        updated["plugins"],
        json!(["keep-plugin", opencode_plugin_spec()])
    );
    assert!(!agent.exists());
    assert!(!skill.exists());
    assert_eq!(fs::read_to_string(unrelated).unwrap(), "keep\n");
    let backups = backup_dirs(&dir);
    assert_eq!(backups.len(), 1);
    assert!(backups[0].starts_with(config_home.path().join("state/oy/backups")));
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&backups[0]).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert_eq!(
        fs::read_to_string(backups[0].join("agents/oy-local.md")).unwrap(),
        "locally modified agent\n"
    );
    assert_eq!(
        fs::read_to_string(backups[0].join("skills/oy-custom/SKILL.md")).unwrap(),
        "locally modified skill\n"
    );
    let previous: Value =
        serde_json::from_str(&fs::read_to_string(backups[0].join("opencode.json")).unwrap())
            .unwrap();
    assert_eq!(previous["commands"]["oy-audit"]["modified"], true);
}

#[test]
fn cross_filesystem_copy_helper_preserves_nested_backup_contents() {
    let source = tempfile::tempdir().unwrap();
    let destination_root = tempfile::tempdir().unwrap();
    let nested = source.path().join("oy-custom/nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("SKILL.md"), "modified\n").unwrap();
    let destination = destination_root.path().join("oy-custom");

    copy_path(&source.path().join("oy-custom"), &destination).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("nested/SKILL.md")).unwrap(),
        "modified\n"
    );
    assert!(source.path().join("oy-custom/nested/SKILL.md").exists());
}

#[test]
fn failed_config_update_restores_files_and_retains_snapshot() {
    let config = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let _backup_state = BackupStateGuard::set(state.path().to_path_buf());
    let old_file = config.path().join("agents/oy-modified.md");
    fs::create_dir_all(old_file.parent().unwrap()).unwrap();
    fs::write(&old_file, "modified\n").unwrap();
    let invalid_config = config.path().join("opencode.json");
    fs::create_dir(&invalid_config).unwrap();
    let updates = [ConfigUpdate {
        path: invalid_config,
        body: "{}\n".to_string(),
        current: Some(b"old config\n".to_vec()),
    }];

    let error = apply_integration_update(config.path(), std::slice::from_ref(&old_file), &updates)
        .unwrap_err();

    assert!(error.to_string().contains("backup retained"));
    assert_eq!(fs::read_to_string(old_file).unwrap(), "modified\n");
    let backups = backup_dirs(config.path());
    assert_eq!(backups.len(), 1);
    assert_eq!(
        fs::read_to_string(backups[0].join("opencode.json")).unwrap(),
        "old config\n"
    );
}

#[test]
fn rollback_attempts_all_moved_paths_after_one_restore_fails() {
    let dir = tempfile::tempdir().unwrap();
    let good_source = dir.path().join("agents/oy-good.md");
    let good_backup = dir.path().join("backup-good.md");
    fs::write(&good_backup, "good\n").unwrap();
    let blocked_parent = dir.path().join("blocked");
    fs::write(&blocked_parent, "not a directory\n").unwrap();
    let bad_source = blocked_parent.join("oy-bad.md");
    let bad_backup = dir.path().join("backup-bad.md");
    fs::write(&bad_backup, "bad\n").unwrap();

    let error = restore_moved_paths(&[
        (good_source.clone(), good_backup),
        (bad_source, bad_backup.clone()),
    ])
    .unwrap_err();

    assert!(!error.to_string().is_empty());
    assert_eq!(fs::read_to_string(good_source).unwrap(), "good\n");
    assert!(bad_backup.exists());
}

#[test]
fn cross_filesystem_copy_helper_does_not_follow_symlinks() {
    use std::os::unix::fs::symlink;

    let source = tempfile::tempdir().unwrap();
    let destination_root = tempfile::tempdir().unwrap();
    let destination = destination_root.path().join("oy-link.md");
    let link = source.path().join("oy-link.md");
    symlink("../outside.md", &link).unwrap();

    copy_path(&link, &destination).unwrap();

    assert_eq!(
        fs::read_link(destination).unwrap(),
        PathBuf::from("../outside.md")
    );
}

#[test]
fn namespace_scan_rejects_symlinked_directories() {
    use std::os::unix::fs::symlink;

    let config = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("oy-victim.md");
    fs::write(&victim, "keep\n").unwrap();
    symlink(outside.path(), config.path().join("agents")).unwrap();

    let error = legacy_oy_paths(config.path()).unwrap_err();

    assert!(error.to_string().contains("symlinked OpenCode namespace"));
    assert_eq!(fs::read_to_string(victim).unwrap(), "keep\n");
}

#[test]
fn config_update_rejects_symlinked_file() {
    use std::os::unix::fs::symlink;

    let config = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("config.json");
    fs::write(&target, r#"{ "model": "keep/me" }"#).unwrap();
    let link = config.path().join("opencode.json");
    symlink(&target, &link).unwrap();

    let error = update_config(&link).unwrap_err();

    assert!(error.to_string().contains("symlinked OpenCode config"));
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        r#"{ "model": "keep/me" }"#
    );
}

#[test]
fn config_replaces_object_form_oy_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    fs::write(
        &path,
        r#"{ "plugins": [{ "package": "@oy-cli/opencode", "options": { "custom": true } }] }"#,
    )
    .unwrap();

    update_config(&path).unwrap();

    let updated: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(updated["plugins"], json!([opencode_plugin_spec()]));
}

#[test]
fn missing_integration_check_does_not_create_files() {
    let _lock = ENV_LOCK.lock().unwrap();
    let config_home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _xdg = EnvGuard::set("XDG_CONFIG_HOME", config_home.path());
    let _backup_state = BackupStateGuard::set(config_home.path().join("state"));
    let _root = EnvGuard::set("OY_ROOT", workspace.path());
    let _host = EnvGuard::set("OY_OPENCODE", &workspace.path().join("missing-opencode"));

    assert!(!integration_complete(&config_home.path().join("opencode")));
    assert!(!integration_complete(&workspace.path().join(".opencode")));

    assert!(!config_home.path().join("opencode/opencode.json").exists());
    assert!(!workspace.path().join(".opencode/opencode.json").exists());
}

#[test]
fn setup_prompt_defaults_to_yes_and_accepts_explicit_yes() {
    assert!(setup_answer_is_yes(""));
    assert!(setup_answer_is_yes("Y\n"));
    assert!(setup_answer_is_yes("yes"));
    assert!(!setup_answer_is_yes("n"));
    assert!(!setup_answer_is_yes("later"));
}

#[test]
fn setup_remove_round_trip_preserves_unrelated_config() {
    let _lock = ENV_LOCK.lock().unwrap();
    let config_home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let _xdg = EnvGuard::set("XDG_CONFIG_HOME", config_home.path());
    let _backup_state = BackupStateGuard::set(config_home.path().join("state"));
    let _root = EnvGuard::set("OY_ROOT", workspace.path());
    let _host = EnvGuard::set("OY_OPENCODE", &workspace.path().join("missing-opencode"));
    let dir = config_home.path().join("opencode");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("opencode.json"), r#"{ "model": "test/model" }"#).unwrap();

    setup_command(false, false, false).unwrap();
    setup_command(false, false, true).unwrap();

    let config: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("opencode.json")).unwrap()).unwrap();
    assert_eq!(config["model"], "test/model");
    assert!(config.pointer("/mcp/servers/oy").is_none());
    assert!(config.get("plugins").is_none());
    assert!(!dir.join("agents/oy.md").exists());
}

#[test]
fn removal_uses_oy_namespace_without_matching_old_contents() {
    let mut config = json!({
        "command": {
            "oy-old": { "modified": true },
            "keep": { "template": "keep" }
        },
        "commands": { "oy-new": "any shape" },
        "mcp": {
            "oy": { "modified": true },
            "servers": {
                "oy": { "modified": true },
                "keep": { "type": "local" }
            }
        },
        "plugins": [
            { "package": "@oy-cli/opencode", "options": { "custom": true } },
            "keep-plugin"
        ]
    });

    remove_oy_config_entries(config.as_object_mut().unwrap()).unwrap();

    assert_eq!(config["command"]["keep"]["template"], "keep");
    assert!(config.get("commands").is_none());
    assert!(config["mcp"].get("oy").is_none());
    assert!(config["mcp"]["servers"].get("oy").is_none());
    assert_eq!(config["mcp"]["servers"]["keep"]["type"], "local");
    assert_eq!(config["plugins"], json!(["keep-plugin"]));
}
