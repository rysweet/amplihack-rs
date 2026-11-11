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
            runtime_dir: None,
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
        runtime_dir: None,
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
    drop(lock);
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

// ----- issue #2291: recorded external runtime folders -----------------------
//
// Contract: `fn external_runtime_check(path: &Path, id: &str, work_root: &Path,
// euid: u32) -> Result<(), String>` in `sweep.rs`. The checks run in this
// order and the first failure is the reason: `not an absolute path`, `wrong
// name`, `is a symbolic link`, `not a folder`, `owned by another user`, `path
// is not canonical`, `inside the work root`. `collect_runs` turns a failure
// into `Item::skip(RunTmp, path, "recorded temp folder refused: <reason>")`.

const EXT_ID: &str = "0123abcd-0000-0000-0000-000000000002";
const EXT_NAME: &str = "amplihack-run-0123abcd0000";

/// A run `EXT_ID` whose `run.json` records `runtime_dir`. An active run uses
/// this test process's own pid and start ticks; a dead one has pid 0 and no
/// end time, as a killed run leaves it.
fn seed_ext(fx: &Fx, active: bool, runtime_dir: Option<&Path>) -> PathBuf {
    let dir = fx.work.runs_dir().join(EXT_ID);
    fs::create_dir_all(&dir).unwrap();
    let pid = std::process::id();
    let report = RunReport {
        schema_version: RUN_SCHEMA_VERSION,
        run_id: EXT_ID.into(),
        recipe: "test".into(),
        pid: if active { pid } else { 0 },
        pid_start_ticks: if active {
            crate::commands::cleanup::run_dir::pid_start_ticks(pid)
        } else {
            None
        },
        cwd: String::new(),
        started_at: now_rfc3339(),
        ended_at: None,
        outcome: "interrupted".into(),
        attempts: Vec::new(),
        steps: Vec::new(),
        log_path: None,
        runtime_dir: runtime_dir.map(|p| p.display().to_string()),
    };
    fs::write(dir.join(RUN_JSON), serde_json::to_vec(&report).unwrap()).unwrap();
    dir
}

/// `<root>/ext/amplihack-run-0123abcd0000`, a private folder holding a file.
fn ext_dir(fx: &Fx) -> PathBuf {
    let dir = dir_with_file(&fx.root.join("ext").join(EXT_NAME));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn check(fx: &Fx, path: &Path, uid: u32) -> Result<(), String> {
    external_runtime_check(path, EXT_ID, &fx.work.path, uid).map_err(|e| e.to_string())
}

fn refused(result: Result<(), String>, reason: &str) {
    let err = result.expect_err("the check must refuse");
    assert!(err.contains(reason), "want {reason:?}, got {err:?}");
}

#[test]
fn external_check_accepts_a_private_folder_with_this_runs_name() {
    let fx = Fx::new();
    let dir = ext_dir(&fx);
    assert_eq!(check(&fx, &dir, euid()), Ok(()));
}

#[test]
fn external_check_refuses_a_relative_path() {
    let fx = Fx::new();
    refused(
        check(&fx, Path::new("ext/amplihack-run-0123abcd0000"), euid()),
        "not an absolute path",
    );
}

#[test]
fn external_check_refuses_another_runs_name() {
    let fx = Fx::new();
    let other = dir_with_file(&fx.root.join("ext/amplihack-run-ffffffffffff"));
    refused(check(&fx, &other, euid()), "wrong name");
    let bare = dir_with_file(&fx.root.join("ext/0123abcd0000"));
    refused(check(&fx, &bare, euid()), "wrong name");
}

/// The link check comes before the folder check, whatever the link names.
#[test]
fn external_check_refuses_a_symbolic_link_before_the_folder_check() {
    let fx = Fx::new();
    let real = dir_with_file(&fx.root.join("real"));
    fs::create_dir_all(fx.root.join("ext")).unwrap();
    let link = fx.root.join("ext").join(EXT_NAME);
    symlink(&real, &link).unwrap();
    refused(check(&fx, &link, euid()), "is a symbolic link");
    fs::remove_file(&link).unwrap();
    symlink(fx.root.join("nowhere"), &link).unwrap();
    refused(check(&fx, &link, euid()), "is a symbolic link");
}

#[test]
fn external_check_refuses_a_file() {
    let fx = Fx::new();
    fs::create_dir_all(fx.root.join("ext")).unwrap();
    let file = fx.root.join("ext").join(EXT_NAME);
    fs::write(&file, "x").unwrap();
    refused(check(&fx, &file, euid()), "not a folder");
}

#[test]
fn external_check_refuses_a_folder_of_another_user() {
    let fx = Fx::new();
    let dir = ext_dir(&fx);
    refused(
        check(&fx, &dir, euid().wrapping_add(1).max(1)),
        "owned by another user",
    );
}

/// S11: a parent swapped for a link makes the recorded path non-canonical.
#[test]
fn external_check_refuses_a_path_through_a_linked_parent() {
    let fx = Fx::new();
    let dir = ext_dir(&fx);
    let lnk = fx.root.join("lnk");
    symlink(dir.parent().unwrap(), &lnk).unwrap();
    refused(
        check(&fx, &lnk.join(EXT_NAME), euid()),
        "path is not canonical",
    );
}

#[test]
fn external_check_refuses_a_folder_inside_the_work_root() {
    let fx = Fx::new();
    let inside = dir_with_file(&fx.work.path.join("tmp").join(EXT_NAME));
    refused(check(&fx, &inside, euid()), "inside the work root");
}

/// AC5: a dead run's recorded folder is removed by the sweep, and the root
/// that holds it is kept.
#[test]
fn recorded_external_folder_of_a_dead_run_is_removed() {
    let fx = Fx::new();
    let dir = ext_dir(&fx);
    seed_ext(&fx, false, Some(&dir));
    let settings = fx.settings(&[Category::RunTmp]);

    let entries = run_sweep(&settings, true).unwrap();

    let e = entry(&entries, &dir);
    assert_eq!(e.category, Category::RunTmp, "{e:?}");
    assert_eq!(e.action, Action::Removed, "{e:?}");
    assert_eq!(e.run_id.as_deref(), Some(EXT_ID));
    assert!(!dir.exists());
    assert!(fx.root.join("ext").is_dir(), "the root is never removed");
}

/// AC8: a live run's recorded folder is kept.
#[test]
fn recorded_external_folder_of_a_live_run_is_kept() {
    let fx = Fx::new();
    let dir = ext_dir(&fx);
    seed_ext(&fx, true, Some(&dir));
    let settings = fx.settings(&[Category::RunTmp]);

    let entries = run_sweep(&settings, true).unwrap();

    let e = entry(&entries, &dir);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert!(e.reason.contains("run is active"), "{e:?}");
    assert!(dir.join("keep-me").exists());
}

/// AC5: a refused recorded path is reported with its reason and left alone.
#[test]
fn refused_recorded_folder_is_reported_and_kept() {
    let fx = Fx::new();
    let real = dir_with_file(&fx.root.join("real"));
    fs::create_dir_all(fx.root.join("ext")).unwrap();
    let link = fx.root.join("ext").join(EXT_NAME);
    symlink(&real, &link).unwrap();
    seed_ext(&fx, false, Some(&link));
    let settings = fx.settings(&[Category::RunTmp]);

    let entries = run_sweep(&settings, true).unwrap();

    let e = entry(&entries, &link);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert_eq!(
        e.reason, "recorded temp folder refused: is a symbolic link",
        "{e:?}"
    );
    assert!(real.join("keep-me").exists(), "the link target survives");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

/// S15: a recorded path that no longer exists produces no item.
#[test]
fn missing_recorded_folder_produces_no_item() {
    let fx = Fx::new();
    let gone = fx.root.join("ext").join(EXT_NAME);
    seed_ext(&fx, false, Some(&gone));
    let settings = fx.settings(&[Category::RunTmp]);

    let items = collect(&settings).unwrap();
    assert!(
        items.iter().all(|i| i.path != gone),
        "no item for a missing path: {items:#?}"
    );
}

/// D9: only a missing path is dropped silently. A path that cannot be
/// inspected for another reason (here ENOTDIR) is reported as refused.
#[test]
fn uninspectable_recorded_folder_is_reported_and_kept() {
    let fx = Fx::new();
    let file = fx.root.join("plain");
    fs::write(&file, "x").unwrap();
    let recorded = file.join(EXT_NAME);
    seed_ext(&fx, false, Some(&recorded));
    let settings = fx.settings(&[Category::RunTmp]);

    let entries = run_sweep(&settings, true).unwrap();

    let e = entry(&entries, &recorded);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert!(
        e.reason.starts_with("recorded temp folder refused: "),
        "{e:?}"
    );
    assert!(file.exists(), "the file in the path survives");
}

/// D10, S12: a `run.json` rewritten between collection and apply, to name
/// another folder or none, means the collected folder is refused.
#[test]
fn recorded_folder_is_refused_when_run_json_changes_before_apply() {
    for rewrite in [Some("other"), None] {
        let fx = Fx::new();
        let dir = ext_dir(&fx);
        let run_dir = seed_ext(&fx, false, Some(&dir));
        let settings = fx.settings(&[Category::RunTmp]);

        let items = collect(&settings).unwrap();
        assert!(
            items
                .iter()
                .any(|i| i.path == dir && matches!(i.op, Op::RemoveRunTmp { .. })),
            "{items:#?}"
        );
        let other = rewrite.map(|name| ext_dir_named(&fx, name));
        seed_ext(&fx, false, other.as_deref());
        assert!(run_dir.join(RUN_JSON).is_file());
        let entries = apply_items(items, &settings, true);

        let e = entry(&entries, &dir);
        assert_eq!(e.action, Action::Skipped, "{rewrite:?}: {e:?}");
        assert!(
            e.reason
                .contains("no longer inside the folders cleanup manages"),
            "{rewrite:?}: {e:?}"
        );
        assert!(dir.join("keep-me").exists(), "{rewrite:?}");
        if let Some(other) = other {
            assert!(other.join("keep-me").exists(), "{rewrite:?}");
        }
    }
}

fn ext_dir_named(fx: &Fx, parent: &str) -> PathBuf {
    let dir = dir_with_file(&fx.root.join(parent).join(EXT_NAME));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// S14: links inside a removed folder are removed, never followed.
#[test]
fn links_inside_a_removed_recorded_folder_are_not_followed() {
    let fx = Fx::new();
    let dir = ext_dir(&fx);
    let outside = dir_with_file(&fx.root.join("outside"));
    symlink(&outside, dir.join("link-out")).unwrap();
    seed_ext(&fx, false, Some(&dir));
    let settings = fx.settings(&[Category::RunTmp]);

    let entries = run_sweep(&settings, true).unwrap();

    assert_eq!(entry(&entries, &dir).action, Action::Removed);
    assert!(!dir.exists());
    assert!(outside.join("keep-me").exists(), "the link target survives");
}

/// S16: `amplihack-run-` is not a `tmp-leftover` pattern, so a folder no
/// `run.json` records is never removed by age.
#[test]
fn per_run_prefix_is_not_a_tmp_leftover_pattern() {
    assert!(!has_prefix(
        Path::new("/x/amplihack-run-0123abcd0000"),
        TMP_PREFIXES
    ));
}
