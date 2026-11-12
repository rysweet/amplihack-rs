//! #1888: `amplihack orch helper select-merge-method`, with a fake `gh` and
//! real git history, so the merge-base rules are checked against git itself.
//!
//! CLI contract: select-merge-method --pr-url URL --repo-root PATH
//! [--base SHA] [--head SHA] [--merge-method VALUE]. It prints one JSON object:
//! the selection, or `{"error": reason}` with exit 1 (79 on a gh policy refusal).
//!
//! History in every fixture (the base moves on after the branch point):
//!
//!   M0 (merge base) --- HEAD (the pull request)
//!     \
//!      M1 (base tip)
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

const URL: &str = "https://github.com/owner/project/pull/7";
const CONFIG: &str = ".amplihack/auto-drive.toml";

struct Fixture {
    tmp: TempDir,
    m0: String,
    base: String,
    head: String,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(repo)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A file to write before a commit: `None` content deletes the path.
type Change<'a> = (&'a str, Option<&'a str>);

impl Fixture {
    /// `at_m0`: files committed at the merge base. `on_head` and `on_base`:
    /// what the pull request and the moved-on base change after it.
    fn with(at_m0: &[Change], on_head: &[Change], on_base: &[Change]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        for dir in ["repo", "bin", "home", "tmp"] {
            fs::create_dir_all(tmp.path().join(dir)).unwrap();
        }
        crate::support::exec_stub::write_executable(
            &tmp.path().join("bin/gh"),
            r#"#!/bin/bash
printf '%s\n' "$*" >> "$TEST_CALLS"
[ "$*" = "api --hostname github.com repos/owner/project" ] || exit 91
cat "$TEST_RESPONSE"
exit "$TEST_API_RC"
"#,
            0o755,
        );
        git(&repo, &["init", "-q", "-b", "main"]);
        let commit = |changes: &[Change], msg: &str| {
            for (path, content) in changes {
                let p = repo.join(path);
                match content {
                    Some(text) => {
                        fs::create_dir_all(p.parent().unwrap()).unwrap();
                        fs::write(&p, text).unwrap();
                    }
                    None => {
                        let _ = fs::remove_file(&p);
                    }
                }
            }
            git(&repo, &["add", "-A"]);
            git(&repo, &["commit", "-q", "--allow-empty", "-m", msg]);
            git(&repo, &["rev-parse", "HEAD"])
        };
        let mut readme: Vec<Change> = vec![("README.md", Some("readme\n"))];
        readme.extend_from_slice(at_m0);
        let m0 = commit(&readme, "m0");
        let base = commit(on_base, "base moves on");
        git(&repo, &["checkout", "-q", "-b", "pr", &m0]);
        let head = commit(on_head, "pull request");
        Self {
            tmp,
            m0,
            base,
            head,
        }
    }
    fn plain() -> Self {
        Self::with(
            &[],
            &[("src.rs", Some("v1\n"))],
            &[("other.txt", Some("base\n"))],
        )
    }
    fn repo(&self) -> PathBuf {
        self.tmp.path().join("repo")
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.tmp.path().join("calls")).unwrap_or_default()
    }
    fn run_with(
        &self,
        url: &str,
        base: &str,
        explicit: Option<&str>,
        body: &str,
        rc: u8,
    ) -> Output {
        let root = self.tmp.path();
        fs::write(root.join("response"), body).unwrap();
        // `isolated` clears the environment except PATH, TMPDIR and the like;
        // everything this selector may read is then set here.
        let mut cmd =
            crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
        cmd.env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("home/.config"))
        .env("GH_CONFIG_DIR", root.join("home/gh"))
        .env("AMPLIHACK_HOME", root.join("home/amplihack"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Neither may choose the repository or the checkout.
        .env("GH_REPO", "owner-b/repo-b")
        .env("GIT_DIR", root.join("home"))
        .env("TEST_CALLS", root.join("calls"))
        .env("TEST_RESPONSE", root.join("response"))
        .env("TEST_API_RC", rc.to_string())
        .current_dir(root.join("home"))
        .args(["orch", "helper", "select-merge-method", "--pr-url", url])
        .arg("--repo-root")
        .arg(self.repo())
        .args(["--base", base, "--head", &self.head]);
        if let Some(value) = explicit {
            cmd.args(["--merge-method", value]);
        }
        cmd.output().unwrap()
    }
    fn select(&self, explicit: Option<&str>, allowed: [bool; 3]) -> Output {
        self.run_with(URL, &self.base, explicit, &permissions(allowed), 0)
    }
}

fn permissions(a: [bool; 3]) -> String {
    json!({"allow_merge_commit":a[0],"allow_squash_merge":a[1],"allow_rebase_merge":a[2]})
        .to_string()
}

fn success(out: Output, method: &str, source: &str) -> Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    // from_slice rejects trailing objects and prose: the output is one object.
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["method"], method, "{value}");
    assert_eq!(value["source"], source, "{value}");
    assert_eq!(value["repository"], "owner/project");
    let rationale = value["rationale"].as_str().expect("string rationale");
    assert!(rationale.contains(method), "{rationale}");
    value
}

/// A refusal: non-zero exit and one `{"error": reason}` object containing `why`.
fn rejected(out: Output, why: &str) -> String {
    assert!(
        !out.status.success(),
        "selection unexpectedly succeeded: {out:?}"
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "refusal is not one JSON object ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    let reason = value["error"]
        .as_str()
        .expect("error is a string")
        .to_string();
    assert!(value.as_object().unwrap().len() == 1, "{value}");
    assert!(reason.contains(why), "reason {reason:?} lacks {why:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains(&reason));
    reason
}

#[test]
fn defaults_cover_every_permission_combination() {
    let f = Fixture::plain();
    for mask in 0..8 {
        let a = [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0];
        let out = f.select(None, a);
        if mask == 0 {
            rejected(out, "allows no merge method");
            continue;
        }
        let want = if a[1] {
            "squash"
        } else if a[0] {
            "merge"
        } else {
            "rebase"
        };
        let value = success(out, want, "default");
        // The file was looked for, at the merge base, and was absent.
        assert_eq!(value["config_commit"], f.m0.as_str());
    }
}

#[test]
fn the_file_at_the_merge_base_is_the_repository_preference() {
    let f = Fixture::with(
        &[(
            CONFIG,
            Some("# Simard requires merge commits\nmerge_method = \"merge\"\n"),
        )],
        &[("src.rs", Some("v1\n"))],
        &[],
    );
    let value = success(f.select(None, [true; 3]), "merge", "repository");
    assert_eq!(value["config_commit"], f.m0.as_str());
    assert!(value["rationale"].as_str().unwrap().contains(&f.m0));
    // Empty context is no context.
    success(f.select(Some(""), [true; 3]), "merge", "repository");
}

#[test]
fn explicit_context_wins_and_the_file_is_not_read() {
    let f = Fixture::with(&[(CONFIG, Some("merge_method = \"rebase\"\n"))], &[], &[]);
    let value = success(f.select(Some("merge"), [true; 3]), "merge", "explicit");
    assert_eq!(value["config_commit"], Value::Null);
    // A disallowed lower-priority preference does not defeat the override.
    success(
        f.select(Some("squash"), [false, true, false]),
        "squash",
        "explicit",
    );
    // Not read means not validated either: a broken file does not block it.
    let broken = Fixture::with(&[(CONFIG, Some("merge_method = rebase\n"))], &[], &[]);
    success(broken.select(Some("merge"), [true; 3]), "merge", "explicit");
    // And an explicit method needs no base, so an unfetched base is fine.
    let out = broken.run_with(URL, "", Some("merge"), &permissions([true; 3]), 0);
    success(out, "merge", "explicit");
}

#[test]
fn a_pull_request_never_chooses_its_own_merge_method() {
    let changes: [&[Change]; 4] = [
        &[(CONFIG, Some("merge_method = \"rebase\"\n"))], // modify
        &[(CONFIG, None)],                                // delete
        &[(CONFIG, Some("merge_method = \"merge\"\n"))],  // unchanged content: a no-op
        &[(".amplihack/qa.toml", Some("command = \"make\"\n"))],
    ];
    for (i, on_head) in changes.iter().enumerate() {
        let f = Fixture::with(
            &[(CONFIG, Some("merge_method = \"merge\"\n"))],
            on_head,
            &[],
        );
        let out = f.select(None, [true; 3]);
        if i >= 2 {
            // Rewriting the same bytes or touching another .amplihack file is not a change.
            success(out, "merge", "repository");
        } else {
            rejected(out, "changed-in-pr");
            assert_eq!(f.calls(), "", "a refused file never reaches gh");
        }
    }
    // Adding the file in the pull request is a change too ...
    let add = Fixture::with(&[], &[(CONFIG, Some("merge_method = \"merge\"\n"))], &[]);
    rejected(add.select(None, [true; 3]), "changed-in-pr");
    // ... which an operator merges with an explicit method.
    success(add.select(Some("merge"), [true; 3]), "merge", "explicit");
}

#[test]
fn the_working_tree_and_the_base_tip_are_never_read() {
    let f = Fixture::with(
        &[],
        &[("src.rs", Some("v1\n"))],
        &[(CONFIG, Some("merge_method = \"merge\"\n"))],
    );
    // The base added the file after the branch point: not at the merge base.
    let value = success(f.select(None, [true; 3]), "squash", "default");
    assert_eq!(value["config_commit"], f.m0.as_str());
    // An untracked copy in the checkout is ignored.
    fs::create_dir_all(f.repo().join(".amplihack")).unwrap();
    fs::write(f.repo().join(CONFIG), "merge_method = \"rebase\"\n").unwrap();
    success(f.select(None, [true; 3]), "squash", "default");
    // So is an uncommitted edit of a committed one.
    let g = Fixture::with(&[(CONFIG, Some("merge_method = \"merge\"\n"))], &[], &[]);
    fs::write(g.repo().join(CONFIG), "merge_method = \"rebase\"\n").unwrap();
    success(g.select(None, [true; 3]), "merge", "repository");
}

#[test]
fn an_invalid_file_is_refused_with_its_problem() {
    for (text, problem) in [
        ("merge_method: merge\n", "syntax"),
        ("merge_method = merge\n", "syntax"),
        ("merge_method = \"merge\"\r\n", "syntax"),
        ("merge_method = \"auto\"\n", "invalid-merge-method"),
        ("merge_method = \"MERGE\"\n", "invalid-merge-method"),
        ("command = \"cargo test\"\n", "unknown-key"),
        (
            "merge_method = \"merge\"\nmerge_method = \"squash\"\n",
            "duplicate-key",
        ),
    ] {
        let f = Fixture::with(&[(CONFIG, Some(text))], &[], &[]);
        let reason = rejected(f.select(None, [true; 3]), problem);
        assert!(
            reason.contains(&f.m0),
            "the reason names the commit read: {reason}"
        );
        assert!(
            !reason.contains("cargo test"),
            "no file content in the reason"
        );
    }
    let big = "# x\n".repeat(1100);
    let f = Fixture::with(&[(CONFIG, Some(&big))], &[], &[]);
    rejected(f.select(None, [true; 3]), "too-large");
}

#[test]
fn symlinks_and_odd_entries_at_the_merge_base_are_refused() {
    use std::os::unix::fs::symlink;
    // The fixture writes files; symlinks are added to the merge base by hand.
    let make = |link: &str, target: &str| {
        let f = Fixture::with(
            &[("policy.toml", Some("merge_method = \"merge\"\n"))],
            &[],
            &[],
        );
        let repo = f.repo();
        git(&repo, &["checkout", "-q", "-B", "m0x", &f.m0]);
        fs::create_dir_all(repo.join(link).parent().unwrap()).unwrap();
        symlink(target, repo.join(link)).unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "link"]);
        let m0 = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["checkout", "-q", "-B", "pr2", &m0]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "pr"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);
        (f, m0, head)
    };
    for (link, target) in [(CONFIG, "../policy.toml"), (".amplihack", "elsewhere")] {
        let (mut f, m0, head) = make(link, target);
        f.head = head;
        let out = f.run_with(URL, &m0, None, &permissions([true; 3]), 0);
        rejected(out, "symlink");
    }
    // A directory where the file should be.
    let f = Fixture::with(&[(".amplihack/auto-drive.toml/x", Some("y\n"))], &[], &[]);
    rejected(f.select(None, [true; 3]), "not-regular");
}

#[test]
fn an_unknown_merge_base_fails_closed() {
    let f = Fixture::plain();
    let allowed = permissions([true; 3]);
    rejected(
        f.run_with(URL, "", None, &allowed, 0),
        "base branch was not fetched",
    );
    rejected(
        f.run_with(URL, "main", None, &allowed, 0),
        "base branch was not fetched",
    );
    let absent = "0123456789abcdef0123456789abcdef01234567";
    rejected(
        f.run_with(URL, absent, None, &allowed, 0),
        "not in the local clone",
    );
    assert_eq!(f.calls(), "");
}

#[test]
fn effective_disallowed_method_never_falls_back() {
    for method in ["merge", "squash", "rebase"] {
        let a = match method {
            "merge" => [false, true, true],
            "squash" => [true, false, true],
            _ => [true, true, false],
        };
        let f = Fixture::with(
            &[(CONFIG, Some(&format!("merge_method = \"{method}\"\n")))],
            &[],
            &[],
        );
        rejected(f.select(Some(method), a), "is disallowed");
        rejected(f.select(None, a), "is disallowed");
    }
}

#[test]
fn unsupported_explicit_context_is_rejected_exactly() {
    let f = Fixture::plain();
    for value in [
        "MERGE",
        " merge",
        "merge ",
        "auto",
        "--admin",
        "merge\nsquash",
    ] {
        rejected(
            f.select(Some(value), [true; 3]),
            "exactly merge, squash or rebase",
        );
    }
}

#[test]
fn api_failures_and_schema_errors_fail_closed() {
    let f = Fixture::plain();
    let refusal = f.run_with(URL, &f.base, None, &permissions([true; 3]), 79);
    assert_eq!(refusal.status.code(), Some(79));
    rejected(refusal, "terminal policy exit 79");
    for body in [
        "",
        "not json",
        "null",
        "[]",
        "[true,false,false]",
        "{}",
        "{}{}",
        r#"{"allow_merge_commit":true,"allow_squash_merge":true}"#,
        r#"{"allow_merge_commit":"true","allow_squash_merge":true,"allow_rebase_merge":true}"#,
        r#"{"allow_merge_commit":true,"allow_squash_merge":null,"allow_rebase_merge":true}"#,
        r#"{"allow_merge_commit":true,"allow_squash_merge":true,"allow_rebase_merge":1}"#,
    ] {
        let out = f.run_with(URL, &f.base, None, body, 0);
        assert_eq!(out.status.code(), Some(1));
        rejected(out, "merge permissions");
    }
    let out = f.run_with(URL, &f.base, None, &permissions([true; 3]), 3);
    rejected(out, "could not read repository merge permissions");
}

#[test]
fn the_repository_is_the_one_the_pull_request_url_names() {
    let f = Fixture::plain();
    for url in [
        "",
        "u",
        "https://github.com/owner/project",
        "https://github.com/owner/project/pull/7/files",
        "https://github.com/../project/pull/7",
        "https://github.com/owner/pro ject/pull/7",
        "https://github.com/owner/project;echo/pull/7",
        "http://github.com/owner/project/pull/7",
    ] {
        rejected(
            f.run_with(url, &f.base, None, &permissions([true; 3]), 0),
            "pull request URL",
        );
    }
    assert_eq!(f.calls(), "", "an invalid URL never reaches gh");
    // GH_REPO=owner-b/repo-b is set and ignored; the stub refuses any other argv.
    success(f.select(None, [true; 3]), "squash", "default");
    assert_eq!(f.calls(), "api --hostname github.com repos/owner/project\n");
}

#[test]
fn recipes_declare_empty_context_and_forward_it_at_nested_boundaries() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/recipes");
    for name in [
        "auto-drive-to-merge",
        "auto-drive-light",
        "autodrive-merge-loop",
    ] {
        let text = fs::read_to_string(root.join(format!("{name}.yaml"))).unwrap();
        let recipe: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        assert_eq!(
            recipe["context"]["merge_method"].as_str(),
            Some(""),
            "{name}: no implicit squash preference"
        );
        // #1888 adds no mandatory repository context (crusty round 4).
        assert!(recipe["context"].get("repository").is_none(), "{name}");
        assert!(!text.contains("autodrive_bind_repository"), "{name}");
        for step in recipe["steps"].as_sequence().unwrap() {
            if matches!(
                step["recipe"].as_str(),
                Some("auto-drive-to-merge" | "autodrive-merge-loop")
            ) {
                assert_eq!(
                    step["context"]["merge_method"].as_str(),
                    Some("{{merge_method}}"),
                    "{name}: nested recipe must receive unchanged context"
                );
            }
        }
        if name == "autodrive-merge-loop" {
            let gate = recipe["steps"]
                .as_sequence()
                .unwrap()
                .iter()
                .find(|s| s["id"].as_str() == Some("step-03-merge-gate"))
                .unwrap();
            assert!(
                gate["command"]
                    .as_str()
                    .unwrap()
                    .contains("--merge-method \"${RECIPE_VAR_merge_method:-}\""),
                "merge loop must forward context to the trusted gate"
            );
        }
    }
}
