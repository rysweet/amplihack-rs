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
fn settings_writes(home: &Path, before: &std::collections::BTreeMap<PathBuf, Vec<u8>>) -> Vec<String> {
    changes(before, &snapshot(home))
        .into_iter()
        .filter(|c| c.contains("settings.json"))
        .collect()
}

/// The #1526 no-settings-rewrite path on an upgrade: running > stamp
/// re-stages and deploys, but leaves `settings.json` byte-identical and
/// writes no backup of it.
#[test]
fn implicit_upgrade_restage_never_writes_settings_json() {
    let fx = Fixture::new();
    fx.write_user_settings();
    version_stamp::write_installed_version(&older_version()).unwrap();
    let before = snapshot(&fx.home);

    fx.install(InstallMode::Implicit);

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
/// binary already in `~/.local/bin` is kept. Its version is unknown, so
/// replacing it could be the #1526 downgrade.
#[test]
fn implicit_restage_without_a_stamp_keeps_installed_binaries() {
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
        "assets still re-stage with no stamp (#502)"
    );
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
