//! Issue #2042: every `amplihack profile` or `sessions` command a test starts
//! reads a fake process table. The binary honours `AMPLIHACK_TEST_PROC_ROOT`
//! only under the S27 conditions and otherwise reads the real `/proc` without
//! a word, so the test side is checked here. Nothing is spawned.
//!
//! The scanner matches text, skips comment lines and splits functions at
//! lines starting with `fn`, `pub fn` or `pub(crate) fn`. Fix a finding by
//! making the code read plainly, not by adding an exception. Its rules are
//! the table in `docs/howto/keep-tests-off-the-host-process-table.md`;
//! change that table and the `BAD`/`GOOD` samples together.

use crate::isolated_amplihack::{
    PROC_ROOT_ENV, isolated, reapply_proc_root, shared_empty_proc_root,
};
use crate::support::profile_rotate::euid;
use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const VAR: &str = "AMPLIHACK_TEST_PROC_ROOT";

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn env_of<'a>(cmd: &'a Command, key: &str) -> Option<Option<&'a OsStr>> {
    cmd.get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v)
}

// --- 1: the default tree --------------------------------------------------

#[test]
fn isolated_points_the_child_at_a_private_empty_tree() {
    assert_eq!(PROC_ROOT_ENV, VAR);
    let cmd = isolated("x");
    let set = env_of(&cmd, PROC_ROOT_ENV)
        .flatten()
        .expect("isolated() sets AMPLIHACK_TEST_PROC_ROOT");
    let tree = Path::new(set);
    assert_eq!(tree, shared_empty_proc_root(), "the shared tree");
    assert_ne!(tree, Path::new("/proc"));
    let canon = tree.canonicalize().unwrap();
    let temp = std::env::temp_dir().canonicalize().unwrap();
    assert!(
        canon.starts_with(&temp) && canon != temp,
        "{canon:?} under {temp:?}"
    );
    for (dir, mode) in [(tree.to_path_buf(), 0o700), (tree.join("self"), 0o700)] {
        let m = std::fs::symlink_metadata(&dir).unwrap();
        assert!(m.is_dir(), "{dir:?} is a real directory");
        assert_eq!(m.uid(), euid(), "{dir:?} is ours");
        assert_eq!(m.mode() & 0o777, mode, "{dir:?} is private");
    }
    let count = |d: &Path| std::fs::read_dir(d).unwrap().count();
    assert_eq!(count(tree), 1, "only self/, no pid entries");
    assert_eq!(count(&tree.join("self")), 1, "only self/environ");
    let environ = tree.join("self/environ");
    let m = std::fs::symlink_metadata(&environ).unwrap();
    assert!(m.is_file());
    assert_eq!(m.mode() & 0o777, 0o600);
    assert_eq!(std::fs::read(&environ).unwrap(), b"AMPLIHACK_FAKE=1\0");
}

#[test]
fn reapply_restores_the_tree_after_env_clear() {
    let mut cmd = isolated("x");
    cmd.env_clear();
    assert_eq!(env_of(&cmd, PROC_ROOT_ENV), None, "env_clear drops it");
    reapply_proc_root(&mut cmd);
    assert_eq!(
        env_of(&cmd, PROC_ROOT_ENV),
        Some(Some(shared_empty_proc_root().as_os_str()))
    );
}

// --- 2 and 3: the production side -----------------------------------------

#[test]
fn procfs_reads_the_variable_by_this_name_and_names_its_exceptions() {
    let src = std::fs::read_to_string(manifest().join("src/commands/agent/procfs.rs")).unwrap();
    assert!(src.contains(&format!("\"{VAR}\"")), "procfs.rs reads {VAR}");
    for exception in ["/proc/loadavg", "util.rs", "rotator_down"] {
        assert!(src.contains(exception), "procfs.rs documents {exception}");
    }
}

const ROUTED: [&str; 9] = [
    "src/commands/profile/auto_switch/liveness.rs",
    "src/commands/profile/rotate/control.rs",
    "src/commands/profile/rotate/owner.rs",
    "src/commands/profile/rotate/daemon.rs",
    "src/commands/profile/rotate/autostart.rs",
    "src/commands/profile/rotate/change_log.rs",
    "src/commands/profile/rotate/ledger.rs",
    "src/commands/profile/rotate/mod.rs",
    "src/commands/sessions/live.rs",
];

#[test]
fn routed_files_read_through_the_seam() {
    let mut bad = Vec::new();
    for file in ROUTED {
        let src = std::fs::read_to_string(manifest().join(file)).unwrap();
        for (i, line) in src.lines().enumerate() {
            if !line.trim_start().starts_with("//") && line.contains("\"/proc\"") {
                bad.push(format!("{file}:{}: {}", i + 1, line.trim()));
            }
        }
        if !src.contains("proc_root()") {
            bad.push(format!("{file}: never calls proc_root()"));
        }
    }
    assert!(bad.is_empty(), "hard-coded /proc:\n{}", bad.join("\n"));
}

// --- 4 and 5: the test side -------------------------------------------------

/// The rules (a)-(e) of the how-to guide's scanner table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rule {
    A,
    B,
    C,
    D,
    E,
}

fn is_fn_start(line: &str) -> bool {
    let t = line.trim_start();
    ["fn ", "pub fn ", "pub(crate) fn "]
        .iter()
        .any(|p| t.starts_with(p))
}

fn sets_tree(line: &str) -> bool {
    line.contains("reapply_proc_root") || line.contains("PROC_ROOT_ENV") || line.contains(VAR)
}

/// `(line, rule)` for each finding in `src`, 1-based lines.
fn scan(src: &str) -> Vec<(usize, Rule)> {
    let lines: Vec<(usize, &str)> = src
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect();
    // `sessions` reads `/proc` through the same seam (`sessions/live.rs`).
    let mentions_profile = lines
        .iter()
        .any(|(_, l)| l.contains("\"profile\"") || l.contains("\"sessions\""));
    let starts_something = lines
        .iter()
        .any(|(_, l)| l.contains(".spawn()") || l.contains("\"rotate\", \"start\""));
    let mut out = Vec::new();
    for &(n, l) in &lines {
        if l.contains("env_remove(") && sets_tree(l) {
            out.push((n, Rule::B));
        }
        if (l.contains("PROC_ROOT_ENV") || l.contains(VAR)) && l.contains("\"/proc\"") {
            out.push((n, Rule::C));
        }
        if l.contains("link_own(") && !l.contains("fn link_own") {
            let arg = l.split("link_own(").nth(1).unwrap_or("");
            if arg.starts_with(|c: char| c.is_ascii_digit()) || !starts_something {
                out.push((n, Rule::E));
            }
        }
    }
    let mut starts: Vec<usize> = (0..lines.len())
        .filter(|&i| is_fn_start(lines[i].1))
        .collect();
    starts.push(lines.len());
    for w in starts.windows(2) {
        let body = &lines[w[0]..w[1]];
        let raw_bin = body
            .iter()
            .find(|(_, l)| l.contains("Command::new(") && l.contains("amplihack"));
        let builds = raw_bin.is_some() || body.iter().any(|(_, l)| l.contains("isolated("));
        if !builds || !mentions_profile {
            continue;
        }
        let any_tree = body.iter().any(|(_, l)| sets_tree(l));
        if let Some(&(n, _)) = raw_bin
            && !body.iter().any(|(_, l)| l.contains("isolated("))
            && !any_tree
        {
            out.push((n, Rule::A));
        }
        if let Some(i) = body.iter().rposition(|(_, l)| l.contains("env_clear()"))
            && !body[i..].iter().any(|(_, l)| sets_tree(l))
        {
            out.push((body[i].0, Rule::D));
        }
    }
    out.sort();
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for path in std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()) {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension() == Some(OsStr::new("rs")) {
            out.push(path);
        }
    }
}

#[test]
fn no_test_lets_a_profile_command_read_the_host_proc() {
    let tests = manifest().join("tests");
    let mut files = Vec::new();
    rust_files(&tests, &mut files);
    files.sort();
    assert!(files.len() > 50, "the scan found the test sources");
    let this = Path::new(file!()).file_name().unwrap();
    let mut findings = Vec::new();
    for file in files.iter().filter(|f| f.file_name() != Some(this)) {
        let src = std::fs::read_to_string(file).unwrap();
        let rel = file.strip_prefix(&tests).unwrap().display().to_string();
        for (line, rule) in scan(&src) {
            findings.push(format!("{rel}:{line} {rule:?}"));
        }
    }
    assert!(
        findings.is_empty(),
        "tests that can read the host /proc (see \
         docs/howto/keep-tests-off-the-host-process-table.md):\n{}",
        findings.join("\n")
    );
}

const BAD: [(&str, Rule); 7] = [
    (
        "fn run() {\n    let out = Command::new(amplihack_bin()).args([\"profile\", \"list\"]).output();\n}\n",
        Rule::A,
    ),
    (
        "fn run() {\n    let mut c = isolated(amplihack_bin());\n    c.env_remove(PROC_ROOT_ENV).args([\"profile\", \"list\"]);\n}\n",
        Rule::B,
    ),
    (
        "fn run() {\n    let mut c = isolated(amplihack_bin());\n    c.env(PROC_ROOT_ENV, \"/proc\").args([\"profile\", \"list\"]);\n}\n",
        Rule::C,
    ),
    (
        "fn base(args: &[&str]) -> Command {\n    let mut c = isolated(amplihack_bin());\n    c.env_clear().env(\"HOME\", \"/tmp/x\").args(args);\n    c\n}\nfn run() {\n    base(&[\"profile\", \"list\"]);\n}\n",
        Rule::D,
    ),
    (
        "fn base(args: &[&str]) -> Command {\n    let mut c = isolated(amplihack_bin());\n    reapply_proc_root(&mut c);\n    c.env_clear().args(args);\n    c\n}\nfn run() {\n    base(&[\"profile\", \"list\"]);\n}\n",
        Rule::D,
    ),
    (
        "fn run(fake: &FakeProc) {\n    let child = Command::new(\"sleep\").arg(\"600\").spawn().unwrap();\n    fake.link_own(4242);\n}\n",
        Rule::E,
    ),
    (
        "fn run(fake: &FakeProc, pid: u32) {\n    fake.link_own(pid);\n}\n",
        Rule::E,
    ),
];

const GOOD: &str = r#"
// A comment may say env_remove(PROC_ROOT_ENV) or link_own(1).
fn base(args: &[&str]) -> Command {
    let mut c = isolated(amplihack_bin());
    c.env_clear();
    reapply_proc_root(&mut c);
    c.args(args);
    c
}
pub fn with_fake(fake: &FakeProc) {
    let mut child = Command::new("sleep").arg("600").spawn().unwrap();
    fake.link_own(child.id());
    let out = base(&["profile", "rotate", "status"])
        .env(PROC_ROOT_ENV, fake.path())
        .output();
    child.kill().unwrap();
    child.wait().unwrap();
}
"#;

#[test]
fn scanner_flags_each_bad_sample_and_passes_the_good_one() {
    for (i, (src, rule)) in BAD.iter().enumerate() {
        let rules: Vec<Rule> = scan(src).into_iter().map(|(_, r)| r).collect();
        assert_eq!(rules, [*rule], "bad sample {i} ({rule:?}):\n{src}");
    }
    assert_eq!(scan(GOOD), [], "the good sample is clean");
    // The finding names the offending line.
    assert_eq!(scan(BAD[0].0), [(2, Rule::A)]);
    assert_eq!(scan(BAD[3].0), [(3, Rule::D)]);
    assert_eq!(scan(BAD[5].0), [(3, Rule::E)]);
}
