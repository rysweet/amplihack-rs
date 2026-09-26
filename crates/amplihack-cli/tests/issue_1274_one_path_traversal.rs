//! Issue #1274 — one `$PATH` traversal on the launch path, and ONE place that
//! decides what an empty result means.
//!
//! `$PATH` used to be read and split independently at nine sites in this
//! crate, and they did not agree. Four walks dropped `$PATH` elements that do
//! not name an absolute directory — `launch_target::path_dirs`,
//! `binary_finder::search_path_dirs`, `docker_detector::which_docker_in`, and
//! `commands/rustyclawd.rs`'s `find_in_path` — because POSIX reads an *empty*
//! element as the current directory, and a trailing colon in a hand-edited
//! shell profile is therefore enough to let a file in a freshly cloned repo be
//! selected and executed. The other five kept them. Whether that mattered
//! depended on what each site did with the result, and a reader could not tell
//! which sites had been audited.
//!
//! The rule now lives in exactly one place — `launch_target::split_path_var`
//! and the two names over it, `path_dirs` (drop) and `env_path_dirs` (drop,
//! read from the environment) — and the sites that deliberately keep relative
//! entries say so by naming `RelativeEntries::Keep` at the call, where a
//! reviewer can see it.
//!
//! This is a ratchet over the launch-path modules issue #1274 enumerated. It
//! scans by shape rather than trusting anyone to remember: any `split_paths`
//! call reappearing in one of these files is a second traversal and a second
//! chance to disagree.

use std::path::{Path, PathBuf};

/// The modules issue #1274 named, relative to `crates/amplihack-cli/src`.
///
/// A list, and deliberately so: the walks elsewhere in this crate (`install/`,
/// `update/`, `fleet/`, `signal/`, `docker/`) are not on the launch path and
/// were not part of the consolidation. Naming the audited set is the honest
/// spelling — a scan over the whole crate would have to carry a much longer
/// exclusion list, which protects nothing.
const LAUNCH_PATH_MODULES: &[&str] = &[
    "auto_update.rs",
    "rust_trial.rs",
    "path_conflicts.rs",
    "env_builder/helpers.rs",
    "commands/rustyclawd.rs",
    "bootstrap.rs",
    "freshness.rs",
    "rust_toolchain.rs",
];

fn src(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(relative)
}

/// Lines of `text` that are not whole-line comments, as `(1-based line, text)`.
fn code_lines(text: &str) -> Vec<(usize, &str)> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .map(|(i, line)| (i + 1, line))
        .collect()
}

#[test]
fn no_launch_path_module_splits_the_path_itself() {
    let mut offenders = Vec::new();
    for module in LAUNCH_PATH_MODULES {
        let path = src(module);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for (line_no, line) in code_lines(&text) {
            if line.contains("split_paths(") {
                offenders.push(format!("{module}:{line_no}: {}", line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these launch-path sites split `$PATH` themselves instead of going \
         through `amplihack_utils::launch_target`. A second traversal is a \
         second place the empty-element rule has to be re-derived, and the \
         nine that existed before issue #1274 did not agree with each \
         other:\n  {}\n\nUse `launch_target::env_path_dirs()` to search, or \
         `launch_target::split_path_var(.., RelativeEntries::Keep)` — with a \
         comment saying why — to rebuild or describe `$PATH`.",
        offenders.join("\n  ")
    );
}

#[test]
fn every_launch_path_module_reads_the_path_only_through_the_seam() {
    // The companion to the scan above: `split_paths` gone is not enough if a
    // site went back to reading `$PATH` and hand-rolling something else.
    // `path_conflicts` and `bootstrap::prepend_path` legitimately read the raw
    // variable to hand it to `split_path_var`, so the test is that a `PATH`
    // read is accompanied by a `launch_target` call, not that it is absent.
    let mut offenders = Vec::new();
    for module in LAUNCH_PATH_MODULES {
        let path = src(module);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let reads_path = code_lines(&text)
            .iter()
            .any(|(_, line)| line.contains("(\"PATH\")"));
        if reads_path && !text.contains("launch_target::") {
            offenders.push((*module).to_string());
        }
    }

    assert!(
        offenders.is_empty(),
        "these modules read `$PATH` without going through \
         `launch_target`:\n  {}",
        offenders.join("\n  ")
    );
}

/// The sites that deliberately differ say why, at the call.
///
/// Issue #1274's third acceptance line. `RelativeEntries::Keep` — and
/// `env_path_entries`, which is that reading of the process `$PATH` — is the
/// whole escape hatch, and an escape hatch with no stated reason is just the
/// old per-site decision with a nicer name.
#[test]
fn every_keep_relative_entries_call_carries_a_reason() {
    let mut offenders = Vec::new();
    for module in LAUNCH_PATH_MODULES {
        let path = src(module);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let keeps_relative_entries =
                line.contains("RelativeEntries::Keep") || line.contains("env_path_entries(");
            if line.trim_start().starts_with("//") || !keeps_relative_entries {
                continue;
            }
            // A comment within the 20 lines above the call — enough for the
            // `let` binding and its explanation, far too little to reach the
            // previous item's doc comment.
            let start = i.saturating_sub(20);
            let has_reason = lines[start..i]
                .iter()
                .any(|l| l.trim_start().starts_with("//") || l.trim_start().starts_with("///"));
            if !has_reason {
                offenders.push(format!("{module}:{}", i + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these calls keep relative `$PATH` entries without saying why. \
         Keeping them is a real requirement for code that REBUILDS or \
         DESCRIBES `$PATH` and a security hole for code that CHOOSES A FILE \
         TO RUN, and the next reader cannot tell which one this is without \
         being told:\n  {}",
        offenders.join("\n  ")
    );
}

/// The empty case, decided once, asserted from a consumer of the seam.
///
/// Not a re-test of `launch_target`'s own unit tests — this is the CLI crate
/// asserting that the shared answer is the one it gets. Before #1274 each site
/// answered this for itself and the answers ranged over `None`, an empty
/// `Vec`, and (for the three sites that used `env::var` rather than
/// `env::var_os`) "a non-UTF-8 `$PATH` does not exist".
#[test]
fn the_seam_answers_the_empty_cases_the_same_way_for_every_caller() {
    use amplihack_utils::launch_target::{RelativeEntries, path_dirs, split_path_var};
    use std::ffi::OsStr;

    assert!(
        path_dirs(OsStr::new("")).is_empty(),
        "an empty $PATH is nowhere to look, never a fallback to cwd"
    );
    assert!(
        path_dirs(OsStr::new(":")).is_empty(),
        "a $PATH of one empty element is nowhere to look — POSIX reads the \
         empty element as the current directory and that is exactly what must \
         not be searched"
    );
    assert!(
        path_dirs(OsStr::new(".:..:relative")).is_empty(),
        "`.`, `..` and bare relative entries are the same hazard spelled out"
    );
    assert_eq!(
        path_dirs(OsStr::new("/usr/bin::/opt/bin")),
        vec![PathBuf::from("/usr/bin"), PathBuf::from("/opt/bin")],
        "a doubled colon drops out without disturbing its neighbours"
    );

    // And the deliberate other answer, for the rebuild/describe sites.
    assert_eq!(
        split_path_var(OsStr::new("/usr/bin::/opt/bin"), RelativeEntries::Keep).len(),
        3,
        "`Keep` must round-trip the user's own $PATH so rewriting it does not \
         silently delete entries from the environment of every child process"
    );
}
