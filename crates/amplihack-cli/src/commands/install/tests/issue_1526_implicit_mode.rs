//! Issue #1526 — what an implicit re-stage may write when it does run.
//!
//! `issue_1526_implicit_downgrade.rs` pins the refusal (running < stamp). This
//! file pins the other rows of the implicit-mode table in
//! `docs/features/self-heal-asset-restage.md`: a re-stage triggered by startup
//! self-heal or the launch bootstrap (`InstallMode::Implicit`) never writes
//! `~/.claude/settings.json`, and replaces a binary in `~/.local/bin` only when
//! the running version is strictly newer than the stamp. With no stamp to
//! compare against it deploys only into an empty slot. `amplihack install`
//! (`InstallMode::Explicit`) keeps today's behaviour.

use super::helpers::*;
use super::issue_1526_implicit_downgrade::{
    IsolatedHome, assert_home_is, changes, newer_version, snapshot,
};
use super::*;
use std::fs;
use std::path::{Path, PathBuf};

/// A user's settings.json with no amplihack hooks: an explicit install would
/// merge hooks into it and back it up.
const USER_SETTINGS: &[u8] =
    b"{\n  \"model\": \"sentinel\",\n  \"permissions\": { \"allow\": [\"Bash(ls:*)\"] }\n}\n";

/// A stamp strictly below `crate::VERSION`.
fn older_version() -> String {
    let v = semver::Version::parse(crate::VERSION).expect("crate::VERSION is semver");
    if v.major > 0 {
        format!("{}.0.0", v.major - 1)
    } else if v.minor > 0 {
        format!("0.{}.0", v.minor - 1)
    } else {
        "0.0.0-0".to_string()
    }
}

// ── The rule (D3) ──

#[test]
fn implicit_deploy_rule_follows_the_table() {
    let running = crate::VERSION;
    let older = older_version();
    let newer = newer_version();
    // (destination exists, running, stamp) → allowed
    let cases: &[(bool, &str, Option<&str>, bool)] = &[
        // Empty slot: always fill it.
        (false, running, None, true),
        (false, running, Some(&older), true),
        (false, running, Some(&newer), true),
        (false, "not-semver", None, true),
        // Occupied slot: only a strictly newer running version replaces it.
        (true, running, Some(&older), true),
        (true, running, Some(running), false),
        (true, running, Some(&newer), false),
        (true, running, None, false),
        (true, running, Some("garbage"), false),
        (true, "not-semver", Some(&older), false),
        // Pre-release ranks below its release.
        (true, "1.0.0-rc.1", Some("1.0.0"), false),
        (true, "1.0.0", Some("1.0.0-rc.1"), true),
    ];
    let wrong: Vec<String> = cases
        .iter()
        .filter(|(exists, run, stamp, want)| {
            binary::implicit_deploy_allowed(*exists, run, *stamp) != *want
        })
        .map(|(exists, run, stamp, want)| {
            format!("exists={exists} running={run} stamp={stamp:?}: want {want}")
        })
        .collect();
    assert!(wrong.is_empty(), "rule violations:\n{}", wrong.join("\n"));
}

// ── The installer in implicit mode ──

/// A source repo, a hooks stub, and an isolated HOME. Stubs and the repo live
/// outside HOME so a HOME snapshot sees only what the installer writes.
struct Fixture {
    _home: tempfile::TempDir,
    _outside: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
    hooks_stub: PathBuf,
    _env: IsolatedHome,
}

impl Fixture {
    fn new() -> Self {
        let home_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let home = home_dir.path().to_path_buf();
        let stub_bin = outside.path().join("stub_bin");
        create_exe_stub(&stub_bin, "python3");
        let hooks_stub = create_exe_stub(&stub_bin, "amplihack-hooks");
        let runner_stub = create_exe_stub(&stub_bin, "recipe-runner-rs");
        let repo = outside.path().join("local-repo");
        create_source_repo(&repo);

        let mut env = IsolatedHome::new(&home);
        env.set("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        env.set("RECIPE_RUNNER_RS_PATH", &runner_stub);
        env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");
        let mut path_dirs = vec![stub_bin];
        path_dirs.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        env.set("PATH", std::env::join_paths(path_dirs).unwrap());
        Self {
            _home: home_dir,
            _outside: outside,
            home,
            repo,
            hooks_stub,
            _env: env,
        }
    }

    fn settings(&self) -> PathBuf {
        self.home.join(".claude").join("settings.json")
    }

    fn write_user_settings(&self) {
        let settings = self.settings();
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(settings, USER_SETTINGS).unwrap();
    }

    fn installed_hooks(&self) -> PathBuf {
        crate::path_conflicts::preferred_user_bin(&self.home)
            .join(crate::path_conflicts::binary_filename("amplihack-hooks"))
    }

    /// A binary an earlier install left in `~/.local/bin`.
    fn plant_installed_hooks(&self) -> Vec<u8> {
        let sentinel = format!("#!/bin/sh\n# installed hooks\n{}\n", "x".repeat(1100));
        let dst = self.installed_hooks();
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::write(&dst, &sentinel).unwrap();
        sentinel.into_bytes()
    }

    fn install(&self, mode: InstallMode) {
        assert_home_is(&self.home);
        local_install_scoped(&self.repo, None, false, mode)
            .unwrap_or_else(|e| panic!("{mode:?} install failed: {e:#}"));
    }
}

/// Settings writes or backups under HOME, as relative paths.
fn settings_writes(
    home: &Path,
    before: &std::collections::BTreeMap<PathBuf, Vec<u8>>,
) -> Vec<String> {
    changes(before, &snapshot(home))
        .into_iter()
        .filter(|c| c.contains("settings.json"))
        .collect()
}

/// A user's Copilot config holding an earlier install's amplihack entry, in
/// the user's own formatting, with the plugin disabled. An explicit install
/// would re-serialize it and stamp a fresh `installed_at`.
fn copilot_config(home: &Path) -> Vec<u8> {
    let plugin_dir = home.join(".copilot/installed-plugins/amplihack@local");
    format!(
        "// user header\n{{ \"theme\": \"dark\",\n  \"installedPlugins\": [ {{ \"name\": \"amplihack\", \
         \"enabled\": false, \"version\": \"0.0.1\", \"installed_at\": \"2020-01-01T00:00:00Z\", \
         \"cache_path\": {} }} ] }}\n",
        serde_json::to_string(&plugin_dir.display().to_string()).unwrap()
    )
    .into_bytes()
}

fn write_copilot_config(home: &Path) -> Vec<u8> {
    let body = copilot_config(home);
    fs::create_dir_all(home.join(".copilot")).unwrap();
    fs::write(home.join(".copilot/config.json"), &body).unwrap();
    body
}

/// The #1526 no-settings-rewrite path on an upgrade: running > stamp
/// re-stages and deploys, but leaves `settings.json` and the user's Copilot
/// `config.json` byte-identical and writes no backup of either.
#[test]
fn implicit_upgrade_restage_never_writes_settings_json() {
    let fx = Fixture::new();
    fx.write_user_settings();
    let copilot = write_copilot_config(&fx.home);
    version_stamp::write_installed_version(&older_version()).unwrap();
    let before = snapshot(&fx.home);

    fx.install(InstallMode::Implicit);

    assert_eq!(
        fs::read(fx.home.join(".copilot/config.json")).unwrap(),
        copilot,
        "an implicit re-stage rewrote ~/.copilot/config.json (#1526)"
    );
    assert!(
        fx.home
            .join(".copilot/installed-plugins/amplihack@local/hooks.json")
            .is_file(),
        "the amplihack-owned plugin directory is still refreshed"
    );

    assert_eq!(
        fs::read(fx.settings()).unwrap(),
        USER_SETTINGS,
        "an implicit re-stage rewrote ~/.claude/settings.json (#1526)"
    );
    assert_eq!(
        settings_writes(&fx.home, &before),
        Vec::<String>::new(),
        "an implicit re-stage must not write or back up settings.json"
    );
    assert_eq!(
        fs::read(fx.installed_hooks()).unwrap(),
        fs::read(&fx.hooks_stub).unwrap(),
        "a strictly newer running version still deploys its binaries"
    );
    assert_eq!(
        version_stamp::read_installed_version().unwrap().as_deref(),
        Some(crate::VERSION),
        "the re-stage still stamps the running version"
    );
}

/// With no settings.json at all, an implicit re-stage does not create one.
#[test]
fn implicit_restage_does_not_create_settings_json() {
    let fx = Fixture::new();
    version_stamp::write_installed_version(&older_version()).unwrap();

    fx.install(InstallMode::Implicit);

    assert!(
        !fx.settings().exists(),
        "an implicit re-stage must not create ~/.claude/settings.json"
    );
}

/// No stamp to compare against (the #502 self-heal): assets re-stage, but a
/// binary already in `~/.local/bin` is kept, because its version is unknown.
/// Security F1: the stamp records the framework as current and the binaries
/// as unknown. Stamping them at this build would record binaries it never
/// wrote, and let a later build that is newer than this one but older than
/// the kept binaries replace them.
#[test]
fn implicit_restage_without_a_stamp_keeps_installed_binaries_and_stamps_them_unknown() {
    let fx = Fixture::new();
    fx.write_user_settings();
    let installed = fx.plant_installed_hooks();
    let before = snapshot(&fx.home);

    fx.install(InstallMode::Implicit);

    assert_eq!(
        fs::read(fx.installed_hooks()).unwrap(),
        installed,
        "with no stamp, an implicit re-stage replaced an installed binary (#1526)"
    );
    assert_eq!(fs::read(fx.settings()).unwrap(), USER_SETTINGS);
    assert_eq!(settings_writes(&fx.home, &before), Vec::<String>::new());
    assert_eq!(
        version_stamp::read_installed_version().unwrap().as_deref(),
        Some(crate::VERSION),
        "the framework was re-staged, so the stamp records it as current"
    );
    assert_eq!(
        version_stamp::read_binaries_version().unwrap(),
        None,
        "a re-stage that kept an installed binary must not stamp v{} over it",
        crate::VERSION
    );
}

/// Review M1: the stamp check vouches only for this build's own binaries. A
/// hooks binary found elsewhere on PATH, of unknown version, must not replace
/// an installed one, even when this build is newer than the stamp.
#[test]
fn implicit_restage_never_replaces_hooks_with_a_path_copy() {
    let mut fx = Fixture::new();
    fx._env.unset("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let installed = fx.plant_installed_hooks();
    version_stamp::write_installed_version(&older_version()).unwrap();

    fx.install(InstallMode::Implicit);

    assert_eq!(
        fs::read(fx.installed_hooks()).unwrap(),
        installed,
        "an implicit re-stage replaced installed hooks with a PATH copy"
    );
    assert_eq!(
        version_stamp::read_binaries_version().unwrap().as_deref(),
        Some(older_version().as_str()),
        "the kept hooks binary is still at the old stamp's version"
    );

    // The mixed state crusty flagged: `amplihack` was upgraded (its source is
    // the running executable) while hooks were kept, so the `binaries:`
    // version above is not the version of ~/.local/bin/amplihack.
    let installed_self = crate::path_conflicts::preferred_user_bin(&fx.home)
        .join(crate::path_conflicts::binary_filename("amplihack"));
    let running_self =
        crate::commands::install::resolve_running_binary_source(&std::env::current_exe().unwrap());
    assert_eq!(
        fs::read(&installed_self).unwrap(),
        fs::read(&running_self).unwrap(),
        "fixture: the implicit re-stage must upgrade ~/.local/bin/amplihack while keeping hooks"
    );
    // A later, older build's refusal must not print the kept version beside
    // the upgraded binary's path.
    let framework = version_stamp::read_installed_version_quiet()
        .unwrap()
        .expect("stamp written");
    crate::self_heal::set_downgrade_warned(false);
    let mut notice = Vec::new();
    crate::self_heal::warn_implicit_downgrade_once(&mut notice, &framework);
    let notice = String::from_utf8(notice).unwrap();
    let line = notice
        .lines()
        .find(|l| l.trim_start().starts_with("installed:"))
        .unwrap_or_else(|| panic!("no `installed:` line in the warning:\n{notice}"));
    assert_eq!(
        line,
        format!(
            "  installed: {}; framework v{framework}; binaries kept from v{}",
            paths::sanitize_path(&installed_self),
            older_version()
        ),
        "the warning attributed the kept hooks' version to the upgraded amplihack binary"
    );
}

/// Review M2: from the pre-#502 state (binaries installed, no stamp), the
/// first launch re-stages and stamps the framework; the second neither
/// re-stages nor replaces a binary.
#[test]
fn pre_502_state_restages_once_then_stops() {
    let fx = Fixture::new();
    let installed = fx.plant_installed_hooks();
    let installs = std::cell::Cell::new(0);
    let launch = |notice: &mut Vec<u8>| {
        crate::self_heal::ensure_assets_match_binary_version_with(
            &super::issue_1526_implicit_downgrade::args(&["amplihack", "launch"]),
            notice,
            || {
                installs.set(installs.get() + 1);
                local_install_scoped(&fx.repo, None, false, InstallMode::Implicit)
            },
        )
        .expect("launch")
    };

    let mut first = Vec::new();
    launch(&mut first);
    assert_eq!(installs.get(), 1, "the first launch re-stages");
    assert!(String::from_utf8_lossy(&first).contains("re-staged"));

    let before = snapshot(&fx.home);
    let mut second = Vec::new();
    launch(&mut second);
    assert_eq!(installs.get(), 1, "the second launch must not re-stage");
    assert!(second.is_empty(), "the second launch printed {second:?}");
    let changed: Vec<String> = changes(&before, &snapshot(&fx.home))
        .into_iter()
        .filter(|c| !c.contains(crate::commands::install::bundle_compat_cache::CACHE_FILE))
        .collect();
    assert_eq!(
        changed,
        Vec::<String>::new(),
        "the second launch wrote files"
    );
    assert_eq!(fs::read(fx.installed_hooks()).unwrap(), installed);
}

/// The same with an equal stamp: an equal version is not newer, so the
/// installed binary stays.
#[test]
fn implicit_restage_at_the_stamped_version_keeps_installed_binaries() {
    let fx = Fixture::new();
    let installed = fx.plant_installed_hooks();
    version_stamp::write_installed_version(crate::VERSION).unwrap();

    fx.install(InstallMode::Implicit);

    assert_eq!(fs::read(fx.installed_hooks()).unwrap(), installed);
}

/// No stamp and an empty `~/.local/bin`: the binary is deployed, so a fresh
/// machine's first launch still ends up with working hooks.
#[test]
fn implicit_restage_without_a_stamp_fills_an_empty_bin_dir() {
    let fx = Fixture::new();

    fx.install(InstallMode::Implicit);

    assert_eq!(
        fs::read(fx.installed_hooks()).unwrap(),
        fs::read(&fx.hooks_stub).unwrap(),
        "an empty slot is always filled"
    );
}

/// Explicit `amplihack install` is unchanged: it replaces binaries whatever
/// the stamp says (a deliberate downgrade) and merges hooks into settings.json.
#[test]
fn explicit_install_still_replaces_binaries_and_writes_settings() {
    let fx = Fixture::new();
    fx.write_user_settings();
    fx.plant_installed_hooks();
    version_stamp::write_installed_version(&newer_version()).unwrap();

    fx.install(InstallMode::Explicit);

    assert_eq!(
        fs::read(fx.installed_hooks()).unwrap(),
        fs::read(&fx.hooks_stub).unwrap(),
        "explicit install must still downgrade deliberately"
    );
    let settings = fs::read_to_string(fx.settings()).unwrap();
    assert!(
        settings.contains("amplihack-hooks"),
        "explicit install must still register hooks in settings.json; got:\n{settings}"
    );
    assert!(
        settings.contains("sentinel"),
        "explicit install merges, it does not discard user settings"
    );
}

/// An implicit re-stage that keeps binaries and settings.json must keep the
/// earlier install's manifest entries for them, so `amplihack uninstall`
/// still finds the hook registrations and the kept binaries.
#[test]
fn implicit_restage_carries_forward_manifest_entries_it_did_not_write() {
    let fx = Fixture::new();
    fx.write_user_settings();
    fx.install(InstallMode::Explicit);
    let path = manifest::manifest_path().unwrap();
    let before = manifest::read_manifest(&path).unwrap();
    assert!(
        !before.hook_registrations.is_empty(),
        "fixture: the explicit install must register hooks"
    );
    // Not the hooks binary: `deploy_binaries` lists it whether kept or not,
    // so only a binary absent from that list exercises the carry-forward.
    let kept = crate::path_conflicts::preferred_user_bin(&fx.home)
        .join(crate::path_conflicts::binary_filename("amplihack"))
        .to_string_lossy()
        .into_owned();
    assert!(
        before.binaries.contains(&kept),
        "fixture: the explicit install must deploy and record {kept}"
    );

    // No stamp: the re-stage keeps every installed binary.
    fs::remove_file(version_stamp::installed_version_path().unwrap()).unwrap();
    fx.install(InstallMode::Implicit);

    let after = manifest::read_manifest(&path).unwrap();
    assert_eq!(
        after.hook_registrations, before.hook_registrations,
        "an implicit re-stage dropped the hook registrations it left in place"
    );
    assert!(
        after.binaries.contains(&kept),
        "an implicit re-stage dropped a kept binary from the manifest"
    );
}

/// With no amplihack entry in `config.json` yet, an implicit re-stage
/// registers one: that is a first install, not a rewrite. The user's other
/// keys survive.
#[test]
fn implicit_restage_registers_copilot_plugin_only_when_absent() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.home.join(".copilot")).unwrap();
    fs::write(
        fx.home.join(".copilot/config.json"),
        b"{ \"theme\": \"dark\" }\n",
    )
    .unwrap();
    version_stamp::write_installed_version(&older_version()).unwrap();

    fx.install(InstallMode::Implicit);

    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(fx.home.join(".copilot/config.json")).unwrap()).unwrap();
    assert_eq!(config["theme"], "dark");
    assert_eq!(config["installedPlugins"][0]["name"], "amplihack");
}

/// `amplihack install` still rewrites an existing entry.
#[test]
fn explicit_install_still_refreshes_copilot_config() {
    let fx = Fixture::new();
    let copilot = write_copilot_config(&fx.home);

    fx.install(InstallMode::Explicit);

    let after = fs::read(fx.home.join(".copilot/config.json")).unwrap();
    assert_ne!(after, copilot, "explicit install must refresh the entry");
    assert!(String::from_utf8_lossy(&after).contains(crate::VERSION));
}

/// An implicit re-stage with Codex selected leaves an installed Codex plugin
/// alone: it never runs Codex, and never touches `~/.codex` or the ledger.
#[test]
fn implicit_restage_leaves_an_installed_codex_plugin_alone() {
    let mut fx = Fixture::new();
    let marker = fx._outside.path().join("codex-ran");
    let codex_bin = fx._outside.path().join("codex_bin");
    fs::create_dir_all(&codex_bin).unwrap();
    let codex = codex_bin.join("codex");
    fs::write(
        &codex,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut path_dirs = vec![codex_bin];
    path_dirs.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    fx._env
        .set("PATH", std::env::join_paths(path_dirs).unwrap());
    fs::create_dir_all(fx.home.join(".amplihack/codex")).unwrap();
    fs::write(
        fx.home.join(".amplihack/codex/ownership.json"),
        b"{\"schema_version\":1}",
    )
    .unwrap();
    fs::create_dir_all(fx.home.join(".codex")).unwrap();
    fs::write(fx.home.join(".codex/hooks.json"), b"{\"hooks\":{}}\n").unwrap();
    fs::write(fx.home.join(".codex/config.toml"), b"model = \"x\"\n").unwrap();
    version_stamp::write_installed_version(&older_version()).unwrap();
    let before = snapshot(&fx.home);

    assert_home_is(&fx.home);
    local_install_scoped(&fx.repo, None, true, InstallMode::Implicit)
        .unwrap_or_else(|e| panic!("implicit install failed: {e:#}"));

    let codex_changes: Vec<String> = changes(&before, &snapshot(&fx.home))
        .into_iter()
        .filter(|c| c.contains(".codex") || c.contains("amplihack/codex"))
        .collect();
    assert_eq!(codex_changes, Vec::<String>::new());
    assert!(
        !marker.exists(),
        "an implicit re-stage ran the Codex binary"
    );
}

// ── Hook drift after an implicit upgrade ──

fn settings_with(specs: &[&types::HookSpec]) -> tempfile::NamedTempFile {
    let mut hooks = serde_json::Map::new();
    for spec in specs {
        let types::HookCommandKind::BinarySubcmd { subcmd } = spec.cmd;
        let mut wrapper = serde_json::json!({ "hooks": [{ "type": "command",
            "command": format!("/home/u/.local/bin/amplihack-hooks {subcmd}") }] });
        if let Some(matcher) = spec.matcher {
            wrapper["matcher"] = serde_json::json!(matcher);
        }
        hooks
            .entry(spec.event.to_string())
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .unwrap()
            .push(wrapper);
    }
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(
        file.path(),
        serde_json::json!({ "hooks": hooks }).to_string(),
    )
    .unwrap();
    file
}

fn drift_notice(path: &Path) -> String {
    let mut notice = Vec::new();
    warn_implicit_hook_drift(&mut notice, path);
    String::from_utf8(notice).unwrap()
}

/// settings.json registered by an older release lacks a hook this build
/// adds: the warning names it and `amplihack install`, on the notice stream.
#[test]
fn hook_drift_names_the_missing_events() {
    let first = &types::AMPLIHACK_HOOK_SPECS[0];
    let types::HookCommandKind::BinarySubcmd { subcmd } = first.cmd;
    let file = settings_with(&[first]);

    let notice = drift_notice(file.path());

    assert!(notice.contains("amplihack install"), "{notice}");
    for spec in &types::AMPLIHACK_HOOK_SPECS[1..] {
        let types::HookCommandKind::BinarySubcmd { subcmd } = spec.cmd;
        assert!(
            notice.contains(&format!("{} ({subcmd})", spec.event)),
            "{notice}"
        );
    }
    assert!(
        !notice.contains(&format!("{} ({subcmd})", first.event)),
        "{notice}"
    );
}

/// Nothing missing, nothing printed: no unconditional notice.
#[test]
fn hook_drift_is_silent_when_settings_are_current() {
    let all: Vec<&types::HookSpec> = types::AMPLIHACK_HOOK_SPECS.iter().collect();
    let file = settings_with(&all);
    assert_eq!(drift_notice(file.path()), "");
}

/// A file with no amplihack hooks, a missing file and a non-JSON file are
/// not drift; the launch bootstrap handles them.
#[test]
fn hook_drift_ignores_unconfigured_missing_and_unparseable_files() {
    let empty = settings_with(&[]);
    assert_eq!(drift_notice(empty.path()), "");
    assert_eq!(drift_notice(Path::new("/nonexistent/settings.json")), "");
    let bad = tempfile::NamedTempFile::new().unwrap();
    fs::write(bad.path(), b"{ not json").unwrap();
    assert_eq!(drift_notice(bad.path()), "");
}
