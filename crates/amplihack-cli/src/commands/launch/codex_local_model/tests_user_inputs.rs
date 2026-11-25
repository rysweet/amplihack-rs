//! The inputs a local-model launch takes from outside its own home: the
//! hooks binary, the user's approval and sandbox settings, the user's rules
//! files, and links that point nowhere.
//!
//! Each case shows that the launch is rejected, that the user's setting
//! applies, or that a stderr line names what is left out, and never that
//! the command check or the sandbox is weakened without a word. Unix only, like the module. Every test runs in a [`LocalFixture`],
//! which asserts that `HOME` is its own tempdir first.

use super::tests_fixture::*;
use super::{LocalHome, prepare_launch};
use amplihack_utils::secure_files::euid;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

fn build(f: &LocalFixture) -> LocalHome {
    f.assert_home_is_private_to_the_test();
    prepare_launch(Some(NAME), no_conflicts())
        .expect("prepared")
        .expect("a local-model launch")
        .refresh()
        .expect("refreshed")
}

/// The error of a launch that must be rejected, with nothing created.
#[track_caller]
fn rejected(f: &LocalFixture) -> String {
    f.assert_home_is_private_to_the_test();
    let error = match prepare_launch(Some(NAME), no_conflicts()) {
        Err(e) => format!("{e:#}"),
        Ok(None) => panic!("an ordinary launch"),
        Ok(Some(prepared)) => match prepared.refresh() {
            Ok(_) => panic!("the launch was not rejected"),
            Err(e) => format!("{e:#}"),
        },
    };
    assert!(!f.codex_local().exists(), "nothing may be created: {error}");
    error
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn hooks_command(f: &LocalFixture) -> String {
    let hooks: serde_json::Value =
        serde_json::from_slice(&fs::read(f.generated(NAME).join("hooks.json")).unwrap()).unwrap();
    hooks["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn generated_config(f: &LocalFixture) -> toml::Value {
    toml::from_str(&fs::read_to_string(f.generated(NAME).join("config.toml")).unwrap()).unwrap()
}

// ---- item 1: the hooks binary ----

/// The binary runs on every tool call. A binary, or a directory holding it,
/// that a group or others can write is rejected, and the message gives the
/// path.
#[test]
fn a_hooks_binary_others_can_write_is_rejected() {
    let f = LocalFixture::new();
    let bin = f.hooks_bin();
    let dir = bin.parent().unwrap().to_path_buf();
    for (path, insecure, secure) in [
        (&bin, 0o720, 0o700),
        (&bin, 0o702, 0o700),
        (&dir, 0o770, 0o700),
        (&dir, 0o707, 0o700),
    ] {
        fs::set_permissions(path, fs::Permissions::from_mode(insecure)).unwrap();
        let error = rejected(&f);
        assert_eq!(mode(path), insecure, "the binary is left alone");
        fs::set_permissions(path, fs::Permissions::from_mode(secure)).unwrap();
        assert!(
            error.contains(&path.display().to_string()),
            "{insecure:o}: {error}"
        );
    }
    build(&f);
}

/// The binary reached through a link is checked where it lives: a target
/// directory others can write is rejected.
#[test]
fn a_linked_hooks_binary_is_checked_where_it_lives() {
    let mut f = LocalFixture::new();
    let shared = f.home().join("shared");
    private_dir(&shared);
    let target = shared.join("amplihack-hooks");
    fs::copy(f.hooks_bin(), &target).unwrap();
    let links = f.home().join("links");
    private_dir(&links);
    let link = links.join("amplihack-hooks");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    f.set(HOOKS_OVERRIDE, &link);
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
    let error = rejected(&f);
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(error.contains(&shared.display().to_string()), "{error}");
    build(&f);
}

/// A binary owned by another user is rejected. Changing a file's owner
/// needs root, so the owner check is given another user's id: the same
/// check the launch makes with the effective user's id.
#[test]
fn a_hooks_binary_owned_by_another_user_is_rejected() {
    let f = LocalFixture::new();
    let bin = f.hooks_bin();
    let meta = fs::metadata(&bin).unwrap();
    super::home::check_owner_for(&bin, &meta, euid()).expect("the user's own binary is accepted");
    let foreign = euid().wrapping_add(1);
    let error = format!(
        "{:#}",
        super::home::check_owner_for(&bin, &meta, foreign)
            .expect_err("another user's binary must be rejected")
    );
    assert!(error.contains(&bin.display().to_string()), "{error}");
}

/// The deployed copy in the user's bin directory is the binary amplihack's
/// entry runs, as for an ordinary install, even when the override gives
/// another binary.
#[test]
fn the_deployed_hooks_binary_is_used() {
    let f = LocalFixture::new();
    let deployed_dir = f.home().join(".local/bin");
    private_dir(&f.home().join(".local"));
    private_dir(&deployed_dir);
    let deployed = deployed_dir.join("amplihack-hooks");
    crate::test_support::write_executable_with_mode(&deployed, "#!/bin/sh\necho '{}'\n", 0o700);
    build(&f);
    let command = hooks_command(&f);
    assert!(
        command.contains(&deployed.display().to_string()),
        "{command}"
    );
    assert!(
        !command.contains(&f.hooks_bin().display().to_string()),
        "{command}"
    );
}

/// A hooks binary found in a build directory, with nothing deployed, is
/// refused: the command-check entry would name a file that goes when the
/// directory is cleaned. Named on purpose through the override, it is used
/// and the launch says on stderr that it is a build output. The deployed
/// copy and a path outside a build directory get no line.
#[test]
fn a_hooks_binary_in_a_build_directory_is_refused_or_announced() {
    use super::home::build_tree_hooks_notice;
    let built = Path::new("/work/amplihack-rs/target/debug/amplihack-hooks");
    let error = format!(
        "{:#}",
        build_tree_hooks_notice(built, false, None).expect_err("a found build output")
    );
    assert!(error.contains(&built.display().to_string()), "{error}");
    assert!(error.contains("amplihack install"), "{error}");
    assert!(error.contains(HOOKS_OVERRIDE), "{error}");
    let other = std::ffi::OsStr::new("/elsewhere/amplihack-hooks");
    assert!(build_tree_hooks_notice(built, false, Some(other)).is_err());

    let named = build_tree_hooks_notice(built, false, Some(built.as_os_str()))
        .expect("a build output named by the override is used")
        .expect("and announced");
    assert!(named.contains(&built.display().to_string()), "{named}");
    assert!(named.contains(HOOKS_OVERRIDE), "{named}");

    assert!(
        build_tree_hooks_notice(built, true, None)
            .unwrap()
            .is_none()
    );
    let plain = Path::new("/home/u/.local/bin/amplihack-hooks");
    assert!(
        build_tree_hooks_notice(plain, false, None)
            .unwrap()
            .is_none()
    );
}

/// Through a launch: the override naming a build output builds the home,
/// the entry runs that binary, and the launch's stderr lines name it.
#[test]
fn a_launch_names_a_build_output_hooks_binary_on_stderr() {
    let mut f = LocalFixture::new();
    assert!(build(&f).notice_lines().is_empty());
    let built_dir = f.home().join("target/debug");
    private_dir(&f.home().join("target"));
    private_dir(&built_dir);
    let built = built_dir.join("amplihack-hooks");
    fs::copy(f.hooks_bin(), &built).unwrap();
    f.set(HOOKS_OVERRIDE, &built);
    let local = build(&f);
    assert!(hooks_command(&f).contains(&built.display().to_string()));
    let lines = local.notice_lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("amplihack: codex local model \"inkling\": "));
    assert!(lines[0].contains(&built.display().to_string()), "{lines:?}");
    assert!(lines[0].contains("build output"), "{lines:?}");
}

// ---- item 3: the user's approval and sandbox settings ----

fn write_user_config(f: &LocalFixture, text: &str) -> PathBuf {
    let path = f.user_codex().join("config.toml");
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

/// A user who has Codex ask first keeps that in a local session, and their
/// sandbox is passed on as the caller's choice, so an `exec` launch does not
/// get the agent step's full-access sandbox.
#[test]
fn the_user_approval_and_sandbox_apply_when_the_profile_has_none() {
    let f = LocalFixture::new();
    write_user_config(
        &f,
        "approval_policy = \"on-request\"\nsandbox_mode = \"read-only\"\n\n\
         [sandbox_workspace_write]\nnetwork_access = false\n",
    );
    let local = build(&f);
    let config = generated_config(&f);
    assert_eq!(config["approval_policy"].as_str(), Some("on-request"));
    assert_eq!(config["sandbox_mode"].as_str(), Some("read-only"));
    assert_eq!(
        config["sandbox_workspace_write"]["network_access"].as_bool(),
        Some(false)
    );
    let args = local.codex_args(strings(&["exec", "-"]));
    assert!(
        args.iter().any(|a| a == "sandbox_mode=\"read-only\""),
        "{args:?}"
    );
    let injected = super::super::codex_agent_step::codex_agent_step_args(&args);
    assert!(
        !injected.iter().any(|a| a == "danger-full-access"),
        "{injected:?}"
    );
}

#[test]
fn the_profile_approval_wins_over_the_user_setting() {
    let f = LocalFixture::new();
    write_user_config(&f, "approval_policy = \"on-request\"\n");
    f.write_profile(NAME, &format!("approval_policy = \"untrusted\"\n{PROFILE}"));
    build(&f);
    assert_eq!(
        generated_config(&f)["approval_policy"].as_str(),
        Some("untrusted")
    );
}

#[test]
fn with_no_policy_anywhere_the_default_is_never() {
    let f = LocalFixture::new();
    write_user_config(&f, "model = \"gpt-5\"\n");
    build(&f);
    let config = generated_config(&f);
    assert_eq!(config["approval_policy"].as_str(), Some("never"));
    assert!(config.get("sandbox_mode").is_none());
}

/// A user config that cannot be read as TOML, or that others can write, is
/// rejected with its path, rather than silently ignored.
#[test]
fn an_unusable_user_config_is_rejected() {
    let f = LocalFixture::new();
    let path = write_user_config(&f, "approval_policy = \n");
    let error = rejected(&f);
    assert!(error.contains(&path.display().to_string()), "{error}");

    write_user_config(&f, "approval_policy = \"on-request\"\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o622)).unwrap();
    let error = rejected(&f);
    assert!(error.contains(&path.display().to_string()), "{error}");
}

/// A profile selected in the user's `config.toml`, or profile tables there,
/// are not read by a local-model session, so a stricter approval or sandbox
/// setting kept in one does not apply. The launch says so on stderr, naming
/// the file and not the profile, and the generated file is unchanged.
#[test]
fn a_user_config_profile_is_announced_on_stderr() {
    let f = LocalFixture::new();
    for text in [
        "profile = \"safe\"\n\n[profiles.safe]\napproval_policy = \"on-request\"\n",
        "profile = \"safe\"\n",
        "[profiles.safe]\nsandbox_mode = \"read-only\"\n",
    ] {
        let path = write_user_config(&f, text);
        let local = build(&f);
        let lines = local.notice_lines();
        assert_eq!(lines.len(), 1, "{text}: {lines:?}");
        assert!(
            lines[0].starts_with("amplihack: codex local model \"inkling\": "),
            "{lines:?}"
        );
        assert!(lines[0].contains(&path.display().to_string()), "{lines:?}");
        assert!(lines[0].contains("approval_policy"), "{lines:?}");
        assert!(
            !lines[0].contains("safe"),
            "the profile name is not quoted: {lines:?}"
        );
        let config = generated_config(&f);
        assert_eq!(config["approval_policy"].as_str(), Some("never"), "{text}");
        assert!(config.get("profile").is_none() && config.get("profiles").is_none());
    }
    for text in ["approval_policy = \"on-request\"\n", "[profiles]\n"] {
        write_user_config(&f, text);
        assert!(build(&f).notice_lines().is_empty(), "{text}");
    }
}

// ---- item 3: the user's rules files ----

fn user_rules_dir(f: &LocalFixture) -> PathBuf {
    let dir = f.user_codex().join("rules");
    private_dir(&dir);
    dir
}

/// Each top-level `*.rules` file of the user's is copied into the generated
/// `rules/` as a private `amplihack-user-<file>`, byte for byte. Other files and
/// subdirectories are not.
#[test]
fn the_user_rules_are_copied_privately() {
    let f = LocalFixture::new();
    let rules = user_rules_dir(&f);
    fs::write(
        rules.join("a.rules"),
        "prefix_rule(pattern=[\"rm\"], decision=\"forbidden\")\n",
    )
    .unwrap();
    fs::write(
        rules.join("b.rules"),
        "prefix_rule(pattern=[\"git\", \"push\"], decision=\"prompt\")\n",
    )
    .unwrap();
    fs::write(rules.join("notes.txt"), "not a rule\n").unwrap();
    private_dir(&rules.join("nested"));
    fs::write(
        rules.join("nested/c.rules"),
        "prefix_rule(pattern=[\"ls\"])\n",
    )
    .unwrap();
    build(&f);
    let generated = f.generated(NAME).join("rules");
    for name in ["a.rules", "b.rules"] {
        let copy = generated.join(format!("amplihack-user-{name}"));
        assert_eq!(
            fs::read(&copy).unwrap(),
            fs::read(rules.join(name)).unwrap(),
            "{name}"
        );
        assert_eq!(mode(&copy), 0o600, "{name}");
    }
    let mut names: Vec<String> = fs::read_dir(&generated)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "amplihack-dangerous-commands.rules",
            "amplihack-user-a.rules",
            "amplihack-user-b.rules"
        ]
    );
}

/// A refresh with unchanged inputs leaves every copy as it is; a changed
/// rule is rewritten and a removed one disappears.
#[test]
fn the_user_rule_copies_follow_a_refresh() {
    let f = LocalFixture::new();
    let rules = user_rules_dir(&f);
    fs::write(rules.join("a.rules"), "prefix_rule(pattern=[\"rm\"])\n").unwrap();
    fs::write(rules.join("b.rules"), "prefix_rule(pattern=[\"dd\"])\n").unwrap();
    build(&f);
    let generated = f.generated(NAME).join("rules");
    let inode = |name: &str| fs::symlink_metadata(generated.join(name)).unwrap().ino();
    let before = (
        inode("amplihack-user-a.rules"),
        inode("amplihack-user-b.rules"),
    );
    build(&f);
    assert_eq!(
        (
            inode("amplihack-user-a.rules"),
            inode("amplihack-user-b.rules")
        ),
        before,
        "an unchanged refresh rewrites nothing"
    );

    fs::write(rules.join("a.rules"), "prefix_rule(pattern=[\"shred\"])\n").unwrap();
    fs::remove_file(rules.join("b.rules")).unwrap();
    build(&f);
    assert_eq!(
        fs::read_to_string(generated.join("amplihack-user-a.rules")).unwrap(),
        "prefix_rule(pattern=[\"shred\"])\n"
    );
    assert!(!generated.join("amplihack-user-b.rules").exists());
}

/// A rules file others can write is rejected, as the profile is.
#[test]
fn a_user_rules_file_others_can_write_is_rejected() {
    let f = LocalFixture::new();
    let rules = user_rules_dir(&f);
    let path = rules.join("a.rules");
    fs::write(&path, "prefix_rule(pattern=[\"rm\"])\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o622)).unwrap();
    let error = rejected(&f);
    assert!(error.contains(&path.display().to_string()), "{error}");
}

/// A nested launch that reuses the home does not reject it for the copies.
#[test]
fn a_nested_launch_reuses_a_home_with_user_rule_copies() {
    let mut f = LocalFixture::new();
    let rules = user_rules_dir(&f);
    fs::write(rules.join("a.rules"), "prefix_rule(pattern=[\"rm\"])\n").unwrap();
    build(&f);
    let generated = f.generated(NAME);
    f.set("CODEX_HOME", &generated);
    build(&f);
}

// ---- item 6: a link that points nowhere ----

/// A dangling link at the user's `hooks.json`, profile, config or rules file
/// is an error giving its path, never read as "no file".
#[test]
fn a_dangling_link_is_rejected() {
    let f = LocalFixture::new();
    let rules = user_rules_dir(&f);
    let profile = f.user_codex().join(format!("{NAME}.config.toml"));
    for path in [
        f.user_codex().join("hooks.json"),
        f.user_codex().join("config.toml"),
        rules.join("gone.rules"),
        profile.clone(),
    ] {
        let saved = fs::read(&path).ok();
        let _ = fs::remove_file(&path);
        std::os::unix::fs::symlink(f.home().join("absent-target"), &path).unwrap();
        let error = rejected(&f);
        fs::remove_file(&path).unwrap();
        if let Some(bytes) = saved {
            fs::write(&path, bytes).unwrap();
        }
        assert!(error.contains(&path.display().to_string()), "{error}");
    }
    build(&f);
}
