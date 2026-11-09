//! Parity corpus runner (#1554, consensus step 3).
//!
//! `tests/fixtures/workflow_checkpoint/corpus/<action>.toml`, one file per
//! row of the engine's ACTIONS table. Expected bytes were captured from the
//! 918c256cf Python (provenance `python-918c256cf:<asset>#<case>`) before the
//! `.py` files were deleted, or come from a named owner decision. `@ROOT@` in
//! argv, env, stdin and expectations is the canonicalised temp root, unless
//! the case sets `raw_path = true`. `files` writes fixture files (keys are
//! relative to the temp root, values are text with `@ROOT@` substituted)
//! after `setup` and before the run.
//!
//! Structure is checked for every file before any case runs, so a malformed
//! file fails even when its cases would pass.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use base64::Engine as _;

use crate::support::amplihack_bin::amplihack_bin;

const CORPUS: &str = "crates/amplihack-cli/tests/fixtures/workflow_checkpoint/corpus";

const KEYS: [&str; 18] = [
    "name",
    "argv",
    "stdin",
    "stdin_b64",
    "env",
    "setup",
    "expect_status",
    "expect_stdout",
    "expect_stdout_b64",
    "expect_stderr",
    "expect_stderr_b64",
    "provenance",
    "inventory_ref",
    "raw_path",
    "note",
    "cwd",
    "expect_files",
    "files",
];

const SETUPS: [&str; 6] = [
    "none",
    "quiet_repo",
    "two_repos_a_b",
    "alt_index",
    "advert_server",
    "git_shim_79",
];

const PROVENANCE: [&str; 6] = [
    "python-918c256cf:",
    "owner:17.126:",
    "owner:17.138:",
    "owner:D1",
    "cc-00:42Z:",
    "cc-22:36Z-float:",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn action_names() -> Vec<String> {
    amplihack_cli::workflow_checkpoint::ACTIONS
        .iter()
        .map(|a| a.name.to_owned())
        .collect()
}

fn load(action: &str) -> Vec<toml::value::Table> {
    let path = root().join(CORPUS).join(format!("{action}.toml"));
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing corpus file {}", path.display()));
    let doc: toml::Value = toml::from_str(&text).expect("corpus parses");
    doc.get("case")
        .and_then(|c| c.as_array())
        .unwrap_or_else(|| panic!("{action}: no [[case]]"))
        .iter()
        .map(|c| c.as_table().expect("case table").clone())
        .collect()
}

/// What `expect_files` may say about a path (a file or a whole directory,
/// such as a repository's `.git`): byte-identical before and after the run,
/// changed by it, absent afterwards, or present afterwards.
const FILE_EXPECTATIONS: [&str; 4] = ["unchanged", "changed", "absent", "present"];

/// One observed filesystem entry. Links are never followed: a symlink is
/// recorded by its literal target, so retargeting it is a change and its
/// target's contents are not read.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Dir {
        mode: u32,
    },
    File {
        mode: u32,
        size: u64,
        sha256: String,
    },
    Symlink {
        target: std::ffi::OsString,
    },
    Other {
        mode: u32,
        file_type: String,
    },
}

/// Every entry at or under a path, keyed by path relative to it.
type FileState = BTreeMap<PathBuf, Entry>;

fn observe_entry(path: &Path) -> std::io::Result<Entry> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    let meta = fs::symlink_metadata(path)?;
    let mode = meta.permissions().mode();
    let kind = meta.file_type();
    Ok(if kind.is_symlink() {
        Entry::Symlink {
            target: fs::read_link(path)?.into_os_string(),
        }
    } else if kind.is_dir() {
        Entry::Dir { mode }
    } else if kind.is_file() {
        let bytes = fs::read(path)?;
        Entry::File {
            mode,
            size: meta.len(),
            sha256: Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        }
    } else {
        let file_type = if kind.is_fifo() {
            "fifo"
        } else if kind.is_socket() {
            "socket"
        } else if kind.is_block_device() {
            "block"
        } else {
            "char"
        };
        Entry::Other {
            mode,
            file_type: file_type.into(),
        }
    })
}

/// The state of `path`: `Ok(None)` only when the path itself is confirmed
/// absent (NotFound). Every other error, at the path or anywhere under it,
/// is returned, so unknown state never counts as absent or unchanged.
fn file_state(path: &Path) -> Result<Option<FileState>, String> {
    let top = match observe_entry(path) {
        Ok(entry) => entry,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut out = FileState::new();
    let walk = matches!(top, Entry::Dir { .. });
    out.insert(PathBuf::new(), top);
    if walk {
        let mut stack = vec![path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            for entry in entries {
                let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
                let child = entry.path();
                let observed =
                    observe_entry(&child).map_err(|e| format!("{}: {e}", child.display()))?;
                if matches!(observed, Entry::Dir { .. }) {
                    stack.push(child.clone());
                }
                let rel = child.strip_prefix(path).unwrap_or(&child).to_path_buf();
                out.insert(rel, observed);
            }
        }
    }
    Ok(Some(out))
}

/// Check that `path` is still confined to the case fixture: the bound root
/// is still a real directory at the same canonical path, `path` is under it
/// with no `.` or `..`, and no existing component between the root and the
/// path's last component is a symlink. Run before every observation, before
/// and after the case, so a parent swapped for a link in between is caught.
fn confined(root: &Path, path: &Path) -> Result<(), String> {
    use std::path::Component;
    match fs::symlink_metadata(root) {
        Ok(m) if m.file_type().is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "fixture root {} is not a directory",
                root.display()
            ));
        }
        Err(e) => return Err(format!("fixture root {}: {e}", root.display())),
    }
    if root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?
        != root
    {
        return Err(format!("fixture root {} moved", root.display()));
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(format!("{}: '.' and '..' are not allowed", path.display()));
    }
    let rel = path.strip_prefix(root).map_err(|_| {
        format!(
            "{}: outside the case fixture {}",
            path.display(),
            root.display()
        )
    })?;
    let parts: Vec<_> = rel.components().collect();
    let mut at = root.to_path_buf();
    for part in parts.iter().take(parts.len().saturating_sub(1)) {
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!("{}: {} is a symlink", path.display(), at.display()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(format!("{}: {}: {e}", path.display(), at.display())),
        }
    }
    Ok(())
}

/// Resolve an `expect_files` key for one case: `@ROOT@` is substituted and
/// a relative key is taken against the case's working directory. The result
/// must pass [`confined`].
fn expect_path(key: &str, root: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let text = subst(key, root, false);
    let candidate = Path::new(&text);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    };
    confined(root, &joined).map_err(|e| format!("{key}: {e}"))?;
    Ok(joined)
}

/// Observe a confined path: the confinement is checked again first.
fn observe_confined(root: &Path, path: &Path) -> Result<Option<FileState>, String> {
    confined(root, path)?;
    file_state(path)
}

fn s<'a>(t: &'a toml::value::Table, k: &str) -> Option<&'a str> {
    t.get(k).and_then(|v| v.as_str())
}

/// Every structural rule from the consensus design. Returns problems.
fn check_structure(action: &str, cases: &[toml::value::Table]) -> Vec<String> {
    let mut p = Vec::new();
    let mut names = BTreeSet::new();
    let mut by_input: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut python = 0;
    for c in cases {
        let name = s(c, "name").unwrap_or("");
        if name.is_empty() || !names.insert(name.to_owned()) {
            p.push(format!("{action}: missing or duplicate name {name:?}"));
        }
        for k in c.keys() {
            if !KEYS.contains(&k.as_str()) {
                p.push(format!("{action}#{name}: unknown key {k}"));
            }
        }
        let prov = s(c, "provenance").unwrap_or("");
        if !PROVENANCE.iter().any(|pre| prov.starts_with(pre)) {
            p.push(format!("{action}#{name}: provenance {prov:?}"));
        }
        if prov.starts_with("python-918c256cf:") {
            python += 1;
        }
        if !SETUPS.contains(&s(c, "setup").unwrap_or("none")) {
            p.push(format!("{action}#{name}: setup"));
        }
        if c.get("expect_status")
            .and_then(|v| v.as_integer())
            .is_none()
        {
            p.push(format!("{action}#{name}: expect_status"));
        }
        if c.get("argv").and_then(|v| v.as_array()).is_none() {
            p.push(format!("{action}#{name}: argv"));
        }
        if let Some(files) = c.get("expect_files") {
            match files.as_table() {
                Some(table) => {
                    for (path, want) in table {
                        if !want
                            .as_str()
                            .is_some_and(|w| FILE_EXPECTATIONS.contains(&w))
                        {
                            p.push(format!(
                                "{action}#{name}: expect_files.{path} must be one of {FILE_EXPECTATIONS:?}"
                            ));
                        }
                    }
                }
                None => p.push(format!("{action}#{name}: expect_files must be a table")),
            }
        }
        if let Some(files) = c.get("files") {
            match files.as_table() {
                Some(table) => {
                    for (path, body) in table {
                        let rel = Path::new(path);
                        if body.as_str().is_none()
                            || rel.is_absolute()
                            || rel
                                .components()
                                .any(|p| !matches!(p, std::path::Component::Normal(_)))
                        {
                            p.push(format!(
                                "{action}#{name}: files.{path} must be a relative path to text"
                            ));
                        }
                    }
                }
                None => p.push(format!("{action}#{name}: files must be a table")),
            }
        }
        for stream in ["stdin", "expect_stdout", "expect_stderr"] {
            let text = c.contains_key(stream);
            let b64 = c.contains_key(&format!("{stream}_b64"));
            let required = stream != "stdin";
            if (text && b64) || (required && !text && !b64) {
                p.push(format!(
                    "{action}#{name}: exactly one of {stream} / {stream}_b64"
                ));
            }
        }
        // The same input (every field the runner feeds the child, including
        // the case's files) must never carry two different expectations.
        let input = format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
            c.get("argv"),
            c.get("env"),
            c.get("stdin").or(c.get("stdin_b64")),
            s(c, "setup"),
            s(c, "cwd"),
            c.get("files"),
            c.get("raw_path")
        );
        let expect = format!(
            "{:?}|{:?}|{:?}",
            c.get("expect_status"),
            c.get("expect_stdout").or(c.get("expect_stdout_b64")),
            c.get("expect_stderr").or(c.get("expect_stderr_b64"))
        );
        if let Some((other_prov, other_expect)) = by_input.get(&input) {
            if *other_expect != expect {
                p.push(format!(
                    "{action}#{name}: contradicts the {other_prov} case for the same input"
                ));
            }
        } else {
            by_input.insert(input, (prov.to_owned(), expect));
        }
    }
    if python == 0 {
        p.push(format!("{action}: no python-918c256cf case"));
    }
    p
}

#[test]
fn corpus_has_one_file_per_action_and_no_strays() {
    let dir = root().join(CORPUS);
    let files: BTreeSet<String> = fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("create {}", dir.display()))
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let want: BTreeSet<String> = action_names().iter().map(|a| format!("{a}.toml")).collect();
    assert_eq!(files, want);
}

#[test]
fn every_corpus_file_is_structurally_valid() {
    let mut problems = Vec::new();
    for action in action_names() {
        problems.extend(check_structure(&action, &load(&action)));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_path_printing_action_has_a_raw_path_case() {
    // @ROOT@ normalisation must not hide a wrong absolute path.
    for action in ["paths", "capture", "worktree-path"] {
        assert!(
            load(action)
                .iter()
                .any(|c| c.get("raw_path").and_then(|v| v.as_bool()) == Some(true)),
            "{action}: add a raw_path = true case"
        );
    }
}

#[test]
fn admission_refused_result_carries_cc_s_thirteen_float_cases() {
    let n = load("admission-refused-result")
        .iter()
        .filter(|c| s(c, "provenance").is_some_and(|p| p.starts_with("cc-22:36Z-float:")))
        .count();
    assert_eq!(n, 13, "cc's 13 matching float cases");
}

#[test]
fn structure_check_rejects_its_negative_cases() {
    let good: toml::value::Table = toml::from_str(
        "name='a'\nargv=[]\nsetup='none'\nexpect_status=0\nexpect_stdout=''\nexpect_stderr=''\nprovenance='python-918c256cf:x#a'\n",
    )
    .unwrap();
    assert!(check_structure("x", std::slice::from_ref(&good)).is_empty());
    type Mutation = fn(&mut toml::value::Table);
    let variants: [(&str, Mutation); 8] = [
        ("unknown key", |t| {
            t.insert("bogus".into(), true.into());
        }),
        ("no provenance", |t| {
            t.remove("provenance");
        }),
        ("bad provenance", |t| {
            t.insert("provenance".into(), "guess".into());
        }),
        ("both stdout forms", |t| {
            t.insert("expect_stdout_b64".into(), "".into());
        }),
        ("no stderr", |t| {
            t.remove("expect_stderr");
        }),
        ("unknown setup", |t| {
            t.insert("setup".into(), "magic".into());
        }),
        ("escaping fixture file", |t| {
            let mut files = toml::value::Table::new();
            files.insert("../x".into(), "".into());
            t.insert("files".into(), files.into());
        }),
        ("absolute fixture file", |t| {
            let mut files = toml::value::Table::new();
            files.insert("/x".into(), "".into());
            t.insert("files".into(), files.into());
        }),
    ];
    for (what, f) in variants {
        let mut t = good.clone();
        f(&mut t);
        assert!(!check_structure("x", &[t]).is_empty(), "{what} accepted");
    }
    // Contradicting provenance for the same input.
    let mut other = good.clone();
    other.insert("provenance".into(), "owner:17.138:2".into());
    other.insert("name".into(), "b".into());
    other.insert("expect_status".into(), toml::Value::Integer(2));
    assert!(!check_structure("x", &[good.clone(), other]).is_empty());
    // Same provenance, same input, different expectation: still refused.
    let mut same_prov = good.clone();
    same_prov.insert("name".into(), "c".into());
    same_prov.insert("expect_status".into(), toml::Value::Integer(2));
    assert!(!check_structure("x", &[good.clone(), same_prov.clone()]).is_empty());
    // Different fixture files are a different input, not a contradiction.
    let mut files = toml::value::Table::new();
    files.insert("tmp/list".into(), "a\0".into());
    same_prov.insert("files".into(), files.into());
    assert!(check_structure("x", &[good.clone(), same_prov]).is_empty());
    // A file with no python-918c256cf case.
    let mut owner_only = good;
    owner_only.insert("provenance".into(), "owner:D1".into());
    assert!(!check_structure("x", &[owner_only]).is_empty());
}

// ----------------------------------------------------------------- runner

fn git(dir: &Path, args: &[&str]) {
    let out = amplihack_git::command_in(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Corpus")
        .env("GIT_AUTHOR_EMAIL", "corpus@example.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_NAME", "Corpus")
        .env("GIT_COMMITTER_EMAIL", "corpus@example.invalid")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn quiet_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "gc.auto", "0"]);
    fs::write(dir.join("README"), "seed\n").unwrap();
    git(dir, &["add", "README"]);
    git(dir, &["commit", "-q", "-m", "seed"]);
}

/// Returns the working directory for the case.
fn setup(kind: &str, root: &Path) -> PathBuf {
    match kind {
        "none" => root.to_path_buf(),
        "quiet_repo" | "alt_index" => {
            quiet_repo(&root.join("a"));
            root.join("a")
        }
        "two_repos_a_b" => {
            quiet_repo(&root.join("a"));
            quiet_repo(&root.join("b"));
            root.join("a")
        }
        other => panic!("corpus setup {other:?} is not implemented in the runner yet"),
    }
}

fn subst(text: &str, root: &Path, raw: bool) -> String {
    if raw {
        text.to_owned()
    } else {
        text.replace("@ROOT@", &root.to_string_lossy())
    }
}

fn bytes(c: &toml::value::Table, key: &str, root: &Path, raw: bool) -> Option<Vec<u8>> {
    if let Some(t) = s(c, key) {
        return Some(subst(t, root, raw).into_bytes());
    }
    s(c, &format!("{key}_b64")).map(|b| {
        base64::engine::general_purpose::STANDARD
            .decode(b)
            .expect("valid base64")
    })
}

/// Runs every case in `<action>.toml` and returns how many ran and every
/// mismatch. It asserts nothing itself; the caller makes the one assertion.
fn run_action(action: &str) -> (usize, Vec<String>) {
    let mut wrong = Vec::new();
    let cases = load(action);
    let ran = cases.len();
    for c in cases {
        let name = s(&c, "name").unwrap_or("?").to_owned();
        let tmp = tempfile::Builder::new()
            .prefix("wf-corpus-")
            .tempdir()
            .unwrap();
        let root = tmp.path().canonicalize().unwrap();
        for sub in ["home", "tmp"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        let raw = c.get("raw_path").and_then(|v| v.as_bool()) == Some(true);
        let cwd = setup(s(&c, "setup").unwrap_or("none"), &root);
        let cwd = s(&c, "cwd").map_or(cwd, |p| PathBuf::from(subst(p, &root, false)));
        for (rel, body) in c
            .get("files")
            .and_then(|v| v.as_table())
            .into_iter()
            .flatten()
        {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, subst(body.as_str().unwrap_or(""), &root, false)).unwrap();
        }
        let mut cmd = crate::isolated_amplihack::isolated(amplihack_bin());
        // Exactly the environment the 918c capture used. `isolated` keeps a
        // few host variables; clearing again drops them too: nothing inherited
        // from the test process (a recipe step's TASK_DESCRIPTION, a Git
        // selector, BASH_ENV or ENV would change the outcome), then the
        // case's own variables.
        cmd.env_clear()
            .env("HOME", root.join("home"))
            .env("TMPDIR", root.join("tmp"))
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LC_ALL", "C")
            .current_dir(&cwd)
            .args(["internal", "workflow-checkpoint", action]);
        for a in c
            .get("argv")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            cmd.arg(subst(a.as_str().expect("argv strings"), &root, raw));
        }
        for (k, v) in c
            .get("env")
            .and_then(|v| v.as_table())
            .into_iter()
            .flatten()
        {
            cmd.env(k, subst(v.as_str().expect("env strings"), &root, raw));
        }
        // expect_files: record each listed path before the run, so file and
        // index preservation are checked, not assumed. A path that cannot be
        // resolved inside the fixture, or observed, fails the case.
        let mut files: Vec<(PathBuf, String, Option<FileState>)> = Vec::new();
        for (key, want) in c
            .get("expect_files")
            .and_then(|v| v.as_table())
            .into_iter()
            .flatten()
        {
            let observed = expect_path(key, &root, &cwd)
                .and_then(|path| observe_confined(&root, &path).map(|state| (path, state)));
            match observed {
                Ok((path, before)) => {
                    files.push((path, want.as_str().unwrap_or("").to_owned(), before))
                }
                Err(e) => wrong.push(format!("{action}#{name}: expect_files {e}")),
            }
        }
        let stdin = bytes(&c, "stdin", &root, raw);
        cmd.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn amplihack");
        if let Some(input) = stdin {
            child.stdin.take().unwrap().write_all(&input).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        for (path, want, before) in &files {
            let held = match observe_confined(&root, path) {
                Err(e) => Err(e),
                Ok(after) => Ok(match want.as_str() {
                    "unchanged" => before.is_some() && *before == after,
                    "changed" => *before != after,
                    "absent" => after.is_none(),
                    "present" => after.is_some(),
                    _ => false,
                }),
            };
            match held {
                Ok(true) => {}
                Ok(false) => wrong.push(format!(
                    "{action}#{name}: expect_files {} = {want} does not hold",
                    path.display()
                )),
                Err(e) => wrong.push(format!("{action}#{name}: expect_files {e}")),
            }
        }
        let want_status = c["expect_status"].as_integer().unwrap() as i32;
        let want_out = bytes(&c, "expect_stdout", &root, raw).unwrap_or_default();
        let want_err = bytes(&c, "expect_stderr", &root, raw).unwrap_or_default();
        if out.status.code() != Some(want_status)
            || out.stdout != want_out
            || out.stderr != want_err
        {
            wrong.push(format!(
                "{action}#{name} [{}]\n  want {want_status} {:?} {:?}\n  got  {:?} {:?} {:?}",
                s(&c, "provenance").unwrap_or(""),
                String::from_utf8_lossy(&want_out),
                String::from_utf8_lossy(&want_err),
                out.status.code(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            ));
        }
    }
    (ran, wrong)
}

/// Every action the corpus runner has a test for, in the order of the
/// `corpus_<action>` functions below.
const CORPUS_ACTIONS: [&str; 16] = [
    "selectors",
    "capture",
    "admit",
    "check",
    "paths",
    "stage-strict",
    "before",
    "sync-fetch-mode",
    "after",
    "destination-is-head",
    "verify-pushed-ref",
    "advertised-ref",
    "sha256",
    "admission-refused-result",
    "cleanup-skip-result",
    "worktree-path",
];

#[test]
fn corpus_selectors() {
    let (ran, wrong) = run_action("selectors");
    assert!(
        ran > 0 && wrong.is_empty(),
        "selectors: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_capture() {
    let (ran, wrong) = run_action("capture");
    assert!(
        ran > 0 && wrong.is_empty(),
        "capture: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_admit() {
    let (ran, wrong) = run_action("admit");
    assert!(
        ran > 0 && wrong.is_empty(),
        "admit: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_check() {
    let (ran, wrong) = run_action("check");
    assert!(
        ran > 0 && wrong.is_empty(),
        "check: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_paths() {
    let (ran, wrong) = run_action("paths");
    assert!(
        ran > 0 && wrong.is_empty(),
        "paths: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_stage_strict() {
    let (ran, wrong) = run_action("stage-strict");
    assert!(
        ran > 0 && wrong.is_empty(),
        "stage-strict: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_before() {
    let (ran, wrong) = run_action("before");
    assert!(
        ran > 0 && wrong.is_empty(),
        "before: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_sync_fetch_mode() {
    let (ran, wrong) = run_action("sync-fetch-mode");
    assert!(
        ran > 0 && wrong.is_empty(),
        "sync-fetch-mode: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_after() {
    let (ran, wrong) = run_action("after");
    assert!(
        ran > 0 && wrong.is_empty(),
        "after: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_destination_is_head() {
    let (ran, wrong) = run_action("destination-is-head");
    assert!(
        ran > 0 && wrong.is_empty(),
        "destination-is-head: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_verify_pushed_ref() {
    let (ran, wrong) = run_action("verify-pushed-ref");
    assert!(
        ran > 0 && wrong.is_empty(),
        "verify-pushed-ref: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_advertised_ref() {
    let (ran, wrong) = run_action("advertised-ref");
    assert!(
        ran > 0 && wrong.is_empty(),
        "advertised-ref: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_sha256() {
    let (ran, wrong) = run_action("sha256");
    assert!(
        ran > 0 && wrong.is_empty(),
        "sha256: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_admission_refused_result() {
    let (ran, wrong) = run_action("admission-refused-result");
    assert!(
        ran > 0 && wrong.is_empty(),
        "admission-refused-result: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_cleanup_skip_result() {
    let (ran, wrong) = run_action("cleanup-skip-result");
    assert!(
        ran > 0 && wrong.is_empty(),
        "cleanup-skip-result: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_worktree_path() {
    let (ran, wrong) = run_action("worktree-path");
    assert!(
        ran > 0 && wrong.is_empty(),
        "worktree-path: {ran} cases run, {} mismatched\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn corpus_runner_covers_every_action() {
    let covered: BTreeSet<&str> = CORPUS_ACTIONS.into_iter().collect();
    let actions = action_names();
    let actions: BTreeSet<&str> = actions.iter().map(String::as_str).collect();
    assert_eq!(covered, actions);
}

// ------------------------------------------- expect_files observations

fn fixture_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix("wf-corpus-obs-")
        .tempdir()
        .unwrap();
    let root = tmp.path().canonicalize().unwrap();
    (tmp, root)
}

#[test]
fn expect_files_tells_an_empty_directory_from_a_file_holding_dir_text() {
    let (_tmp, root) = fixture_root();
    fs::create_dir(root.join("d")).unwrap();
    fs::write(root.join("f"), b"<dir>").unwrap();
    let d = file_state(&root.join("d")).unwrap().unwrap();
    let f = file_state(&root.join("f")).unwrap().unwrap();
    assert_ne!(d, f);
}

#[test]
fn expect_files_records_a_symlink_by_its_literal_target_without_following_it() {
    let (_tmp, root) = fixture_root();
    fs::write(root.join("a"), b"same").unwrap();
    fs::write(root.join("b"), b"same").unwrap();
    std::os::unix::fs::symlink("a", root.join("link")).unwrap();
    let before = file_state(&root.join("link")).unwrap();
    // Same target bytes, different literal target: a change.
    fs::remove_file(root.join("link")).unwrap();
    std::os::unix::fs::symlink("b", root.join("link")).unwrap();
    assert_ne!(before, file_state(&root.join("link")).unwrap());
    // A dangling link is present, not absent.
    std::os::unix::fs::symlink("missing", root.join("dangling")).unwrap();
    assert!(file_state(&root.join("dangling")).unwrap().is_some());
}

#[test]
fn expect_files_sees_a_mode_change_with_identical_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, root) = fixture_root();
    fs::write(root.join("x"), b"x").unwrap();
    // Set both modes explicitly, so the umask cannot make them equal.
    fs::set_permissions(root.join("x"), fs::Permissions::from_mode(0o644)).unwrap();
    let before = file_state(&root.join("x")).unwrap();
    fs::set_permissions(root.join("x"), fs::Permissions::from_mode(0o600)).unwrap();
    let after = file_state(&root.join("x")).unwrap();
    assert_ne!(before, after);
    let mode = |state: &Option<FileState>| match state.as_ref().unwrap()[&PathBuf::new()] {
        Entry::File { mode, .. } => mode & 0o777,
        ref other => panic!("{other:?}"),
    };
    assert_eq!((mode(&before), mode(&after)), (0o644, 0o600));
}

#[test]
fn expect_files_reports_an_observation_error_and_only_not_found_is_absence() {
    // A path through a regular file fails with ENOTDIR whatever the
    // privileges, so the error is deterministic and inside the fixture.
    let (_tmp, root) = fixture_root();
    fs::write(root.join("plain"), b"p").unwrap();
    let err = file_state(&root.join("plain/child")).unwrap_err();
    assert!(err.contains("plain/child"), "{err}");
    assert_eq!(file_state(&root.join("nope")), Ok(None));
    // A link under a directory whose target fails is recorded literally;
    // its target is never followed, so it is not an error.
    fs::create_dir(root.join("d")).unwrap();
    std::os::unix::fs::symlink("../plain/child", root.join("d/link")).unwrap();
    let state = file_state(&root.join("d")).unwrap().unwrap();
    assert!(matches!(
        state[&PathBuf::from("link")],
        Entry::Symlink { .. }
    ));
}

#[test]
fn a_parent_redirected_to_another_task_owned_directory_is_caught_before_the_after_read() {
    let (_tmp, root) = fixture_root();
    let case_dir = root.join("a");
    fs::create_dir_all(case_dir.join(".git")).unwrap();
    fs::write(case_dir.join(".git/index"), b"original").unwrap();
    // Another task-owned directory with an identical index.
    fs::create_dir_all(root.join("other/.git")).unwrap();
    fs::write(root.join("other/.git/index"), b"original").unwrap();
    let path = expect_path(".git/index", &root, &case_dir).unwrap();
    let before = observe_confined(&root, &path).unwrap();
    // The case swaps its own parent for a link to the other directory.
    fs::rename(&case_dir, root.join("a-moved")).unwrap();
    std::os::unix::fs::symlink(root.join("other"), &case_dir).unwrap();
    // Read through the path, the bytes look unchanged ...
    assert_eq!(file_state(&path).unwrap(), before);
    // ... but the confined read refuses the redirected parent.
    let err = observe_confined(&root, &path).unwrap_err();
    assert!(err.contains("is a symlink"), "{err}");
}

#[test]
fn expect_files_keys_resolve_against_the_case_directory_and_stay_in_the_fixture() {
    let (_tmp, root) = fixture_root();
    let case_dir = root.join("a");
    fs::create_dir_all(case_dir.join(".git")).unwrap();
    assert_eq!(
        expect_path(".git/index", &root, &case_dir).unwrap(),
        case_dir.join(".git/index")
    );
    assert_eq!(
        expect_path("@ROOT@/a/.git", &root, &case_dir).unwrap(),
        case_dir.join(".git")
    );
    assert!(expect_path("../outside", &root, &case_dir).is_err());
    assert!(expect_path("/etc/passwd", &root, &case_dir).is_err());
    std::os::unix::fs::symlink("/etc", root.join("escape")).unwrap();
    assert!(expect_path("@ROOT@/escape/passwd", &root, &case_dir).is_err());
}
