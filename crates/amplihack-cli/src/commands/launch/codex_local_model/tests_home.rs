//! Building, refreshing and reusing the generated home, and what the launch
//! core receives from it.
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
//! Unix only, like the module. Every test runs in a [`LocalFixture`],
//! which asserts that `HOME` is its own tempdir first.

use super::tests_fixture::*;
use super::{LOCAL_MODEL_ENV, prepare_launch};
use crate::env_builder::EnvBuilder;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

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
fn only_the_generated_command_check_is_automatically_trusted() {
    let f = LocalFixture::new();
    build(&f);
    let home = f.generated(NAME);
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
    let state = config["hooks"]["state"].as_table().unwrap();
    let key = format!("{}:pre_tool_use:0:0", home.join("hooks.json").display());
    assert_eq!(state.len(), 1);
    assert_eq!(state[&key]["enabled"].as_bool(), Some(true));
    assert_eq!(
        state[&key]["trusted_hash"].as_str(),
        Some(super::hooks_json::command_check_hash(&f.hooks_bin()).as_str())
    );
}

#[test]
fn a_missing_amplihack_directory_is_created_private() {
    let f = LocalFixture::new();
    fs::remove_dir(f.home().join(".amplihack")).unwrap();
    build(&f);
    assert_eq!(mode(&f.home().join(".amplihack")), 0o700);
}

/// an existing `~/.amplihack` writable by others is refused, before
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

/// a file is skipped only when its bytes are right and it is a private
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

/// when the previous config.toml does not parse, nothing is written.
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

// ---- the hooks binary ----

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
    // an executable that is not amplihack-hooks, as a nested launch
    // would refuse it.
    let other = f.home().join("bin/true");
    crate::test_support::write_executable_with_mode(&other, "#!/bin/sh\n", 0o700);
    for (what, value) in [
        ("relative", Path::new("bin/amplihack-hooks").to_path_buf()),
        ("a directory", f.home().join("bin")),
        ("not executable", not_executable.clone()),
        ("a control character", control_bin.clone()),
        ("missing", f.home().join("bin/absent")),
        ("another name", other.clone()),
    ] {
        f.set(HOOKS_OVERRIDE, &value);
        let text = refresh_error(&f, Some(NAME));
        assert!(text.contains(HOOKS_OVERRIDE), "{what}: {text}");
        assert!(!f.codex_local().exists(), "{what}: nothing written");
    }
}

// ---- inputs read with care ----

#[test]
fn a_fifo_profile_is_refused_without_blocking() {
    let f = LocalFixture::new();
    let fifo = f.user_codex().join("piped.config.toml");
    let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    // No writer is opened. A blocking read would wait for a writer, so this
    // exercises the non-blocking input path without a time limit.
    let error =
        prepare_launch(Some("piped"), no_conflicts()).expect_err("a FIFO is not a regular file");
    let text = format!("{error:#}");
    assert!(text.contains(&fifo.display().to_string()), "{text}");
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

/// The hook-trust flag first unless the caller passed it, then
/// `-c features.hooks=true` always, then the caller's arguments.
#[test]
fn codex_args_put_hook_trust_first_unless_the_caller_passed_it() {
    let f = LocalFixture::new();
    let local = build(&f);
    const C: &str = "-c";
    const ON: &str = "features.hooks=true";
    for (input, expected) in [
        (vec![], vec![C, ON]),
        (vec!["exec", "-"], vec![HOOK_TRUST, C, ON, "exec", "-"]),
        (vec!["resume", "--last"], vec![C, ON, "resume", "--last"]),
        (vec![HOOK_TRUST], vec![C, ON, HOOK_TRUST]),
        (
            vec![HOOK_TRUST, "exec", "-"],
            vec![C, ON, HOOK_TRUST, "exec", "-"],
        ),
        (
            vec!["exec", HOOK_TRUST, "-"],
            vec![C, ON, "exec", HOOK_TRUST, "-"],
        ),
        (
            vec!["exec", "--", HOOK_TRUST],
            vec![HOOK_TRUST, C, ON, "exec", "--", HOOK_TRUST],
        ),
        // A caller's own override comes later, so Codex applies it last.
        (
            vec!["--disable", "hooks", "exec", "-"],
            vec![HOOK_TRUST, C, ON, "--disable", "hooks", "exec", "-"],
        ),
    ] {
        assert_eq!(
            local.codex_args(strings(&input)),
            strings(&expected),
            "{input:?}"
        );
    }
}

/// With the agent-step injection after it, the flag appears exactly once,
/// and the injection still finds `exec` behind `-c features.hooks=true`.
#[test]
fn the_agent_step_injection_adds_no_second_flag() {
    let f = LocalFixture::new();
    let local = build(&f);
    for input in [
        vec!["exec", "-"],
        vec!["exec", "--output-last-message", "/work/out", "-"],
        vec!["exec", HOOK_TRUST, "-"],
        vec![HOOK_TRUST, "exec", "-"],
        vec!["-m", "x", "exec", "-"],
    ] {
        let args = super::super::codex_agent_step::codex_agent_step_args(
            &local.codex_args(strings(&input)),
        );
        let count = args.iter().filter(|a| *a == HOOK_TRUST).count();
        assert_eq!(count, 1, "{input:?} -> {args:?}");
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--sandbox", "danger-full-access"]),
            "{input:?} -> {args:?}"
        );
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

/// three variables on the child's builder, none on
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

// ---- nested launches: the user Codex home and reuse ----

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

// ---- the profile's sandbox ----

/// A `sandbox_mode` in the profile reaches the argv as a `-c` override after
/// `-c features.hooks=true`, so the agent-step injection treats it as the
/// caller's choice and adds no `--sandbox danger-full-access`. A nested
/// launch that reuses the home reads it from the generated `config.toml`.
#[test]
fn a_profile_sandbox_is_passed_on_as_the_callers_choice() {
    let mut f = LocalFixture::new();
    f.write_profile(NAME, &format!("sandbox_mode = \"read-only\"\n{PROFILE}"));
    let local = build(&f);
    let args = local.codex_args(strings(&["exec", "-"]));
    assert_eq!(
        args,
        strings(&[
            HOOK_TRUST,
            "-c",
            "features.hooks=true",
            "-c",
            "sandbox_mode=\"read-only\"",
            "exec",
            "-",
        ])
    );
    let injected = super::super::codex_agent_step::codex_agent_step_args(&args);
    assert!(
        !injected.iter().any(|a| a == "danger-full-access"),
        "{injected:?}"
    );
    assert_eq!(injected.iter().filter(|a| *a == HOOK_TRUST).count(), 1);

    let generated = f.generated(NAME);
    f.set("CODEX_HOME", &generated);
    let reused = build(&f);
    assert!(
        reused
            .codex_args(strings(&["exec", "-"]))
            .iter()
            .any(|a| a == "sandbox_mode=\"read-only\""),
        "a nested launch keeps the sandbox"
    );
}

/// With no `sandbox_mode` in the profile nothing is added, and an `exec`
/// launch gets the agent-step sandbox as an ordinary one does.
#[test]
fn no_profile_sandbox_adds_no_override() {
    let f = LocalFixture::new();
    let local = build(&f);
    let args = local.codex_args(strings(&["exec", "-"]));
    assert!(
        !args.iter().any(|a| a.starts_with("sandbox_mode")),
        "{args:?}"
    );
}

// ---- who may supply the inputs ----

/// The profile and the user's `hooks.json` decide what runs in a session,
/// so a file others can write, or one in a directory others can write, is
/// refused before anything is created.
#[test]
fn inputs_others_can_write_are_refused() {
    let f = LocalFixture::new();
    let profile = f.user_codex().join(format!("{NAME}.config.toml"));
    let hooks = f.user_codex().join("hooks.json");
    fs::write(&hooks, "{\"hooks\": {}}\n").unwrap();
    fs::set_permissions(&hooks, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&profile, fs::Permissions::from_mode(0o600)).unwrap();
    build(&f);
    fs::remove_dir_all(f.codex_local()).unwrap();

    for file in [&profile, &hooks] {
        for insecure_mode in [0o620, 0o602] {
            fs::set_permissions(file, fs::Permissions::from_mode(insecure_mode)).unwrap();
            let error = refresh_error(&f, Some(NAME));
            assert!(error.contains(&file.display().to_string()), "{error}");
            assert!(!f.codex_local().exists(), "nothing is created: {error}");
            assert_eq!(mode(file), insecure_mode, "input is left alone");
            fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    for insecure_mode in [0o720, 0o703] {
        fs::set_permissions(f.user_codex(), fs::Permissions::from_mode(insecure_mode)).unwrap();
        let error = refresh_error(&f, Some(NAME));
        assert!(
            error.contains(&f.user_codex().display().to_string()),
            "{error}"
        );
        assert!(!f.codex_local().exists(), "nothing is created: {error}");
        assert_eq!(mode(&f.user_codex()), insecure_mode, "input is left alone");
        fs::set_permissions(f.user_codex(), fs::Permissions::from_mode(0o700)).unwrap();
    }
}

/// Both inputs may be symbolic links, but neither their resolved files nor
/// their containing directories may be writable by a group or by others.
#[test]
fn linked_inputs_are_checked_where_they_live() {
    let f = LocalFixture::new();
    let profile = f.user_codex().join(format!("{NAME}.config.toml"));
    let hooks = f.user_codex().join("hooks.json");
    let shared = f.home().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_file(&profile).unwrap();
    let profile_target = shared.join("profile.toml");
    let hooks_target = shared.join("hooks.json");
    fs::write(&profile_target, PROFILE).unwrap();
    fs::write(&hooks_target, "{\"hooks\": {}}\n").unwrap();
    std::os::unix::fs::symlink(&profile_target, &profile).unwrap();
    std::os::unix::fs::symlink(&hooks_target, &hooks).unwrap();

    for target in [&profile_target, &hooks_target] {
        fs::set_permissions(target, fs::Permissions::from_mode(0o600)).unwrap();
    }
    for (input, target) in [(&profile, &profile_target), (&hooks, &hooks_target)] {
        for insecure_mode in [0o620, 0o602] {
            fs::set_permissions(target, fs::Permissions::from_mode(insecure_mode)).unwrap();
            let error = refresh_error(&f, Some(NAME));
            assert!(error.contains(&input.display().to_string()), "{error}");
            assert!(!f.codex_local().exists(), "nothing is created: {error}");
            assert_eq!(mode(target), insecure_mode, "target is left alone");
            fs::set_permissions(target, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    for insecure_mode in [0o720, 0o703] {
        fs::set_permissions(&shared, fs::Permissions::from_mode(insecure_mode)).unwrap();
        let error = refresh_error(&f, Some(NAME));
        assert!(error.contains(&shared.display().to_string()), "{error}");
        assert!(!f.codex_local().exists(), "nothing is created: {error}");
        assert_eq!(mode(&shared), insecure_mode, "directory is left alone");
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o700)).unwrap();
    }

    // Read access for others is allowed; the policy protects who can change inputs.
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&profile_target, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(&hooks_target, fs::Permissions::from_mode(0o644)).unwrap();
    build(&f);
}

/// Exercise foreign ownership where changing a fixture's owner is permitted.
/// The inputs are restored before assertions so the fixture can be cleaned up.
#[test]
fn foreign_owned_inputs_and_directories_are_rejected_when_supported() {
    let f = LocalFixture::new();
    let profile = f.user_codex().join(format!("{NAME}.config.toml"));
    let hooks = f.user_codex().join("hooks.json");
    fs::write(&hooks, "{\"hooks\": {}}\n").unwrap();
    let owner = fs::metadata(&profile).unwrap().uid();
    let foreign_owner = if owner == 0 { 1 } else { 0 };
    for path in [&profile, &hooks, &f.user_codex()] {
        match std::os::unix::fs::chown(path, Some(foreign_owner), None) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "foreign ownership was not exercised: changing fixture ownership is not permitted"
                );
                return;
            }
            Err(error) => panic!("could not change fixture ownership: {error}"),
        }
        let result = prepare_launch(Some(NAME), no_conflicts());
        std::os::unix::fs::chown(path, Some(owner), None).unwrap();
        let error = format!("{:#}", result.expect_err("foreign input must be rejected"));
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(!f.codex_local().exists(), "nothing is created: {error}");
    }

    let targets = f.home().join("targets");
    fs::create_dir(&targets).unwrap();
    for (input, text) in [(&profile, PROFILE), (&hooks, "{\"hooks\": {}}\n")] {
        let target = targets.join(input.file_name().unwrap());
        fs::write(&target, text).unwrap();
        fs::remove_file(input).unwrap();
        std::os::unix::fs::symlink(&target, input).unwrap();
        std::os::unix::fs::chown(&target, Some(foreign_owner), None).unwrap();
        let result = prepare_launch(Some(NAME), no_conflicts());
        std::os::unix::fs::chown(&target, Some(owner), None).unwrap();
        assert!(result.is_err(), "foreign target must be rejected");
        assert!(!f.codex_local().exists());
    }
    std::os::unix::fs::chown(&targets, Some(foreign_owner), None).unwrap();
    let result = prepare_launch(Some(NAME), no_conflicts());
    std::os::unix::fs::chown(&targets, Some(owner), None).unwrap();
    assert!(result.is_err(), "foreign target directory must be rejected");
    assert!(!f.codex_local().exists());
}
