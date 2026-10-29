//! Run configurations as older OpenIDEA versions saved them, in `<root>/.openidea/run.toml`.
//! Only the migration to `.idea` reads these now:
//!
//! ```toml
//! [[configurations]]
//! id = "user:server"
//! name = "Server"
//! kind = "shell"
//! command = "cargo"
//! args = ["run", "--release"]
//! cwd = "server"
//! env = { RUST_LOG = "debug" }
//! is-test = false
//! debug-adapter = "codelldb"
//! allow-parallel = true
//! folder = "Servers"
//! params = { mode = "command" }
//! env-files = [".env"]
//! include-system-env = false
//! stdin-file = "input.txt"
//! output-file = "out/server.log"
//! before-launch = [{ kind = "build" }, { kind = "run", target = "user:migrate" }]
//! emulate-terminal = true
//! stop-command = "docker stop server"
//!
//! [[templates]]          # per-type defaults for new configurations
//! kind = "npm"
//! env = { NODE_ENV = "development" }
//! ```
//!
//! Configurations not stored as project files were in the same format in the
//! workspace-local `run.local.toml`.

use crate::{BeforeLaunchTask, RunConfig, RunExtras};
use ide_core::{IdeError, IdeResult, project_dir};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const FILE: &str = "run.toml";
const LOCAL_FILE: &str = "run.local.toml";
fn config_path(root: &Path) -> PathBuf {
    project_dir::legacy_path(root, FILE)
}

fn local_path(root: &Path) -> PathBuf {
    project_dir::legacy_path(root, LOCAL_FILE)
}

#[derive(Deserialize)]
struct FileDoc {
    #[serde(default)]
    configurations: Vec<FileConfig>,
    #[serde(default)]
    templates: Vec<FileConfig>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct FileConfig {
    id: Option<String>,
    name: Option<String>,
    kind: Option<String>,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<PathBuf>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default, alias = "isTest", alias = "is_test")]
    is_test: bool,
    #[serde(alias = "debugAdapter", alias = "debug_adapter")]
    debug_adapter: Option<String>,
    #[serde(default, alias = "allowParallel", alias = "allow_parallel")]
    allow_parallel: bool,
    #[serde(default)]
    params: BTreeMap<String, String>,
    folder: Option<String>,
    #[serde(default = "yes", alias = "includeSystemEnv")]
    include_system_env: bool,
    #[serde(default, alias = "envFiles")]
    env_files: Vec<String>,
    #[serde(alias = "stdinFile")]
    stdin_file: Option<String>,
    #[serde(alias = "outputFile")]
    output_file: Option<String>,
    #[serde(default, alias = "beforeLaunch")]
    before_launch: Vec<BeforeLaunchTask>,
    #[serde(default)]
    compound: Vec<String>,
    #[serde(default, alias = "showOnStdout")]
    show_on_stdout: bool,
    #[serde(default, alias = "showOnStderr")]
    show_on_stderr: bool,
    #[serde(alias = "openToolWindow")]
    open_tool_window: Option<bool>,
    #[serde(default, alias = "focusToolWindow")]
    focus_tool_window: bool,
    #[serde(default, alias = "emulateTerminal")]
    emulate_terminal: bool,
    #[serde(alias = "stopCommand")]
    stop_command: Option<String>,
    target: Option<String>,
}

pub(crate) fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

fn read_doc(path: &Path) -> IdeResult<Option<FileDoc>> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    toml::from_str(&text).map(Some).map_err(|e| IdeError::invalid(format!("{}: {}", path.display(), e.message())))
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

fn convert(configs: Vec<FileConfig>, local: bool) -> Vec<RunConfig> {
    let mut out = Vec::with_capacity(configs.len());
    for (i, c) in configs.into_iter().enumerate() {
        let name = c.name.filter(|n| !n.is_empty()).unwrap_or_else(|| {
            if c.command.is_empty() { format!("Configuration {}", i + 1) } else { c.command.clone() }
        });
        let id = c.id.filter(|id| !id.is_empty()).unwrap_or_else(|| format!("user:{}", slug(&name)));
        out.push(RunConfig {
            id,
            name,
            kind: c.kind.filter(|k| !k.is_empty()).unwrap_or_else(|| "shell".to_owned()),
            command: c.command,
            args: c.args,
            cwd: c.cwd.filter(|p| !p.as_os_str().is_empty()).map(Into::into),
            env: c.env,
            detected: false,
            is_test: c.is_test,
            debug_adapter: c.debug_adapter.filter(|d| !d.is_empty()),
            allow_parallel: c.allow_parallel,
            extras: RunExtras {
                params: c.params,
                folder: non_empty(c.folder),
                local,
                include_system_env: c.include_system_env,
                env_files: c.env_files.into_iter().filter(|f| !f.trim().is_empty()).collect(),
                stdin_file: non_empty(c.stdin_file),
                output_file: non_empty(c.output_file),
                before_launch: c.before_launch.into_iter().filter(|t| !t.kind.is_empty()).collect(),
                compound: c.compound,
                show_on_stdout: c.show_on_stdout,
                show_on_stderr: c.show_on_stderr,
                open_tool_window: c.open_tool_window,
                focus_tool_window: c.focus_tool_window,
                emulate_terminal: c.emulate_terminal,
                stop_command: non_empty(c.stop_command),
                target: non_empty(c.target),
            },
        });
    }
    out
}

/// Project configurations (`run.toml`) followed by workspace-local ones (`run.local.toml`).
pub(crate) fn load(root: &Path) -> IdeResult<Vec<RunConfig>> {
    let mut out = read_doc(&config_path(root))?.map(|d| convert(d.configurations, false)).unwrap_or_default();
    if let Some(local) = read_doc(&local_path(root))? {
        let ids: Vec<String> = out.iter().map(|c| c.id.clone()).collect();
        out.extend(convert(local.configurations, true).into_iter().filter(|c| !ids.contains(&c.id)));
    }
    Ok(out)
}

pub(crate) fn load_templates(root: &Path) -> IdeResult<Vec<RunConfig>> {
    Ok(read_doc(&config_path(root))?.map(|d| convert(d.templates, false)).unwrap_or_default())
}
