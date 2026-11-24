//! Issue #3220: `amplihack install` writes an index of amplihack's own skills
//! and agents, which `amplihack directory` reads.
//!
//! These tests install a fixture bundle into a temporary home and check the
//! index by its behaviour: one line for each fixture skill and agent, the
//! same bytes from two installs, no user-owned skill, an empty description
//! for an agent without front matter, and paths that exist.

use super::helpers::*;
use super::*;
use crate::commands::directory::index::{INDEX_FILE_NAME, index_path, read_index};
use std::fs;
use std::path::{Path, PathBuf};

const USER_SKILL: &str = "user-owned-directory-fixture";

fn write_file(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A fixture bundle with two top-level skills, one nested skill, an agent
/// with front matter, a nested agent with `meta` front matter, and an agent
/// with none.
fn write_fixture_bundle(repo: &Path) {
    let bundle = repo.join("amplifier-bundle");
    write_file(
        &bundle.join("skills/alpha-fixture/SKILL.md"),
        "---\nname: alpha-fixture\ndescription: Kubernetes cluster help.\n---\nbody\n",
    );
    write_file(
        &bundle.join("skills/beta-fixture/SKILL.md"),
        "---\nname: beta-fixture\ndescription: >\n  Spreadsheet\n  formulas.\n---\nbody\n",
    );
    write_file(
        &bundle.join("skills/group/nested-fixture/SKILL.md"),
        "---\nname: nested-fixture\ndescription: Nested skill.\n---\nbody\n",
    );
    write_file(
        &bundle.join("agents/front-fixture.md"),
        "---\nname: front-fixture\ndescription: Reviews TLA+ specifications.\n---\nbody\n",
    );
    write_file(
        &bundle.join("agents/core/meta-fixture.md"),
        "---\nmeta:\n  name: meta-fixture\n  description: Meta agent.\n---\nbody\n",
    );
    write_file(
        &bundle.join("agents/bare-fixture.md"),
        "# An agent with no front matter\n",
    );
}

struct InstalledHome {
    _guard: std::sync::MutexGuard<'static, ()>,
    _env: crate::test_support::EnvGuard,
    temp: tempfile::TempDir,
}

impl InstalledHome {
    fn staged(&self) -> PathBuf {
        self.temp.path().join(".amplihack/.claude")
    }

    fn index(&self) -> PathBuf {
        index_path(&self.staged())
    }

    fn install(&self) {
        local_install(self.temp.path(), None).unwrap();
    }
}

/// A temporary home holding a fixture repository and a user-owned skill,
/// with `HOME` pointed at it. `AMPLIHACK_HOME` points at a different, empty
/// folder, to show the index follows the installer's staging folder.
fn installed_home() -> InstalledHome {
    let guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let mut env = crate::test_support::EnvGuard::capture(&[
        "AMPLIHACK_AGENT_BINARY",
        "AMPLIHACK_SKIP_MMDC",
        "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
        "AMPLIHACK_HOME",
        "HOME",
        "PATH",
    ]);
    env.assign("HOME", temp.path());
    let elsewhere = temp.path().join("elsewhere-amplihack-home");
    fs::create_dir_all(&elsewhere).unwrap();
    env.assign("AMPLIHACK_HOME", &elsewhere);
    env.assign("AMPLIHACK_AGENT_BINARY", "claude");
    env.assign("AMPLIHACK_SKIP_MMDC", "1");
    let bin_dir = temp.path().join("stub_bin");
    let hooks_stub = create_exe_stub(&bin_dir, "amplihack-hooks");
    env.assign("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin_dir];
    dirs.extend(std::env::split_paths(&path));
    env.assign("PATH", std::env::join_paths(dirs).unwrap());

    create_source_repo(temp.path());
    write_fixture_bundle(temp.path());
    write_file(
        &temp
            .path()
            .join(".claude/skills")
            .join(USER_SKILL)
            .join("SKILL.md"),
        "---\nname: user-owned-directory-fixture\ndescription: Kubernetes, but mine.\n---\n",
    );
    InstalledHome {
        _guard: guard,
        _env: env,
        temp,
    }
}

#[test]
fn install_writes_one_line_for_each_bundle_skill_and_agent() {
    let home = installed_home();
    home.install();

    assert_eq!(
        home.index().parent().unwrap(),
        crate::claude_plugin::ownership_manifest_path(&home.staged())
            .parent()
            .unwrap(),
        "the index sits beside the skill ownership manifest"
    );
    let entries = read_index(&home.index()).unwrap().expect("index written");
    let keys: Vec<(&str, &str)> = entries
        .iter()
        .map(|e| (e.kind.label(), e.name.as_str()))
        .collect();
    assert_eq!(
        keys,
        [
            ("agent", "bare-fixture"),
            ("agent", "front-fixture"),
            ("agent", "meta-fixture"),
            ("skill", "alpha-fixture"),
            ("skill", "beta-fixture"),
            ("skill", "nested-fixture"),
        ]
    );
    let bare = &entries[0];
    assert_eq!(bare.description, "", "an agent without front matter");
    assert_eq!(entries[4].description, "Spreadsheet formulas.");
    assert_eq!(entries[2].description, "Meta agent.");
    assert!(
        entries[5]
            .path
            .ends_with("skills/group/nested-fixture/SKILL.md")
    );

    for entry in &entries {
        assert!(entry.path.is_absolute(), "{:?}", entry.path);
        assert!(entry.path.starts_with(home.staged()), "{:?}", entry.path);
        assert!(entry.path.is_file(), "{:?}", entry.path);
    }
    assert!(
        entries.iter().all(|e| e.name != USER_SKILL),
        "a skill in the user's own folder is never in the index"
    );
    assert!(
        !home
            .temp
            .path()
            .join("elsewhere-amplihack-home")
            .join(INDEX_FILE_NAME)
            .exists()
    );
}

#[test]
fn two_installs_of_the_same_bundle_write_the_same_bytes() {
    let home = installed_home();
    home.install();
    let first = fs::read(home.index()).unwrap();
    home.install();
    assert_eq!(fs::read(home.index()).unwrap(), first);
}

#[test]
fn directory_reads_the_index_install_wrote_when_amplihack_home_is_elsewhere() {
    use crate::commands::directory::{DirectoryCommands, run_for_test};
    let home = installed_home();
    home.install();

    let found = run_for_test(DirectoryCommands::Find {
        words: vec!["KUBERNETES".into()],
    })
    .unwrap();
    let lines: Vec<&str> = found.lines().collect();
    assert_eq!(lines.len(), 1, "{found}");
    let fields: Vec<&str> = lines[0].split('\t').collect();
    assert_eq!(fields[..2], ["skill", "alpha-fixture"]);
    assert!(Path::new(fields[3]).is_file());

    let none = run_for_test(DirectoryCommands::Find {
        words: vec!["zzz-no-such-word".into()],
    })
    .unwrap();
    assert!(none.is_empty());
}
