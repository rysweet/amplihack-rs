//! Behaviour of `amplihack directory find` and `list` against an index file.

use super::*;
use std::fs;
use std::path::PathBuf;

fn staged_with_index(text: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let staged = dir.path().join(".amplihack/.claude");
    let path = index::index_path(&staged);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    (dir, staged)
}

const INDEX: &str = "agent\tguide\t/h/agents/guide.md\tInteractive guide\n\
agent\tguide\t/h/agents/core/guide.md\tCore guide\n\
skill\tazure-admin\t/h/skills/azure-admin/SKILL.md\tAzure RBAC and Entra ID\n\
skill\tpdf\t/h/skills/pdf/SKILL.md\tRead and write PDF files\n";

fn run_text(staged: &Path, command: DirectoryCommands) -> Result<String> {
    let mut out = Vec::new();
    run_in(staged, command, &mut out)?;
    Ok(String::from_utf8(out).unwrap())
}

fn find(words: &[&str]) -> DirectoryCommands {
    DirectoryCommands::Find {
        words: words.iter().map(|w| (*w).to_owned()).collect(),
    }
}

#[test]
fn find_matches_every_word_without_regard_to_case() {
    let (_dir, staged) = staged_with_index(INDEX);
    let out = run_text(&staged, find(&["AZURE", "entra"])).unwrap();
    assert_eq!(
        out,
        "skill\tazure-admin\tAzure RBAC and Entra ID\t/h/skills/azure-admin/SKILL.md\n"
    );
    assert_eq!(run_text(&staged, find(&["azure", "pdf"])).unwrap(), "");
    assert_eq!(run_text(&staged, find(&["no-such-word"])).unwrap(), "");
}

#[test]
fn find_without_words_prints_names_grouped_by_type() {
    let (_dir, staged) = staged_with_index(INDEX);
    let out = run_text(&staged, find(&[])).unwrap();
    assert_eq!(out, "skills:\n  azure-admin\n  pdf\nagents:\n  guide\n");
}

#[test]
fn list_prints_every_entry() {
    let (_dir, staged) = staged_with_index(INDEX);
    let out = run_text(&staged, DirectoryCommands::List).unwrap();
    assert_eq!(out.lines().count(), 4);
    assert!(out.lines().all(|line| line.split('\t').count() == 4));
}

#[test]
fn a_missing_index_is_an_error_that_names_install() {
    let dir = tempfile::tempdir().unwrap();
    let error = run_text(dir.path(), DirectoryCommands::List).unwrap_err();
    assert!(
        format!("{error:#}").contains("amplihack install"),
        "{error:#}"
    );
    let error = run_text(dir.path(), find(&["x"])).unwrap_err();
    assert!(
        format!("{error:#}").contains("amplihack install"),
        "{error:#}"
    );
}

#[test]
fn an_index_with_no_entry_is_an_error_for_find_and_list() {
    for text in [
        "",
        "not an entry\nskill\tonly-three\t/p\n",
        "thing\tx\t/p\td\n",
    ] {
        let (_dir, staged) = staged_with_index(text);
        assert!(
            run_text(&staged, DirectoryCommands::List).is_err(),
            "{text:?}"
        );
        assert!(run_text(&staged, find(&[])).is_err(), "{text:?}");
        assert!(run_text(&staged, find(&["x"])).is_err(), "{text:?}");
    }
}

#[test]
fn a_word_with_a_prefix_is_matched_by_the_part_after_its_last_colon() {
    let (_dir, staged) = staged_with_index(INDEX);
    let out = run_text(&staged, find(&["amplihack:core:guide"])).unwrap();
    assert_eq!(out.lines().count(), 2, "{out}");
    assert!(out.lines().all(|line| line.starts_with("agent\tguide\t")));
    let out = run_text(&staged, find(&["amplihack:PDF"])).unwrap();
    assert_eq!(
        out,
        "skill\tpdf\tRead and write PDF files\t/h/skills/pdf/SKILL.md\n"
    );
    // The part before the colon is not searched.
    assert_eq!(
        run_text(&staged, find(&["pdf:guide"]))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(run_text(&staged, find(&["guide:nomatch"])).unwrap(), "");
}

/// A writer whose reader has gone, as `head -1` is once it has its line.
struct Closed(std::io::ErrorKind);

impl Write for Closed {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(self.0.into())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_closed_standard_output_ends_the_command_normally() {
    let (_dir, staged) = staged_with_index(INDEX);
    let broken = || Closed(std::io::ErrorKind::BrokenPipe);
    run_in(&staged, DirectoryCommands::List, &mut broken()).unwrap();
    run_in(&staged, find(&["guide"]), &mut broken()).unwrap();
    run_in(&staged, find(&[]), &mut broken()).unwrap();
}

#[test]
fn another_failure_to_write_is_still_an_error() {
    let (_dir, staged) = staged_with_index(INDEX);
    let other = || Closed(std::io::ErrorKind::PermissionDenied);
    assert!(run_in(&staged, DirectoryCommands::List, &mut other()).is_err());
    assert!(run_in(&staged, find(&["guide"]), &mut other()).is_err());
    assert!(run_in(&staged, find(&[]), &mut other()).is_err());
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path.clone());
                found.push((path, Vec::new()));
            } else {
                let bytes = fs::read(&path).unwrap();
                found.push((path, bytes));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn neither_subcommand_writes_anything() {
    let (dir, staged) = staged_with_index(INDEX);
    let before = snapshot(dir.path());
    run_text(&staged, find(&["guide"])).unwrap();
    run_text(&staged, find(&[])).unwrap();
    run_text(&staged, DirectoryCommands::List).unwrap();
    assert_eq!(snapshot(dir.path()), before);
}
