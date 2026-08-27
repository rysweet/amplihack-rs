//! Issue #1358 / #1340 — `RUSTC_WRAPPER: sccache` is job-wide, so the wrapper
//! must be installed before the first cargo invocation in that job.
//!
//! `env:` at job level applies to every step, including ones that run before
//! `Setup sccache`. Cargo then tries to exec a binary that is not on PATH yet
//! and dies with:
//!
//! ```text
//! error: could not execute process `sccache` (never executed)
//! ```
//!
//! That is not a cache miss — it is a hard failure of the build. When it landed
//! in `ci.yml` it turned `Lint & Format` red on `main` and blocked every open
//! PR until #1341 reordered the steps. #1358 adds the same job-wide wrapper to
//! `release.yml`, which is the same trap in a workflow nobody runs on a PR — a
//! release would be the first thing to notice.
//!
//! The guard is file-agnostic on purpose: it checks every workflow that opts
//! into the wrapper, so the next one to adopt sccache is covered on arrival.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

/// Split a workflow into `(job_name, [(lineno, line)])`.
///
/// Text-based rather than YAML-parsed so a failure can name the line a human
/// has to edit, and so the guard needs no YAML dependency.
fn jobs(body: &str) -> Vec<(String, Vec<(usize, String)>)> {
    let mut out: Vec<(String, Vec<(usize, String)>)> = Vec::new();
    let mut in_jobs = false;
    for (n, line) in body.lines().enumerate() {
        if line.starts_with("jobs:") {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        if let Some(rest) = line.strip_prefix("  ")
            && !rest.starts_with(' ')
            && !rest.starts_with('#')
            && let Some(name) = rest.split(':').next()
            && !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            out.push((name.to_string(), Vec::new()));
            continue;
        }
        if let Some(last) = out.last_mut() {
            last.1.push((n + 1, line.to_string()));
        }
    }
    out
}

/// Does this line install sccache?
///
/// Matches the action rather than the step's display name so renaming the step
/// does not silently disable the check.
fn installs_sccache(line: &str) -> bool {
    line.contains("sccache-action")
}

/// Does this line invoke cargo?
///
/// Cargo must be the *command*, not merely a word on the line. `echo "Run:
/// cargo update ..."` is a remediation message, and matching it would fail
/// ci.yml, which is correctly ordered. So each shell segment is reduced to its
/// first word, after stripping the YAML `- `/`run: ` prefixes and any leading
/// `VAR=value` assignments.
///
/// `uses:` lines are excluded: a marketplace action named `...cargo...` is not
/// a cargo invocation in this job's shell.
fn invokes_cargo(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with('#') || t.starts_with("uses:") || t.starts_with("- uses:") {
        return false;
    }
    // A `run:` step body may hold several commands on one line.
    t.split("&&")
        .flat_map(|s| s.split("||"))
        .flat_map(|s| s.split(';'))
        .flat_map(|s| s.split('|'))
        .any(|segment| {
            let mut s = segment.trim();
            for prefix in ["- ", "run: ", "- run: "] {
                s = s.strip_prefix(prefix).unwrap_or(s).trim_start();
            }
            // Skip leading environment assignments: `RUSTFLAGS=-x cargo build`.
            let mut words = s.split_whitespace().skip_while(|w| {
                w.contains('=')
                    && !w.starts_with('-')
                    && w.split('=').next().is_some_and(|k| {
                        !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                    })
            });
            words.next() == Some("cargo")
        })
}

fn workflow_files(root: &Path) -> Vec<PathBuf> {
    let dir = root.join(".github/workflows");
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yml" || x == "yaml"))
        .collect();
    out.sort();
    out
}

#[test]
fn sccache_is_installed_before_the_first_cargo_invocation() {
    let root = repo_root();
    let files = workflow_files(&root);
    assert!(
        !files.is_empty(),
        "no workflow files found; the guard would pass vacuously"
    );

    let mut checked = 0usize;
    let mut findings = Vec::new();

    for path in &files {
        let body = std::fs::read_to_string(path).expect("read workflow");
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path.as_path())
            .display()
            .to_string();

        for (name, lines) in jobs(&body) {
            // Only jobs that set the wrapper job-wide are at risk. A step-level
            // `env:` applies to that one step, which cannot precede itself.
            let wrapper_job_wide = lines
                .iter()
                .any(|(_, l)| l.trim().starts_with("RUSTC_WRAPPER:") && l.contains("sccache"));
            if !wrapper_job_wide {
                continue;
            }
            checked += 1;

            let install = lines.iter().find(|(_, l)| installs_sccache(l));
            let first_cargo = lines.iter().find(|(_, l)| invokes_cargo(l));

            match (install, first_cargo) {
                (None, Some((cargo_line, _))) => findings.push(format!(
                    "  {rel}  job '{name}' sets RUSTC_WRAPPER: sccache but never installs \
                     sccache; the cargo at line {cargo_line} cannot run"
                )),
                (Some((install_line, _)), Some((cargo_line, cargo_src)))
                    if install_line > cargo_line =>
                {
                    findings.push(format!(
                        "  {rel}  job '{name}' installs sccache at line {install_line}, after \
                         cargo runs at line {cargo_line}:\n      {}",
                        cargo_src.trim()
                    ))
                }
                _ => {}
            }
        }
    }

    assert!(
        checked > 0,
        "no job sets RUSTC_WRAPPER: sccache; the guard would pass vacuously. If sccache \
         was removed deliberately, delete this file too."
    );

    assert!(
        findings.is_empty(),
        "cargo runs before sccache is installed:\n{}\n\n\
         `env:` at job level applies to every step, so cargo fails outright with \
         \"could not execute process `sccache`\" rather than merely missing the cache. \
         Move the sccache install above the first cargo step (issues #1340, #1358).",
        findings.join("\n")
    );
}
