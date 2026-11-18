//! Unit tests for `sweep.rs` (issue #1808): the apply-time re-checks and the
//! failure paths that must stay visible. `collect` and `apply_items` are run
//! separately so a test can change the filesystem between them (§8 test 3).

use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    work: WorkRoot,
    hook_base: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let work = WorkRoot {
            path: root.join("work"),
            explicit: false,
        };
        let hook_base = root.join("cache/amplihack");
        for dir in [work.runs_dir(), work.targets_dir(), hook_base.clone()] {
            fs::create_dir_all(&dir).unwrap();
        }
        Self {
            _tmp: tmp,
            root,
            work,
            hook_base,
        }
    }

    fn settings(&self, categories: &[Category]) -> Settings {
        Settings {
            work_root: self.work.clone(),
            keep: Duration::ZERO,
            log_age: Duration::MAX,
            archive_dir: self.work.path.join("archive"),
            categories: categories.iter().copied().collect(),
            apply: true,
            report_unrecorded: false,
            deadline: None,
            recipe_log_dir: None,
            codex_sessions_dir: None,
            claude_projects_dir: None,
            hook_cache_base: Some(self.hook_base.clone()),
        }
    }

    /// An ended, dead run that owns `targets` and has a `runtime/` folder.
    fn run(&self, targets: &[&Path]) -> PathBuf {
        let dir = self
            .work
            .runs_dir()
            .join("0123abcd-0000-0000-0000-000000000001");
        fs::create_dir_all(dir.join("runtime/tmp")).unwrap();
        let report = RunReport {
            schema_version: RUN_SCHEMA_VERSION,
            run_id: "0123abcd-0000-0000-0000-000000000001".into(),
            recipe: "test".into(),
            pid: 0,
            pid_start_ticks: None,
            cwd: String::new(),
            started_at: now_rfc3339(),
            ended_at: Some(now_rfc3339()),
            outcome: "success".into(),
            attempts: Vec::new(),
            steps: Vec::new(),
            log_path: None,
        };
        fs::write(dir.join(RUN_JSON), serde_json::to_vec(&report).unwrap()).unwrap();
        let mut owned = Vec::new();
        for t in targets {
            owned.extend(format_owned_line(&OwnedPath {
                kind: OwnedKind::Target,
                path: t.to_path_buf(),
                branch: None,
                target: None,
            }));
        }
        fs::write(dir.join(OWNED_PATHS), owned).unwrap();
        dir
    }
}

fn dir_with_file(path: &Path) -> PathBuf {
    fs::create_dir_all(path).unwrap();
    fs::write(path.join("keep-me"), "data").unwrap();
    path.to_path_buf()
}

fn entry<'a>(entries: &'a [Entry], path: &Path) -> &'a Entry {
    entries
        .iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("no entry for {}: {entries:#?}", path.display()))
}

/// §8 test 3: a recorded path whose symlinked parent is repointed between
/// collection and apply is refused, and the folder it now names survives.
#[test]
fn symlinked_parent_repointed_after_collection_is_refused() {
    let fx = Fx::new();
    dir_with_file(&fx.work.targets_dir().join("foo"));
    let outside = dir_with_file(&fx.root.join("projects/foo"));
    let lnk = fx.root.join("lnk");
    symlink(fx.work.targets_dir(), &lnk).unwrap();
    let recorded = lnk.join("foo");
    fx.run(&[&recorded]);
    let settings = fx.settings(&[Category::RunTarget]);

    let items = collect(&settings).unwrap();
    fs::remove_file(&lnk).unwrap();
    symlink(fx.root.join("projects"), &lnk).unwrap();
    let entries = apply_items(items, &settings, true);

    let e = entry(&entries, &recorded);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert!(e.reason.contains("resolves"), "{e:?}");
    assert!(outside.join("keep-me").exists(), "outside folder deleted");
}

/// A contained item whose canonical path changes to another managed folder
/// is still skipped: only the folder seen at collection may be removed.
#[test]
fn canonical_change_inside_managed_roots_is_skipped() {
    let fx = Fx::new();
    let name = "amplihack-precommit-target-0123456789ab";
    dir_with_file(&fx.work.targets_dir().join(name));
    let other = dir_with_file(&fx.hook_base.join(name));
    let lnk = fx.root.join("lnk");
    symlink(fx.work.targets_dir(), &lnk).unwrap();
    let recorded = lnk.join(name);
    fx.run(&[&recorded]);
    let settings = fx.settings(&[Category::RunTarget]);

    let items = collect(&settings).unwrap();
    fs::remove_file(&lnk).unwrap();
    symlink(&fx.hook_base, &lnk).unwrap();
    let entries = apply_items(items, &settings, true);

    let e = entry(&entries, &recorded);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert!(other.join("keep-me").exists());
    assert!(fx.work.targets_dir().join(name).join("keep-me").exists());
}

/// P0-2: `run.json` that becomes unreadable after collection means skip,
/// for run targets as well as run-tmp.
#[test]
fn unreadable_run_json_at_apply_skips() {
    let fx = Fx::new();
    let target = dir_with_file(&fx.work.targets_dir().join("t1"));
    let run_dir = fx.run(&[&target]);
    let settings = fx.settings(&[Category::RunTarget, Category::RunTmp]);

    let items = collect(&settings).unwrap();
    assert_eq!(
        items.iter().filter(|i| !matches!(i.op, Op::Skip)).count(),
        2,
        "{items:#?}"
    );
    fs::write(run_dir.join(RUN_JSON), "{ not json").unwrap();
    let entries = apply_items(items, &settings, true);

    for path in [target.clone(), run_dir.join("runtime")] {
        let e = entry(&entries, &path);
        assert_eq!(e.action, Action::Skipped, "{e:?}");
        assert!(e.reason.contains("run.json"), "{e:?}");
        assert!(path.exists());
    }
}

/// The happy path still removes, so the refusals above are not vacuous.
#[test]
fn unchanged_items_are_removed() {
    let fx = Fx::new();
    let target = dir_with_file(&fx.work.targets_dir().join("t1"));
    let run_dir = fx.run(&[&target]);
    let settings = fx.settings(&[Category::RunTarget, Category::RunTmp]);

    let entries = run_sweep(&settings, true).unwrap();

    assert_eq!(entry(&entries, &target).action, Action::Removed);
    assert!(!target.exists() && !run_dir.join("runtime").exists());
    assert!(
        !run_dir.join(OWNED_PATHS).exists(),
        "an empty owned-paths is removed (P2-1)"
    );
}

/// P1-5: an unreadable `owned-paths` is reported, not treated as empty, and
/// nothing of that run is deleted.
#[test]
fn unreadable_owned_paths_is_reported_as_skipped() {
    let fx = Fx::new();
    let target = dir_with_file(&fx.work.targets_dir().join("t1"));
    let run_dir = fx.run(&[&target]);
    let owned = run_dir.join(OWNED_PATHS);
    let real = fx.root.join("elsewhere-owned-paths");
    fs::rename(&owned, &real).unwrap();
    symlink(&real, &owned).unwrap();
    let settings = fx.settings(&[Category::RunTarget, Category::UntaggedTarget]);

    let entries = run_sweep(&settings, true).unwrap();

    let e = entry(&entries, &owned);
    assert_eq!(e.action, Action::Skipped, "{entries:#?}");
    assert!(e.reason.starts_with("owned-paths unreadable: "), "{e:?}");
    assert!(target.join("keep-me").exists(), "{entries:#?}");
}

/// P1-5: a corrupt `cleanup-record.json` is kept aside, not overwritten.
#[test]
fn corrupt_cleanup_record_is_renamed_aside() {
    let fx = Fx::new();
    let run_dir = fx.run(&[]);
    fs::write(run_dir.join(CLEANUP_RECORD), "{ corrupt").unwrap();

    write_record(&run_dir, "0123abcd-0000-0000-0000-000000000001", &[]).unwrap();

    let aside: Vec<_> = fs::read_dir(&run_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("cleanup-record.json.corrupt-"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(
        fs::read_to_string(run_dir.join(&aside[0])).unwrap(),
        "{ corrupt"
    );
    let fresh: CleanupRecord =
        serde_json::from_slice(&fs::read(run_dir.join(CLEANUP_RECORD)).unwrap()).unwrap();
    assert_eq!(fresh.sweeps.len(), 1);
}

/// P1-5: a record that exists but cannot be read is an error, not a reset.
#[test]
fn unreadable_cleanup_record_is_an_error() {
    let fx = Fx::new();
    let run_dir = fx.run(&[]);
    let record = run_dir.join(CLEANUP_RECORD);
    symlink(fx.root.join("elsewhere"), &record).unwrap();

    assert!(write_record(&run_dir, "0123abcd-0000-0000-0000-000000000001", &[]).is_err());
    assert!(fs::symlink_metadata(&record).unwrap().is_symlink());
}

/// P1-5: a folder that cannot be listed keeps the target, with its own
/// reason rather than `in use` (review of PR #2152).
#[test]
fn unlistable_folder_is_refused_with_its_own_reason() {
    if euid() == 0 {
        return;
    }
    let fx = Fx::new();
    let dir = dir_with_file(&fx.root.join("locked"));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
    let refusal = cargo_lock_refusal(&dir);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    // The folder's own `.cargo-lock` is the first thing it cannot open.
    let refusal = refusal.expect("an unlistable folder is refused");
    assert!(
        refusal.starts_with(&format!("permission denied: {}", dir.display())),
        "{refusal}"
    );
}

/// A free lock is no refusal; the shared lock taken to check it is released.
#[test]
fn free_cargo_lock_is_no_refusal_and_is_released() {
    let fx = Fx::new();
    let dir = dir_with_file(&fx.root.join("free"));
    fs::create_dir_all(dir.join("debug")).unwrap();
    fs::write(dir.join("debug/.cargo-lock"), "").unwrap();
    assert_eq!(cargo_lock_refusal(&dir), None);
    let file = fs::File::open(dir.join("debug/.cargo-lock")).unwrap();
    use std::os::fd::AsRawFd;
    // SAFETY: the fd is valid for the life of `file`.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(rc, 0, "the check left a lock behind");
}

/// P1-3: the end-of-run sweep only covers run-owned categories.
#[test]
fn end_of_run_categories_are_run_owned_only() {
    assert_eq!(
        END_OF_RUN_CATEGORIES,
        [Category::RunTmp, Category::RunWorktree, Category::RunTarget]
    );
}

/// `--short` would print `heads/feat` here, which `gh pr list --head` does
/// not match; the full ref is stripped of `refs/heads/` instead.
#[test]
fn checked_out_branch_ignores_a_tag_with_the_same_name() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let status = amplihack_git::command_in(dir.path())
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "feat"]);
    git(&["commit", "-q", "--allow-empty", "-m", "x"]);
    git(&["tag", "feat"]);
    assert_eq!(
        checked_out_branch(dir.path()).unwrap().as_deref(),
        Some("feat")
    );
    git(&["checkout", "-q", "--detach"]);
    assert_eq!(checked_out_branch(dir.path()).unwrap(), None);
}

// ----- issue #2039: new-format targets (D4, D7, D8; AC5, AC9) -----------------
//
// Contract for `sweep.rs`:
// - `Op::RemoveTarget { run_dir: PathBuf, shape: target_remove::Shape,
//   after_checkout_gone: Option<PathBuf> }` removes a run target through
//   `target_remove::remove`;
// - new-format (5-field) entries follow the D4 table and never the age rule;
// - report texts: `checkout gone`, `worktree removed by run cleanup`,
//   `worktree kept`, `checkout not owned by this run`,
//   `reused target; checkout still exists`, `checkout state unknown: <error>`,
//   `in use: .cargo-lock is held`.

const T1: &str = "amplihack-precommit-target-0123456789ab";
const T2: &str = "amplihack-precommit-target-abcdef012345";
const RUN_A: &str = "0123abcd-0000-0000-0000-000000000001";
const RUN_B: &str = "0123abcd-0000-0000-0000-000000000002";

fn cargo_target(path: &Path) -> PathBuf {
    fs::create_dir_all(path.join("debug/.fingerprint")).unwrap();
    fs::write(path.join(".rustc_info.json"), "{}").unwrap();
    fs::write(path.join("debug/.cargo-lock"), "").unwrap();
    fs::write(path.join("debug/keep-me"), "data").unwrap();
    path.to_path_buf()
}

fn new_line(target: &Path, origin: &str, checkout: &Path) -> Vec<u8> {
    format!(
        "target\t{}\t\t{origin}\t{}\n",
        target.display(),
        checkout.display()
    )
    .into_bytes()
}

/// A run folder `id` with `owned` as its `owned-paths`. An active run uses
/// this test process's own pid and start ticks.
fn seed(fx: &Fx, id: &str, active: bool, owned: &[u8]) -> PathBuf {
    let dir = fx.work.runs_dir().join(id);
    fs::create_dir_all(&dir).unwrap();
    let pid = std::process::id();
    let report = RunReport {
        schema_version: RUN_SCHEMA_VERSION,
        run_id: id.into(),
        recipe: "test".into(),
        pid: if active { pid } else { 0 },
        pid_start_ticks: if active {
            crate::commands::cleanup::run_dir::pid_start_ticks(pid)
        } else {
            None
        },
        cwd: String::new(),
        started_at: now_rfc3339(),
        ended_at: if active { None } else { Some(now_rfc3339()) },
        outcome: if active { "running" } else { "success" }.into(),
        attempts: Vec::new(),
        steps: Vec::new(),
        log_path: None,
    };
    fs::write(dir.join(RUN_JSON), serde_json::to_vec(&report).unwrap()).unwrap();
    fs::write(dir.join(OWNED_PATHS), owned).unwrap();
    dir
}

fn far_keep(fx: &Fx) -> Settings {
    let mut s = fx.settings(&[Category::RunTarget, Category::RunWorktree]);
    s.keep = Duration::from_secs(87_600 * 3600);
    s
}

/// D4 (b), AC5, AC9: a recorded checkout that no longer exists lets the
/// target go whatever its origin, even with the longest keep period.
#[test]
fn checkout_gone_removes_created_and_reused_targets() {
    let fx = Fx::new();
    let a = cargo_target(&fx.work.targets_dir().join(T1));
    let b = cargo_target(&fx.hook_base.join(T2));
    let mut owned = new_line(&a, "created", &fx.root.join("gone-a"));
    owned.extend(new_line(&b, "reused", &fx.root.join("gone-b")));
    seed(&fx, RUN_A, false, &owned);

    let entries = run_sweep(&far_keep(&fx), true).unwrap();

    for t in [&a, &b] {
        let e = entry(&entries, t);
        assert_eq!(e.action, Action::Removed, "{e:?}");
        assert_eq!(e.category, Category::RunTarget);
        assert_eq!(e.reason, "checkout gone", "{e:?}");
        assert!(!t.exists());
    }
}

/// D4: `reused` while the checkout exists is kept, never aged out.
#[test]
fn reused_target_with_checkout_present_is_kept() {
    let fx = Fx::new();
    let checkout = dir_with_file(&fx.root.join("main-checkout"));
    let t = cargo_target(&fx.work.targets_dir().join(T1));
    seed(&fx, RUN_A, false, &new_line(&t, "reused", &checkout));

    let entries = run_sweep(&fx.settings(&[Category::RunTarget]), true).unwrap();

    let e = entry(&entries, &t);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert_eq!(e.reason, "reused target; checkout still exists");
    assert!(t.join("debug/keep-me").exists());
}

/// D4: `created` for a checkout the run does not own (the main checkout, an
/// adopted worktree) is kept, even with a zero keep period.
#[test]
fn created_target_for_a_checkout_not_owned_is_kept() {
    let fx = Fx::new();
    let checkout = dir_with_file(&fx.root.join("main-checkout"));
    let t = cargo_target(&fx.work.targets_dir().join(T1));
    seed(&fx, RUN_A, false, &new_line(&t, "created", &checkout));

    let entries = run_sweep(&fx.settings(&[Category::RunTarget]), true).unwrap();

    let e = entry(&entries, &t);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert_eq!(e.reason, "checkout not owned by this run");
    assert!(t.join("debug/keep-me").exists());
}

/// SR1: a checkout whose state cannot be read is never "gone".
#[test]
fn checkout_state_unknown_is_skipped() {
    if euid() == 0 {
        return;
    }
    let fx = Fx::new();
    let sealed = fx.root.join("sealed");
    fs::create_dir(&sealed).unwrap();
    let t = cargo_target(&fx.work.targets_dir().join(T1));
    seed(
        &fx,
        RUN_A,
        false,
        &new_line(&t, "created", &sealed.join("wt")),
    );
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000)).unwrap();

    let entries = run_sweep(&fx.settings(&[Category::RunTarget]), true);
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o700)).unwrap();
    let entries = entries.unwrap();

    let e = entry(&entries, &t);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert!(e.reason.starts_with("checkout state unknown: "), "{e:?}");
    assert!(t.join("debug/keep-me").exists());
}

/// D7, AC6: a recorded linked target loses its destination and its link;
/// the old blanket "symbolic link; not followed" is gone for run targets.
#[test]
fn recorded_linked_target_is_removed_with_its_destination() {
    let fx = Fx::new();
    let dest = cargo_target(&fx.hook_base.join(T1));
    let link = fx.work.targets_dir().join(T1);
    symlink(&dest, &link).unwrap();
    seed(
        &fx,
        RUN_A,
        false,
        &new_line(&link, "created", &fx.root.join("gone")),
    );

    let entries = run_sweep(&far_keep(&fx), true).unwrap();

    let e = entry(&entries, &link);
    assert_eq!(e.action, Action::Removed, "{entries:#?}");
    assert_eq!(e.reason, "checkout gone");
    assert!(fs::symlink_metadata(&link).is_err(), "link removed");
    assert!(fs::symlink_metadata(&dest).is_err(), "destination removed");
    assert!(
        !entries
            .iter()
            .any(|e| e.reason.contains("symbolic link; not followed")),
        "{entries:#?}"
    );
}

/// D7, D8: a linked target whose destination is outside the allowed roots
/// is reported and left alone.
#[test]
fn linked_target_outside_allowed_roots_is_reported_and_kept() {
    let fx = Fx::new();
    let dest = cargo_target(&fx.root.join("second-disk").join(T1));
    let link = fx.work.targets_dir().join(T1);
    symlink(&dest, &link).unwrap();
    seed(
        &fx,
        RUN_A,
        false,
        &new_line(&link, "created", &fx.root.join("gone")),
    );

    let entries = run_sweep(&far_keep(&fx), true).unwrap();

    let e = entry(&entries, &link);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert_eq!(e.reason, "link not followed: outside allowed roots");
    assert!(dest.join("debug/keep-me").exists());
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
}

/// D9: `collect_untagged_targets` still never follows a link.
#[test]
fn untagged_target_links_are_still_not_followed() {
    let fx = Fx::new();
    let dest = cargo_target(&fx.hook_base.join(T2));
    let link = fx.work.targets_dir().join(T1);
    symlink(&dest, &link).unwrap();

    let entries = run_sweep(&fx.settings(&[Category::UntaggedTarget]), true).unwrap();

    assert!(
        fs::symlink_metadata(&link).unwrap().is_symlink(),
        "{entries:#?}"
    );
    assert!(dest.join("debug/keep-me").exists(), "{entries:#?}");
}

/// A folder a managed link points at is not an untagged target on its own:
/// removing it alone would leave the link dangling. The same folder with no
/// link to it is removed, so the guard is what keeps it.
#[test]
fn untagged_folder_behind_a_managed_link_is_not_planned() {
    let fx = Fx::new();
    let linked = cargo_target(&fx.hook_base.join(T2));
    symlink(&linked, fx.work.targets_dir().join(T1)).unwrap();
    let control = cargo_target(&fx.hook_base.join("amplihack-precommit-target-fedcba987654"));

    let entries = run_sweep(&fx.settings(&[Category::UntaggedTarget]), true).unwrap();

    assert!(
        !entries.iter().any(|e| e.path == linked),
        "a link destination must not be planned: {entries:#?}"
    );
    assert!(linked.join("debug/keep-me").exists());
    assert_eq!(entry(&entries, &control).action, Action::Removed);
}

/// D9, SR1: a forged 5-field line naming a folder outside the managed roots
/// is skipped even when its checkout is gone.
#[test]
fn forged_line_outside_the_managed_roots_is_skipped() {
    let fx = Fx::new();
    let precious = cargo_target(&fx.root.join("precious").join(T1));
    seed(
        &fx,
        RUN_A,
        false,
        &new_line(&precious, "created", &fx.root.join("gone")),
    );

    let entries = run_sweep(&far_keep(&fx), true).unwrap();

    assert_eq!(entry(&entries, &precious).action, Action::Skipped);
    assert!(precious.join("debug/keep-me").exists());
}

/// D8: a held `.cargo-lock` is reported `in use` and nothing is deleted.
#[test]
fn new_format_target_with_held_cargo_lock_is_in_use() {
    use std::os::fd::AsRawFd;
    let fx = Fx::new();
    let t = cargo_target(&fx.work.targets_dir().join(T1));
    seed(
        &fx,
        RUN_A,
        false,
        &new_line(&t, "created", &fx.root.join("gone")),
    );
    let lock = fs::File::open(t.join("debug/.cargo-lock")).unwrap();
    // SAFETY: the fd is valid while `lock` lives.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );

    let entries = run_sweep(&far_keep(&fx), true).unwrap();

    let e = entry(&entries, &t);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert_eq!(e.reason, "in use: .cargo-lock is held");
    assert!(t.join("debug/keep-me").exists());
    crate::test_support::release_flock(lock, &t.join("debug/.cargo-lock"));
    let entries = run_sweep(&far_keep(&fx), true).unwrap();
    assert_eq!(entry(&entries, &t).action, Action::Removed);
}

/// D4: a target another active run records is kept, even when this run's
/// checkout is gone.
#[test]
fn target_recorded_by_another_active_run_is_kept() {
    let fx = Fx::new();
    let t = cargo_target(&fx.work.targets_dir().join(T1));
    let checkout = dir_with_file(&fx.root.join("main-checkout"));
    seed(
        &fx,
        RUN_A,
        false,
        &new_line(&t, "created", &fx.root.join("gone")),
    );
    seed(&fx, RUN_B, true, &new_line(&t, "reused", &checkout));

    let entries = run_sweep(&far_keep(&fx), true).unwrap();

    assert!(
        entries
            .iter()
            .filter(|e| e.path == t)
            .all(|e| e.action == Action::Skipped),
        "{entries:#?}"
    );
    assert!(t.join("debug/keep-me").exists());
}

/// Legacy 3-field lines keep their #1808 verdict: the ended run's target is
/// removed under the keep period of zero, by the age rule.
#[test]
fn legacy_target_lines_keep_their_verdict() {
    let fx = Fx::new();
    let t = cargo_target(&fx.work.targets_dir().join(T1));
    fx.run(&[&t]);
    let entries = run_sweep(&fx.settings(&[Category::RunTarget]), true).unwrap();
    assert_eq!(entry(&entries, &t).action, Action::Removed);

    let kept = cargo_target(&fx.work.targets_dir().join(T2));
    fx.run(&[&kept]);
    let entries = run_sweep(&far_keep(&fx), true).unwrap();
    assert_eq!(
        entry(&entries, &kept).action,
        Action::Skipped,
        "a legacy line still waits for the keep period"
    );
}

/// The `after_checkout_gone` gate: a rule-(a) target whose worktree is still
/// there at apply time (the worktree was kept) is not removed.
#[test]
fn after_checkout_gone_gate_keeps_the_target_while_the_checkout_exists() {
    let fx = Fx::new();
    let t = cargo_target(&fx.work.targets_dir().join(T1));
    let checkout = dir_with_file(&fx.root.join("wt"));
    let run_dir = seed(&fx, RUN_A, false, &new_line(&t, "created", &checkout));
    let settings = fx.settings(&[Category::RunTarget]);
    let item = |path: &Path| Item {
        category: Category::RunTarget,
        path: path.to_path_buf(),
        canonical: path.to_path_buf(),
        size_bytes: None,
        reason: "worktree removed by run cleanup".into(),
        run_id: Some(RUN_A.into()),
        op: Op::RemoveTarget {
            run_dir: run_dir.clone(),
            shape: target_remove::Shape::Dir,
            after_checkout_gone: Some(checkout.clone()),
        },
    };

    let entries = apply_items(vec![item(&t)], &settings, true);
    let e = entry(&entries, &t);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert_eq!(e.reason, "worktree kept");
    assert!(t.join("debug/keep-me").exists());

    fs::remove_dir_all(&checkout).unwrap();
    let entries = apply_items(vec![item(&t)], &settings, true);
    assert_eq!(entry(&entries, &t).action, Action::Removed);
    assert!(!t.exists());
}

/// `RemoveTarget` checks the shape again at apply time.
#[test]
fn remove_target_refuses_a_shape_change_at_apply() {
    let fx = Fx::new();
    let t = cargo_target(&fx.work.targets_dir().join(T1));
    seed(
        &fx,
        RUN_A,
        false,
        &new_line(&t, "created", &fx.root.join("gone")),
    );
    let settings = far_keep(&fx);

    let items = collect(&settings).unwrap();
    let moved = fx.hook_base.join(T2);
    fs::rename(&t, &moved).unwrap();
    symlink(&moved, &t).unwrap();
    let entries = apply_items(items, &settings, true);

    let e = entry(&entries, &t);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    // The existing canonical re-check may refuse first; either way the
    // folder that moved survives.
    assert!(
        e.reason == "changed since plan" || e.reason.contains("resolves"),
        "{e:?}"
    );
    assert!(moved.join("debug/keep-me").exists());
    assert!(fs::symlink_metadata(&t).unwrap().is_symlink());
}

// ----- issue #2785: git work in a stopped run's runtime folder ----------------
//
// A run stopped on a usage limit is not active: its pid is dead, and
// `ended_at` is set with outcome `failure` (as the three stopped runs
// recorded it) or left empty (interrupted). Its short runtime folder
// `r/<id>` can hold a git tree with the run's only copy of its work.
//
// Contract:
// - `end_of_run_inner(work_root: &WorkRoot) -> Result<EndOfRun>`, with
//   `EndOfRun { entries: Vec<Entry>, warnings: Vec<String> }`, is the code
//   `end_of_run` runs; `end_of_run` prints preservation reasons and failures;
// - `run-tmp` keeps a folder with git work as a `skipped` entry whose reason
//   is `git_work::work_inside`'s, at plan time and again at apply time;
// - a kept-for-work skip alone writes no `cleanup-record.json`;
// - a folder the sweep cannot remove is warned about once: the record keeps
//   the failure with `"failed": true`, and a later sweep that fails on the
//   same path still reports it as `failed` but adds no warning.

use crate::commands::cleanup::git_fixture::{
    Unlock, chmod, clean_pushed_worktree, clone_of, commit, git, has_object, linked_worktree,
    make_dirty, origin_and_clone, push, running_as_root,
};

const STOPPED_RUN: &str = "2785abcd-0000-4000-8000-000000000001";

/// The pid of a process that has exited and been reaped.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("sh")
        .env("PATH", crate::test_support::host_path())
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

/// A stopped run: its run folder and its short runtime folder `r/<id>`,
/// which holds `tmp/` and `artifacts/` as a real run's does.
fn stopped_run(fx: &Fx, ended: bool) -> (PathBuf, PathBuf) {
    let dir = fx.work.runs_dir().join(STOPPED_RUN);
    fs::create_dir_all(&dir).unwrap();
    let report = RunReport {
        schema_version: RUN_SCHEMA_VERSION,
        run_id: STOPPED_RUN.into(),
        recipe: "smart-orchestrator".into(),
        pid: dead_pid(),
        // Never a live process's start time, so a reused pid is not this run.
        pid_start_ticks: Some(1),
        cwd: String::new(),
        started_at: now_rfc3339(),
        ended_at: ended.then(now_rfc3339),
        outcome: if ended { "failure" } else { "interrupted" }.into(),
        attempts: Vec::new(),
        steps: Vec::new(),
        log_path: None,
    };
    fs::write(dir.join(RUN_JSON), serde_json::to_vec(&report).unwrap()).unwrap();
    let runtime = fx.work.short_runtime_dir().join(short_id(STOPPED_RUN));
    fs::create_dir_all(runtime.join("tmp")).unwrap();
    fs::create_dir_all(runtime.join("artifacts")).unwrap();
    (dir, runtime)
}

/// Before a second end-of-run sweep on the same work root: a child that
/// another test forked while the first sweep held the work-root lock shares
/// that lock until it execs, so the second sweep would find it held. This
/// waits for that, as [`crate::test_support::release_flock`] does elsewhere
/// in this file; it is not part of the behaviour under test.
fn first_sweep_lock_released(fx: &Fx) {
    let path = fx.work.lock_path();
    let probe = fs::File::open(&path).unwrap();
    crate::test_support::release_flock(probe, &path);
}

/// R10: a worktree inside the stopped run's folder, of a repository outside
/// it, holds an uncommitted file and an unpushed commit. The end-of-run sweep
/// keeps the folder and exposes why through its normal output.
#[test]
fn end_of_run_keeps_a_stopped_runs_folder_whose_worktree_holds_work() {
    for ended in [true, false] {
        let fx = Fx::new();
        let (_, clone) = origin_and_clone(&fx.work.path.join("src"));
        let (run_dir, runtime) = stopped_run(&fx, ended);
        let wt = linked_worktree(&clone, &runtime.join("tmp/wt"), "feat");
        let sha = commit(&wt, "step-07-tests.rs");
        let file = make_dirty(&wt);

        let outcome = end_of_run_inner(&fx.work).unwrap();

        let e = entry(&outcome.entries, &runtime);
        assert_eq!(e.category, Category::RunTmp, "ended={ended}: {e:?}");
        assert_eq!(e.action, Action::Skipped, "ended={ended}: {e:?}");
        assert_eq!(
            e.reason,
            format!("git tree {wt:?} has uncommitted changes"),
            "ended={ended}"
        );
        assert!(
            file.is_file(),
            "ended={ended}: the uncommitted file is kept"
        );
        assert!(has_object(&wt, &sha), "ended={ended}: the commit is kept");
        assert!(
            outcome
                .warnings
                .iter()
                .any(|line| line.contains(&format!("{runtime:?}"))
                    && line.contains(&format!("{wt:?}"))
                    && line.contains("uncommitted")),
            "ended={ended}: preservation must be visible: {:?}",
            outcome.warnings
        );
        assert!(
            !run_dir.join(CLEANUP_RECORD).exists(),
            "ended={ended}: a kept-for-work skip alone writes no record"
        );
    }
}

/// R10, the same run once its work is committed and pushed: the next
/// end-of-run sweep removes the folder.
#[test]
fn end_of_run_removes_the_folder_once_its_work_is_committed_and_pushed() {
    let fx = Fx::new();
    let (_, clone) = origin_and_clone(&fx.work.path.join("src"));
    let (_, runtime) = stopped_run(&fx, true);
    let wt = linked_worktree(&clone, &runtime.join("tmp/wt"), "feat");
    commit(&wt, "step-07-tests.rs");
    make_dirty(&wt);
    let first = end_of_run_inner(&fx.work).unwrap();
    assert_eq!(entry(&first.entries, &runtime).action, Action::Skipped);

    git(&wt, &["add", "uncommitted.txt"]);
    git(&wt, &["commit", "-q", "-m", "the rest of the work"]);
    push(&wt);
    first_sweep_lock_released(&fx);
    let second = end_of_run_inner(&fx.work).unwrap();

    let e = entry(&second.entries, &runtime);
    assert_eq!(e.action, Action::Removed, "{e:?}");
    assert!(!runtime.exists());
    assert!(second.warnings.is_empty(), "{:?}", second.warnings);
}

/// A clone inside the runtime folder whose origin is outside it, with a
/// commit made before the stop that is not on the origin: the layout of the
/// usage-limit scenario. The commit stays on disk for a continue run.
#[test]
fn end_of_run_keeps_a_clone_inside_the_folder_with_an_unpushed_commit() {
    let fx = Fx::new();
    let (origin, _) = origin_and_clone(&fx.work.path.join("src"));
    let (_, runtime) = stopped_run(&fx, true);
    let work = clone_of(&origin, &runtime.join("tmp/work"));
    let sha = commit(&work, "made-before-the-stop.txt");

    let outcome = end_of_run_inner(&fx.work).unwrap();

    let e = entry(&outcome.entries, &runtime);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert_eq!(e.reason, format!("git tree {work:?} has unpushed commits"));
    assert!(has_object(&work, &sha));
}

/// A repository's lack of a remote must never authorize losing its work.
#[test]
fn end_of_run_keeps_a_self_contained_repository_with_the_folder() {
    let fx = Fx::new();
    let (_, runtime) = stopped_run(&fx, true);
    let fixture = runtime.join("tmp/fixture");
    fs::create_dir_all(&fixture).unwrap();
    git(&fixture, &["init", "-q", "-b", "main"]);
    commit(&fixture, "fixture.txt");
    make_dirty(&fixture);

    let outcome = end_of_run_inner(&fx.work).unwrap();

    assert_eq!(entry(&outcome.entries, &runtime).action, Action::Skipped);
    assert!(fixture.join("uncommitted.txt").is_file());
    assert!(
        outcome
            .warnings
            .iter()
            .any(|line| line.contains(&format!("{fixture:?}")) && line.contains("uncommitted")),
        "{:?}",
        outcome.warnings
    );
}

/// `--apply` searches again just before removal: work made after the plan
/// keeps the folder.
#[test]
fn run_tmp_apply_checks_for_git_work_again() {
    let fx = Fx::new();
    let (_, clone) = origin_and_clone(&fx.work.path.join("src"));
    let (_, runtime) = stopped_run(&fx, true);
    let wt = clean_pushed_worktree(&clone, &runtime.join("tmp/wt"), "feat");
    let settings = fx.settings(&[Category::RunTmp]);

    let items = collect(&settings).unwrap();
    assert!(
        items
            .iter()
            .any(|i| i.path == runtime && matches!(i.op, Op::RemoveRunTmp { .. })),
        "a clean, pushed tree does not stop the plan: {items:#?}"
    );
    make_dirty(&wt);
    let entries = apply_items(items, &settings, true);

    let e = entry(&entries, &runtime);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert!(
        e.reason
            .contains(&format!("git tree {wt:?} has uncommitted changes")),
        "{e:?}"
    );
    assert!(wt.join("uncommitted.txt").is_file());
}

/// A dry run of `amplihack cleanup` shows the same skip.
#[test]
fn run_tmp_dry_run_reports_the_folder_as_kept_for_work() {
    let fx = Fx::new();
    let (_, clone) = origin_and_clone(&fx.work.path.join("src"));
    let (_, runtime) = stopped_run(&fx, false);
    let wt = linked_worktree(&clone, &runtime.join("tmp/wt"), "feat");
    commit(&wt, "local.txt");
    let settings = Settings {
        apply: false,
        ..fx.settings(&[Category::RunTmp])
    };

    let entries = run_sweep(&settings, false).unwrap();

    let e = entry(&entries, &runtime);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert_eq!(
        e.reason,
        format!("git tree {wt:?} has no upstream and commits on no remote branch")
    );
}

/// R11: a folder the end-of-run sweep cannot remove is warned about once.
/// Later sweeps still try it and report it as failed, without a warning, and
/// the run's record holds the failure once.
#[test]
fn end_of_run_warns_once_about_a_folder_it_cannot_remove() {
    if running_as_root() {
        return;
    }
    let fx = Fx::new();
    let (run_dir, runtime) = stopped_run(&fx, true);
    let locked = runtime.join("artifacts/locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(locked.join("fixture.db"), "x").unwrap();
    chmod(&locked, 0o500);
    let _restore = Unlock(locked);
    let quoted = format!("{runtime:?}");

    let first = end_of_run_inner(&fx.work).unwrap();
    assert_eq!(entry(&first.entries, &runtime).action, Action::Failed);
    let named: Vec<&String> = first
        .warnings
        .iter()
        .filter(|w| w.contains(&quoted))
        .collect();
    assert_eq!(named.len(), 1, "{:?}", first.warnings);
    assert!(
        named[0].starts_with("amplihack: warning: end-of-run cleanup could not remove "),
        "{named:?}"
    );

    first_sweep_lock_released(&fx);
    let second = end_of_run_inner(&fx.work).unwrap();
    assert_eq!(
        entry(&second.entries, &runtime).action,
        Action::Failed,
        "a later sweep still tries the folder, once"
    );
    assert!(
        !second.warnings.iter().any(|w| w.contains(&quoted)),
        "{:?}",
        second.warnings
    );

    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(run_dir.join(CLEANUP_RECORD)).unwrap()).unwrap();
    let failed: Vec<&serde_json::Value> = record["sweeps"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["skipped"].as_array().unwrap().iter())
        .filter(|i| i["failed"] == true)
        .collect();
    assert_eq!(failed.len(), 1, "{record:#}");
    assert_eq!(failed[0]["kind"], "run-tmp", "{record:#}");
    assert_eq!(
        failed[0]["path"],
        runtime.display().to_string(),
        "{record:#}"
    );
}

/// A failure recorded by `amplihack cleanup --apply` is not warned about by
/// a later end-of-run sweep: every sweep with `--apply` records failures.
#[test]
fn a_failure_recorded_by_a_manual_sweep_is_not_warned_about_at_end_of_run() {
    if running_as_root() {
        return;
    }
    let fx = Fx::new();
    let (run_dir, runtime) = stopped_run(&fx, true);
    let locked = runtime.join("artifacts/locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(locked.join("fixture.db"), "x").unwrap();
    chmod(&locked, 0o500);
    let _restore = Unlock(locked);

    let manual = run_sweep(&fx.settings(&[Category::RunTmp]), true).unwrap();
    assert_eq!(entry(&manual, &runtime).action, Action::Failed);
    assert!(run_dir.join(CLEANUP_RECORD).is_file());

    let end = end_of_run_inner(&fx.work).unwrap();
    assert_eq!(entry(&end.entries, &runtime).action, Action::Failed);
    assert!(
        !end.warnings
            .iter()
            .any(|w| w.contains(&format!("{runtime:?}"))),
        "{:?}",
        end.warnings
    );
}

/// Either kind of local work keeps an older run while unrelated eligible
/// runtime storage is still removed by the same end-of-run sweep.
#[test]
fn end_of_run_keeps_each_kind_of_work_and_cleans_another_run() {
    for state in ["clean", "dirty", "unpushed", "both"] {
        let fx = Fx::new();
        let (origin, _) = origin_and_clone(&fx.root.join("src"));
        let (_, runtime) = stopped_run(&fx, false);
        let tree = clone_of(&origin, &runtime.join("tmp/work"));
        let sha = if state == "unpushed" || state == "both" {
            commit(&tree, "local.txt")
        } else {
            git(&tree, &["rev-parse", "HEAD"])
        };
        if state == "dirty" || state == "both" {
            make_dirty(&tree);
        }
        let other = fx.run(&[]).join("runtime");

        let result = end_of_run_inner(&fx.work).unwrap();
        assert_eq!(
            entry(&result.entries, &other).action,
            Action::Removed,
            "{state}: {result:?}"
        );
        assert!(!other.exists());
        if state == "clean" {
            assert_eq!(entry(&result.entries, &runtime).action, Action::Removed);
            assert!(!runtime.exists());
        } else {
            let kept = entry(&result.entries, &runtime);
            assert_eq!(kept.action, Action::Skipped, "{state}: {kept:?}");
            assert!(
                has_object(&tree, &sha),
                "{state}: the retained repository remains usable"
            );
            if state != "unpushed" {
                assert!(tree.join("uncommitted.txt").is_file());
            }
            assert!(
                result
                    .warnings
                    .iter()
                    .any(|line| line.contains(&format!("{runtime:?}"))
                        && line.contains(&format!("{tree:?}"))
                        && line.contains(if state == "unpushed" {
                            "unpushed"
                        } else {
                            "uncommitted"
                        })),
                "{state}: preservation must reach normal end-of-run output: {:?}",
                result.warnings
            );
        }
    }
}

/// Legacy run targets also need the recursive check, including changes made
/// after a clean plan was collected.
#[test]
fn run_target_apply_keeps_new_work_in_a_nested_repository() {
    let fx = Fx::new();
    let (origin, _) = origin_and_clone(&fx.root.join("src"));
    let target = cargo_target(&fx.work.targets_dir().join("target-with-work"));
    let tree = clone_of(&origin, &target.join("debug/fixture"));
    fx.run(&[&target]);
    let settings = fx.settings(&[Category::RunTarget]);
    let items = collect(&settings).unwrap();
    assert!(
        items
            .iter()
            .any(|item| item.path == target && !matches!(item.op, Op::Skip)),
        "{items:#?}"
    );
    let sha = commit(&tree, "after-plan.txt");

    let entries = apply_items(items, &settings, true);
    let kept = entry(&entries, &target);
    assert_eq!(kept.action, Action::Skipped, "{kept:?}");
    assert!(kept.reason.contains("unpushed"), "{kept:?}");
    assert!(has_object(&tree, &sha));
}

#[test]
fn end_of_run_removal_warning_survives_a_missing_or_unwritable_run_record() {
    assert!(
        !running_as_root(),
        "run permission coverage as an unprivileged user"
    );
    for unwritable in [false, true] {
        let fx = Fx::new();
        let (run_dir, runtime) = stopped_run(&fx, true);
        let locked = runtime.join("artifacts/locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("only-copy"), "data").unwrap();
        chmod(&locked, 0o500);
        let _restore = Unlock(locked.clone());
        let quoted = format!("{runtime:?}");
        let first = end_of_run_inner(&fx.work).unwrap();
        assert_eq!(entry(&first.entries, &runtime).action, Action::Failed);
        assert_eq!(
            first
                .warnings
                .iter()
                .filter(|line| line.contains(&quoted))
                .count(),
            1
        );
        let persisted = fx.work.path.join("cleanup-failures.json");
        assert!(
            persisted.is_file(),
            "warning state must live outside the failed folder and its run record"
        );
        // Simulate a lost record; a read-only run folder additionally prevents
        // the next sweep from replacing it. The runtime is stored separately.
        if run_dir.join(CLEANUP_RECORD).exists() {
            fs::remove_file(run_dir.join(CLEANUP_RECORD)).unwrap();
        }
        let _restore_run = Unlock(run_dir.clone());
        if unwritable {
            chmod(&run_dir, 0o500);
        }
        let other = fx.run(&[]).join("runtime");
        first_sweep_lock_released(&fx);
        let second =
            end_of_run_inner(&fx.work).expect("record persistence cannot block unrelated cleanup");
        assert_eq!(entry(&second.entries, &runtime).action, Action::Failed);
        assert_eq!(entry(&second.entries, &other).action, Action::Removed);
        assert!(!other.exists());
        assert!(
            !second
                .warnings
                .iter()
                .any(|line| line.contains("could not remove") && line.contains(&quoted)),
            "same path was warned about twice: {:?}",
            second.warnings
        );
        // Successful removal clears obsolete suppression state.
        chmod(&run_dir, 0o700);
        chmod(&locked, 0o700);
        first_sweep_lock_released(&fx);
        let third = end_of_run_inner(&fx.work).unwrap();
        assert_eq!(entry(&third.entries, &runtime).action, Action::Removed);
        if persisted.exists() {
            let record = fs::read_to_string(&persisted).unwrap();
            assert!(!record.contains(runtime.to_str().unwrap()), "{record}");
        }
    }
}

#[test]
fn owned_worktree_apply_preserves_new_work_in_an_ignored_nested_clone() {
    let fx = Fx::new();
    let (origin, main) = origin_and_clone(&fx.root.join("src"));
    let wt = clean_pushed_worktree(&main, &fx.work.worktrees_dir().join("repo/task"), "task");
    // A detached, remotely backed checkout uses the existing no-PR rule;
    // this filesystem test must never invoke the host's GitHub CLI.
    git(&wt, &["checkout", "--detach", "-q"]);
    let common = PathBuf::from(git(
        &wt,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ));
    fs::write(common.join("info/exclude"), "nested/\n").unwrap();
    let nested = clone_of(&origin, &wt.join("nested"));
    let owned = format_owned_line(&OwnedPath {
        kind: OwnedKind::Worktree,
        path: wt.clone(),
        branch: None,
        target: None,
    });
    seed(&fx, STOPPED_RUN, false, &owned);
    let settings = fx.settings(&[Category::RunWorktree]);
    let items = collect(&settings).unwrap();
    assert!(
        items
            .iter()
            .any(|item| item.path == wt && matches!(item.op, Op::RemoveWorktree { .. })),
        "{items:#?}"
    );
    let sha = commit(&nested, "after-plan.txt");
    assert!(git(&wt, &["status", "--porcelain"]).is_empty());

    let entries = apply_items(items, &settings, true);
    let kept = entry(&entries, &wt);
    assert_eq!(kept.action, Action::Skipped, "{kept:?}");
    assert!(
        kept.reason.contains(&format!("{nested:?}")) && kept.reason.contains("unpushed"),
        "{kept:?}"
    );
    assert!(has_object(&nested, &sha));
    assert!(
        !git(&wt, &["rev-parse", "HEAD"]).is_empty(),
        "backing metadata remains usable"
    );
}
