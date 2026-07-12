//! OpenCode package setup, namespace migration, backup, and integration validation.

#[cfg(test)]
use super::OY_AGENT;
use super::{OpenCodeHost, api};
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::fs;
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{config, ui};

const OPENCODE_PLUGIN_PACKAGE: &str = "@oy-cli/opencode";

#[cfg(test)]
thread_local! {
    static TEST_BACKUP_STATE_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[derive(Debug)]
struct SetupOutcome {
    config_path: PathBuf,
    backup: Option<PathBuf>,
}

struct ConfigUpdate {
    path: PathBuf,
    body: String,
    current: Option<Vec<u8>>,
}

impl ConfigUpdate {
    fn new(path: PathBuf, body: String) -> Result<Self> {
        let current = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("failed reading {}", path.display()));
            }
        };
        Ok(Self {
            path,
            body,
            current,
        })
    }

    fn changed(&self) -> bool {
        self.current.as_deref() != Some(self.body.as_bytes())
    }
}

pub(crate) fn setup_command(workspace: bool, dry_run: bool, remove: bool) -> Result<i32> {
    let scope = SetupScope::from_workspace_flag(workspace);
    if remove {
        remove_opencode(scope, dry_run)
    } else {
        setup_opencode(scope, true, dry_run)
    }
}

pub(crate) fn global_config_path() -> Result<PathBuf> {
    Ok(config_path_in(&global_opencode_dir()?))
}

pub(crate) fn workspace_config_path() -> Result<PathBuf> {
    let root = config::oy_root()?;
    Ok(config_path_in(&root.join(".opencode")))
}

fn global_opencode_dir() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("OPENCODE_CONFIG_DIR") {
        if value.is_empty() {
            bail!("OPENCODE_CONFIG_DIR must not be empty");
        }
        let path = PathBuf::from(value);
        return Ok(if path.is_absolute() {
            path
        } else {
            config::oy_root()?.join(path)
        });
    }
    dirs::config_dir()
        .context("failed to find user config directory")
        .map(|dir| dir.join("opencode"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            Self::Global => global_opencode_dir(),
            Self::Workspace => {
                let root = config::oy_root()?;
                Ok(root.join(".opencode"))
            }
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Workspace => "workspace",
        }
    }
}

fn setup_opencode(scope: SetupScope, report: bool, dry_run: bool) -> Result<i32> {
    let dir = scope.dir()?;
    if dry_run {
        return preview_setup(scope, &dir);
    }
    let _lock = SetupLock::acquire(&dir)?;
    let files = legacy_oy_paths(&dir)?;
    let updates = setup_config_updates(&dir)?;
    let changed = !files.is_empty() || updates.iter().any(ConfigUpdate::changed);
    let backup = apply_integration_update(&dir, &files, &updates)?;
    if changed && let Ok(root) = config::oy_root() {
        let host = OpenCodeHost::selected_in(&root);
        if host.supported() {
            let _ = api::OpenCodeApi::new(&host).evict_location(&root);
        }
    }

    if report {
        report_setup(
            "installed",
            scope,
            &SetupOutcome {
                config_path: config_path_in(&dir),
                backup,
            },
        )?;
    }
    Ok(0)
}

fn remove_opencode(scope: SetupScope, dry_run: bool) -> Result<i32> {
    let dir = scope.dir()?;
    let _lock = if dry_run {
        None
    } else {
        Some(SetupLock::acquire(&dir)?)
    };
    let files = legacy_oy_paths(&dir)?;
    let updates = removal_config_updates(&dir)?;
    if dry_run {
        ui::section(format!("{} oy integration removal dry run", scope.label()).as_str());
        for path in &files {
            ui::kv("move", path.display());
        }
        preview_config_updates(&updates);
        return Ok(0);
    }
    let backup = apply_integration_update(&dir, &files, &updates)?;
    report_setup(
        "removed",
        scope,
        &SetupOutcome {
            config_path: config_path_in(&dir),
            backup,
        },
    )?;
    Ok(0)
}

fn report_setup(status: &str, scope: SetupScope, outcome: &SetupOutcome) -> Result<()> {
    if ui::is_json() {
        ui::line(serde_json::to_string_pretty(&json!({
            "status": status,
            "scope": scope.label(),
            "config": outcome.config_path,
            "backup": outcome.backup,
        }))?);
        return Ok(());
    }
    ui::success(format_args!("{status} {} oy integration", scope.label()));
    if let Some(backup) = &outcome.backup {
        ui::line(format_args!(
            "Previous oy integration files were moved to {}.",
            backup.display()
        ));
    }
    if status == "installed" {
        ui::line(format_args!(
            "Restart opencode for {} to install and load.",
            opencode_plugin_spec()
        ));
    }
    Ok(())
}

struct SetupLock {
    path: PathBuf,
}

impl SetupLock {
    fn acquire(dir: &Path) -> Result<Self> {
        let parent = dir.parent().unwrap_or(dir);
        fs::create_dir_all(parent)?;
        let path = parent.join(".oy-opencode-setup.lock");
        for attempt in 0..2 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    use std::io::Write as _;
                    writeln!(file, "{}", std::process::id())?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt == 0 => {
                    if stale_setup_lock(&path) {
                        fs::remove_file(&path)?;
                        continue;
                    }
                    return Err(error).with_context(|| {
                        format!("another oy setup/remove may be running: {}", path.display())
                    });
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed acquiring setup lock: {}", path.display())
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

fn remove_owned_config(path: &Path) -> Result<String> {
    let mut root = read_config(path)?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} must contain a JSON object", path.display()))?;
    remove_oy_config_entries(object)?;
    format_json(&root)
}

fn preview_setup(scope: SetupScope, dir: &Path) -> Result<i32> {
    ui::section(format!("{} oy integration dry run", scope.label()).as_str());
    for path in legacy_oy_paths(dir)? {
        ui::kv("move", path.display());
    }
    preview_config_updates(&setup_config_updates(dir)?);
    Ok(0)
}

fn setup_config_updates(dir: &Path) -> Result<Vec<ConfigUpdate>> {
    let primary = config_path_in(dir);
    let mut updates = vec![ConfigUpdate::new(primary.clone(), config_body(&primary)?)?];
    for path in config_paths_in(dir) {
        if path != primary && path.exists() {
            updates.push(ConfigUpdate::new(
                path.clone(),
                remove_owned_config(&path)?,
            )?);
        }
    }
    Ok(updates)
}

fn removal_config_updates(dir: &Path) -> Result<Vec<ConfigUpdate>> {
    config_paths_in(dir)
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| {
            let body = remove_owned_config(&path)?;
            ConfigUpdate::new(path, body)
        })
        .collect()
}

fn config_paths_in(dir: &Path) -> [PathBuf; 2] {
    [dir.join("opencode.json"), dir.join("opencode.jsonc")]
}

fn preview_config_updates(updates: &[ConfigUpdate]) {
    for update in updates {
        let action = if update.current.is_none() {
            "create"
        } else if update.changed() {
            "backup+update"
        } else {
            "unchanged"
        };
        ui::kv(action, update.path.display());
    }
}

fn legacy_oy_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for namespace in ["agents", "commands", "skills"] {
        let parent = dir.join(namespace);
        let metadata = match fs::symlink_metadata(&parent) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("failed reading {}", parent.display()));
            }
        };
        if metadata.file_type().is_symlink() {
            bail!(
                "refusing to scan symlinked OpenCode namespace {}",
                parent.display()
            );
        }
        if !metadata.is_dir() {
            bail!(
                "OpenCode namespace is not a directory: {}",
                parent.display()
            );
        }
        let entries = fs::read_dir(&parent)
            .with_context(|| format!("failed reading {}", parent.display()))?;
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name == "oy" || name.starts_with("oy-") || name.starts_with("oy.") {
                paths.push(entry.path());
            }
        }
    }
    paths.sort();
    Ok(paths)
}

fn apply_integration_update(
    dir: &Path,
    old_paths: &[PathBuf],
    updates: &[ConfigUpdate],
) -> Result<Option<PathBuf>> {
    let changed = updates
        .iter()
        .filter(|update| update.changed())
        .collect::<Vec<_>>();
    let existing_configs = changed
        .iter()
        .filter(|update| update.current.is_some())
        .collect::<Vec<_>>();
    let backup = if old_paths.is_empty() && existing_configs.is_empty() {
        None
    } else {
        Some(create_backup_dir()?)
    };

    if let Some(backup) = &backup {
        let result = (|| -> Result<()> {
            for update in existing_configs {
                let relative = update.path.strip_prefix(dir).with_context(|| {
                    format!("{} is outside {}", update.path.display(), dir.display())
                })?;
                let destination = backup.join(relative);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(
                    &destination,
                    update.current.as_deref().expect("existing config bytes"),
                )
                .with_context(|| format!("failed backing up {}", update.path.display()))?;
                let permissions = fs::metadata(&update.path)?.permissions();
                fs::set_permissions(&destination, permissions).with_context(|| {
                    format!(
                        "failed preserving permissions for {}",
                        destination.display()
                    )
                })?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_dir_all(backup);
            return Err(error);
        }
    }

    let mut moved = Vec::new();
    if let Some(backup) = &backup {
        for source in old_paths {
            let relative = source
                .strip_prefix(dir)
                .with_context(|| format!("{} is outside {}", source.display(), dir.display()))?;
            let destination = backup.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            if let Err(error) = move_path(source, &destination) {
                if let Err(rollback) = restore_moved_paths(&moved) {
                    return Err(error).context(format!(
                        "failed moving {} to {}; rollback also failed: {rollback:#}",
                        source.display(),
                        destination.display()
                    ));
                }
                return Err(error).with_context(|| {
                    format!(
                        "failed moving {} to {}",
                        source.display(),
                        destination.display()
                    )
                });
            }
            moved.push((source.clone(), destination));
        }
    }

    let mutations = changed
        .iter()
        .map(|update| crate::config::FileMutation::Write {
            path: update.path.as_path(),
            bytes: update.body.as_bytes(),
        })
        .collect::<Vec<_>>();
    if let Err(error) = crate::config::apply_file_batch_in(dir, &mutations) {
        if let Err(rollback) = restore_moved_paths(&moved) {
            return Err(error).context(format!(
                "setup rollback failed; backup retained at {}: {rollback:#}",
                backup
                    .as_ref()
                    .map_or_else(|| Path::new("<none>"), PathBuf::as_path)
                    .display()
            ));
        }
        if let Some(backup) = &backup {
            return Err(error).context(format!(
                "config update failed; backup retained at {}",
                backup.display()
            ));
        }
        return Err(error);
    }

    for namespace in ["agents", "commands", "skills"] {
        let _ = fs::remove_dir(dir.join(namespace));
    }
    Ok(backup)
}

fn create_backup_dir() -> Result<PathBuf> {
    let state = backup_state_dir()?;
    fs::create_dir_all(&state)
        .with_context(|| format!("failed creating state directory {}", state.display()))?;
    let oy_state = state.join("oy");
    create_private_backup_directory(&oy_state)?;
    let base = oy_state.join("backups");
    create_private_backup_directory(&base)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_nanos();
    for suffix in 0..100 {
        let path = base.join(format!(
            "opencode-{}-{timestamp}-{}-{suffix}",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => {
                restrict_backup_directory(&path)?;
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
        "failed to allocate a unique backup directory in {}",
        base.display()
    )
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

fn create_private_backup_directory(path: &Path) -> Result<()> {
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
    restrict_backup_directory(path)
}

fn move_path(source: &Path, destination: &Path) -> Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() != std::io::ErrorKind::CrossesDevices => {
            return Err(error).with_context(|| {
                format!(
                    "failed moving {} to {}",
                    source.display(),
                    destination.display()
                )
            });
        }
        Err(_) => {}
    }
    if let Err(error) = copy_path(source, destination) {
        let _ = remove_path(destination);
        return Err(error);
    }
    if let Err(error) = remove_path(source) {
        return Err(error).with_context(|| {
            format!(
                "failed removing {}; complete copy retained at {}",
                source.display(),
                destination.display()
            )
        });
    }
    Ok(())
}

fn copy_path(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed reading {}", source.display()))?;
    if metadata.file_type().is_symlink() {
        use std::os::unix::fs::symlink;
        symlink(fs::read_link(source)?, destination).with_context(|| {
            format!(
                "failed copying symlink {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        return Ok(());
    }
    if metadata.is_file() {
        fs::copy(source, destination).with_context(|| {
            format!(
                "failed copying {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        fs::set_permissions(destination, metadata.permissions())?;
        return Ok(());
    }
    if metadata.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::set_permissions(destination, metadata.permissions())?;
        return Ok(());
    }
    bail!("unsupported oy backup file type: {}", source.display())
}

fn remove_path(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn restrict_backup_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed securing backup directory {}", path.display()))?;
    Ok(())
}

fn restore_moved_paths(moved: &[(PathBuf, PathBuf)]) -> Result<()> {
    let mut first_error = None;
    for (source, backup) in moved.iter().rev() {
        let result = (|| -> Result<()> {
            if let Some(parent) = source.parent() {
                fs::create_dir_all(parent)?;
            }
            move_path(backup, source).with_context(|| {
                format!(
                    "failed restoring {} from {}",
                    source.display(),
                    backup.display()
                )
            })
        })();
        if let Err(error) = result
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn config_path_in(dir: &Path) -> PathBuf {
    let jsonc = dir.join("opencode.jsonc");
    if jsonc.exists() {
        jsonc
    } else {
        dir.join("opencode.json")
    }
}

pub(super) fn ensure_opencode_integration() -> Result<()> {
    let global = SetupScope::Global.dir()?;
    let workspace = SetupScope::Workspace.dir()?;
    if [&global, &workspace]
        .iter()
        .any(|dir| integration_complete(dir))
    {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() || ui::is_json() {
        bail!(
            "oy integration is missing or incomplete; run `oy setup` (or `oy setup --workspace`) first"
        );
    }
    let scope = if integration_present(&workspace)? {
        SetupScope::Workspace
    } else {
        SetupScope::Global
    };
    ui::out(&format!(
        "oy is not set up. Set up the {} integration now? [Y/n] ",
        scope.label()
    ));
    std::io::stdout().flush()?;
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer)? == 0 {
        bail!("oy setup was cancelled; run `oy setup` when ready");
    }
    if !setup_answer_is_yes(&answer) {
        bail!("oy setup was declined; run `oy setup` when ready");
    }
    setup_opencode(scope, true, false)?;
    if integration_complete(&scope.dir()?) {
        return Ok(());
    }
    bail!("oy setup completed, but the integration is still incomplete; run `oy doctor --check`")
}

fn setup_answer_is_yes(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

fn integration_present(dir: &Path) -> Result<bool> {
    if !legacy_oy_paths(dir)?.is_empty() {
        return Ok(true);
    }
    Ok(config_paths_in(dir).iter().any(|path| {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| parse_opencode_config(&text).ok())
            .is_some_and(|config| config_has_oy_entries(&config))
    }))
}

fn integration_complete(dir: &Path) -> bool {
    config_has_all_oy_entries(&config_path_in(dir))
}

fn config_body(path: &Path) -> Result<String> {
    let mut root = read_config(path)?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} must contain a JSON object", path.display()))?;
    remove_oy_config_entries(object)?;
    merge_plugin(object)?;
    format_json(&root)
}

fn read_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!(
            "refusing to read symlinked OpenCode config {}",
            path.display()
        );
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    parse_opencode_config(&text).with_context(|| {
        format!(
            "{} must be valid opencode JSON/JSONC for oy setup to update it",
            path.display()
        )
    })
}

#[cfg(test)]
fn update_config(path: &Path) -> Result<()> {
    let body = config_body(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, body)?;
    Ok(())
}

fn remove_oy_config_entries(object: &mut Map<String, Value>) -> Result<()> {
    remove_owned_plugins(object)?;
    for key in ["command", "commands"] {
        let remove = object
            .get_mut(key)
            .and_then(Value::as_object_mut)
            .is_some_and(|entries| {
                entries.retain(|name, _| !is_oy_name(name));
                entries.is_empty()
            });
        if remove {
            object.remove(key);
        }
    }
    if let Some(mcp) = object.get_mut("mcp").and_then(Value::as_object_mut) {
        mcp.remove("oy");
        if let Some(servers) = mcp.get_mut("servers").and_then(Value::as_object_mut) {
            servers.remove("oy");
            if servers.is_empty() {
                mcp.remove("servers");
            }
        }
        if mcp.is_empty() {
            object.remove("mcp");
        }
    }
    Ok(())
}

fn is_oy_name(name: &str) -> bool {
    name == "oy" || name.starts_with("oy-")
}

fn opencode_plugin_spec() -> String {
    format!("{OPENCODE_PLUGIN_PACKAGE}@{}", env!("CARGO_PKG_VERSION"))
}

fn is_oy_plugin_spec(value: &str) -> bool {
    value == OPENCODE_PLUGIN_PACKAGE
        || value
            .strip_prefix(OPENCODE_PLUGIN_PACKAGE)
            .is_some_and(|suffix| suffix.starts_with('@') && suffix.len() > 1)
}

fn is_oy_plugin_value(value: &Value) -> bool {
    value.as_str().is_some_and(is_oy_plugin_spec)
        || value
            .get("package")
            .and_then(Value::as_str)
            .is_some_and(is_oy_plugin_spec)
}

fn remove_owned_plugins(object: &mut Map<String, Value>) -> Result<()> {
    let Some(plugins) = object.get_mut("plugins") else {
        return Ok(());
    };
    let Some(plugins) = plugins.as_array_mut() else {
        bail!("native OpenCode `plugins` must be an array");
    };
    plugins.retain(|plugin| !is_oy_plugin_value(plugin));
    if plugins.is_empty() {
        object.remove("plugins");
    }
    Ok(())
}

fn merge_plugin(object: &mut Map<String, Value>) -> Result<()> {
    remove_owned_plugins(object)?;
    let plugins = object
        .entry("plugins")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(plugins) = plugins.as_array_mut() else {
        bail!("native OpenCode `plugins` must be an array");
    };
    plugins.push(Value::String(opencode_plugin_spec()));
    Ok(())
}

fn config_has_oy_entries(config: &Value) -> bool {
    config
        .get("plugins")
        .and_then(Value::as_array)
        .is_some_and(|plugins| plugins.iter().any(is_oy_plugin_value))
        || ["command", "commands"].iter().any(|key| {
            config
                .get(*key)
                .and_then(Value::as_object)
                .is_some_and(|entries| entries.keys().any(|name| is_oy_name(name)))
        })
        || config
            .get("mcp")
            .and_then(Value::as_object)
            .is_some_and(|mcp| {
                mcp.contains_key("oy")
                    || mcp
                        .get("servers")
                        .and_then(Value::as_object)
                        .is_some_and(|servers| servers.contains_key("oy"))
            })
}

fn config_has_all_oy_entries(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(config) = parse_opencode_config(&text) else {
        return false;
    };
    config
        .get("plugins")
        .and_then(Value::as_array)
        .is_some_and(|plugins| {
            plugins
                .iter()
                .any(|plugin| plugin.as_str() == Some(opencode_plugin_spec().as_str()))
        })
}

fn format_json(value: &Value) -> Result<String> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    Ok(text)
}

fn parse_opencode_config(text: &str) -> Result<Value> {
    Ok(serde_json::from_str::<Value>(text)
        .or_else(|_| serde_json::from_str::<Value>(&strip_jsonc(text)))?)
}

fn strip_jsonc(text: &str) -> String {
    let mut without_comments = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            without_comments.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            without_comments.push(ch);
            continue;
        }
        if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            without_comments.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for next in chars.by_ref() {
                        if previous == '*' && next == '/' {
                            break;
                        }
                        if next == '\n' {
                            without_comments.push('\n');
                        }
                        previous = next;
                    }
                }
                _ => without_comments.push(ch),
            }
            continue;
        }
        without_comments.push(ch);
    }

    remove_trailing_commas(&without_comments)
}

fn remove_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars = text.chars().collect::<Vec<_>>();
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in chars.iter().copied().enumerate() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == ',' {
            let next = chars[idx + 1..]
                .iter()
                .copied()
                .find(|next| !next.is_whitespace());
            if matches!(next, Some('}' | ']')) {
                continue;
            }
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests;
