//! Run configurations stored the way IntelliJ stores them:
//!
//! - shared ones ("Store as project file") one per file in
//!   `.idea/runConfigurations/<Name>.xml`, or in `<root>/.run/*.run.xml` when
//!   that's where IntelliJ put them;
//! - workspace-local ones and templates in the `RunManager` component of
//!   `.idea/workspace.xml`.
//!
//! A configuration is identified by its IntelliJ type and name. Configurations
//! of types OpenIDEA doesn't know are never surfaced and never touched, and
//! whatever the XML can't hold goes to a sidecar (see [`sidecar`]).

mod sidecar;
mod types;

use crate::legacy::slug;
use crate::{BeforeLaunchTask, RunConfig};
use ide_core::idea_xml::{self, Element, component, component_mut};
use ide_core::{IdeResult, project_dir};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

const MANAGER: &str = "ProjectRunConfigurationManager";
const RUN_MANAGER: &str = "RunManager";
const SHARED_DIR: &str = "runConfigurations";
const DOT_RUN: &str = ".run";
const TOOL_PREFIX: &str = "Tool_External Tools_";

/// `p` (project-relative or absolute) as IntelliJ writes paths: `$PROJECT_DIR$/…`.
pub(crate) fn to_idea(root: &Path, p: &str) -> String {
    let abs = if p.is_empty() { root.to_path_buf() } else { root.join(p) };
    idea_xml::collapse_path(root, &abs.to_string_lossy())
}

/// The inverse of [`to_idea`]: project-relative when inside the project, `""` for the root itself.
pub(crate) fn from_idea(root: &Path, v: &str) -> String {
    let expanded = idea_xml::expand_path(root, v);
    let base = root.to_string_lossy().replace('\\', "/");
    if expanded == base {
        return String::new();
    }
    match expanded.strip_prefix(&base).and_then(|r| r.strip_prefix('/')) {
        Some(rel) => rel.to_owned(),
        None => expanded,
    }
}

fn workspace(root: &Path) -> PathBuf {
    project_dir::idea_path(root, "workspace.xml")
}

fn shared_sidecar(root: &Path) -> PathBuf {
    project_dir::path(root, "run.toml")
}

fn local_sidecar(root: &Path) -> PathBuf {
    project_dir::path(root, "workspace.toml")
}

fn is_config(e: &Element) -> bool {
    e.name == "configuration"
}

fn is_template(e: &Element) -> bool {
    e.attr("default") == Some("true")
}

fn kind_of(e: &Element) -> Option<String> {
    types::kind_of(e.attr("type")?, e.attr("factoryName"))
}

/// Ours to rewrite: a configuration of a type OpenIDEA maps.
fn mapped(e: &Element) -> bool {
    is_config(e) && kind_of(e).is_some()
}

/// IntelliJ's file name for a shared configuration.
fn file_name(name: &str) -> String {
    let stem: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    format!("{stem}.xml")
}

fn in_dot_run(root: &Path, p: &Path) -> bool {
    p.parent() == Some(root.join(DOT_RUN).as_path())
}

/// Shared configuration files, `.idea/runConfigurations` first.
fn shared_files(root: &Path) -> IdeResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    for (dir, suffix) in [(project_dir::idea_path(root, SHARED_DIR), ".xml"), (root.join(DOT_RUN), ".run.xml")] {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.to_string_lossy().ends_with(suffix))
            .collect();
        files.sort();
        out.extend(files);
    }
    Ok(out)
}

/// The `<configuration>`s of a shared file (its root is the manager component).
fn read_shared(path: &Path) -> IdeResult<Vec<Element>> {
    let Some(root) = idea_xml::read_project_file(path)? else { return Ok(Vec::new()) };
    let manager = if root.name == "project" { component(&root, MANAGER).cloned() } else { Some(root) };
    Ok(manager.map(|m| m.elements().filter(|e| is_config(e)).cloned().collect()).unwrap_or_default())
}

fn write_shared(path: &Path, configs: Vec<Element>) -> IdeResult<()> {
    if configs.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    let mut manager = Element::new("component").with_attr("name", MANAGER);
    for c in configs {
        manager.push(c);
    }
    let text = manager.to_xml();
    if std::fs::read_to_string(path).ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    idea_xml::write_atomic(path, &text)
}

fn blank(kind: String, name: String, local: bool) -> RunConfig {
    let mut c = RunConfig {
        id: String::new(),
        name,
        kind,
        command: String::new(),
        args: Vec::new(),
        cwd: None,
        env: BTreeMap::new(),
        detected: false,
        is_test: false,
        debug_adapter: None,
        allow_parallel: false,
        extras: Default::default(),
    };
    c.extras.local = local;
    c
}

type Resolve<'a> = &'a dyn Fn(&str, &str) -> Option<String>;
type Refs<'a> = &'a dyn Fn(&str) -> Option<(String, String)>;

/// A configuration element as OpenIDEA sees it (id left empty). `resolve` turns
/// IntelliJ's `(type, name)` references into configuration ids.
fn parse(e: &Element, root: &Path, local: bool, resolve: Resolve<'_>) -> Option<RunConfig> {
    let kind = kind_of(e)?;
    let mut c = blank(kind.clone(), e.attr("name").unwrap_or_default().to_owned(), local);
    c.extras.folder = e.attr("folderName").filter(|f| !f.is_empty()).map(str::to_owned);
    c.allow_parallel = e.attr("singleton") == Some("false");
    c.extras.show_on_stdout = e.attr("show_console_on_std_out") == Some("true");
    c.extras.show_on_stderr = e.attr("show_console_on_std_err") == Some("true");
    if let Some(out) = e.child("output_file").filter(|o| o.attr("is_save") == Some("true")) {
        c.extras.output_file = out.attr("path").map(|p| from_idea(root, p)).filter(|p| !p.is_empty());
    }
    if types::has_envs(&kind) {
        for env in e.child("envs").into_iter().flat_map(Element::elements) {
            if let Some(name) = env.attr("name") {
                c.env.insert(name.to_owned(), env.attr("value").unwrap_or_default().to_owned());
            }
        }
    }
    for task in e.child("method").into_iter().flat_map(Element::elements) {
        if task.attr("enabled") != Some("true") {
            continue;
        }
        let step = match task.attr("name") {
            Some("Make") => Some(BeforeLaunchTask { kind: "build".into(), target: String::new() }),
            Some("RunConfigurationTask") => resolve(
                task.attr("run_configuration_type").unwrap_or_default(),
                task.attr("run_configuration_name").unwrap_or_default(),
            )
            .map(|id| BeforeLaunchTask { kind: "run".into(), target: id }),
            Some("ToolBeforeRunTask") => task
                .attr("actionId")
                .and_then(|a| a.strip_prefix(TOOL_PREFIX))
                .map(|id| BeforeLaunchTask { kind: "tool".into(), target: id.to_owned() }),
            _ => None,
        };
        c.extras.before_launch.extend(step);
    }
    if kind == types::COMPOUND {
        c.extras.compound = e
            .elements()
            .filter(|x| x.name == "toRun")
            .filter_map(|x| resolve(x.attr("type")?, x.attr("name")?))
            .collect();
    }
    types::read(&kind, e, root, &mut c);
    Some(c)
}

/// The element for `c`, starting from `base` (the one already on disk) so the
/// options OpenIDEA doesn't write survive. `refs` turns ids into `(type, name)`.
fn build(c: &RunConfig, base: Option<&Element>, root: &Path, refs: Refs<'_>, template: bool) -> Element {
    let (ty, factory) = types::idea_type(&c.kind);
    let fresh = base.is_none();
    let mut e = base.cloned().unwrap_or_else(|| Element::new("configuration"));
    e.set_attr("default", if template { "true" } else { "false" });
    if !template {
        e.set_attr("name", c.name.as_str());
    }
    e.set_attr("type", ty);
    match factory {
        Some(f) => e.set_attr("factoryName", f),
        None => e.remove_attr("factoryName"),
    }
    let flag = |e: &mut Element, k: &str, on: bool, v: &str| if on { e.set_attr(k, v) } else { e.remove_attr(k) };
    match c.extras.folder.as_deref() {
        Some(f) => e.set_attr("folderName", f),
        None => e.remove_attr("folderName"),
    }
    flag(&mut e, "singleton", c.allow_parallel, "false");
    flag(&mut e, "show_console_on_std_out", c.extras.show_on_stdout, "true");
    flag(&mut e, "show_console_on_std_err", c.extras.show_on_stderr, "true");

    types::write(&c.kind, c, &mut e, root, fresh);
    if c.kind == types::COMPOUND {
        e.retain_elements(|x| x.name != "toRun");
        for (ty, name) in c.extras.compound.iter().filter_map(|id| refs(id)) {
            e.push(Element::new("toRun").with_attr("name", name).with_attr("type", ty));
        }
    }
    if types::has_envs(&c.kind) {
        let envs = e.child_or_insert("envs");
        envs.children.clear();
        for (k, v) in &c.env {
            envs.push(Element::new("env").with_attr("name", k.as_str()).with_attr("value", v.as_str()));
        }
    }
    match &c.extras.output_file {
        Some(path) => {
            let out = e.child_or_insert("output_file");
            out.set_attr("path", to_idea(root, path));
            out.set_attr("is_save", "true");
        }
        None => e.retain_elements(|x| x.name != "output_file"),
    }
    let method = e.child_or_insert("method");
    if method.attr("v").is_none() {
        method.set_attr("v", "2");
    }
    method.retain_elements(|o| !matches!(o.attr("name"), Some("Make" | "RunConfigurationTask" | "ToolBeforeRunTask")));
    for task in &c.extras.before_launch {
        let option = match task.kind.as_str() {
            "build" => Some(Element::new("option").with_attr("name", "Make").with_attr("enabled", "true")),
            "run" => refs(&task.target).map(|(ty, name)| {
                Element::new("option")
                    .with_attr("name", "RunConfigurationTask")
                    .with_attr("enabled", "true")
                    .with_attr("run_configuration_name", name)
                    .with_attr("run_configuration_type", ty)
            }),
            "tool" => Some(
                Element::new("option")
                    .with_attr("name", "ToolBeforeRunTask")
                    .with_attr("enabled", "true")
                    .with_attr("actionId", format!("{TOOL_PREFIX}{}", task.target)),
            ),
            _ => None,
        };
        if let Some(o) = option {
            method.push(o);
        }
    }
    e
}

/// `user:<name>`, or `user:<type>-<name>` for names that clash.
fn derive_ids(identities: &[(String, String)]) -> Vec<String> {
    let plain: Vec<String> = identities.iter().map(|(_, n)| format!("user:{}", slug(n))).collect();
    plain
        .iter()
        .zip(identities)
        .map(|(id, (ty, n))| {
            if plain.iter().filter(|x| *x == id).count() > 1 {
                format!("user:{}-{}", slug(ty), slug(n))
            } else {
                id.clone()
            }
        })
        .collect()
}

fn key(ty: &str, name: &str) -> String {
    format!("{ty}/{name}")
}

enum Origin {
    Shared(PathBuf),
    Local,
}

struct Entry {
    config: RunConfig,
    element: Element,
    origin: Origin,
}

fn scan(root: &Path) -> IdeResult<Vec<Entry>> {
    let mut raw: Vec<(Element, Origin)> = Vec::new();
    for path in shared_files(root)? {
        for e in read_shared(&path)? {
            if mapped(&e) && !is_template(&e) {
                raw.push((e, Origin::Shared(path.clone())));
            }
        }
    }
    if let Some(ws) = idea_xml::read_project_file(&workspace(root))?
        && let Some(rm) = component(&ws, RUN_MANAGER)
    {
        raw.extend(rm.elements().filter(|e| mapped(e) && !is_template(e)).map(|e| (e.clone(), Origin::Local)));
    }
    let shared_side = sidecar::read(&shared_sidecar(root), "run");
    let local_side = sidecar::read(&local_sidecar(root), "run");
    let side = |o: &Origin| if matches!(o, Origin::Local) { &local_side } else { &shared_side };

    let identities: Vec<(String, String)> = raw
        .iter()
        .map(|(e, _)| (e.attr("type").unwrap_or_default().to_owned(), e.attr("name").unwrap_or_default().to_owned()))
        .collect();
    let mut ids = derive_ids(&identities);
    for (i, (_, origin)) in raw.iter().enumerate() {
        let (ty, name) = &identities[i];
        if let Some(id) = side(origin).get(&key(ty, name)).and_then(sidecar::id_of) {
            ids[i] = id.to_owned();
        }
    }
    let by_identity: HashMap<(&str, &str), &str> =
        identities.iter().zip(&ids).map(|((t, n), id)| ((t.as_str(), n.as_str()), id.as_str())).collect();
    let resolve = |t: &str, n: &str| by_identity.get(&(t, n)).map(|id| (*id).to_owned());

    let mut out = Vec::with_capacity(raw.len());
    for (i, (element, origin)) in raw.into_iter().enumerate() {
        let Some(mut config) = parse(&element, root, matches!(origin, Origin::Local), &resolve) else { continue };
        config.id = ids[i].clone();
        let (ty, name) = &identities[i];
        if let Some(entry) = side(&origin).get(&key(ty, name)) {
            config = sidecar::apply(config, entry);
        }
        out.push(Entry { config, element, origin });
    }
    Ok(out)
}

/// Where each configuration was in the list when OpenIDEA last saved it; the
/// files themselves have no order (IntelliJ sorts its tree by type).
const ORDER: &str = "order";

pub(crate) fn load(root: &Path) -> IdeResult<Vec<RunConfig>> {
    let mut configs: Vec<RunConfig> = scan(root)?.into_iter().map(|e| e.config).collect();
    let order = sidecar::read_list(&local_sidecar(root), ORDER);
    let rank = |c: &RunConfig| order.iter().position(|id| *id == c.id).unwrap_or(usize::MAX);
    configs.sort_by_key(rank);
    Ok(configs)
}

/// Puts `ours` into the `RunManager` component in place of the mapped
/// configurations `which` selects, keeping everything else where it is.
fn update_run_manager(root: &Path, which: impl Fn(&Element) -> bool, ours: Vec<Element>) -> IdeResult<()> {
    idea_xml::update_project_file(&workspace(root), |p| {
        let rm = component_mut(p, RUN_MANAGER);
        rm.retain_elements(|e| !(mapped(e) && which(e)));
        // configurations come before IntelliJ's <list> and <recent_temporary>
        let at = rm
            .children
            .iter()
            .position(|c| matches!(c, idea_xml::Node::Element(e) if !is_config(e)))
            .unwrap_or(rm.children.len());
        for (i, e) in ours.into_iter().enumerate() {
            rm.children.insert(at + i, idea_xml::Node::Element(e));
        }
        if rm.children.is_empty() && rm.attrs.len() == 1 {
            p.retain_elements(|e| component_name(e) != Some(RUN_MANAGER));
        }
    })
}

fn component_name(e: &Element) -> Option<&str> {
    (e.name == "component").then(|| e.attr("name")).flatten()
}

pub(crate) fn save(root: &Path, configs: &[RunConfig]) -> IdeResult<()> {
    let existing = scan(root)?;
    let previous: HashMap<&str, &Entry> = existing.iter().map(|e| (e.config.id.as_str(), e)).collect();
    let identities: Vec<(String, String)> =
        configs.iter().map(|c| (types::idea_type(&c.kind).0.to_owned(), c.name.clone())).collect();
    let derived = derive_ids(&identities);
    let by_id: HashMap<&str, (String, String)> =
        configs.iter().zip(&identities).map(|(c, idn)| (c.id.as_str(), idn.clone())).collect();
    let by_identity: HashMap<(&str, &str), &str> =
        identities.iter().zip(configs).map(|((t, n), c)| ((t.as_str(), n.as_str()), c.id.as_str())).collect();
    let refs = |id: &str| by_id.get(id).cloned();
    let resolve = |t: &str, n: &str| by_identity.get(&(t, n)).map(|id| (*id).to_owned());

    let mut shared: BTreeMap<PathBuf, Vec<Element>> = BTreeMap::new();
    let mut local = Vec::new();
    let (mut shared_side, mut local_side) = (sidecar::Entries::new(), sidecar::Entries::new());
    for (i, c) in configs.iter().enumerate() {
        let prev = previous.get(c.id.as_str()).filter(|p| p.config.kind == c.kind);
        let element = build(c, prev.map(|p| &p.element), root, &refs, false);
        let mut lossy = parse(&element, root, c.extras.local, &resolve).expect("built from a mapped kind");
        lossy.id = derived[i].clone();
        let (ty, name) = &identities[i];
        let mut original = c.clone();
        original.detected = false;
        if let Some(d) = sidecar::diff(&original, &lossy) {
            if c.extras.local { &mut local_side } else { &mut shared_side }.insert(key(ty, name), d);
        }
        if c.extras.local {
            local.push(element);
            continue;
        }
        let path = match prev.map(|p| &p.origin) {
            Some(Origin::Shared(p)) if in_dot_run(root, p) => p.clone(),
            Some(Origin::Shared(p)) if p.file_name().is_some_and(|f| f.to_string_lossy() == file_name(&c.name)) => {
                p.clone()
            }
            _ => project_dir::idea_path(root, SHARED_DIR).join(file_name(&c.name)),
        };
        shared.entry(path).or_default().push(element);
    }

    let paths: BTreeSet<PathBuf> = shared_files(root)?.into_iter().chain(shared.keys().cloned()).collect();
    for path in paths {
        let mut keep: Vec<Element> = read_shared(&path)?.into_iter().filter(|e| !mapped(e) || is_template(e)).collect();
        keep.extend(shared.remove(&path).unwrap_or_default());
        write_shared(&path, keep)?;
    }
    update_run_manager(root, |e| !is_template(e), local)?;
    sidecar::write(&shared_sidecar(root), "run", &shared_side)?;
    sidecar::write(&local_sidecar(root), "run", &local_side)?;
    let ids: Vec<String> = if configs.len() > 1 { configs.iter().map(|c| c.id.clone()).collect() } else { Vec::new() };
    sidecar::write_list(&local_sidecar(root), ORDER, &ids)?;
    if !configs.is_empty() {
        project_dir::ensure_gitignore(root)?;
    }
    Ok(())
}

fn template_id(kind: &str) -> String {
    format!("template:{kind}")
}

fn no_refs(_: &str, _: &str) -> Option<String> {
    None
}

fn scan_templates(root: &Path) -> IdeResult<Vec<(RunConfig, Element)>> {
    let Some(ws) = idea_xml::read_project_file(&workspace(root))? else { return Ok(Vec::new()) };
    let Some(rm) = component(&ws, RUN_MANAGER) else { return Ok(Vec::new()) };
    let side = sidecar::read(&local_sidecar(root), "templates");
    Ok(rm
        .elements()
        .filter(|e| mapped(e) && is_template(e))
        .filter_map(|e| {
            let mut c = parse(e, root, false, &no_refs)?;
            c.id = template_id(&c.kind);
            if let Some(entry) = side.get(&c.id) {
                c = sidecar::apply(c, entry);
            }
            Some((c, e.clone()))
        })
        .collect())
}

pub(crate) fn load_templates(root: &Path) -> IdeResult<Vec<RunConfig>> {
    Ok(scan_templates(root)?.into_iter().map(|(c, _)| c).collect())
}

pub(crate) fn save_templates(root: &Path, templates: &[RunConfig]) -> IdeResult<()> {
    let existing = scan_templates(root)?;
    let mut ours = Vec::new();
    let mut side = sidecar::Entries::new();
    for t in templates {
        let base = existing.iter().find(|(c, _)| c.kind == t.kind).map(|(_, e)| e);
        let element = build(t, base, root, &|_| None, true);
        let mut lossy = parse(&element, root, false, &no_refs).expect("built from a mapped kind");
        lossy.id = template_id(&t.kind);
        if let Some(d) = sidecar::diff(t, &lossy) {
            side.insert(template_id(&t.kind), d);
        }
        ours.push(element);
    }
    update_run_manager(root, is_template, ours)?;
    sidecar::write(&local_sidecar(root), "templates", &side)?;
    if !templates.is_empty() {
        project_dir::ensure_gitignore(root)?;
    }
    Ok(())
}
