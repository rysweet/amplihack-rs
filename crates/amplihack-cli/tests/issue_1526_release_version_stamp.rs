//! Issue #1526 — a source build of a release tag must report that release.
//!
//! The tag used to sit on the `main` commit, where `Cargo.toml` holds the base
//! version. A `cargo install --git` build of the commit v0.18.39 was cut from
//! therefore reported `0.18.0`, looked older than the release built from the
//! same source, and re-staged over a v0.18.39 install.
//!
//! The Auto Release `release` job now runs `scripts/stamp-release-version.sh X`
//! on the checked-out `main` SHA, commits the three manifests as
//! `chore(release): stamp vX` and tags that child commit. Nothing is pushed to
//! `main`.
//!
//! This guard covers both halves:
//!
//! - **The script**, run on temporary copies of the real manifests: only the
//!   version lines change, the stamped `Cargo.lock` entries are exactly the
//!   workspace members, a second run is a no-op, bad arguments are usage
//!   errors, and every inconsistency fails closed without writing anything.
//! - **`release.yml`**, read as text: the `release` job stamps before it tags,
//!   pushes only `refs/tags/`, never forces, never swallows a git failure, and
//!   `version-bump` recognises a tag on a stamped child of `HEAD`.

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MANIFESTS: [&str; 3] = ["Cargo.toml", "Cargo.lock", "package.json"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn script() -> PathBuf {
    repo_root().join("scripts").join("stamp-release-version.sh")
}

/// The stamp target: this build's `MAJOR.MINOR.(PATCH+99)`.
fn target_version() -> String {
    let patch: u64 = env!("CARGO_PKG_VERSION_PATCH").parse().unwrap();
    format!(
        "{}.{}.{}",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        patch + 99
    )
}

// ── Independent manifest readers (deliberately not the script's logic) ──

/// The `[section]` header in force at each line of a TOML file.
fn sections(toml: &str) -> Vec<String> {
    let mut current = String::new();
    toml.lines()
        .map(|line| {
            let t = line.trim();
            if t.starts_with('[') {
                current = t.to_string();
            }
            current.clone()
        })
        .collect()
}

fn workspace_package_version(cargo_toml: &str) -> String {
    let found: Vec<String> = cargo_toml
        .lines()
        .zip(sections(cargo_toml))
        .filter(|(_, section)| section == "[workspace.package]")
        .filter_map(|(line, _)| {
            let rest = line.trim().strip_prefix("version")?.trim_start();
            Some(rest.strip_prefix('=')?.trim().trim_matches('"').to_string())
        })
        .collect();
    assert_eq!(
        found.len(),
        1,
        "[workspace.package] must have exactly one version line; found {found:?}"
    );
    found.into_iter().next().unwrap()
}

fn package_json_version(raw: &str) -> String {
    let json: serde_json::Value = serde_json::from_str(raw).expect("package.json parses");
    json["version"]
        .as_str()
        .expect("package.json has a string version")
        .to_string()
}

/// `[package] name` of every workspace member, read from each member's own
/// manifest. Also asserts each member inherits the workspace version: the
/// stamp rewrites every path package in `Cargo.lock`, which only matches what
/// cargo produces if every member takes `version.workspace = true`.
fn workspace_member_names() -> BTreeSet<String> {
    let root = repo_root();
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let mut in_members = false;
    let mut members = Vec::new();
    for line in cargo.lines() {
        let t = line.trim();
        if t.starts_with("members") && t.contains('[') {
            in_members = true;
        }
        if in_members {
            members.extend(t.split('"').skip(1).step_by(2).map(str::to_string));
            if t.contains(']') {
                break;
            }
        }
    }
    assert!(!members.is_empty(), "could not read [workspace] members");

    members
        .iter()
        .map(|member| {
            let manifest = fs::read_to_string(root.join(member).join("Cargo.toml")).unwrap();
            let in_package: Vec<&str> = manifest
                .lines()
                .zip(sections(&manifest))
                .filter(|(_, section)| section == "[package]")
                .map(|(line, _)| line.trim())
                .collect();
            assert!(
                in_package.contains(&"version.workspace = true"),
                "{member}/Cargo.toml must inherit the workspace version, or the stamped \
                 Cargo.lock would disagree with `cargo update --workspace`"
            );
            in_package
                .iter()
                .find_map(|line| {
                    let rest = line.strip_prefix("name")?.trim_start().strip_prefix('=')?;
                    Some(rest.trim().trim_matches('"').to_string())
                })
                .unwrap_or_else(|| panic!("{member}/Cargo.toml has no [package] name"))
        })
        .collect()
}

/// For each line of a `Cargo.lock`: the `name` of the `[[package]]` block it
/// belongs to and whether that block has a `source` line.
fn lock_blocks(lock: &str) -> Vec<Option<(String, bool)>> {
    let lines: Vec<&str> = lock.lines().collect();
    let mut block_of = vec![None; lines.len()];
    let mut blocks: Vec<(String, bool)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if *line == "[[package]]" {
            blocks.push((String::new(), false));
        } else if line.is_empty() || line.starts_with('[') {
            continue;
        }
        if let Some(block) = blocks.last_mut() {
            if let Some(name) = line.strip_prefix("name = ") {
                block.0 = name.trim_matches('"').to_string();
            }
            if line.starts_with("source = ") {
                block.1 = true;
            }
            block_of[i] = Some(blocks.len() - 1);
        }
    }
    block_of
        .into_iter()
        .map(|b| b.map(|idx| blocks[idx].clone()))
        .collect()
}

/// Indexes of lines that differ; both sides must have the same line count.
fn changed_lines(file: &str, old: &str, new: &str) -> Vec<usize> {
    let (old, new): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    assert_eq!(
        old.len(),
        new.len(),
        "{file}: the stamp must rewrite lines in place, never add or remove them"
    );
    (0..old.len()).filter(|&i| old[i] != new[i]).collect()
}

// ── Running the script on scratch copies ──

fn real_manifest_bytes() -> BTreeMap<&'static str, Vec<u8>> {
    MANIFESTS
        .iter()
        .map(|name| (*name, fs::read(repo_root().join(name)).unwrap()))
        .collect()
}

/// A temp dir holding copies of the real manifests.
struct Scratch(tempfile::TempDir);

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for name in MANIFESTS {
            fs::copy(repo_root().join(name), dir.path().join(name)).unwrap();
        }
        Self(dir)
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path().join(name)).unwrap()
    }

    fn write(&self, name: &str, contents: impl AsRef<[u8]>) {
        fs::write(self.path().join(name), contents).unwrap();
    }

    /// Every entry in the scratch dir, so a stray temp file shows up too.
    /// Symlinks are recorded by target, not followed.
    fn contents(&self) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(self.path())
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                let meta = fs::symlink_metadata(&path).unwrap();
                let bytes = if meta.file_type().is_symlink() {
                    let target = fs::read_link(&path).unwrap();
                    format!("<symlink to {}>", target.display()).into_bytes()
                } else {
                    fs::read(&path).unwrap()
                };
                (name, bytes)
            })
            .collect()
    }

    /// Run the script here. The real manifests must come out untouched.
    fn stamp(&self, args: &[&str]) -> Output {
        let real_before = real_manifest_bytes();
        let out = Command::new("bash")
            .arg(script())
            .args(args)
            .current_dir(self.path())
            .env_remove("BASH_ENV")
            .env_remove("ENV")
            .output()
            .expect("spawn bash");
        assert_eq!(
            real_manifest_bytes(),
            real_before,
            "the stamp script modified the repository's own manifests; it must only \
             touch the current directory"
        );
        out
    }
}

fn describe(out: &Output) -> String {
    format!(
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn combined(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

// ── The script ──

#[test]
fn stamp_rewrites_exactly_the_version_lines() {
    let scratch = Scratch::new();
    let old_toml = scratch.read("Cargo.toml");
    let old_lock = scratch.read("Cargo.lock");
    let old_json = scratch.read("package.json");
    let old = workspace_package_version(&old_toml);
    let x = target_version();
    let members = workspace_member_names();

    let out = scratch.stamp(&[&x]);
    assert!(
        out.status.success(),
        "stamping {x} failed:\n{}",
        describe(&out)
    );
    assert!(
        combined(&out).contains(&format!(
            "stamped {x} (was {old}): Cargo.toml 1, Cargo.lock {}, package.json 1",
            members.len()
        )),
        "the script must report what it stamped:\n{}",
        describe(&out)
    );

    // Cargo.toml: one line, inside [workspace.package].
    let new_toml = scratch.read("Cargo.toml");
    let changed = changed_lines("Cargo.toml", &old_toml, &new_toml);
    assert_eq!(
        changed.len(),
        1,
        "Cargo.toml: exactly one line may change; changed {changed:?}"
    );
    let i = changed[0];
    assert_eq!(sections(&old_toml)[i], "[workspace.package]");
    assert_eq!(
        old_toml.lines().nth(i),
        Some(format!(r#"version = "{old}""#).as_str())
    );
    assert_eq!(
        new_toml.lines().nth(i),
        Some(format!(r#"version = "{x}""#).as_str())
    );
    assert_eq!(workspace_package_version(&new_toml), x);

    // package.json: one line, the top-level version.
    let new_json = scratch.read("package.json");
    let changed = changed_lines("package.json", &old_json, &new_json);
    assert_eq!(
        changed.len(),
        1,
        "package.json: exactly one line may change; changed {changed:?}"
    );
    assert_eq!(
        old_json.lines().nth(changed[0]),
        Some(format!(r#"  "version": "{old}","#).as_str())
    );
    assert_eq!(
        new_json.lines().nth(changed[0]),
        Some(format!(r#"  "version": "{x}","#).as_str())
    );
    assert_eq!(
        package_json_version(&new_json),
        workspace_package_version(&new_toml),
        "version_contract_test: package.json must match [workspace.package]"
    );

    // Cargo.lock: only version lines of path packages, and exactly the members.
    let new_lock = scratch.read("Cargo.lock");
    let blocks = lock_blocks(&old_lock);
    let (old_lines, new_lines): (Vec<&str>, Vec<&str>) =
        (old_lock.lines().collect(), new_lock.lines().collect());
    let mut stamped = BTreeSet::new();
    for i in changed_lines("Cargo.lock", &old_lock, &new_lock) {
        let (name, has_source) = blocks[i].clone().unwrap_or_else(|| {
            panic!(
                "Cargo.lock:{}: changed a line outside any [[package]]",
                i + 1
            )
        });
        assert!(
            !has_source,
            "Cargo.lock:{}: stamped `{name}`, which comes from a registry or git source",
            i + 1
        );
        assert_eq!(
            old_lines[i],
            format!(r#"version = "{old}""#),
            "Cargo.lock:{}",
            i + 1
        );
        assert_eq!(
            new_lines[i],
            format!(r#"version = "{x}""#),
            "Cargo.lock:{}",
            i + 1
        );
        assert!(
            stamped.insert(name.clone()),
            "Cargo.lock: `{name}` stamped twice"
        );
    }
    assert_eq!(
        stamped, members,
        "the stamped Cargo.lock entries must be exactly the workspace members"
    );
}

#[test]
fn stamping_twice_or_to_the_current_version_is_a_no_op() {
    let scratch = Scratch::new();
    let x = target_version();

    let first = scratch.stamp(&[&x]);
    assert!(first.status.success(), "{}", describe(&first));
    let after_first = scratch.contents();

    let second = scratch.stamp(&[&x]);
    assert_eq!(second.status.code(), Some(0), "{}", describe(&second));
    assert!(
        combined(&second).contains(&format!("already at {x}; nothing to do")),
        "{}",
        describe(&second)
    );
    assert_eq!(
        scratch.contents(),
        after_first,
        "a second run must not change a byte"
    );

    // The first release of a MAJOR.MINOR series: the manifests already say X.
    let fresh = Scratch::new();
    let before = fresh.contents();
    let old = workspace_package_version(&fresh.read("Cargo.toml"));
    let out = fresh.stamp(&[&old]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(
        combined(&out).contains(&format!("already at {old}; nothing to do")),
        "{}",
        describe(&out)
    );
    assert_eq!(
        fresh.contents(),
        before,
        "stamping the current version changes nothing"
    );
}

#[test]
fn bad_arguments_are_usage_errors_that_touch_nothing() {
    let x = target_version();
    let cases: &[&[&str]] = &[
        &[],
        &[""],
        &["v1.2.3"],
        &["1.2"],
        &["1.2.3.4"],
        &["1.2.3-rc.1"],
        &["1.2.3+build.1"],
        &["01.2.3"],
        &["1.02.3"],
        &["1.2.03"],
        &[" 1.2.3"],
        &["1.2.3 "],
        &["1.2.3\n"],
        &["1.2.3\n4.5.6"],
        &["1.2.x"],
        &["١.2.3"],
        &[&x, &x],
    ];
    for args in cases {
        let scratch = Scratch::new();
        let before = scratch.contents();
        let out = scratch.stamp(args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?} must be a usage error (exit 2):\n{}",
            describe(&out)
        );
        assert_eq!(
            scratch.contents(),
            before,
            "{args:?} must not touch any file"
        );
    }
}

#[test]
fn inconsistent_manifests_fail_closed_and_touch_nothing() {
    type Mutation = fn(&Scratch, &str);
    // A path package not at the workspace version: see
    // `unexpected_path_package_is_named_in_the_error`.
    let cases: &[(&str, Mutation)] = &[
        (
            "a dependency naming a workspace member as `<name> <version>`",
            |s, old| {
                let lock = s.read("Cargo.lock").replacen(
                    "dependencies = [\n",
                    &format!("dependencies = [\n \"amplihack-types {old}\",\n"),
                    1,
                );
                s.write("Cargo.lock", lock);
            },
        ),
        ("a second version line in [workspace.package]", |s, old| {
            let toml = s.read("Cargo.toml").replacen(
                "[workspace.package]\n",
                &format!("[workspace.package]\nversion = \"{old}\"\n"),
                1,
            );
            s.write("Cargo.toml", toml);
        }),
        ("package.json at a different version", |s, old| {
            let json = s.read("package.json").replacen(
                &format!("  \"version\": \"{old}\","),
                "  \"version\": \"0.0.1\",",
                1,
            );
            assert!(json.contains("\"0.0.1\""), "mutation did not apply");
            s.write("package.json", json);
        }),
        ("CRLF line endings", |s, _| {
            let json = s.read("package.json").replace('\n', "\r\n");
            s.write("package.json", json);
        }),
        ("a missing package.json", |s, _| {
            fs::remove_file(s.path().join("package.json")).unwrap();
        }),
        ("a missing Cargo.lock", |s, _| {
            fs::remove_file(s.path().join("Cargo.lock")).unwrap();
        }),
    ];

    let x = target_version();
    for (what, mutate) in cases {
        let scratch = Scratch::new();
        let old = workspace_package_version(&scratch.read("Cargo.toml"));
        mutate(&scratch, &old);
        let before = scratch.contents();

        let out = scratch.stamp(&[&x]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{what}: the script must fail validation (exit 1):\n{}",
            describe(&out)
        );
        assert_eq!(
            scratch.contents(),
            before,
            "{what}: validation must finish before any file is written"
        );
    }
}

#[test]
fn unexpected_path_package_is_named_in_the_error() {
    let scratch = Scratch::new();
    let old = workspace_package_version(&scratch.read("Cargo.toml"));
    let lock = scratch.read("Cargo.lock").replacen(
        &format!("name = \"amplihack-cli\"\nversion = \"{old}\"\n"),
        "name = \"amplihack-cli\"\nversion = \"0.0.1\"\n",
        1,
    );
    assert!(
        lock.contains("version = \"0.0.1\""),
        "mutation did not apply"
    );
    scratch.write("Cargo.lock", lock);
    let before = scratch.contents();

    let out = scratch.stamp(&[&target_version()]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(
        combined(&out).contains("unexpected path package amplihack-cli at 0.0.1"),
        "the error must name the package and its version:\n{}",
        describe(&out)
    );
    assert_eq!(
        scratch.contents(),
        before,
        "validation must finish before any file is written"
    );
}

/// A leftover temp file — or a symlink planted where one will be written — is
/// refused, and left exactly where it was: the script removes only what it
/// created.
#[test]
fn a_leftover_stamp_tmp_fails_closed_and_is_left_in_place() {
    let x = target_version();

    let scratch = Scratch::new();
    let planted = scratch.path().join("Cargo.toml.stamp-tmp");
    fs::write(&planted, "planted\n").unwrap();
    let before = scratch.contents();
    let out = scratch.stamp(&[&x]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(
        scratch.contents(),
        before,
        "nothing may change, the planted file included"
    );

    let scratch = Scratch::new();
    let dangling = scratch.path().join("Cargo.lock.stamp-tmp");
    std::os::unix::fs::symlink(scratch.path().join("does-not-exist"), &dangling).unwrap();
    let before = scratch.contents();
    let out = scratch.stamp(&[&x]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a dangling symlink at a temp path must be refused:\n{}",
        describe(&out)
    );
    assert_eq!(
        scratch.contents(),
        before,
        "nothing may change, the symlink included"
    );
    assert!(
        !scratch.path().join("does-not-exist").exists(),
        "the script must not write through a planted symlink"
    );
}

/// The `release` job has no Rust toolchain, and macOS/BSD tools differ from
/// GNU ones: the script must stay plain bash plus POSIX awk.
#[test]
fn script_is_portable_and_self_contained() {
    use std::os::unix::fs::PermissionsExt;

    let path = script();
    let body = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_ne!(mode & 0o111, 0, "{} must be executable", path.display());
    assert!(
        body.starts_with("#!/usr/bin/env bash\n"),
        "the script must declare bash"
    );
    assert!(
        body.contains("set -euo pipefail"),
        "the script must fail fast"
    );
    assert!(
        body.contains("LC_ALL=C"),
        "the script must pin the C locale"
    );

    let code: Vec<(usize, &str)> = body
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with('#'))
        .collect();
    for forbidden in [
        "sed -i",
        "gensub(",
        "mapfile",
        "readarray",
        "python",
        "bump-version",
        "cargo update",
        "cargo metadata",
        "cargo generate-lockfile",
        "| head",
        "grep -m",
    ] {
        let hits: Vec<String> = code
            .iter()
            .filter(|(_, l)| l.contains(forbidden))
            .map(|(n, l)| format!("  {}:{}  {}", path.display(), n + 1, l.trim()))
            .collect();
        assert!(
            hits.is_empty(),
            "the stamp script must not use `{forbidden}`:\n{}",
            hits.join("\n")
        );
    }
}

// ── release.yml ──

/// Split the workflow into `(job_name, body)` pairs. Copied from
/// `issue_1356_release_tags_last.rs`: text-based so a failure can name the
/// line a human has to edit.
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

fn release_yml() -> String {
    fs::read_to_string(repo_root().join(".github/workflows/release.yml")).expect("read release.yml")
}

fn job(name: &str) -> Vec<(usize, String)> {
    let all = jobs(&release_yml());
    let names: Vec<String> = all.iter().map(|(n, _)| n.clone()).collect();
    all.into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("release.yml has no `{name}` job (found {names:?})"))
        .1
}

/// Non-comment lines of a job.
fn code(lines: &[(usize, String)]) -> Vec<(usize, String)> {
    lines
        .iter()
        .filter(|(_, l)| !l.trim_start().starts_with('#'))
        .cloned()
        .collect()
}

fn first(lines: &[(usize, String)], needle: &str) -> Option<usize> {
    lines
        .iter()
        .find(|(_, l)| l.contains(needle))
        .map(|(n, _)| *n)
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The `run:` script of the step called `name`, one entry per line.
fn step_run_script(lines: &[(usize, String)], name: &str) -> Vec<(usize, String)> {
    let header = format!("- name: {name}");
    let start = lines
        .iter()
        .position(|(_, l)| l.trim() == header)
        .unwrap_or_else(|| panic!("release job has no step `{name}`"));
    let step_indent = indent(&lines[start].1);
    let end = lines[start + 1..]
        .iter()
        .position(|(_, l)| !l.trim().is_empty() && indent(l) <= step_indent)
        .map_or(lines.len(), |p| start + 1 + p);
    let step = &lines[start + 1..end];
    let run_at = step
        .iter()
        .position(|(_, l)| l.trim_start().starts_with("run:"))
        .unwrap_or_else(|| panic!("step `{name}` has no run:"));
    let run_indent = indent(&step[run_at].1);
    step[run_at + 1..]
        .iter()
        .take_while(|(_, l)| l.trim().is_empty() || indent(l) > run_indent)
        .cloned()
        .collect()
}

#[test]
fn release_job_stamps_a_child_commit_and_pushes_only_the_tag() {
    let release = job("release");
    let lines = code(&release);
    let at = |needle: &str| {
        first(&lines, needle).unwrap_or_else(|| panic!("the release job must contain `{needle}`"))
    };

    // Order: artifacts collected, then stamp, then tag, then publish.
    assert!(
        at("Collect assets") < at("- name: Stamp version and create tag"),
        "the tag is created only once the artifacts exist (#1356)"
    );
    assert!(
        at("stamp-release-version.sh") < at("git tag -a"),
        "the tag must go on the stamped commit, so the script runs before `git tag -a`"
    );
    assert!(
        at("- name: Stamp version and create tag") < at("softprops/action-gh-release"),
        "the tag must exist before the release is published"
    );

    // Stage by name, commit with the documented subject.
    at("git add Cargo.toml Cargo.lock package.json");
    at("git commit");
    at("chore(release): stamp");
    // Second layer: whatever is staged is checked against the three manifests
    // before the commit, so a stray path fails the job instead of shipping.
    let staged_check = at("git diff --cached --name-only");
    assert!(
        at("git add Cargo.toml Cargo.lock package.json") < staged_check
            && staged_check < at("git commit"),
        "the staged paths must be checked between `git add` and `git commit`"
    );
    at("only the three manifests may be committed");
    at("ls-remote --exit-code --tags origin");
    at("cat-file -p");
    for (n, l) in &lines {
        for forbidden in [
            "git add -A",
            "git add .",
            "git add --all",
            "git commit -a",
            "commit -am",
        ] {
            assert!(
                !l.contains(forbidden),
                "release.yml:{n}: `{forbidden}` would commit the downloaded artifacts: {}",
                l.trim()
            );
        }
    }

    // Push the tag only: never a branch, never forced.
    let pushes: Vec<&(usize, String)> = lines
        .iter()
        .filter(|(_, l)| l.contains("git push"))
        .collect();
    assert!(
        !pushes.is_empty(),
        "the release job must push the tag it creates"
    );
    for (n, l) in pushes {
        assert!(
            l.contains("refs/tags/"),
            "release.yml:{n}: push must name refs/tags/: {}",
            l.trim()
        );
        for forbidden in [
            "--force",
            " -f",
            "+refs/",
            "refs/heads",
            "HEAD",
            "main",
            "--all",
            "--mirror",
        ] {
            assert!(
                !l.contains(forbidden),
                "release.yml:{n}: the release job may push only the tag (found `{forbidden}`): {}",
                l.trim()
            );
        }
    }

    // No git failure may be swallowed.
    for (n, l) in &lines {
        if [
            "git tag",
            "git push",
            "git commit",
            "ls-remote",
            "stamp-release-version",
        ]
        .iter()
        .any(|cmd| l.contains(cmd))
        {
            assert!(
                !l.contains("|| true"),
                "release.yml:{n}: a failed tag/push must fail the job, not be ignored: {}",
                l.trim()
            );
        }
    }

    // Inputs reach the shell through env:, never by template expansion.
    let run = step_run_script(&release, "Stamp version and create tag");
    assert!(
        run.iter().any(|(_, l)| l.contains("set -euo pipefail")),
        "the stamp step must fail fast"
    );
    for (n, l) in &run {
        assert!(
            !l.contains("${{"),
            "release.yml:{n}: pass expressions through env:, not into run: {}",
            l.trim()
        );
    }

    // The release job builds nothing: no toolchain while contents:write is live.
    for (n, l) in &lines {
        assert!(
            !l.contains("rust-toolchain") && !l.trim_start().starts_with("cargo "),
            "release.yml:{n}: the release job must not run cargo: {}",
            l.trim()
        );
    }
}

#[test]
fn version_bump_recognises_a_tag_on_a_stamped_child_of_head() {
    let lines = code(&job("version-bump"));
    let at = |needle: &str| {
        first(&lines, needle)
            .unwrap_or_else(|| panic!("the version-bump job must contain `{needle}`"))
    };

    // Old-style tags (and a series' first release) sit on HEAD itself...
    at("git tag --points-at HEAD");
    // ...stamped tags sit on a child whose first parent is HEAD.
    at("^{commit}^1");
    at("chore(release): stamp");
    assert!(
        at("HEAD_SHA=$(git rev-parse HEAD)") < at("^{commit}^1"),
        "HEAD_SHA must be computed before it is compared against tag parents"
    );
    // The `sha` output stays the main SHA, so build and release check it out.
    at(r#"echo "sha=${HEAD_SHA}""#);

    assert!(
        !release_yml().contains("lightweight"),
        "release.yml still describes the release tag as lightweight; it is annotated \
         and sits on a stamped child commit"
    );
}

#[test]
fn build_job_still_pins_the_release_version() {
    let lines = code(&job("build"));
    assert!(
        first(
            &lines,
            "AMPLIHACK_RELEASE_VERSION: ${{ needs.version-bump.outputs.version }}"
        )
        .is_some(),
        "released binaries must still be built with AMPLIHACK_RELEASE_VERSION"
    );
    assert!(
        first(&lines, "stamp-release-version.sh").is_none(),
        "the build job is not stamped: that would bust sccache on every target"
    );
}
