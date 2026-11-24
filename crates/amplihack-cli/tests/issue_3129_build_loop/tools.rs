//! Shared fixture: the bundle's tools run directly, with a stand-in `gh`.

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use crate::support::exec_stub::write_executable;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn tools_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/tools")
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(dir)
        .args(["-c", "user.name=T", "-c", "user.email=t@example.invalid"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Stand-in gh: `pr list`, `pr view`, `pr ready`, `issue view`, `repo view`,
/// `auth status` and `api`, answered from fixture files in `$GH_STATE`.
/// Every call is appended to `calls.log`.
pub const GH: &str = r##"#!/usr/bin/env bash
G="$GH_STATE"
printf '%s\n' "$*" >> "$G/calls.log"
jqf() { local f=""; while [ $# -gt 0 ]; do case "$1" in --jq|-q) f="$2"; shift 2 ;; *) shift ;; esac; done
  if [ -n "$f" ]; then jq -r "$f"; else cat; fi; }
case "${1:-} ${2:-}" in
  "pr list")
    state=open; for a in "$@"; do :; done
    prev=""; for a in "$@"; do [ "$prev" = --state ] && state="$a"; prev="$a"; done
    [ -e "$G/list-fails" ] && { echo "HTTP 502" >&2; exit 1; }
    if [ "$state" = merged ]; then echo '[]'; else cat "$G/prs.json" 2>/dev/null || echo '[]'; fi ;;
  "pr view")
    [ -e "$G/view-fails" ] && { echo "HTTP 502" >&2; exit 1; }
    cat "$G/pr-view.json" | jqf "$@" ;;
  "pr ready") : > "$G/ready-called"; exit 0 ;;
  "issue view") echo '{"state":"OPEN","url":"https://github.com/o/r/issues/463"}' | jqf "$@" ;;
  "repo view") echo '{"nameWithOwner":"o/r","defaultBranchRef":{"name":"main"}}' | jqf "$@" ;;
  "auth status") exit 0 ;;
  api*) echo '{}' ;;
  *) echo "stand-in gh: unexpected call: $*" >&2; exit 1 ;;
esac
"##;

/// A launch checkout on `main` with origin `git@github.com:o/r.git`.
pub struct World {
    pub _tmp: tempfile::TempDir,
    pub base: PathBuf,
    pub repo: PathBuf,
    pub bin: PathBuf,
    pub gh: PathBuf,
}

impl World {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let repo = base.join("repo");
        let bin = base.join("bin");
        let gh = base.join("gh");
        for d in [&repo, &bin, &gh] {
            fs::create_dir_all(d).unwrap();
        }
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &["remote", "add", "origin", "git@github.com:o/r.git"],
        );
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "base"]);
        write_executable(&bin.join("gh"), GH, 0o755);
        World {
            _tmp: tmp,
            base,
            repo,
            bin,
            gh,
        }
    }

    pub fn prs(&self, json: &serde_json::Value) {
        fs::write(self.gh.join("prs.json"), json.to_string()).unwrap();
    }

    pub fn pr_view(&self, json: &serde_json::Value) {
        fs::write(self.gh.join("pr-view.json"), json.to_string()).unwrap();
    }

    pub fn flag(&self, name: &str) {
        fs::write(self.gh.join(name), "").unwrap();
    }

    pub fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.gh.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// A tool of the bundle, run in `dir` with the stand-in gh first on PATH.
    pub fn tool(&self, name: &str, dir: &Path) -> Command {
        let mut c = Command::new("bash");
        c.arg(tools_dir().join(name))
            .current_dir(dir)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.base)
            .env("TMPDIR", &self.base)
            .env("GH_STATE", &self.gh)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("AMPLIHACK_SKIP_ISSUE_CLAIM_CHECK")
            .env_remove("ALLOW_EXISTING_PR")
            .env_remove("AMPLIHACK_ALLOW_EXISTING_PR")
            .env_remove("REPO_PATH");
        for (k, _) in std::env::vars() {
            if k.starts_with("RECIPE_VAR_") || k.starts_with("AUTODRIVE_") {
                c.env_remove(k);
            }
        }
        // The tools run git; under a hook an inherited GIT_DIR would point it
        // at this repository instead of the fixture's (#1278).
        amplihack_git::scrub(&mut c);
        c
    }
}

pub fn text(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}
