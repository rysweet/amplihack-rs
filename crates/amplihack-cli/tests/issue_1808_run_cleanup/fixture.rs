//! Shared fixture for the issue #1808 tests.
//!
//! Everything lives under one temp directory:
//!
//! - `tmp/home/` the isolated `HOME` of every child (never the real one);
//! - `work/`   `AMPLIHACK_WORK_ROOT` (mode 0700);
//! - `tmp/`    `TMPDIR` of every child, so `tmp-leftover` never scans `/tmp`;
//! - `bin/`    the only `PATH` entry: symlinks to the real tools the children
//!   need, plus a fake `gh` and a stub `recipe-runner-rs`. `/usr/bin` is never
//!   on `PATH`, so the real `gh` can never be reached;
//! - `gh/`     canned answers for the fake `gh`, one file per branch;
//! - `origin.git/` and `repo/`  a bare origin and its main checkout.
//!
//! Children are started with `isolated(..)`, so they never inherit
//! `AMPLIHACK_CLAUDE_PROFILE_ROOT` or the developer's `HOME`. No test starts
//! or talks to tmux.

#![allow(dead_code)]

use crate::isolated_amplihack::isolated;
use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Tools symlinked into `bin/`. Missing ones are skipped.
const TOOLS: &[&str] = &[
    "sh",
    "bash",
    "git",
    "cat",
    "mkdir",
    "rm",
    "env",
    "sleep",
    "flock",
    "dd",
    "touch",
    "date",
    "sha256sum",
    "shasum",
    "tar",
    "gzip",
    "head",
    "tr",
    "sed",
    "awk",
    "dirname",
    "basename",
    "mktemp",
    "chmod",
    "ls",
    "find",
    "id",
    "stat",
    "readlink",
    "realpath",
    "wc",
    "cut",
    "sort",
    "grep",
    "ln",
    "cp",
    "mv",
    "true",
    "false",
    "df",
    "rmdir",
    "uname",
    "xargs",
    "lsof",
    "printf",
];

/// Answers `gh pr list --head=<branch> ...` from `$FAKE_GH_DIR`, keyed by the
/// branch with `/` replaced by `_`: `<key>.json` is printed, `<key>.fail`
/// makes it fail, anything else prints `[]` (no PR). Every call is logged.
/// Only the single-argument `--head=<branch>` form is understood, as the
/// design requires.
const FAKE_GH: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_GH_DIR/calls.log"
head=
for a in "$@"; do
  case "$a" in --head=*) head=${a#--head=} ;; esac
done
key=$(printf '%s' "$head" | tr '/' '_')
if [ -z "$key" ]; then echo "fake gh: no --head=<branch> argument" >&2; exit 64; fi
if [ -f "$FAKE_GH_DIR/$key.fail" ]; then echo "fake gh: simulated failure" >&2; exit 1; fi
if [ -f "$FAKE_GH_DIR/$key.json" ]; then cat "$FAKE_GH_DIR/$key.json"; exit 0; fi
echo '[]'
"#;

pub const RECIPE: &str = "name: issue-1808-probe\ndescription: probe\nversion: 1.0.0\nsteps:\n  - id: probe\n    type: bash\n    command: \"true\"\n";

pub struct Fx {
    _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub work: PathBuf,
    pub tmp: PathBuf,
    pub bin: PathBuf,
    pub gh: PathBuf,
    pub origin: PathBuf,
    pub repo: PathBuf,
    pub records: PathBuf,
    pub recipe: PathBuf,
    children: Vec<Child>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

fn resolve_tool(tool: &str) -> Option<PathBuf> {
    let out = Command::new("/bin/sh")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .ok()?;
    let found = String::from_utf8(out.stdout).ok()?.trim().to_string();
    found.starts_with('/').then(|| PathBuf::from(found))
}

pub fn mkdir_0700(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

impl Fx {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let fx = Self {
            // Inside TMPDIR: the test home guard only accepts a HOME under
            // the child's temp dir.
            home: root.join("tmp/home"),
            work: root.join("work"),
            tmp: root.join("tmp"),
            bin: root.join("bin"),
            gh: root.join("gh"),
            origin: root.join("origin.git"),
            repo: root.join("repo"),
            records: root.join("records"),
            recipe: root.join("recipe.yaml"),
            root,
            _tmp: tmp,
            children: Vec::new(),
        };
        for d in [&fx.tmp, &fx.home, &fx.work, &fx.bin, &fx.gh, &fx.records] {
            mkdir_0700(d);
        }
        for tool in TOOLS {
            if let Some(real) = resolve_tool(tool) {
                let _ = std::os::unix::fs::symlink(real, fx.bin.join(tool));
            }
        }
        write_executable(&fx.bin.join("gh"), FAKE_GH, 0o755);
        fs::write(&fx.recipe, RECIPE).unwrap();
        fx.init_repo();
        fx
    }

    fn init_repo(&self) {
        self.git(&self.root, &["init", "-q", "--bare", "-b", "main"])
            .arg(&self.origin)
            .status()
            .unwrap();
        assert!(
            self.git(&self.root, &["init", "-q", "-b", "main"])
                .arg(&self.repo)
                .status()
                .unwrap()
                .success()
        );
        fs::write(self.repo.join("file.txt"), "base\n").unwrap();
        fs::write(self.repo.join(".gitignore"), "worktrees/\ntarget/\n").unwrap();
        self.git_ok(&self.repo, &["add", "file.txt", ".gitignore"]);
        self.git_ok(&self.repo, &["commit", "-q", "-m", "base"]);
        self.git_ok(
            &self.repo,
            &["remote", "add", "origin", self.origin.to_str().unwrap()],
        );
        self.git_ok(&self.repo, &["push", "-q", "-u", "origin", "main"]);
    }

    /// A `git` command with no global or system config and a fixed identity.
    pub fn git(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(self.bin.join("git"));
        c.current_dir(cwd)
            .args(args)
            .env_clear()
            .env("PATH", &self.bin)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid");
        c
    }

    pub fn git_ok(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self.git(cwd, args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            describe(&out)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Environment shared by `amplihack cleanup` and `amplihack recipe run`.
    pub fn amplihack(&self) -> Command {
        let mut c = isolated(amplihack_bin());
        c.current_dir(&self.root)
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .env("PATH", &self.bin)
            .env("FAKE_GH_DIR", &self.gh)
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_WORK_ROOT", &self.work)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS", "0")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env_remove("AMPLIHACK_CLAUDE_PROFILE_ROOT")
            // isolated() keeps CARGO*; the hook would honour an inherited
            // CARGO_TARGET_DIR (this test binary's own) as explicit.
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("TMUX")
            .stdin(Stdio::null());
        c
    }

    /// `amplihack cleanup <args>`.
    pub fn cleanup(&self, args: &[&str]) -> Output {
        self.cleanup_with(args, |_| {})
    }

    pub fn cleanup_with(&self, args: &[&str], tweak: impl FnOnce(&mut Command)) -> Output {
        let mut c = self.amplihack();
        c.arg("cleanup").args(args);
        tweak(&mut c);
        c.output().expect("run amplihack cleanup")
    }

    /// `amplihack cleanup --format json <args>`, parsed; asserts exit 0.
    pub fn cleanup_json(&self, args: &[&str]) -> serde_json::Value {
        let mut all = vec!["--format", "json"];
        all.extend_from_slice(args);
        let out = self.cleanup(&all);
        assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("cleanup JSON did not parse ({e}): {}", describe(&out)))
    }

    /// `amplihack recipe run` with `runner_body` as the recipe runner.
    pub fn recipe_run(&self, runner_body: &str, tweak: impl FnOnce(&mut Command)) -> Output {
        let runner = self.bin.join("recipe-runner-rs");
        write_executable(&runner, runner_body, 0o755);
        let mut c = self.amplihack();
        c.arg("recipe")
            .arg("run")
            .arg(&self.recipe)
            .arg("-w")
            .arg(&self.repo)
            .current_dir(&self.repo)
            .env("RECIPE_RUNNER_RS_PATH", &runner)
            .env("AMPLIHACK_SOURCE_ROOT", repo_root());
        tweak(&mut c);
        c.output().expect("run amplihack recipe run")
    }

    /// The `KEY=VALUE` lines a runner stub wrote with [`record_env_snippet`].
    pub fn recorded_env(&self) -> BTreeMap<String, String> {
        let text = fs::read_to_string(self.records.join("env"))
            .expect("the stub runner did not record its environment");
        text.lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.work.join("runs")
    }

    pub fn run_dirs(&self) -> Vec<PathBuf> {
        let Ok(rd) = fs::read_dir(self.runs_dir()) else {
            return Vec::new();
        };
        let mut v: Vec<_> = rd.map(|e| e.unwrap().path()).collect();
        v.sort();
        v
    }

    // ----- gh answers -------------------------------------------------------

    fn gh_key(branch: &str) -> String {
        branch.replace('/', "_")
    }

    pub fn gh_closed(&self, branch: &str, number: u64, closed_hours_ago: i64, merged: bool) {
        let at = rfc3339_hours_ago(closed_hours_ago);
        let state = if merged { "MERGED" } else { "CLOSED" };
        let merged_at = if merged {
            format!("\"{at}\"")
        } else {
            "null".into()
        };
        fs::write(
            self.gh.join(format!("{}.json", Self::gh_key(branch))),
            format!(
                r#"[{{"number":{number},"state":"{state}","closedAt":"{at}","mergedAt":{merged_at}}}]"#
            ),
        )
        .unwrap();
    }

    pub fn gh_open(&self, branch: &str, number: u64) {
        fs::write(
            self.gh.join(format!("{}.json", Self::gh_key(branch))),
            format!(r#"[{{"number":{number},"state":"OPEN","closedAt":null,"mergedAt":null}}]"#),
        )
        .unwrap();
    }

    pub fn gh_fail(&self, branch: &str) {
        fs::write(self.gh.join(format!("{}.fail", Self::gh_key(branch))), "").unwrap();
    }

    pub fn gh_calls(&self) -> String {
        fs::read_to_string(self.gh.join("calls.log")).unwrap_or_default()
    }

    // ----- seeded state -----------------------------------------------------

    /// A pushed, clean linked worktree for `branch` at
    /// `<work>/worktrees/repo-1a2b3c4d/<branch>`.
    pub fn worktree(&self, branch: &str) -> PathBuf {
        let path = self.work.join("worktrees/repo-1a2b3c4d").join(branch);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        self.git_ok(
            &self.repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                path.to_str().unwrap(),
            ],
        );
        fs::write(path.join("work.txt"), format!("{branch}\n")).unwrap();
        self.git_ok(&path, &["add", "work.txt"]);
        self.git_ok(&path, &["commit", "-q", "-m", branch]);
        self.git_ok(&path, &["push", "-q", "-u", "origin", branch]);
        path
    }

    /// A tagged target under `<work>/targets/` holding `bytes` bytes.
    pub fn target(&self, name: &str, bytes: usize) -> PathBuf {
        let path = self.work.join("targets").join(name);
        fs::create_dir_all(path.join("debug")).unwrap();
        fs::write(
            path.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        fs::write(path.join("debug/blob"), vec![7u8; bytes]).unwrap();
        path
    }

    /// Writes a run folder with `run.json` and `owned-paths`.
    pub fn seed_run(&self, run: &SeedRun) -> PathBuf {
        let dir = self.runs_dir().join(&run.id);
        mkdir_0700(&dir);
        let ended = match run.ended_hours_ago {
            Some(h) => format!("\"{}\"", rfc3339_hours_ago(h)),
            None => "null".into(),
        };
        let json = format!(
            r#"{{"schema_version":{sv},"run_id":"{id}","recipe":"seeded","pid":{pid},"pid_start_ticks":{ticks},"cwd":"{cwd}","started_at":"{started}","ended_at":{ended},"outcome":"{outcome}","attempts":[{{"correlation_id":"{corr}"}}],"steps":[],"log_path":null}}"#,
            sv = run.schema_version,
            id = run.id,
            pid = run.pid,
            ticks = run.pid_start_ticks,
            cwd = self.repo.display(),
            started = rfc3339_hours_ago(run.ended_hours_ago.unwrap_or(0) + 1),
            outcome = if run.ended_hours_ago.is_some() {
                "success"
            } else {
                "interrupted"
            },
            corr = uuid::Uuid::new_v4(),
        );
        fs::write(dir.join("run.json"), json).unwrap();
        fs::set_permissions(dir.join("run.json"), fs::Permissions::from_mode(0o600)).unwrap();
        let mut owned = String::new();
        for (kind, path, branch) in &run.owned {
            owned.push_str(&format!("{kind}\t{}\t{branch}\n", path.display()));
        }
        fs::write(dir.join("owned-paths"), owned).unwrap();
        if run.with_runtime {
            fs::create_dir_all(dir.join("runtime/tmp")).unwrap();
            fs::create_dir_all(dir.join("runtime/artifacts")).unwrap();
            fs::write(dir.join("runtime/tmp/scratch"), "scratch").unwrap();
        }
        set_mtime_hours_ago(&dir.join("run.json"), run.run_json_mtime_hours_ago);
        dir
    }

    /// A long-lived child, killed when the fixture drops. Its pid is a live
    /// process whose start ticks the test can read.
    pub fn live_process(&mut self, cwd: &Path) -> u32 {
        let child = Command::new(self.bin.join("sleep"))
            .arg("600")
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        self.children.push(child);
        pid
    }

    /// Holds an exclusive `flock` on `file` until the fixture drops.
    pub fn hold_flock(&mut self, file: &Path) {
        assert!(
            self.bin.join("flock").exists(),
            "flock(1) is needed for this test"
        );
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .unwrap();
        let ready = self
            .root
            .join(format!("flock-ready-{}", self.children.len()));
        let child = Command::new(self.bin.join("flock"))
            .arg("--exclusive")
            .arg(file)
            .arg(self.bin.join("sh"))
            .arg("-c")
            .arg("touch \"$1\"; exec sleep 600")
            .arg("sh")
            .arg(&ready)
            .env("PATH", &self.bin)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        self.children.push(child);
        for _ in 0..200 {
            if ready.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("flock holder never started");
    }
}

pub struct SeedRun {
    pub id: String,
    pub pid: u32,
    pub pid_start_ticks: u64,
    pub ended_hours_ago: Option<i64>,
    pub run_json_mtime_hours_ago: i64,
    pub schema_version: u32,
    pub owned: Vec<(&'static str, PathBuf, String)>,
    pub with_runtime: bool,
}

impl SeedRun {
    /// A finished, dead run that ended `hours` ago.
    pub fn ended(hours: i64) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            pid: dead_pid(),
            pid_start_ticks: 1,
            ended_hours_ago: Some(hours),
            run_json_mtime_hours_ago: hours,
            schema_version: 1,
            owned: Vec::new(),
            with_runtime: false,
        }
    }

    pub fn owns_worktree(mut self, path: &Path, branch: &str) -> Self {
        self.owned
            .push(("worktree", path.to_path_buf(), branch.to_string()));
        self
    }

    pub fn owns_target(mut self, path: &Path) -> Self {
        self.owned
            .push(("target", path.to_path_buf(), String::new()));
        self
    }
}

/// The pid of a process that has exited.
pub fn dead_pid() -> u32 {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("exit 0")
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

/// Field 22 of `/proc/<pid>/stat` (start time in clock ticks).
pub fn start_ticks(pid: u32) -> u64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let after = &stat[stat.rfind(')').unwrap() + 2..];
    after.split_whitespace().nth(19).unwrap().parse().unwrap()
}

pub fn rfc3339_hours_ago(hours: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::hours(hours))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// Sets the mtime of `path` (not following symlinks) to `hours` ago.
pub fn set_mtime_hours_ago(path: &Path, hours: i64) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - hours * 3600;
    let status = Command::new("touch")
        .arg("-h")
        .arg("-d")
        .arg(format!("@{secs}"))
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

/// Sets the mtime of `path` and everything under it to `hours` ago.
pub fn age_tree(path: &Path, hours: i64) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - hours * 3600;
    let status = Command::new("find")
        .arg(path)
        .args(["-depth", "-exec", "touch", "-h", "-d"])
        .arg(format!("@{secs}"))
        .args(["{}", "+"])
        .status()
        .unwrap();
    assert!(status.success());
}

/// `days` days ago, for logs.
pub fn set_mtime_days_ago(path: &Path, days: i64) {
    set_mtime_hours_ago(path, days * 24);
}

pub fn describe(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A recursive listing of `root` with file type, size and mtime (ns), not
/// following symlinks. Missing roots give an empty listing.
pub fn snapshot(root: &Path) -> BTreeMap<PathBuf, (String, u64, i128)> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (String, u64, i128)>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for entry in rd {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let kind = if meta.file_type().is_symlink() {
                "link"
            } else if meta.is_dir() {
                "dir"
            } else {
                "file"
            };
            let mtime = meta
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as i128)
                .unwrap_or(-1);
            let len = if meta.is_dir() { 0 } else { meta.len() };
            out.insert(
                path.strip_prefix(root).unwrap().to_path_buf(),
                (kind.to_string(), len, mtime),
            );
            if meta.is_dir() {
                walk(root, &path, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Entries of a cleanup JSON report.
pub fn entries(report: &serde_json::Value) -> Vec<serde_json::Value> {
    report["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("report has no entries array: {report}"))
        .clone()
}

/// The entry whose `path` is `path`.
pub fn entry_for(report: &serde_json::Value, path: &Path) -> serde_json::Value {
    let want = path.to_string_lossy();
    entries(report)
        .into_iter()
        .find(|e| e["path"].as_str() == Some(want.as_ref()))
        .unwrap_or_else(|| panic!("no entry for {want} in {report:#}"))
}

/// A shell snippet for a runner stub that records the variables a step sees.
pub const RECORD_ENV_SNIPPET: &str = r#"
for v in AMPLIHACK_WORK_ROOT AMPLIHACK_WORK_ROOT_EXPLICIT AMPLIHACK_RUN_ID AMPLIHACK_RUN_DIR \
         AMPLIHACK_RECIPE_RUN_ID AMPLIHACK_WORKFLOW_RUNTIME_DIR AMPLIHACK_RUNTIME_ROOT \
         AMPLIHACK_WORKFLOW_ARTIFACT_DIR TMPDIR TMP TEMP HOME XDG_CACHE_HOME; do
  eval "val=\${$v-__unset__}"
  printf '%s=%s\n' "$v" "$val" >> "$RECORD_DIR/env"
done
[ -d "$TMPDIR" ] && printf 'TMPDIR_EXISTS=1\n' >> "$RECORD_DIR/env"
"#;

pub const RUNNER_SUCCESS_JSON: &str = r#"printf '%s\n' '{"recipe_name":"issue-1808-probe","success":true,"step_results":[],"context":{}}'"#;

pub const RUNNER_FAILURE_JSON: &str = r#"printf '%s\n' '{"recipe_name":"issue-1808-probe","success":false,"step_results":[],"context":{}}'
exit 7"#;

/// The runtime folder the 80-byte rule picks for `run_id` under `work`.
pub fn expected_runtime_dir(work: &Path, run_id: &str) -> PathBuf {
    let default = work.join("runs").join(run_id).join("runtime");
    if default.join("tmp").as_os_str().len() <= 80 {
        default
    } else {
        let short: String = run_id.chars().filter(|c| *c != '-').take(12).collect();
        work.join("r").join(short)
    }
}

pub fn is_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok() && s.len() == 36
}

pub fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}
