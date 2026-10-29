//! Change lists (IDE-side grouping of local changes), kept where IntelliJ keeps
//! them: the `ChangeListManager` component of `.idea/workspace.xml`. Which list
//! a file belongs to is decided by the UI; this module only loads and saves the state.

use crate::{Changelist, ChangelistState};
use ide_core::idea_xml::{self, Element};
use ide_core::{IdeResult, project_dir};
use std::path::Path;

pub const DEFAULT_ID: &str = "default";
pub const DEFAULT_NAME: &str = "Changes";

impl ChangelistState {
    /// The default "Changes" list, active, with no file assignments.
    pub fn initial() -> Self {
        ChangelistState {
            lists: vec![Changelist { id: DEFAULT_ID.into(), name: DEFAULT_NAME.into(), comment: String::new() }],
            active: DEFAULT_ID.into(),
            files: Default::default(),
        }
    }

    /// Repairs hand-edited or stale state: at least one list, a valid active list,
    /// unique ids, assignments only to existing lists.
    fn normalized(mut self) -> Self {
        let mut seen = std::collections::HashSet::new();
        self.lists.retain(|l| !l.id.is_empty() && seen.insert(l.id.clone()));
        if self.lists.is_empty() {
            return ChangelistState { files: Default::default(), ..Self::initial() };
        }
        if !self.lists.iter().any(|l| l.id == self.active) {
            self.active = self.lists[0].id.clone();
        }
        let ids: std::collections::HashSet<String> = self.lists.iter().map(|l| l.id.clone()).collect();
        self.files.retain(|_, id| ids.contains(id));
        self
    }
}

const COMPONENT: &str = "ChangeListManager";

fn workspace(root: &Path) -> std::path::PathBuf {
    project_dir::idea_path(root, "workspace.xml")
}

/// The lists in `.idea/workspace.xml`'s `ChangeListManager`, the default
/// "Changes" list when there are none (or the file can't be read).
pub fn load(root: &Path) -> ChangelistState {
    let project = match idea_xml::read_project_file(&workspace(root)) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("cannot read changelists: {}", e.message);
            None
        }
    };
    let Some(manager) = project.as_ref().and_then(|p| idea_xml::component(p, COMPONENT)) else {
        return ChangelistState::initial();
    };
    let mut state = ChangelistState { lists: Vec::new(), active: String::new(), files: Default::default() };
    for list in manager.elements().filter(|e| e.name == "list") {
        let id = list.attr("id").unwrap_or_default().to_owned();
        if list.attr("default") == Some("true") {
            state.active = id.clone();
        }
        for change in list.elements().filter(|e| e.name == "change") {
            let Some(path) = change.attr("afterPath").or(change.attr("beforePath")) else { continue };
            let abs = idea_xml::expand_path(root, path);
            if let Ok(rel) = Path::new(&abs).strip_prefix(root) {
                state.files.insert(rel.to_string_lossy().replace('\\', "/"), id.clone());
            }
        }
        state.lists.push(Changelist {
            id,
            name: list.attr("name").unwrap_or_default().to_owned(),
            comment: list.attr("comment").unwrap_or_default().to_owned(),
        });
    }
    state.normalized()
}

/// The pre-`.idea` `changelists.json` (migration only); `None` when there is none.
pub fn legacy_load(file: &Path) -> Option<ChangelistState> {
    let bytes = std::fs::read(file).ok()?;
    Some(
        serde_json::from_slice::<ChangelistState>(&bytes)
            .map(ChangelistState::normalized)
            .unwrap_or_else(|_| ChangelistState::initial()),
    )
}

/// Writes the lists into `workspace.xml`, keeping the component's options and
/// everything else IntelliJ keeps in that file.
pub fn save(root: &Path, state: &ChangelistState) -> IdeResult<()> {
    let state = state.clone().normalized();
    idea_xml::update_project_file(&workspace(root), |p| {
        let manager = idea_xml::component_mut(p, COMPONENT);
        let at = manager
            .children
            .iter()
            .position(|c| matches!(c, idea_xml::Node::Element(e) if e.name == "list"))
            .unwrap_or(0);
        manager.retain_elements(|e| e.name != "list");
        for (i, l) in state.lists.iter().enumerate() {
            let mut list = Element::new("list");
            if l.id == state.active {
                list.set_attr("default", "true");
            }
            list.set_attr("id", l.id.as_str());
            list.set_attr("name", l.name.as_str());
            list.set_attr("comment", l.comment.as_str());
            for (rel, _) in state.files.iter().filter(|(_, id)| **id == l.id) {
                let path = idea_xml::collapse_path(root, &root.join(rel).to_string_lossy());
                list.push(
                    Element::new("change")
                        .with_attr("beforePath", path.as_str())
                        .with_attr("beforeDir", "false")
                        .with_attr("afterPath", path.as_str())
                        .with_attr("afterDir", "false"),
                );
            }
            manager.children.insert(at + i, idea_xml::Node::Element(list));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(root: &Path) -> std::path::PathBuf {
        root.join(".idea/workspace.xml")
    }

    const INTELLIJ: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project version="4">
  <component name="ChangeListManager">
    <list id="0d7c" name="Feature" comment="wip" />
    <list default="true" id="a1b2" name="Changes" comment="">
      <change afterPath="$PROJECT_DIR$/src/a.rs" afterDir="false" />
      <change beforePath="$PROJECT_DIR$/gone.rs" beforeDir="false" />
      <change beforePath="/elsewhere/x.rs" beforeDir="false" afterPath="/elsewhere/x.rs" afterDir="false" />
    </list>
    <option name="HIGHLIGHT_CONFLICTS" value="true" />
  </component>
</project>"#;

    #[test]
    fn reads_intellij_changelists() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".idea")).unwrap();
        std::fs::write(workspace(tmp.path()), INTELLIJ).unwrap();
        let st = load(tmp.path());
        assert_eq!(st.lists.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Feature", "Changes"]);
        assert_eq!(st.lists[0].comment, "wip");
        assert_eq!(st.active, "a1b2");
        assert_eq!(st.files.get("src/a.rs").map(String::as_str), Some("a1b2"));
        assert_eq!(st.files.get("gone.rs").map(String::as_str), Some("a1b2"));
        assert_eq!(st.files.len(), 2, "files outside the project are skipped");
    }

    #[test]
    fn save_keeps_component_options() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".idea")).unwrap();
        std::fs::write(workspace(tmp.path()), INTELLIJ).unwrap();
        let mut st = load(tmp.path());
        st.active = "0d7c".into();
        st.files.insert("b.rs".into(), "0d7c".into());
        save(tmp.path(), &st).unwrap();
        let text = std::fs::read_to_string(workspace(tmp.path())).unwrap();
        assert!(text.contains(r#"<option name="HIGHLIGHT_CONFLICTS" value="true" />"#), "{text}");
        assert!(
            text.contains(r#"<change beforePath="$PROJECT_DIR$/b.rs" beforeDir="false" afterPath="$PROJECT_DIR$/b.rs" afterDir="false" />"#),
            "{text}"
        );
        assert!(text.contains(r#"<list default="true" id="0d7c" name="Feature" comment="wip">"#), "{text}");
        assert_eq!(load(tmp.path()), st.clone().normalized());
    }

    #[test]
    fn missing_component_is_initial_and_repaired() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(tmp.path()), ChangelistState::initial());

        let mut st = ChangelistState::initial();
        st.lists.push(Changelist { id: "f".into(), name: "Feature".into(), comment: String::new() });
        st.active = "f".into();
        st.files.insert("src/a.rs".into(), "f".into());
        st.files.insert("b.rs".into(), "missing".into());
        save(tmp.path(), &st).unwrap();
        let back = load(tmp.path());
        assert_eq!(back.active, "f");
        assert_eq!(back.files.get("src/a.rs").map(String::as_str), Some("f"));
        assert!(!back.files.contains_key("b.rs"));
    }

    #[test]
    fn legacy_json_still_reads() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("changelists.json");
        assert!(legacy_load(&file).is_none());
        std::fs::write(&file, r#"{"lists":[],"active":"x","files":{}}"#).unwrap();
        assert_eq!(legacy_load(&file).unwrap().active, DEFAULT_ID);
    }
}
