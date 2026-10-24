//! Source guard for issue #1634: the shared-session code never overwrites,
//! copies by path, or deletes anything but its two reserved names. Every move
//! into `~/.claude` goes through `renameat2(RENAME_NOREPLACE)`, the link swap
//! through `RENAME_EXCHANGE` or the checked fallback, and new files through
//! `O_EXCL`. Register from `shared_dirs/mod.rs` as
//! `#[cfg(test)] mod tests_source_scan;`.

use std::fs;
use std::path::{Path, PathBuf};

/// Calls that replace a destination, follow a path, or delete a tree.
const FORBIDDEN: &[&str] = &[
    "fs::rename(",
    "std::fs::rename(",
    "fs::copy(",
    "io::copy(",
    "fs::remove_dir_all(",
    "remove_dir_all(",
    "fs::remove_dir(",
    "fs::remove_file(",
    "fs::hard_link(",
    "fs::write(",
    "File::create(",
    "fs::set_permissions(",
    "canonicalize_with_missing",
];

/// Login files may be named only to be recognised and skipped.
const LOGIN_NAMES: &[&str] = &[".credentials", ".claude.json"];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/shared_dirs")
}

fn sources() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir())
        .expect("shared_dirs/ exists")
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "rs")
                && !p
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("tests_")
        })
        .collect();
    v.sort();
    v
}

/// Lines of `src` outside `#[cfg(test)]` modules and comments.
fn code_lines(src: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut skip_depth: Option<i32> = None;
    let mut depth = 0i32;
    let mut pending_test = false;
    for (i, line) in src.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with("#[cfg(test)]") || t.starts_with("#[cfg(all(test") {
            pending_test = true;
            continue;
        }
        let opens = line.matches('{').count() as i32;
        let closes = line.matches('}').count() as i32;
        if pending_test && opens > 0 {
            skip_depth.get_or_insert(depth);
            pending_test = false;
        }
        depth += opens - closes;
        if let Some(d) = skip_depth {
            if depth <= d {
                skip_depth = None;
            }
            continue;
        }
        if t.starts_with("//") {
            continue;
        }
        out.push((i + 1, line));
    }
    out
}

#[test]
fn shared_dirs_has_the_planned_files() {
    let names: Vec<String> = sources()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for want in ["mod.rs", "merge.rs"] {
        assert!(
            names.contains(&want.to_string()),
            "missing {want}: {names:?}"
        );
    }
}

#[test]
fn every_source_file_stays_under_400_lines() {
    for p in sources() {
        let n = fs::read_to_string(&p).unwrap().lines().count();
        assert!(n <= 400, "{} has {n} lines", p.display());
    }
}

#[test]
fn no_replacing_copying_or_deleting_call_appears() {
    let mut hits = Vec::new();
    for p in sources() {
        let src = fs::read_to_string(&p).unwrap();
        for (n, line) in code_lines(&src) {
            for f in FORBIDDEN {
                if line.contains(f) {
                    hits.push(format!("{}:{n}: {f}", p.display()));
                }
            }
        }
    }
    assert!(hits.is_empty(), "forbidden calls:\n{}", hits.join("\n"));
}

#[test]
fn no_rename_without_noreplace_or_exchange() {
    let mut hits = Vec::new();
    for p in sources() {
        let src = fs::read_to_string(&p).unwrap();
        for (n, line) in code_lines(&src) {
            if line.contains("libc::renameat2(")
                && !(line.contains("RENAME_NOREPLACE") || line.contains("RENAME_EXCHANGE"))
            {
                // The flags argument may be on a following line; require a
                // named flag within the call instead of a bare `0`.
                let rest: String = src.lines().skip(n).take(6).collect();
                if !(rest.contains("RENAME_NOREPLACE") || rest.contains("RENAME_EXCHANGE")) {
                    hits.push(format!("{}:{n}", p.display()));
                }
            }
            if line.contains("libc::rename(") || line.contains("libc::renameat(") {
                hits.push(format!("{}:{n}: plain rename", p.display()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "renames that may replace:\n{}",
        hits.join("\n")
    );
}

#[test]
fn login_names_appear_only_in_the_credential_filter() {
    let mut hits = Vec::new();
    for p in sources() {
        let src = fs::read_to_string(&p).unwrap();
        let mut in_filter = false;
        for (n, line) in code_lines(&src) {
            if line.contains("fn is_credential_name") {
                in_filter = true;
            } else if in_filter && line.starts_with('}') {
                in_filter = false;
                continue;
            }
            if in_filter {
                continue;
            }
            for name in LOGIN_NAMES {
                if line.contains(name) {
                    hits.push(format!("{}:{n}: {name}", p.display()));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "login names outside is_credential_name:\n{}",
        hits.join("\n")
    );
}

#[test]
fn nothing_is_printed_to_stdout() {
    let stdout = regex::Regex::new(r"(^|[^e])print(ln)?!\(|stdout\(\)").unwrap();
    for p in sources() {
        let src = fs::read_to_string(&p).unwrap();
        for (n, line) in code_lines(&src) {
            assert!(
                !stdout.is_match(line),
                "{}:{n}: stdout output (all messages go to stderr)",
                p.display()
            );
        }
    }
}
