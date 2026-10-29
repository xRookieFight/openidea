//! The per-project config folder: IntelliJ's `<root>/.idea/`. OpenIDEA keeps
//! what IntelliJ has no file for in `<root>/.idea/openidea/`.

use crate::IdeResult;
use std::path::{Path, PathBuf};

/// IntelliJ's project folder under the root.
pub const IDEA: &str = ".idea";

/// OpenIDEA's own subfolder inside [`IDEA`].
pub const OWN: &str = "openidea";

/// The folder older OpenIDEA versions used. Only the migration reads it.
pub const DIR: &str = ".openidea";

/// Per-user files in `.idea`, kept out of version control. IntelliJ's own
/// default `.gitignore` lists the first two.
const IGNORED: [&str; 6] = [
    "/shelf/",
    "/workspace.xml",
    "/dataSources/",
    "/dataSources.local.xml",
    "/openidea/workspace.toml",
    "/openidea/migrated-openidea/",
];

/// Where OpenIDEA's own `rel` (e.g. `settings.toml`) is read from and written to.
pub fn path(root: &Path, rel: impl AsRef<Path>) -> PathBuf {
    root.join(IDEA).join(OWN).join(rel)
}

/// An IntelliJ file in `.idea`, e.g. `misc.xml`.
pub fn idea_path(root: &Path, rel: impl AsRef<Path>) -> PathBuf {
    root.join(IDEA).join(rel)
}

/// `rel` in the old `.openidea` folder.
pub fn legacy_path(root: &Path, rel: impl AsRef<Path>) -> PathBuf {
    root.join(DIR).join(rel)
}

/// Makes sure `.idea/.gitignore` lists the per-user files. Lines already there
/// stay as they are; a new file starts the way IntelliJ writes it.
pub fn ensure_gitignore(root: &Path) -> IdeResult<()> {
    let file = idea_path(root, ".gitignore");
    let current = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let missing: Vec<&str> = IGNORED.into_iter().filter(|e| !current.lines().any(|l| l.trim() == *e)).collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut text = current;
    if text.is_empty() {
        text.push_str("# Default ignored files\n");
    } else if !text.ends_with('\n') {
        text.push('\n');
    }
    for e in missing {
        text.push_str(e);
        text.push('\n');
    }
    crate::idea_xml::write_atomic(&file, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_live_under_idea() {
        let root = Path::new("/p");
        assert_eq!(path(root, "settings.toml"), Path::new("/p/.idea/openidea/settings.toml"));
        assert_eq!(idea_path(root, "misc.xml"), Path::new("/p/.idea/misc.xml"));
        assert_eq!(legacy_path(root, "run.toml"), Path::new("/p/.openidea/run.toml"));
    }

    #[test]
    fn gitignore_appends_missing_entries_once() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".idea/.gitignore");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "# Default ignored files\n/shelf/\n/workspace.xml\n").unwrap();
        ensure_gitignore(tmp.path()).unwrap();
        ensure_gitignore(tmp.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "# Default ignored files\n/shelf/\n/workspace.xml\n/dataSources/\n/dataSources.local.xml\n\
             /openidea/workspace.toml\n/openidea/migrated-openidea/\n"
        );
    }

    #[test]
    fn new_gitignore_starts_with_intellij_header() {
        let tmp = tempfile::tempdir().unwrap();
        ensure_gitignore(tmp.path()).unwrap();
        let text = std::fs::read_to_string(tmp.path().join(".idea/.gitignore")).unwrap();
        assert!(text.starts_with("# Default ignored files\n/shelf/\n/workspace.xml\n"), "{text}");
    }
}
