//! Issue #1344: `amplihack install` staged the 23 slash-command markdown
//! files into the Copilot CLI plugin and nowhere else.
//!
//! On a fully installed host
//! `~/.copilot/installed-plugins/amplihack@local/commands/` held 23 entries
//! while `~/.claude/commands/` did not exist at all, so `/lock`, `/unlock`,
//! `/auto` and `/ultrathink` were unavailable in every Claude Code session.
//!
//! These tests drive the real `local_install` against a temp `HOME` (never the
//! developer's own) and assert the Claude-side half of the staging: the
//! namespace directory Claude discovers as `/amplihack:<name>` is populated
//! from the same source directory the Copilot plugin reads, with the same file
//! count.

use super::helpers::*;
use super::*;
use std::fs;
use std::path::Path;

/// The command set shipped at `docs/claude/commands/amplihack/`.
const SOURCE_COMMANDS: &[&str] = &[
    "analyze",
    "auto",
    "cascade",
    "customize",
    "debate",
    "expert-panel",
    "fix",
    "improve",
    "ingest-code",
    "install",
    "knowledge-builder",
    "lock",
    "modular-build",
    "n-version",
    "reflect",
    "remote",
    "skill-builder",
    "socratic",
    "transcripts",
    "ultrathink",
    "uninstall",
    "unlock",
    "xpia",
];

/// Write the canonical `docs/claude/commands/amplihack/` source directory,
/// plus a non-markdown file that must never be staged.
fn write_command_source(repo_root: &Path) -> std::path::PathBuf {
    let dir = repo_root
        .join("docs")
        .join("claude")
        .join("commands")
        .join("amplihack");
    fs::create_dir_all(&dir).unwrap();
    for command in SOURCE_COMMANDS {
        fs::write(
            dir.join(format!("{command}.md")),
            format!("---\nname: amplihack:{command}\n---\n# /{command}\n"),
        )
        .unwrap();
    }
    fs::write(dir.join("README.txt"), "not a command\n").unwrap();
    dir
}

/// Run a real `local_install` with `HOME` pointed at `temp`, returning the
/// source command directory that was installed from.
///
/// Mirrors the env dance in `install_flow`: the shared env lock is held for
/// the whole install, a stub `amplihack-hooks` is placed on `PATH`, and every
/// mutated variable is restored before any assertion runs.
/// Refuse to run a real install/uninstall unless `HOME` has actually been
/// redirected into the fixture.
///
/// These tests drive `run_uninstall`, which deletes directories under `HOME`.
/// A fixture bug that left `HOME` pointing at the developer's own home would
/// otherwise be discovered by its consequences.
fn assert_home_is_the_fixture(temp: &Path) {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME must be set"));
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    assert_eq!(
        canonical(&home),
        canonical(temp),
        "HOME must point at the fixture before a real install runs"
    );
    assert!(
        canonical(&home).starts_with(canonical(&std::env::temp_dir())),
        "refusing to install into {} — it is not under {}",
        home.display(),
        std::env::temp_dir().display()
    );
}

fn install_into_temp_home(temp: &Path) -> std::path::PathBuf {
    assert_home_is_the_fixture(temp);
    let bin_dir = temp.join("stub_bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let hooks_stub = create_exe_stub(&bin_dir, "amplihack-hooks");

    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    let prev_skip_mmdc = std::env::var_os("AMPLIHACK_SKIP_MMDC");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        let new_path = format!(
            "{}:{}",
            bin_dir.display(),
            prev_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        std::env::set_var("PATH", &new_path);
        // The optional mermaid CLI provisioning is irrelevant here and would
        // otherwise shell out to npm during the test.
        std::env::set_var("AMPLIHACK_SKIP_MMDC", "1");
    }

    create_source_repo(temp);
    let source_dir = write_command_source(temp);
    let result = local_install(temp, None);

    unsafe {
        match prev_hooks {
            Some(v) => std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v),
            None => std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH"),
        }
        if let Some(v) = prev_path {
            std::env::set_var("PATH", v);
        }
        match prev_skip_mmdc {
            Some(v) => std::env::set_var("AMPLIHACK_SKIP_MMDC", v),
            None => std::env::remove_var("AMPLIHACK_SKIP_MMDC"),
        }
    }
    result.unwrap();
    source_dir
}

#[test]
fn local_install_stages_claude_slash_commands_from_the_source_directory() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(temp.path());

    install_into_temp_home(temp.path());

    let claude_commands = temp
        .path()
        .join(".claude")
        .join("commands")
        .join("amplihack");
    assert!(
        claude_commands.is_dir(),
        "issue #1344: install must create {} so Claude Code discovers the \
         amplihack commands; before the fix only the Copilot plugin dir was \
         populated and ~/.claude/commands/ did not exist at all",
        claude_commands.display()
    );
    for command in ["lock", "unlock", "auto", "ultrathink"] {
        assert!(
            claude_commands.join(format!("{command}.md")).is_file(),
            "/amplihack:{command} must be staged at {}",
            claude_commands.join(format!("{command}.md")).display()
        );
    }
    let lock = fs::read_to_string(claude_commands.join("lock.md")).unwrap();
    assert!(
        lock.contains("# /lock"),
        "staged command body must come from the source markdown, got:\n{lock}"
    );
    assert!(
        !claude_commands.join("README.txt").exists(),
        "non-markdown files must not be staged"
    );
}

#[test]
fn local_install_stages_every_source_command_for_claude() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(temp.path());

    let source_dir = install_into_temp_home(temp.path());

    let claude_commands = temp
        .path()
        .join(".claude")
        .join("commands")
        .join("amplihack");
    let expected: Vec<String> = SOURCE_COMMANDS
        .iter()
        .map(|command| format!("{command}.md"))
        .collect();

    assert_eq!(
        markdown_names(&source_dir),
        expected,
        "fixture drift: the source no longer ships the command set under test"
    );
    assert_eq!(
        entry_names(&claude_commands),
        expected,
        "every command in {} must reach {} and nothing else may; the Copilot \
         plugin got all {} of them while Claude got none (issue #1344)",
        source_dir.display(),
        claude_commands.display(),
        expected.len()
    );
}

/// Sorted names of the `*.md` files directly in `dir`.
fn markdown_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} must exist: {e}", dir.display()))
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|x| x.to_str()) == Some("md"))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Sorted names of *every* entry in `dir`, markdown or not.
fn entry_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} must exist: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn local_install_stages_the_same_command_set_for_claude_and_copilot() {
    // The two surfaces must not drift: whatever the Copilot plugin advertises,
    // a Claude session must be able to invoke. Names alone cannot fail this —
    // both sides read one source through a content-only transform — so the
    // per-surface frontmatter contract is asserted here as well: Copilot
    // rejects a colon in `name:` and gets it stripped, Claude derives the
    // namespace from the directory and must get the source byte-for-byte.
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(temp.path());

    // Copilot staging is a no-op unless ~/.copilot exists.
    fs::create_dir_all(temp.path().join(".copilot")).unwrap();

    install_into_temp_home(temp.path());

    let copilot = temp
        .path()
        .join(".copilot")
        .join("installed-plugins")
        .join("amplihack@local")
        .join("commands");
    let claude = temp
        .path()
        .join(".claude")
        .join("commands")
        .join("amplihack");

    let expected: Vec<String> = SOURCE_COMMANDS
        .iter()
        .map(|command| format!("{command}.md"))
        .collect();
    assert_eq!(entry_names(&claude), expected);
    assert_eq!(
        entry_names(&copilot),
        expected,
        "Claude and Copilot must be staged from the same command set"
    );

    assert_eq!(
        fs::read_to_string(claude.join("lock.md")).unwrap(),
        "---\nname: amplihack:lock\n---\n# /lock\n",
        "Claude namespaces by directory, so the source frontmatter is verbatim"
    );
    assert_eq!(
        fs::read_to_string(copilot.join("lock.md")).unwrap(),
        "---\nname: lock\n---\n# /lock\n",
        "Copilot flattens plugin commands into one namespace and rejects the colon"
    );
}

/// Issue #1344 review finding 2: the install swap renamed the whole namespace
/// aside and deleted it, so a user's own `~/.claude/commands/amplihack/*.md`
/// was destroyed under a green `✅ Claude Code staged 23 command(s)`.
#[test]
fn local_install_preserves_a_users_own_file_in_the_claude_command_namespace() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(temp.path());

    let claude_commands = temp
        .path()
        .join(".claude")
        .join("commands")
        .join("amplihack");
    fs::create_dir_all(&claude_commands).unwrap();
    fs::write(claude_commands.join("my-thing.md"), "mine\n").unwrap();

    install_into_temp_home(temp.path());

    assert_eq!(
        fs::read_to_string(claude_commands.join("my-thing.md")).unwrap(),
        "mine\n",
        "/amplihack:my-thing belongs to the user; install must not delete it"
    );
    assert!(
        claude_commands.join("lock.md").is_file(),
        "amplihack's own commands must still be staged alongside it"
    );
}

/// Issue #1344 review finding 3: `~/.claude/commands/<dir>/` *is* a namespace,
/// so a leftover `amplihack.staging/` would surface every command a second
/// time as `/amplihack.staging:<name>`.
#[test]
fn local_install_leaves_no_scratch_namespace_under_claudes_command_root() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(temp.path());

    install_into_temp_home(temp.path());
    // A second install exercises the move-aside/restore path.
    install_into_temp_home(temp.path());

    assert_eq!(
        entry_names(&temp.path().join(".claude").join("commands")),
        vec!["amplihack".to_string()],
        "every subdirectory here is a slash-command namespace"
    );
}

#[test]
fn uninstall_removes_the_claude_slash_command_directory() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(temp.path());

    install_into_temp_home(temp.path());
    let claude_commands = temp
        .path()
        .join(".claude")
        .join("commands")
        .join("amplihack");
    assert!(claude_commands.is_dir(), "precondition: commands staged");

    assert_home_is_the_fixture(temp.path());
    run_uninstall().unwrap();

    assert!(
        !claude_commands.exists(),
        "uninstall must remove {} so /amplihack:* stops resolving",
        claude_commands.display()
    );
}

/// Issue #1344 review finding 1: uninstall blind-deleted
/// `$HOME/.claude/commands/amplihack` — the first unconditional `rm` under the
/// user's own `~/.claude` in the codebase. It now removes the namespace only
/// while it still hashes to what install recorded.
#[test]
fn uninstall_preserves_a_claude_command_namespace_the_user_has_added_to() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(temp.path());

    install_into_temp_home(temp.path());
    let claude_commands = temp
        .path()
        .join(".claude")
        .join("commands")
        .join("amplihack");
    fs::write(claude_commands.join("my-thing.md"), "mine\n").unwrap();

    assert_home_is_the_fixture(temp.path());
    run_uninstall().unwrap();

    assert!(
        claude_commands.is_dir(),
        "the namespace no longer matches what amplihack staged, so it must survive"
    );
    assert_eq!(
        fs::read_to_string(claude_commands.join("my-thing.md")).unwrap(),
        "mine\n"
    );
}

/// Uninstall must also take the amplihack-private scratch root with it.
#[test]
fn uninstall_removes_the_command_staging_scratch_root() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(temp.path());

    install_into_temp_home(temp.path());
    let scratch = temp
        .path()
        .join(".claude")
        .join(".amplihack-command-staging");
    fs::create_dir_all(scratch.join("amplihack.old")).unwrap();

    assert_home_is_the_fixture(temp.path());
    run_uninstall().unwrap();

    assert!(
        !scratch.exists(),
        "an orphan scratch root left by an interrupted install must be cleaned"
    );
}
