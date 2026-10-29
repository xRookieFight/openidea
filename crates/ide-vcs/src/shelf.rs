//! The shelf: named patches of local changes kept outside git, in IntelliJ's
//! `.idea/shelf` layout. Each shelved change list `<id>` is a descriptor
//! `<shelf>/<id>.xml` (name, date) plus its patch `<shelf>/<id>/shelved.patch`;
//! the files are read back from the patch.

use crate::cli::run;
use crate::{ApplyOutcome, ChangeKind, Repo, ShelvedChangeList};
use ide_core::idea_xml::{self, Element};
use ide_core::{IdeError, IdeResult};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const PATCH: &str = "shelved.patch";
const LEGACY_INFO: &str = "shelf.json";
const BRANCH: &str = "openidea.branch";

/// What older OpenIDEA versions kept next to the patch.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyInfo {
    name: String,
    time_ms: f64,
    #[serde(default)]
    branch: Option<String>,
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

fn slug(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    s.truncate(s.char_indices().nth(60).map(|(i, _)| i).unwrap_or(s.len()));
    if s.is_empty() { "shelf".to_string() } else { s }
}

fn entry_dir(shelf: &Path, id: &str) -> IdeResult<PathBuf> {
    if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
        return Err(IdeError::invalid(format!("invalid shelf id '{id}'")));
    }
    let dir = shelf.join(id);
    if !dir.join(PATCH).is_file() {
        return Err(IdeError::not_found(format!("shelved changes '{id}' not found")));
    }
    Ok(dir)
}

fn descriptor(shelf: &Path, id: &str) -> PathBuf {
    shelf.join(format!("{id}.xml"))
}

/// The project root when `shelf` is `<root>/.idea/shelf`, for `$PROJECT_DIR$` paths.
fn project_root(shelf: &Path) -> Option<&Path> {
    let idea = shelf.parent()?;
    (idea.file_name()? == ".idea").then(|| idea.parent()).flatten()
}

fn read_descriptor(shelf: &Path, id: &str) -> Option<Element> {
    idea_xml::read_project_file(&descriptor(shelf, id)).ok().flatten()
}

fn write_descriptor(shelf: &Path, e: &Element) -> IdeResult<()> {
    let id = e.attr("name").unwrap_or_default();
    idea_xml::write_atomic(&descriptor(shelf, id), &e.to_xml())
}

fn files_of(patch: &str) -> Vec<crate::ChangedFile> {
    crate::patch::parse_patch(patch)
        .into_iter()
        .map(|f| crate::ChangedFile { path: f.path, kind: f.kind, old_path: f.old_path })
        .collect()
}

fn free_id(shelf: &Path, base: &str) -> String {
    let mut id = base.to_owned();
    let mut n = 1;
    while shelf.join(&id).exists() || descriptor(shelf, &id).exists() {
        n += 1;
        id = format!("{base}_{n}");
    }
    id
}

/// Writes one entry: the patch, then its descriptor.
fn write_entry(shelf: &Path, id: &str, name: &str, time_ms: f64, branch: Option<&str>, patch: &str) -> IdeResult<()> {
    let dir = shelf.join(id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(PATCH), patch.as_bytes())?;
    let patch_path = dir.join(PATCH).to_string_lossy().into_owned();
    let path = match project_root(shelf) {
        Some(root) => idea_xml::collapse_path(root, &patch_path),
        None => patch_path.replace('\\', "/"),
    };
    let mut e = Element::new("changelist")
        .with_attr("name", id)
        .with_attr("date", format!("{}", time_ms as i64))
        .with_attr("recycled", "false")
        .with_attr("toDelete", "false");
    e.set_option("PATH", Some(&path));
    e.set_option("DESCRIPTION", Some(name));
    e.set_option(BRANCH, branch);
    write_descriptor(shelf, &e)
}

/// Shelved change lists, newest first. IntelliJ's recycled ("recently deleted") ones are left out.
pub fn list(shelf: &Path) -> IdeResult<Vec<ShelvedChangeList>> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(shelf) else { return Ok(out) };
    for e in entries.flatten() {
        let dir = e.path();
        if !dir.join(PATCH).is_file() {
            continue;
        }
        let id = e.file_name().to_string_lossy().into_owned();
        let desc = read_descriptor(shelf, &id);
        if let Some(d) = &desc
            && (d.attr("recycled") == Some("true") || d.attr("toDelete") == Some("true"))
        {
            continue;
        }
        let name = desc.as_ref().and_then(|d| d.option("DESCRIPTION")).filter(|n| !n.is_empty()).unwrap_or(&id);
        out.push(ShelvedChangeList {
            id: id.clone(),
            name: name.to_owned(),
            time_ms: desc.as_ref().and_then(|d| d.attr("date")?.parse().ok()).unwrap_or(0.0),
            branch: desc.as_ref().and_then(|d| d.option(BRANCH)).map(str::to_owned),
            files: std::fs::read_to_string(dir.join(PATCH)).map(|p| files_of(&p)).unwrap_or_default(),
        });
    }
    out.sort_by(|a, b| b.time_ms.total_cmp(&a.time_ms).then_with(|| a.name.cmp(&b.name)));
    Ok(out)
}

/// The patch text of a shelved change list.
pub fn patch_of(shelf: &Path, id: &str) -> IdeResult<String> {
    Ok(std::fs::read_to_string(entry_dir(shelf, id)?.join(PATCH))?)
}

pub fn delete(shelf: &Path, id: &str) -> IdeResult<()> {
    std::fs::remove_dir_all(entry_dir(shelf, id)?)?;
    match std::fs::remove_file(descriptor(shelf, id)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

pub fn rename(shelf: &Path, id: &str, name: &str) -> IdeResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(IdeError::invalid("name must not be empty"));
    }
    entry_dir(shelf, id)?;
    match read_descriptor(shelf, id) {
        Some(mut d) => {
            d.set_option("DESCRIPTION", Some(name));
            write_descriptor(shelf, &d)
        }
        None => write_entry(shelf, id, name, now_ms(), None, &patch_of(shelf, id)?),
    }
}

/// Copies the entries of an old `.openidea/shelf` into `shelf`; returns how many.
pub fn import_legacy(legacy: &Path, shelf: &Path) -> IdeResult<usize> {
    let Ok(entries) = std::fs::read_dir(legacy) else { return Ok(0) };
    let mut n = 0;
    for e in entries.flatten() {
        let dir = e.path();
        let Ok(patch) = std::fs::read_to_string(dir.join(PATCH)) else { continue };
        let old_id = e.file_name().to_string_lossy().into_owned();
        let info: Option<LegacyInfo> =
            std::fs::read(dir.join(LEGACY_INFO)).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let (name, time, branch) = match info {
            Some(i) => (i.name, i.time_ms, i.branch),
            None => (old_id.clone(), now_ms(), None),
        };
        let id = free_id(shelf, &old_id);
        write_entry(shelf, &id, &name, time, branch.as_deref(), &patch)?;
        n += 1;
    }
    Ok(n)
}

/// Saves the changes of `rel_paths` to the shelf and reverts them in the working tree.
pub(crate) fn shelve(r: &Repo, shelf: &Path, name: &str, rel_paths: &[String]) -> IdeResult<ShelvedChangeList> {
    let name = name.trim();
    if name.is_empty() {
        return Err(IdeError::invalid("name must not be empty"));
    }
    let (patch, files) = crate::patch::working_patch(r, rel_paths, false)?;
    if files.is_empty() || patch.trim().is_empty() {
        return Err(IdeError::invalid("no changes to shelve"));
    }
    std::fs::create_dir_all(shelf)?;
    let id = free_id(shelf, &slug(name));
    let time_ms = now_ms();
    let branch = r.branch_info()?.head;
    if let Err(e) = write_entry(shelf, &id, name, time_ms, branch.as_deref(), &patch) {
        let _ = std::fs::remove_dir_all(shelf.join(&id));
        let _ = std::fs::remove_file(descriptor(shelf, &id));
        return Err(e);
    }
    revert_files(r, &files)?;
    Ok(ShelvedChangeList { id, name: name.to_string(), time_ms, branch, files })
}

/// Restores `files` to HEAD in both index and working tree; files absent from HEAD are removed.
fn revert_files(r: &Repo, files: &[crate::ChangedFile]) -> IdeResult<()> {
    let has_head = r.has_head()?;
    let (in_head, new): (Vec<_>, Vec<_>) = files.iter().partition(|f| has_head && f.kind != ChangeKind::Added);
    if !in_head.is_empty() {
        let mut args = vec!["--literal-pathspecs", "restore", "--source=HEAD", "--staged", "--worktree", "--"];
        args.extend(in_head.iter().map(|f| f.path.as_str()));
        run(r.root(), &args, None)?;
    }
    if !new.is_empty() {
        let mut args = vec!["--literal-pathspecs", "rm", "--cached", "-q", "-r", "--ignore-unmatch", "--"];
        args.extend(new.iter().map(|f| f.path.as_str()));
        run(r.root(), &args, None)?;
        for f in new {
            let abs = r.abs_path(&f.path);
            if abs.is_file() || abs.is_symlink() {
                std::fs::remove_file(&abs)?;
            }
        }
    }
    Ok(())
}

/// Applies a shelved change list; with `remove`, deletes it from the shelf once it
/// applied without conflicts (conflicts are left for the merge UI and the entry is kept).
pub(crate) fn unshelve(r: &Repo, shelf: &Path, id: &str, remove: bool) -> IdeResult<ApplyOutcome> {
    let patch = patch_of(shelf, id)?;
    let outcome = crate::patch::apply(r, &patch)?;
    if remove && outcome.conflicts.is_empty() {
        delete(shelf, id)?;
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH_TEXT: &str =
        "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-one\n+two\n";

    #[test]
    fn slugs() {
        assert_eq!(
            slug("Uncommitted changes before Update at 1/2/24, 10:00"),
            "Uncommitted_changes_before_Update_at_1_2_24_10_00"
        );
        assert_eq!(slug("  ../  "), "shelf");
        assert_eq!(slug("Фикс"), "Фикс");
    }

    #[test]
    fn lists_intellij_shelf() {
        let tmp = tempfile::tempdir().unwrap();
        let shelf = tmp.path().join(".idea/shelf");
        for (id, recycled) in [("Fix_login", "false"), ("Old_stuff", "true")] {
            std::fs::create_dir_all(shelf.join(id)).unwrap();
            std::fs::write(shelf.join(id).join(PATCH), PATCH_TEXT).unwrap();
            std::fs::write(
                shelf.join(format!("{id}.xml")),
                format!(
                    "<changelist name=\"{id}\" date=\"1700000000000\" recycled=\"{recycled}\" toDelete=\"false\">\n  \
                     <option name=\"PATH\" value=\"$PROJECT_DIR$/.idea/shelf/{id}/shelved.patch\" />\n  \
                     <option name=\"DESCRIPTION\" value=\"Fix login\" />\n</changelist>"
                ),
            )
            .unwrap();
        }
        let listed = list(&shelf).unwrap();
        assert_eq!(listed.len(), 1, "recycled entries stay hidden");
        assert_eq!(listed[0].id, "Fix_login");
        assert_eq!(listed[0].name, "Fix login");
        assert_eq!(listed[0].time_ms, 1_700_000_000_000.0);
        assert_eq!(listed[0].files[0].path, "src/a.rs");
    }

    #[test]
    fn entries_are_written_like_intellij() {
        let tmp = tempfile::tempdir().unwrap();
        let shelf = tmp.path().join(".idea/shelf");
        write_entry(&shelf, "wip", "Work in progress", 1_700_000_000_000.0, Some("main"), PATCH_TEXT).unwrap();
        let descriptor = std::fs::read_to_string(shelf.join("wip.xml")).unwrap();
        assert_eq!(
            descriptor,
            "<changelist name=\"wip\" date=\"1700000000000\" recycled=\"false\" toDelete=\"false\">\n  \
             <option name=\"PATH\" value=\"$PROJECT_DIR$/.idea/shelf/wip/shelved.patch\" />\n  \
             <option name=\"DESCRIPTION\" value=\"Work in progress\" />\n  \
             <option name=\"openidea.branch\" value=\"main\" />\n</changelist>"
        );
        rename(&shelf, "wip", "Renamed").unwrap();
        let listed = list(&shelf).unwrap();
        assert_eq!((listed[0].name.as_str(), listed[0].branch.as_deref()), ("Renamed", Some("main")));
        delete(&shelf, "wip").unwrap();
        assert!(!shelf.join("wip.xml").exists() && !shelf.join("wip").exists());
    }

    #[test]
    fn import_legacy_converts_json_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join(".openidea/shelf");
        std::fs::create_dir_all(legacy.join("wip")).unwrap();
        std::fs::write(legacy.join("wip").join(PATCH), PATCH_TEXT).unwrap();
        std::fs::write(
            legacy.join("wip").join("shelf.json"),
            r#"{"name":"My work","timeMs":5.0,"branch":"dev","files":[]}"#,
        )
        .unwrap();
        let shelf = tmp.path().join(".idea/shelf");
        // an IntelliJ entry with the same id already there
        write_entry(&shelf, "wip", "Theirs", 1.0, None, PATCH_TEXT).unwrap();
        assert_eq!(import_legacy(&legacy, &shelf).unwrap(), 1);
        let mut names: Vec<String> = list(&shelf).unwrap().into_iter().map(|e| e.name).collect();
        names.sort();
        assert_eq!(names, ["My work", "Theirs"]);
    }
}
