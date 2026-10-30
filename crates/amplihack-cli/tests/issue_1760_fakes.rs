//! Fakes for the issue #1760 tests: the `tmux`, `git` and `claude` scripts.
//!
//! - The fake `tmux` records each call's argv, environment, working
//!   directory, time and open file descriptors; keeps `session<TAB>window`
//!   lines for `has-session`; answers `display-message` from `panes`; fails
//!   `new-session` while `fail_new` exists, `has-session` without `-S` while
//!   `no_default` does, and everything while `broken` does. It never runs the
//!   pane command.
//! - The fake `git` records argv and environment, and for `status` prints as
//!   many lines as `<dir>/.fake-dirty` says.
//! - The fake `claude` records argv, environment and pid, prints
//!   `FAKE-CLAUDE-STARTED` on stderr, then acts on `records/claude.mode`:
//!   `exit:N`, or `wait:N` (exit N once `records/claude.release` exists).
//!
//! The synthetic `/proc` is in `issue_1760_fake_proc.rs`.

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use crate::issue_1760_fixture::Fx;
use crate::support::exec_stub::write_executable;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const FAKE_TMUX: &str = r#"#!/bin/bash
d='@STATE@'
n=$(( $(cat "$d/count" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$d/count"
{ for a in "$@"; do printf '%s\037' "$a"; done; printf '\n'; } >> "$d/calls.log"
date +%s%N > "$d/time.$n"
env -0 > "$d/env.$n"
pwd -P > "$d/cwd.$n"
for f in /proc/$$/fd/*; do readlink "$f"; done > "$d/fds.$n" 2>/dev/null
[ -e "$d/broken" ] && { echo "fake tmux: broken" >&2; exit 2; }
sock=""; [ "$1" = "-S" ] && { sock="$2"; shift 2; }
cmd="$1"; shift
target=""; sname=""; wname=""; fmt=""
while [ $# -gt 0 ]; do
  case "$1" in
    -t) target="$2"; shift 2 ;;
    -s) sname="$2"; shift 2 ;;
    -n) wname="$2"; shift 2 ;;
    -c) shift 2 ;;
    -d|-p) shift ;;
    --) shift; break ;;
    *) fmt="$1"; shift ;;
  esac
done
touch "$d/sessions" "$d/panes"
case "$cmd" in
  has-session)
    s="${target#=}"
    [ -z "$sock" ] && [ -e "$d/no_default" ] && { echo "error connecting to $TMUX_TMPDIR/tmux-$(id -u)/default (No such file or directory)" >&2; exit 1; }
    cut -f1 "$d/sessions" | grep -qxF -- "$s" && exit 0
    if [ -s "$d/sessions" ]; then echo "can't find session: $s" >&2
    else echo "no server running on /fake/socket" >&2; fi
    exit 1 ;;
  new-session)
    [ -e "$d/fail_new" ] && { cat "$d/fail_new" >&2; exit 1; }
    printf '%s\t%s\n' "$sname" "$wname" >> "$d/sessions"
    exit 0 ;;
  display-message)
    line=$(awk -F'\t' -v t="$target" '$1==t' "$d/panes" | head -n 1)
    [ -n "$line" ] || { echo "can't find pane: $target" >&2; exit 1; }
    IFS=$'\t' read -r _ ps pw pp <<< "$line"
    out="${fmt//'#{session_name}'/$ps}"; out="${out//'#{window_name}'/$pw}"
    out="${out//'#{pane_pid}'/$pp}"
    printf '%s\n' "$out"; exit 0 ;;
esac
echo "fake tmux: unexpected command $cmd" >&2; exit 98
"#;

const FAKE_GIT: &str = r#"#!/bin/sh
d='@RECORDS@'
n=$(( $(cat "$d/git.count" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$d/git.count"
: > "$d/git.$n.argv"; for a in "$@"; do printf '%s\0' "$a" >> "$d/git.$n.argv"; done
env -0 > "$d/git.$n.env"
dir=.
while [ $# -gt 0 ]; do
  case "$1" in
    -C) dir="$2"; shift 2 ;;
    -c) shift 2 ;;
    status) break ;;
    *) shift ;;
  esac
done
[ "$1" = status ] || exit 0
k=$(cat "$dir/.fake-dirty" 2>/dev/null || echo 0)
i=0; while [ "$i" -lt "$k" ]; do echo " M file$i"; i=$((i+1)); done
exit 0
"#;

const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in --version) echo '2.1.314 (Claude Code)'; exit 0 ;; esac
d='@RECORDS@'
echo "$$" > "$d/claude.pid"
: > "$d/claude.argv"; for a in "$@"; do printf '%s\0' "$a" >> "$d/claude.argv"; done
env -0 > "$d/claude.env"
echo "FAKE-CLAUDE-STARTED" >&2
: > "$d/claude.started"
mode=$(cat "$d/claude.mode" 2>/dev/null || echo exit:0)
case "$mode" in
  exit:*) exit "${mode#exit:}" ;;
  wait:*) while [ ! -e "$d/claude.release" ]; do sleep 0.05; done; exit "${mode#wait:}" ;;
esac
exit 0
"#;

/// One recorded call of the fake tmux.
#[derive(Debug, Clone)]
pub struct TmuxCall {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: String,
    pub time_ns: u128,
    /// Targets of the call's open file descriptors.
    pub fds: Vec<String>,
}

impl TmuxCall {
    /// `-S SOCKET` when given.
    pub fn socket(&self) -> Option<&str> {
        (self.argv.first().map(String::as_str) == Some("-S")).then(|| self.argv[1].as_str())
    }
    /// The tmux command, after any `-S SOCKET`.
    pub fn command(&self) -> &str {
        let i = if self.socket().is_some() { 2 } else { 0 };
        self.argv.get(i).map(String::as_str).unwrap_or("")
    }
    /// Everything after the first `--` (the pane argv of `new-session`).
    pub fn pane_argv(&self) -> Vec<String> {
        match self.argv.iter().position(|a| a == "--") {
            Some(i) => self.argv[i + 1..].to_vec(),
            None => Vec::new(),
        }
    }
    /// The value after `flag` before any `--`.
    pub fn opt(&self, flag: &str) -> Option<&str> {
        let end = self
            .argv
            .iter()
            .position(|a| a == "--")
            .unwrap_or(self.argv.len());
        let i = self.argv[..end].iter().position(|a| a == flag)?;
        self.argv.get(i + 1).map(String::as_str)
    }
}

/// `KEY=value` pairs of an `env -0` dump.
pub fn env0(path: &Path) -> BTreeMap<String, String> {
    let raw = fs::read(path).unwrap_or_default();
    String::from_utf8_lossy(&raw)
        .split('\0')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The elements of a NUL-terminated argv dump.
pub fn argv0(path: &Path) -> Vec<String> {
    let raw = fs::read(path).unwrap_or_default();
    let text = String::from_utf8_lossy(&raw);
    let mut v: Vec<String> = text.split('\0').map(String::from).collect();
    v.pop();
    v
}

impl Fx {
    pub(crate) fn install_fakes(&self) {
        let state = self.tmux_state.display().to_string();
        let records = self.records.display().to_string();
        write_executable(
            &self.tools.join("tmux"),
            FAKE_TMUX.replace("@STATE@", &state),
            0o755,
        );
        write_executable(
            &self.tools.join("git"),
            FAKE_GIT.replace("@RECORDS@", &records),
            0o755,
        );
        write_executable(
            &self.tools.join("claude"),
            FAKE_CLAUDE.replace("@RECORDS@", &records),
            0o755,
        );
    }

    // ----- fake tmux -----

    pub fn tmux_session(&self, session: &str, window: &str) {
        append(
            &self.tmux_state.join("sessions"),
            &format!("{session}\t{window}\n"),
        );
    }
    pub fn tmux_sessions(&self) -> Vec<String> {
        let raw = fs::read_to_string(self.tmux_state.join("sessions")).unwrap_or_default();
        raw.lines()
            .map(|l| l.split('\t').next().unwrap_or("").to_string())
            .collect()
    }
    pub fn tmux_pane(&self, target: &str, session: &str, window: &str, pane_pid: u32) {
        let line = format!("{target}\t{session}\t{window}\t{pane_pid}\n");
        append(&self.tmux_state.join("panes"), &line);
    }
    /// `new-session` fails with `line` on stderr.
    pub fn tmux_fail_new(&self, line: &str) {
        fs::write(self.tmux_state.join("fail_new"), format!("{line}\n")).unwrap();
    }
    /// `has-session` without `-S` fails as tmux 3.7b does when no server
    /// has started on the default socket.
    pub fn tmux_no_default_server(&self) {
        fs::write(self.tmux_state.join("no_default"), "").unwrap();
    }
    /// Every tmux call exits 2 with `fake tmux: broken`.
    pub fn tmux_broken(&self) {
        fs::write(self.tmux_state.join("broken"), "").unwrap();
    }
    pub fn tmux_records(&self) -> Vec<TmuxCall> {
        let s = &self.tmux_state;
        let raw = fs::read_to_string(s.join("calls.log")).unwrap_or_default();
        raw.lines()
            .enumerate()
            .map(|(i, l)| {
                let n = i + 1;
                let argv = l.strip_suffix('\x1f').unwrap_or(l);
                let read =
                    |f: &str| fs::read_to_string(s.join(format!("{f}.{n}"))).unwrap_or_default();
                TmuxCall {
                    argv: if argv.is_empty() {
                        Vec::new()
                    } else {
                        argv.split('\x1f').map(String::from).collect()
                    },
                    env: env0(&s.join(format!("env.{n}"))),
                    cwd: read("cwd").trim().to_string(),
                    time_ns: read("time").trim().parse().unwrap_or(0),
                    fds: read("fds").lines().map(String::from).collect(),
                }
            })
            .collect()
    }
    pub fn new_sessions(&self) -> Vec<TmuxCall> {
        self.tmux_records()
            .into_iter()
            .filter(|c| c.command() == "new-session")
            .collect()
    }

    // ----- fake git -----

    /// `git status --porcelain` in `dir` prints `n` lines.
    pub fn dirty(&self, dir: &Path, n: usize) {
        fs::write(dir.join(".fake-dirty"), n.to_string()).unwrap();
    }
    /// `(argv, environment)` of every git call.
    pub fn git_calls(&self) -> Vec<(Vec<String>, BTreeMap<String, String>)> {
        let n: usize = fs::read_to_string(self.records.join("git.count"))
            .map(|s| s.trim().parse().unwrap())
            .unwrap_or(0);
        (1..=n)
            .map(|i| {
                let r = &self.records;
                (
                    argv0(&r.join(format!("git.{i}.argv"))),
                    env0(&r.join(format!("git.{i}.env"))),
                )
            })
            .collect()
    }

    // ----- fake claude -----

    pub fn claude_mode(&self, mode: &str) {
        fs::write(self.records.join("claude.mode"), mode).unwrap();
    }
    pub fn claude_started(&self) -> bool {
        self.records.join("claude.started").exists()
    }
    pub fn claude_release(&self) {
        fs::write(self.records.join("claude.release"), "").unwrap();
    }
    pub fn claude_argv(&self) -> Vec<String> {
        argv0(&self.records.join("claude.argv"))
    }
    pub fn claude_env(&self) -> BTreeMap<String, String> {
        env0(&self.records.join("claude.env"))
    }
}

fn append(path: &Path, text: &str) {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    f.write_all(text.as_bytes()).unwrap();
}
