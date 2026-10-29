//! `misc.xml`'s `ProjectRootManager`: the project SDK, by name. Names are
//! IntelliJ's (its `jdk.table.xml`); a JDK it never registered gets the name
//! IntelliJ would give it, from the JDK's `release` file.

use super::{Writer, idea, put};
use ide_core::IdeResult;
use ide_core::idea_xml::{self, Element, component, component_mut};
use serde_json::Value as Json;
use std::path::{Path, PathBuf};

const COMPONENT: &str = "ProjectRootManager";
const JAVA: &str = "JavaSDK";
const PYTHON: &str = "Python SDK";
const JAVA_KEY: &str = "languages.java.jdk";
const PYTHON_KEY: &str = "languages.python.interpreter";

fn file(root: &Path) -> PathBuf {
    idea(root, "misc.xml")
}

/// `(name, type, home)` of every SDK in the newest IntelliJ install's `jdk.table.xml`.
fn sdk_table() -> Vec<(String, String, String)> {
    let Some(base) = dirs::config_dir().map(|d| d.join("JetBrains")) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(base) else { return Vec::new() };
    let newest = entries
        .flatten()
        .map(|e| e.path().join("options/jdk.table.xml"))
        .filter(|p| p.is_file())
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok());
    let Some(table) = newest.and_then(|p| idea_xml::read_project_file(&p).ok().flatten()) else { return Vec::new() };
    let home = dirs::home_dir().unwrap_or_default();
    let value = |e: &Element, name: &str| e.child(name).and_then(|c| c.attr("value")).unwrap_or_default().to_owned();
    component(&table, "ProjectJdkTable")
        .into_iter()
        .flat_map(Element::elements)
        .filter(|e| e.name == "jdk")
        .map(|e| {
            let path = value(e, "homePath").replace("$USER_HOME$", &home.to_string_lossy());
            (value(e, "name"), value(e, "type"), path)
        })
        .collect()
}

fn home_for(name: &str, ty: &str) -> Option<String> {
    sdk_table().into_iter().find(|(n, t, _)| n == name && t == ty).map(|(_, _, home)| home)
}

/// `KEY="value"` lines of a JDK's `release` file.
fn release(home: &str) -> Vec<(String, String)> {
    std::fs::read_to_string(Path::new(home).join("release"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_owned(), v.trim().trim_matches('"').to_owned()))
        })
        .collect()
}

fn java_major(home: &str) -> Option<String> {
    let version = release(home).into_iter().find(|(k, _)| k == "JAVA_VERSION")?.1;
    let mut parts = version.split('.');
    let first = parts.next()?;
    // hmm, 1.8.0_392 is Java 8
    Some(if first == "1" { parts.next()?.to_owned() } else { first.to_owned() })
}

/// What IntelliJ calls the SDK at `home`: its own name when registered, else its naming scheme.
fn name_for(home: &str, ty: &str) -> String {
    if let Some((name, ..)) = sdk_table().into_iter().find(|(_, t, h)| t == ty && h == home) {
        return name;
    }
    if ty == PYTHON {
        return "Python".into();
    }
    let info = release(home);
    let field = |k: &str| info.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str()).unwrap_or_default();
    let Some(major) = java_major(home) else {
        return Path::new(home).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    };
    let vendor = match field("IMPLEMENTOR") {
        "Eclipse Adoptium" => Some("temurin"),
        "Amazon.com Inc." => Some("corretto"),
        "Azul Systems, Inc." => Some("zulu"),
        "JetBrains s.r.o." => Some("jbr"),
        _ if !field("GRAALVM_VERSION").is_empty() => Some("graalvm-ce"),
        "Oracle Corporation" | "N/A" | "" => Some("openjdk"),
        _ => None,
    };
    match vendor {
        Some(v) => format!("{v}-{major}"),
        None => major,
    }
}

pub(super) fn read(root: &Path, out: &mut Json) {
    let Some(project) = super::read_component_file(&file(root)) else { return };
    let Some(c) = component(&project, COMPONENT) else { return };
    let (Some(name), Some(ty)) = (c.attr("project-jdk-name"), c.attr("project-jdk-type")) else { return };
    let key = match ty {
        JAVA => JAVA_KEY,
        PYTHON => PYTHON_KEY,
        _ => return,
    };
    // a name we can't resolve leaves the key to the sidecar copy
    if let Some(home) = home_for(name, ty) {
        put(out, key, Json::String(home));
    }
}

pub(super) fn write(w: &Writer<'_>, key: &str) -> IdeResult<()> {
    let (key, ty) = if super::under(key, JAVA_KEY) { (JAVA_KEY, JAVA) } else { (PYTHON_KEY, PYTHON) };
    let home = w.str(key).to_owned();
    idea_xml::update_project_file(&file(w.root), |p| {
        let c = component_mut(p, COMPONENT);
        let current = c.attr("project-jdk-type").map(str::to_owned);
        // only one project SDK in IntelliJ: Python doesn't push a JDK out
        if ty == PYTHON && current.as_deref().is_some_and(|t| t != PYTHON) {
            return;
        }
        if home.is_empty() {
            if current.as_deref() == Some(ty) {
                c.remove_attr("project-jdk-name");
                c.remove_attr("project-jdk-type");
            }
        } else {
            if c.attr("version").is_none() {
                c.set_attr("version", "2");
            }
            if ty == JAVA
                && c.attr("languageLevel").is_none()
                && let Some(major) = java_major(&home)
            {
                c.set_attr("languageLevel", format!("JDK_{major}"));
            }
            if c.attr("default").is_none() {
                c.set_attr("default", "true");
            }
            c.set_attr("project-jdk-name", name_for(&home, ty));
            c.set_attr("project-jdk-type", ty);
        }
        if c.children.is_empty() && c.attrs.len() == 1 {
            p.retain_elements(|e| !(e.name == "component" && e.attr("name") == Some(COMPONENT)));
        }
    })
}
