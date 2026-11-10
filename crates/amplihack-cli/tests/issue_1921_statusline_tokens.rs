//! Issue #1921: `calculate_tokens()` in `amplifier-bundle/tools/statusline.sh`
//! used to re-read the whole session transcript four times on every refresh.
//! It now counts tokens incrementally: one pass over the bytes after a cached
//! offset, up to the last newline at or before the size read at the start of
//! the call, with a per-transcript cache entry `INODE OFFSET TOTAL` stored in
//! `${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/statusline-tokens/<digest>`.
//!
//! Contract: docs/reference/STATUSLINE.md, section "Token count". These tests
//! pull the shipped function out of the script with
//! `sed -n '/^calculate_tokens()/,/^}/p'`, run it under `bash -c` with a
//! scrubbed environment whose `HOME` and `XDG_CACHE_HOME` point into a
//! temporary directory, and compare its output with a Rust oracle that applies
//! the four-key match to complete lines only. One case runs the whole script.
//!
//! No sleeps, retries or timing windows are used.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const KEYS: [&str; 4] = [
    "input_tokens",
    "output_tokens",
    "cache_read_input_tokens",
    "cache_creation_input_tokens",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn script() -> PathBuf {
    let path = repo_root().join("amplifier-bundle/tools/statusline.sh");
    assert!(path.is_file(), "{} must exist", path.display());
    path
}

/// A scratch area with `home/` and `xdg/` and a transcript path inside it.
struct Env {
    dir: TempDir,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir(dir.path().join("home")).unwrap();
        fs::create_dir(dir.path().join("xdg")).unwrap();
        fs::create_dir(dir.path().join("work")).unwrap();
        Env { dir }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    fn xdg(&self) -> PathBuf {
        self.root().join("xdg")
    }

    fn transcript(&self) -> PathBuf {
        self.root().join("work/transcript.jsonl")
    }

    /// The cache directory used when `XDG_CACHE_HOME` is set.
    fn cache_dir(&self) -> PathBuf {
        self.xdg().join("amplihack/statusline-tokens")
    }

    /// The cache directory used when `XDG_CACHE_HOME` is unset.
    fn home_cache_dir(&self) -> PathBuf {
        self.home().join(".cache/amplihack/statusline-tokens")
    }
}

/// How `XDG_CACHE_HOME` is presented to the function.
#[derive(Clone, Copy)]
enum Xdg<'a> {
    Default,
    Unset,
    At(&'a Path),
}

/// Run the shipped `calculate_tokens` with `args`. `prefix` runs before the
/// call and `suffix` after it, in the same shell.
fn run_raw(env: &Env, args: &[&str], xdg: Xdg<'_>, prefix: &str, suffix: &str) -> Output {
    let program = format!(
        "{prefix}\n\
         eval \"$(sed -n '/^calculate_tokens()/,/^}}/p' \"$1\")\" || exit 97\n\
         shift\n\
         calculate_tokens \"$@\"\n\
         {suffix}"
    );
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(program)
        .arg("bash")
        .arg(script())
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", env.home())
        .env("LANG", "C.UTF-8")
        .current_dir(env.root())
        .stdin(Stdio::null());
    match xdg {
        Xdg::Default => {
            cmd.env("XDG_CACHE_HOME", env.xdg());
        }
        Xdg::At(p) => {
            cmd.env("XDG_CACHE_HOME", p);
        }
        Xdg::Unset => {}
    }
    cmd.output().expect("run bash")
}

/// Call the function on `path` and return the integer it prints. Asserts the
/// display-only contract: exit 0, nothing on stderr, digits and a newline.
fn count_with(env: &Env, args: &[&str], xdg: Xdg<'_>) -> u64 {
    let out = run_raw(env, args, xdg, "", "");
    assert_eq!(
        out.status.code(),
        Some(0),
        "calculate_tokens must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "calculate_tokens must write nothing to stderr, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let digits = stdout
        .strip_suffix('\n')
        .unwrap_or_else(|| panic!("output must end with one newline: {stdout:?}"));
    assert!(
        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
        "output must be digits only (format_tokens interpolates it into awk): {stdout:?}"
    );
    assert!(
        digits == "0" || !digits.starts_with('0'),
        "output must not have leading zeros: {stdout:?}"
    );
    digits.parse().expect("u64 total")
}

fn count(env: &Env, path: &Path) -> u64 {
    count_with(env, &[path.to_str().unwrap()], Xdg::Default)
}

/// Count, then check the cache hygiene of the default cache directory.
fn count_checked(env: &Env, path: &Path) -> u64 {
    let total = count(env, path);
    assert_hygiene(&env.cache_dir());
    total
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Sum of `"KEY"[[:space:]]*:[[:space:]]*[0-9]*` over the four keys, on
/// complete lines only (bytes up to and including the last newline). An empty
/// digit run counts as 0.
fn oracle(bytes: &[u8]) -> u64 {
    let end = match bytes.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => return 0,
    };
    let mut total = 0u64;
    for line in bytes[..end].split(|&b| b == b'\n') {
        let mut i = 0;
        'scan: while i < line.len() {
            if line[i] == b'"' {
                for key in KEYS {
                    let k = key.as_bytes();
                    let after = i + 1 + k.len();
                    if line.len() > after && &line[i + 1..after] == k && line[after] == b'"' {
                        let mut j = after + 1;
                        while j < line.len() && is_space(line[j]) {
                            j += 1;
                        }
                        if j < line.len() && line[j] == b':' {
                            j += 1;
                            while j < line.len() && is_space(line[j]) {
                                j += 1;
                            }
                            let start = j;
                            while j < line.len() && line[j].is_ascii_digit() {
                                j += 1;
                            }
                            if j > start {
                                let n: u64 = std::str::from_utf8(&line[start..j])
                                    .unwrap()
                                    .parse()
                                    .unwrap();
                                total += n;
                            }
                            i = j;
                            continue 'scan;
                        }
                    }
                }
            }
            i += 1;
        }
    }
    total
}

/// One transcript line with all four keys, plus distractors that must not
/// match: a prefixed key, an escaped key inside a string and a non-usage key.
fn line(i: u64, o: u64, cr: u64, cc: u64) -> String {
    format!(
        "{{\"type\":\"assistant\",\"message\":{{\"usage\":{{\"input_tokens\":{i},\
         \"output_tokens\" : {o},\"cache_read_input_tokens\":\t{cr},\
         \"cache_creation_input_tokens\":{cc}}}}},\"total_input_tokens\":999,\
         \"text\":\"quoted \\\"input_tokens\\\":555 inside a string\",\
         \"tokens\":12345}}\n"
    )
}

fn lines(n: u64, seed: u64) -> String {
    (0..n)
        .map(|k| {
            let s = seed + k;
            line(s * 3 + 1, s * 7 + 2, s * 101, s * 13 + 5)
        })
        .collect()
}

fn append(path: &Path, data: &str) {
    let mut f = OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(data.as_bytes()).unwrap();
}

/// Non-hidden entries of a cache directory (empty when it does not exist).
fn visible_entries(dir: &Path) -> Vec<PathBuf> {
    match fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.unwrap().path())
            .filter(|p| !p.file_name().unwrap().to_str().unwrap().starts_with('.'))
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn hidden_entries(dir: &Path) -> Vec<PathBuf> {
    match fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.unwrap().path())
            .filter(|p| p.file_name().unwrap().to_str().unwrap().starts_with('.'))
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn is_digest_name(name: &str) -> bool {
    let hex = name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit());
    let cksum = match name.split_once('-') {
        Some((a, b)) => {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes().all(|c| c.is_ascii_digit())
                && b.bytes().all(|c| c.is_ascii_digit())
        }
        None => false,
    };
    hex || cksum
}

/// The single cache entry of `dir`, parsed as `(inode, offset, total)`.
fn entry(dir: &Path) -> (PathBuf, u64, u64, u64) {
    let entries = visible_entries(dir);
    assert_eq!(
        entries.len(),
        1,
        "cache dir {} must hold exactly one entry, got {entries:?}",
        dir.display()
    );
    let path = entries.into_iter().next().unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let body = text
        .strip_suffix('\n')
        .unwrap_or_else(|| panic!("cache entry must be one line ending in a newline: {text:?}"));
    let fields: Vec<&str> = body.split(' ').collect();
    assert_eq!(
        fields.len(),
        3,
        "cache entry must be 'INODE OFFSET TOTAL': {text:?}"
    );
    let nums: Vec<u64> = fields
        .iter()
        .map(|f| {
            assert!(
                !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()),
                "cache field must be decimal digits: {text:?}"
            );
            f.parse().unwrap()
        })
        .collect();
    (path, nums[0], nums[1], nums[2])
}

/// One visible entry with a digest name and three integer fields; no hidden
/// (temporary) files.
fn assert_hygiene(dir: &Path) {
    let hidden = hidden_entries(dir);
    assert!(
        hidden.is_empty(),
        "no temporary files may be left in {}: {hidden:?}",
        dir.display()
    );
    let (path, _, _, _) = entry(dir);
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(
        is_digest_name(name),
        "cache entry name must be a digest of the path, got {name:?}"
    );
}

/// Every regular file anywhere under `dir`.
fn all_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd {
            let p = e.unwrap().path();
            let ft = fs::symlink_metadata(&p).unwrap().file_type();
            if ft.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

// (a) Appends: each call's total equals the oracle.
#[test]
fn appended_lines_are_counted_incrementally() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(50, 1)).unwrap();
    assert_eq!(count_checked(&env, &t), oracle(&fs::read(&t).unwrap()));

    for k in 0..5u64 {
        append(&t, &lines(k + 1, 100 + k * 10));
        let expected = oracle(&fs::read(&t).unwrap());
        assert_eq!(count_checked(&env, &t), expected, "after append {k}");
        let (_, _, off, total) = entry(&env.cache_dir());
        assert_eq!(
            off,
            fs::metadata(&t).unwrap().len(),
            "offset after append {k}"
        );
        assert_eq!(total, expected, "cached total after append {k}");
    }
}

// (b) A second call with no new data gives the same total; the cache records
// the inode and an offset equal to the file size.
#[test]
fn repeat_call_without_new_data_is_stable() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(20, 7)).unwrap();
    let first = count_checked(&env, &t);
    let second = count_checked(&env, &t);
    assert_eq!(first, second);
    assert_eq!(first, oracle(&fs::read(&t).unwrap()));

    let meta = fs::metadata(&t).unwrap();
    let (_, ino, off, total) = entry(&env.cache_dir());
    assert_eq!(
        ino,
        meta.ino(),
        "cache INODE must be the transcript's inode"
    );
    assert_eq!(off, meta.len(), "cache OFFSET must equal the file size");
    assert_eq!(total, first, "cache TOTAL must equal the printed total");
}

// The cache is used: a valid entry whose total differs from the content is
// trusted when the inode and offset match. This shows the call does not
// recount the whole file.
#[test]
fn valid_cache_entry_is_trusted_when_nothing_changed() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(10, 3)).unwrap();
    count_checked(&env, &t);
    let (path, ino, off, _) = entry(&env.cache_dir());
    fs::write(&path, format!("{ino} {off} 424242\n")).unwrap();
    assert_eq!(count_checked(&env, &t), 424_242);

    append(&t, &line(1, 2, 3, 4));
    assert_eq!(
        count_checked(&env, &t),
        424_242 + 10,
        "only the bytes after the cached offset may be read"
    );
}

// (c) A partial last line is left out, then counted once after its newline.
#[test]
fn partial_last_line_is_counted_once_when_complete() {
    let env = Env::new();
    let t = env.transcript();
    let head = lines(3, 5);
    fs::write(&t, format!("{head}{{\"usage\":{{\"input_tokens\":500")).unwrap();
    let base = oracle(head.as_bytes());
    assert_eq!(count_checked(&env, &t), base, "partial line must not count");
    let (_, _, off, _) = entry(&env.cache_dir());
    assert_eq!(
        off,
        head.len() as u64,
        "offset must stop after the last complete line"
    );

    // The digits continue across the append: 5000, not 500 + 5000.
    append(&t, "0,\"output_tokens\":7}}\n");
    let full = fs::read(&t).unwrap();
    assert_eq!(oracle(&full), base + 5007);
    assert_eq!(count_checked(&env, &t), base + 5007);
    assert_eq!(count_checked(&env, &t), base + 5007);
}

// A transcript with no newline at all prints 0.
#[test]
fn transcript_without_newline_counts_zero() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, "{\"input_tokens\":42").unwrap();
    assert_eq!(count(&env, &t), 0);
}

// (d) Replaced by rename while the old inode stays alive: the new file's
// total only.
#[test]
fn replaced_transcript_counts_new_file_only() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(5, 1)).unwrap();
    count_checked(&env, &t);

    let keep = env.root().join("work/old.jsonl");
    fs::hard_link(&t, &keep).unwrap();
    let new = env.root().join("work/transcript.jsonl.new");
    let replacement = lines(12, 900);
    fs::write(&new, &replacement).unwrap();
    assert!(replacement.len() as u64 > fs::metadata(&keep).unwrap().len());
    fs::rename(&new, &t).unwrap();

    assert_eq!(count_checked(&env, &t), oracle(replacement.as_bytes()));
    let (_, ino, _, _) = entry(&env.cache_dir());
    assert_eq!(ino, fs::metadata(&t).unwrap().ino());
}

// (e) Truncated in place: the total of the remaining content.
#[test]
fn truncated_transcript_is_recounted() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(30, 2)).unwrap();
    let ino = fs::metadata(&t).unwrap().ino();
    count_checked(&env, &t);

    let shorter = lines(4, 700);
    fs::write(&t, &shorter).unwrap();
    assert_eq!(fs::metadata(&t).unwrap().ino(), ino, "same inode");
    assert_eq!(count_checked(&env, &t), oracle(shorter.as_bytes()));
}

fn cached_entry_path(env: &Env, t: &Path) -> PathBuf {
    count_checked(env, t);
    entry(&env.cache_dir()).0
}

// (f) Corrupt cache entries still give the correct total, and are replaced
// by a valid entry.
#[test]
fn corrupt_cache_entry_is_recounted() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(15, 4)).unwrap();
    let expected = oracle(&fs::read(&t).unwrap());
    let path = cached_entry_path(&env, &t);
    let ino = fs::metadata(&t).unwrap().ino();
    let len = fs::metadata(&t).unwrap().len();

    let payloads = [
        "garbage\n".to_string(),
        "1 2\n".to_string(),
        "1 2 3 4\n".to_string(),
        "a b c\n".to_string(),
        String::new(),
        format!("{ino} {len}\n"),
        format!("{ino} {len} 5 6\n"),
        format!("{ino} -{len} 5\n"),
        format!("{ino} {len} 1e3\n"),
    ];
    for p in payloads {
        fs::write(&path, &p).unwrap();
        assert_eq!(count_checked(&env, &t), expected, "payload {p:?}");
        let (_, i, o, tot) = entry(&env.cache_dir());
        assert_eq!((i, o, tot), (ino, len, expected), "rewritten after {p:?}");
    }
}

// A cache entry for a different inode is ignored.
#[test]
fn stale_inode_in_cache_is_recounted() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(8, 40)).unwrap();
    let path = cached_entry_path(&env, &t);
    let meta = fs::metadata(&t).unwrap();
    fs::write(&path, format!("{} {} 5\n", meta.ino() + 1, meta.len())).unwrap();
    assert_eq!(count_checked(&env, &t), oracle(&fs::read(&t).unwrap()));
}

// (f2) Arithmetic-injection payloads in each field must not run commands.
#[test]
fn cache_fields_are_never_evaluated() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(6, 11)).unwrap();
    let expected = oracle(&fs::read(&t).unwrap());
    let path = cached_entry_path(&env, &t);
    let meta = fs::metadata(&t).unwrap();
    let pwned = env.root().join("PWNED");
    let inj = format!("a[$(touch {})]", pwned.display());

    let payloads = [
        format!("{inj} {} {expected}\n", meta.len()),
        format!("{} {inj} {expected}\n", meta.ino()),
        format!("{} {} {inj}\n", meta.ino(), meta.len()),
        format!("{} 0 {inj}\n", meta.ino()),
    ];
    for p in payloads {
        fs::write(&path, &p).unwrap();
        assert_eq!(count_checked(&env, &t), expected, "payload {p:?}");
        assert!(!pwned.exists(), "cache payload {p:?} executed a command");
    }
}

// (f3) A leading-zero offset and a 20-digit value are rejected.
#[test]
fn leading_zeros_and_oversized_numbers_are_corrupt() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(9, 21)).unwrap();
    let expected = oracle(&fs::read(&t).unwrap());
    let path = cached_entry_path(&env, &t);
    let meta = fs::metadata(&t).unwrap();
    let (ino, len) = (meta.ino(), meta.len());

    let payloads = [
        // Accepted as decimal this would print 0; as octal, misread.
        format!("{ino} 0{len} 0\n"),
        format!("{ino} {len} 00\n"),
        format!("0{ino} {len} 7\n"),
        format!("{ino} {len} 99999999999999999999\n"),
        format!("{ino} 99999999999999999999 0\n"),
    ];
    for p in payloads {
        fs::write(&path, &p).unwrap();
        assert_eq!(count_checked(&env, &t), expected, "payload {p:?}");
    }
}

// (h) Unset, empty, missing, directory or zero-size path: prints 0 and
// writes no cache entry.
#[test]
fn absent_or_empty_transcript_prints_zero_and_writes_nothing() {
    let env = Env::new();
    let missing = env.root().join("work/missing.jsonl");
    let dir = env.root().join("work");
    let empty = env.root().join("work/empty.jsonl");
    fs::write(&empty, "").unwrap();

    assert_eq!(count_with(&env, &[], Xdg::Default), 0, "no argument");
    assert_eq!(count_with(&env, &[""], Xdg::Default), 0, "empty string");
    assert_eq!(count(&env, &missing), 0, "missing path");
    assert_eq!(count(&env, &dir), 0, "directory");
    assert_eq!(count(&env, &empty), 0, "zero-byte file");

    let written = all_files(&env.xdg());
    assert!(
        written.is_empty(),
        "no cache entry may be written: {written:?}"
    );
    assert!(all_files(&env.home()).is_empty());
}

// (h2) With XDG_CACHE_HOME unset, the cache lives under $HOME/.cache.
#[test]
fn home_cache_fallback_when_xdg_unset() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(7, 2)).unwrap();
    let total = count_with(&env, &[t.to_str().unwrap()], Xdg::Unset);
    assert_eq!(total, oracle(&fs::read(&t).unwrap()));
    assert_hygiene(&env.home_cache_dir());
    let (_, _, off, cached) = entry(&env.home_cache_dir());
    assert_eq!(off, fs::metadata(&t).unwrap().len());
    assert_eq!(cached, total);
    assert!(all_files(&env.xdg()).is_empty());
}

// The cache key depends on the path: two transcripts get two entries.
#[test]
fn each_transcript_path_has_its_own_entry() {
    let env = Env::new();
    let a = env.transcript();
    let b = env.root().join("work/other transcript.jsonl");
    fs::write(&a, lines(3, 1)).unwrap();
    fs::write(&b, lines(4, 50)).unwrap();
    assert_eq!(count(&env, &a), oracle(&fs::read(&a).unwrap()));
    assert_eq!(count(&env, &b), oracle(&fs::read(&b).unwrap()));
    assert_eq!(visible_entries(&env.cache_dir()).len(), 2);
    assert!(hidden_entries(&env.cache_dir()).is_empty());
    // Neither the path nor transcript text is written into the entries.
    for p in visible_entries(&env.cache_dir()) {
        let text = fs::read_to_string(&p).unwrap();
        assert!(!text.contains("transcript"), "{text:?}");
        assert!(!text.contains("tokens"), "{text:?}");
    }
    assert_eq!(count(&env, &a), oracle(&fs::read(&a).unwrap()));
}

// (j) Totals above 2^32 are exact, both on a full count and when added to a
// cached total.
#[test]
fn large_totals_are_exact() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, line(28_725_262, 0, 10_000_000_000, 0)).unwrap();
    append(&t, &line(0, 0, 10_000_000_000, 0));
    assert_eq!(count_checked(&env, &t), 20_028_725_262);

    append(&t, &line(1, 0, 5_000_000_000, 0));
    assert_eq!(count_checked(&env, &t), 25_028_725_263);
    assert_eq!(count_checked(&env, &t), oracle(&fs::read(&t).unwrap()));
}

// (k) A symlinked transcript is counted through its target, without double
// counting on repeat calls.
#[test]
fn symlinked_transcript_is_not_double_counted() {
    let env = Env::new();
    let real = env.root().join("work/real.jsonl");
    let link = env.transcript();
    fs::write(&real, lines(10, 30)).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    assert_eq!(
        count_checked(&env, &link),
        oracle(&fs::read(&real).unwrap())
    );
    assert_eq!(
        count_checked(&env, &link),
        oracle(&fs::read(&real).unwrap())
    );
    let (_, ino, off, _) = entry(&env.cache_dir());
    let meta = fs::metadata(&real).unwrap();
    assert_eq!(
        (ino, off),
        (meta.ino(), meta.len()),
        "stat must follow the link"
    );

    append(&real, &lines(2, 77));
    assert_eq!(
        count_checked(&env, &link),
        oracle(&fs::read(&real).unwrap())
    );
}

// (l) Directory 0700, entry 0600, and the caller's umask is unchanged.
#[test]
fn cache_modes_are_private_and_umask_does_not_leak() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(3, 9)).unwrap();
    let out = run_raw(
        &env,
        &[t.to_str().unwrap()],
        Xdg::Default,
        "umask 022",
        "umask",
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let got: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        got,
        vec![oracle(&fs::read(&t).unwrap()).to_string().as_str(), "0022"],
        "total then the caller's unchanged umask"
    );

    let dir_mode = fs::metadata(env.cache_dir()).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "cache directory mode");
    let (path, _, _, _) = entry(&env.cache_dir());
    let file_mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(file_mode, 0o600, "cache entry mode");
}

// The cache cannot be created (XDG_CACHE_HOME is a regular file): the total
// is still correct, with no stderr and exit 0.
#[test]
fn unusable_cache_location_still_prints_total() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(6, 60)).unwrap();
    let blocker = env.root().join("not-a-dir");
    fs::write(&blocker, "x").unwrap();
    let expected = oracle(&fs::read(&t).unwrap());
    assert_eq!(
        count_with(&env, &[t.to_str().unwrap()], Xdg::At(&blocker)),
        expected
    );
    assert_eq!(fs::read_to_string(&blocker).unwrap(), "x");
    assert_eq!(
        count_with(&env, &[t.to_str().unwrap()], Xdg::At(&blocker)),
        expected
    );
}

// A cache directory that is a symlink is not used, and nothing is written
// through it.
#[test]
fn symlinked_cache_directory_is_not_written_through() {
    let env = Env::new();
    let t = env.transcript();
    fs::write(&t, lines(6, 61)).unwrap();
    let victim = env.root().join("victim");
    fs::create_dir(&victim).unwrap();
    fs::create_dir_all(env.xdg().join("amplihack")).unwrap();
    std::os::unix::fs::symlink(&victim, env.cache_dir()).unwrap();

    let expected = oracle(&fs::read(&t).unwrap());
    assert_eq!(count(&env, &t), expected);
    assert_eq!(count(&env, &t), expected);
    assert!(
        all_files(&victim).is_empty(),
        "nothing may be written through a symlinked cache directory"
    );
}

// (i) End to end: the whole script shows the token segment computed from the
// transcript, and writes the cache under XDG_CACHE_HOME.
#[test]
fn statusline_shows_token_segment_end_to_end() {
    let env = Env::new();
    let t = env.transcript();
    // 1,234,567 tokens, displayed as 1.2M by format_tokens.
    fs::write(&t, line(1_000_000, 200_000, 34_000, 567)).unwrap();
    assert_eq!(oracle(&fs::read(&t).unwrap()), 1_234_567);

    let cwd = env.root().join("work");
    let json = format!(
        "{{\"model\":{{\"id\":\"claude-test\",\"display_name\":\"Test\"}},\
         \"cwd\":\"{}\",\"transcript_path\":\"{}\",\
         \"cost\":{{\"total_cost_usd\":0.5,\"total_duration_ms\":0}}}}",
        cwd.display(),
        t.display()
    );
    let mut child = Command::new("bash")
        .arg(script())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", env.home())
        .env("XDG_CACHE_HOME", env.xdg())
        .env("LANG", "C.UTF-8")
        .env("GIT_CEILING_DIRECTORIES", env.root())
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn statusline.sh");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(json.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("🎫 1.2M"),
        "status line must show the token segment, got {stdout:?}"
    );
    assert_hygiene(&env.cache_dir());
    let (_, _, off, total) = entry(&env.cache_dir());
    assert_eq!(off, fs::metadata(&t).unwrap().len());
    assert_eq!(total, 1_234_567);
}

// The oracle itself: the leading quote keeps "input_tokens" from matching
// inside the cache keys, and an empty digit run counts as 0.
#[test]
fn oracle_matches_the_four_key_rule() {
    assert_eq!(oracle(b"{\"cache_read_input_tokens\":5}\n"), 5);
    assert_eq!(oracle(b"{\"input_tokens\": \t7,\"output_tokens\":}\n"), 7);
    assert_eq!(oracle(b"{\"total_input_tokens\":9}\n"), 0);
    assert_eq!(oracle(b"{\"input_tokens\":1}\n{\"input_tokens\":2"), 1);
    assert_eq!(oracle(line(1, 2, 3, 4).as_bytes()), 10);
}
