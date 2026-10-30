//! Issue #1710 (D8): `autodrive_paths` is the only Rust code that knows where
//! auto-drive keeps its state. The writer is `amplifier-bundle/tools/
//! autodrive_state.sh`, so these tests run that script under bash and check
//! that both give the same key and root, and that every file-name constant is
//! one the shell writers use.
//!
//! Contract:
//! - `state_key(origin: Option<&str>, ident: &str) -> String`: the origin
//!   with one trailing `.git` removed (`local` when `None`), then `/<ident>`,
//!   with every byte outside `A-Za-z0-9._-` replaced by `_`.
//! - `state_root_with(state_dir: Option<&OsStr>, home: Option<&Path>) ->
//!   Option<PathBuf>`: `<state_dir>/auto-drive` when `state_dir` is set and
//!   non-empty, else `<home>/.amplihack/state/auto-drive`, else `None` (the
//!   shell falls back to `/tmp`; Rust reads nothing in that case).
//! - `state_dir(root: &Path, key: &str) -> Option<PathBuf>`: the key
//!   directory when it and the root are real directories owned by this user
//!   with no group or world write bit.
//! - `round_count(dir: &Path, prefix: &str) -> u32`: the number of distinct
//!   round numbers `N` among regular files named `<prefix>N` followed by `.`
//!   or the end, skipping names that contain `health` or `before`, reading at
//!   most 500 entries.
//! - `concern_count(dir: &Path) -> Option<u64>`: `concern_count` from
//!   `crusty-latest.json` when the file is private and the value is a whole
//!   non-negative number.

use super::autodrive_paths::{
    CRUSTY_LATEST, CRUSTY_ROUND_PREFIX, MERGE_READY_ROUND_PREFIX, PHASES_FILE, STATE_SUBDIR,
    concern_count, round_count, state_dir, state_key, state_root_with,
};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn workspace_root() -> PathBuf {
    let mut cur = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if cur.join("amplifier-bundle").is_dir() && cur.join("Cargo.toml").is_file() {
            return cur;
        }
        assert!(cur.pop(), "no workspace root above the crate");
    }
}

fn state_script() -> PathBuf {
    workspace_root().join("amplifier-bundle/tools/autodrive_state.sh")
}

/// Runs `body` under bash with `autodrive_state.sh` sourced, in a cleared
/// environment with a temporary HOME. Never touches the user's state.
fn bash(home: &Path, extra_env: &[(&str, &str)], body: &str, args: &[&str]) -> String {
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(format!(". \"$0\"; {body}"))
        .arg(state_script())
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run bash");
    assert!(
        out.status.success(),
        "bash failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).expect("utf-8 output");
    text.strip_suffix('\n').unwrap_or(&text).to_string()
}

fn git(home: &Path, args: &[&str]) {
    let ok = amplihack_git::command()
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git")
        .success();
    assert!(ok, "git {args:?} failed");
}

/// A repository whose `origin` is `url`, or none when `url` is `None`.
fn repo_with_origin(home: &Path, name: &str, url: Option<&str>) -> PathBuf {
    let repo = home.join(name);
    fs::create_dir_all(&repo).unwrap();
    git(home, &["init", "-q", repo.to_str().unwrap()]);
    if let Some(url) = url {
        git(
            home,
            &["-C", repo.to_str().unwrap(), "remote", "add", "origin", url],
        );
    }
    repo
}

fn have(tool: &str) -> bool {
    let found = Command::new(tool)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !found {
        eprintln!("skipping: {tool} is not installed");
    }
    found
}

// ---------------------------------------------------------------------------
// Parity with autodrive_state.sh
// ---------------------------------------------------------------------------

#[test]
fn state_key_matches_the_shell_writer_for_every_origin_shape() {
    if !have("bash") || !have("git") {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let origins = [
        "https://github.com/rysweet/amplihack-rs.git",
        "git@github.com:rysweet/amplihack-rs.git",
        "https://user:tok3n@github.com/owner/name.git",
        "ssh://git@example.com:2222/owner/name",
        "https://example.com/a b/\u{fc}n\u{ef}c\u{f6}de.git",
        "https://github.com/o/double.git.git",
    ];
    for (i, origin) in origins.iter().enumerate() {
        let repo = repo_with_origin(&home, &format!("r{i}"), Some(origin));
        for ident in ["1710", "feat/issue-1710"] {
            let shell = bash(
                &home,
                &[],
                "autodrive_state_key \"$1\" \"$2\"",
                &[repo.to_str().unwrap(), ident],
            );
            assert_eq!(
                state_key(Some(origin), ident),
                shell,
                "origin #{i} with ident {ident:?}"
            );
        }
    }
}

#[test]
fn state_key_without_an_origin_matches_the_shell_local_fallback() {
    if !have("bash") || !have("git") {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let repo = repo_with_origin(&home, "bare", None);
    let shell = bash(
        &home,
        &[],
        "autodrive_state_key \"$1\" \"$2\"",
        &[repo.to_str().unwrap(), "1710"],
    );
    assert_eq!(shell, "local_1710");
    assert_eq!(state_key(None, "1710"), shell);
}

#[test]
fn state_root_matches_the_shell_with_and_without_a_state_dir() {
    if !have("bash") {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let custom = home.join("custom-state");
    let custom_s = custom.to_str().unwrap();

    let shell = bash(
        &home,
        &[("AMPLIHACK_STATE_DIR", custom_s)],
        "autodrive_state_root",
        &[],
    );
    assert_eq!(
        state_root_with(Some(OsStr::new(custom_s)), Some(&home)),
        Some(PathBuf::from(&shell))
    );

    let shell = bash(&home, &[], "autodrive_state_root", &[]);
    assert_eq!(
        state_root_with(None, Some(&home)),
        Some(PathBuf::from(&shell))
    );

    // `${AMPLIHACK_STATE_DIR:-...}` treats an empty value as unset.
    let shell = bash(
        &home,
        &[("AMPLIHACK_STATE_DIR", "")],
        "autodrive_state_root",
        &[],
    );
    assert_eq!(
        state_root_with(Some(OsStr::new("")), Some(&home)),
        Some(PathBuf::from(&shell))
    );
}

#[test]
fn every_file_name_constant_is_used_by_the_shell_writers() {
    let bundle = workspace_root().join("amplifier-bundle");
    let mut writers = String::new();
    for rel in [
        "tools/autodrive_state.sh",
        "recipes/autodrive-crusty-round.yaml",
        "recipes/autodrive-merge-round.yaml",
    ] {
        writers.push_str(&fs::read_to_string(bundle.join(rel)).unwrap());
    }
    for constant in [
        PHASES_FILE,
        CRUSTY_ROUND_PREFIX,
        MERGE_READY_ROUND_PREFIX,
        CRUSTY_LATEST,
    ] {
        assert!(
            writers.contains(constant),
            "{constant:?} is not used by any auto-drive writer"
        );
    }
    assert!(
        fs::read_to_string(state_script())
            .unwrap()
            .contains(&format!("/{STATE_SUBDIR}")),
        "STATE_SUBDIR must match autodrive_state_root"
    );
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

#[test]
fn state_key_strips_one_git_suffix_and_maps_every_other_byte() {
    assert_eq!(
        state_key(Some("https://github.com/o/n.git"), "12"),
        "https___github.com_o_n_12"
    );
    assert_eq!(state_key(Some("x.git.git"), "1"), "x.git_1");
    // Two bytes of UTF-8 become two underscores, as `tr -c` does.
    assert_eq!(state_key(Some("\u{e9}"), "1"), "___1");
    assert_eq!(state_key(None, ""), "local_");
}

#[test]
fn state_root_is_none_without_home_or_state_dir() {
    assert_eq!(state_root_with(None, None), None);
    assert_eq!(state_root_with(Some(OsStr::new("")), None), None);
    assert_eq!(
        state_root_with(Some(OsStr::new("/s")), None),
        Some(PathBuf::from("/s/auto-drive"))
    );
}

fn private_dir(path: &Path) -> PathBuf {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    path.to_path_buf()
}

fn private_file(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn state_dir_accepts_a_private_directory_and_refuses_loose_or_linked_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let root = private_dir(&tmp.path().join("auto-drive"));
    let key = private_dir(&root.join("k"));
    assert_eq!(state_dir(&root, "k"), Some(key.clone()));

    assert_eq!(state_dir(&root, "missing"), None);

    fs::set_permissions(&key, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(state_dir(&root, "k"), None, "a world-writable key dir");
    fs::set_permissions(&key, fs::Permissions::from_mode(0o700)).unwrap();

    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(state_dir(&root, "k"), None, "a world-writable root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();

    let elsewhere = private_dir(&tmp.path().join("elsewhere"));
    std::os::unix::fs::symlink(&elsewhere, root.join("link")).unwrap();
    assert_eq!(state_dir(&root, "link"), None, "a symlinked key dir");
}

#[test]
fn rounds_are_distinct_round_numbers_not_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = private_dir(&tmp.path().join("k"));
    for name in [
        "crusty-round-1.log",
        "crusty-round-1.json",
        "crusty-round-1.json.findings",
        "crusty-round-1-health.log",
        "crusty-round-1-before.log",
        "crusty-round-2.log",
        "crusty-round-2.json",
        "merge-ready-round-1.json",
        "merge-ready-round-1-before.log",
        "crusty-records.tsv",
        "crusty-latest.json",
    ] {
        private_file(&dir.join(name), "{}");
    }
    assert_eq!(round_count(&dir, CRUSTY_ROUND_PREFIX), 2);
    assert_eq!(round_count(&dir, MERGE_READY_ROUND_PREFIX), 1);
}

#[test]
fn health_and_before_files_alone_are_not_rounds() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = private_dir(&tmp.path().join("k"));
    private_file(&dir.join("crusty-round-3-health.log"), "");
    private_file(&dir.join("merge-ready-round-2-before.log"), "");
    assert_eq!(round_count(&dir, CRUSTY_ROUND_PREFIX), 0);
    assert_eq!(round_count(&dir, MERGE_READY_ROUND_PREFIX), 0);
}

#[test]
fn only_regular_files_count_as_rounds() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = private_dir(&tmp.path().join("k"));
    private_file(&dir.join("crusty-round-1.json"), "{}");
    fs::create_dir(dir.join("crusty-round-2.json")).unwrap();
    std::os::unix::fs::symlink(
        dir.join("crusty-round-1.json"),
        dir.join("crusty-round-3.json"),
    )
    .unwrap();
    assert_eq!(round_count(&dir, CRUSTY_ROUND_PREFIX), 1);
}

#[test]
fn a_missing_directory_has_no_rounds() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(
        round_count(&tmp.path().join("nope"), CRUSTY_ROUND_PREFIX),
        0
    );
    assert_eq!(concern_count(&tmp.path().join("nope")), None);
}

#[test]
fn round_counting_reads_at_most_500_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = private_dir(&tmp.path().join("k"));
    for n in 1..=700 {
        private_file(&dir.join(format!("crusty-round-{n}.json")), "");
    }
    let counted = round_count(&dir, CRUSTY_ROUND_PREFIX);
    assert!(counted <= 500, "counted {counted}");
}

#[test]
fn concern_count_reads_only_a_whole_number() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = private_dir(&tmp.path().join("k"));
    let latest = dir.join(CRUSTY_LATEST);
    let cases: [(&str, Option<u64>); 7] = [
        (
            r#"{"crusty_verdict":"CONCERNS","concern_count":3}"#,
            Some(3),
        ),
        (r#"{"concern_count":0}"#, Some(0)),
        (r#"{"concern_count":"3"}"#, None),
        (r#"{"concern_count":-1}"#, None),
        (r#"{"concern_count":2.5}"#, None),
        (r#"{"other":1}"#, None),
        ("not json", None),
    ];
    for (body, want) in cases {
        private_file(&latest, body);
        assert_eq!(concern_count(&dir), want, "{body}");
    }
}

#[test]
fn concern_count_ignores_a_writable_or_linked_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = private_dir(&tmp.path().join("k"));
    let latest = dir.join(CRUSTY_LATEST);
    private_file(&latest, r#"{"concern_count":4}"#);
    fs::set_permissions(&latest, fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(concern_count(&dir), None);

    fs::remove_file(&latest).unwrap();
    let target = tmp.path().join("real.json");
    private_file(&target, r#"{"concern_count":4}"#);
    std::os::unix::fs::symlink(&target, &latest).unwrap();
    assert_eq!(concern_count(&dir), None);
}
