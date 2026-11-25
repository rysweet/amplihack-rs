//! The user's Codex home is never written; a generated home is
//! refreshed in place and reused by a nested launch, which
//! verifies it first.

use super::harness::*;
use super::launch::base_profile;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// a recursive snapshot of the user's Codex home is identical after a
/// local launch, interactive and `exec`.
#[test]
fn the_user_codex_home_is_unchanged() {
    let f = Fixture::new();
    let before = snapshot(&f.user_codex());
    for tail in [&[][..], &["exec", "-"][..], &["resume", "--last"][..]] {
        f.local(NAME, tail).assert_started();
        assert_eq!(snapshot(&f.user_codex()), before, "after {tail:?}");
    }
    let launch = f.launch(
        f.command(&["codex", "--", "exec", "-"])
            .env(LOCAL_MODEL_ENV, NAME),
    );
    launch.assert_started();
    assert_eq!(snapshot(&f.user_codex()), before, "by the variable");
}

/// with start-up self-heal on, it does not reconcile the Codex
/// plugin into the user's Codex home, whatever agent binary is inherited.
#[test]
fn start_up_self_heal_leaves_the_user_codex_home_alone() {
    for agent in [None, Some("codex")] {
        let f = Fixture::new();
        let before = snapshot(&f.user_codex());
        let mut command =
            f.command_with_self_heal(&["codex", "--local-model", NAME, "--", "exec", "-"]);
        match agent {
            Some(value) => command.env("AMPLIHACK_AGENT_BINARY", value),
            None => command.env_remove("AMPLIHACK_AGENT_BINARY"),
        };
        let launch = f.launch(&mut command);
        launch.assert_started();
        assert_eq!(snapshot(&f.user_codex()), before, "agent {agent:?}");
        assert!(
            !launch.calls.lines().any(|l| l.starts_with("plugin")),
            "agent {agent:?}: no plugin calls: {}",
            launch.calls
        );
        assert!(
            !f.home().join(".amplihack/codex/market").exists(),
            "agent {agent:?}: the Codex plugin package is not staged"
        );
        let generated = snapshot(&f.generated(NAME));
        assert!(
            generated
                .keys()
                .all(|p| !p.starts_with("plugins") && !p.starts_with("skills")),
            "agent {agent:?}: no plugin or skills in the generated home: {:?}",
            generated.keys().collect::<Vec<_>>()
        );
    }
}

/// An interactive local launch runs the launch bootstrap, which for a
/// local model is the generic one: the user's Codex home is unchanged. An
/// ordinary interactive launch of the same fixture is the control. It runs
/// the Codex bootstrap, which writes the user's Codex home, so the test
/// shows that the bootstrap ran and that the local launch replaced it.
#[test]
fn an_interactive_local_launch_leaves_the_user_codex_home_alone() {
    for local in [true, false] {
        let f = Fixture::new();
        // The ordinary launch's plugin installation refuses the plugin
        // wrapper entries of the fixture's `hooks.json` as unowned; both runs
        // start from the same home without it.
        fs::remove_file(f.user_codex().join("hooks.json")).unwrap();
        // Keeps the unrelated first-launch memory-consent dialog off stdin.
        fs::write(
            f.home().join(".amplihack/config"),
            r#"{"node_options_consent":true}"#,
        )
        .unwrap();
        let args: &[&str] = if local {
            &["codex", "--local-model", NAME, "--no-reflection"]
        } else {
            &["codex", "--no-reflection"]
        };
        let mut command = f.command_with_staged_bundle(args);
        let settings = f.home().join(".claude/settings.json");
        // Start-up self-heal is disabled. Recreating these registrations
        // therefore demonstrates that interactive bootstrap executed.
        fs::remove_file(&settings).unwrap();
        let before = snapshot(&f.user_codex());
        let launch = f.launch_on_terminal(&mut command);
        launch.assert_started();
        let after = snapshot(&f.user_codex());
        if local {
            let repaired: serde_json::Value =
                serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
            assert!(
                repaired["hooks"]["PreToolUse"]
                    .as_array()
                    .is_some_and(|groups| !groups.is_empty())
            );
            assert_eq!(
                after.keys().collect::<Vec<_>>(),
                before.keys().collect::<Vec<_>>(),
                "no file is added to or removed from the user's Codex home\nstderr: {}",
                launch.stderr()
            );
            assert_eq!(after, before, "the user's Codex home is unchanged");
            assert!(f.generated(NAME).join("config.toml").is_file());
        } else {
            assert_ne!(
                after,
                before,
                "control: an ordinary interactive launch runs the Codex bootstrap, which \
                 writes the user's Codex home\nstderr: {}",
                launch.stderr()
            );
            let config = read_toml(&f.user_codex().join("config.toml"));
            assert_eq!(config["approval_policy"].as_str(), Some("on-request"));
            assert_eq!(
                config["plugins"]["amplihack@amplihack-local"]["enabled"].as_bool(),
                Some(true)
            );
            assert!(launch.calls.lines().any(|call| call.starts_with("plugin")));
        }
    }
}

/// an unchanged profile writes nothing on the next launch.
#[test]
fn an_unchanged_home_keeps_every_inode() {
    let f = Fixture::new();
    f.local(NAME, &[]).assert_started();
    let home = f.generated(NAME);
    let first = inodes(&home);
    let bytes = snapshot(&home);
    f.local(NAME, &["exec", "-"]).assert_started();
    assert_eq!(inodes(&home), first);
    assert_eq!(snapshot(&home), bytes);
}

/// a changed owned file is put back on the next top-level launch.
#[test]
fn changed_owned_files_are_restored() {
    let f = Fixture::new();
    f.local(NAME, &[]).assert_started();
    let home = f.generated(NAME);
    let pristine = snapshot(&home);

    let config = home.join("config.toml");
    let edited = fs::read_to_string(&config)
        .unwrap()
        .replace("\"disabled\"", "\"live\"");
    fs::write(&config, edited).unwrap();
    fs::write(home.join("hooks.json"), "{\"hooks\":{}}\n").unwrap();
    fs::remove_file(home.join(RULES_FILE)).unwrap();
    fs::write(home.join("instructions.md"), "Do something else.\n").unwrap();

    f.local(NAME, &[]).assert_started();
    assert_eq!(read_toml(&config)["web_search"].as_str(), Some("disabled"));
    assert_eq!(snapshot(&home), pristine);
}

/// a mode amplihack did not set is fixed by rewriting, not chmod.
#[test]
fn an_owned_file_with_a_loose_mode_is_rewritten_privately() {
    let f = Fixture::new();
    f.local(NAME, &[]).assert_started();
    let hooks = f.generated(NAME).join("hooks.json");
    fs::set_permissions(&hooks, fs::Permissions::from_mode(0o644)).unwrap();
    let old = inode(&hooks);
    f.local(NAME, &[]).assert_started();
    assert_eq!(mode(&hooks), 0o600);
    assert_ne!(inode(&hooks), old, "replaced by a new private file");
}

const CODEX_TABLES: &str = r#"
[projects."/work/trusted-by-codex"]
trust_level = "trusted"

[hooks.state."/work/trusted-by-codex/.codex/hooks.json:pre_tool_use:0:0"]
trusted_hash = "sha256:0123456789abcdef"

[notice]
hide_full_access_warning = true

[[hooks.PreToolUse]]
command = "echo written-into-the-generated-file"
"#;

/// Codex state survives a refresh, while hook definitions are not copied.
#[test]
fn tables_codex_writes_survive_a_refresh() {
    let f = Fixture::new();
    f.local(NAME, &[]).assert_started();
    let config = f.generated(NAME).join("config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(CODEX_TABLES);
    fs::write(&config, &text).unwrap();
    let written = read_toml(&config);

    f.local(NAME, &[]).assert_started();
    let after = read_toml(&config);
    assert_eq!(after["projects"], written["projects"]);
    assert_eq!(after["notice"], written["notice"]);
    assert_eq!(after["hooks"]["state"], written["hooks"]["state"]);
    assert!(
        after["hooks"].get("PreToolUse").is_none(),
        "hook definitions are not copied: {after:#?}"
    );
    assert!(
        after["projects"].get("/work/checkout").is_none(),
        "the profile's projects table is still dropped"
    );

    // A second refresh with the same inputs changes nothing.
    let settled = inode(&config);
    f.local(NAME, &[]).assert_started();
    assert_eq!(inode(&config), settled);
}

/// The `[hooks.state]` entry that switches amplihack's PreToolUse handler
/// off in a generated home, keyed as Codex 0.166 keys it.
fn disable_command_check(config_home: &Path, key_home: &Path) {
    let config = config_home.join("config.toml");
    let mut value = read_toml(&config);
    let key = format!("{}:pre_tool_use:0:0", key_home.join("hooks.json").display());
    let state = value["hooks"]["state"].as_table_mut().unwrap();
    state
        .entry(key)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .unwrap()
        .insert("enabled".into(), toml::Value::Boolean(false));
    fs::write(config, toml::to_string(&value).unwrap()).unwrap();
}

/// a top-level launch over a generated config that switches amplihack's
/// handler off restores its enabled and trusted state.
#[test]
fn a_top_level_launch_restores_a_disabled_guard() {
    let f = Fixture::new();
    f.local(NAME, &[]).assert_started();
    let home = f.generated(NAME);
    let config = home.join("config.toml");
    let mut value = read_toml(&config);
    let key = format!("{}:pre_tool_use:0:0", home.join("hooks.json").display());
    value["hooks"]["state"][&key]["enabled"] = toml::Value::Boolean(false);
    fs::write(&config, toml::to_string(&value).unwrap()).unwrap();
    assert!(
        read_toml(&config)["hooks"]["state"].is_table(),
        "fixture check"
    );

    f.local(NAME, &[]).assert_started();
    let after = read_toml(&config);
    let key = format!("{}:pre_tool_use:0:0", home.join("hooks.json").display());
    assert_eq!(
        after["hooks"]["state"][&key]["enabled"].as_bool(),
        Some(true)
    );
}

/// what Codex keeps in its home is left alone.
#[test]
fn runtime_files_in_the_home_are_untouched() {
    let f = Fixture::new();
    f.local(NAME, &[]).assert_started();
    let home = f.generated(NAME);
    fs::create_dir_all(home.join("sessions/2026/11/24")).unwrap();
    fs::write(home.join("sessions/2026/11/24/rollout.jsonl"), "{}\n").unwrap();
    fs::write(home.join("history.jsonl"), "{\"text\":\"hi\"}\n").unwrap();
    fs::write(home.join("auth.json"), "{\"token\":\"local\"}\n").unwrap();
    fs::write(home.join("rules/user-added.rules"), "# mine\n").unwrap();
    let runtime = [
        "sessions/2026/11/24/rollout.jsonl",
        "history.jsonl",
        "auth.json",
        "rules/user-added.rules",
    ];
    let before: Vec<_> = runtime
        .iter()
        .map(|p| (inode(&home.join(p)), fs::read(home.join(p)).unwrap()))
        .collect();
    f.local(NAME, &["exec", "-"]).assert_started();
    let after: Vec<_> = runtime
        .iter()
        .map(|p| (inode(&home.join(p)), fs::read(home.join(p)).unwrap()))
        .collect();
    assert_eq!(after, before);
}

/// a symbolic link at the home, at `rules/` or at an owned file stops
/// the launch, and what it points at is not written.
#[test]
fn symlinks_in_the_generated_home_stop_the_launch() {
    let f = Fixture::new();
    let elsewhere = f.home().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o700)).unwrap();

    // The home itself.
    fs::create_dir(f.codex_local()).unwrap();
    fs::set_permissions(f.codex_local(), fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.generated(NAME)).unwrap();
    let launch = f.local(NAME, &[]);
    assert!(
        !launch.out.status.success() && !launch.started(),
        "home link"
    );
    assert_eq!(
        fs::read_dir(&elsewhere).unwrap().count(),
        0,
        "nothing written through it"
    );
    fs::remove_file(f.generated(NAME)).unwrap();

    // An owned file and `rules/`.
    f.local(NAME, &[]).assert_started();
    let home = f.generated(NAME);
    let target = elsewhere.join("hooks.json");
    fs::write(&target, "untouched").unwrap();
    fs::remove_file(home.join("hooks.json")).unwrap();
    std::os::unix::fs::symlink(&target, home.join("hooks.json")).unwrap();
    let launch = f.local(NAME, &[]);
    assert!(
        !launch.out.status.success() && !launch.started(),
        "owned file link"
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
    fs::remove_file(home.join("hooks.json")).unwrap();

    let rules_target = elsewhere.join("rules");
    fs::create_dir(&rules_target).unwrap();
    fs::set_permissions(&rules_target, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir_all(home.join("rules")).unwrap();
    std::os::unix::fs::symlink(&rules_target, home.join("rules")).unwrap();
    let launch = f.local(NAME, &[]);
    assert!(
        !launch.out.status.success() && !launch.started(),
        "rules link"
    );
    assert_eq!(fs::read_dir(&rules_target).unwrap().count(), 0);
}

/// an existing generated config.toml that does not parse stops the
/// launch before anything is written.
#[test]
fn an_unparseable_generated_config_stops_the_launch() {
    let f = Fixture::new();
    f.local(NAME, &[]).assert_started();
    let home = f.generated(NAME);
    let config = home.join("config.toml");
    fs::write(&config, "this is = not [valid toml\n").unwrap();
    fs::remove_file(home.join(RULES_FILE)).unwrap();
    let hooks_inode = inode(&home.join("hooks.json"));

    let launch = f.local(NAME, &[]);
    assert!(!launch.out.status.success() && !launch.started());
    assert!(
        launch.stderr().contains(&config.display().to_string()),
        "{}",
        launch.stderr()
    );
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "this is = not [valid toml\n"
    );
    assert!(!home.join(RULES_FILE).exists(), "nothing is written");
    assert_eq!(inode(&home.join("hooks.json")), hooks_inode);
}

/// `--subprocess-safe` keeps its meaning and the home is refreshed.
#[test]
fn subprocess_safe_still_refreshes_the_home() {
    let f = Fixture::new();
    let launch = f.launch(&mut f.command(&[
        "codex",
        "--subprocess-safe",
        "--local-model",
        NAME,
        "--",
        "exec",
        "-",
    ]));
    launch.assert_started();
    assert_eq!(
        launch.env("CODEX_HOME").map(Path::new),
        Some(f.generated(NAME).as_path())
    );
    for file in OWNED {
        assert!(f.generated(NAME).join(file).is_file(), "{file}");
    }
}

/// `--reset-recovery` runs at start-up, before the name is checked.
#[test]
fn reset_recovery_runs_before_the_name_check() {
    let f = Fixture::new();
    let root = f.home().join(".amplihack/codex");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("pending.json"), "{}").unwrap();
    fs::set_permissions(root.join("pending.json"), fs::Permissions::from_mode(0o600)).unwrap();
    let launch = f.launch(&mut f.command(&["codex", "--reset-recovery", "--local-model", "../x"]));
    launch.assert_refused_at_run_time(&f, "invalid name after a reset");
    assert!(!root.join("pending.json").exists(), "{}", launch.stderr());
    assert!(
        fs::read_dir(&root).unwrap().flatten().any(|e| e
            .file_name()
            .to_string_lossy()
            .starts_with("pending.json.stale-")),
        "the journal is archived"
    );
}

/// A launch inside a local session, as an agent step of a recipe the
/// session runs would be: `CODEX_HOME` is the generated home.
fn nested(f: &Fixture, codex_home: &Path, name: &str) -> Launched {
    f.launch(
        f.command(&["codex", "--", "exec", "-"])
            .env("CODEX_HOME", codex_home)
            .env(LOCAL_MODEL_ENV, name)
            .env(HOOK_TEXT_ENV, "none"),
    )
}

/// a nested launch for the same name reuses the home without reading
/// the profile or writing a file.
#[test]
fn a_nested_launch_reuses_the_home() {
    let f = Fixture::new();
    f.local(NAME, &[]).assert_started();
    let home = f.generated(NAME);
    let before = snapshot(&home);
    let ids = inodes(&home);
    fs::remove_file(f.user_codex().join(format!("{NAME}.config.toml"))).unwrap();

    let launch = nested(&f, &home, NAME);
    launch.assert_started();
    assert_eq!(
        launch.env("CODEX_HOME").map(Path::new),
        Some(home.as_path())
    );
    assert_eq!(launch.env(HOOK_TEXT_ENV), Some("none"));
    assert_eq!(launch.env(LOCAL_MODEL_ENV), Some(NAME));
    assert_eq!(launch.count(HOOK_TRUST), 1, "{:?}", launch.argv());
    assert_eq!(inodes(&home), ids);
    assert_eq!(snapshot(&home), before);
}

#[test]
fn a_symlinked_amplihack_root_uses_canonical_hook_trust() {
    let f = Fixture::new();
    let root = f.home().join(".amplihack");
    let target = f.home().join("amplihack-target");
    fs::rename(&root, &target).unwrap();
    std::os::unix::fs::symlink(&target, &root).unwrap();
    f.local(NAME, &[]).assert_started();
    let home = f.generated(NAME);
    let canonical = fs::canonicalize(home.join("hooks.json")).unwrap();
    let key = format!("{}:pre_tool_use:0:0", canonical.display());
    let config = read_toml(&home.join("config.toml"));
    let state = config["hooks"]["state"].as_table().unwrap();
    assert_eq!(state.len(), 1);
    assert_eq!(state[&key]["enabled"].as_bool(), Some(true));
    let before = snapshot(&home);
    nested(&f, &home, NAME).assert_started();
    assert_eq!(snapshot(&home), before, "reuse writes nothing");
}

/// under another name, a `CODEX_HOME` inside `codex-local` is not a
/// user Codex home, so the profile comes from `~/.codex`.
#[test]
fn a_nested_launch_under_another_name_reads_the_user_codex_home() {
    let f = Fixture::new();
    f.write_profile("alpha", base_profile());
    f.write_profile("beta", base_profile());
    f.local("alpha", &[]).assert_started();
    let alpha = f.generated("alpha");
    let alpha_before = snapshot(&alpha);

    let launch = nested(&f, &alpha, "beta");
    launch.assert_started();
    let beta = f.generated("beta");
    assert_eq!(
        launch.env("CODEX_HOME").map(Path::new),
        Some(beta.as_path())
    );
    assert_eq!(
        read_toml(&beta.join("config.toml"))["model"].as_str(),
        Some("m")
    );
    assert_eq!(snapshot(&alpha), alpha_before, "home alpha is untouched");
    assert!(!alpha.join("beta.config.toml").exists());
}

/// reuse refuses a home whose guard has been weakened, and writes
/// nothing to it.
#[test]
fn reuse_refuses_a_weakened_home() {
    type Weaken = fn(&Path);
    let cases: [(&str, Weaken); 11] = [
        ("command whitespace changed without renewed trust", |home| {
            let path = home.join("hooks.json");
            let text = fs::read_to_string(&path)
                .unwrap()
                .replacen("env ", "env  ", 1);
            fs::write(path, text).unwrap();
        }),
        ("command quoting changed without renewed trust", |home| {
            let path = home.join("hooks.json");
            let text = fs::read_to_string(&path)
                .unwrap()
                .replacen("env ", "'env' ", 1);
            fs::write(path, text).unwrap();
        }),
        ("amplihack trust removed", |home| {
            let config = home.join("config.toml");
            let mut value = read_toml(&config);
            value["hooks"]["state"].as_table_mut().unwrap().clear();
            fs::write(config, toml::to_string(&value).unwrap()).unwrap();
        }),
        ("amplihack trust changed", |home| {
            let config = home.join("config.toml");
            let mut value = read_toml(&config);
            let key = format!("{}:pre_tool_use:0:0", home.join("hooks.json").display());
            value["hooks"]["state"][&key]["trusted_hash"] =
                toml::Value::String("sha256:changed".into());
            fs::write(config, toml::to_string(&value).unwrap()).unwrap();
        }),
        ("amplihack entry removed", |home| {
            let path = home.join("hooks.json");
            let mut hooks = read_json(&path);
            hooks["hooks"]["PreToolUse"]
                .as_array_mut()
                .unwrap()
                .remove(0);
            fs::write(&path, serde_json::to_string(&hooks).unwrap()).unwrap();
        }),
        ("rules file deleted", |home| {
            fs::remove_file(home.join(RULES_FILE)).unwrap();
        }),
        ("hooks.json readable by others", |home| {
            fs::set_permissions(home.join("hooks.json"), fs::Permissions::from_mode(0o644))
                .unwrap();
        }),
        ("hooks turned off", |home| {
            let path = home.join("config.toml");
            let text = fs::read_to_string(&path)
                .unwrap()
                .replace("hooks = true", "hooks = false");
            fs::write(&path, text).unwrap();
        }),
        ("config.toml unparseable", |home| {
            fs::write(home.join("config.toml"), "not = [toml\n").unwrap();
        }),
        // the session switched amplihack's handler off since the last
        // refresh, under the home's path as given and as canonicalised.
        ("amplihack's handler disabled in [hooks.state]", |home| {
            disable_command_check(home, home);
        }),
        (
            "amplihack's handler disabled under the canonical path",
            |home| {
                disable_command_check(home, &fs::canonicalize(home).unwrap());
            },
        ),
    ];
    for (what, weaken) in cases {
        let f = Fixture::new();
        f.local(NAME, &[]).assert_started();
        let home = f.generated(NAME);
        weaken(&home);
        let before = snapshot(&home);

        let launch = nested(&f, &home, NAME);
        assert!(!launch.out.status.success(), "{what}: refused");
        assert!(!launch.started(), "{what}: Codex not started");
        assert!(
            launch.stderr().contains(&home.display().to_string()),
            "{what}: the error names the home:\n{}",
            launch.stderr()
        );
        assert_eq!(snapshot(&home), before, "{what}: reuse writes nothing");
    }
}
