//! `encodings.xml`: the project encoding, the one for `.properties` files and
//! whether new UTF-8 files get a BOM. Per-file encodings in there are IntelliJ's alone.

use super::{Writer, idea, put};
use ide_core::IdeResult;
use ide_core::idea_xml::{self, Element, component, component_mut};
use serde_json::Value as Json;
use std::path::{Path, PathBuf};

const COMPONENT: &str = "Encoding";
const BOM: &[(&str, &str)] =
    &[("never", "with NO BOM"), ("always", "with BOM"), ("windows", "with BOM under Windows, with no BOM otherwise")];

fn file(root: &Path) -> PathBuf {
    idea(root, "encodings.xml")
}

fn is_project(e: &Element) -> bool {
    e.name == "file" && e.attr("url") == Some("PROJECT")
}

pub(super) fn read(root: &Path, out: &mut Json) {
    let Some(project) = super::read_component_file(&file(root)) else { return };
    let Some(c) = component(&project, COMPONENT) else { return };
    if let Some(cs) = c.elements().find(|e| is_project(e)).and_then(|e| e.attr("charset")) {
        put(out, "editor.encodings.project", Json::String(cs.to_lowercase()));
    }
    if let Some(cs) = c.attr("defaultCharsetForPropertiesFiles") {
        put(out, "editor.encodings.properties", Json::String(cs.to_lowercase()));
    }
    if let Some((ours, _)) = c.attr("addBOMForNewFiles").and_then(|v| BOM.iter().find(|(_, t)| *t == v)) {
        put(out, "editor.encodings.utf8Bom", Json::String((*ours).to_owned()));
    }
}

pub(super) fn write(w: &Writer<'_>) -> IdeResult<()> {
    let project = w.changed("editor.encodings.project").and_then(Json::as_str).map(str::to_uppercase);
    let properties = w.changed("editor.encodings.properties").and_then(Json::as_str).map(str::to_uppercase);
    let bom = w
        .changed("editor.encodings.utf8Bom")
        .and_then(Json::as_str)
        .and_then(|v| BOM.iter().find(|(o, _)| *o == v))
        .map(|(_, t)| *t);
    idea_xml::update_project_file(&file(w.root), |p| {
        let c = component_mut(p, COMPONENT);
        match &properties {
            Some(cs) => c.set_attr("defaultCharsetForPropertiesFiles", cs.as_str()),
            None => c.remove_attr("defaultCharsetForPropertiesFiles"),
        }
        match bom {
            Some(v) => c.set_attr("addBOMForNewFiles", v),
            None => c.remove_attr("addBOMForNewFiles"),
        }
        match &project {
            Some(cs) => {
                if !c.elements().any(is_project) {
                    c.push(Element::new("file").with_attr("url", "PROJECT").with_attr("charset", ""));
                }
                let e = c.elements_mut().find(|e| is_project(e)).expect("just ensured");
                e.set_attr("charset", cs.as_str());
            }
            None => c.retain_elements(|e| !is_project(e)),
        }
        if c.children.is_empty() && c.attrs.len() == 1 {
            p.retain_elements(|e| !(e.name == "component" && e.attr("name") == Some(COMPONENT)));
        }
    })
}
