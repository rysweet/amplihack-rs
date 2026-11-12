//! The fake `systemd-run` for the issue #1760 tests (G1). Every fixture
//! installs it in `tools`, ahead of the host's own on `PATH`, so no test
//! creates a real scope. It records each call's argv, then acts on
//! `tmux-state/systemd_run.mode`:
//!
//! - unset: runs the command after `--` (the fake tmux) and exits with its
//!   status, as `systemd-run --scope` does;
//! - `refuse`: prints a bus error and exits 1 without running anything;
//! - `fail-after`: runs the command, then prints a unit error and exits 1;
//! - `hang`: runs the command, then sleeps past the supervisor's timeout.

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use crate::issue_1760_fixture::Fx;
use crate::support::exec_stub::write_executable;
use std::fs;
use std::path::PathBuf;

const FAKE_SYSTEMD_RUN: &str = r#"#!/bin/bash
d='@STATE@'
{ for a in "$@"; do printf '%s\037' "$a"; done; printf '\n'; } >> "$d/systemd_run.log"
mode=$(cat "$d/systemd_run.mode" 2>/dev/null)
[ "$mode" = refuse ] && { echo "Failed to connect to bus: No medium found" >&2; exit 1; }
while [ $# -gt 0 ] && [ "$1" != "--" ]; do shift; done
shift
"$@"; rc=$?
case "$mode" in
  fail-after) echo "Failed to start transient scope unit: Unit already exists." >&2; exit 1 ;;
  hang) exec sleep 30 ;;
esac
exit "$rc"
"#;

impl Fx {
    pub(crate) fn install_fake_systemd_run(&self) {
        let state = self.tmux_state.display().to_string();
        write_executable(
            &self.tools.join("systemd-run"),
            FAKE_SYSTEMD_RUN.replace("@STATE@", &state),
            0o755,
        );
    }

    /// One of `refuse`, `fail-after` or `hang`.
    pub fn systemd_run_mode(&self, mode: &str) {
        fs::write(self.tmux_state.join("systemd_run.mode"), mode).unwrap();
    }

    /// The argv of every `systemd-run` call.
    pub fn systemd_run_calls(&self) -> Vec<Vec<String>> {
        let raw = fs::read_to_string(self.tmux_state.join("systemd_run.log")).unwrap_or_default();
        raw.lines()
            .map(|l| {
                let l = l.strip_suffix('\x1f').unwrap_or(l);
                l.split('\x1f').map(String::from).collect()
            })
            .collect()
    }

    /// A `PATH` with no `systemd-run` at all: the fake is removed from
    /// `tools`, and a directory of links to every host program except
    /// `systemd-run` stands in for `/usr/bin` and `/bin`.
    pub fn path_without_systemd_run(&self) -> String {
        fs::remove_file(self.tools.join("systemd-run")).unwrap();
        let shim = self.root.join("usr-bin-shim");
        fs::create_dir_all(&shim).unwrap();
        let mut seen = std::collections::HashSet::new();
        for dir in ["/usr/bin", "/bin"] {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name == "systemd-run" || !seen.insert(name.clone()) {
                    continue;
                }
                let target: PathBuf = entry.path();
                let _ = std::os::unix::fs::symlink(target, shim.join(&name));
            }
        }
        format!("{}:{}", self.tools.display(), shim.display())
    }
}
