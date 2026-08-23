//! crates/amplihack-utils/tests/no_global_path_mutation.rs
//!
//! Contract: no unit test in this crate may mutate the process-global `$PATH`.
//!
//! Why this is a hard rule rather than a style preference:
//!
//! libtest runs this crate's unit tests on parallel threads of ONE process.
//! Several of them spawn helpers by bare name — `artifact_guard` and
//! `worktree` shell out to `git` both from their fixtures and from the
//! production code under test. `$PATH` is process-global, so a test that
//! points it at a nonexistent directory makes every concurrent bare-name
//! spawn fail with `ENOENT` ("No such file or directory (os error 2)").
//!
//! That is exactly what happened: `find_falls_back_to_npm_global_when_not_on_path`
//! set `PATH=/nonexistent-just-for-this-test`, and 15 unrelated
//! `artifact_guard` tests failed in `cargo test --workspace` while passing
//! when run alone. It reads as flakiness and is not — it is a deterministic
//! race that fires whenever the scheduler overlaps the two.
//!
//! The `env_lock` in `test_support` does NOT make this safe. It serialises env
//! *mutators* against each other; the bare-name spawners are *readers* and
//! never take it.
//!
//! If a test genuinely must exercise `$PATH` resolution, do it without
//! mutating the process: pick a needle name that cannot exist on the real
//! `$PATH` (the fallback test's approach), or thread an explicit search path
//! through the function under test.

use std::path::{Path, PathBuf};

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_unit_test_in_this_crate_clobbers_the_process_path() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    assert!(
        !files.is_empty(),
        "found no sources under {}",
        src.display()
    );

    let mut offenders = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
        for (i, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            // The doc-comment references in this very contract are prose, not code.
            if trimmed.starts_with("//") {
                continue;
            }
            if line.contains(r#"set_var("PATH""#) || line.contains(r#"remove_var("PATH""#) {
                offenders.push(format!("{}:{}", file.display(), i + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these lines mutate the process-global $PATH, which breaks concurrent \
         bare-name subprocess spawns (git) in sibling tests:\n  {}\n\nSee this \
         file's module docs for the supported alternatives.",
        offenders.join("\n  ")
    );
}

/// F-S5 ratchet — every `$PATH` walk in this crate drops relative entries.
///
/// The previous version of this scan named ONE file, `launch_target.rs`, and
/// that is exactly how F-S5 happened: `binary_finder::search_path_dirs` is a
/// second, independent `$PATH` → directory funnel in the same crate, with its
/// own callers (`bootstrap.rs` reaches it), and it had a bare `split_paths`
/// walk. The fix landed on one seam and the reviewer found the other still
/// open. `docker_detector::which_docker_in` was a third.
///
/// So this scans by *shape* over every source in the crate rather than by
/// filename: wherever `split_paths` appears outside a comment, an
/// `is_absolute` test must appear in the expression that follows it. A ratchet
/// that lists filenames only ever protects the filenames someone remembered.
///
/// The window is deliberately loose — it proves the filter is adjacent, not
/// that it is correct — because the behavioural cases are pinned elsewhere
/// (`launch_target`'s `path_dirs` tests, `launch_target_health_gate.rs`). What
/// it catches is the walk that has *no* filter at all, which is the only way
/// this defect has ever actually appeared.
#[test]
fn every_path_walk_in_this_crate_drops_relative_entries() {
    /// How far past a `split_paths` call an `is_absolute` test may sit and
    /// still count. Wide enough for `.filter(|dir| dir.is_absolute())` on the
    /// following line or two; far too narrow to reach the next statement.
    const WINDOW: usize = 200;

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);

    let mut offenders = Vec::new();
    let mut sites = 0usize;
    for file in &files {
        let text = std::fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
        // Whole-line comments only, matching the scan above: the prose in this
        // crate's doc comments discusses `split_paths` at length.
        let code: Vec<&str> = text
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("//") {
                    ""
                } else {
                    line
                }
            })
            .collect();
        for (i, line) in code.iter().enumerate() {
            if !line.contains("split_paths(") {
                continue;
            }
            sites += 1;
            let window: String = code[i..].join("\n").chars().take(WINDOW).collect();
            if !window.contains("is_absolute") {
                offenders.push(format!("{}:{}: {}", file.display(), i + 1, line.trim()));
            }
        }
    }

    assert!(
        sites >= 3,
        "expected at least the three known $PATH walks in this crate \
         (launch_target::path_dirs, binary_finder::search_path_dirs, \
         docker_detector::which_docker_in); found {sites}. If a walk moved, \
         follow it — do not weaken the scan."
    );
    assert!(
        offenders.is_empty(),
        "these $PATH walks do not drop relative entries. An empty element is \
         POSIX for the current directory, so the joined candidate is a bare \
         name that is stat'd against amplihack's cwd and executed from \
         wherever execvp finds it:\n  {}",
        offenders.join("\n  ")
    );
}

/// F-S2 ratchet — the `$PATH` → candidate-directory seam keeps its filter.
///
/// The behavioural cases live in `launch_target`'s own test module, against
/// the pure `path_dirs` seam, precisely because this file forbids the
/// alternative: pinning it end-to-end would mean setting `PATH` on the
/// process, and the module docs above explain what that does to the fifteen
/// unrelated tests that spawn `git` by bare name.
///
/// A pure seam can be tested and can also be quietly bypassed — someone
/// reintroducing a direct `split_paths` walk in `candidate_paths` would pass
/// every `path_dirs` test while restoring the bug. This scan is the guard
/// against that: the seam must exist, must filter on absoluteness, and must be
/// the only place `candidate_paths` learns about `$PATH`.
#[test]
fn the_path_to_candidate_directory_seam_still_filters_relative_entries() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("launch_target.rs");
    let text =
        std::fs::read_to_string(&src).unwrap_or_else(|e| panic!("read {}: {e}", src.display()));

    let seam = fn_body(&text, "fn path_dirs(")
        .expect("launch_target must route $PATH through a pure `path_dirs` seam");
    assert!(
        seam.contains("is_absolute"),
        "`path_dirs` must drop relative and empty $PATH entries: an empty \
         element is POSIX for the current directory, and the resulting bare \
         candidate is resolved by execvp from wherever amplihack happens to \
         be.\nGot:\n{seam}"
    );

    let candidates =
        fn_body(&text, "fn candidate_paths(").expect("launch_target must define candidate_paths");
    assert!(
        !candidates.contains("split_paths"),
        "`candidate_paths` must obtain its directories from `path_dirs`, not \
         by walking $PATH itself — a second walk reintroduces the relative \
         candidate the seam exists to remove.\nGot:\n{candidates}"
    );
}

/// Extract a function body by brace matching from its signature prefix.
fn fn_body(text: &str, signature: &str) -> Option<String> {
    let start = text.find(signature)?;
    let open = text[start..].find('{')? + start;
    let mut depth = 0usize;
    for (offset, ch) in text[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[open..=open + offset].to_string());
                }
            }
            _ => {}
        }
    }
    None
}
