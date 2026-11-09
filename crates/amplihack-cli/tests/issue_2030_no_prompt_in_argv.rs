//! Issue #2030 (red phase): no prompt, brief or contract text in the argv of
//! any agent process amplihack launches.
//!
//! Each test runs the real `amplihack` binary, isolated, with stub agent CLIs
//! first on `PATH`. A stub records, per run `n`:
//! - `argv.<n>`: its argv, NUL-separated (what `/proc/<pid>/cmdline` shows);
//! - `cmdline.<n>`: a copy of the stub's own `/proc/$$/cmdline`;
//! - `stdin.<n>`: everything on its stdin;
//! - `files.<n>`: for each argument after a `--*-file` or `--add-dir` flag,
//!   one tab-separated line `flag  path  <type>:<mode>:<uid>  <parent mode>`;
//! - `content.<n>.<flag>`: a copy of that file (it is removed after the run);
//! - `calls`: one line per invocation, `version` or `run`.
//!
//! Spec: `docs/reference/agent-prompt-channels.md`.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use amplihack_utils::prompt_delivery::FILE_REFERENCE_PREFIX;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Task-prompt sentinel. Passed by the test; must reach stdin or a file only.
const SENTINEL: &str = "SENTINEL-2030-task-brief";
/// System-prompt sentinel (stands in for an agent .md in a recipe step).
const SYS_SENTINEL: &str = "SENTINEL-2030-agent-system-prompt";
/// Phrases from the compiled-in contract. The incident: `pkill -f` on the
/// first one matched 63 of 79 sessions.
const CONTRACT_PHRASES: &[&str] = &["tail -f /dev/null", "Amplihack operating contract"];

const STUB: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo version >> "$RECORD_DIR/calls"
  [ -n "$STUB_VERSION" ] && printf '%s\n' "$STUB_VERSION"
  exit 0
fi
echo run >> "$RECORD_DIR/calls"
n=$(grep -c '^run$' "$RECORD_DIR/calls")
cp /proc/$$/cmdline "$RECORD_DIR/cmdline.$n" 2>/dev/null
: > "$RECORD_DIR/files.$n.tmp"
: > "$RECORD_DIR/argv.$n.tmp"
prev=
for a in "$@"; do
  printf '%s\0' "$a" >> "$RECORD_DIR/argv.$n.tmp"
  case "$prev" in
    --*-file|--add-dir)
      t=$(stat -c '%F:%a:%u' -- "$a" 2>/dev/null || echo missing)
      d=$(stat -c '%a' -- "$(dirname -- "$a")" 2>/dev/null || echo missing)
      printf '%s\t%s\t%s\t%s\n' "$prev" "$a" "$t" "$d" >> "$RECORD_DIR/files.$n.tmp"
      [ -f "$a" ] && cp -- "$a" "$RECORD_DIR/content.$n.${prev#--}" ;;
  esac
  case "$a" in
    "$FILE_REFERENCE_PREFIX"*)
      rest=${a#"$FILE_REFERENCE_PREFIX"}
      path=${rest%%". Its contents are the task prompt."}
      t=$(stat -c '%F:%a:%u' -- "$path" 2>/dev/null || echo missing)
      d=$(stat -c '%a' -- "$(dirname -- "$path")" 2>/dev/null || echo missing)
      printf '%s\t%s\t%s\t%s\n' pointer "$path" "$t" "$d" >> "$RECORD_DIR/files.$n.tmp"
      cp -- "$path" "$RECORD_DIR/content.$n.pointer" 2>/dev/null ;;
  esac
  prev=$a
done
mv "$RECORD_DIR/argv.$n.tmp" "$RECORD_DIR/argv.$n"
mv "$RECORD_DIR/files.$n.tmp" "$RECORD_DIR/files.$n"
if [ -n "$STUB_SLEEP" ]; then
  : > "$RECORD_DIR/stdin.$n"
  : > "$RECORD_DIR/ready.$n"
  exec sleep "$STUB_SLEEP"
fi
cat > "$RECORD_DIR/stdin.$n"
printf 'stub-ok\nVERDICT: ready\n'
exit "${STUB_EXIT:-0}"
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    work: PathBuf,
    tools: PathBuf,
    records: PathBuf,
}

/// One `files.<n>` line.
#[derive(Debug)]
struct FileRec {
    flag: String,
    path: PathBuf,
    kind: String,
    mode: String,
    uid: u32,
    parent_mode: String,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let work = root.join("work");
        let tools = root.join("tools");
        let records = root.join("records");
        for d in [&home, &work, &tools, &records] {
            fs::create_dir_all(d).unwrap();
        }
        for name in ["claude", "copilot", "codex"] {
            crate::support::exec_stub::write_executable(&tools.join(name), STUB, 0o755);
        }
        Self {
            _tmp: tmp,
            root,
            home,
            work,
            tools,
            records,
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("RECORD_DIR", &self.records)
            .env("FILE_REFERENCE_PREFIX", FILE_REFERENCE_PREFIX)
            .env("STUB_VERSION", "2.1.319 (Claude Code)")
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_COPILOT_BINARY_PATH", self.tools.join("copilot"))
            .env("AMPLIHACK_CODEX_BINARY_PATH", self.tools.join("codex"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.root.join("profiles"))
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_GOVERNOR", "off")
            .stdin(Stdio::null());
        c
    }

    fn run(&self, c: &mut Command) -> Output {
        let mut tries = 0;
        loop {
            match c.output() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 50 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
                r => return r.expect("run amplihack"),
            }
        }
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.records.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn runs(&self) -> usize {
        self.calls().iter().filter(|l| *l == "run").count()
    }

    fn argv(&self, n: usize) -> Vec<String> {
        fs::read(self.records.join(format!("argv.{n}")))
            .unwrap_or_default()
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    }

    fn stdin(&self, n: usize) -> String {
        fs::read_to_string(self.records.join(format!("stdin.{n}"))).unwrap_or_default()
    }

    fn files(&self, n: usize) -> Vec<FileRec> {
        fs::read_to_string(self.records.join(format!("files.{n}")))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                let t: Vec<&str> = f[2].split(':').collect();
                FileRec {
                    flag: f[0].to_string(),
                    path: PathBuf::from(f[1]),
                    kind: t.first().copied().unwrap_or("").to_string(),
                    mode: t.get(1).copied().unwrap_or("").to_string(),
                    uid: t.get(2).and_then(|u| u.parse().ok()).unwrap_or(u32::MAX),
                    parent_mode: f[3].to_string(),
                }
            })
            .collect()
    }

    fn file(&self, n: usize, flag: &str) -> FileRec {
        self.files(n)
            .into_iter()
            .find(|r| r.flag == flag)
            .unwrap_or_else(|| panic!("run {n} has no {flag} file: {:?}", self.argv(n)))
    }

    fn content(&self, n: usize, flag: &str) -> String {
        let name = format!("content.{n}.{}", flag.trim_start_matches("--"));
        fs::read_to_string(self.records.join(&name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn contract_path(&self) -> PathBuf {
        self.home.join(".amplihack/private/system-prompt-append.md")
    }
}

fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

fn describe(out: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn contract_text() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/context/SYSTEM_PROMPT_APPEND.md");
    fs::read_to_string(&p).unwrap()
}

/// The core #2030 assertion for one recorded child argv.
fn assert_argv_clean(argv: &[String], extra: &[&str]) {
    for a in argv {
        for needle in CONTRACT_PHRASES
            .iter()
            .chain([SENTINEL, SYS_SENTINEL].iter())
            .chain(extra)
        {
            assert!(
                !a.contains(needle),
                "argv element contains {needle:?}: {a:?}\nfull argv: {argv:?}"
            );
        }
    }
}

/// A file named after a `*-file` flag: regular, 0600, ours, in a 0700 parent
/// when the parent is per-session.
fn assert_private(rec: &FileRec, per_session_parent: bool) {
    assert_eq!(rec.kind, "regular file", "{rec:?}");
    assert_eq!(rec.mode, "600", "{rec:?}");
    assert_eq!(rec.uid, euid(), "{rec:?}");
    if per_session_parent {
        assert_eq!(rec.parent_mode, "700", "{rec:?}");
    }
}

// ---------------------------------------------------------------------------
// amplihack claude / amplihack launch
// ---------------------------------------------------------------------------

#[test]
fn claude_launch_uses_contract_file() {
    let f = Fixture::new();
    let out = f.run(&mut f.cmd(&["claude", "--", "--verbose"]));
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.runs(), 1, "{}", describe(&out));

    let argv = f.argv(1);
    assert_argv_clean(&argv, &[]);
    assert!(
        !argv.iter().any(|a| a == "--append-system-prompt"),
        "the text flag is gone: {argv:?}"
    );
    let rec = f.file(1, "--append-system-prompt-file");
    assert_eq!(rec.path, f.contract_path());
    assert_private(&rec, false);
    assert_eq!(rec.parent_mode, "700", "private/ is 0700: {rec:?}");
    assert_eq!(f.content(1, "--append-system-prompt-file"), contract_text());
}

#[test]
fn launch_subcommand_uses_contract_file() {
    let f = Fixture::new();
    let out = f.run(&mut f.cmd(&["launch", "--", "--verbose"]));
    assert!(out.status.success(), "{}", describe(&out));
    let argv = f.argv(1);
    assert_argv_clean(&argv, &[]);
    assert_private(&f.file(1, "--append-system-prompt-file"), false);
}

#[test]
fn amplihack_home_env_moves_the_contract_file() {
    let f = Fixture::new();
    let custom = f.root.join("custom-home");
    fs::create_dir_all(&custom).unwrap();
    let out = f.run(
        f.cmd(&["claude", "--", "--verbose"])
            .env("AMPLIHACK_HOME", &custom),
    );
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.file(1, "--append-system-prompt-file").path,
        custom.join("private/system-prompt-append.md")
    );
}

#[test]
fn nested_recipe_step_rewrite() {
    // What a recipe agent step runs: amplihack claude -- -p <brief>
    // --system-prompt <agent md>.
    let f = Fixture::new();
    let out = f.run(&mut f.cmd(&[
        "claude",
        "--subprocess-safe",
        "--",
        "-p",
        SENTINEL,
        "--system-prompt",
        SYS_SENTINEL,
    ]));
    assert!(out.status.success(), "{}", describe(&out));
    let argv = f.argv(1);
    assert_argv_clean(&argv, &[]);
    assert!(argv.iter().any(|a| a == "-p" || a == "--print"), "{argv:?}");
    assert_eq!(f.stdin(1), SENTINEL, "the brief arrives on stdin");

    let sys = f.file(1, "--system-prompt-file");
    assert_private(&sys, true);
    assert_eq!(f.content(1, "--system-prompt-file"), SYS_SENTINEL);

    let contract = f.file(1, "--append-system-prompt-file");
    assert_eq!(contract.path, f.contract_path());
    assert_private(&contract, false);
}

#[test]
fn user_append_text_is_rewritten_and_the_contract_not_added() {
    let f = Fixture::new();
    let out = f.run(&mut f.cmd(&[
        "claude",
        "--",
        "-p",
        SENTINEL,
        "--append-system-prompt",
        SYS_SENTINEL,
    ]));
    assert!(out.status.success(), "{}", describe(&out));
    let argv = f.argv(1);
    assert_argv_clean(&argv, &[]);
    let appends: Vec<_> = f
        .files(1)
        .into_iter()
        .filter(|r| r.flag == "--append-system-prompt-file")
        .collect();
    assert_eq!(appends.len(), 1, "only the user's own: {argv:?}");
    assert_ne!(appends[0].path, f.contract_path());
    assert_private(&appends[0], true);
    assert_eq!(f.content(1, "--append-system-prompt-file"), SYS_SENTINEL);
}

// ---------------------------------------------------------------------------
// Per-session file lifetime
// ---------------------------------------------------------------------------

fn per_session_paths(f: &Fixture) -> Vec<PathBuf> {
    f.files(1)
        .into_iter()
        .filter(|r| r.path != f.contract_path())
        .map(|r| r.path)
        .collect()
}

#[test]
fn per_session_files_removed_after_exit0() {
    let f = Fixture::new();
    let out = f.run(&mut f.cmd(&[
        "claude",
        "--",
        "-p",
        SENTINEL,
        "--system-prompt",
        SYS_SENTINEL,
    ]));
    assert!(out.status.success(), "{}", describe(&out));
    let paths = per_session_paths(&f);
    assert!(!paths.is_empty());
    for p in paths {
        assert!(!p.exists(), "{} survived exit 0", p.display());
        assert!(!p.parent().unwrap().exists(), "its directory survived too");
    }
    assert!(
        f.contract_path().is_file(),
        "the per-install contract stays"
    );
}

#[test]
fn per_session_files_removed_after_exit1() {
    let f = Fixture::new();
    let out = f.run(
        f.cmd(&[
            "claude",
            "--",
            "-p",
            SENTINEL,
            "--system-prompt",
            SYS_SENTINEL,
        ])
        .env("STUB_EXIT", "1"),
    );
    assert!(!out.status.success(), "the child's failure is reported");
    let paths = per_session_paths(&f);
    assert!(!paths.is_empty());
    for p in paths {
        assert!(!p.exists(), "{} survived a non-zero exit", p.display());
    }
}

#[test]
fn per_session_files_removed_after_sigterm() {
    let f = Fixture::new();
    let mut child = f
        .cmd(&[
            "claude",
            "--",
            "-p",
            SENTINEL,
            "--system-prompt",
            SYS_SENTINEL,
        ])
        .env("STUB_SLEEP", "60")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !f.records.join("ready.1").exists() {
        if Instant::now() > deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            panic!("the stub never started");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let paths = per_session_paths(&f);
    assert!(!paths.is_empty());
    for p in &paths {
        assert!(
            p.exists(),
            "{} must exist while the agent runs",
            p.display()
        );
    }
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("amplihack did not exit after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    for p in paths {
        assert!(!p.exists(), "{} survived a forwarded SIGTERM", p.display());
    }
}

// ---------------------------------------------------------------------------
// Version gate
// ---------------------------------------------------------------------------

fn assert_gate_refused(f: &Fixture, out: &Output) {
    assert!(!out.status.success(), "{}", describe(out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("2.1.319"), "{}", describe(out));
    assert!(
        err.contains("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND=1"),
        "{}",
        describe(out)
    );
    assert_eq!(f.runs(), 0, "the agent is never started: {:?}", f.calls());
    for stream in [&out.stdout, &out.stderr] {
        let s = String::from_utf8_lossy(stream);
        assert!(!s.contains(SENTINEL), "error paths never echo the prompt");
        for phrase in CONTRACT_PHRASES {
            assert!(!s.contains(phrase), "error paths never echo the contract");
        }
    }
}

#[test]
fn version_gate_2_1_318() {
    let f = Fixture::new();
    let out = f.run(
        f.cmd(&["claude", "--", "-p", SENTINEL])
            .env("STUB_VERSION", "2.1.318 (Claude Code)"),
    );
    assert_gate_refused(&f, &out);
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("needs Claude Code 2.1.319 or later; found 2.1.318."),
        "{}",
        describe(&out)
    );
}

#[test]
fn version_gate_unparseable() {
    let f = Fixture::new();
    let out = f.run(
        f.cmd(&["claude", "--", "-p", SENTINEL])
            .env("STUB_VERSION", "Claude Code (dev build)"),
    );
    assert_gate_refused(&f, &out);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("could not read a Claude Code version from"),
        "{}",
        describe(&out)
    );
}

#[test]
fn version_gate_missing() {
    let f = Fixture::new();
    let out = f.run(
        f.cmd(&["claude", "--", "-p", SENTINEL])
            .env("STUB_VERSION", ""),
    );
    assert_gate_refused(&f, &out);
}

#[test]
fn opt_out_skips_the_gate_and_the_contract() {
    let f = Fixture::new();
    let out = f.run(
        f.cmd(&["claude", "--", "--verbose"])
            .env("STUB_VERSION", "2.1.318")
            .env("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", "1"),
    );
    assert!(out.status.success(), "{}", describe(&out));
    let argv = f.argv(1);
    assert!(
        !argv.iter().any(|a| a.starts_with("--append-system-prompt")),
        "{argv:?}"
    );
    assert_argv_clean(&argv, &[]);
}

#[test]
fn opt_out_with_system_prompt_text_still_gates_the_file_flag() {
    // The rewrite emits --system-prompt-file, which needs the version too.
    let f = Fixture::new();
    let out = f.run(
        f.cmd(&[
            "claude",
            "--",
            "-p",
            SENTINEL,
            "--system-prompt",
            SYS_SENTINEL,
        ])
        .env("STUB_VERSION", "2.1.318")
        .env("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", "1"),
    );
    assert!(!out.status.success(), "{}", describe(&out));
    assert_eq!(f.runs(), 0);
    assert!(!String::from_utf8_lossy(&out.stderr).contains(SYS_SENTINEL));
}

// ---------------------------------------------------------------------------
// Contract file repair and refusal, end to end
// ---------------------------------------------------------------------------

#[test]
fn contract_file_repairs_0644_and_symlink() {
    let f = Fixture::new();
    let private = f.home.join(".amplihack/private");
    fs::create_dir_all(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    let victim = f.root.join("victim.txt");
    fs::write(&victim, "victim").unwrap();
    symlink(&victim, f.contract_path()).unwrap();

    let out = f.run(&mut f.cmd(&["claude", "--", "--verbose"]));
    assert!(out.status.success(), "{}", describe(&out));
    assert_private(&f.file(1, "--append-system-prompt-file"), false);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "victim");

    fs::set_permissions(f.contract_path(), fs::Permissions::from_mode(0o644)).unwrap();
    let out = f.run(&mut f.cmd(&["claude", "--", "--verbose"]));
    assert!(out.status.success(), "{}", describe(&out));
    assert_private(&f.file(2, "--append-system-prompt-file"), false);
}

#[test]
fn private_dir_symlink_fails_before_spawn() {
    let f = Fixture::new();
    fs::create_dir_all(f.home.join(".amplihack")).unwrap();
    let elsewhere = f.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    symlink(&elsewhere, f.home.join(".amplihack/private")).unwrap();
    let out = f.run(&mut f.cmd(&["claude", "--", "--verbose"]));
    assert!(!out.status.success(), "{}", describe(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("private is not a directory owned by you"),
        "{}",
        describe(&out)
    );
    assert_eq!(f.runs(), 0);
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn two_prompts_in_passthrough_fail_without_echo() {
    let f = Fixture::new();
    let out = f.run(&mut f.cmd(&["claude", "--", "-p", SENTINEL, "--print", "again"]));
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("the prompt was given twice (-p/--print); pass it once"),
        "{err}"
    );
    assert!(!err.contains(SENTINEL));
    assert_eq!(f.runs(), 0);
}

// ---------------------------------------------------------------------------
// Auto mode: Claude, Copilot, Codex
// ---------------------------------------------------------------------------

fn task_file(f: &Fixture) -> PathBuf {
    let p = f.root.join("task.md");
    fs::write(&p, SENTINEL).unwrap();
    p
}

#[test]
fn auto_mode_turn_uses_stdin() {
    // A short prompt went on argv before #2030 (the 4 KiB threshold).
    let f = Fixture::new();
    let task = task_file(&f);
    let out = f.run(&mut f.cmd(&[
        "claude",
        "--auto",
        "--max-turns",
        "1",
        "--prompt-file",
        task.to_str().unwrap(),
    ]));
    assert!(out.status.success(), "{}", describe(&out));
    assert!(f.runs() >= 1, "{}", describe(&out));
    let mut saw_sentinel_on_stdin = false;
    for n in 1..=f.runs() {
        assert_argv_clean(&f.argv(n), &[]);
        saw_sentinel_on_stdin |= f.stdin(n).contains(SENTINEL);
    }
    assert!(saw_sentinel_on_stdin, "the task reaches a turn on stdin");
}

#[test]
fn auto_mode_copilot_turn_uses_pointer() {
    let f = Fixture::new();
    let task = task_file(&f);
    let out = f.run(&mut f.cmd(&[
        "copilot",
        "--auto",
        "--max-turns",
        "1",
        "--prompt-file",
        task.to_str().unwrap(),
    ]));
    assert!(out.status.success(), "{}", describe(&out));
    assert!(f.runs() >= 1, "{}", describe(&out));
    let argv = f.argv(1);
    assert_argv_clean(&argv, &[]);
    let pointer = f.file(1, "pointer");
    assert_private(&pointer, true);
    let add_dir = f.file(1, "--add-dir");
    assert_eq!(
        Some(add_dir.path.as_path()),
        pointer.path.parent(),
        "--add-dir names only the prompt's private directory"
    );
    assert!(f.content(1, "pointer").contains(SENTINEL));
    assert!(
        !pointer.path.exists(),
        "the pointer file is removed after the turn"
    );
}

/// Auto mode only (`--auto --prompt-file`): amplihack builds `codex exec -`
/// itself. The passthrough form, `amplihack codex -- exec <text>`, is
/// covered by `codex_passthrough_exec_moves_the_task_to_stdin`.
#[test]
fn auto_mode_codex_turn_reads_stdin() {
    let f = Fixture::new();
    let task = task_file(&f);
    let out = f.run(&mut f.cmd(&[
        "codex",
        "--auto",
        "--max-turns",
        "1",
        "--prompt-file",
        task.to_str().unwrap(),
    ]));
    assert!(out.status.success(), "{}", describe(&out));
    assert!(f.runs() >= 1, "{}", describe(&out));
    let argv = f.argv(1);
    assert_argv_clean(&argv, &[]);
    assert!(argv.iter().any(|a| a == "exec"), "{argv:?}");
    assert_eq!(argv.last().map(String::as_str), Some("-"), "{argv:?}");
    assert!(f.stdin(1).contains(SENTINEL));
}

// ---------------------------------------------------------------------------
// Passthrough: amplihack codex -- exec <text>, amplihack copilot -- -p <text>
// ---------------------------------------------------------------------------

impl Fixture {
    /// The stub's own `/proc/$$/cmdline`, the bytes `pkill -f` matches.
    fn proc_cmdline(&self, n: usize) -> String {
        let raw = fs::read(self.records.join(format!("cmdline.{n}")))
            .unwrap_or_else(|e| panic!("cmdline.{n}: {e}"));
        String::from_utf8_lossy(&raw).replace('\0', " ")
    }
}

fn assert_proc_cmdline_clean(f: &Fixture, n: usize) {
    let cmdline = f.proc_cmdline(n);
    for needle in CONTRACT_PHRASES
        .iter()
        .chain([SENTINEL, SYS_SENTINEL].iter())
    {
        assert!(
            !cmdline.contains(needle),
            "/proc cmdline has {needle:?}: {cmdline:?}"
        );
    }
}

#[test]
fn codex_passthrough_exec_moves_the_task_to_stdin() {
    // What a recipe agent step on Codex, or a user, runs:
    // amplihack codex -- exec <task>.
    for form in [
        vec!["codex", "--subprocess-safe", "--", "exec", SENTINEL],
        vec![
            "codex",
            "--subprocess-safe",
            "--",
            "exec",
            "--json",
            SENTINEL,
        ],
        vec!["codex", "--subprocess-safe", "--", "exec", "--", SENTINEL],
    ] {
        let f = Fixture::new();
        let out = f.run(&mut f.cmd(&form));
        assert!(out.status.success(), "{form:?}\n{}", describe(&out));
        let argv = f.argv(1);
        assert_argv_clean(&argv, &[]);
        assert_proc_cmdline_clean(&f, 1);
        assert_eq!(argv.first().map(String::as_str), Some("exec"), "{argv:?}");
        assert_eq!(
            argv.last().map(String::as_str),
            Some("-"),
            "{form:?}: {argv:?}"
        );
        assert_eq!(f.stdin(1), SENTINEL, "{form:?}: the task arrives on stdin");
    }
}

#[test]
fn codex_passthrough_exec_dash_is_unchanged() {
    let f = Fixture::new();
    let out = f.run(&mut f.cmd(&["codex", "--subprocess-safe", "--", "exec", "-"]));
    assert!(out.status.success(), "{}", describe(&out));
    let argv = f.argv(1);
    assert_eq!(argv.iter().filter(|a| *a == "-").count(), 1, "{argv:?}");
    assert_eq!(argv.last().map(String::as_str), Some("-"), "{argv:?}");
}

#[test]
fn codex_passthrough_two_prompts_fail_without_echo() {
    let f = Fixture::new();
    let out = f.run(&mut f.cmd(&[
        "codex",
        "--subprocess-safe",
        "--",
        "exec",
        SENTINEL,
        "again",
    ]));
    assert!(!out.status.success(), "{}", describe(&out));
    assert_eq!(f.runs(), 0, "nothing is spawned");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("more than one positional prompt"), "{all}");
    assert!(!all.contains(SENTINEL), "{all}");
}

#[test]
fn copilot_passthrough_prompt_uses_pointer() {
    for form in [
        vec!["copilot", "--subprocess-safe", "--", "-p", SENTINEL],
        vec!["copilot", "--subprocess-safe", "--", "--prompt", SENTINEL],
        vec!["copilot", "--subprocess-safe", "--", "-i", SENTINEL],
    ] {
        let f = Fixture::new();
        let out = f.run(&mut f.cmd(&form));
        assert!(out.status.success(), "{form:?}\n{}", describe(&out));
        let argv = f.argv(1);
        assert_argv_clean(&argv, &[]);
        assert_proc_cmdline_clean(&f, 1);
        let pointer = f.file(1, "pointer");
        assert_private(&pointer, true);
        let add_dir = f.file(1, "--add-dir");
        assert_eq!(
            Some(add_dir.path.as_path()),
            pointer.path.parent(),
            "{form:?}: --add-dir names only the prompt's private directory"
        );
        assert_eq!(f.content(1, "pointer"), SENTINEL, "{form:?}");
        assert!(
            !pointer.path.exists(),
            "{form:?}: the pointer file is removed after exit"
        );
    }
}

/// Crusty round 2: which Codex options take a value depends on the
/// subcommand. Review option values stay put; the text after a fork `-i`
/// value is the prompt.
#[test]
fn codex_passthrough_review_and_fork_options() {
    for (form, want_tail, want_stdin) in [
        (
            vec!["exec", "review", "--commit", "abc123"],
            vec!["review", "--commit", "abc123"],
            "",
        ),
        (
            vec!["exec", "review", "--base", "main", SENTINEL],
            vec!["review", "--base", "main", "-"],
            SENTINEL,
        ),
        (
            vec!["exec", "fork", "abc-123", "-i", "a.png", SENTINEL],
            vec!["fork", "abc-123", "-i", "a.png", "-"],
            SENTINEL,
        ),
    ] {
        let f = Fixture::new();
        let mut args = vec!["codex", "--subprocess-safe", "--"];
        args.extend(&form);
        let out = f.run(&mut f.cmd(&args));
        assert!(out.status.success(), "{form:?}\n{}", describe(&out));
        let argv = f.argv(1);
        assert_argv_clean(&argv, &[]);
        assert_proc_cmdline_clean(&f, 1);
        assert!(
            argv.ends_with(&want_tail.iter().map(|a| a.to_string()).collect::<Vec<_>>()),
            "{form:?}: {argv:?}"
        );
        assert_eq!(f.stdin(1), want_stdin, "{form:?}");
    }
}

/// Crusty round 4: input piped to amplihack is not dropped when the prompt
/// takes the agent's stdin. The agent reads the prompt, a blank line, then
/// the piped input, for Claude `-p` and for `codex exec`. A null stdin adds
/// nothing after the prompt.
#[test]
fn piped_stdin_follows_the_prompt() {
    const PIPED: &str = "diff --git a/x b/x\n+added line\n";
    for args in [
        vec!["claude", "--subprocess-safe", "--", "-p", SENTINEL],
        vec!["codex", "--subprocess-safe", "--", "exec", SENTINEL],
    ] {
        let f = Fixture::new();
        let mut c = f.cmd(&args);
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().expect("spawn amplihack");
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(PIPED.as_bytes()).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{args:?}\n{}", describe(&out));
        assert_eq!(
            f.stdin(1),
            format!("{SENTINEL}\n\n{PIPED}"),
            "{args:?}: the agent gets the prompt and then the piped input"
        );
        assert_argv_clean(&f.argv(1), &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("not passed on"), "{stderr}");
        assert!(!stderr.contains(SENTINEL), "{stderr}");

        let f = Fixture::new();
        let out = f.run(&mut f.cmd(&args));
        assert!(out.status.success(), "{args:?}\n{}", describe(&out));
        assert_eq!(f.stdin(1), SENTINEL, "{args:?}: null stdin adds nothing");
    }
}

/// The external LiteLLM gateway, with a stub Copilot reporting a verified
/// version.
fn gateway(c: &mut Command) -> &mut Command {
    c.env("AMPLIHACK_LITELLM_ENDPOINT", "http://127.0.0.1:9")
        .env("AMPLIHACK_LITELLM_API_KEY", "sk-qa")
        .env("AMPLIHACK_LITELLM_MODEL", "claude-opus-5.5")
        .env(
            "STUB_VERSION",
            amplihack_utils::litellm_proxy::COPILOT_ISOLATED_RUNTIME_VERSION,
        )
}

/// Crusty round 2: under the gateway Copilot gets no `--add-dir`, so the
/// pointer file is readable only with path access.
///
/// These runs have no terminal, so amplihack runs subprocess-safe and adds
/// `--allow-all-paths` even when the caller's `--allow-all-tools` stops it
/// adding `--allow-all`: that form (Copilot's own `--help` example) launches
/// and Copilot can read the file. On a terminal there is no subprocess-safe
/// grant; that case is `copilot_path_access_on_a_terminal` in
/// `tests_codex_copilot_rewrite.rs`. `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1`
/// removes every grant, so that launch is refused before anything runs.
#[test]
fn gateway_copilot_prompt_needs_path_access() {
    let f = Fixture::new();
    let out = f.run(
        gateway(&mut f.cmd(&["copilot", "--", "-p", SENTINEL, "--allow-all-tools"]))
            .env("AMPLIHACK_COPILOT_NO_ALLOW_ALL", "1"),
    );
    assert!(!out.status.success(), "{}", describe(&out));
    assert_eq!(f.runs(), 0, "nothing is spawned\n{}", describe(&out));
    let all = describe(&out);
    assert!(all.contains("add --allow-all-paths"), "{all}");
    assert!(!all.contains(SENTINEL), "{all}");

    for (extra, grant) in [
        (vec!["--allow-all-tools"], "--allow-all-paths"),
        (
            vec!["--allow-all-tools", "--allow-all-paths"],
            "--allow-all-paths",
        ),
        (vec![], "--allow-all"),
    ] {
        let f = Fixture::new();
        let mut args = vec!["copilot", "--", "-p", SENTINEL];
        args.extend(&extra);
        let out = f.run(gateway(&mut f.cmd(&args)));
        assert!(out.status.success(), "{extra:?}\n{}", describe(&out));
        let argv = f.argv(1);
        assert_argv_clean(&argv, &[]);
        assert!(!argv.iter().any(|a| a == "--add-dir"), "{argv:?}");
        assert!(argv.iter().any(|a| a == grant), "{extra:?}: {argv:?}");
        assert_eq!(f.content(1, "pointer"), SENTINEL, "{extra:?}");
    }
}

#[test]
fn env_argv_is_not_an_opt_in_and_keeps_the_task_out_of_argv() {
    // R1.2: `AMPLIHACK_PROMPT_DELIVERY=argv` is inherited by every nested
    // step, so it is no longer accepted. It resolves to auto (with a
    // tracing warning naming the value), and neither the task nor the
    // contract reaches any child's argv.
    let f = Fixture::new();
    let task = task_file(&f);
    let out = f.run(
        f.cmd(&[
            "claude",
            "--auto",
            "--max-turns",
            "1",
            "--prompt-file",
            task.to_str().unwrap(),
        ])
        .env("AMPLIHACK_PROMPT_DELIVERY", "argv"),
    );
    assert!(out.status.success(), "{}", describe(&out));
    assert!(f.runs() >= 1, "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("the prompt will be visible in the process table"),
        "the argv opt-in warning belongs to the removed mode: {}",
        describe(&out)
    );
    for n in 1..=f.runs() {
        assert_argv_clean(&f.argv(n), &[]);
    }
    assert!(
        (1..=f.runs()).any(|n| f.stdin(n).contains(SENTINEL)),
        "the task goes by stdin instead: {}",
        describe(&out)
    );
}

// ---------------------------------------------------------------------------
// quality-loop.yaml Copilot steps, run under bash with a stub `amplihack`
// ---------------------------------------------------------------------------

const QUALITY_LOOP_STEPS: &[&str] = &["coe-design-critique", "quality-audit", "coe-diff-critique"];

fn quality_loop_command(step_id: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/recipes/quality-loop.yaml");
    let yaml: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(p).unwrap()).unwrap();
    yaml["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(step_id))
        .and_then(|s| s["command"].as_str())
        .unwrap_or_else(|| panic!("no step {step_id}"))
        .to_string()
}

/// Run one step's command under bash with `amplihack` and `gh` stubbed. The
/// `amplihack` stub is the same recording stub, so `argv.1`/`files.1` hold
/// what `amplihack copilot` received.
fn run_quality_loop_step(f: &Fixture, step_id: &str, extra_path: Option<&Path>) -> Output {
    crate::support::exec_stub::write_executable(&f.tools.join("amplihack"), STUB, 0o755);
    crate::support::exec_stub::write_executable(
        &f.tools.join("gh"),
        "#!/bin/sh\necho 'diff --git a/x b/x'\n",
        0o755,
    );
    let script = quality_loop_command(step_id)
        .replace("{{task_description}}", SENTINEL)
        .replace("{{repo_path}}", f.work.to_str().unwrap())
        .replace("{{new_pr_number}}", "7");
    let tmpdir = f.root.join("tmp");
    fs::create_dir_all(&tmpdir).unwrap();
    let path = match extra_path {
        Some(p) => format!("{}:{}:/usr/bin:/bin", p.display(), f.tools.display()),
        None => format!("{}:/usr/bin:/bin", f.tools.display()),
    };
    Command::new("bash")
        .arg("-c")
        .arg(script)
        .env_clear()
        .env("PATH", path)
        .env("HOME", &f.home)
        .env("TMPDIR", &tmpdir)
        .env("COPILOT_SESSION_ID", "s2030")
        .env("RECORD_DIR", &f.records)
        .env("FILE_REFERENCE_PREFIX", FILE_REFERENCE_PREFIX)
        .current_dir(&f.work)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn quality_loop_snippet_under_bash() {
    for step in QUALITY_LOOP_STEPS {
        let f = Fixture::new();
        let out = run_quality_loop_step(&f, step, None);
        assert!(out.status.success(), "{step}: {}", describe(&out));
        let argv = f.argv(1);
        assert_eq!(
            argv.first().map(String::as_str),
            Some("copilot"),
            "{step}: {argv:?}"
        );
        // No prompt text: neither the untrusted task nor the fixed header.
        assert_argv_clean(&argv, &["Invoke the "]);
        let pointer = f.file(1, "pointer");
        assert_private(&pointer, true);
        assert!(f.content(1, "pointer").contains("Invoke the "), "{step}");
        let dirs: Vec<PathBuf> = f
            .files(1)
            .into_iter()
            .filter(|r| r.flag == "--add-dir")
            .map(|r| r.path)
            .collect();
        assert!(
            dirs.iter()
                .any(|d| Some(d.as_path()) == pointer.path.parent()),
            "{step}: --add-dir grants the prompt's private dir: {argv:?}"
        );
        assert!(
            !pointer.path.exists(),
            "{step}: the EXIT trap removed the prompt"
        );
        assert!(
            !pointer.path.parent().unwrap().exists(),
            "{step}: and its dir"
        );
    }
}

#[test]
fn quality_loop_trap_safe_when_mktemp_fails() {
    for step in QUALITY_LOOP_STEPS {
        let f = Fixture::new();
        let broken = f.root.join("broken");
        fs::create_dir_all(&broken).unwrap();
        crate::support::exec_stub::write_executable(
            &broken.join("mktemp"),
            "#!/bin/sh\nexit 1\n",
            0o755,
        );
        let keep = f.work.join("keep.txt");
        fs::write(&keep, "keep").unwrap();
        let out = run_quality_loop_step(&f, step, Some(&broken));
        assert!(!out.status.success(), "{step}: {}", describe(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(step),
            "{step}: the failure names the step: {}",
            describe(&out)
        );
        assert_eq!(
            f.runs(),
            0,
            "{step}: copilot never runs without a prompt file"
        );
        assert!(
            keep.exists(),
            "{step}: the trap removed nothing it did not create"
        );
    }
}
