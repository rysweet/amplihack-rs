//! Issue #2820: judging one same-user process whose environment cannot be
//! read. Stand-in `/proc` trees only; the real `/proc` is never read.

use super::unreadable::{self, Verdict};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

struct Stand {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    dir: PathBuf,
}

impl Stand {
    fn new(cmdline: &[u8]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("profiles");
        for name in ["X", "Y", "Z"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        let dir = tmp.path().join("proc/700000");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cmdline"), cmdline).unwrap();
        fs::create_dir(dir.join("environ")).unwrap(); // unreadable
        Self {
            _tmp: tmp,
            root,
            dir,
        }
    }

    /// Rewrites the command line in place, so the root stays the same.
    fn args(&self, args: &[String]) -> &Self {
        let mut raw = Vec::new();
        for a in args {
            raw.extend_from_slice(a.as_bytes());
            raw.push(0);
        }
        fs::write(self.dir.join("cmdline"), raw).unwrap();
        self
    }

    fn with_args(args: &[String]) -> Self {
        let s = Self::new(b"");
        s.args(args);
        s
    }

    fn judge(&self, comm: &str) -> Option<unreadable::Unreadable> {
        unreadable::judge(&self.dir, 700000, comm, &self.root)
    }

    fn p(&self, rest: &str) -> String {
        format!("{}{rest}", self.root.display())
    }
}

fn counted(names: &[&str]) -> Verdict {
    Verdict::Counted(names.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>())
}

#[test]
fn a_claude_with_a_settings_path_names_that_profile() {
    let s = Stand::new(b"");
    s.args(&["claude".into(), "--settings".into(), s.p("/Y")]);
    assert_eq!(s.judge("claude").unwrap().verdict, counted(&["Y"]));
}

#[test]
fn key_equals_path_and_flag_equals_key_equals_path_name_the_profile() {
    let s = Stand::new(b"");
    s.args(&["claude".into(), format!("CLAUDE_CONFIG_DIR={}", s.p("/Z"))]);
    assert_eq!(s.judge("claude").unwrap().verdict, counted(&["Z"]));
    s.args(&[
        "claude".into(),
        format!("--env=CLAUDE_CONFIG_DIR={}", s.p("/Z")),
    ]);
    assert_eq!(s.judge("claude").unwrap().verdict, counted(&["Z"]));
}

#[test]
fn a_path_under_a_profile_names_that_profile() {
    let s = Stand::new(b"");
    s.args(&["claude".into(), s.p("/X/projects/a.json")]);
    assert_eq!(s.judge("claude").unwrap().verdict, counted(&["X"]));
}

#[test]
fn several_named_profiles_are_each_doubtful() {
    let s = Stand::new(b"");
    s.args(&["claude".into(), s.p("/X"), s.p("/Y")]);
    assert_eq!(s.judge("claude").unwrap().verdict, counted(&["X", "Y"]));
}

#[test]
fn a_claude_naming_no_profile_is_profile_unknown_and_makes_none_live() {
    let s = Stand::with_args(&["claude".into(), "-p".into(), "x".into()]);
    assert_eq!(s.judge("claude").unwrap().verdict, Verdict::ProfileUnknown);
}

#[test]
fn amplihack_executables_are_claude_like() {
    for exe in ["amplihack", "amplihack-hooks", "/usr/bin/amplihack-rs"] {
        let s = Stand::with_args(&[exe.into(), "x".into()]);
        let v = s.judge("amplihack").unwrap().verdict;
        assert_eq!(v, Verdict::ProfileUnknown, "{exe}");
    }
}

#[test]
fn node_running_the_claude_code_entry_point_is_claude_like() {
    let s = Stand::new(b"");
    s.args(&[
        "node".into(),
        "/x/node_modules/@anthropic-ai/claude-code/cli.js".into(),
        s.p("/Y"),
    ]);
    assert_eq!(s.judge("node").unwrap().verdict, counted(&["Y"]));
}

/// The Tailscale SSH case of issue #2820: text only, not an executable.
#[test]
fn tailscaled_with_amplihack_in_its_command_is_not_claude() {
    let s = Stand::with_args(&[
        "tailscaled".into(),
        "be-child".into(),
        "ssh".into(),
        "amplihack wait".into(),
    ]);
    assert_eq!(s.judge("tailscaled").unwrap().verdict, Verdict::NotClaude);
}

#[test]
fn an_unrelated_unreadable_process_is_not_reported_at_all() {
    let s = Stand::with_args(&["ssh-agent".into(), "-D".into()]);
    assert!(s.judge("ssh-agent").is_none());
}

#[test]
fn a_renamed_binary_is_judged_by_comm_or_argv0_not_by_text() {
    let by_comm = Stand::with_args(&["python".into(), "run.py".into()]);
    assert!(by_comm.judge("claude").unwrap().verdict != Verdict::NotClaude);
    let spoof = Stand::with_args(&["bash".into(), "-c".into(), "claude".into()]);
    assert_eq!(spoof.judge("bash").unwrap().verdict, Verdict::NotClaude);
}

#[test]
fn paths_that_only_look_like_profiles_name_nothing() {
    let s = Stand::new(b"");
    let outside = s._tmp.path().join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, s.root.join("link")).unwrap();
    let cases = [
        s.p(""),
        s.p("/.."),
        s.p("/../X"),
        s.p("/link"),
        s.p("/missing/../X"),
        "X".to_string(),
        String::new(),
    ];
    for arg in cases {
        s.args(&["claude".into(), arg.clone()]);
        let v = s.judge("claude").unwrap().verdict;
        assert_eq!(v, Verdict::ProfileUnknown, "{arg:?}");
    }
}

#[test]
fn a_symlinked_root_still_names_its_profiles() {
    let s = Stand::new(b"");
    let alias = s._tmp.path().join("alias");
    std::os::unix::fs::symlink(&s.root, &alias).unwrap();
    s.args(&["claude".into(), format!("{}/X", alias.display())]);
    let v = s.judge("claude").unwrap().verdict;
    assert_eq!(v, counted(&["X"]));
}

#[test]
fn exe_is_the_argv0_basename_and_falls_back_to_comm() {
    let a = Stand::with_args(&["/usr/local/bin/claude".into()]);
    assert_eq!(a.judge("claude").unwrap().exe, "claude");
    let empty = Stand::new(b"");
    assert_eq!(empty.judge("claude").unwrap().exe, "claude");
    let assign = Stand::with_args(&["FOO=bar".into(), "claude".into()]);
    assert_eq!(assign.judge("claude").unwrap().exe, "claude");
}

#[test]
fn odd_command_lines_never_panic() {
    let huge = vec![b'a'; 200 * 1024];
    for raw in [&huge[..], b"\xff\xfe\0claude\0", b"\0\0\0", b""] {
        let s = Stand::new(raw);
        let _ = s.judge("claude");
    }
}

#[test]
fn debug_output_never_shows_argv_text() {
    let s = Stand::with_args(&["claude".into(), "SECRET_TOKEN_VALUE".into()]);
    let u = s.judge("claude").unwrap();
    let shown = format!("{u:?}");
    assert!(!shown.contains("SECRET_TOKEN_VALUE"), "{shown}");
}

#[test]
fn judge_does_not_need_exe_or_environ_content() {
    // Only cmdline exists besides the environ directory: no `exe` link.
    let s = Stand::with_args(&["claude".into()]);
    assert!(!Path::new(&s.dir.join("exe")).exists());
    assert!(s.judge("claude").is_some());
}
