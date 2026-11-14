//! Issue #2042: every `amplihack profile` or `sessions` command a test starts
//! reads a fake process table. The binary honours `AMPLIHACK_TEST_PROC_ROOT`
//! only under the S27 conditions and otherwise reads the real `/proc`; only
//! debug builds say so, on stderr, which a test may not read. So the test
//! side is checked here too. Nothing is spawned.
//!
//! The scanner is `support/proc_root_scan.rs`. Fix a finding by making the
//! code read plainly, not by adding an exception. Its rules are the table in
//! `docs/howto/keep-tests-off-the-host-process-table.md`; change that table
//! and the `BAD`/`GOOD` samples here together.

use crate::isolated_amplihack::{
    PROC_ROOT_ENV, isolated, reapply_proc_root, shared_empty_proc_root,
};
use crate::support::proc_root_scan::{Rule, is_fn_start, scan};
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

#[test]
#[should_panic(expected = "read the host /proc")]
fn the_fixture_check_fails_on_the_refusal_line() {
    crate::isolated_amplihack::assert_proc_root_honoured(
        "amplihack: AMPLIHACK_TEST_PROC_ROOT ignored (x); reading /proc\n",
    );
}

// --- 2 and 3: the production side -----------------------------------------

#[test]
fn procfs_reads_the_variable_by_this_name_and_names_its_exceptions() {
    let root = manifest().join("src/commands/agent/procfs/root.rs");
    let src = std::fs::read_to_string(root).unwrap();
    assert!(
        src.contains(&format!("\"{VAR}\"")),
        "procfs/root.rs reads {VAR}"
    );
    for exception in [
        "/proc/loadavg",
        "/proc/self",
        "util.rs",
        "rotator_down",
        "running_on_host",
        "recipe/process_tree.rs",
    ] {
        assert!(
            src.contains(exception),
            "procfs/root.rs documents {exception}"
        );
    }
    // S27: the seam is read in one place; only its unit tests name it too.
    let mut files = Vec::new();
    rust_files(&manifest().join("src"), &mut files);
    let mut naming: Vec<String> = files
        .iter()
        .filter(|f| std::fs::read_to_string(f).unwrap().contains(VAR))
        .map(|f| f.strip_prefix(manifest()).unwrap().display().to_string())
        .collect();
    naming.sort();
    let expected = [
        "src/commands/agent/procfs/root.rs",
        "src/commands/agent/procfs/tests_root.rs",
    ];
    assert_eq!(naming, expected);
}

/// Files that read the process table and must do it through `proc_root()`.
const ROUTED: [&str; 10] = [
    // Issue #2040 moved the system scan into the shared snapshot: liveness.rs
    // takes the proc root as an argument and proc_snapshot names it.
    "src/commands/profile/proc_snapshot/mod.rs",
    "src/commands/profile/rotate/control.rs",
    "src/commands/profile/rotate/owner.rs",
    "src/commands/profile/rotate/daemon.rs",
    "src/commands/profile/rotate/autostart.rs",
    "src/commands/profile/rotate/change_log.rs",
    "src/commands/profile/rotate/ledger.rs",
    "src/commands/profile/rotate/mod.rs",
    "src/commands/sessions/live.rs",
    "src/commands/cleanup/procs.rs",
];

/// Trees in which no non-test file may name a `/proc` path, apart from
/// [`HOST_PROC_ALLOWED`]. A whole directory, not a file list, so a reader
/// added in a new file is caught too.
const GUARDED_TREES: [&str; 2] = ["src/commands/profile", "src/commands/sessions"];

/// `/proc` paths that may stay on the host table, each documented in
/// `procfs/root.rs`: the process's own entries and the load average.
const HOST_PROC_ALLOWED: [&str; 2] = ["\"/proc/self/", "\"/proc/loadavg\""];

/// Every line of `src` that starts a string literal with `/proc` (for
/// example `"/proc"`, `"/proc/{pid}/cmdline"` inside `format!`) and is not a
/// comment or an allowed host path.
fn host_proc_literals(src: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find("\"/proc") {
            let tail = &rest[at..];
            let next = tail.as_bytes().get(6).copied();
            let is_proc_path = matches!(next, Some(b'"') | Some(b'/'));
            if is_proc_path && !HOST_PROC_ALLOWED.iter().any(|ok| tail.starts_with(ok)) {
                found.push((i + 1, line.trim().to_string()));
                break;
            }
            rest = &tail[1..];
        }
    }
    found
}

fn is_unit_test_file(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|n| n.starts_with("tests"))
}

#[test]
fn routed_files_read_through_the_seam() {
    let mut bad = Vec::new();
    let mut files: Vec<PathBuf> = ROUTED.iter().map(|f| manifest().join(f)).collect();
    for tree in GUARDED_TREES {
        rust_files(&manifest().join(tree), &mut files);
    }
    files.sort();
    files.dedup();
    for path in files.iter().filter(|p| !is_unit_test_file(p)) {
        let src = std::fs::read_to_string(path).unwrap();
        let rel = path.strip_prefix(manifest()).unwrap().display().to_string();
        for (line, text) in host_proc_literals(&src) {
            bad.push(format!("{rel}:{line}: {text}"));
        }
    }
    for file in ROUTED {
        let src = std::fs::read_to_string(manifest().join(file)).unwrap();
        if !src.contains("proc_root()") {
            bad.push(format!("{file}: never calls proc_root()"));
        }
    }
    assert!(
        bad.is_empty(),
        "host /proc paths outside procfs::proc_root() (see \
         docs/howto/keep-tests-off-the-host-process-table.md):\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_host_proc_matcher_catches_every_spelling_of_a_proc_path() {
    let bad = [
        "let d = fs::read_dir(\"/proc\");",
        "let p = format!(\"/proc/{pid}/cmdline\");",
        "let e = format!(\"/proc/{}/environ\", pid);",
        "let r = PathBuf::from(\"/proc/\");",
        "x(\"/proc/self\".to_string(), \"/proc/1/stat\")",
    ];
    for sample in bad {
        assert_eq!(host_proc_literals(sample).len(), 1, "missed: {sample}");
    }
    let good = [
        "// fs::read_dir(\"/proc\")",
        "let f = File::open(\"/proc/loadavg\");",
        "let f = File::open(\"/proc/self/fd/0\");",
        "let r = proc_root().join(pid.to_string());",
        "let s = \"/process\";",
    ];
    for sample in good {
        assert!(
            host_proc_literals(sample).is_empty(),
            "false finding: {sample}"
        );
    }
    assert!(is_unit_test_file(Path::new("a/tests_live.rs")));
    assert!(!is_unit_test_file(Path::new("a/live.rs")));
}

// --- 4 and 5: the test side -------------------------------------------------

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
    // This file holds the bad samples; the scanner holds the rule strings.
    let skip = [Path::new(file!()), Path::new("proc_root_scan.rs")].map(|p| p.file_name());
    let mut findings = Vec::new();
    for file in files.iter().filter(|f| !skip.contains(&f.file_name())) {
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

const BAD: [(&str, Rule); 9] = [
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
    // `async fn` and `pub(super) fn` split: the reapply in `run` does not
    // cover the `env_clear()` in `base`.
    (
        "async fn base() -> Command {\n    let mut c = isolated(amplihack_bin());\n    c.env_clear();\n    c\n}\npub(super) fn run() {\n    let mut c = base();\n    reapply_proc_root(&mut c);\n    c.args([\"profile\", \"list\"]);\n}\n",
        Rule::D,
    ),
    // A spawn in another function does not cover this `link_own`.
    (
        "fn hold() -> Child {\n    Command::new(\"sleep\").arg(\"600\").spawn().unwrap()\n}\nfn run(fake: &FakeProc, pid: u32) {\n    fake.link_own(pid);\n}\n",
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
pub(super) async fn later(args: &[&str]) -> Command {
    let mut c = isolated(amplihack_bin());
    c.env_clear();
    reapply_proc_root(&mut c);
    c.args(args);
    c
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
    assert_eq!(scan(BAD[7].0), [(3, Rule::D)]);
    assert_eq!(scan(BAD[8].0), [(5, Rule::E)]);
}

#[test]
fn every_function_declaration_starts_a_function() {
    for decl in [
        "fn a() {",
        "pub fn a() {",
        "  pub(crate) fn a() {",
        "pub(super) fn a() {",
    ] {
        assert!(is_fn_start(decl), "{decl}");
    }
    for decl in [
        "async fn a() {",
        "pub(in crate::x) async fn a() {",
        "const unsafe fn a() {",
    ] {
        assert!(is_fn_start(decl), "{decl}");
    }
    for other in [
        "let f = fn_name;",
        "pub struct Fn;",
        "const FN: u8 = 1;",
        "publish fn",
    ] {
        assert!(!is_fn_start(other), "{other}");
    }
}
