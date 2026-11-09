//! Shell helpers: `git-hook-cargo.sh` tagging and naming (D8, AC6) and the
//! bash suite for `workflow_worktree_root.sh base` and
//! `amplihack_run_owned.sh record` (D1, D4).

use super::fixture::*;
use crate::isolated_amplihack::isolated;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const TAG_SIGNATURE: &str = "Signature: 8a477f597d28d172789f06886806bc55";

/// Runs `git-hook-cargo.sh sh -c 'echo $CARGO_TARGET_DIR'` in the fixture
/// repo with `vars` set, and returns the target folder it chose.
fn hook_target(fx: &Fx, vars: &[(&str, &Path)], strs: &[(&str, &str)]) -> (PathBuf, Output) {
    hook_target_in(fx, &fx.repo, vars, strs)
}

/// As [`hook_target`], run from `cwd`.
fn hook_target_in(
    fx: &Fx,
    cwd: &Path,
    vars: &[(&str, &Path)],
    strs: &[(&str, &str)],
) -> (PathBuf, Output) {
    let mut c = isolated(fx.bin.join("bash"));
    c.arg(repo_root().join("scripts/git-hook-cargo.sh"))
        .args(["sh", "-c", "printf '%s' \"$CARGO_TARGET_DIR\""])
        .current_dir(cwd)
        .env("HOME", &fx.home)
        .env("TMPDIR", &fx.tmp)
        .env("PATH", &fx.bin)
        .env("XDG_CACHE_HOME", fx.home.join(".cache"))
        .env("AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS", "0")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("AMPLIHACK_WORK_ROOT")
        .env_remove("AMPLIHACK_RUN_ID")
        .env_remove("AMPLIHACK_RUN_DIR");
    for (k, v) in vars {
        c.env(k, v);
    }
    for (k, v) in strs {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    (
        PathBuf::from(String::from_utf8_lossy(&out.stdout).into_owned()),
        out,
    )
}

/// The hook follows #1771: a missing target is left for cargo to create
/// (cargo writes CACHEDIR.TAG into a target it creates), and a target that
/// exists without a tag is tagged on the next hook run. `run` invokes the
/// hook with the same environment each time.
fn assert_left_for_cargo_then_tagged(target: &Path, run: impl Fn() -> PathBuf) {
    assert!(
        !target.exists(),
        "the hook must leave a missing target for cargo to create: {target:?}"
    );
    fs::create_dir_all(target).unwrap();
    assert_eq!(run(), target, "the second run must choose the same target");
    assert_tagged(target);
}

fn assert_tagged(dir: &Path) {
    let tag = fs::read_to_string(dir.join("CACHEDIR.TAG"))
        .unwrap_or_else(|_| panic!("{dir:?} has no CACHEDIR.TAG"));
    assert!(
        tag.starts_with(TAG_SIGNATURE),
        "tag must start with the signature: {tag:?}"
    );
}

/// Issue #2039 (D1, D3): inside a run the hook target is
/// `<work>/targets/amplihack-precommit-target-<checkout hash>`, the same name
/// as outside a run (no run suffix). It is tagged once it exists and recorded
/// once, as `created`, with the checkout path.
#[test]
fn ac6_hook_target_inside_a_run_is_tagged_named_and_recorded() {
    let fx = Fx::new();
    let run_id = uuid::Uuid::new_v4().to_string();
    let run_dir = fx.work.join("runs").join(&run_id);
    mkdir_0700(&run_dir);
    let run = || {
        hook_target(
            &fx,
            &[
                ("AMPLIHACK_WORK_ROOT", &fx.work),
                ("AMPLIHACK_RUN_DIR", &run_dir),
            ],
            &[("AMPLIHACK_RUN_ID", &run_id)],
        )
        .0
    };
    let target = run();
    assert_eq!(target.parent(), Some(fx.work.join("targets").as_path()));
    let name = target.file_name().unwrap().to_string_lossy().into_owned();
    assert!(!name.contains("-run-"), "no run suffix: {name}");
    assert_eq!(name, hook_target_name(&fx.repo), "named by checkout only");
    let outside = hook_target(&fx, &[("AMPLIHACK_WORK_ROOT", &fx.work)], &[]).0;
    assert_eq!(outside, target, "the same name inside and outside a run");

    assert_left_for_cargo_then_tagged(&target, run);
    let owned = fs::read_to_string(run_dir.join("owned-paths")).expect("owned-paths written");
    assert_eq!(
        owned,
        format!(
            "target\t{}\t\tcreated\t{}\n",
            target.display(),
            fx.repo.display()
        ),
        "one 5-field line; the second hook run adds nothing"
    );
}

/// Issue #2039 (D3): a target that already exists before the hook runs is
/// recorded as `reused`.
#[test]
fn hook_records_an_existing_target_as_reused() {
    let fx = Fx::new();
    let run_id = uuid::Uuid::new_v4().to_string();
    let run_dir = fx.work.join("runs").join(&run_id);
    mkdir_0700(&run_dir);
    let existing = fx.work.join("targets").join(hook_target_name(&fx.repo));
    fs::create_dir_all(&existing).unwrap();

    let (target, _) = hook_target(
        &fx,
        &[
            ("AMPLIHACK_WORK_ROOT", &fx.work),
            ("AMPLIHACK_RUN_DIR", &run_dir),
        ],
        &[("AMPLIHACK_RUN_ID", &run_id)],
    );
    assert_eq!(target, existing);
    assert_eq!(
        fs::read_to_string(run_dir.join("owned-paths")).unwrap(),
        format!(
            "target\t{}\t\treused\t{}\n",
            target.display(),
            fx.repo.display()
        )
    );
}

/// Issue #2039: a dangling link at the target path is removed first, so the
/// target counts as `created`.
#[test]
fn hook_records_a_replaced_dangling_link_as_created() {
    let fx = Fx::new();
    let run_id = uuid::Uuid::new_v4().to_string();
    let run_dir = fx.work.join("runs").join(&run_id);
    mkdir_0700(&run_dir);
    let at = fx.work.join("targets").join(hook_target_name(&fx.repo));
    fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(fx.root.join("nowhere"), &at).unwrap();

    hook_target(
        &fx,
        &[
            ("AMPLIHACK_WORK_ROOT", &fx.work),
            ("AMPLIHACK_RUN_DIR", &run_dir),
        ],
        &[("AMPLIHACK_RUN_ID", &run_id)],
    );
    let owned = fs::read_to_string(run_dir.join("owned-paths")).unwrap();
    assert!(
        owned.ends_with(&format!("\tcreated\t{}\n", fx.repo.display())),
        "{owned:?}"
    );
}

/// Issue #2039 (D3): outside a run nothing is recorded, even with a run
/// folder in the environment.
#[test]
fn hook_records_nothing_outside_a_run() {
    let fx = Fx::new();
    let run_dir = fx.work.join("runs").join(uuid::Uuid::new_v4().to_string());
    mkdir_0700(&run_dir);
    hook_target(
        &fx,
        &[
            ("AMPLIHACK_WORK_ROOT", &fx.work),
            ("AMPLIHACK_RUN_DIR", &run_dir),
        ],
        &[],
    );
    assert!(!run_dir.join("owned-paths").exists());
}

/// AC6: outside a run the name keeps the hash-only form, still tagged.
#[test]
fn ac6_hook_target_outside_a_run_keeps_hash_only_name() {
    let fx = Fx::new();
    let run = || hook_target(&fx, &[("AMPLIHACK_WORK_ROOT", &fx.work)], &[]).0;
    let target = run();
    assert_eq!(target.parent(), Some(fx.work.join("targets").as_path()));
    let name = target.file_name().unwrap().to_string_lossy().into_owned();
    let hash = name.strip_prefix("amplihack-precommit-target-").unwrap();
    assert_eq!(hash.len(), 12, "hash-only name: {name}");
    assert_left_for_cargo_then_tagged(&target, run);
}

/// Without `AMPLIHACK_WORK_ROOT` the XDG cache path stays, tagged once it
/// exists.
#[test]
fn hook_target_without_work_root_uses_xdg_cache_and_is_tagged() {
    let fx = Fx::new();
    let run = || hook_target(&fx, &[], &[]).0;
    let target = run();
    assert_eq!(
        target.parent(),
        Some(fx.home.join(".cache/amplihack").as_path())
    );
    assert_left_for_cargo_then_tagged(&target, run);
}

/// #1771 and https://bford.info/cachedir/: a link named CACHEDIR.TAG, even a
/// dangling one, is never written through, and the hook still runs.
#[test]
fn hook_never_writes_through_a_cachedir_tag_link() {
    let fx = Fx::new();
    let (target, _) = hook_target(&fx, &[("AMPLIHACK_WORK_ROOT", &fx.work)], &[]);
    fs::create_dir_all(&target).unwrap();
    let outside = fx.root.join("tag-link-target");
    std::os::unix::fs::symlink(&outside, target.join("CACHEDIR.TAG")).unwrap();
    let (again, _) = hook_target(&fx, &[("AMPLIHACK_WORK_ROOT", &fx.work)], &[]);
    assert_eq!(again, target);
    assert!(
        !outside.exists(),
        "the hook wrote through a dangling CACHEDIR.TAG link"
    );
    assert!(
        fs::symlink_metadata(target.join("CACHEDIR.TAG"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

/// A run id that does not match `^[0-9a-f-]{1,64}$` is never used in a
/// path, changes nothing in the name and records nothing.
#[test]
fn hook_ignores_invalid_run_ids() {
    let fx = Fx::new();
    let (plain, _) = hook_target(&fx, &[("AMPLIHACK_WORK_ROOT", &fx.work)], &[]);
    for bad in ["../../escape", "ABC", "a b", &"a".repeat(65)] {
        let run_dir = fx.work.join("runs").join("by-bad-id");
        mkdir_0700(&run_dir);
        let (target, _) = hook_target(
            &fx,
            &[
                ("AMPLIHACK_WORK_ROOT", &fx.work),
                ("AMPLIHACK_RUN_DIR", &run_dir),
            ],
            &[("AMPLIHACK_RUN_ID", bad)],
        );
        let name = target.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!name.contains("-run-"), "{bad:?} gave {name}");
        assert_eq!(target, plain, "{bad:?}");
        assert!(!run_dir.join("owned-paths").exists(), "{bad:?} recorded");
    }
    assert!(!fx.root.join("escape").exists());
}

/// A relative `AMPLIHACK_WORK_ROOT` is ignored by the hook.
#[test]
fn hook_ignores_relative_work_root() {
    let fx = Fx::new();
    let (target, _) = hook_target(&fx, &[], &[("AMPLIHACK_WORK_ROOT", "relative/root")]);
    assert!(target.is_absolute(), "{target:?}");
    assert_eq!(
        target.parent(),
        Some(fx.home.join(".cache/amplihack").as_path())
    );
    assert!(!fx.repo.join("relative").exists());
}

/// P1-6: the hook uses `AMPLIHACK_WORK_ROOT` only when it already exists as
/// a private folder of this user that is neither `/` nor `$HOME`; otherwise
/// it falls back to the XDG path and creates nothing under the root.
#[test]
fn hook_falls_back_to_xdg_for_unusable_work_roots() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new();
    let missing = fx.root.join("missing-root");
    let group_writable = fx.root.join("gw-root");
    fs::create_dir_all(&group_writable).unwrap();
    fs::set_permissions(&group_writable, fs::Permissions::from_mode(0o770)).unwrap();
    let home = fx.home.display().to_string();
    let cases = [
        missing.display().to_string(),
        group_writable.display().to_string(),
        "/".to_string(),
        home,
    ];
    for root in &cases {
        let (target, _) = hook_target(&fx, &[], &[("AMPLIHACK_WORK_ROOT", root)]);
        assert_eq!(
            target.parent(),
            Some(fx.home.join(".cache/amplihack").as_path()),
            "{root}"
        );
    }
    assert!(!missing.exists(), "the hook must not create the work root");
    assert!(!group_writable.join("targets").exists());
    assert!(!fx.home.join("targets").exists());
}

/// P1-6: `targets/` under a usable work root is created private (0700).
#[test]
fn hook_creates_targets_folder_private() {
    let fx = Fx::new();
    let (target, _) = hook_target(&fx, &[("AMPLIHACK_WORK_ROOT", &fx.work)], &[]);
    assert_eq!(target.parent(), Some(fx.work.join("targets").as_path()));
    assert_eq!(mode_of(&fx.work.join("targets")), 0o700);
}

/// P1-5: when the ownership record cannot be written the hook still runs the
/// build but says so on stderr.
#[test]
fn hook_warns_when_ownership_cannot_be_recorded() {
    let fx = Fx::new();
    let run_id = uuid::Uuid::new_v4().to_string();
    let run_dir = fx.work.join("runs").join(&run_id);
    mkdir_0700(&run_dir.join("owned-paths"));
    let (target, out) = hook_target(
        &fx,
        &[
            ("AMPLIHACK_WORK_ROOT", &fx.work),
            ("AMPLIHACK_RUN_DIR", &run_dir),
        ],
        &[("AMPLIHACK_RUN_ID", &run_id)],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!(
            "amplihack: warning: could not record cache ownership for {}",
            target.display()
        )),
        "{stderr}"
    );
}

/// The bash suite for the worktree base and the owned-path recorder.
#[test]
fn worktree_base_and_owned_path_recorder_bash_suite() {
    let fx = Fx::new();
    let suite = repo_root().join("amplifier-bundle/recipes/tests/test-issue-1808-worktree-base.sh");
    assert!(suite.is_file(), "missing {suite:?}");
    let out = isolated(fx.bin.join("bash"))
        .arg(&suite)
        .env("HOME", &fx.home)
        .env("TMPDIR", &fx.tmp)
        .env("PATH", &fx.bin)
        .env_remove("AMPLIHACK_WORK_ROOT")
        .env_remove("AMPLIHACK_WORK_ROOT_EXPLICIT")
        .env_remove("AMPLIHACK_RUN_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(!stdout.contains("FAIL["), "{stdout}");
}

// ----- issue #2039: hook input handling and teardown sites -------------------

/// A git repository at `<root>/<name>`; returns its path.
fn repo_at(fx: &Fx, name: &str) -> PathBuf {
    let dir = fx.root.join("odd").join(name);
    fs::create_dir_all(&dir).unwrap();
    fx.git_ok(&dir, &["init", "-q", "-b", "main"]);
    dir
}

fn hook_in_run(fx: &Fx, cwd: &Path) -> (PathBuf, PathBuf) {
    let run_id = uuid::Uuid::new_v4().to_string();
    let run_dir = fx.work.join("runs").join(&run_id);
    mkdir_0700(&run_dir);
    let (target, _) = hook_target_in(
        fx,
        cwd,
        &[
            ("AMPLIHACK_WORK_ROOT", &fx.work),
            ("AMPLIHACK_RUN_DIR", &run_dir),
        ],
        &[("AMPLIHACK_RUN_ID", &run_id)],
    );
    (target, run_dir)
}

/// SR: the checkout path is data. Shell syntax, printf directives and an
/// echo option in it are written literally and never run.
#[test]
fn hook_writes_odd_checkout_paths_literally() {
    let fx = Fx::new();
    let repo = repo_at(&fx, "c $(touch pwned) %s -e `touch pwned2` \\n");
    let (target, run_dir) = hook_in_run(&fx, &repo);
    assert_eq!(
        target.file_name().unwrap().to_string_lossy(),
        hook_target_name(&repo)
    );
    assert_eq!(
        fs::read_to_string(run_dir.join("owned-paths")).unwrap(),
        format!(
            "target\t{}\t\tcreated\t{}\n",
            target.display(),
            repo.display()
        )
    );
    for dir in [&repo, &fx.root, &fx.repo] {
        assert!(!dir.join("pwned").exists() && !dir.join("pwned2").exists());
    }
}

/// A checkout path holding a tab cannot be written as one line, so the hook
/// records nothing for it and still runs the build.
#[test]
fn hook_records_nothing_for_a_checkout_path_with_a_tab() {
    let fx = Fx::new();
    let repo = repo_at(&fx, "tab\there");
    let (target, run_dir) = hook_in_run(&fx, &repo);
    assert!(target.is_absolute());
    let owned = fs::read_to_string(run_dir.join("owned-paths")).unwrap_or_default();
    assert!(owned.is_empty(), "{owned:?}");
}

/// `amplihack_run_owned.sh adopt` still matches worktree lines only (its
/// `cut -f1,2` check) when another run's owned-paths holds 5-field lines.
#[test]
fn run_owned_adopt_works_beside_five_field_lines() {
    let fx = Fx::new();
    let helper = repo_root().join("amplifier-bundle/tools/amplihack_run_owned.sh");
    let runs = fx.work.join("runs");
    let earlier = runs.join(uuid::Uuid::new_v4().to_string());
    let later = runs.join(uuid::Uuid::new_v4().to_string());
    mkdir_0700(&earlier);
    mkdir_0700(&later);
    let wt = fx.work.join("worktrees/repo-1a2b3c4d/feat");
    let only_target = fx.work.join("worktrees/repo-1a2b3c4d/other");
    let tgt = fx
        .work
        .join("targets/amplihack-precommit-target-0123456789ab");
    fs::write(
        earlier.join("owned-paths"),
        format!(
            "target\t{t}\t\tcreated\t{o}\nworktree\t{w}\tfeat\ntarget\t{t}\t\treused\t{w}\n",
            t = tgt.display(),
            w = wt.display(),
            o = only_target.display(),
        ),
    )
    .unwrap();
    let adopt = |path: &Path| {
        let out = isolated(fx.bin.join("bash"))
            .arg(&helper)
            .args(["adopt", "worktree"])
            .arg(path)
            .arg("feat")
            .env("PATH", &fx.bin)
            .env("HOME", &fx.home)
            .env("AMPLIHACK_RUN_DIR", &later)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", describe(&out));
    };
    adopt(&wt);
    adopt(&only_target);
    assert_eq!(
        fs::read_to_string(later.join("owned-paths")).unwrap(),
        format!("worktree\t{}\tfeat\n", wt.display()),
        "a checkout named only in a target line is not a recorded worktree"
    );
}

/// A stub `amplihack` on PATH that appends its arguments, one call per line.
fn stub_amplihack(fx: &Fx) -> PathBuf {
    let calls = fx.records.join("amplihack-calls");
    crate::support::exec_stub::write_executable(
        &fx.bin.join("amplihack"),
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 3\n",
            calls.display()
        ),
        0o755,
    );
    calls
}

fn run_bash(fx: &Fx, script: &str, args: &[&Path]) -> Output {
    let out = isolated(fx.bin.join("bash"))
        .arg("-c")
        .arg(script)
        .arg("bash")
        .args(args)
        .current_dir(&fx.root)
        .env("PATH", &fx.bin)
        .env("HOME", &fx.home)
        .env("TMPDIR", &fx.tmp)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("AMPLIHACK_RUN_DIR")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    out
}

/// D6: `cleanup_nested_worktrees` hands each removed worktree to
/// `amplihack cleanup --apply --worktree-target`, and a failing `amplihack`
/// never fails the teardown.
#[test]
fn runtime_artifacts_teardown_calls_worktree_target() {
    let fx = Fx::new();
    let calls = stub_amplihack(&fx);
    let outer = fx.worktree("outer");
    let nested = outer.join("worktrees/inner");
    fx.git_ok(
        &fx.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "inner",
            nested.to_str().unwrap(),
        ],
    );
    let script = repo_root().join("amplifier-bundle/tools/workflow_runtime_artifacts.sh");

    run_bash(
        &fx,
        "source \"$1\" && cleanup_nested_worktrees \"$2\"",
        &[&script, &outer],
    );

    assert!(!nested.exists(), "the nested worktree is removed");
    let calls = fs::read_to_string(&calls).unwrap_or_default();
    assert_eq!(
        calls,
        format!("cleanup --apply --worktree-target {}\n", nested.display()),
        "{calls:?}"
    );
}

/// D6: the sweep's `remove_worktree_and_branch` hands the removed worktree
/// to `amplihack cleanup --apply --worktree-target`.
#[test]
fn worktree_sweep_teardown_calls_worktree_target() {
    let fx = Fx::new();
    let calls = stub_amplihack(&fx);
    let wt = fx.worktree("swept");
    let script = repo_root().join("amplifier-bundle/tools/workflow_worktree_sweep.sh");

    run_bash(
        &fx,
        "source \"$1\" && remove_worktree_and_branch \"$2\" \"$3\" swept",
        &[&script, &fx.repo, &wt],
    );

    assert!(!wt.exists());
    let calls = fs::read_to_string(&calls).unwrap_or_default();
    assert!(
        calls
            .lines()
            .any(|l| l == format!("cleanup --apply --worktree-target {}", wt.display())),
        "{calls:?}"
    );
}

/// D6: without `amplihack` on PATH the teardown still succeeds.
#[test]
fn teardown_without_amplihack_on_path_still_succeeds() {
    let fx = Fx::new();
    let _ = fs::remove_file(fx.bin.join("amplihack"));
    let outer = fx.worktree("outer2");
    let nested = outer.join("worktrees/inner2");
    fx.git_ok(
        &fx.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "inner2",
            nested.to_str().unwrap(),
        ],
    );
    let script = repo_root().join("amplifier-bundle/tools/workflow_runtime_artifacts.sh");
    run_bash(
        &fx,
        "source \"$1\" && cleanup_nested_worktrees \"$2\"",
        &[&script, &outer],
    );
    assert!(!nested.exists());
}

/// D6: every teardown site calls the single entry point, and no shell code
/// deletes a pre-commit target itself.
#[test]
fn teardown_sites_use_worktree_target_and_never_rm_a_target() {
    let call = "amplihack cleanup --apply --worktree-target";
    for (file, sites) in [
        ("amplifier-bundle/recipes/workflow-worktree.yaml", 1),
        ("amplifier-bundle/tools/workflow_worktree_sweep.sh", 2),
        ("amplifier-bundle/tools/workflow_runtime_artifacts.sh", 1),
    ] {
        let text = fs::read_to_string(repo_root().join(file)).unwrap();
        assert!(
            text.matches(call).count() >= sites,
            "{file}: expected {sites} call(s) to `{call}`"
        );
        for line in text.lines().filter(|l| l.contains(call)) {
            assert!(line.contains("|| true"), "{file}: best effort: {line}");
            assert!(line.contains("command -v amplihack"), "{file}: {line}");
            // Failures stay visible in the step log: stderr is never hidden.
            let after = &line[line.find(call).unwrap()..];
            assert!(
                !after.contains("2>&1") && !after.contains("2>/dev/null"),
                "{file}: --worktree-target must not hide stderr: {line}"
            );
        }
    }
    for file in [
        "amplifier-bundle/recipes/workflow-worktree.yaml",
        "amplifier-bundle/tools/workflow_worktree_sweep.sh",
        "amplifier-bundle/tools/workflow_runtime_artifacts.sh",
        "scripts/git-hook-cargo.sh",
    ] {
        let text = fs::read_to_string(repo_root().join(file)).unwrap();
        for line in text.lines() {
            let code = line.trim_start();
            if code.starts_with('#') {
                continue;
            }
            assert!(
                !(code.contains("rm ") && code.contains("precommit-target")),
                "{file}: shell code must not delete a target: {line}"
            );
        }
    }
}

/// The hook never reads `AMPLIHACK_TARGET_LINK_ROOTS`; only Rust does.
#[test]
fn hook_never_reads_target_link_roots() {
    let hook = fs::read_to_string(repo_root().join("scripts/git-hook-cargo.sh")).unwrap();
    assert!(!hook.contains("AMPLIHACK_TARGET_LINK_ROOTS"));
}
