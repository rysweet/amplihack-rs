//! Issue #2006: Claude picks and Claude launch pins
//! (docs/reference/account-pick.md, "Candidates", "Launch integration",
//! "Pins" and "The off switch and the rotator").
//!
//! Uses the `profile rotate` fixture (`support::profile_rotate`): `work` is
//! the live login in `~/.claude` (so never a candidate), `spare` reads
//! 10/20 (live headroom 80) and `extra` 50/50 (headroom 50). Usage is read
//! with `AMPLIHACK_PROFILE_USAGE_PROBE=text` through the fixture's fake
//! `claude`. A wrapper in front of that fake records the environment of the
//! launch itself, recognised by the `MARKER2006` prompt, to
//! `$RECORD_DIR/launch.env`, and passes every other call (the probes)
//! through. Since #2030 the launch receives a `-p` prompt on stdin with a
//! bare `-p` in argv, so the wrapper reads stdin for the marker on any
//! print-mode call that is not a usage probe (`--input-format`).

#![cfg(unix)]

use crate::support::exec_stub::{copy_executable, write_executable};
use crate::support::profile_rotate::Fixture;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const MARKER: &str = "MARKER2006";
const WRAPPER: &str = r#"#!/bin/sh
record() {
  {
    printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}"
    printf 'AMPLIHACK_CLAUDE_PROFILE=%s\n' "${AMPLIHACK_CLAUDE_PROFILE-<unset>}"
  } > "$RECORD_DIR/launch.env"
  exit 0
}
case " $* " in
  *" MARKER2006 "*) record ;;
  *" --input-format "*) ;;
  *" -p "*)
    payload=$(cat)
    case "$payload" in
      *MARKER2006*) record ;;
    esac
    printf '%s' "$payload" | exec "$(dirname "$0")/claude-fixture" "$@" ;;
esac
exec "$(dirname "$0")/claude-fixture" "$@"
"#;

struct Cx {
    f: Fixture,
    leases: PathBuf,
}

impl Cx {
    fn new() -> Self {
        let f = Fixture::new();
        copy_executable(&f.tools.join("claude"), &f.tools.join("claude-fixture"));
        write_executable(&f.tools.join("claude"), WRAPPER, 0o755);
        let leases = f.home.join("leases");
        Self { f, leases }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = self.f.cmd(args);
        c.env("AMPLIHACK_PROFILE_USAGE_PROBE", "text")
            .env("AMPLIHACK_ACCOUNT_LEASE_DIR", &self.leases)
            .env("AMPLIHACK_ACCOUNT_LEASE_TTL_SECS", "86400")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("AMPLIHACK_CLAUDE_CONFIG_DIR")
            .env_remove("AMPLIHACK_CLAUDE_PROFILE")
            .env_remove("AMPLIHACK_ACCOUNT_PICK");
        c
    }

    fn launch(&self, pre: &[&str], set: &[(&str, &str)]) -> Output {
        let mut args = vec!["claude"];
        args.extend_from_slice(pre);
        args.extend(["--subprocess-safe", "--", "-p", MARKER]);
        let mut c = self.cmd(&args);
        for (k, v) in set {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn launch_var(&self, out: &Output, key: &str) -> String {
        let raw = fs::read_to_string(self.f.records.join("launch.env")).unwrap_or_else(|_| {
            panic!(
                "the launch never reached claude\nstatus: {}\nstderr: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            )
        });
        let prefix = format!("{key}=");
        raw.lines()
            .find_map(|l| l.strip_prefix(&prefix))
            .unwrap_or("<missing>")
            .to_string()
    }

    fn lease_files(&self) -> Vec<PathBuf> {
        fs::read_dir(self.leases.join("claude"))
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "json"))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn profile(&self, name: &str) -> PathBuf {
        self.f.root.join(name)
    }
}

fn text(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn claude_pick_chooses_the_profile_with_most_headroom_never_the_live_one() {
    let cx = Cx::new();
    let out = cx
        .cmd(&["account", "pick", "--cli", "claude", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["cli"], "claude");
    assert_eq!(v["account"], "spare");
    assert_eq!(v["home"], cx.profile("spare").to_str().unwrap());
    assert_eq!(v["live_headroom"], 80.0);
    let names: Vec<&str> = v["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["account"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["spare", "extra"], "work is live in ~/.claude");
    let home_claude = cx.f.home.join(".claude");
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains(home_claude.to_str().unwrap()),
        "~/.claude is never a candidate"
    );
    assert_eq!(cx.lease_files().len(), 1);
}

/// The reference requires the Claude home to be exactly what
/// `amplihack claude --profile <name>` sets, or a launch would not be
/// counted as a job on the picked account.
#[test]
fn the_picked_home_equals_the_config_dir_a_profile_launch_sets() {
    let cx = Cx::new();
    let out = cx
        .cmd(&["account", "pick", "--cli", "claude"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let picked = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    let launch = cx.launch(&["--profile", "spare"], &[]);
    assert!(launch.status.success(), "{}", text(&launch));
    assert_eq!(cx.launch_var(&launch, "CLAUDE_CONFIG_DIR"), picked);
}

#[test]
fn an_unpinned_claude_launch_runs_on_the_picked_profile() {
    let cx = Cx::new();
    let out = cx.launch(&[], &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        cx.launch_var(&out, "CLAUDE_CONFIG_DIR"),
        cx.profile("spare").to_str().unwrap()
    );
    assert_eq!(
        cx.launch_var(&out, "AMPLIHACK_CLAUDE_PROFILE"),
        "spare",
        "the picked profile is pinned for nested launches"
    );
    assert!(
        cx.lease_files().is_empty(),
        "the lease is dropped after the launch"
    );
}

#[test]
fn an_explicit_profile_is_never_changed_and_writes_no_lease() {
    let cx = Cx::new();
    let out = cx.launch(&["--profile", "extra"], &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        cx.launch_var(&out, "CLAUDE_CONFIG_DIR"),
        cx.profile("extra").to_str().unwrap()
    );
    assert!(cx.lease_files().is_empty());
    assert!(!cx.leases.join("claude").exists() || cx.lease_files().is_empty());
}

#[test]
fn an_inherited_profile_is_never_changed() {
    let cx = Cx::new();
    let out = cx.launch(&[], &[("AMPLIHACK_CLAUDE_PROFILE", "extra")]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        cx.launch_var(&out, "CLAUDE_CONFIG_DIR"),
        cx.profile("extra").to_str().unwrap()
    );
    assert!(cx.lease_files().is_empty());
}

#[test]
fn a_raw_claude_config_dir_passes_through_unchanged() {
    let cx = Cx::new();
    let raw = cx.f.home.join("raw-claude");
    fs::create_dir_all(&raw).unwrap();
    let out = cx.launch(&[], &[("CLAUDE_CONFIG_DIR", raw.to_str().unwrap())]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        cx.launch_var(&out, "CLAUDE_CONFIG_DIR"),
        raw.to_str().unwrap()
    );
    assert!(cx.lease_files().is_empty());
}

#[test]
fn the_alias_alone_pins_and_restores_claude_config_dir() {
    let cx = Cx::new();
    let raw = cx.f.home.join("raw-claude");
    fs::create_dir_all(&raw).unwrap();
    let out = cx.launch(
        &[],
        &[("AMPLIHACK_CLAUDE_CONFIG_DIR", raw.to_str().unwrap())],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        cx.launch_var(&out, "CLAUDE_CONFIG_DIR"),
        raw.to_str().unwrap()
    );
    assert!(cx.lease_files().is_empty());
}

#[test]
fn the_codex_only_switch_keeps_claude_launches_on_home_claude() {
    for value in ["codex", "off"] {
        let cx = Cx::new();
        let out = cx.launch(&[], &[("AMPLIHACK_ACCOUNT_PICK", value)]);
        assert!(out.status.success(), "{value}: {}", text(&out));
        assert_eq!(
            cx.launch_var(&out, "CLAUDE_CONFIG_DIR"),
            "<unset>",
            "{value}: a Claude launch picked a profile"
        );
        assert!(cx.lease_files().is_empty(), "{value}");
    }
}
