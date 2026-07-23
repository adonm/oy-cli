use super::*;

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

fn backup_dirs(state: &Path) -> Vec<PathBuf> {
    let mut backups = fs::read_dir(state.join("oy/backups"))
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    backups.sort();
    backups
}

fn setup_workspace() -> (tempfile::TempDir, tempfile::TempDir, BackupStateGuard) {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let backup_state = BackupStateGuard::set(state.path().to_path_buf());
    (workspace, state, backup_state)
}

fn setup_cursor(workspace: &Path, dry_run: bool, remove: bool) -> Result<i32> {
    setup_in(
        SetupScope::Workspace,
        &workspace.join(".cursor"),
        dry_run,
        remove,
    )
}

#[test]
fn global_cursor_directory_is_under_home() {
    assert_eq!(
        cursor_dir_in_home(Path::new("/home/example")),
        PathBuf::from("/home/example/.cursor")
    );
}

#[test]
fn workspace_setup_installs_cursor_rule_agent_and_skills() {
    let (workspace, _state, _backup_state) = setup_workspace();

    setup_cursor(workspace.path(), false, false).unwrap();

    let cursor = workspace.path().join(".cursor");
    assert_eq!(
        fs::read_to_string(cursor.join("rules/oy.mdc")).unwrap(),
        OY_RULE
    );
    assert_eq!(
        fs::read_to_string(cursor.join("agents/oy.md")).unwrap(),
        OY_AGENT
    );
    assert_eq!(
        fs::read_to_string(cursor.join("skills/oy-audit/SKILL.md")).unwrap(),
        OY_AUDIT_SKILL
    );
    assert_eq!(
        fs::read_to_string(cursor.join("skills/oy-review/SKILL.md")).unwrap(),
        OY_REVIEW_SKILL
    );
    assert_eq!(
        fs::read_to_string(cursor.join("skills/oy-enhance/SKILL.md")).unwrap(),
        OY_ENHANCE_SKILL
    );
}

#[test]
fn cursor_setup_is_idempotent() {
    let (workspace, state, _backup_state) = setup_workspace();

    setup_cursor(workspace.path(), false, false).unwrap();
    let first = fs::read(workspace.path().join(".cursor/agents/oy.md")).unwrap();
    setup_cursor(workspace.path(), false, false).unwrap();

    assert_eq!(
        fs::read(workspace.path().join(".cursor/agents/oy.md")).unwrap(),
        first
    );
    assert!(backup_dirs(state.path()).is_empty());
}

#[test]
fn cursor_setup_backs_up_modified_owned_files() {
    let (workspace, state, _backup_state) = setup_workspace();
    setup_cursor(workspace.path(), false, false).unwrap();
    let agent = workspace.path().join(".cursor/agents/oy.md");
    fs::write(&agent, "locally modified\n").unwrap();

    setup_cursor(workspace.path(), false, false).unwrap();

    assert_eq!(fs::read_to_string(agent).unwrap(), OY_AGENT);
    let backups = backup_dirs(state.path());
    assert_eq!(backups.len(), 1);
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&backups[0]).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert_eq!(
        fs::read_to_string(backups[0].join("agents/oy.md")).unwrap(),
        "locally modified\n"
    );
}

#[test]
fn cursor_setup_dry_run_does_not_write() {
    let (workspace, _state, _backup_state) = setup_workspace();

    setup_cursor(workspace.path(), true, false).unwrap();

    assert!(!workspace.path().join(".cursor").exists());
}

#[test]
fn cursor_remove_backs_up_owned_files_and_preserves_unrelated_files() {
    let (workspace, state, _backup_state) = setup_workspace();
    setup_cursor(workspace.path(), false, false).unwrap();
    let unrelated = workspace.path().join(".cursor/skills/keep/SKILL.md");
    fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
    fs::write(&unrelated, "keep\n").unwrap();

    setup_cursor(workspace.path(), false, true).unwrap();

    assert_eq!(fs::read_to_string(unrelated).unwrap(), "keep\n");
    assert!(!workspace.path().join(".cursor/agents/oy.md").exists());
    assert!(!workspace.path().join(".cursor/rules/oy.mdc").exists());
    let backups = backup_dirs(state.path());
    assert_eq!(backups.len(), 1);
    assert_eq!(
        fs::read_to_string(backups[0].join("skills/oy-review/SKILL.md")).unwrap(),
        OY_REVIEW_SKILL
    );
}

#[test]
fn cursor_setup_rejects_symlinked_namespaces() {
    use std::os::unix::fs::symlink;

    let (workspace, _state, _backup_state) = setup_workspace();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(workspace.path().join(".cursor")).unwrap();
    symlink(outside.path(), workspace.path().join(".cursor/skills")).unwrap();

    let error = setup_cursor(workspace.path(), false, false).unwrap_err();

    assert!(error.to_string().contains("symlinked Cursor namespace"));
    assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
}
