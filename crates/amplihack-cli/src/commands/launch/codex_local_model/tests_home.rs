//! Building, refreshing and reusing the generated home, and what the launch
//! core receives from it (D11, D15, D17, D18, SF2, SF3, SF4, SF6, SF10).
//!
//! The contract held here, beyond `tests_selection.rs`:
//!
//! - `impl Prepared { pub(crate) fn refresh(&self) ->
//!   anyhow::Result<LocalHome> }`, the last step before spawn;
//! - `impl LocalHome { pub(crate) fn codex_args(&self, args: Vec<String>) ->
//!   Vec<String>; pub(crate) fn with_env(&self, builder: EnvBuilder) ->
//!   EnvBuilder }`;
//! - `codex_agent_step::has_hook_trust_bypass(args: &[String]) -> bool`.
//!
//! Unix only, like the module (D21). Every test runs in a [`LocalFixture`],
//! which asserts that `HOME` is its own tempdir first (SEC-T1).

use super::tests_fixture::*;
use super::{LOCAL_MODEL_ENV, prepare_launch};
use crate::env_builder::EnvBuilder;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

const OWNED: [&str; 4] = [
    "config.toml",
    "hooks.json",
    "instructions.md",
    "rules/amplihack-dangerous-commands.rules",
];

fn build(f: &LocalFixture) -> super::LocalHome {
    f.assert_home_is_private_to_the_test();
    prepare_launch(Some(NAME), no_conflicts())
        .expect("prepared")
        .expect("a local-model launch")
        .refresh()
        .expect("refreshed")
}

fn refresh_error(f: &LocalFixture, flag: Option<&str>) -> String {
    f.assert_home_is_private_to_the_test();
    match prepare_launch(flag, no_conflicts()) {
        Err(e) => format!("{e:#}"),
        Ok(None) => panic!("an ordinary launch"),
        Ok(Some(prepared)) => match prepared.refresh() {
            Ok(_) => panic!("refresh succeeded"),
            Err(e) => format!("{e:#}"),
        },
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

fn inodes(home: &Path) -> Vec<u64> {
    OWNED
        .iter()
        .map(|f| fs::symlink_metadata(home.join(f)).unwrap().ino())
        .collect()
}

// ---- building ----

#[test]
fn refresh_builds_the_four_files_privately() {
    let f = LocalFixture::new();
    build(&f);
    let home = f.generated(NAME);
    for dir in [f.codex_local(), home.clone(), home.join("rules")] {
        assert_eq!(mode(&dir), 0o700, "{}", dir.display());
    }
    for file in OWNED {
        assert_eq!(mode(&home.join(file)), 0o600, "{file}");
    }
    let instructions = fs::read(home.join("instructions.md")).unwrap();
    assert!(!instructions.is_empty() && instructions.len() <= 1024);
}

#[test]
fn a_missing_amplihack_directory_is_created_private() {
    let f = LocalFixture::new();
    fs::remove_dir(f.home().join(".amplihack")).unwrap();
    build(&f);
    assert_eq!(mode(&f.home().join(".amplihack")), 0o700);
}

/// SF2: an existing `~/.amplihack` writable by others is refused, before
/// anything is written, and is not changed.
#[test]
fn a_loose_amplihack_directory_is_refused_and_left_alone() {
    let f = LocalFixture::new();
    let root = f.home().join(".amplihack");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    let text = refresh_error(&f, Some(NAME));
    assert!(text.contains(&root.display().to_string()), "{text}");
    assert!(!f.codex_local().exists());
    assert_eq!(mode(&root), 0o777);
}

/// SF10: a file is skipped only when its bytes are right and it is a private
/// regular file; otherwise it is replaced.
#[test]
fn unchanged_files_keep_their_inodes_and_others_are_replaced() {
    let f = LocalFixture::new();
    build(&f);
    let home = f.generated(NAME);
    let first = inodes(&home);
    build(&f);
    assert_eq!(inodes(&home), first, "nothing changed, nothing written");

    let hooks = home.join("hooks.json");
    fs::set_permissions(&hooks, fs::Permissions::from_mode(0o640)).unwrap();
    let instructions = home.join("instructions.md");
    let original = fs::read(&instructions).unwrap();
    fs::write(&instructions, "changed\n").unwrap();
    build(&f);
    let after = inodes(&home);
    assert_ne!(after[1], first[1], "hooks.json replaced");
    assert_eq!(mode(&hooks), 0o600);
    assert_ne!(after[2], first[2], "instructions.md replaced");
    assert_eq!(fs::read(&instructions).unwrap(), original);
    assert_eq!(after[0], first[0], "config.toml untouched");
    assert_eq!(after[3], first[3], "the rules file untouched");
}

/// D15: when the previous config.toml does not parse, nothing is written.
#[test]
fn an_unparseable_previous_config_writes_nothing() {
    let f = LocalFixture::new();
    build(&f);
    let home = f.generated(NAME);
    let config = home.join("config.toml");
    fs::write(&config, "x = [\n").unwrap();
    fs::remove_file(home.join("instructions.md")).unwrap();
    let text = refresh_error(&f, Some(NAME));
    assert!(text.contains(&config.display().to_string()), "{text}");
    assert!(
        !home.join("instructions.md").exists(),
        "computed first, then written"
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), "x = [\n");
}

// ---- the hooks binary (SF4) ----

#[test]
fn an_unusable_hooks_binary_is_refused_naming_the_override() {
    let mut f = LocalFixture::new();
    let not_executable = f.home().join("bin/plain");
    fs::write(&not_executable, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&not_executable, fs::Permissions::from_mode(0o600)).unwrap();
    let control = f.home().join("bin/ctl\u{1b}x");
    fs::create_dir(&control).unwrap();
    let control_bin = control.join("amplihack-hooks");
    crate::test_support::write_executable_with_mode(&control_bin, "#!/bin/sh\n", 0o700);
    for (what, value) in [
        ("relative", Path::new("bin/amplihack-hooks").to_path_buf()),
        ("a directory", f.home().join("bin")),
        ("not executable", not_executable.clone()),
        ("a control character", control_bin.clone()),
        ("missing", f.home().join("bin/absent")),
    ] {
        f.set(HOOKS_OVERRIDE, &value);
        let text = refresh_error(&f, Some(NAME));
        assert!(text.contains(HOOKS_OVERRIDE), "{what}: {text}");
        assert!(!f.codex_local().exists(), "{what}: nothing written");
    }
}

// ---- inputs read with care (SF6) ----

#[test]
fn a_fifo_profile_is_refused_without_blocking() {
    let f = LocalFixture::new();
    let fifo = f.user_codex().join("piped.config.toml");
    let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = prepare_launch(Some("piped"), no_conflicts()).map(|p| p.is_some());
        let _ = tx.send(result.map_err(|e| format!("{e:#}")));
    });
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(result) => {
            let text = result.expect_err("a FIFO is not a regular file");
            assert!(text.contains(&fifo.display().to_string()), "{text}");
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("prepare_launch panicked on a FIFO profile");
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Release the blocked reader before failing. A non-blocking open
            // for writing never waits: it fails at once when nothing reads.
            use std::os::unix::fs::OpenOptionsExt;
            let _ = fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo);
            panic!("reading the profile blocked on a FIFO");
        }
    }
}

#[test]
fn an_oversized_profile_is_refused() {
    let f = LocalFixture::new();
    f.write_profile("big", &format!("{PROFILE}# {}\n", "x".repeat(1024 * 1024)));
    let text = refresh_error(&f, Some("big"));
    assert!(text.contains("big.config.toml"), "{text}");
}

#[test]
fn a_profile_reached_through_a_symlink_is_read() {
    let f = LocalFixture::new();
    let real = f.home().join("profiles/linked.toml");
    fs::create_dir(f.home().join("profiles")).unwrap();
    fs::write(&real, PROFILE).unwrap();
    std::os::unix::fs::symlink(&real, f.user_codex().join("linked.config.toml")).unwrap();
    assert!(
        prepare_launch(Some("linked"), no_conflicts())
            .unwrap()
            .is_some()
    );
}

// ---- what the launch core receives ----

#[test]
fn codex_args_put_hook_trust_first_unless_the_caller_passed_it() {
    let f = LocalFixture::new();
    let local = build(&f);
    for (input, expected) in [
        (vec![], vec![HOOK_TRUST]),
        (vec!["exec", "-"], vec![HOOK_TRUST, "exec", "-"]),
        (
            vec!["resume", "--last"],
            vec![HOOK_TRUST, "resume", "--last"],
        ),
        (vec![HOOK_TRUST], vec![HOOK_TRUST]),
        (vec![HOOK_TRUST, "exec", "-"], vec![HOOK_TRUST, "exec", "-"]),
        (vec!["exec", HOOK_TRUST, "-"], vec!["exec", HOOK_TRUST, "-"]),
        (
            vec!["exec", "--", HOOK_TRUST],
            vec![HOOK_TRUST, "exec", "--", HOOK_TRUST],
        ),
    ] {
        assert_eq!(
            local.codex_args(strings(&input)),
            strings(&expected),
            "{input:?}"
        );
    }
}

/// With the agent-step injection after it, the flag appears exactly once.
#[test]
fn the_agent_step_injection_adds_no_second_flag() {
    let f = LocalFixture::new();
    let local = build(&f);
    for input in [
        vec!["exec", "-"],
        vec!["exec", "--output-last-message", "/tmp/out", "-"],
        vec!["exec", HOOK_TRUST, "-"],
        vec![HOOK_TRUST, "exec", "-"],
        vec!["-m", "x", "exec", "-"],
    ] {
        let args = super::super::codex_agent_step::codex_agent_step_args(
            &local.codex_args(strings(&input)),
        );
        let count = args.iter().filter(|a| *a == HOOK_TRUST).count();
        assert_eq!(count, 1, "{input:?} -> {args:?}");
    }
}

#[test]
fn has_hook_trust_bypass_agrees_with_the_agent_step() {
    use super::super::codex_agent_step::has_hook_trust_bypass;
    for (args, expected) in [
        (vec![], false),
        (vec![HOOK_TRUST], true),
        (vec!["exec", HOOK_TRUST, "-"], true),
        (vec!["-m", "x", HOOK_TRUST], true),
        (vec!["exec", "--", HOOK_TRUST], false),
        (vec!["-c", HOOK_TRUST], false),
        (vec!["--dangerously-bypass-hook-trust=true"], false),
    ] {
        assert_eq!(has_hook_trust_bypass(&strings(&args)), expected, "{args:?}");
    }
}

/// D18 and SEC-X5: three variables on the child's builder, none on
/// amplihack's own process, and never the hook protocol.
#[test]
fn with_env_sets_the_three_variables_on_the_builder_only() {
    let f = LocalFixture::new();
    let local = build(&f);
    let builder = local.with_env(EnvBuilder::new());
    let home = f.generated(NAME);
    assert_eq!(
        builder.var("CODEX_HOME").map(Path::new),
        Some(home.as_path())
    );
    assert_eq!(builder.var("AMPLIHACK_HOOK_TEXT"), Some("none"));
    assert_eq!(builder.var(LOCAL_MODEL_ENV), Some(NAME));
    assert_eq!(builder.var("AMPLIHACK_HOOK_PROTOCOL"), None);
    for name in [
        "CODEX_HOME",
        "AMPLIHACK_HOOK_TEXT",
        LOCAL_MODEL_ENV,
        "AMPLIHACK_HOOK_PROTOCOL",
    ] {
        assert!(
            std::env::var_os(name).is_none(),
            "{name} must not be set on amplihack's own process"
        );
    }
}

// ---- nested launches: the user Codex home and reuse (D11, SF3) ----

#[test]
fn a_codex_home_inside_codex_local_is_not_a_user_codex_home() {
    let mut f = LocalFixture::new();
    f.write_profile("beta", PROFILE);
    build(&f);
    f.set("CODEX_HOME", f.generated(NAME));
    f.select("beta");
    let local = prepare_launch(None, no_conflicts())
        .unwrap()
        .unwrap()
        .refresh()
        .unwrap();
    let builder = local.with_env(EnvBuilder::new());
    assert_eq!(
        builder.var("CODEX_HOME").map(Path::new),
        Some(f.generated("beta").as_path())
    );
}

#[test]
fn a_nested_launch_for_the_same_name_reuses_without_the_profile() {
    let mut f = LocalFixture::new();
    build(&f);
    let home = f.generated(NAME);
    let before = inodes(&home);
    fs::remove_file(f.user_codex().join(format!("{NAME}.config.toml"))).unwrap();
    f.set("CODEX_HOME", &home);
    f.select(NAME);
    let local = prepare_launch(None, no_conflicts())
        .unwrap()
        .unwrap()
        .refresh()
        .unwrap();
    assert_eq!(inodes(&home), before);
    assert_eq!(
        local
            .with_env(EnvBuilder::new())
            .var("CODEX_HOME")
            .map(Path::new),
        Some(home.as_path())
    );
}

/// Reuse needs config.toml and hooks.json; without them the home is built,
/// and the profile comes from `~/.codex`.
#[test]
fn a_codex_home_without_the_files_is_built_not_reused() {
    let mut f = LocalFixture::new();
    f.set("CODEX_HOME", f.generated(NAME));
    f.select(NAME);
    prepare_launch(None, no_conflicts())
        .unwrap()
        .unwrap()
        .refresh()
        .unwrap();
    for file in OWNED {
        assert!(f.generated(NAME).join(file).is_file(), "{file}");
    }
}

#[test]
fn reuse_refuses_a_home_without_amplihacks_entry() {
    let mut f = LocalFixture::new();
    build(&f);
    let home = f.generated(NAME);
    let hooks = home.join("hooks.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&hooks).unwrap()).unwrap();
    value["hooks"]["PreToolUse"] = serde_json::json!([]);
    fs::write(&hooks, value.to_string()).unwrap();
    let written = fs::read(&hooks).unwrap();
    f.set("CODEX_HOME", &home);
    f.select(NAME);
    let text = refresh_error(&f, None);
    assert!(text.contains(&home.display().to_string()), "{text}");
    assert_eq!(fs::read(&hooks).unwrap(), written, "reuse writes nothing");
}

#[test]
fn reuse_refuses_a_rules_file_that_differs_from_the_policy() {
    let mut f = LocalFixture::new();
    build(&f);
    let home = f.generated(NAME);
    let rules = home.join(OWNED[3]);
    fs::write(&rules, "# amplihack: dangerous-command deny rules\n").unwrap();
    f.set("CODEX_HOME", &home);
    f.select(NAME);
    let text = refresh_error(&f, None);
    assert!(text.contains(&home.display().to_string()), "{text}");
}
