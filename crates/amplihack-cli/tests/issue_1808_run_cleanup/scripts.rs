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
    let mut c = isolated(fx.bin.join("bash"));
    c.arg(repo_root().join("scripts/git-hook-cargo.sh"))
        .args(["sh", "-c", "printf '%s' \"$CARGO_TARGET_DIR\""])
        .current_dir(&fx.repo)
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

/// AC6: inside a run, the hook target is
/// `<work>/targets/amplihack-precommit-target-<hash>-run-<run id>`, is
/// tagged once it exists, and is recorded in `owned-paths`.
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
    let hash = name
        .strip_prefix("amplihack-precommit-target-")
        .and_then(|rest| rest.strip_suffix(&format!("-run-{run_id}")))
        .unwrap_or_else(|| panic!("unexpected name {name}"));
    assert_eq!(hash.len(), 12, "{name}");
    assert_left_for_cargo_then_tagged(&target, run);
    let owned = fs::read_to_string(run_dir.join("owned-paths")).expect("owned-paths written");
    assert_eq!(
        owned
            .matches(&format!("target\t{}\t", target.display()))
            .count(),
        1,
        "recorded once across both hook runs: {owned:?}"
    );
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

/// A run id that does not match `^[0-9a-f-]{1,64}$` adds no suffix and is
/// never used in a path.
#[test]
fn hook_ignores_invalid_run_ids() {
    let fx = Fx::new();
    for bad in ["../../escape", "ABC", "a b", &"a".repeat(65)] {
        let (target, _) = hook_target(
            &fx,
            &[("AMPLIHACK_WORK_ROOT", &fx.work)],
            &[("AMPLIHACK_RUN_ID", bad)],
        );
        let name = target.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!name.contains("-run-"), "{bad:?} gave {name}");
        assert_eq!(target.parent(), Some(fx.work.join("targets").as_path()));
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
