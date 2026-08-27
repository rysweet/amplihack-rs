//! Issue #1356 — a release must not publish its version tag before the
//! artifacts exist.
//!
//! `release.yml` sets `cancel-in-progress: true`, so a push to `main` cancels
//! whatever release is currently running. While the `version-bump` job pushed
//! the tag before `build` started, a cancelled run left a public tag with
//! nothing behind it. The next run reads `git tag --list` as its source of
//! truth, sees the stranded tag, and bumps past it — so the version is burned
//! and never retried, and nothing reports a failure because the cancelled run
//! is `cancelled`, not `failure`.
//!
//! Eleven merges in one night produced eight stranded tags (v0.18.6 through
//! v0.18.13) and left `amplihack update` reporting "already at the latest
//! version" against a `main` it would never fetch.
//!
//! The rule this guard encodes: a cancelled run must leave nothing behind.
//! Reserving a version number in a job output costs nothing if the run dies;
//! publishing it does.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

/// Split the workflow into `(job_name, body)` pairs.
///
/// Text-based rather than YAML-parsed so a failure can name the line a human
/// has to edit, and so the guard does not depend on a YAML library.
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
        // A job key is exactly two spaces of indent then `name:`.
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

#[test]
fn no_job_pushes_a_version_tag_before_the_artifacts_exist() {
    let root = repo_root();
    let path = root.join(".github/workflows/release.yml");
    let body = std::fs::read_to_string(&path).expect("read release.yml");

    let all = jobs(&body);
    assert!(
        all.iter().any(|(n, _)| n == "release"),
        "could not parse release.yml's jobs; the guard would pass vacuously \
         (found: {:?})",
        all.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );

    // Only the job that also uploads the artifacts may create the tag.
    let publisher = "release";
    let mut findings = Vec::new();
    for (name, lines) in &all {
        if name == publisher {
            continue;
        }
        for (lineno, line) in lines {
            let t = line.trim();
            if t.starts_with('#') {
                continue;
            }
            if t.contains("git push") && t.contains("origin") && t.contains('v') {
                findings.push(format!(
                    "  .github/workflows/release.yml:{lineno}  job '{name}' pushes a tag\n      {t}"
                ));
            }
        }
    }

    assert!(
        findings.is_empty(),
        "a version tag is pushed before the artifacts exist:\n{}\n\n\
         release.yml sets cancel-in-progress, so a superseded run would leave \
         this tag public with no release behind it, and the next run bumps past \
         it. Only the '{publisher}' job — which has the built artifacts — may \
         create the tag (issue #1356).",
        findings.join("\n")
    );
}

#[test]
fn the_publishing_job_still_creates_the_tag() {
    let root = repo_root();
    let body = std::fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("read release.yml");
    let release_body: String = jobs(&body)
        .into_iter()
        .find(|(n, _)| n == "release")
        .map(|(_, lines)| {
            lines
                .into_iter()
                .map(|(_, l)| l)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .expect("release job present");

    assert!(
        release_body.contains("git tag"),
        "removing the early push must not remove tagging altogether — the \
         release job has to create the tag itself, or every release lands \
         untagged"
    );
    assert!(
        release_body.contains("git push") && release_body.contains("origin"),
        "the release job must push the tag it creates"
    );
}
