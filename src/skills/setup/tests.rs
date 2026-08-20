use super::*;
use super::{
    backup::{TEST_BACKUP_STATE_DIR, backup_state_dir, copy_path},
    legacy_config::{config_has_oy_entries, remove_oy_config_entries, update_config},
};
use crate::skills::{OY_PERSONA, OY_SETUP_SKILL};
use serde_json::Value;
use std::ffi::OsString;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &Path) -> Self {
        let previous = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        unsafe {
            std::env::remove_var(key);
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

/// A controlled environment: HOME and XDG_CONFIG_HOME under a temp dir, a
/// separate workspace root, and a missing OpenCode host so eviction is skipped.
struct TestEnv {
    home: tempfile::TempDir,
    workspace: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
    _xdg: EnvGuard,
    _skills: EnvGuard,
    _cache: EnvGuard,
    _backup_state: BackupStateGuard,
    _root: EnvGuard,
    _host: EnvGuard,
    _config_dir: EnvGuard,
}

impl TestEnv {
    fn new() -> Self {
        let lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let xdg = EnvGuard::set("XDG_CONFIG_HOME", &home.path().join(".config"));
        let skills = EnvGuard::set("OY_SKILLS_DIR", &home.path().join(".agents/skills"));
        let cache = EnvGuard::set("XDG_CACHE_HOME", &home.path().join(".cache"));
        let backup_state = BackupStateGuard::set(home.path().join("state"));
        let root = EnvGuard::set("OY_ROOT", workspace.path());
        let host = EnvGuard::set("OY_OPENCODE", &workspace.path().join("missing-opencode"));
        let _config_dir = EnvGuard::remove("OPENCODE_CONFIG_DIR");
        Self {
            home,
            workspace,
            _lock: lock,
            _xdg: xdg,
            _skills: skills,
            _cache: cache,
            _backup_state: backup_state,
            _root: root,
            _host: host,
            _config_dir,
        }
    }

    fn plugin_cache(&self) -> PathBuf {
        self.home.path().join(".cache/opencode/packages")
    }

    fn global_skills(&self) -> PathBuf {
        self.home.path().join(".agents/skills")
    }

    fn global_opencode(&self) -> PathBuf {
        self.home.path().join(".config/opencode")
    }

    fn workspace_skills(&self) -> PathBuf {
        self.workspace.path().join(".agents/skills")
    }
}

fn backup_dirs() -> Vec<PathBuf> {
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

fn assert_skills_installed(dir: &Path) {
    for (relative, canonical) in bundled_files() {
        let path = dir.join(relative);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            canonical,
            "{} must be canonical",
            path.display()
        );
    }
    assert!(skills_complete(dir));
}

#[test]
fn setup_defaults_to_global_skills_dir() {
    let env = TestEnv::new();

    setup_command(false, false, false).unwrap();

    assert_skills_installed(&env.global_skills());
    assert!(!env.workspace_skills().exists());
    assert!(!env.global_opencode().exists());
    assert!(backup_dirs().is_empty());
}

#[test]
fn workspace_setup_writes_project_skills() {
    let env = TestEnv::new();

    setup_command(true, false, false).unwrap();

    assert_skills_installed(&env.workspace_skills());
    assert!(!env.global_skills().exists());
    assert!(backup_dirs().is_empty());
}

#[test]
fn setup_dry_run_does_not_write_files() {
    let env = TestEnv::new();

    setup_command(false, true, false).unwrap();

    assert!(!env.global_skills().exists());
    assert!(!env.global_opencode().exists());
}

#[test]
fn setup_is_idempotent_without_churn() {
    let env = TestEnv::new();

    setup_command(false, false, false).unwrap();
    let first = fs::read_to_string(env.global_skills().join("oy-audit/SKILL.md")).unwrap();

    setup_command(false, false, false).unwrap();
    let second = fs::read_to_string(env.global_skills().join("oy-audit/SKILL.md")).unwrap();

    assert_eq!(second, first);
    assert!(backup_dirs().is_empty());
}

#[test]
fn setup_preserves_user_modified_skill_files() {
    let env = TestEnv::new();
    setup_command(false, false, false).unwrap();
    let path = env.global_skills().join("oy-enhance/SKILL.md");
    fs::write(&path, "user-owned skill without the setup marker\n").unwrap();

    setup_command(false, false, false).unwrap();

    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "user-owned skill without the setup marker\n"
    );
    assert!(backup_dirs().is_empty());
}

#[test]
fn setup_refreshes_stale_owned_skills_and_backs_them_up() {
    let env = TestEnv::new();
    setup_command(false, false, false).unwrap();
    let path = env.global_skills().join("oy-review/SKILL.md");
    fs::write(&path, format!("{GENERATED_MARKER}\nstale review skill\n")).unwrap();

    setup_command(false, false, false).unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), OY_REVIEW_SKILL);
    let backups = backup_dirs();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        fs::read_to_string(backups[0].join("oy-review/SKILL.md")).unwrap(),
        format!("{GENERATED_MARKER}\nstale review skill\n")
    );
}

#[test]
fn setup_strips_legacy_opencode_plugin_config() {
    let env = TestEnv::new();
    let dir = env.global_opencode();
    fs::create_dir_all(&dir).unwrap();
    let original = r#"{
  "$schema": "https://opencode.ai/config.json",
  "model": "test/model",
  "plugins": ["@oy-cli/opencode@0.14.0", "keep-plugin"],
  "command": { "keep": { "template": "keep me" }, "oy-review": { "template": "old" } }
}
"#;
    fs::write(dir.join("opencode.json"), original).unwrap();

    setup_command(false, false, false).unwrap();

    let config: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("opencode.json")).unwrap()).unwrap();
    assert_eq!(config["model"], "test/model");
    assert_eq!(config["plugins"], json!(["keep-plugin"]));
    assert_eq!(config["command"]["keep"]["template"], "keep me");
    assert!(config["command"].get("oy-review").is_none());
    assert_skills_installed(&env.global_skills());
    let backups = backup_dirs();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        fs::read_to_string(backups[0].join("opencode.json")).unwrap(),
        original
    );
}

#[test]
fn setup_moves_legacy_direct_files_to_backup() {
    let env = TestEnv::new();
    let dir = env.global_opencode();
    let agent = dir.join("agents/oy.md");
    let plugin = dir.join("plugins/oy.js");
    let plugin_agent = dir.join("plugins/assets/agents/oy.md");
    fs::create_dir_all(agent.parent().unwrap()).unwrap();
    fs::create_dir_all(plugin.parent().unwrap()).unwrap();
    fs::create_dir_all(plugin_agent.parent().unwrap()).unwrap();
    fs::write(&agent, "user-owned agent\n").unwrap();
    fs::write(&plugin, "legacy plugin\n").unwrap();
    fs::write(&plugin_agent, "legacy plugin agent\n").unwrap();

    setup_command(false, false, false).unwrap();

    assert!(!agent.exists());
    assert!(!plugin.exists());
    assert!(!plugin_agent.exists());
    let backups = backup_dirs();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        fs::read_to_string(backups[0].join("agents/oy.md")).unwrap(),
        "user-owned agent\n"
    );
    assert_eq!(
        fs::read_to_string(backups[0].join("plugins/oy.js")).unwrap(),
        "legacy plugin\n"
    );
    assert_eq!(
        fs::read_to_string(backups[0].join("plugins/assets/agents/oy.md")).unwrap(),
        "legacy plugin agent\n"
    );
}

#[test]
fn setup_preserves_default_agent_even_when_pointing_at_oy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    let original = r#"{ "default_agent": "oy", "model": "test/model" }"#;
    fs::write(&path, original).unwrap();

    update_config(&path).unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn setup_keeps_an_unrelated_default_agent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    let original = r#"{ "default_agent": "build", "model": "test/model" }"#;
    fs::write(&path, original).unwrap();

    update_config(&path).unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn setup_leaves_unrelated_opencode_config_byte_for_byte() {
    let env = TestEnv::new();
    let dir = env.global_opencode();
    fs::create_dir_all(&dir).unwrap();
    let original = r#"{
  // opencode allows comments and trailing commas.
  "$schema": "https://opencode.ai/config.json",
  "model": "test/model",
  "command": {
    "keep": { "template": "https://example.com//not-a-comment" },
  },
}
"#;
    let path = dir.join("opencode.jsonc");
    fs::write(&path, original).unwrap();

    setup_command(false, false, false).unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    assert!(backup_dirs().is_empty());
}

#[test]
fn skills_complete_detects_drift() {
    let env = TestEnv::new();
    setup_command(false, false, false).unwrap();
    let dir = env.global_skills();
    assert!(skills_complete(&dir));

    fs::write(dir.join("oy-setup/oy-persona.md"), "drifted persona\n").unwrap();
    assert!(!skills_complete(&dir));
}

#[test]
fn setup_remove_round_trip_preserves_unrelated_files() {
    let env = TestEnv::new();
    let dir = env.global_opencode();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("opencode.json"), r#"{ "model": "test/model" }"#).unwrap();
    setup_command(false, false, false).unwrap();
    let user_skill = env.global_skills().join("team-skill/SKILL.md");
    fs::create_dir_all(user_skill.parent().unwrap()).unwrap();
    fs::write(&user_skill, "team-owned skill\n").unwrap();

    setup_command(false, false, true).unwrap();

    assert!(!env.global_skills().join("oy-audit").exists());
    assert!(!env.global_skills().join("oy-review").exists());
    assert!(!env.global_skills().join("oy-enhance").exists());
    assert!(!env.global_skills().join("oy-setup").exists());
    assert_eq!(
        fs::read_to_string(&user_skill).unwrap(),
        "team-owned skill\n"
    );
    let config: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("opencode.json")).unwrap()).unwrap();
    assert_eq!(config["model"], "test/model");
    assert!(!skills_complete(&env.global_skills()));
}

#[test]
fn setup_remove_preserves_user_skill_files_without_the_marker() {
    let env = TestEnv::new();
    setup_command(false, false, false).unwrap();
    let path = env.global_skills().join("oy-audit/SKILL.md");
    fs::write(&path, "user replaced this skill\n").unwrap();

    setup_command(false, false, true).unwrap();

    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "user replaced this skill\n"
    );
    assert!(!env.global_skills().join("oy-review").exists());
}

#[test]
fn config_strip_accepts_object_form_plugin_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    fs::write(
        &path,
        r#"{ "plugins": [{ "package": "@oy-cli/opencode", "options": { "custom": true } }, "keep-plugin"] }"#,
    )
    .unwrap();

    update_config(&path).unwrap();

    let updated: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(updated["plugins"], json!(["keep-plugin"]));
}

#[test]
fn config_strip_preserves_generic_settings_without_oy_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.json");
    let original = r#"{
  "$schema": "https://opencode.ai/config.json",
  "tool_output": { "max_bytes": 262144, "max_lines": 20000, "extra_user_key": true }
}
"#;
    fs::write(&path, original).unwrap();

    update_config(&path).unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn config_strip_rejects_symlinked_files() {
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
fn failed_config_update_restores_files_and_retains_snapshot() {
    let env = TestEnv::new();
    let old_file = env.global_skills().join("oy-audit/SKILL.md");
    fs::create_dir_all(old_file.parent().unwrap()).unwrap();
    fs::write(&old_file, "modified\n").unwrap();
    let invalid_config = env.global_skills().join("blocked");
    fs::create_dir(&invalid_config).unwrap();
    let updates = [ConfigUpdate {
        path: invalid_config.clone(),
        body: "{}\n".to_string(),
        current: Some(b"old config\n".to_vec()),
    }];

    let error = apply_integration_update(
        &[&env.global_skills(), &env.global_opencode()],
        std::slice::from_ref(&old_file),
        &updates,
    )
    .unwrap_err();

    assert!(error.to_string().contains("backup retained"));
    assert_eq!(fs::read_to_string(old_file).unwrap(), "modified\n");
    let backups = backup_dirs();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        fs::read_to_string(backups[0].join("blocked")).unwrap(),
        "old config\n"
    );
}

#[test]
fn persona_and_setup_skill_files_are_canonical_after_setup() {
    let env = TestEnv::new();

    setup_command(false, false, false).unwrap();

    assert_eq!(
        fs::read_to_string(env.global_skills().join("oy-setup/SKILL.md")).unwrap(),
        OY_SETUP_SKILL
    );
    assert_eq!(
        fs::read_to_string(env.global_skills().join("oy-setup/oy-persona.md")).unwrap(),
        OY_PERSONA
    );
    assert!(OY_SETUP_SKILL.contains("oy doctor --check"));
    assert!(!config_has_oy_entries(&json!({ "model": "test/model" })));
    assert!(config_has_oy_entries(
        &json!({ "plugins": ["@oy-cli/opencode@0.1.0"] })
    ));
}

fn seed_plugin_cache(cache: &Path) {
    let oy = cache.join("@oy-cli/opencode@0.14.0/node_modules/oy");
    let fork = cache.join("cursor-opencode-provider@0.3.2/node_modules/fork");
    let fork_scope = cache.join("@stablekernel/opencode-cursor@1.0.0/node_modules/fork");
    let other = cache.join("other-keep/pkg/node_modules/other");
    fs::create_dir_all(oy.parent().unwrap()).unwrap();
    fs::create_dir_all(fork.parent().unwrap()).unwrap();
    fs::create_dir_all(fork_scope.parent().unwrap()).unwrap();
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&oy, "cached plugin\n").unwrap();
    fs::write(&fork, "cached fork\n").unwrap();
    fs::write(&fork_scope, "cached scoped fork\n").unwrap();
    fs::write(&other, "keep\n").unwrap();
}

fn assert_plugin_cache_clean(cache: &Path) {
    assert!(plugin_cache_paths().is_empty());
    assert!(!cache.join("@oy-cli").exists());
    assert!(!cache.join("@stablekernel").exists());
    assert_eq!(
        fs::read_to_string(cache.join("other-keep/pkg/node_modules/other")).unwrap(),
        "keep\n"
    );
}

#[test]
fn setup_deletes_the_obsolete_opencode_plugin_cache() {
    let env = TestEnv::new();
    seed_plugin_cache(&env.plugin_cache());

    setup_command(false, false, false).unwrap();

    assert_plugin_cache_clean(&env.plugin_cache());
    assert_skills_installed(&env.global_skills());
}

#[test]
fn setup_remove_deletes_the_obsolete_opencode_plugin_cache() {
    let env = TestEnv::new();
    setup_command(false, false, false).unwrap();
    seed_plugin_cache(&env.plugin_cache());

    setup_command(false, false, true).unwrap();

    assert_plugin_cache_clean(&env.plugin_cache());
    assert!(!skills_complete(&env.global_skills()));
}

#[test]
fn setup_dry_run_leaves_the_plugin_cache_untouched() {
    let env = TestEnv::new();
    seed_plugin_cache(&env.plugin_cache());

    setup_command(false, true, false).unwrap();

    assert_eq!(plugin_cache_paths().len(), 3);
    assert!(
        env.plugin_cache()
            .join("@oy-cli/opencode@0.14.0/node_modules/oy")
            .exists()
    );
}
