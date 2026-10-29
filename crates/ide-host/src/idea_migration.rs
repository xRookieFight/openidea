//! One-time move from the old `.openidea` folder into IntelliJ's `.idea`. Runs
//! when a project opens; every piece goes through the same writers the IDE
//! uses, so the result is what saving it in OpenIDEA would have produced.
//! Where IntelliJ already has something (a run configuration with the same
//! name, its own changelists), IntelliJ's version stays.

use ide_core::{IdeResult, project_dir};
use ide_vcs::ChangelistState;
use std::path::{Path, PathBuf};

/// Where the leftovers of `.openidea` end up once everything is converted.
const PARKED: &str = "migrated-openidea";

/// Migrates `<root>/.openidea` into `<root>/.idea`; `Ok(false)` when there's
/// nothing to migrate. On error the old folder stays, so the next open retries.
pub fn migrate(root: &Path) -> IdeResult<bool> {
    let legacy = root.join(project_dir::DIR);
    if !legacy.is_dir() {
        return Ok(false);
    }
    settings(root)?;
    run_configs(root)?;
    changelists(root)?;
    ide_vcs::shelf::import_legacy(&legacy.join("shelf"), &project_dir::idea_path(root, "shelf"))?;
    data_sources(root)?;
    move_if_free(&legacy.join("plugins"), &project_dir::path(root, "plugins"))?;
    let parked = free_path(&project_dir::path(root, PARKED));
    if let Some(dir) = parked.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::rename(&legacy, &parked)?;
    project_dir::ensure_gitignore(root)?;
    Ok(true)
}

/// The project settings layer moves as is; a sidecar already there wins.
fn settings(root: &Path) -> IdeResult<()> {
    let from = project_dir::legacy_path(root, "settings.toml");
    let to = project_dir::path(root, "settings.toml");
    if from.is_file() && !to.exists() {
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::copy(&from, &to)?;
    }
    Ok(())
}

fn run_configs(root: &Path) -> IdeResult<()> {
    let legacy = ide_run::legacy_user_configs(root)?;
    if !legacy.is_empty() {
        let mut current = ide_run::load_user_configs(root)?;
        // hmm, the same configuration made in both IDEs: keep IntelliJ's
        let fresh: Vec<_> = legacy
            .into_iter()
            .filter(|c| !current.iter().any(|x| x.id == c.id || (x.kind == c.kind && x.name == c.name)))
            .collect();
        if !fresh.is_empty() {
            current.extend(fresh);
            ide_run::save_user_configs(root, &current)?;
        }
    }
    let legacy = ide_run::legacy_templates(root)?;
    if !legacy.is_empty() {
        let mut current = ide_run::load_templates(root)?;
        let fresh: Vec<_> = legacy.into_iter().filter(|t| !current.iter().any(|x| x.kind == t.kind)).collect();
        if !fresh.is_empty() {
            current.extend(fresh);
            ide_run::save_templates(root, &current)?;
        }
    }
    Ok(())
}

fn changelists(root: &Path) -> IdeResult<()> {
    let Some(legacy) = ide_vcs::changelists::legacy_load(&project_dir::legacy_path(root, "changelists.json")) else {
        return Ok(());
    };
    if ide_vcs::changelists::load(root) == ChangelistState::initial() {
        ide_vcs::changelists::save(root, &legacy)?;
    }
    Ok(())
}

fn data_sources(root: &Path) -> IdeResult<()> {
    let legacy = ide_db::source::legacy_load(root)?;
    if legacy.is_empty() {
        return Ok(());
    }
    let mut current = ide_db::source::load(root)?;
    let fresh: Vec<_> =
        legacy.into_iter().filter(|s| !current.iter().any(|x| x.id == s.id || x.name == s.name)).collect();
    if !fresh.is_empty() {
        current.extend(fresh);
        ide_db::source::save(root, &current)?;
    }
    Ok(())
}

fn move_if_free(from: &Path, to: &Path) -> IdeResult<()> {
    if !from.exists() || to.exists() {
        return Ok(());
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::rename(from, to)?;
    Ok(())
}

/// `path`, or `path-2`, `path-3`… when it's taken (a second migration of a restored folder).
fn free_path(path: &Path) -> PathBuf {
    let mut candidate = path.to_path_buf();
    let mut n = 1;
    while candidate.exists() {
        n += 1;
        candidate = path.with_file_name(format!("{PARKED}-{n}"));
    }
    candidate
}
