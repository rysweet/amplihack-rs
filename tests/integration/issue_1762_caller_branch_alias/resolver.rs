//! The resolver, `workflow_remote_branch.sh` (#1762 contract C1, D2, AC10):
//! maps the checked-out branch to its name on origin, reads only, and exits
//! non-zero only on a usage error.

use std::fs;
use std::path::{Path, PathBuf};

use super::fixture::*;

/// A linked worktree at a fresh directory on new branch `name` (from main),
/// with `branch.<name>.remote` and `.merge` set when `upstream` is given.
fn checkout(fx: &Fx, name: &str, upstream: Option<(&str, &str)>) -> PathBuf {
    let dir = fx.root.join("checkouts").join(name.replace('/', "_"));
    fx.git(
        &fx.caller,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            name,
            "--",
            dir.to_str().unwrap(),
            "main",
        ],
    );
    if let Some((remote, merge)) = upstream {
        fx.git(
            &fx.caller,
            &["config", &format!("branch.{name}.remote"), remote],
        );
        fx.git(
            &fx.caller,
            &["config", &format!("branch.{name}.merge"), merge],
        );
    }
    dir
}

/// Assert the resolver prints `expected` (one line, or nothing) and exits 0.
fn assert_resolves(fx: &Fx, dir: &Path, expected: &str, why: &str) {
    let run = fx.tool(RESOLVER, &fx.root, &[dir.to_str().unwrap()], &[]);
    assert_eq!(run.code, Some(0), "{why}\n{}", run.describe());
    let want = if expected.is_empty() {
        String::new()
    } else {
        format!("{expected}\n")
    };
    assert_eq!(run.stdout, want, "{why}\n{}", run.describe());
}

#[test]
fn remote_branch_resolver_contract() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let heads = |b: &str| format!("refs/heads/{b}");
    let alias1 = checkout(&fx, &alias(1), Some(("origin", heads(BRANCH).as_str())));

    assert_resolves(&fx, &fx.caller, BRANCH, "a plain branch prints unchanged");
    assert_resolves(&fx, &alias1, BRANCH, "an alias prints the remote branch");

    // The configuration is read, not @{u}: a missing tracking ref is fine.
    fx.git(
        &fx.caller,
        &["update-ref", "-d", &format!("refs/remotes/origin/{BRANCH}")],
    );
    assert_resolves(&fx, &alias1, BRANCH, "no refs/remotes/origin/<B>");

    // DIR defaults to `.`.
    let run = fx.tool(RESOLVER, &alias1, &[], &[]);
    assert_eq!(run.code, Some(0), "{}", run.describe());
    assert_eq!(run.stdout, format!("{BRANCH}\n"), "DIR defaults to `.`");

    let unchanged = [
        (
            format!("{BRANCH}--wt-2"),
            Some(("upstream", heads(BRANCH))),
            "another remote",
        ),
        (
            format!("{BRANCH}--wt-3"),
            Some(("origin", heads("main"))),
            "another upstream branch",
        ),
        (format!("{BRANCH}--wt-4"), None, "no upstream configured"),
        (
            format!("{BRANCH}--wt-0"),
            Some(("origin", heads(BRANCH))),
            "n is 0",
        ),
        (
            format!("{BRANCH}--wt-05"),
            Some(("origin", heads(BRANCH))),
            "n has a leading zero",
        ),
        (
            format!("{BRANCH}--wt-x"),
            Some(("origin", heads(BRANCH))),
            "n is not a number",
        ),
        (
            format!("{BRANCH}--wt-6--wt-7"),
            Some(("origin", heads(BRANCH))),
            "only the last suffix is parsed",
        ),
        (
            "main--wt-1".to_owned(),
            Some(("origin", heads("main"))),
            "main is a default branch",
        ),
        (
            "master--wt-1".to_owned(),
            Some(("origin", heads("master"))),
            "master is a default branch",
        ),
    ];
    for (name, upstream, why) in &unchanged {
        let dir = checkout(&fx, name, upstream.as_ref().map(|(r, m)| (*r, m.as_str())));
        assert_resolves(&fx, &dir, name, why);
    }
    let nested = format!("{BRANCH}--wt-1--wt-2");
    let dir = checkout(&fx, &nested, Some(("origin", heads(&alias(1)).as_str())));
    assert_resolves(
        &fx,
        &dir,
        &alias(1),
        "the last suffix maps to its own prefix",
    );

    // The origin/HEAD target is a default branch; develop is one only when
    // origin/HEAD does not resolve.
    fx.git(&fx.caller, &["push", "-q", "origin", "main:trunk"]);
    fx.git(&fx.caller, &["remote", "set-head", "origin", "trunk"]);
    let trunk = checkout(
        &fx,
        "trunk--wt-1",
        Some(("origin", heads("trunk").as_str())),
    );
    assert_resolves(&fx, &trunk, "trunk--wt-1", "the origin/HEAD target");
    let develop = checkout(
        &fx,
        "develop--wt-1",
        Some(("origin", heads("develop").as_str())),
    );
    assert_resolves(
        &fx,
        &develop,
        "develop",
        "develop while origin/HEAD resolves",
    );
    fx.git(&fx.caller, &["remote", "set-head", "origin", "-d"]);
    assert_resolves(
        &fx,
        &develop,
        "develop--wt-1",
        "develop with no origin/HEAD",
    );

    // Nothing to report: detached HEAD, a missing DIR, outside a repository.
    let detached = fx.root.join("checkouts/detached");
    fx.git(
        &fx.caller,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            detached.to_str().unwrap(),
            "main",
        ],
    );
    assert_resolves(&fx, &detached, "", "detached HEAD");
    assert_resolves(&fx, &fx.root.join("does-not-exist"), "", "missing DIR");
    let outside = fx.root.join("home");
    let run = fx.tool(
        RESOLVER,
        &fx.root,
        &[outside.to_str().unwrap()],
        &[("GIT_CEILING_DIRECTORIES", fx.root.to_str().unwrap())],
    );
    assert_eq!(
        run.code,
        Some(0),
        "outside a repository\n{}",
        run.describe()
    );
    assert!(
        run.stdout.is_empty(),
        "outside a repository\n{}",
        run.describe()
    );

    // More than one argument is the only error.
    let run = fx.tool(RESOLVER, &fx.root, &[".", "."], &[]);
    assert_eq!(run.code, Some(2), "{}", run.describe());
    assert!(run.stdout.is_empty(), "{}", run.describe());
    assert!(
        run.stderr
            .contains("usage: workflow_remote_branch.sh [DIR]"),
        "{}",
        run.describe()
    );
}

/// S7: an inherited GIT_DIR, GIT_WORK_TREE or GIT_INDEX_FILE cannot point the
/// resolver at another repository.
#[test]
fn resolver_ignores_inherited_git_dir() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let wt = fx.alias_worktree(&alias(1), BRANCH);
    let other = fx.other_repo("other", "other-branch");
    let git_dir = other.join(".git");
    let index = git_dir.join("index");

    let run = fx.tool(
        RESOLVER,
        &wt,
        &[wt.to_str().unwrap()],
        &[
            ("GIT_DIR", git_dir.to_str().unwrap()),
            ("GIT_WORK_TREE", other.to_str().unwrap()),
            ("GIT_INDEX_FILE", index.to_str().unwrap()),
        ],
    );

    assert_eq!(run.code, Some(0), "{}", run.describe());
    assert_eq!(run.stdout, format!("{BRANCH}\n"), "{}", run.describe());
}

/// D1: each of the three helpers is a bash 3.2 script that starts strict and
/// is executable. The reuse helper is included because it is the one that
/// writes (a fast-forward), so an inherited `GIT_DIR` there would move
/// another repository's branch.
#[test]
fn helpers_are_portable_executables() {
    for name in [RESOLVER, PLANNER, REUSE] {
        let path = tool_path(name);
        let text = read(&path);
        assert!(text.starts_with("#!/usr/bin/env bash\n"), "{name}: shebang");
        assert!(
            code_lines(&text).any(|l| l.trim() == "set -euo pipefail"),
            "{name}: set -euo pipefail"
        );
        assert!(
            code_lines(&text).any(|l| {
                let l = l.trim();
                l.starts_with("unset ")
                    && ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"]
                        .iter()
                        .all(|v| l.split_whitespace().any(|w| w == *v))
            }),
            "{name}: unsets GIT_DIR, GIT_WORK_TREE and GIT_INDEX_FILE (S7)"
        );
        for bash4 in ["mapfile", "readarray", "declare -A", "local -A"] {
            assert!(
                code_lines(&text).all(|l| !l.contains(bash4)),
                "{name}: `{bash4}` is not in bash 3.2"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(path)
                .expect("stat helper")
                .permissions()
                .mode();
            assert_eq!(mode & 0o755, 0o755, "{name}: mode 100755");
        }
    }
}
