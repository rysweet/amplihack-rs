//! #1888: `amplihack orch helper select-merge-method`, with a fake `gh` and
//! real git history, so the base-tip and merge-base rules are checked against
//! git itself.
//!
//! CLI contract: select-merge-method --pr-url URL --repo-root PATH
//! [--base-ref NAME] [--base SHA] [--head SHA] [--merge-method VALUE]. It
//! prints one JSON object: the selection, or `{"error": reason}` with exit 1
//! (79 on a gh policy refusal).
//!
//! History in every fixture (the base moves on after the branch point):
//!
//!   M0 (merge base) --- HEAD (the pull request; always changes pr.txt)
//!     \
//!      BASE (base tip: `.amplihack/auto-drive.toml` is read here)
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

const URL: &str = "https://github.com/owner/project/pull/7";
const CONFIG: &str = ".amplihack/auto-drive.toml";
const QA_TOML: &str = ".amplihack/qa.toml";

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

/// A path to write before a commit. `None` deletes it (a file or a whole
/// directory). Content `@link:<target>` makes a symlink and `@exec:<text>` an
/// executable file, as the shell suite's `fx_tree` does.
type Change<'a> = (&'a str, Option<&'a str>);

fn apply(repo: &Path, changes: &[Change]) {
    for (path, content) in changes {
        let p = repo.join(path);
        if fs::symlink_metadata(&p).is_ok() {
            if p.is_dir() && !p.is_symlink() {
                fs::remove_dir_all(&p).unwrap();
            } else {
                fs::remove_file(&p).unwrap();
            }
        }
        let Some(text) = content else { continue };
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        if let Some(target) = text.strip_prefix("@link:") {
            std::os::unix::fs::symlink(target, &p).unwrap();
        } else if let Some(text) = text.strip_prefix("@exec:") {
            use std::os::unix::fs::PermissionsExt;
            fs::write(&p, text).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        } else {
            fs::write(&p, text).unwrap();
        }
    }
}

/// One `select-merge-method` run. `call()` gives the defaults.
struct Call<'a> {
    url: &'a str,
    base_ref: &'a str,
    /// `None` is the fixture's base tip.
    base: Option<&'a str>,
    explicit: Option<&'a str>,
    settings: String,
    settings_rc: u8,
    rules: String,
    rules_rc: u8,
}
fn call() -> Call<'static> {
    Call {
        url: URL,
        base_ref: "main",
        base: None,
        explicit: None,
        settings: permissions([true; 3]),
        settings_rc: 0,
        rules: "[]".to_string(),
        rules_rc: 0,
    }
}

impl Fixture {
    /// `at_m0`: what the merge base commits. `on_head` and `on_base`: what
    /// the pull request and the moved-on base change after it. The pull
    /// request also changes `pr.txt`, so its diff is never empty.
    fn with(at_m0: &[Change], on_head: &[Change], on_base: &[Change]) -> Self {
        let mut head: Vec<Change> = on_head.to_vec();
        head.push(("pr.txt", Some("pr\n")));
        Self::build(at_m0, &head, on_base)
    }
    /// As `with`, with exactly `on_head` and nothing more on the head.
    fn build(at_m0: &[Change], on_head: &[Change], on_base: &[Change]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        for dir in ["repo", "bin", "home", "tmp"] {
            fs::create_dir_all(tmp.path().join(dir)).unwrap();
        }
        crate::support::exec_stub::write_executable(
            &tmp.path().join("bin/gh"),
            r#"#!/bin/bash
printf '%s\n' "$*" >> "$TEST_CALLS"
case "$*" in
  "api --hostname github.com repos/owner/project")
    cat "$TEST_RESPONSE"; exit "$TEST_API_RC" ;;
  "api --hostname github.com --paginate repos/owner/project/rules/branches/$TEST_BASE_REF")
    cat "$TEST_RULES"; exit "$TEST_RULES_RC" ;;
esac
exit 91
"#,
            0o755,
        );
        git(&repo, &["init", "-q", "-b", "main"]);
        let commit = |changes: &[Change], msg: &str| {
            apply(&repo, changes);
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
        // What the gate's fetch leaves behind; the shell qa.toml reader
        // finds the merge base through it.
        git(&repo, &["update-ref", "refs/remotes/origin/main", &base]);
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
    fn run(&self, c: Call<'_>) -> Output {
        let root = self.tmp.path();
        fs::write(root.join("response"), &c.settings).unwrap();
        fs::write(root.join("rules"), &c.rules).unwrap();
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
        .env("TEST_API_RC", c.settings_rc.to_string())
        .env("TEST_RULES", root.join("rules"))
        .env("TEST_RULES_RC", c.rules_rc.to_string())
        .env("TEST_BASE_REF", c.base_ref)
        .current_dir(root.join("home"))
        .args(["orch", "helper", "select-merge-method", "--pr-url", c.url])
        .arg("--repo-root")
        .arg(self.repo())
        .args(["--base-ref", c.base_ref])
        .args([
            "--base",
            c.base.unwrap_or(self.base.as_str()),
            "--head",
            &self.head,
        ]);
        if let Some(value) = c.explicit {
            cmd.args(["--merge-method", value]);
        }
        cmd.output().unwrap()
    }
    fn select(&self, explicit: Option<&str>, allowed: [bool; 3]) -> Output {
        self.run(Call {
            explicit,
            settings: permissions(allowed),
            ..call()
        })
    }
    /// The shell `.amplihack/qa.toml` reader, `autodrive_qa_config`, on this
    /// fixture as the evidence step runs it: base `main`, head the pull request.
    fn qa_config(&self) -> String {
        let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/tools");
        let out = std::process::Command::new("bash")
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", self.tmp.path().join("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("AUTODRIVE_QA_BASE", "main")
            .env("AUTODRIVE_QA_HEAD", &self.head)
            .args(["-c", r#". "$1" && . "$2" && autodrive_qa_config "$3""#, "_"])
            .arg(tools.join("autodrive_trust.sh"))
            .arg(tools.join("autodrive_qa_sources.sh"))
            .arg(self.repo())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout)
            .unwrap()
            .trim_end()
            .to_string()
    }
}

fn permissions(a: [bool; 3]) -> String {
    json!({"allow_merge_commit":a[0],"allow_squash_merge":a[1],"allow_rebase_merge":a[2]})
        .to_string()
}

/// One page holding one `pull_request` rule that allows `methods`.
fn pr_rule(methods: &[&str]) -> String {
    json!([{"type":"pull_request","ruleset_id":1,
        "parameters":{"required_approving_review_count":0,"allowed_merge_methods":methods}}])
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
        // The file was looked for, at the base tip, and was absent.
        assert_eq!(value["config_commit"], f.base.as_str());
    }
}

#[test]
fn the_file_at_the_base_tip_is_the_repository_preference() {
    let f = Fixture::with(
        &[(
            CONFIG,
            Some("# Simard requires merge commits\nmerge_method = \"merge\"\n"),
        )],
        &[("src.rs", Some("v1\n"))],
        &[],
    );
    let value = success(f.select(None, [true; 3]), "merge", "repository");
    assert_eq!(value["config_commit"], f.base.as_str());
    let rationale = value["rationale"].as_str().unwrap();
    assert!(
        rationale.contains(&format!("base tip {}", f.base)),
        "{rationale}"
    );
    assert!(
        rationale.contains("no ruleset on branch main limits the merge method"),
        "{rationale}"
    );
    // Empty context is no context.
    success(f.select(Some(""), [true; 3]), "merge", "repository");
}

/// The case behind #1888: the pull request branched before the base gained
/// the file and was never brought up to date (on a base that does not
/// require up-to-date branches GitHub never reports BEHIND). The base's
/// policy, as the base stands now, decides.
#[test]
fn a_policy_the_base_gains_after_the_branch_point_applies_to_open_pull_requests() {
    let f = Fixture::with(
        &[],
        &[("src.rs", Some("v1\n"))],
        &[(CONFIG, Some("merge_method = \"merge\"\n"))],
    );
    let value = success(f.select(None, [true; 3]), "merge", "repository");
    assert_eq!(value["config_commit"], f.base.as_str());
    assert_ne!(f.base, f.m0);
    // A policy the base changes after the branch point: the new one.
    let changed = Fixture::with(
        &[(CONFIG, Some("merge_method = \"rebase\"\n"))],
        &[],
        &[(CONFIG, Some("merge_method = \"merge\"\n"))],
    );
    success(changed.select(None, [true; 3]), "merge", "repository");
    // A policy the base drops after the branch point: none.
    let dropped = Fixture::with(
        &[(CONFIG, Some("merge_method = \"merge\"\n"))],
        &[],
        &[(CONFIG, None)],
    );
    let value = success(dropped.select(None, [true; 3]), "squash", "default");
    assert_eq!(value["config_commit"], dropped.base.as_str());
    // An invalid file the base gains after the branch point blocks, naming
    // the base tip it was read at.
    let broken = Fixture::with(&[], &[], &[(CONFIG, Some("merge_method = merge\n"))]);
    rejected(
        broken.select(None, [true; 3]),
        &format!("{CONFIG} at base tip {}: syntax", broken.base),
    );
}

#[test]
fn the_working_tree_is_never_read() {
    let f = Fixture::plain();
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
    // And an explicit method needs no base tip, so an unfetched base is fine.
    let out = broken.run(Call {
        base: Some(""),
        explicit: Some("merge"),
        ..call()
    });
    success(out, "merge", "explicit");
}

#[test]
fn a_pull_request_never_chooses_its_own_merge_method() {
    let changes: [&[Change]; 4] = [
        &[(CONFIG, Some("merge_method = \"rebase\"\n"))], // modify
        &[(CONFIG, None)],                                // delete
        &[(CONFIG, Some("merge_method = \"merge\"\n"))],  // unchanged content: a no-op
        &[(QA_TOML, Some("command = \"make\"\n"))],
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
    // A pull request that merged in the base, and with it the file the base
    // gained, does not change it.
    let f = Fixture::with(&[], &[], &[(CONFIG, Some("merge_method = \"merge\"\n"))]);
    git(&f.repo(), &["checkout", "-q", "pr"]);
    git(&f.repo(), &["merge", "-q", "--no-edit", &f.base]);
    let merged_head = git(&f.repo(), &["rev-parse", "HEAD"]);
    let g = Fixture {
        head: merged_head,
        ..f
    };
    success(g.select(None, [true; 3]), "merge", "repository");
}

#[test]
fn an_empty_pull_request_diff_is_refused_as_qa_toml_refuses_it() {
    // The head adds a commit that changes nothing: no diff shows that the
    // pull request leaves the file alone (qa.toml's `paths-empty`).
    let f = Fixture::build(&[(CONFIG, Some("merge_method = \"merge\"\n"))], &[], &[]);
    rejected(f.select(None, [true; 3]), "paths-empty");
    assert_eq!(f.calls(), "");
    success(f.select(Some("merge"), [true; 3]), "merge", "explicit");
    // With no file at the base tip there is nothing to protect: the default.
    let g = Fixture::build(&[], &[], &[]);
    success(g.select(None, [true; 3]), "squash", "default");
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
            reason.contains(&f.base),
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
fn symlinks_and_odd_entries_at_the_base_tip_are_refused() {
    for (link, target) in [(CONFIG, "../policy.toml"), (".amplihack", "elsewhere")] {
        let f = Fixture::with(
            &[
                ("policy.toml", Some("merge_method = \"merge\"\n")),
                (link, Some(&format!("@link:{target}"))),
            ],
            &[],
            &[],
        );
        rejected(f.select(None, [true; 3]), "symlink");
    }
    // A directory where the file should be.
    let f = Fixture::with(&[(".amplihack/auto-drive.toml/x", Some("y\n"))], &[], &[]);
    rejected(f.select(None, [true; 3]), "not-regular");
}

#[test]
fn an_unknown_base_tip_fails_closed() {
    let f = Fixture::plain();
    for base in ["", "main"] {
        rejected(
            f.run(Call {
                base: Some(base),
                ..call()
            }),
            "base branch was not fetched",
        );
    }
    let absent = "0123456789abcdef0123456789abcdef01234567";
    rejected(
        f.run(Call {
            base: Some(absent),
            ..call()
        }),
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
    let refusal = f.run(Call {
        settings_rc: 79,
        ..call()
    });
    assert_eq!(refusal.status.code(), Some(79));
    rejected(
        refusal,
        "repository merge permissions (terminal policy exit 79)",
    );
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
        let out = f.run(Call {
            settings: body.to_string(),
            ..call()
        });
        assert_eq!(out.status.code(), Some(1));
        rejected(out, "merge permissions");
    }
    let out = f.run(Call {
        settings_rc: 3,
        ..call()
    });
    rejected(out, "could not read repository merge permissions");
}

/// GitHub's `pull_request` ruleset rule can limit the merge methods without
/// changing the repository's `allow_*` fields. The allowed set is both.
#[test]
fn ruleset_merge_method_rules_limit_every_source() {
    let f = Fixture::plain();
    // Settings allow all three, a ruleset only merge: the default is merge.
    let value = success(
        f.run(Call {
            rules: pr_rule(&["merge"]),
            ..call()
        }),
        "merge",
        "default",
    );
    let rationale = value["rationale"].as_str().unwrap();
    assert!(
        rationale.contains("1 ruleset pull_request rule(s) on branch main allow merge"),
        "{rationale}"
    );
    // An explicit method the ruleset does not list is refused, not replaced.
    rejected(
        f.run(Call {
            explicit: Some("squash"),
            rules: pr_rule(&["merge"]),
            ..call()
        }),
        "selected merge method squash is disallowed by the rulesets on branch main (pull_request allowed_merge_methods allow merge); no other method is tried",
    );
    // So is the repository file's.
    let file = Fixture::with(&[(CONFIG, Some("merge_method = \"squash\"\n"))], &[], &[]);
    rejected(
        file.run(Call {
            rules: pr_rule(&["merge", "rebase"]),
            ..call()
        }),
        "squash is disallowed by the rulesets on branch main",
    );
    // Settings allow only squash, the ruleset only merge: nothing is both.
    rejected(
        f.run(Call {
            settings: permissions([false, true, false]),
            rules: pr_rule(&["merge"]),
            ..call()
        }),
        "the repository settings allow squash and the rulesets on branch main allow merge: no merge method is allowed by both",
    );
    // Every rule on every page counts: a reader that stops at page one
    // would allow merge.
    success(
        f.run(Call {
            rules: format!(
                "{}\n{}",
                pr_rule(&["merge", "rebase"]),
                pr_rule(&["rebase"])
            ),
            ..call()
        }),
        "rebase",
        "default",
    );
    // Other rules, and a pull_request rule that lists no methods, do not limit.
    let value = success(
        f.run(Call {
            rules: r#"[{"type":"required_linear_history"},{"type":"pull_request","parameters":{"required_approving_review_count":1}}]"#.to_string(),
            ..call()
        }),
        "squash",
        "default",
    );
    assert!(
        value["rationale"]
            .as_str()
            .unwrap()
            .contains("no ruleset on branch main limits the merge method")
    );
    // The rules of the pull request's own base branch are the ones read.
    let out = f.run(Call {
        base_ref: "release/1.0",
        rules: pr_rule(&["rebase"]),
        ..call()
    });
    success(out, "rebase", "default");
    assert!(
        f.calls().contains(
            "api --hostname github.com --paginate repos/owner/project/rules/branches/release/1.0\n"
        ),
        "{}",
        f.calls()
    );
}

#[test]
fn ruleset_read_failures_fail_closed() {
    let f = Fixture::plain();
    let refusal = f.run(Call {
        rules_rc: 79,
        ..call()
    });
    assert_eq!(refusal.status.code(), Some(79));
    rejected(
        refusal,
        "gh api refused to read the rulesets on branch main (terminal policy exit 79)",
    );
    rejected(
        f.run(Call {
            rules_rc: 3,
            ..call()
        }),
        "gh api could not read the rulesets on branch main",
    );
    for body in [
        "",
        "not json",
        "null",
        "{}",
        "[1]",
        r#"[{"type":"pull_request"}]"#,
        r#"[{"type":"pull_request","parameters":{"allowed_merge_methods":"merge"}}]"#,
        r#"[{"type":"pull_request","parameters":{"allowed_merge_methods":[true]}}]"#,
        "[] trailing",
    ] {
        let out = f.run(Call {
            rules: body.to_string(),
            ..call()
        });
        assert_eq!(out.status.code(), Some(1), "{body:?}");
        let reason = rejected(out, "invalid answer for the rulesets on branch main");
        assert!(!reason.contains("trailing"), "no response text in {reason}");
    }
}

#[test]
fn the_base_branch_name_is_checked_before_gh_runs() {
    let f = Fixture::plain();
    for base_ref in ["", "-main", "a/../b", "a b", ".hidden", "a//b", "main/"] {
        for explicit in [None, Some("merge")] {
            rejected(
                f.run(Call {
                    base_ref,
                    explicit,
                    ..call()
                }),
                "base branch name",
            );
        }
    }
    assert_eq!(
        f.calls(),
        "",
        "an invalid base branch name never reaches gh"
    );
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
        rejected(f.run(Call { url, ..call() }), "pull request URL");
    }
    assert_eq!(f.calls(), "", "an invalid URL never reaches gh");
    // GH_REPO=owner-b/repo-b is set and ignored; the stub refuses any other argv.
    success(f.select(None, [true; 3]), "squash", "default");
    assert_eq!(
        f.calls(),
        "api --hostname github.com repos/owner/project\n\
         api --hostname github.com --paginate repos/owner/project/rules/branches/main\n"
    );
}

/// Concern 3 of crusty round 5: the merge-method file and `.amplihack/qa.toml`
/// are read by two readers in two languages. This runs both, the Rust one
/// through the CLI and `autodrive_qa_config` from `autodrive_qa_sources.sh`,
/// over one fixture set, and requires the same token from each for every rule
/// they share: tree entries, size, grammar, `changed-in-pr` and
/// `paths-empty`. Each fixture writes both files with the same shape, each
/// with its own key (`command = "make"`, `merge_method = "merge"`), at the
/// merge base, and the base does not move, so both read the same content.
#[test]
fn the_two_amplihack_toml_readers_agree_on_their_shared_rules() {
    const TOKENS: [&str; 8] = [
        "symlink",
        "not-regular",
        "too-large",
        "syntax",
        "unknown-key",
        "duplicate-key",
        "changed-in-pr",
        "paths-empty",
    ];
    // `none` (absent), `ok` (read and valid) or a refusal token.
    let qa_token = |qa: &str| -> String {
        match qa {
            "none" => "none".into(),
            "ok\t.\tmake" => "ok".into(),
            other => other
                .strip_prefix("invalid\t")
                .unwrap_or_else(|| panic!("qa.toml reader printed {other:?}"))
                .to_string(),
        }
    };
    let mm_token = |out: &Output| -> String {
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        if out.status.success() {
            return match value["source"].as_str() {
                Some("repository") => "ok".into(),
                Some("default") => "none".into(),
                other => panic!("source {other:?}"),
            };
        }
        let reason = value["error"].as_str().unwrap();
        TOKENS
            .iter()
            .find(|t| reason.contains(&format!(": {t}")))
            .unwrap_or_else(|| panic!("no token in {reason:?}"))
            .to_string()
    };
    // A body for each reader from one template: K is the key, V the value.
    let bodies = |template: &str| -> [String; 2] {
        [("command", "make"), ("merge_method", "merge")]
            .map(|(k, v)| template.replace('K', k).replace('V', v))
    };
    // A file of exactly `size` bytes ending in the valid line.
    let sized = |size: usize| -> [String; 2] {
        [("command", "make"), ("merge_method", "merge")].map(|(k, v)| {
            let line = format!("{k} = \"{v}\"\n");
            format!("#{}\n{line}", "x".repeat(size - line.len() - 2))
        })
    };
    let both = |bodies: &[String; 2]| -> Vec<(String, Option<String>)> {
        vec![
            (QA_TOML.to_string(), Some(bodies[0].clone())),
            (CONFIG.to_string(), Some(bodies[1].clone())),
        ]
    };
    let valid = bodies("K = \"V\"\n");

    // (label, at the merge base, on the head (beside pr.txt), want, empty head)
    type Owned = Vec<(String, Option<String>)>;
    let mut cases: Vec<(String, Owned, Owned, &str, bool)> = Vec::new();
    for (template, want) in [
        ("K = \"V\"\n", "ok"),
        ("  K\t=\t\"V\"  \n", "ok"),
        ("K=\"V\"", "ok"),
        ("# comment\n\n\t\n  # indented\nK = \"V\"\n", "ok"),
        ("K = V\n", "syntax"),
        ("K: V\n", "syntax"),
        ("K = \"V\" # trailing\n", "syntax"),
        ("K = \"V\"\r\n", "syntax"),
        ("K = \"a\\b\"\n", "syntax"),
        ("K = \"a\"b\"\n", "syntax"),
        ("[section]\nK = \"V\"\n", "syntax"),
        ("K = \"V\u{e9}\"\n", "syntax"),
        ("K = \"V\"\n\u{0}", "syntax"),
        ("K = \"V\"\n\u{b}", "syntax"),
        ("= \"V\"\n", "syntax"),
        ("K.x = \"V\"\n", "syntax"),
        ("other = \"x\"\nK = \"V\"\n", "unknown-key"),
        ("K = \"V\"\nK = \"V\"\n", "duplicate-key"),
    ] {
        cases.push((
            format!("{template:?}"),
            both(&bodies(template)),
            vec![],
            want,
            false,
        ));
    }
    cases.push(("4096 bytes".into(), both(&sized(4096)), vec![], "ok", false));
    cases.push((
        "4097 bytes".into(),
        both(&sized(4097)),
        vec![],
        "too-large",
        false,
    ));
    let link = |p: &str, target: &str| (p.to_string(), Some(format!("@link:{target}")));
    let file = |p: &str, text: &str| (p.to_string(), Some(text.to_string()));
    let gone = |p: &str| (p.to_string(), None);
    let mut exec = both(&valid);
    for entry in &mut exec {
        entry.1 = entry.1.take().map(|t| format!("@exec:{t}"));
    }
    for (label, at_m0, on_head, want, empty) in [
        ("absent", vec![], vec![], "none", false),
        (
            "directory without the file",
            vec![file(".amplihack/other.txt", "x\n")],
            vec![],
            "none",
            false,
        ),
        (
            ".amplihack is a file",
            vec![file(".amplihack", "x\n")],
            vec![],
            "not-regular",
            false,
        ),
        (
            ".amplihack is a symlink",
            vec![file("elsewhere/x", "x\n"), link(".amplihack", "elsewhere")],
            vec![],
            "symlink",
            false,
        ),
        (
            "the file is a symlink",
            vec![
                file("policy", "x\n"),
                link(QA_TOML, "../policy"),
                link(CONFIG, "../policy"),
            ],
            vec![],
            "symlink",
            false,
        ),
        (
            "the file is a directory",
            vec![
                file(&format!("{QA_TOML}/x"), "y\n"),
                file(&format!("{CONFIG}/x"), "y\n"),
            ],
            vec![],
            "not-regular",
            false,
        ),
        ("the file is executable", exec.clone(), vec![], "ok", false),
        (
            "the pull request edits it",
            both(&valid),
            both(&bodies("K = \"V\"\n# edited\n")),
            "changed-in-pr",
            false,
        ),
        (
            "the pull request deletes it",
            both(&valid),
            vec![gone(QA_TOML), gone(CONFIG)],
            "changed-in-pr",
            false,
        ),
        (
            "the pull request adds it",
            vec![],
            both(&valid),
            "changed-in-pr",
            false,
        ),
        (
            "the pull request replaces .amplihack",
            both(&valid),
            vec![gone(".amplihack"), file(".amplihack", "x\n")],
            "changed-in-pr",
            false,
        ),
        (
            "the pull request rewrites the same bytes",
            both(&valid),
            both(&valid),
            "ok",
            false,
        ),
        ("an empty diff", both(&valid), vec![], "paths-empty", true),
        ("an empty diff and no file", vec![], vec![], "none", true),
    ] {
        cases.push((label.to_string(), at_m0, on_head, want, empty));
    }

    for (label, at_m0, on_head, want, empty) in &cases {
        let m0: Vec<Change> = at_m0
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_deref()))
            .collect();
        let head: Vec<Change> = on_head
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_deref()))
            .collect();
        let f = if *empty {
            Fixture::build(&m0, &head, &[])
        } else {
            Fixture::with(&m0, &head, &[])
        };
        let qa = qa_token(&f.qa_config());
        let mm = mm_token(&f.select(None, [true; 3]));
        assert_eq!(qa, *want, "qa.toml reader, case {label}");
        assert_eq!(mm, *want, "auto-drive.toml reader, case {label}");
    }
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
