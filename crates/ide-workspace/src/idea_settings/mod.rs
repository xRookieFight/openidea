//! Project settings IntelliJ has a file for, read from and written to that file
//! instead of OpenIDEA's own `settings.toml`: the project SDK (`misc.xml`), code
//! style (`codeStyles/`), inspection profiles (`inspectionProfiles/`), issue
//! navigation (`vcs.xml`) and encodings (`encodings.xml`).
//!
//! Reading yields a partial settings object (camelCase keys, like the TOML
//! layers) holding only what the files define. Writing takes the project's view
//! of the settings (defaults, the sidecar TOML and these files merged) and
//! re-emits the owning file's section, leaving whatever OpenIDEA doesn't map.

mod code_style;
mod encodings;
mod inspections;
mod misc;
mod vcs;

use ide_core::idea_xml::{self, Element};
use ide_core::{IdeResult, project_dir};
use serde_json::{Map, Value as Json};
use std::path::Path;

/// Which module owns which keys; a key owns everything below it too.
const OWNERS: &[(&str, Module)] = &[
    ("languages.java.jdk", Module::Misc),
    ("languages.python.interpreter", Module::Misc),
    ("editor.codeStyle.lineSeparator", Module::CodeStyle),
    ("editor.codeStyle.hardWrapAt", Module::CodeStyle),
    ("editor.codeStyle.visualGuides", Module::CodeStyle),
    ("editor.codeStyle.editorconfig", Module::CodeStyle),
    ("editor.codeStyle.doNotFormat", Module::CodeStyle),
    ("editor.codeStyle.formatterTagsEnabled", Module::CodeStyle),
    ("editor.codeStyle.formatterOffTag", Module::CodeStyle),
    ("editor.codeStyle.formatterOnTag", Module::CodeStyle),
    ("editor.codeStyle.formatterTagsRegexp", Module::CodeStyle),
    ("editor.encodings.project", Module::Encodings),
    ("editor.encodings.properties", Module::Encodings),
    ("editor.encodings.utf8Bom", Module::Encodings),
    ("inspections.profile", Module::Inspections),
    ("inspections.project", Module::Inspections),
    ("inspections.projectProfiles", Module::Inspections),
    ("vcs.issueNavigation", Module::Vcs),
];

/// Values that also stay in the sidecar: IntelliJ only stores a name for
/// them, and a name we can't resolve back must not lose the setting.
const ALSO_IN_SIDECAR: &[&str] = &["languages.java.jdk", "languages.python.interpreter", "inspections.profile"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Module {
    Misc,
    CodeStyle,
    Encodings,
    Inspections,
    Vcs,
}

fn under(key: &str, prefix: &str) -> bool {
    key == prefix || key.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.'))
}

fn owner(key: &str) -> Option<Module> {
    if let Some(lang) = key.strip_prefix("editor.codeStyle.languages.") {
        let id = lang.split('.').next().unwrap_or_default();
        return code_style::maps_language(id).then_some(Module::CodeStyle);
    }
    OWNERS.iter().find(|(p, _)| under(key, p)).map(|(_, m)| *m)
}

/// Whether `key` (dotted, camelCase) is stored in an IntelliJ file.
pub(crate) fn owns(key: &str) -> bool {
    owner(key).is_some()
}

pub(crate) fn also_in_sidecar(key: &str) -> bool {
    ALSO_IN_SIDECAR.iter().any(|p| under(key, p))
}

/// The settings IntelliJ's files under `<root>/.idea` define.
pub(crate) fn read(root: &Path) -> Json {
    let mut out = Json::Object(Map::new());
    misc::read(root, &mut out);
    code_style::read(root, &mut out);
    encodings::read(root, &mut out);
    inspections::read(root, &mut out);
    vcs::read(root, &mut out);
    out
}

/// Writes the file that owns `key` from `project` (defaults + project layers,
/// after the change). Values equal to `defaults` are left out, like IntelliJ does.
pub(crate) fn write(root: &Path, key: &str, project: &Json, defaults: &Json) -> IdeResult<()> {
    let w = Writer { root, project, defaults };
    match owner(key) {
        Some(Module::Misc) => misc::write(&w, key),
        Some(Module::CodeStyle) => code_style::write(&w),
        Some(Module::Encodings) => encodings::write(&w),
        Some(Module::Inspections) => inspections::write(&w),
        Some(Module::Vcs) => vcs::write(&w),
        None => Ok(()),
    }
}

struct Writer<'a> {
    root: &'a Path,
    project: &'a Json,
    defaults: &'a Json,
}

impl Writer<'_> {
    fn get(&self, key: &str) -> Option<&Json> {
        get(self.project, key)
    }

    /// The value when it isn't the default, else `None`.
    fn changed(&self, key: &str) -> Option<&Json> {
        self.get(key).filter(|v| get(self.defaults, key) != Some(*v))
    }

    fn str(&self, key: &str) -> &str {
        self.get(key).and_then(Json::as_str).unwrap_or_default()
    }
}

pub(crate) fn get<'a>(v: &'a Json, key: &str) -> Option<&'a Json> {
    key.split('.').try_fold(v, |v, seg| v.get(seg))
}

/// Sets `key` in `v`, creating the objects on the way; `Json::Null` removes it.
pub(crate) fn put(v: &mut Json, key: &str, value: Json) {
    let segments: Vec<&str> = key.split('.').collect();
    let (last, parents) = segments.split_last().expect("non-empty key");
    let mut cur = v;
    for seg in parents {
        if !cur.get(*seg).is_some_and(Json::is_object) {
            if value.is_null() {
                return;
            }
            cur.as_object_mut().expect("object").insert((*seg).to_owned(), Json::Object(Map::new()));
        }
        cur = cur.get_mut(*seg).expect("just ensured");
    }
    let obj = cur.as_object_mut().expect("object");
    if value.is_null() {
        obj.remove(*last);
    } else {
        obj.insert((*last).to_owned(), value);
    }
}

fn idea(root: &Path, rel: &str) -> std::path::PathBuf {
    project_dir::idea_path(root, rel)
}

/// Read-modify-write of a file whose root is a `<component>` (code styles,
/// inspection profiles): no XML declaration, like IntelliJ writes them. A file
/// left without children is removed.
fn update_component_file(path: &Path, name: &str, f: impl FnOnce(&mut Element)) -> IdeResult<()> {
    let existing = std::fs::read_to_string(path).ok();
    let mut root = existing
        .as_deref()
        .and_then(|t| idea_xml::parse(t).ok())
        .unwrap_or_else(|| Element::new("component").with_attr("name", name));
    f(&mut root);
    if root.children.is_empty() {
        if existing.is_some() {
            std::fs::remove_file(path)?;
        }
        return Ok(());
    }
    let text = root.to_xml();
    if existing.as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    idea_xml::write_atomic(path, &text)
}

fn read_component_file(path: &Path) -> Option<Element> {
    idea_xml::read_project_file(path)
        .inspect_err(|e| tracing::warn!("ignoring {}: {}", path.display(), e.message))
        .ok()
        .flatten()
}

fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}
