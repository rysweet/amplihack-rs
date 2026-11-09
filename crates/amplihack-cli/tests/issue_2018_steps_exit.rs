//! Issue #2018 (part 2): `amplihack recipe steps` exit codes and usage
//! errors (D9), and the empty listings that still exit 0.
//!
//! Linux only, like the rest of the issue #2018 integration tests.

#![cfg(target_os = "linux")]

use crate::support::step_2018::{Fx, states, text};
use crate::support::step_2018_plant::{me, plant, planted};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;

// ---------------------------------------------------------------------
// Empty listings (D9)
// ---------------------------------------------------------------------

#[test]
fn a_missing_or_empty_root_lists_nothing_and_succeeds() {
    let fx = Fx::new();
    for create in [false, true] {
        if create {
            fs::create_dir_all(fx.root().join("run-empty")).unwrap();
        }
        let table = fx.steps(&[]);
        assert!(table.status.success(), "stderr:\n{}", text(&table.stderr));
        assert_eq!(
            text(&table.stdout),
            "",
            "no entries prints nothing, not even a header"
        );
        assert_eq!(fx.steps_json(&[]), Vec::<Value>::new());
        let raw = fx.steps(&["-f", "json"]);
        assert_eq!(text(&raw.stdout).trim(), "[]");
    }
}

#[test]
fn listing_a_missing_root_creates_nothing() {
    let fx = Fx::new();
    assert!(fx.steps(&[]).status.success());
    assert!(
        !fx.home.join(".amplihack").exists(),
        "recipe steps only reads"
    );
}

// ---------------------------------------------------------------------
// Exit codes (D9)
// ---------------------------------------------------------------------

fn assert_exit_1(cmd: &mut Command, want: &str) {
    let out = cmd.output().expect("run amplihack recipe steps");
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr:\n{stderr}");
    assert_eq!(text(&out.stdout), "", "nothing on stdout");
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("step records"))
        .collect();
    assert_eq!(lines, [want], "stderr:\n{stderr}");
}

#[test]
fn a_symlinked_or_non_directory_root_exits_1() {
    for as_file in [false, true] {
        let fx = Fx::new();
        fs::create_dir_all(fx.root().parent().unwrap()).unwrap();
        if as_file {
            fs::write(fx.root(), "x").unwrap();
        } else {
            let target = fx.work.join("elsewhere");
            plant(
                &fx,
                "run-a",
                me(),
                &planted("run-a", "claude", me(), me(), me()),
            );
            fs::rename(fx.root(), &target).unwrap();
            symlink(&target, fx.root()).unwrap();
        }
        let want = format!(
            "amplihack: cannot list step records: unsafe directory {}",
            fx.root().display()
        );
        for args in [
            &["recipe", "steps"][..],
            &["recipe", "steps", "-f", "json"][..],
        ] {
            assert_exit_1(&mut fx.amplihack(args), &want);
        }
    }
}

#[test]
fn an_unset_empty_or_relative_home_exits_1() {
    let fx = Fx::new();
    let want = "amplihack: cannot list step records: HOME is not set";
    for home in [None, Some(""), Some("relative/home")] {
        let mut c = fx.amplihack(&["recipe", "steps"]);
        match home {
            Some(h) => c.env("HOME", h),
            None => c.env_remove("HOME"),
        };
        assert_exit_1(&mut c, want);
    }
}

/// A group- or world-writable root is listed, not refused.
#[test]
fn a_world_writable_root_is_listed_normally() {
    let fx = Fx::new();
    plant(
        &fx,
        "run-a",
        me(),
        &planted("run-a", "claude", me(), me(), me()),
    );
    fs::set_permissions(fx.root(), fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(states(&fx.steps_json(&[])), ["live"]);
}

#[test]
fn yaml_and_positional_arguments_are_usage_errors() {
    let fx = Fx::new();
    for args in [&["--format", "yaml"][..], &["run-id"][..]] {
        let out = fx.steps(args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}; stderr:\n{}",
            text(&out.stderr)
        );
    }
    let stderr = text(&fx.steps(&["--format", "yaml"]).stderr);
    assert!(
        stderr.contains("invalid value 'yaml'"),
        "a value error, not an unknown subcommand:\n{stderr}"
    );
}
