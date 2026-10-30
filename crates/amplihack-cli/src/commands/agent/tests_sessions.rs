//! Unit tests for `sessions.rs`: Claude's `sessions/<pid>.json` files, the
//! config directories (`~/.claude` and every profile directory) with their
//! privacy rule (S10) and profile-root errors (B6), and the transcript path
//! with both encodings (A4, D12).
//!
//! API under test (`env` stands for the process environment, so tests never
//! change it; `config_dirs(home)` is `config_dirs_from(home, |k|
//! std::env::var_os(k))`, and likewise `profile_config_dir`):
//! - `config_dirs_from(home: &Path, env: impl Fn(&str) -> Option<OsString>)
//!   -> Result<Vec<PathBuf>, String>`: `home/.claude` first, then
//!   `root/<name>` for each real directory in the profile root
//! - `profile_config_dir_from(home: &Path, profile: Option<&str>, env: impl
//!   Fn(&str) -> Option<OsString>) -> Result<PathBuf, String>`
//! - `parse_session_file(raw: &[u8]) -> Option<SessionFile>`,
//!   `SessionFile { pid: u32, session_id: String, proc_start: u64, cwd:
//!   Option<PathBuf> }`
//! - `index(dirs: &[PathBuf]) -> Result<Index, String>`, `Index { by_id: map
//!   String -> Vec<(u32, u64)> }`
//! - `for_process(dirs: &[PathBuf], pid: u32, start: u64) -> Option<SessionFile>`
//! - `project_dir_names(launch_dir: &Path) -> Vec<String>`,
//!   `transcript_exists(config_dir: &Path, launch_dir: &Path, id: &str) -> bool`

use super::sessions::*;
use serde_json::json;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const ID: &str = "0b6c3f2e-8d4a-4f1b-9c7e-2a5d6e8f1a3b";
const ID2: &str = "6f3b9d2e-4a7c-4e1b-8c5d-2e9a7f1b4d36";
const ROOT_VAR: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";

fn chmod(p: &Path, m: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
}

fn private_dir(p: &Path) -> PathBuf {
    fs::create_dir_all(p).unwrap();
    chmod(p, 0o700);
    p.to_path_buf()
}

/// An environment holding only the profile root `root`.
fn env_with(root: &Path) -> impl Fn(&str) -> Option<OsString> + '_ {
    move |k: &str| (k == ROOT_VAR).then(|| root.as_os_str().to_owned())
}

/// `home/.claude` and a root with profiles `a` and `b` (real, 0700), a
/// symlinked profile `s` and a stray file.
fn layout() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = private_dir(&tmp.path().join("home"));
    private_dir(&home.join(".claude"));
    let root = private_dir(&tmp.path().join("profiles"));
    private_dir(&root.join("a"));
    private_dir(&root.join("b"));
    let elsewhere = private_dir(&tmp.path().join("elsewhere"));
    std::os::unix::fs::symlink(&elsewhere, root.join("s")).unwrap();
    fs::write(root.join("notes"), "").unwrap();
    (tmp, home, root)
}

fn write_session(dir: &Path, name: &str, body: &serde_json::Value) {
    let s = private_dir(&dir.join("sessions"));
    fs::write(s.join(name), body.to_string()).unwrap();
}

#[test]
fn session_files_need_pid_id_and_start() {
    let raw = json!({"pid": 41237, "sessionId": ID, "procStart": "88123456", "cwd": "/srv/p",
                     "kind": "interactive", "entrypoint": "cli", "startedAt": 1});
    let s = parse_session_file(raw.to_string().as_bytes()).expect("string procStart");
    assert_eq!(
        (s.pid, s.session_id.as_str(), s.proc_start),
        (41237, ID, 88123456)
    );
    assert_eq!(s.cwd, Some(PathBuf::from("/srv/p")));
    let raw = json!({"pid": 7, "sessionId": ID, "procStart": 99});
    let s = parse_session_file(raw.to_string().as_bytes()).expect("integer procStart");
    assert_eq!((s.proc_start, s.cwd), (99, None));
    for bad in [
        json!({"sessionId": ID, "procStart": "1"}),
        json!({"pid": 7, "procStart": "1"}),
        json!({"pid": 7, "sessionId": ID}),
        json!({"pid": -7, "sessionId": ID, "procStart": "1"}),
        json!({"pid": 7, "sessionId": "--print", "procStart": "1"}),
        json!({"pid": 7, "sessionId": "../x", "procStart": "1"}),
        json!({"pid": 7, "sessionId": ID, "procStart": "soon"}),
        json!([1, 2]),
    ] {
        assert!(
            parse_session_file(bad.to_string().as_bytes()).is_none(),
            "{bad}"
        );
    }
    assert!(parse_session_file(b"\xff not json").is_none());
}

#[test]
fn config_dirs_are_claude_then_each_real_profile_directory() {
    let (_t, home, root) = layout();
    let mut dirs = config_dirs_from(&home, env_with(&root)).unwrap();
    assert_eq!(dirs.remove(0), home.join(".claude"));
    dirs.sort();
    assert_eq!(
        dirs,
        [root.join("a"), root.join("b")],
        "symlinks and files are skipped"
    );
    let missing = root.join("not-there");
    assert_eq!(
        config_dirs_from(&home, env_with(&missing)).unwrap(),
        [home.join(".claude")]
    );
}

#[test]
fn claude_and_the_root_may_be_symlinks_to_private_directories() {
    let (t, home, root) = layout();
    fs::remove_dir(home.join(".claude")).unwrap();
    let dot = private_dir(&t.path().join("dotfiles/claude"));
    std::os::unix::fs::symlink(&dot, home.join(".claude")).unwrap();
    let link = t.path().join("root-link");
    std::os::unix::fs::symlink(&root, &link).unwrap();
    let dirs = config_dirs_from(&home, env_with(&link)).unwrap();
    assert_eq!(dirs.len(), 3, "{dirs:?}");
}

#[test]
fn a_config_directory_others_can_write_makes_the_list_an_error() {
    let (_t, home, root) = layout();
    chmod(&root.join("b"), 0o777);
    let e = config_dirs_from(&home, env_with(&root)).unwrap_err();
    assert!(
        e.contains("profile b") && !e.contains(&root.display().to_string()),
        "S10: {e}"
    );
    chmod(&root.join("b"), 0o700);
    chmod(&home.join(".claude"), 0o777);
    let e = config_dirs_from(&home, env_with(&root)).unwrap_err();
    assert!(e.contains("~/.claude"), "{e}");
    chmod(&home.join(".claude"), 0o700);
    private_dir(&root.join("a/sessions"));
    chmod(&root.join("a/sessions"), 0o757);
    let r = config_dirs_from(&home, env_with(&root)).and_then(|d| index(&d).map(|_| ()));
    assert!(r.is_err(), "a sessions/ others can write is an error");
    chmod(&root, 0o777);
    assert!(
        config_dirs_from(&home, env_with(&root)).is_err(),
        "the root itself"
    );
}

#[test]
fn an_unlistable_root_is_an_error() {
    if amplihack_utils::secure_files::euid() == 0 {
        return; // root lists anything
    }
    let (_t, home, root) = layout();
    chmod(&root, 0o300);
    let r = config_dirs_from(&home, env_with(&root));
    chmod(&root, 0o700);
    assert!(r.is_err());
}

#[test]
fn profile_root_errors_never_quote_the_variable() {
    let (_t, home, _) = layout();
    let env = |k: &str| (k == ROOT_VAR).then(|| OsString::from("relative/secret-value"));
    let e = config_dirs_from(&home, env).unwrap_err();
    assert!(
        e.contains("AMPLIHACK_CLAUDE_PROFILE_ROOT is not a valid absolute path"),
        "B6: {e}"
    );
    assert!(!e.contains("secret-value"), "{e}");
}

#[test]
fn the_index_reads_every_sessions_directory() {
    let (_t, home, root) = layout();
    let (claude, a) = (home.join(".claude"), root.join("a"));
    write_session(
        &claude,
        "10.json",
        &json!({"pid": 10, "sessionId": ID, "procStart": "100"}),
    );
    write_session(
        &a,
        "20.json",
        &json!({"pid": 20, "sessionId": ID, "procStart": 200}),
    );
    write_session(
        &a,
        "30.json",
        &json!({"pid": 30, "sessionId": ID2, "procStart": "300"}),
    );
    write_session(&a, "40.json", &json!({"broken": true}));
    write_session(
        &a,
        "50.json",
        &json!({"pid": 50, "sessionId": ID2, "procStart": "500", "pad": "x".repeat(70_000)}),
    );
    std::os::unix::fs::symlink(a.join("sessions/30.json"), a.join("sessions/31.json")).unwrap();
    let dirs = vec![claude, a, root.join("b")];
    let idx = index(&dirs).expect("a missing sessions/ is empty, not an error");
    let mut one = idx.by_id.get(ID).cloned().unwrap_or_default();
    one.sort_unstable();
    assert_eq!(one, [(10, 100), (20, 200)]);
    assert_eq!(
        idx.by_id.get(ID2).cloned().unwrap_or_default(),
        [(30, 300)],
        "no symlink, no 64 KiB+"
    );
}

#[test]
fn for_process_matches_pid_and_start() {
    let (_t, home, root) = layout();
    let (claude, a) = (home.join(".claude"), root.join("a"));
    write_session(
        &a,
        "20.json",
        &json!({"pid": 20, "sessionId": ID, "procStart": "200", "cwd": "/w"}),
    );
    write_session(
        &claude,
        "other.json",
        &json!({"pid": 30, "sessionId": ID2, "procStart": "300"}),
    );
    let dirs = [claude, a];
    let s = for_process(&dirs, 20, 200).expect("found in the second directory");
    assert_eq!(
        (s.session_id.as_str(), s.cwd),
        (ID, Some(PathBuf::from("/w")))
    );
    assert!(for_process(&dirs, 20, 201).is_none(), "pid reused");
    assert!(
        for_process(&dirs, 30, 300).is_none(),
        "only <pid>.json is read"
    );
}

#[test]
fn the_profile_config_directory() {
    let (_t, home, root) = layout();
    let env = env_with(&root);
    assert_eq!(
        profile_config_dir_from(&home, None, &env).unwrap(),
        home.join(".claude")
    );
    assert_eq!(
        profile_config_dir_from(&home, Some("a"), &env).unwrap(),
        root.join("a")
    );
    let missing = "--profile: the profile's directory must exist under the profile root and must not be a symlink";
    for p in ["gone", "s"] {
        assert_eq!(
            profile_config_dir_from(&home, Some(p), &env).unwrap_err(),
            missing,
            "{p}"
        );
    }
    let e = profile_config_dir_from(&home, Some("../elsewhere"), &env).unwrap_err();
    assert!(
        e.starts_with("--profile: names match") && !e.contains("elsewhere"),
        "S8: {e}"
    );
}

#[test]
fn transcripts_are_found_under_either_encoding() {
    assert_eq!(
        project_dir_names(Path::new("/home/u/src/plain")),
        ["-home-u-src-plain"]
    );
    let names = project_dir_names(Path::new("/home/u/src/my.app_x"));
    assert_eq!(names, ["-home-u-src-my-app_x", "-home-u-src-my-app-x"]);
    let tmp = tempfile::tempdir().unwrap();
    let launch = Path::new("/home/u/src/my.app_x");
    assert!(!transcript_exists(tmp.path(), launch, ID));
    let alt = tmp.path().join("projects/-home-u-src-my-app-x");
    fs::create_dir_all(&alt).unwrap();
    fs::write(alt.join(format!("{ID}.jsonl")), "{}\n").unwrap();
    assert!(
        transcript_exists(tmp.path(), launch, ID),
        "A4 second encoding"
    );
    assert!(!transcript_exists(tmp.path(), launch, ID2));
}
