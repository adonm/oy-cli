//! Direct installation of oy's Cursor rule, subagent, and skills.

use super::{OY_AGENT, OY_AUDIT_SKILL, OY_ENHANCE_SKILL, OY_REVIEW_SKILL, OY_RULE};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{config, ui};

const ASSETS: [Asset; 5] = [
    Asset::new("rules/oy.mdc", OY_RULE),
    Asset::new("agents/oy.md", OY_AGENT),
    Asset::new("skills/oy-audit/SKILL.md", OY_AUDIT_SKILL),
    Asset::new("skills/oy-review/SKILL.md", OY_REVIEW_SKILL),
    Asset::new("skills/oy-enhance/SKILL.md", OY_ENHANCE_SKILL),
];

#[derive(Clone, Copy)]
struct Asset {
    relative: &'static str,
    body: &'static str,
}

impl Asset {
    const fn new(relative: &'static str, body: &'static str) -> Self {
        Self { relative, body }
    }
}

struct AssetUpdate {
    path: PathBuf,
    body: &'static str,
    current: Option<Vec<u8>>,
}

impl AssetUpdate {
    fn changed(&self) -> bool {
        self.current.as_deref() != Some(self.body.as_bytes())
    }
}

#[derive(Clone, Copy)]
enum SetupScope {
    Global,
    Workspace,
}

impl SetupScope {
    fn from_workspace_flag(workspace: bool) -> Self {
        if workspace {
            Self::Workspace
        } else {
            Self::Global
        }
    }

    fn dir(self) -> Result<PathBuf> {
        match self {
            Self::Global => dirs::home_dir()
                .context("failed to find home directory")
                .map(|home| cursor_dir_in_home(&home)),
            Self::Workspace => Ok(config::oy_root()?.join(".cursor")),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Workspace => "workspace",
        }
    }
}

fn cursor_dir_in_home(home: &Path) -> PathBuf {
    home.join(".cursor")
}

pub(crate) fn setup_command(workspace: bool, dry_run: bool, remove: bool) -> Result<i32> {
    let scope = SetupScope::from_workspace_flag(workspace);
    let dir = scope.dir()?;
    setup_in(scope, &dir, dry_run, remove)
}

fn setup_in(scope: SetupScope, dir: &Path, dry_run: bool, remove: bool) -> Result<i32> {
    if dry_run {
        return preview(scope, dir, remove);
    }

    let _lock = SetupLock::acquire(dir)?;
    let updates = inspect_assets(dir)?;
    if remove {
        remove_assets(scope, dir, &updates)
    } else {
        install_assets(scope, dir, &updates)
    }
}

fn install_assets(scope: SetupScope, dir: &Path, updates: &[AssetUpdate]) -> Result<i32> {
    let changed = updates
        .iter()
        .filter(|update| update.changed())
        .collect::<Vec<_>>();
    let backup = backup_existing(dir, &changed)?;
    let mutations = changed
        .iter()
        .map(|update| config::FileMutation::Write {
            path: update.path.as_path(),
            bytes: update.body.as_bytes(),
        })
        .collect::<Vec<_>>();
    if let Err(error) = config::apply_file_batch_in(dir, &mutations) {
        return Err(retain_backup_context(error, backup.as_deref()));
    }
    report("installed", scope, dir, backup.as_deref())?;
    Ok(0)
}

fn remove_assets(scope: SetupScope, dir: &Path, updates: &[AssetUpdate]) -> Result<i32> {
    let existing = updates
        .iter()
        .filter(|update| update.current.is_some())
        .collect::<Vec<_>>();
    let backup = backup_existing(dir, &existing)?;
    let mut removed = Vec::new();
    for update in existing {
        if let Err(error) = fs::remove_file(&update.path) {
            if let Err(rollback) = restore_removed(dir, &removed) {
                return Err(error).context(format!(
                    "failed removing {}; rollback also failed: {rollback:#}",
                    update.path.display()
                ));
            }
            return Err(retain_backup_context(
                anyhow::Error::new(error)
                    .context(format!("failed removing {}", update.path.display())),
                backup.as_deref(),
            ));
        }
        removed.push(update);
    }
    remove_empty_owned_dirs(dir);
    report("removed", scope, dir, backup.as_deref())?;
    Ok(0)
}

fn restore_removed(dir: &Path, removed: &[&AssetUpdate]) -> Result<()> {
    let mutations = removed
        .iter()
        .map(|update| config::FileMutation::Write {
            path: update.path.as_path(),
            bytes: update.current.as_deref().expect("removed asset bytes"),
        })
        .collect::<Vec<_>>();
    config::apply_file_batch_in(dir, &mutations)
}

fn inspect_assets(dir: &Path) -> Result<Vec<AssetUpdate>> {
    ASSETS
        .iter()
        .map(|asset| {
            let path = dir.join(asset.relative);
            reject_symlink_ancestors(dir, &path)?;
            let current = match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    bail!(
                        "refusing to replace symlinked Cursor asset {}",
                        path.display()
                    )
                }
                Ok(metadata) if !metadata.is_file() => {
                    bail!("Cursor asset path is not a file: {}", path.display())
                }
                Ok(_) => Some(
                    fs::read(&path)
                        .with_context(|| format!("failed reading {}", path.display()))?,
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed reading {}", path.display()));
                }
            };
            Ok(AssetUpdate {
                path,
                body: asset.body,
                current,
            })
        })
        .collect()
}

fn reject_symlink_ancestors(root: &Path, path: &Path) -> Result<()> {
    for ancestor in path.parent().unwrap_or(root).ancestors() {
        if ancestor == root {
            break;
        }
        if fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            bail!(
                "refusing to use symlinked Cursor namespace {}",
                ancestor.display()
            );
        }
    }
    Ok(())
}

fn preview(scope: SetupScope, dir: &Path, remove: bool) -> Result<i32> {
    let updates = inspect_assets(dir)?;
    ui::section(
        format!(
            "{} Cursor oy integration {} dry run",
            scope.label(),
            if remove { "removal" } else { "setup" }
        )
        .as_str(),
    );
    for update in updates {
        let action = if remove {
            if update.current.is_some() {
                "backup+remove"
            } else {
                "absent"
            }
        } else if update.current.is_none() {
            "create"
        } else if update.changed() {
            "backup+update"
        } else {
            "unchanged"
        };
        ui::kv(action, update.path.display());
    }
    Ok(0)
}

fn report(status: &str, scope: SetupScope, dir: &Path, backup: Option<&Path>) -> Result<()> {
    if ui::is_json() {
        ui::line(serde_json::to_string_pretty(&json!({
            "status": status,
            "integration": "cursor",
            "scope": scope.label(),
            "directory": dir,
            "files": ASSETS
                .iter()
                .map(|asset| dir.join(asset.relative))
                .collect::<Vec<_>>(),
            "backup": backup,
        }))?);
        return Ok(());
    }
    ui::success(format_args!(
        "{status} {} Cursor oy integration",
        scope.label()
    ));
    if let Some(backup) = backup {
        ui::line(format_args!(
            "Previous Cursor oy files were copied to {}.",
            backup.display()
        ));
    }
    if status == "installed" {
        ui::line("Start a new Cursor Agent chat to load the oy rule, subagent, and skills.");
    }
    Ok(())
}

fn retain_backup_context(error: anyhow::Error, backup: Option<&Path>) -> anyhow::Error {
    if let Some(backup) = backup {
        error.context(format!(
            "Cursor integration backup retained at {}",
            backup.display()
        ))
    } else {
        error
    }
}

fn backup_existing(dir: &Path, updates: &[&AssetUpdate]) -> Result<Option<PathBuf>> {
    let existing = updates
        .iter()
        .copied()
        .filter(|update| update.current.is_some())
        .collect::<Vec<_>>();
    if existing.is_empty() {
        return Ok(None);
    }
    let backup = create_backup_dir()?;
    let result = (|| -> Result<()> {
        for update in existing {
            let relative = update.path.strip_prefix(dir).with_context(|| {
                format!("{} is outside {}", update.path.display(), dir.display())
            })?;
            let destination = backup.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(
                &destination,
                update.current.as_deref().expect("existing asset bytes"),
            )
            .with_context(|| format!("failed backing up {}", update.path.display()))?;
            fs::set_permissions(&destination, fs::metadata(&update.path)?.permissions())
                .with_context(|| format!("failed preserving {}", destination.display()))?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&backup);
        return Err(error);
    }
    Ok(Some(backup))
}

#[cfg(test)]
thread_local! {
    static TEST_BACKUP_STATE_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn backup_state_dir() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_BACKUP_STATE_DIR.with(|state| state.borrow().clone()) {
        return Ok(path);
    }
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .context("failed to find a user state directory for oy backups")
}

fn create_backup_dir() -> Result<PathBuf> {
    let state = backup_state_dir()?;
    fs::create_dir_all(&state)
        .with_context(|| format!("failed creating state directory {}", state.display()))?;
    let oy_state = state.join("oy");
    create_private_dir(&oy_state)?;
    let base = oy_state.join("backups");
    create_private_dir(&base)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_nanos();
    for suffix in 0..100 {
        let path = base.join(format!(
            "cursor-{}-{timestamp}-{}-{suffix}",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => {
                restrict_private_dir(&path)?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed creating backup {}", path.display()));
            }
        }
    }
    bail!(
        "failed to allocate a unique Cursor backup directory in {}",
        base.display()
    )
}

fn create_private_dir(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!(
            "refusing to use symlinked backup directory {}",
            path.display()
        );
    }
    if path.exists() {
        if !fs::metadata(path)?.is_dir() {
            bail!("backup path is not a directory: {}", path.display());
        }
    } else {
        fs::create_dir(path)
            .with_context(|| format!("failed creating backup directory {}", path.display()))?;
    }
    restrict_private_dir(path)
}

fn restrict_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed securing backup directory {}", path.display()))
}

fn remove_empty_owned_dirs(dir: &Path) {
    for relative in [
        "skills/oy-audit",
        "skills/oy-review",
        "skills/oy-enhance",
        "skills",
        "agents",
        "rules",
    ] {
        let _ = fs::remove_dir(dir.join(relative));
    }
}

struct SetupLock {
    path: PathBuf,
}

impl SetupLock {
    fn acquire(dir: &Path) -> Result<Self> {
        let parent = dir.parent().unwrap_or(dir);
        fs::create_dir_all(parent)?;
        let path = parent.join(".oy-cursor-setup.lock");
        for attempt in 0..2 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt == 0 => {
                    if stale_setup_lock(&path) {
                        fs::remove_file(&path)?;
                        continue;
                    }
                    return Err(error).with_context(|| {
                        format!(
                            "another Cursor oy setup/remove may be running: {}",
                            path.display()
                        )
                    });
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed acquiring Cursor setup lock: {}", path.display())
                    });
                }
            }
        }
        unreachable!("setup lock loop always returns")
    }
}

fn stale_setup_lock(path: &Path) -> bool {
    let pid = fs::read_to_string(path)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    let Some(pid) = pid else {
        return fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > std::time::Duration::from_secs(30));
    };
    let result = unsafe { libc::kill(pid as i32, 0) };
    result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

impl Drop for SetupLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests;
