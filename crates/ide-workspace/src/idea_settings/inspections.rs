//! `inspectionProfiles/`: the project's inspection profiles (`Project_Default.xml`
//! and copies) and which one is in effect (`profiles_settings.xml`).
//!
//! Every `<inspection_tool>` in a profile reads as an override, IntelliJ's own
//! inspections included, so writing the map back keeps them; their non-boolean
//! options, which OpenIDEA has no place for, stay on the element.

use super::{Writer, bool_str, idea, put, read_component_file, update_component_file};
use ide_core::IdeResult;
use ide_core::idea_xml::Element;
use serde_json::{Map, Value as Json, json};
use std::path::{Path, PathBuf};

const COMPONENT: &str = "InspectionProjectProfileManager";
const SETTINGS: &str = "profiles_settings.xml";
const PROJECT_FILE: &str = "Project_Default.xml";
const PROJECT_NAME: &str = "Project Default";

const LEVELS: &[(&str, &str)] = &[
    ("error", "ERROR"),
    ("warning", "WARNING"),
    ("weakWarning", "WEAK WARNING"),
    ("serverProblem", "SERVER PROBLEM"),
    ("typo", "TYPO"),
    ("none", "INFORMATION"),
];

fn dir(root: &Path) -> PathBuf {
    idea(root, "inspectionProfiles")
}

/// Project profile copies: `(key, file)` for every profile file but the defaults.
fn copies(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir(root)) else { return Vec::new() };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "xml"))
        .filter(|p| p.file_name().is_some_and(|n| n != SETTINGS && n != PROJECT_FILE))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), p)))
        .collect();
    out.sort();
    out
}

fn profile_of(file: &Element) -> Option<&Element> {
    file.child("profile")
}

/// `(name, overrides)` of a profile file.
fn read_profile(path: &Path) -> Option<(String, Map<String, Json>)> {
    let file = read_component_file(path)?;
    let profile = profile_of(&file)?;
    let mut overrides = Map::new();
    for tool in profile.elements().filter(|e| e.name == "inspection_tool") {
        let Some(class) = tool.attr("class") else { continue };
        let mut o = Map::new();
        if let Some(on) = tool.attr("enabled") {
            o.insert("enabled".into(), json!(on == "true"));
        }
        if let Some((ours, _)) = tool.attr("level").and_then(|l| LEVELS.iter().find(|(_, t)| *t == l)) {
            o.insert("severity".into(), json!(ours));
        }
        let options: Map<String, Json> = tool
            .elements()
            .filter(|e| e.name == "option")
            .filter_map(|e| match (e.attr("name"), e.attr("value")) {
                (Some(n), Some("true")) => Some((n.to_owned(), json!(true))),
                (Some(n), Some("false")) => Some((n.to_owned(), json!(false))),
                _ => None,
            })
            .collect();
        if !options.is_empty() {
            o.insert("options".into(), Json::Object(options));
        }
        overrides.insert(class.to_owned(), Json::Object(o));
    }
    Some((profile.option("myName").unwrap_or_default().to_owned(), overrides))
}

pub(super) fn read(root: &Path, out: &mut Json) {
    let d = dir(root);
    if let Some((_, overrides)) = read_profile(&d.join(PROJECT_FILE)) {
        put(out, "inspections.project", Json::Object(overrides));
    }
    let mut profiles = Map::new();
    let mut by_name = Vec::new();
    for (key, path) in copies(root) {
        if let Some((name, overrides)) = read_profile(&path) {
            by_name.push((name.clone(), key.clone()));
            profiles.insert(key, json!({ "name": name, "overrides": overrides }));
        }
    }
    if !profiles.is_empty() {
        put(out, "inspections.projectProfiles", Json::Object(profiles));
    }
    let Some(settings) = read_component_file(&d.join(SETTINGS)) else { return };
    let Some(s) = settings.child("settings") else { return };
    let selected = s.option("PROJECT_PROFILE");
    let profile = if s.option("USE_PROJECT_PROFILE") == Some("false") {
        // an IDE-level copy is known by name only; the sidecar keeps which one
        match selected {
            None => Some("default".to_owned()),
            Some(_) => None,
        }
    } else {
        match selected {
            None | Some(PROJECT_NAME) => Some("project".to_owned()),
            Some(name) => by_name.iter().find(|(n, _)| n == name).map(|(_, k)| format!("project:{k}")),
        }
    };
    if let Some(p) = profile {
        put(out, "inspections.profile", Json::String(p));
    }
}

fn write_tool(tool: &mut Element, o: &Map<String, Json>) {
    match o.get("enabled").and_then(Json::as_bool) {
        Some(on) => {
            tool.set_attr("enabled", bool_str(on));
            if tool.attr("enabled_by_default").is_none() {
                tool.set_attr("enabled_by_default", bool_str(on));
            }
        }
        None => tool.remove_attr("enabled"),
    }
    match o.get("severity").and_then(Json::as_str).and_then(|s| LEVELS.iter().find(|(ours, _)| *ours == s)) {
        Some((_, level)) => tool.set_attr("level", *level),
        None => tool.remove_attr("level"),
    }
    let options = o.get("options").and_then(Json::as_object);
    // boolean options are OpenIDEA's to rewrite; anything else stays
    tool.retain_elements(|e| {
        !(e.name == "option" && matches!(e.attr("value"), Some("true" | "false")))
            || options.is_some_and(|m| e.attr("name").is_some_and(|n| m.contains_key(n)))
    });
    for (name, v) in options.into_iter().flatten() {
        if let Some(b) = v.as_bool() {
            tool.set_option(name, Some(bool_str(b)));
        }
    }
}

fn write_profile(path: &Path, name: &str, overrides: &Map<String, Json>) -> IdeResult<()> {
    if overrides.is_empty() && !path.exists() {
        return Ok(());
    }
    update_component_file(path, COMPONENT, |c| {
        let profile = c.child_or_insert("profile");
        if profile.attr("version").is_none() {
            profile.set_attr("version", "1.0");
        }
        profile.set_option("myName", Some(name));
        profile.retain_elements(|e| {
            e.name != "inspection_tool" || e.attr("class").is_some_and(|k| overrides.contains_key(k))
        });
        for (class, o) in overrides {
            let Some(o) = o.as_object() else { continue };
            let is = |e: &Element| e.name == "inspection_tool" && e.attr("class") == Some(class);
            if !profile.elements().any(is) {
                profile.push(Element::new("inspection_tool").with_attr("class", class.as_str()));
            }
            write_tool(profile.elements_mut().find(|e| is(e)).expect("just ensured"), o);
        }
    })
}

pub(super) fn write(w: &Writer<'_>) -> IdeResult<()> {
    let d = dir(w.root);
    let empty = Map::new();
    let project = w.get("inspections.project").and_then(Json::as_object).unwrap_or(&empty);
    write_profile(&d.join(PROJECT_FILE), PROJECT_NAME, project)?;

    let copies_now = w.get("inspections.projectProfiles").and_then(Json::as_object).unwrap_or(&empty);
    for (key, path) in copies(w.root) {
        if !copies_now.contains_key(&key) {
            std::fs::remove_file(path)?;
        }
    }
    let name_of =
        |key: &str| copies_now.get(key).and_then(|p| p.get("name")).and_then(Json::as_str).unwrap_or(key).to_owned();
    for (key, p) in copies_now {
        let overrides = p.get("overrides").and_then(Json::as_object).unwrap_or(&empty);
        write_profile(&d.join(format!("{key}.xml")), &name_of(key), overrides)?;
    }

    let profile = w.str("inspections.profile");
    let (use_project, selected) = match profile {
        "default" => (false, None),
        p if p.starts_with("ide:") => (false, Some(p["ide:".len()..].to_owned())),
        p if p.starts_with("project:") => (true, Some(name_of(&p["project:".len()..]))),
        _ => (true, Some(PROJECT_NAME.to_owned())),
    };
    let settings_file = d.join(SETTINGS);
    if !settings_file.exists() && profile.is_empty() && project.is_empty() && copies_now.is_empty() {
        return Ok(());
    }
    update_component_file(&settings_file, COMPONENT, |c| {
        let s = c.child_or_insert("settings");
        s.set_option("PROJECT_PROFILE", selected.as_deref());
        s.set_option("USE_PROJECT_PROFILE", Some(bool_str(use_project)));
        if s.child("version").is_none() {
            s.push(Element::new("version").with_attr("value", "1.0"));
        }
    })
}
