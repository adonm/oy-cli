//! OpenCode package setup, namespace migration, backup, and integration validation.

mod backup;
mod config_file;

use super::{OpenCodeHost, api};
use anyhow::{Context, Result, bail};
use backup::{create_backup_dir, move_path, restore_moved_paths};
use config_file::{
    config_has_oy_entries, opencode_plugin_package, parse_opencode_config, strip_owned_config,
};
use serde_json::json;
use std::fs;
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

use crate::{config, ui};

use super::{OPENCODE_PLUGIN_JS, OY_AGENT, OY_AUDIT_SKILL, OY_ENHANCE_SKILL, OY_REVIEW_SKILL};

/// OpenCode 2 auto-discovery loads only direct `.js`/`.ts` children of the
/// global `plugins/` directory, so the plugin is a single entrypoint file
/// with assets in a sibling directory, mirroring the npm package layout.
const PLUGIN_ENTRYPOINT: &str = "plugins/oy.js";
const PLUGIN_ASSETS: [(&str, &str); 4] = [
    ("plugins/assets/agents/oy.md", OY_AGENT),
    ("plugins/assets/skills/oy-audit/SKILL.md", OY_AUDIT_SKILL),
    ("plugins/assets/skills/oy-review/SKILL.md", OY_REVIEW_SKILL),
    (
        "plugins/assets/skills/oy-enhance/SKILL.md",
        OY_ENHANCE_SKILL,
    ),
];

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
    let old_paths = setup_owned_paths(&dir)?;
    let updates = strip_oy_config_updates(&dir)?;
    let changed = !old_paths.is_empty() || updates.iter().any(ConfigUpdate::changed);
    let backup = apply_integration_update(&dir, &old_paths, &updates)?;
    install_plugin_files(&dir)?;
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
    let old_paths = remove_owned_paths(&dir)?;
    let updates = strip_oy_config_updates(&dir)?;
    if dry_run {
        ui::section(format!("{} oy integration removal dry run", scope.label()).as_str());
        for path in &old_paths {
            ui::kv("move", path.display());
        }
        preview_config_updates(&updates);
        return Ok(0);
    }
    let backup = apply_integration_update(&dir, &old_paths, &updates)?;
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

/// Paths owned by the current oy integration: legacy namespace files, plus the
/// plugin entrypoint and assets when their content no longer matches this build.
fn setup_owned_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = legacy_oy_paths(dir)?;
    if !plugin_files_match(dir)? {
        paths.extend(plugin_paths(dir).into_iter().filter(|path| path.exists()));
    }
    Ok(paths)
}

/// Everything the integration owns, including a current plugin installation.
fn remove_owned_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = legacy_oy_paths(dir)?;
    paths.extend(plugin_paths(dir).into_iter().filter(|path| path.exists()));
    Ok(paths)
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
            "Restart opencode to load the {} plugin.",
            opencode_plugin_package()
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

fn preview_setup(scope: SetupScope, dir: &Path) -> Result<i32> {
    ui::section(format!("{} oy integration dry run", scope.label()).as_str());
    for path in setup_owned_paths(dir)? {
        ui::kv("move", path.display());
    }
    if !plugin_files_match(dir)? {
        for path in plugin_paths(dir).into_iter().filter(|path| !path.exists()) {
            ui::kv("create", path.display());
        }
    }
    preview_config_updates(&strip_oy_config_updates(dir)?);
    Ok(0)
}

fn strip_oy_config_updates(dir: &Path) -> Result<Vec<ConfigUpdate>> {
    let mut updates = Vec::new();
    for path in config_paths_in(dir) {
        if !path.exists() {
            continue;
        }
        let Some(body) = strip_owned_config(&path)? else {
            continue;
        };
        updates.push(ConfigUpdate::new(path, body)?);
    }
    Ok(updates)
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

    for namespace in ["agents", "commands", "plugins", "skills"] {
        let _ = fs::remove_dir(dir.join(namespace));
    }
    Ok(backup)
}

/// Plugin files shipped by this build, relative to the OpenCode config
/// directory, paired with their content.
fn plugin_files(dir: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut files = vec![(dir.join(PLUGIN_ENTRYPOINT), OPENCODE_PLUGIN_JS)];
    files.extend(
        PLUGIN_ASSETS
            .iter()
            .map(|(relative, content)| (dir.join(relative), *content)),
    );
    files
}

/// Plugin file paths shipped by this build, relative to the OpenCode config
/// directory.
fn plugin_paths(dir: &Path) -> Vec<PathBuf> {
    plugin_files(dir)
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

/// Whether the installed plugin matches this build exactly.
fn plugin_files_match(dir: &Path) -> Result<bool> {
    Ok(plugin_files(dir).iter().all(|(path, content)| {
        fs::read_to_string(path).is_ok_and(|installed| installed == *content)
    }))
}

/// Write the plugin files, skipping when they already match this build.
fn install_plugin_files(dir: &Path) -> Result<()> {
    if plugin_files_match(dir)? {
        return Ok(());
    }
    let files = plugin_files(dir);
    let mutations = files
        .iter()
        .map(|(path, content)| config::FileMutation::Write {
            path,
            bytes: content.as_bytes(),
        })
        .collect::<Vec<_>>();
    config::apply_file_batch_in(dir, &mutations)
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
    if !legacy_oy_paths(dir)?.is_empty() || plugin_files(dir).iter().any(|(path, _)| path.exists())
    {
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
    plugin_files_match(dir).unwrap_or(false)
}

#[cfg(test)]
mod tests;
