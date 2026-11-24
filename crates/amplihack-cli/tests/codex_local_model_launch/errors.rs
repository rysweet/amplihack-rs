//! Refused local-model launches (AC12 and the security findings SEC-L1,
//! SEC-L6, SEC-D1, SEC-D2, SF2, SF6).
//!
//! Every refusal exits non-zero, starts no Codex and leaves `codex-local`
//! absent. A message is checked for what it names (a path, a variable, a
//! flag), never for its wording.

use super::harness::*;
use super::launch::base_profile;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[test]
fn a_missing_profile_names_the_expected_path() {
    let f = Fixture::new();
    let launch = f.local("absent", &["exec", "-"]);
    launch.assert_refused_at_run_time(&f, "missing profile");
    let expected = f.user_codex().join("absent.config.toml");
    assert!(
        launch.stderr().contains(&expected.display().to_string()),
        "the error names {}:\n{}",
        expected.display(),
        launch.stderr()
    );
}

/// The profile comes from `$CODEX_HOME` when that is set and is not a
/// generated home.
#[test]
fn the_profile_is_read_from_codex_home() {
    let f = Fixture::new();
    let custom = f.home().join("custom-codex");
    fs::create_dir(&custom).unwrap();
    fs::write(custom.join("alt.config.toml"), base_profile()).unwrap();
    let launch = f.launch(
        f.command(&["codex", "--local-model", "alt"])
            .env("CODEX_HOME", &custom),
    );
    launch.assert_started();
    assert_eq!(
        launch.env("CODEX_HOME").map(std::path::Path::new),
        Some(f.generated("alt").as_path())
    );

    // With CODEX_HOME unset the user Codex home is ~/.codex, which has no
    // `alt` profile.
    let mut command = f.command(&["codex", "--local-model", "alt"]);
    command.env_remove("CODEX_HOME");
    let launch = f.launch(&mut command);
    assert!(!launch.out.status.success());
    assert!(!launch.started());
    assert!(
        launch
            .stderr()
            .contains(&f.user_codex().join("alt.config.toml").display().to_string()),
        "{}",
        launch.stderr()
    );
}

const BAD_NAMES: &[&str] = &[
    "../x",
    "a/b",
    ".",
    "..",
    "a b",
    "caf\u{e9}",
    "name;rm",
    "x\u{2603}",
];

/// SEC-L1, by the flag: the rule runs before the name is used anywhere.
#[test]
fn an_invalid_name_from_the_flag_is_refused() {
    let f = Fixture::new();
    let long = "a".repeat(65);
    for name in BAD_NAMES.iter().copied().chain([long.as_str()]) {
        let mut command = f.command(&["codex", "--local-model", name]);
        let launch = f.launch(&mut command);
        launch.assert_refused_at_run_time(&f, name);
        assert!(
            launch.stderr().contains("--local-model"),
            "{name:?}: the error names its source:\n{}",
            launch.stderr()
        );
    }
}

/// SEC-L1, by the variable: never trimmed, never a fallback to an ordinary
/// (possibly hosted) launch.
#[test]
fn an_invalid_name_from_the_variable_is_refused() {
    let f = Fixture::new();
    for name in BAD_NAMES
        .iter()
        .copied()
        .chain([" ", " inkling", "inkling\t"])
    {
        let launch = f.launch(
            f.command(&["codex", "--", "exec", "-"])
                .env(LOCAL_MODEL_ENV, name),
        );
        launch.assert_refused_at_run_time(&f, name);
        assert!(
            launch.stderr().contains(LOCAL_MODEL_ENV),
            "{name:?}: the error names its source:\n{}",
            launch.stderr()
        );
    }
}

#[test]
fn the_longest_valid_name_is_accepted() {
    let f = Fixture::new();
    let name = format!("{}.v_1-x", "q".repeat(58));
    assert_eq!(name.len(), 64);
    f.write_profile(&name, base_profile());
    f.local(&name, &[]).assert_started();
    assert!(f.generated(&name).is_dir());
}

#[test]
fn an_empty_flag_value_is_a_usage_error() {
    let f = Fixture::new();
    let launch = f.launch(&mut f.command(&["codex", "--local-model", ""]));
    launch.assert_refused(&f, "--local-model \"\"");
    assert_eq!(launch.out.status.code(), Some(2), "{}", launch.stderr());
    assert!(
        launch.stderr().contains("--local-model"),
        "{}",
        launch.stderr()
    );
}

#[test]
fn a_malformed_user_hooks_json_is_refused() {
    let f = Fixture::new();
    let path = f.user_codex().join("hooks.json");
    for (what, text) in [
        ("not JSON", "{\"hooks\": {\"PreToolUse\": [\n"),
        (
            "PreToolUse not an array",
            r#"{"hooks": {"PreToolUse": {"hooks": []}}}"#,
        ),
        ("hooks not an object", r#"{"hooks": []}"#),
    ] {
        fs::write(&path, text).unwrap();
        let launch = f.local(NAME, &[]);
        launch.assert_refused_at_run_time(&f, what);
        assert!(
            launch.stderr().contains(&path.display().to_string()),
            "{what}: the error names the file:\n{}",
            launch.stderr()
        );
    }
}

#[test]
fn an_absent_user_hooks_json_means_no_user_entries() {
    let f = Fixture::new();
    fs::remove_file(f.user_codex().join("hooks.json")).unwrap();
    f.local(NAME, &[]).assert_started();
    let hooks = read_json(&f.generated(NAME).join("hooks.json"));
    assert_eq!(hooks["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
}

#[test]
fn a_missing_hooks_binary_is_refused() {
    let f = Fixture::new();
    let launch = f.launch(
        f.command(&["codex", "--local-model", NAME])
            .env(HOOKS_OVERRIDE_ENV, f.home().join("no-such-hooks-binary")),
    );
    launch.assert_refused_at_run_time(&f, "missing hooks binary");
    assert!(
        launch.stderr().contains(HOOKS_OVERRIDE_ENV),
        "{}",
        launch.stderr()
    );
}

/// SEC-L6 and SF4: the guard's path is never made absolute from the working
/// directory, and must be an executable regular file.
#[test]
fn an_unusable_hooks_binary_is_refused() {
    let f = Fixture::new();
    let relative_dir = f.home().join("checkout");
    fs::create_dir(&relative_dir).unwrap();
    crate::support::exec_stub::write_executable(
        &relative_dir.join("amplihack-hooks"),
        "#!/bin/sh\necho '{}'\n",
        0o700,
    );
    let not_executable = f.home().join("hooks-not-executable");
    fs::write(&not_executable, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&not_executable, fs::Permissions::from_mode(0o600)).unwrap();
    let directory = f.home().join("hooks-directory");
    fs::create_dir(&directory).unwrap();

    for (what, value) in [
        (
            "relative",
            std::ffi::OsString::from("checkout/amplihack-hooks"),
        ),
        ("not executable", not_executable.into_os_string()),
        ("a directory", directory.into_os_string()),
    ] {
        let launch = f.launch(
            f.command(&["codex", "--local-model", NAME])
                .env(HOOKS_OVERRIDE_ENV, &value),
        );
        launch.assert_refused_at_run_time(&f, what);
        assert!(
            launch.stderr().contains(HOOKS_OVERRIDE_ENV),
            "{what}:\n{}",
            launch.stderr()
        );
    }
}

#[test]
fn docker_is_refused_by_flag() {
    let f = Fixture::new();
    let launch = f.launch(&mut f.command(&["codex", "--local-model", NAME, "--docker"]));
    launch.assert_refused_at_run_time(&f, "--docker");
    assert!(launch.stderr().contains("--docker"), "{}", launch.stderr());

    let launch = f.launch(
        f.command(&["codex", "--docker", "--", "exec", "-"])
            .env(LOCAL_MODEL_ENV, NAME),
    );
    launch.assert_refused_at_run_time(&f, "--docker with the variable");
    assert!(launch.stderr().contains("--docker"), "{}", launch.stderr());
}

#[test]
fn docker_is_refused_by_environment() {
    if std::path::Path::new("/.dockerenv").exists() {
        // Inside a container AMPLIHACK_USE_DOCKER does not turn Docker on.
        eprintln!(
            "skipped docker_is_refused_by_environment: running in a container (/.dockerenv), \
             where AMPLIHACK_USE_DOCKER does not turn Docker on"
        );
        return;
    }
    let f = Fixture::new();
    let launch = f.launch(
        f.command(&["codex", "--local-model", NAME])
            .env("AMPLIHACK_USE_DOCKER", "1"),
    );
    launch.assert_refused_at_run_time(&f, "AMPLIHACK_USE_DOCKER");
    assert!(
        launch.stderr().contains("AMPLIHACK_USE_DOCKER"),
        "{}",
        launch.stderr()
    );
}

#[test]
fn auto_is_refused() {
    let f = Fixture::new();
    let launch = f.launch(&mut f.command(&[
        "codex",
        "--local-model",
        NAME,
        "--auto",
        "--max-turns",
        "1",
        "--",
        "exec",
        "-",
    ]));
    launch.assert_refused_at_run_time(&f, "--auto");
    assert!(launch.stderr().contains("--auto"), "{}", launch.stderr());

    let launch = f.launch_within(
        f.command(&["codex", "--auto", "--max-turns", "1", "--", "exec", "-"])
            .env(LOCAL_MODEL_ENV, NAME),
        Duration::from_secs(120),
    );
    launch.assert_refused_at_run_time(&f, "--auto with the variable");
    assert!(launch.stderr().contains("--auto"), "{}", launch.stderr());
}

/// `--append` starts no Codex process and is unaffected.
#[test]
fn append_is_unaffected() {
    let f = Fixture::new();
    let plain = f.launch(&mut f.command(&["codex", "--append", "more"]));
    let local = f.launch(&mut f.command(&["codex", "--local-model", NAME, "--append", "more"]));
    assert_eq!(local.out.status.code(), plain.out.status.code());
    assert!(!local.started());
    assert!(!f.codex_local().exists());
}

/// SF6: a FIFO in place of the profile is refused without blocking.
#[test]
fn a_profile_that_is_not_a_regular_file_is_refused_promptly() {
    let f = Fixture::new();
    let fifo = f.user_codex().join("piped.config.toml");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo");
    assert!(status.success());
    let launch = f.launch_within(
        &mut f.command(&["codex", "--local-model", "piped"]),
        Duration::from_secs(120),
    );
    launch.assert_refused_at_run_time(&f, "FIFO profile");
    assert!(
        launch.stderr().contains(&fifo.display().to_string()),
        "{}",
        launch.stderr()
    );

    fs::create_dir(f.user_codex().join("dir.config.toml")).unwrap();
    f.local("dir", &[])
        .assert_refused_at_run_time(&f, "directory profile");
}

/// SF6: at most 1 MiB is read from the profile or the user's hooks.json.
#[test]
fn oversized_inputs_are_refused() {
    let f = Fixture::new();
    let big = format!("{}\n# {}\n", base_profile(), "x".repeat(1024 * 1024));
    f.write_profile("big", &big);
    f.local("big", &[])
        .assert_refused_at_run_time(&f, "oversized profile");

    let hooks = format!(
        "{{\"hooks\":{{}},\"padding\":\"{}\"}}",
        "x".repeat(1024 * 1024)
    );
    fs::write(f.user_codex().join("hooks.json"), hooks).unwrap();
    f.local(NAME, &[])
        .assert_refused_at_run_time(&f, "oversized hooks.json");
}

/// E8: features, skills and skills.bundled must be tables in the profile.
#[test]
fn a_profile_with_the_wrong_shape_is_refused() {
    let f = Fixture::new();
    for (what, extra) in [
        ("features a string", "features = \"all\"\n"),
        ("features an array", "features = [\"hooks\"]\n"),
        ("skills a number", "skills = 1\n"),
        ("skills.bundled a bool", "skills = { bundled = true }\n"),
        ("dotted skills.bundled", "skills.bundled = \"yes\"\n"),
    ] {
        f.write_profile("shape", &format!("{extra}{}", base_profile()));
        let launch = f.local("shape", &[]);
        launch.assert_refused_at_run_time(&f, what);
        assert!(
            launch.stderr().contains(
                &f.user_codex()
                    .join("shape.config.toml")
                    .display()
                    .to_string()
            ),
            "{what}: the error names the file:\n{}",
            launch.stderr()
        );
    }
}

/// SEC-D1: no value from the profile or the user's hooks.json reaches a
/// message, even with tracing at its most verbose.
#[test]
fn no_profile_or_hooks_value_reaches_the_output() {
    let f = Fixture::new();
    let secret = sentinel();
    let provider = format!(
        "{}experimental_bearer_token = \"{secret}\"\n",
        base_profile()
    );
    let cases = [
        ("valid", provider.clone(), None),
        (
            "parse error",
            format!("token = \"{secret}\n{}", base_profile()),
            None,
        ),
        (
            "shape error",
            format!("features = \"{secret}\"\n{}", base_profile()),
            None,
        ),
        (
            "hooks.json shape error",
            provider.clone(),
            Some(format!(r#"{{"hooks": {{"PreToolUse": "{secret}"}}}}"#)),
        ),
        (
            "hooks.json parse error",
            provider.clone(),
            Some(format!("{{\"hooks\": {{\"PreToolUse\": [\"{secret}\"")),
        ),
    ];
    for (what, profile, hooks) in cases {
        f.write_profile("secret", &profile);
        if let Some(hooks) = hooks {
            fs::write(f.user_codex().join("hooks.json"), hooks).unwrap();
        }
        let launch = f.launch(
            f.command(&["codex", "--local-model", "secret"])
                .env("RUST_LOG", "trace"),
        );
        assert!(
            !launch.stdout().contains(&secret) && !launch.stderr().contains(&secret),
            "{what}: a profile value reached the output"
        );
        assert_eq!(launch.started(), what == "valid", "{what}");
        fs::write(f.user_codex().join("hooks.json"), user_hooks_json(f.home())).unwrap();
    }
}

/// SEC-D2: a HOME holding a newline and an escape sequence is printed
/// escaped, so an error cannot forge a line or drive the terminal.
#[test]
fn paths_in_errors_are_escaped() {
    let f = Fixture::with_home_named("home\nforged line \u{1b}[31m");
    let launch = f.local("absent", &[]);
    launch.assert_refused_at_run_time(&f, "missing profile under an awkward HOME");
    let stderr = launch.stderr();
    assert!(
        !stderr.contains('\u{1b}'),
        "a raw escape reached stderr: {stderr:?}"
    );
    assert!(
        !stderr.lines().any(|l| l.starts_with("forged line")),
        "a raw newline split a path: {stderr:?}"
    );
}

/// SF2: an existing `~/.amplihack` writable by others is refused before
/// anything is written.
#[test]
fn a_loose_amplihack_directory_is_refused() {
    let f = Fixture::new();
    let root = f.home().join(".amplihack");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    let launch = f.local(NAME, &[]);
    launch.assert_refused_at_run_time(&f, "~/.amplihack at 0777");
    assert!(
        launch.stderr().contains(&root.display().to_string()),
        "{}",
        launch.stderr()
    );
    assert_eq!(mode(&root), 0o777, "amplihack never changes ~/.amplihack");
}

/// D15: a directory amplihack would own that has looser permissions is an
/// error; amplihack does not repair it.
#[test]
fn a_loose_codex_local_directory_is_refused() {
    let f = Fixture::new();
    fs::create_dir(f.codex_local()).unwrap();
    fs::set_permissions(f.codex_local(), fs::Permissions::from_mode(0o755)).unwrap();
    let launch = f.local(NAME, &[]);
    assert!(!launch.out.status.success());
    assert!(!launch.started());
    assert_eq!(mode(&f.codex_local()), 0o755, "the mode is not changed");
    assert!(!f.generated(NAME).exists(), "nothing is built inside it");
}
