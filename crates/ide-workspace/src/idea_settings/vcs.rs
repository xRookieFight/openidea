//! `vcs.xml`'s Issue Navigation links. The VCS directory mappings next to them
//! are IntelliJ's; OpenIDEA finds repositories on its own.

use super::{Writer, idea, put};
use ide_core::IdeResult;
use ide_core::idea_xml::{self, Element, component, component_mut};
use serde_json::{Value as Json, json};
use std::path::{Path, PathBuf};

const COMPONENT: &str = "IssueNavigationConfiguration";
const KEY: &str = "vcs.issueNavigation";

fn file(root: &Path) -> PathBuf {
    idea(root, "vcs.xml")
}

pub(super) fn read(root: &Path, out: &mut Json) {
    let Some(project) = super::read_component_file(&file(root)) else { return };
    let Some(list) = component(&project, COMPONENT).and_then(|c| c.option_node("links")).and_then(|o| o.child("list"))
    else {
        return;
    };
    let links: Vec<Json> = list
        .elements()
        .filter(|e| e.name == "IssueNavigationLink")
        .map(|l| {
            json!({
                "pattern": l.option("issueRegexp").unwrap_or_default(),
                "link": l.option("linkRegexp").unwrap_or_default(),
            })
        })
        .collect();
    put(out, KEY, Json::Array(links));
}

pub(super) fn write(w: &Writer<'_>) -> IdeResult<()> {
    let links: Vec<(String, String)> = w
        .changed(KEY)
        .and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .map(|l| {
                    let s = |k: &str| l.get(k).and_then(Json::as_str).unwrap_or_default().to_owned();
                    (s("pattern"), s("link"))
                })
                .collect()
        })
        .unwrap_or_default();
    idea_xml::update_project_file(&file(w.root), |p| {
        if links.is_empty() {
            p.retain_elements(|e| !(e.name == "component" && e.attr("name") == Some(COMPONENT)));
            return;
        }
        let mut list = Element::new("list");
        for (pattern, link) in &links {
            let mut l = Element::new("IssueNavigationLink");
            l.set_option("issueRegexp", Some(pattern));
            l.set_option("linkRegexp", Some(link));
            list.push(l);
        }
        let o = component_mut(p, COMPONENT).option_node_mut("links");
        o.children = vec![idea_xml::Node::Element(list)];
    })
}
