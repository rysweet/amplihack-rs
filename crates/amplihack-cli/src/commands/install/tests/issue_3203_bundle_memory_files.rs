//! Issue #3203: no file Claude Code loads as memory sits under
//! `amplifier-bundle/`, in the checkout or in the staged copy, and the
//! installer still stages `$AMPLIHACK_HOME/CLAUDE.md`.
//!
//! Claude Code inserts a `CLAUDE.md` or `AGENTS.md` as nested memory when a
//! session reads a file in that file's folder. With none under the bundle, a
//! session that reads a recipe or any other bundle file is given no memory
//! text from it. The source of `$AMPLIHACK_HOME/CLAUDE.md` is
//! `amplifier-bundle/context/AMPLIHACK_HOME_CLAUDE.md`.

use super::helpers;
use super::issue_527_tests::with_install_env;
use super::*;
use std::fs;
use std::path::{Path, PathBuf};

/// The file names Claude Code loads as memory from a folder a session reads.
const MEMORY_FILE_NAMES: &[&str] = &["CLAUDE.md", "AGENTS.md"];

/// Every memory-named entry under `root`, symlinks included. A symlinked
/// directory is not descended into.
fn memory_files_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries =
            fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| MEMORY_FILE_NAMES.contains(&name))
            {
                found.push(path.clone());
            }
            if fs::symlink_metadata(&path).unwrap().file_type().is_dir() {
                pending.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn the_checkout_bundle_holds_no_memory_file() {
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle");
    assert!(bundle.is_dir(), "missing {}", bundle.display());
    assert_eq!(
        memory_files_under(&bundle),
        Vec::<PathBuf>::new(),
        "issue #3203: a file under amplifier-bundle/ is loaded as memory by any \
         session that reads a bundle file"
    );
}

#[test]
fn install_stages_home_claude_md_and_no_bundle_memory_file() {
    if crate::test_support::isolated_install_test() {
        return;
    }

    with_install_env(|home, bin_dir| {
        helpers::create_exe_stub(bin_dir, "recipe-runner-rs");

        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        helpers::create_bundle_only_source_repo(&repo);
        for old_source in [
            repo.join("CLAUDE.md"),
            repo.join("amplifier-bundle/CLAUDE.md"),
        ] {
            if old_source.exists() {
                fs::remove_file(&old_source).unwrap();
            }
        }
        let content = "# Amplihack\n\nAMPLIHACK_HOME CLAUDE.md fixture for issue #3203.\n";
        fs::write(
            repo.join("amplifier-bundle/context/AMPLIHACK_HOME_CLAUDE.md"),
            content,
        )
        .unwrap();

        local_install(&repo, None).expect(
            "issue #3203: install must succeed with CLAUDE.md's source at \
             amplifier-bundle/context/AMPLIHACK_HOME_CLAUDE.md",
        );

        assert_eq!(
            fs::read_to_string(home.join(".amplihack/CLAUDE.md")).unwrap(),
            content,
            "issue #3203: $AMPLIHACK_HOME/CLAUDE.md holds the renamed source"
        );
        // The published bundle is a link into releases/<release>/.
        let staged_bundle = fs::canonicalize(home.join(".amplihack/amplifier-bundle")).unwrap();
        assert_eq!(
            memory_files_under(&staged_bundle),
            Vec::<PathBuf>::new(),
            "issue #3203: the staged bundle must hold no file loaded as memory"
        );
    });
}
