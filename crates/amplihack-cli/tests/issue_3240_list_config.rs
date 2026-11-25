//! Public configuration writes validate selection before changing user bytes.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Output;

struct Install {
    home: tempfile::TempDir,
}

impl Install {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(home.path().join(".amplihack")).unwrap();
        let install = Self { home };
        fs::write(install.config(), "# keep\n[recipe]\nverbose = true\n").unwrap();
        fs::set_permissions(install.config(), fs::Permissions::from_mode(0o600)).unwrap();
        install
    }

    fn config(&self) -> PathBuf {
        self.home.path().join(".amplihack/config.toml")
    }

    fn index(&self) -> PathBuf {
        self.home
            .path()
            .join(".amplihack/.claude/install/skills-and-agents-index.tsv")
    }

    fn write_index(&self) {
        let staged = self.home.path().join(".amplihack/.claude");
        fs::create_dir_all(self.index().parent().unwrap()).unwrap();
        fs::write(self.index(), format!(
            "skill\tcode-visualizer\t{}/skills/code-visualizer/SKILL.md\tDraw code\nagent\tguide\t{}/agents/core/guide.md\tGuide\n",
            staged.display(), staged.display()
        )).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin())
            .args(args)
            .current_dir(self.home.path())
            .env("HOME", self.home.path())
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .output()
            .unwrap()
    }
}

#[test]
fn list_settings_report_defaults_and_persist_typed_extensions() {
    let install = Install::new();
    let default = install.run(&["config", "get", "lists.set"]);
    assert!(default.status.success(), "{default:?}");
    assert_eq!(default.stdout, b"default\n");
    let enabled = install.run(&["config", "get", "lists.enabled"]);
    assert!(enabled.status.success());
    assert_eq!(enabled.stdout, b"\n");
    install.write_index();
    let index = fs::read(install.index()).unwrap();
    let result = install.run(&[
        "config",
        "set",
        "lists.enabled",
        r#"["skill:code-visualizer", "agent:core/guide", "skill:code-visualizer"]"#,
    ]);
    assert!(result.status.success(), "{result:?}");
    let document = fs::read_to_string(install.config())
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    let values: Vec<_> = document["lists"]["enabled"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(values, ["agent:core/guide", "skill:code-visualizer"]);
    assert_eq!(fs::read(install.index()).unwrap(), index);
    assert!(
        fs::read_to_string(install.config())
            .unwrap()
            .starts_with("# keep\n[recipe]\nverbose = true\n")
    );
}

#[test]
fn invalid_or_unknown_entries_leave_configuration_unchanged() {
    let install = Install::new();
    install.write_index();
    let original = fs::read(install.config()).unwrap();
    for raw in [
        r#"["skill:no-such-entry"]"#,
        r#"["agent:guide"]"#,
        r#"["guide"]"#,
        "[1]",
        "true",
    ] {
        let result = install.run(&["config", "set", "lists.enabled", raw]);
        assert!(!result.status.success(), "{raw}: {result:?}");
        assert_eq!(fs::read(install.config()).unwrap(), original);
    }
    let result = install.run(&["config", "set", "lists.set", "everything"]);
    assert!(!result.status.success());
    assert_eq!(fs::read(install.config()).unwrap(), original);
}

#[test]
fn missing_index_rejects_extensions_without_changing_configuration() {
    let install = Install::new();
    let original = fs::read(install.config()).unwrap();
    let result = install.run(&[
        "config",
        "set",
        "lists.enabled",
        r#"["skill:code-visualizer"]"#,
    ]);
    assert!(!result.status.success());
    assert_eq!(fs::read(install.config()).unwrap(), original);
    assert!(!install.index().exists());
}

#[test]
fn all_mode_and_empty_extensions_can_be_saved_without_an_index() {
    let install = Install::new();
    for (key, value) in [("lists.set", "all"), ("lists.enabled", "[]")] {
        let result = install.run(&["config", "set", key, value]);
        assert!(result.status.success(), "{result:?}");
    }
    let document = fs::read_to_string(install.config())
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(document["lists"]["set"].as_str(), Some("all"));
    assert!(document["lists"]["enabled"].as_array().unwrap().is_empty());
    assert!(!install.index().exists());
}

#[test]
fn trailing_colon_discovery_has_no_matches_and_changes_no_files() {
    let install = Install::new();
    install.write_index();
    let config = fs::read(install.config()).unwrap();
    let index = fs::read(install.index()).unwrap();
    for word in ["amplihack:", "amplihack:core:"] {
        let result = install.run(&["directory", "find", word]);
        assert!(result.status.success(), "{result:?}");
        assert!(result.stdout.is_empty());
        assert!(!result.stderr.is_empty());
        assert_eq!(fs::read(install.config()).unwrap(), config);
        assert_eq!(fs::read(install.index()).unwrap(), index);
    }
}

#[test]
fn directory_mutations_accept_qualified_names_without_installing_another_tool() {
    let install = Install::new();
    install.write_index();
    let config = fs::read(install.config()).unwrap();
    let index = fs::read(install.index()).unwrap();
    for name in ["amplihack:code-visualizer", "amplihack:core:guide"] {
        for _ in 0..2 {
            let result = install.run(&["directory", "enable", name]);
            assert!(result.status.success(), "{result:?}");
        }
        for _ in 0..2 {
            let result = install.run(&["directory", "disable", name]);
            assert!(result.status.success(), "{result:?}");
        }
        assert_eq!(fs::read(install.config()).unwrap(), config);
    }
    assert_eq!(fs::read(install.index()).unwrap(), index);
    for tool in [".claude", ".codex", ".copilot"] {
        assert!(!install.home.path().join(tool).exists());
    }
    for name in ["amplihack:", "unknown", "agent:missing"] {
        let result = install.run(&["directory", "enable", name]);
        assert!(!result.status.success());
        assert_eq!(fs::read(install.config()).unwrap(), config);
    }
}
