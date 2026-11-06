//! A fake CLI's record of one invocation, and the fixture self-checks.

use super::fixture::{Cli, Fixture, markers_of};
use super::script::marker_names;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One invocation of a fake CLI.
#[derive(Debug)]
pub struct Record {
    fields: BTreeMap<String, String>,
    logs: String,
}

impl Record {
    pub(super) fn parse(text: &str, fx: &Fixture) -> Self {
        let known = [
            "seq",
            "ran",
            "self",
            "role",
            "argv",
            "binary",
            "source",
            "nonce_set",
            "children_see",
        ];
        let markers = marker_names();
        let mut fields = BTreeMap::new();
        for line in text.lines() {
            let (key, value) = line
                .split_once('=')
                .unwrap_or_else(|| panic!("bad record line {line:?} in\n{text}"));
            assert!(
                known.contains(&key) || markers.contains(&key),
                "unknown record key {key:?} in\n{text}"
            );
            fields.insert(key.to_string(), value.to_string());
        }
        let record = Self {
            fields,
            logs: fx.logs(),
        };
        let tools = fx.tools().canonicalize().unwrap();
        assert!(
            Path::new(record.get("self")).starts_with(&tools),
            "a fake outside {} ran: {text}",
            tools.display()
        );
        record
    }

    pub fn get(&self, key: &str) -> &str {
        self.fields
            .get(key)
            .unwrap_or_else(|| panic!("record has no {key}: {self:?}"))
    }

    /// `cli` ran, was handed `cli` tagged `session:<cli>`, and any command it
    /// starts resolves `cli`.
    pub fn assert_cli(&self, cli: Cli) {
        let want = [
            ("ran", cli.name().to_string()),
            ("binary", cli.name().to_string()),
            ("source", format!("session:{}", cli.name())),
            ("children_see", cli.name().to_string()),
        ];
        for (key, value) in want {
            assert_eq!(
                self.get(key),
                value,
                "{key} of the {} record; record: {:#?}\n{}",
                self.get("role"),
                self.fields,
                self.logs
            );
        }
    }

    /// No marker variable of `cli` reached this fake.
    pub fn assert_no_markers_of(&self, cli: Cli) {
        for name in markers_of(cli) {
            assert_eq!(
                self.get(name),
                "<unset>",
                "{name} reached the {} record of {}; record: {:#?}",
                self.get("role"),
                self.get("ran"),
                self.fields
            );
        }
    }
}

#[test]
fn marker_names_are_shell_safe_and_include_every_claude_marker() {
    let names = marker_names();
    for claude in markers_of(Cli::Claude) {
        assert!(names.contains(&claude));
    }
    assert!(names.contains(&"CLAUDECODE"));
}

#[test]
fn fixture_command_sets_no_secret_variables() {
    let fx = Fixture::new();
    let c = fx.cmd(["--version"]);
    for (key, value) in c.get_envs() {
        let key = key.to_string_lossy().to_ascii_uppercase();
        let Some(value) = value else { continue };
        // `no_git_prompt` sets SSH_ASKPASS=true so no git can prompt.
        if key == "SSH_ASKPASS" && value == "true" {
            continue;
        }
        let bad = ["TOKEN", "KEY", "SECRET"].iter().any(|w| key.contains(w))
            || key.starts_with("SSH_")
            || key == "CLAUDE_CONFIG_DIR"
            || key == "DBUS_SESSION_BUS_ADDRESS";
        assert!(!bad, "fixture command sets {key}");
    }
}

#[test]
fn fixture_path_finds_only_the_fakes() {
    let fx = Fixture::new();
    let tools = fx.tools();
    for cli in ["claude", "codex", "recipe-runner-rs"] {
        let out = Command::new("/bin/sh")
            .args(["-c", "command -v \"$1\"", "sh", cli])
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", tools.display()),
            )
            .output()
            .unwrap();
        let found = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        assert!(
            found.starts_with(&tools),
            "{cli} resolves to {}",
            found.display()
        );
    }
}
