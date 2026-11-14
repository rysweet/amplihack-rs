//! Issue #2096, Crusty finding 1: the private bare clone a recipe run works
//! in must resolve the base ref the worktree recipes look for.
//!
//! `RunCheckout::prepare` gives the private repository the normal origin
//! fetch refspec, seeds `refs/remotes/origin/*` from the caller and copies
//! the caller's `origin/HEAD` when it is a symbolic ref inside
//! `refs/remotes/origin/`. These tests run the real `resolve_base_ref`
//! bodies of `workflow-worktree`, `consensus-issue-worktree` and
//! `workflow-terminal-state`, and `workflow_delivery_inputs.sh worktree`,
//! inside the private checkout, with no fetch beforehand.
//!
//! The caller's origin is a local bare repository; nothing here reaches the
//! network, and HOME, the profile root and Git's global and system
//! configuration are private to each test.
use super::*;

// One copy of the checkout source per test binary (clippy::duplicate_mod).
use super::checkout_units::implementation;

const BASE_REF: &str = "origin/main";

fn isolated_body(name: &str, body: impl FnOnce(Fixture)) {
    crate::support::quiet_process::in_quiet_process(module_path!(), name, || {
        use crate::support::quiet_process::{child_remove_var, child_set_var};
        let f = Fixture::new();
        for key in amplihack_git::REPOSITORY_ENV_VARS {
            child_remove_var(key);
        }
        for (key, _) in std::env::vars_os() {
            let text = key.to_string_lossy();
            if text.starts_with("GIT_CONFIG_") || text == "AMPLIHACK_RUN_CHECKOUT" {
                child_remove_var(key);
            }
        }
        let home = f.root.join("home");
        for (key, value) in [
            ("HOME", home.clone()),
            ("XDG_CONFIG_HOME", home.join("config")),
            ("CLAUDE_CONFIG_DIR", home.join(".claude")),
            ("CODEX_HOME", home.join(".codex")),
            ("AMPLIHACK_HOME", home.join(".amplihack")),
            ("AMPLIHACK_CLAUDE_PROFILE_ROOT", home.join("profiles")),
        ] {
            child_set_var(key, value);
        }
        child_set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        child_set_var("GIT_CONFIG_NOSYSTEM", "1");
        child_set_var("GIT_TERMINAL_PROMPT", "0");
        body(f);
    });
}

/// The caller's branch is `main`, and it has an `origin` (a local bare
/// clone) with fetched `origin/*` refs and `origin/HEAD -> origin/main`.
fn with_origin(f: &Fixture) -> PathBuf {
    f.git(&["branch", "-M", "main"]);
    let origin = f.root.join("origin.git");
    f.git(&[
        "clone",
        "--bare",
        "--quiet",
        f.repo.to_str().unwrap(),
        origin.to_str().unwrap(),
    ]);
    f.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
    f.git(&["fetch", "--quiet", "origin"]);
    f.git(&["remote", "set-head", "origin", "main"]);
    origin
}

fn scratch(f: &Fixture) -> PathBuf {
    let scratch = f.root.join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    scratch
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap().trim().to_string()
}

/// Git in `dir` without asserting success: (exit code, stdout).
fn git_status(f: &Fixture, dir: &Path, args: &[&str]) -> (Option<i32>, String) {
    let mut c = amplihack_git::command();
    f.environment(&mut c);
    let out = c.current_dir(dir).args(args).output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

/// The `resolve_base_ref() { ... }` function text of a shipped recipe,
/// exactly as written there, dedented.
fn resolve_base_ref_of(recipe: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/recipes")
        .join(recipe);
    let source = fs::read_to_string(&path).unwrap();
    let mut lines = source.lines();
    let first = lines
        .by_ref()
        .find(|l| l.trim_start().starts_with("resolve_base_ref() {"))
        .unwrap_or_else(|| panic!("{recipe} has no resolve_base_ref"));
    let indent = first.len() - first.trim_start().len();
    let mut body = vec![first[indent..].to_string()];
    for line in lines {
        let dedented = line.get(indent..).unwrap_or(line.trim_start());
        body.push(dedented.to_string());
        if line.len() - line.trim_start().len() == indent && line.trim() == "}" {
            return body.join("\n");
        }
    }
    panic!("{recipe}: resolve_base_ref has no closing brace at its indentation");
}

/// Runs `script` with bash in `dir`, in the fixture's private environment.
fn bash_in(f: &Fixture, dir: &Path, script: &str, args: &[&str]) -> Output {
    let mut c = Command::new("bash");
    f.environment(&mut c);
    c.current_dir(dir)
        .arg("-c")
        .arg(script)
        .arg("resolve-base-ref")
        .args(args)
        .output()
        .unwrap()
}

fn resolved_base(f: &Fixture, dir: &Path, recipe: &str) -> String {
    // workflow-terminal-state reads its explicit base from BASE_INPUT.
    let script = format!(
        "set -u\nBASE_INPUT=''\n{}\nresolve_base_ref",
        resolve_base_ref_of(recipe)
    );
    let out = bash_in(f, dir, &script, &[]);
    assert!(
        out.status.success(),
        "{recipe}: resolve_base_ref failed in the private checkout\nstdout {}\nstderr {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn private_checkout_resolves_workflow_worktree_base() {
    isolated_body("private_checkout_resolves_workflow_worktree_base", |f| {
        with_origin(&f);
        let caller_main = text(f.git(&["rev-parse", "refs/remotes/origin/main"]));
        let checkout = implementation::RunCheckout::prepare(&f.repo, &scratch(&f), false).unwrap();
        let dir = checkout.exec_dir();
        assert_eq!(
            text(f.git_in(dir, &["config", "--get", "remote.origin.fetch"])),
            "+refs/heads/*:refs/remotes/origin/*",
            "the private repository has the normal origin fetch refspec"
        );
        assert_eq!(
            text(f.git_in(dir, &["rev-parse", "--verify", "refs/remotes/origin/main"])),
            caller_main,
            "origin/main is seeded from the caller"
        );
        assert_eq!(
            text(f.git_in(dir, &["symbolic-ref", "refs/remotes/origin/HEAD"])),
            "refs/remotes/origin/main",
            "origin/HEAD is copied as a symbolic ref, not invented or flattened"
        );
        assert_eq!(resolved_base(&f, dir, "workflow-worktree.yaml"), BASE_REF);
        let delivery = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../amplifier-bundle/tools/workflow_delivery_inputs.sh");
        let mut c = Command::new("bash");
        f.environment(&mut c);
        let out = c
            .current_dir(dir)
            .arg(&delivery)
            .args(["worktree", "main", "false", "", ""])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "workflow_delivery_inputs.sh worktree main failed in the private checkout: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(text(out.stdout), BASE_REF);
    });
}

#[test]
fn private_checkout_resolves_consensus_and_terminal_bases() {
    isolated_body(
        "private_checkout_resolves_consensus_and_terminal_bases",
        |f| {
            with_origin(&f);
            let checkout =
                implementation::RunCheckout::prepare(&f.repo, &scratch(&f), false).unwrap();
            for recipe in [
                "consensus-issue-worktree.yaml",
                "workflow-terminal-state.yaml",
            ] {
                // workflow-terminal-state falls back to local `main`, which the
                // bare clone has; only origin/main shows the seeded base.
                assert_eq!(
                    resolved_base(&f, checkout.exec_dir(), recipe),
                    BASE_REF,
                    "{recipe}"
                );
            }
        },
    );
}

#[test]
fn private_checkout_fetch_updates_origin_refs() {
    isolated_body("private_checkout_fetch_updates_origin_refs", |f| {
        with_origin(&f);
        let checkout = implementation::RunCheckout::prepare(&f.repo, &scratch(&f), false).unwrap();
        fs::write(f.repo.join("advanced"), b"origin moves on\n").unwrap();
        f.git(&["add", "advanced"]);
        f.git(&["commit", "-qm", "origin advances"]);
        f.git(&["push", "--quiet", "origin", "main"]);
        let advanced = text(f.git(&["rev-parse", "HEAD"]));
        f.git_in(checkout.exec_dir(), &["fetch", "--quiet", "origin"]);
        assert_eq!(
            text(f.git_in(
                checkout.exec_dir(),
                &["rev-parse", "--verify", "refs/remotes/origin/main"]
            )),
            advanced,
            "a later `git fetch origin` in the private repository moves origin/main"
        );
    });
}

#[test]
fn private_checkout_with_origin_url_but_no_origin_refs_gets_only_the_refspec() {
    isolated_body(
        "private_checkout_with_origin_url_but_no_origin_refs_gets_only_the_refspec",
        |f| {
            f.git(&["branch", "-M", "main"]);
            let origin = f.root.join("origin.git");
            f.git(&[
                "clone",
                "--bare",
                "--quiet",
                f.repo.to_str().unwrap(),
                origin.to_str().unwrap(),
            ]);
            f.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
            let checkout =
                implementation::RunCheckout::prepare(&f.repo, &scratch(&f), false).unwrap();
            let dir = checkout.exec_dir();
            assert_eq!(
                text(f.git_in(dir, &["config", "--get", "remote.origin.fetch"])),
                "+refs/heads/*:refs/remotes/origin/*"
            );
            assert_eq!(
                text(f.git_in(dir, &["for-each-ref", "refs/remotes/"])),
                "",
                "preparation never fetches over the remote; nothing is seeded"
            );
        },
    );
}

#[test]
fn private_checkout_without_origin_has_no_refspec() {
    isolated_body("private_checkout_without_origin_has_no_refspec", |f| {
        let checkout = implementation::RunCheckout::prepare(&f.repo, &scratch(&f), false).unwrap();
        let dir = checkout.exec_dir();
        assert_eq!(
            git_status(&f, dir, &["config", "--get", "remote.origin.fetch"]).0,
            Some(1),
            "no origin, no refspec"
        );
        assert_eq!(text(f.git_in(dir, &["for-each-ref", "refs/remotes/"])), "");
    });
}

#[test]
fn private_checkout_without_caller_origin_head_invents_none() {
    isolated_body(
        "private_checkout_without_caller_origin_head_invents_none",
        |f| {
            with_origin(&f);
            f.git(&["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
            let checkout =
                implementation::RunCheckout::prepare(&f.repo, &scratch(&f), false).unwrap();
            let dir = checkout.exec_dir();
            assert_ne!(
                git_status(&f, dir, &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"]).0,
                Some(0),
                "origin/HEAD is copied only when the caller has one"
            );
            assert!(
                git_status(
                    &f,
                    dir,
                    &["rev-parse", "--verify", "-q", "refs/remotes/origin/main"]
                )
                .0 == Some(0),
                "origin/main is still seeded"
            );
        },
    );
}

#[test]
fn private_checkout_rejects_foreign_origin_head() {
    isolated_body("private_checkout_rejects_foreign_origin_head", |f| {
        with_origin(&f);
        // An unpushed local commit, so a leaked origin/HEAD at the caller's
        // main is distinguishable from origin/main.
        f.git(&["commit", "-q", "--allow-empty", "-m", "unpushed"]);
        // A symbolic origin/HEAD that leaves refs/remotes/origin/.
        f.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/heads/main",
        ]);
        // D4: a foreign target warns and preparation carries on, so it must
        // succeed. The rev-parse check below also covers a symbolic copy.
        let checkout = implementation::RunCheckout::prepare(&f.repo, &scratch(&f), false).unwrap();
        let dir = checkout.exec_dir();
        let (code, leaked) = git_status(
            &f,
            dir,
            &["rev-parse", "--verify", "-q", "refs/remotes/origin/HEAD"],
        );
        assert_ne!(
            code,
            Some(0),
            "a rejected origin/HEAD must not arrive in the private repository: {leaked}"
        );
    });
}

#[test]
fn private_checkout_does_not_copy_plain_origin_head() {
    isolated_body("private_checkout_does_not_copy_plain_origin_head", |f| {
        with_origin(&f);
        let tip = text(f.git(&["rev-parse", "HEAD"]));
        // Replace the symbolic origin/HEAD with a plain ref to the same commit.
        f.git(&["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
        f.git(&["update-ref", "refs/remotes/origin/HEAD", &tip]);
        assert_eq!(
            git_status(
                &f,
                &f.repo,
                &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"]
            )
            .0,
            Some(1),
            "the caller's origin/HEAD is a plain ref"
        );
        let checkout = implementation::RunCheckout::prepare(&f.repo, &scratch(&f), false).unwrap();
        let dir = checkout.exec_dir();
        assert_ne!(
            git_status(&f, dir, &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"]).0,
            Some(0),
            "a plain origin/HEAD is never turned into a symbolic one"
        );
        let (code, leaked) = git_status(
            &f,
            dir,
            &["rev-parse", "--verify", "-q", "refs/remotes/origin/HEAD"],
        );
        assert_ne!(code, Some(0), "a plain origin/HEAD is not copied: {leaked}");
        assert_eq!(
            git_status(
                &f,
                dir,
                &["rev-parse", "--verify", "-q", "refs/remotes/origin/main"]
            )
            .0,
            Some(0),
            "origin/main is still seeded"
        );
    });
}

/// Runs `amplifier-bundle/tools/<tool>` with bash in `dir`.
fn tool_in(f: &Fixture, dir: &Path, tool: &str, args: &[&str]) -> Output {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/tools")
        .join(tool);
    let mut c = Command::new("bash");
    f.environment(&mut c);
    c.current_dir(dir).arg(path).args(args).output().unwrap()
}

/// Found by outside-in testing of PR #2290: the main root of the private
/// checkout's bare repository is the folder beside it, which is not a
/// repository. `base` must still work when given the checkout (step-04 and
/// `workflow_branch_name.sh` pass it), and `assert-not-nested` with that main
/// root must still see the repository's worktrees and refuse a nested path,
/// not list nothing and pass.
#[test]
fn private_checkout_worktree_root_helpers_work_beside_a_bare_repository() {
    isolated_body(
        "private_checkout_worktree_root_helpers_work_beside_a_bare_repository",
        |f| {
            with_origin(&f);
            let checkout =
                implementation::RunCheckout::prepare(&f.repo, &scratch(&f), false).unwrap();
            let dir = checkout.exec_dir();
            let root = tool_in(&f, dir, "workflow_worktree_root.sh", &["root", "."]);
            assert!(
                root.status.success(),
                "root: {}",
                String::from_utf8_lossy(&root.stderr)
            );
            let main = PathBuf::from(text(root.stdout));
            let (code, _) = git_status(&f, &main, &["rev-parse", "--git-dir"]);
            assert_ne!(
                code,
                Some(0),
                "the main root of a bare repository is not a repository"
            );

            let base = tool_in(&f, dir, "workflow_worktree_root.sh", &["base", "."]);
            assert!(
                base.status.success(),
                "base: {}",
                String::from_utf8_lossy(&base.stderr)
            );
            assert!(!text(base.stdout).is_empty());

            let outer = f.root.join("outer-wt");
            f.git_in(
                dir,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    "qa-outer",
                    outer.to_str().unwrap(),
                    "HEAD",
                ],
            );
            let nested = outer.join("inner");
            let refused = tool_in(
                &f,
                dir,
                "workflow_worktree_root.sh",
                &[
                    "assert-not-nested",
                    nested.to_str().unwrap(),
                    main.to_str().unwrap(),
                ],
            );
            assert!(
                !refused.status.success(),
                "a path inside another worktree must be refused beside a bare repository"
            );
            assert!(String::from_utf8_lossy(&refused.stderr).contains("nested inside"));
            let free = f.root.join("free-wt");
            let allowed = tool_in(
                &f,
                dir,
                "workflow_worktree_root.sh",
                &[
                    "assert-not-nested",
                    free.to_str().unwrap(),
                    main.to_str().unwrap(),
                ],
            );
            assert!(
                allowed.status.success(),
                "{}",
                String::from_utf8_lossy(&allowed.stderr)
            );
        },
    );
}
