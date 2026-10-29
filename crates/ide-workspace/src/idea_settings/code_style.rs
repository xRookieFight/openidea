//! `codeStyles/Project.xml`, the project's code style scheme, and
//! `codeStyles/codeStyleConfig.xml`, which tells IntelliJ to use it.

use super::{Writer, bool_str, get, idea, put, read_component_file, update_component_file};
use ide_core::IdeResult;
use ide_core::idea_xml::Element;
use serde_json::{Value as Json, json};
use std::path::{Path, PathBuf};

const COMPONENT: &str = "ProjectCodeStyleConfiguration";
const BASE: &str = "editor.codeStyle";

/// OpenIDEA language ids and IntelliJ's `language` names.
const LANGUAGES: &[(&str, &str)] = &[
    ("java", "JAVA"),
    ("kotlin", "kotlin"),
    ("javascript", "JavaScript"),
    ("typescript", "TypeScript"),
    ("python", "Python"),
    ("rust", "Rust"),
    ("go", "go"),
    ("json", "JSON"),
    ("xml", "XML"),
    ("yaml", "yaml"),
    ("css", "CSS"),
    ("html", "HTML"),
    ("markdown", "Markdown"),
    ("sql", "SQL"),
    ("shell", "Shell Script"),
    ("toml", "TOML"),
];

pub(super) fn maps_language(id: &str) -> bool {
    LANGUAGES.iter().any(|(ours, _)| *ours == id)
}

/// Scheme options holding one scalar setting: `(setting, option)`.
const SCALARS: &[(&str, &str)] = &[
    ("hardWrapAt", "RIGHT_MARGIN"),
    ("formatterTagsEnabled", "FORMATTER_TAGS_ENABLED"),
    ("formatterOffTag", "FORMATTER_OFF_TAG"),
    ("formatterOnTag", "FORMATTER_ON_TAG"),
    ("formatterTagsRegexp", "FORMATTER_TAGS_ACCEPT_REGEXP"),
];

/// Per-language options: `(setting, option, inside <indentOptions>)`.
const LANGUAGE_OPTIONS: &[(&str, &str, bool)] = &[
    ("hardWrapAt", "RIGHT_MARGIN", false),
    ("lineCommentAtFirstColumn", "LINE_COMMENT_AT_FIRST_COLUMN", false),
    ("lineCommentAddSpace", "LINE_COMMENT_ADD_SPACE", false),
    ("indent", "INDENT_SIZE", true),
    ("tabSize", "TAB_SIZE", true),
    ("useTab", "USE_TAB_CHARACTER", true),
];

const SEPARATORS: &[(&str, &str)] = &[("lf", "\n"), ("crlf", "\r\n"), ("cr", "\r")];

fn project_file(root: &Path) -> PathBuf {
    idea(root, "codeStyles/Project.xml")
}

fn config_file(root: &Path) -> PathBuf {
    idea(root, "codeStyles/codeStyleConfig.xml")
}

/// An option value as JSON: numbers and booleans as such, the rest as strings.
fn typed(v: &str) -> Json {
    match v {
        "true" => Json::Bool(true),
        "false" => Json::Bool(false),
        _ => v.parse::<u64>().map(Json::from).unwrap_or_else(|_| Json::String(v.to_owned())),
    }
}

fn plain(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub(super) fn read(root: &Path, out: &mut Json) {
    let per_project = read_component_file(&config_file(root))
        .and_then(|c| c.child("state").and_then(|s| s.option("USE_PER_PROJECT_SETTINGS")).map(|v| v == "true"))
        .unwrap_or(false);
    if !per_project {
        return;
    }
    let Some(file) = read_component_file(&project_file(root)) else { return };
    let Some(scheme) = file.child("code_scheme") else { return };
    for (setting, option) in SCALARS {
        if let Some(v) = scheme.option(option) {
            // the formatter tags are strings even when they look like numbers
            let value = if setting.ends_with("Tag") { Json::String(v.to_owned()) } else { typed(v) };
            put(out, &format!("{BASE}.{setting}"), value);
        }
    }
    if let Some(sep) = scheme.option("LINE_SEPARATOR") {
        let name = SEPARATORS.iter().find(|(_, s)| *s == sep).map(|(n, _)| *n).unwrap_or("system");
        put(out, &format!("{BASE}.lineSeparator"), json!(name));
    }
    if let Some(m) = scheme.option("SOFT_MARGINS") {
        let guides: Vec<Json> = m.split(',').filter_map(|g| g.trim().parse::<u64>().ok()).map(Json::from).collect();
        put(out, &format!("{BASE}.visualGuides"), Json::Array(guides));
    }
    if let Some(list) = scheme.option_node("DO_NOT_FORMAT").and_then(|o| o.child("list")) {
        let patterns: Vec<Json> =
            list.elements().filter_map(|f| f.attr("pattern")).map(|p| Json::String(p.to_owned())).collect();
        put(out, &format!("{BASE}.doNotFormat"), Json::Array(patterns));
    }
    if let Some(ec) = scheme.child("EditorConfig").and_then(|e| e.option("ENABLED")) {
        put(out, &format!("{BASE}.editorconfig"), json!(ec == "true"));
    }
    for lang in scheme.elements().filter(|e| e.name == "codeStyleSettings") {
        let Some(id) = lang.attr("language").and_then(|l| LANGUAGES.iter().find(|(_, i)| *i == l)).map(|(o, _)| *o)
        else {
            continue;
        };
        for (setting, option, indent) in LANGUAGE_OPTIONS {
            let holder = if *indent { lang.child("indentOptions") } else { Some(lang) };
            if let Some(v) = holder.and_then(|h| h.option(option)) {
                put(out, &format!("{BASE}.languages.{id}.{setting}"), typed(v));
            }
        }
    }
}

pub(super) fn write(w: &Writer<'_>) -> IdeResult<()> {
    let key = |s: &str| format!("{BASE}.{s}");
    update_component_file(&project_file(w.root), COMPONENT, |c| {
        let scheme = c.child_or_insert("code_scheme");
        if scheme.attr("name").is_none() {
            scheme.set_attr("name", "Project");
            scheme.set_attr("version", "173");
        }
        let sep = w.changed(&key("lineSeparator")).and_then(Json::as_str);
        scheme
            .set_option("LINE_SEPARATOR", sep.and_then(|s| SEPARATORS.iter().find(|(n, _)| *n == s)).map(|(_, v)| *v));
        for (setting, option) in SCALARS {
            scheme.set_option(option, w.changed(&key(setting)).map(plain).as_deref());
        }
        let guides = w
            .changed(&key("visualGuides"))
            .and_then(Json::as_array)
            .map(|g| g.iter().map(plain).collect::<Vec<_>>().join(","));
        scheme.set_option("SOFT_MARGINS", guides.as_deref());
        match w.changed(&key("doNotFormat")).and_then(Json::as_array) {
            Some(patterns) => {
                let o = scheme.option_node_mut("DO_NOT_FORMAT");
                let mut list = Element::new("list");
                for p in patterns.iter().filter_map(Json::as_str) {
                    list.push(Element::new("fileSet").with_attr("type", "globPattern").with_attr("pattern", p));
                }
                o.children = vec![ide_core::idea_xml::Node::Element(list)];
            }
            None => scheme.set_option("DO_NOT_FORMAT", None),
        }
        match w.changed(&key("editorconfig")).and_then(Json::as_bool) {
            Some(on) => scheme.child_or_insert("EditorConfig").set_option("ENABLED", Some(bool_str(on))),
            None => scheme.retain_elements(|e| e.name != "EditorConfig"),
        }
        let langs = w.get(&key("languages")).and_then(Json::as_object);
        for (ours, theirs) in LANGUAGES {
            let style = langs.and_then(|l| l.get(*ours));
            let existing =
                scheme.elements().any(|e| e.name == "codeStyleSettings" && e.attr("language") == Some(theirs));
            if style.is_none() && !existing {
                continue;
            }
            if !existing {
                scheme.push(Element::new("codeStyleSettings").with_attr("language", *theirs));
            }
            let el = scheme
                .elements_mut()
                .find(|e| e.name == "codeStyleSettings" && e.attr("language") == Some(theirs))
                .expect("just ensured");
            for (setting, option, indent) in LANGUAGE_OPTIONS {
                let v = style.and_then(|s| get(s, setting)).filter(|v| !v.is_null()).map(plain);
                if *indent {
                    if v.is_some() || el.child("indentOptions").is_some() {
                        el.child_or_insert("indentOptions").set_option(option, v.as_deref());
                    }
                } else {
                    el.set_option(option, v.as_deref());
                }
            }
            el.retain_elements(|e| !(e.name == "indentOptions" && e.children.is_empty()));
        }
        scheme.retain_elements(|e| !(e.name == "codeStyleSettings" && e.children.is_empty()));
        if scheme.children.is_empty() {
            c.retain_elements(|e| e.name != "code_scheme");
        }
    })?;
    update_component_file(&config_file(w.root), COMPONENT, |c| {
        c.child_or_insert("state").set_option("USE_PER_PROJECT_SETTINGS", Some("true"));
    })
}
