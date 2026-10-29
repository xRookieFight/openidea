//! IntelliJ's project files (`.idea/*.xml`): a small DOM that parses them and
//! writes them back the way IntelliJ does, so a file we touch diffs only where
//! we changed it. Every feature edits its own components through
//! [`update_project_file`] and leaves the rest alone.

use crate::{IdeError, IdeResult};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Element {
    pub name: String,
    /// In file order: IntelliJ never sorts attributes, and neither do we.
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Element {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), ..Default::default() }
    }

    /// Builder-style `set_attr`, handy when assembling new elements.
    pub fn with_attr(mut self, k: &str, v: impl Into<String>) -> Self {
        self.set_attr(k, v);
        self
    }

    pub fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    /// Replaces the value in place, or appends the attribute.
    pub fn set_attr(&mut self, k: &str, v: impl Into<String>) {
        let v = v.into();
        match self.attrs.iter_mut().find(|(n, _)| n == k) {
            Some((_, old)) => *old = v,
            None => self.attrs.push((k.to_owned(), v)),
        }
    }

    pub fn remove_attr(&mut self, k: &str) {
        self.attrs.retain(|(n, _)| n != k);
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|c| match c {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    pub fn elements_mut(&mut self) -> impl Iterator<Item = &mut Element> {
        self.children.iter_mut().filter_map(|c| match c {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    pub fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.name == name)
    }

    pub fn child_mut(&mut self, name: &str) -> Option<&mut Element> {
        self.elements_mut().find(|e| e.name == name)
    }

    pub fn child_or_insert(&mut self, name: &str) -> &mut Element {
        if self.child(name).is_none() {
            self.push(Element::new(name));
        }
        self.child_mut(name).expect("just inserted")
    }

    pub fn push(&mut self, e: Element) {
        self.children.push(Node::Element(e));
    }

    /// Drops the child elements `keep` says no to; text is kept.
    pub fn retain_elements(&mut self, mut keep: impl FnMut(&Element) -> bool) {
        self.children.retain(|c| match c {
            Node::Element(e) => keep(e),
            Node::Text(_) => true,
        });
    }

    pub fn text(&self) -> String {
        self.children
            .iter()
            .filter_map(|c| match c {
                Node::Text(t) => Some(t.as_str()),
                Node::Element(_) => None,
            })
            .collect()
    }

    pub fn set_text(&mut self, v: impl Into<String>) {
        self.children = vec![Node::Text(v.into())];
    }

    fn option_element(&self, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.name == "option" && e.attr("name") == Some(name))
    }

    fn option_element_mut(&mut self, name: &str) -> &mut Element {
        if self.option_element(name).is_none() {
            self.push(Element::new("option").with_attr("name", name));
        }
        self.elements_mut().find(|e| e.name == "option" && e.attr("name") == Some(name)).expect("just inserted")
    }

    /// `value` of `<option name="…" value="…" />`.
    pub fn option(&self, name: &str) -> Option<&str> {
        self.option_element(name)?.attr("value")
    }

    /// Sets `<option name="…" value="…" />`; `None` removes it.
    pub fn set_option(&mut self, name: &str, value: Option<&str>) {
        match value {
            Some(v) => {
                let o = self.option_element_mut(name);
                o.set_attr("value", v);
            }
            None => self.retain_elements(|e| !(e.name == "option" && e.attr("name") == Some(name))),
        }
    }

    /// Values of `<option name="…"><list><option value="…" /></list></option>`.
    pub fn option_list(&self, name: &str) -> Vec<String> {
        self.option_element(name)
            .and_then(|o| o.child("list"))
            .map(|l| l.elements().filter_map(|e| e.attr("value")).map(str::to_owned).collect())
            .unwrap_or_default()
    }

    pub fn set_option_list(&mut self, name: &str, values: &[String]) {
        let mut list = Element::new("list");
        for v in values {
            list.push(Element::new("option").with_attr("value", v.as_str()));
        }
        let o = self.option_element_mut(name);
        o.remove_attr("value");
        o.children = vec![Node::Element(list)];
    }

    /// The whole file: XML declaration plus this element, the way IntelliJ writes it.
    pub fn to_idea_string(&self) -> String {
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        out.push_str(&pad);
        out.push('<');
        out.push_str(&self.name);
        for (k, v) in &self.attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            escape_attr(v, out);
            out.push('"');
        }
        if self.children.is_empty() {
            out.push_str(" />");
            return;
        }
        out.push('>');
        if self.children.iter().all(|c| matches!(c, Node::Text(_))) {
            escape_text(&self.text(), out);
        } else {
            for c in &self.children {
                out.push('\n');
                match c {
                    Node::Element(e) => e.write(out, depth + 1),
                    Node::Text(t) => {
                        out.push_str(&"  ".repeat(depth + 1));
                        escape_text(t, out);
                    }
                }
            }
            out.push('\n');
            out.push_str(&pad);
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push('>');
    }
}

fn escape_attr(v: &str, out: &mut String) {
    for c in v.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
}

fn escape_text(v: &str, out: &mut String) {
    for c in v.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
}

/// `&amp;`, `&#10;`, `&#x9;`… back to characters. Unknown entities stay as written.
fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        match resolve_entity(&rest[1..end]) {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn resolve_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let code = if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                name.strip_prefix('#')?.parse().ok()?
            };
            char::from_u32(code)
        }
    }
}

fn bad(e: impl std::fmt::Display) -> IdeError {
    IdeError::invalid(format!("invalid XML: {e}"))
}

fn start(e: &quick_xml::events::BytesStart<'_>) -> IdeResult<Element> {
    let mut el = Element::new(e.name().as_ref());
    for a in e.attributes() {
        let a = a.map_err(bad)?;
        // the raw value: entities are still in there
        el.attrs.push((a.key.as_ref().to_owned(), unescape(&a.value)));
    }
    Ok(el)
}

/// Parses a project file and returns its root element. Comments, the XML
/// declaration and indentation whitespace are dropped.
pub fn parse(text: &str) -> IdeResult<Element> {
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Element> = Vec::new();
    let mut root = None;
    // yeah, quick-xml hands entity references over as separate events, so text is
    // collected raw and unescaped in one go when the next tag shows up
    let mut pending = String::new();
    let flush = |pending: &mut String, stack: &mut Vec<Element>| {
        if !pending.trim().is_empty()
            && let Some(top) = stack.last_mut()
        {
            top.children.push(Node::Text(unescape(pending)));
        }
        pending.clear();
    };
    loop {
        match reader.read_event().map_err(bad)? {
            Event::Start(e) => {
                flush(&mut pending, &mut stack);
                stack.push(start(&e)?);
            }
            Event::Empty(e) => {
                flush(&mut pending, &mut stack);
                let el = start(&e)?;
                match stack.last_mut() {
                    Some(top) => top.push(el),
                    None => root = Some(el),
                }
            }
            Event::End(_) => {
                flush(&mut pending, &mut stack);
                let el = stack.pop().ok_or_else(|| bad("unbalanced end tag"))?;
                match stack.last_mut() {
                    Some(top) => top.push(el),
                    None => root = Some(el),
                }
            }
            Event::Text(t) => pending.push_str(&t),
            Event::CData(t) => pending.push_str(&t.replace('&', "&amp;").replace('<', "&lt;")),
            Event::GeneralRef(r) => {
                pending.push('&');
                pending.push_str(&r);
                pending.push(';');
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(bad("unclosed element"));
    }
    root.ok_or_else(|| bad("no root element"))
}

/// The `<component name="…">` of a project file.
pub fn component<'a>(project: &'a Element, name: &str) -> Option<&'a Element> {
    project.elements().find(|e| e.name == "component" && e.attr("name") == Some(name))
}

/// Like [`component`], inserting an empty one where IntelliJ would put it
/// (components are kept sorted by name).
pub fn component_mut<'a>(project: &'a mut Element, name: &str) -> &'a mut Element {
    let is = |e: &Element| e.name == "component" && e.attr("name") == Some(name);
    let at = match project.children.iter().position(|c| matches!(c, Node::Element(e) if is(e))) {
        Some(i) => i,
        None => {
            let i = project
                .children
                .iter()
                .position(|c| {
                    matches!(c, Node::Element(e) if e.name == "component" && e.attr("name").is_some_and(|n| n > name))
                })
                .unwrap_or(project.children.len());
            project.children.insert(i, Node::Element(Element::new("component").with_attr("name", name)));
            i
        }
    };
    match &mut project.children[at] {
        Node::Element(e) => e,
        Node::Text(_) => unreachable!("position matched an element"),
    }
}

/// The root of the project file at `path`; `None` when there is none.
pub fn read_project_file(path: &Path) -> IdeResult<Option<Element>> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map(Some).map_err(|e| IdeError::invalid(format!("{}: {}", path.display(), e.message))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Read-modify-write of one project file. Always reads the file fresh (IntelliJ
/// may have rewritten it a second ago), writes atomically, and skips the write
/// when nothing changed. A missing or broken file starts as `<project version="4" />`.
pub fn update_project_file(path: &Path, f: impl FnOnce(&mut Element)) -> IdeResult<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut project = existing
        .as_deref()
        .and_then(|t| {
            parse(t)
                .inspect_err(|e| tracing::warn!("{} is not valid XML, replacing it: {}", path.display(), e.message))
                .ok()
        })
        .unwrap_or_else(|| Element::new("project").with_attr("version", "4"));
    f(&mut project);
    if existing.is_none() && project.children.is_empty() {
        return Ok(());
    }
    let text = project.to_idea_string();
    if existing.as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    write_atomic(path, &text)
}

/// Temp file + rename, creating the parent folders.
pub fn write_atomic(path: &Path, text: &str) -> IdeResult<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

fn slashed(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn with_macro(path: &str, base: &str, name: &str) -> Option<String> {
    if base.is_empty() {
        return None;
    }
    if path == base {
        return Some(name.to_owned());
    }
    let rest = path.strip_prefix(base)?.strip_prefix('/')?;
    Some(format!("{name}/{rest}"))
}

/// `$PROJECT_DIR$/…` for paths in the project, `$USER_HOME$/…` under the home folder.
pub fn collapse_path(root: &Path, path: &str) -> String {
    let path = path.replace('\\', "/");
    if let Some(p) = with_macro(&path, &slashed(root), "$PROJECT_DIR$") {
        return p;
    }
    if let Some(home) = dirs::home_dir()
        && let Some(p) = with_macro(&path, &slashed(&home), "$USER_HOME$")
    {
        return p;
    }
    path
}

/// The inverse of [`collapse_path`]; values without macros come back unchanged.
pub fn expand_path(root: &Path, value: &str) -> String {
    let mut out = value.replace("$PROJECT_DIR$", &slashed(root));
    if out.contains("$USER_HOME$")
        && let Some(home) = dirs::home_dir()
    {
        out = out.replace("$USER_HOME$", &slashed(&home));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn round_trips_intellij_output_unchanged() {
        let text = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<project version=\"4\">\n  <component name=\"A\">\n    <option name=\"X\" value=\"a&quot;b&#10;c\" />\n    <list>\n      <item itemvalue=\"Shell Script.run\" />\n    </list>\n    <text>hi &amp; bye</text>\n  </component>\n</project>";
        assert_eq!(parse(text).unwrap().to_idea_string(), text);
    }

    #[test]
    fn component_mut_inserts_sorted() {
        let mut p = parse("<project version=\"4\"><component name=\"B\" /><component name=\"D\" /></project>").unwrap();
        component_mut(&mut p, "C");
        let names: Vec<_> = p.elements().map(|e| e.attr("name").unwrap()).collect();
        assert_eq!(names, ["B", "C", "D"]);
    }

    #[test]
    fn option_list_round_trip() {
        let mut e = Element::new("settings");
        e.set_option_list("taskNames", &["clean".into(), "build".into()]);
        assert_eq!(e.option_list("taskNames"), ["clean", "build"]);
    }

    #[test]
    fn update_keeps_concurrent_component() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("workspace.xml");
        update_project_file(&file, |p| {
            component_mut(p, "Ours").set_option("v", Some("1"));
        })
        .unwrap();
        // IntelliJ adds its own component between our writes.
        let mut p = read_project_file(&file).unwrap().unwrap();
        component_mut(&mut p, "ToolWindowManager").set_option("x", Some("y"));
        std::fs::write(&file, p.to_idea_string()).unwrap();
        update_project_file(&file, |p| {
            component_mut(p, "Ours").set_option("v", Some("2"));
        })
        .unwrap();
        let p = read_project_file(&file).unwrap().unwrap();
        assert_eq!(component(&p, "ToolWindowManager").unwrap().option("x"), Some("y"));
        assert_eq!(component(&p, "Ours").unwrap().option("v"), Some("2"));
    }

    #[test]
    fn macros() {
        let root = Path::new("/w/proj");
        assert_eq!(collapse_path(root, "/w/proj/src/a.rs"), "$PROJECT_DIR$/src/a.rs");
        assert_eq!(collapse_path(root, "/w/proj"), "$PROJECT_DIR$");
        assert_eq!(expand_path(root, "$PROJECT_DIR$/src"), "/w/proj/src");
    }
}
