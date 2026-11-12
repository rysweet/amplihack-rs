//! #1728: failed probes end to end through `install`.
use super::probe_tests::*;
use super::*;

// End-to-end through `install`: fakes stand in for the selected Codex.
struct InstallFixture {
    _dir: tempfile::TempDir,
    _env: crate::test_support::EnvGuard,
    home: PathBuf,
    log: PathBuf,
    binary: PathBuf,
}

/// `list_body` handles `plugin list --json`; `help_text`/`help_code` answer
/// `plugin --help`. Every call is logged by its arguments.
fn install_fixture(list_body: &str, help_text: &str, help_code: i32) -> InstallFixture {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let codex_home = home.join(".codex");
    fs::create_dir_all(&codex_home).unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = dir.path().join("calls");
    let body = format!(
        "printf '%s\\n' \"$*\" >> '{log}'\ncase \"$*\" in\n  'plugin --help')\n{help}\n    ;;\n  *)\n{list}\n    ;;\nesac",
        log = log.display(),
        help = help_body(help_text, help_code),
        list = list_body,
    );
    let binary = probe_fake(&bin, "codex", &body);
    let _env = crate::test_support::EnvGuard::set([
        ("HOME", home.to_str().unwrap()),
        ("CODEX_HOME", codex_home.to_str().unwrap()),
    ]);
    InstallFixture {
        _dir: dir,
        _env,
        home,
        log,
        binary,
    }
}

impl InstallFixture {
    fn install_as(&self, provider: &str) -> Result<bool> {
        let source = self.home.join("unused-source");
        let hooks = self.home.join("unused-hooks");
        crate::freshness::with_provider(provider, || {
            super::with_binary(&self.binary, || super::install(&source, &hooks))
        })
    }
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
    fn staging_root_created(&self) -> bool {
        self.home.join(".amplihack/codex").exists()
    }
}

#[test]
fn selected_codex_with_plugin_support_reports_probe_exit_not_update_advice() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let fixture = install_fixture("printf 'stdout-canary'\nexit 1", PLUGIN_HELP, 0);
    let error = fixture.install_as("codex").unwrap_err();
    let rendered = format!("{error:#}");
    assert!(rendered.contains("supports native plugins"), "{rendered}");
    assert!(rendered.contains("exit status: 1"), "{rendered}");
    assert!(!rendered.contains("update Codex"), "{rendered}");
    assert_no_child_output(&rendered);
    assert_eq!(fixture.calls(), ["plugin list --json", "plugin --help"]);
    assert!(
        !fixture.staging_root_created(),
        "install mutated after a failed probe"
    );
}

#[test]
fn selected_codex_without_plugin_subcommand_keeps_update_advice() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let fixture = install_fixture("exit 2", TOP_LEVEL_HELP, 0);
    let error = fixture.install_as("codex").unwrap_err();
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("requires native plugin support; update Codex and retry"),
        "{rendered}"
    );
    assert_no_child_output(&rendered);
    assert_eq!(fixture.calls(), ["plugin list --json", "plugin --help"]);
    assert!(!fixture.staging_root_created());
}

#[test]
fn selected_codex_with_invalid_inventory_reports_parse_failure() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let fixture = install_fixture("printf 'not-json stdout-canary'\nexit 0", PLUGIN_HELP, 0);
    let error = fixture.install_as("codex").unwrap_err();
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("invalid native Codex plugin response"),
        "{rendered}"
    );
    assert!(!rendered.contains("update Codex"), "{rendered}");
    assert_no_child_output(&rendered);
    assert!(!fixture.staging_root_created());
}

// T7: when Codex is not the selected provider, a failed probe skips the
// optional plugin and never runs the capability check.
#[test]
fn unselected_codex_probe_failure_skips_without_capability_check() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let fixture = install_fixture("exit 1", PLUGIN_HELP, 0);
    assert!(!fixture.install_as("claude").unwrap());
    assert_eq!(fixture.calls(), ["plugin list --json"]);
    assert!(!fixture.staging_root_created());
}
