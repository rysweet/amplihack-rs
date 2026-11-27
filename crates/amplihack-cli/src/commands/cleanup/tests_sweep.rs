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
            pull_requests: PullRequests::LocalOnly,
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

/// Issue #2935. Where running processes cannot be seen, the runner keeps a
/// run's runtime folder and no sweep removes it. The dry run is then the list
/// of what waits to be deleted by hand: it names each such folder, says why
/// it is kept in words that do not claim a process uses it, and gives its
/// size. Nothing is removed, with or without `--apply`.
#[test]
fn a_dry_run_lists_a_runtime_folder_it_cannot_judge_with_its_size_and_why() {
    use super::super::procs::{FakeProcesses, REASON_IN_USE, REASON_USE_UNKNOWN};
    let fx = Fx::new();
    let runtime = fx.run(&[]).join("runtime");
    fs::write(runtime.join("tmp/left-behind"), vec![b'x'; 4096]).unwrap();
    let unseen = || Some(ProcScan::of(FakeProcesses::unseen().as_ref()));

    let mut settings = fx.settings(&[Category::RunTmp]);
    settings.apply = false;
    let listed = apply_items(
        collect_seeing(&settings, unseen()).unwrap(),
        &settings,
        true,
    );
    let kept = entry(&listed, &runtime);
    assert_eq!(kept.action, Action::Skipped);
    assert_eq!(kept.reason, REASON_USE_UNKNOWN);
    assert_eq!(
        kept.reason,
        "running processes cannot be seen on this system"
    );
    assert!(
        kept.size_bytes.is_some_and(|size| size >= 4096),
        "the list gives the folder's size: {kept:?}"
    );

    // The sweep that ends every run applies. It keeps the folder for the
    // same reason and does not walk it for a size.
    settings.apply = true;
    let swept = apply_items(
        collect_seeing(&settings, unseen()).unwrap(),
        &settings,
        true,
    );
    let kept = entry(&swept, &runtime);
    assert_eq!(
        (kept.action, kept.reason.as_str()),
        (Action::Skipped, REASON_USE_UNKNOWN)
    );
    assert_eq!(kept.size_bytes, None);
    assert!(
        runtime.join("tmp/left-behind").is_file(),
        "nothing is removed"
    );

    // A folder a process is seen to hold keeps the other reason, and no size.
    let held = FakeProcesses::seen();
    held.hold(&runtime.join("tmp"));
    settings.apply = false;
    let scan = Some(ProcScan::of(held.as_ref()));
    let busy = apply_items(collect_seeing(&settings, scan).unwrap(), &settings, true);
    let kept = entry(&busy, &runtime);
    assert_eq!(
        (kept.action, kept.reason.as_str()),
        (Action::Skipped, REASON_IN_USE)
    );
    assert_eq!(kept.reason, "in use by a running process");
    assert_eq!(kept.size_bytes, None);

    // Seen and free: the same folder is a candidate, as before.
    let free = Some(ProcScan::of(FakeProcesses::seen().as_ref()));
    let plan = apply_items(collect_seeing(&settings, free).unwrap(), &settings, true);
    assert_eq!(entry(&plan, &runtime).action, Action::WouldRemove);
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

/// The full ref supplies the exact head branch for pull request matching,
/// even when a tag shares the branch name.
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

// ----- issue #2785: a folder that holds unsaved Git work is kept ----------------
//
// Before cleanup removes a folder, it judges whether the folder holds a Git
// work tree with uncommitted changes, or commits that are not on its
// upstream. If it does, the folder is kept and one line says so on stderr.
//
// Contract for `sweep.rs` (no new module and no new file):
//
// - `const MAX_WORK_TREES: usize = 8`: how many `.git` entries that pass the
//   structural filter are judged in one folder; a ninth keeps the folder.
// - `pub(super) struct KeepReason { pub worktree: PathBuf, .. }` with
//   `Display`. The text is prefix-free and is also the report's `reason` and
//   the text of the apply-time refusal:
//     * `<reason>` when `worktree` is the folder that was judged;
//     * `work tree <worktree:?>: <reason>` when it is a folder below it.
//   `<reason>` is `has uncommitted changes`, or the text of
//   [`unpushed_reason`] (`has unpushed commits`, `has no upstream and
//   commits on no remote branch`), or `cannot check for unpushed commits:
//   <detail>`. No character that [`needs_escape`] names appears in it (the
//   controls, and the format and separator characters), and it never
//   contains `still in use`.
// - `pub(super) fn judge_unsaved_work(dir: &Path, owner: u32) ->
//   Option<KeepReason>`: the judgement with no output. `owner` is the uid a
//   folder and its `.git` must belong to; production passes `euid()`.
// - `pub(super) fn keep_and_report_unsaved_work(dir: &Path) ->
//   Option<KeepReason>`: `judge_unsaved_work(dir, euid())`, and when it keeps
//   the folder, the one message (`amplihack: leaving <dir:?> in place:
//   <KeepReason>` on stderr, and a `tracing::warn!`). The stderr line is
//   asserted from the binary in `tests/issue_1808_run_cleanup/unsaved_work.rs`.
//
// The judgement is a funnel. A folder with no `.git` in it, in a folder
// directly inside it, or in a folder two levels inside it is not kept, and no
// Git command is run for it (that cost property holds by construction and is
// not tested: a fake `git` on the process-wide PATH would make parallel tests
// unsound). Each `.git` found must belong to the folder that holds it, so a
// folder that only sits inside some enclosing repository is never judged by
// that repository. What cannot be settled is kept, never deleted.

use crate::test_support::origin_clones::{self as repos, Scratch, make_dirty, make_unpushed};

/// The judgement, which must keep `dir`.
fn kept(dir: &Path) -> KeepReason {
    keep_and_report_unsaved_work(dir)
        .unwrap_or_else(|| panic!("{} was not kept, but holds unsaved work", dir.display()))
}

/// The text for a work tree below the judged folder.
fn below(worktree: &Path, reason: &str) -> String {
    format!("work tree {worktree:?}: {reason}")
}

const UNCOMMITTED: &str = "has uncommitted changes";
const UNPUSHED: &str = "has unpushed commits";
const CANNOT_CHECK: &str = "cannot check for unpushed commits: ";

/// Gives a folder back owner access when dropped, so a failed test leaves
/// nothing `TempDir` cannot delete. List a parent before its child.
struct Unlock(Vec<PathBuf>);

impl Drop for Unlock {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        }
    }
}

#[test]
fn the_work_tree_cap_is_eight() {
    // The documentation says eight; a change here is a change there.
    assert_eq!(MAX_WORK_TREES, 8);
}

/// A `git status` that Git declines to answer, such as one that timed out, is
/// not changes anybody made, and the line must not say it is. The boolean the
/// removal paths read still counts it as dirty, so the folder is still kept.
#[test]
fn a_git_status_that_fails_gives_its_own_words_and_not_uncommitted_changes() {
    let s = Scratch::new();
    let broken = s.root.join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join(".git"), "gitdir: /nonexistent\n").unwrap();

    let detail = git_dirty_state(&broken).expect_err("Git cannot answer for this folder");
    assert!(!detail.is_empty(), "Git's own words follow the reason");
    assert!(
        !detail.contains(UNCOMMITTED),
        "what Git said, not a claim of changes: {detail}"
    );
    assert!(
        git_has_dirty_state(&broken),
        "the removal paths still treat it as dirty, so the folder stays"
    );
}

#[test]
fn a_folder_with_no_git_entry_is_not_kept() {
    let s = Scratch::new();
    let dir = s.root.join("plain");
    fs::create_dir_all(dir.join("a/b/c")).unwrap();
    fs::write(dir.join("a/b/c/file"), "x").unwrap();
    fs::write(dir.join("top"), "x").unwrap();

    assert!(keep_and_report_unsaved_work(&dir).is_none());
}

#[test]
fn a_folder_that_is_gone_or_is_a_file_is_not_kept() {
    let s = Scratch::new();
    assert!(keep_and_report_unsaved_work(&s.root.join("missing")).is_none());
    let file = s.root.join("a-file");
    fs::write(&file, "x").unwrap();
    assert!(keep_and_report_unsaved_work(&file).is_none());
}

/// The one that matters most: `git rev-parse` walks up. A run folder under a
/// work root that sits inside a repository must be judged by what is in it,
/// not by the enclosing repository, or nothing is ever removed.
#[test]
fn a_folder_inside_a_dirty_repository_with_no_git_entry_of_its_own_is_not_kept() {
    let s = Scratch::new();
    let outer = s.root.join("outer");
    s.origin.clone_to(&outer);
    make_dirty(&outer);
    make_unpushed(&outer);
    let inner = outer.join("work/run");
    fs::create_dir_all(inner.join("tmp")).unwrap();
    fs::write(inner.join("tmp/file"), "x").unwrap();
    // The trap is real: asked from inside, Git reports the enclosing
    // repository's changes.
    assert!(git_has_dirty_state(&inner));

    assert!(keep_and_report_unsaved_work(&inner).is_none());
}

#[test]
fn a_clean_pushed_clone_is_not_kept_at_each_depth_the_search_reaches() {
    let s = Scratch::new();
    for depth in 0..=2 {
        let base = s.root.join(format!("depth{depth}"));
        let mut clone = base.clone();
        for level in ["a", "b"].iter().take(depth) {
            clone.push(level);
        }
        s.origin.clone_to(&clone);

        assert!(
            keep_and_report_unsaved_work(&base).is_none(),
            "a clean pushed clone at {clone:?} must not keep {base:?}"
        );
    }
}

#[test]
fn a_dirty_clone_in_a_folder_below_names_the_work_tree() {
    let s = Scratch::new();
    let outer = s.root.join("outer");
    let clone = outer.join("clone");
    s.origin.clone_to(&clone);
    make_dirty(&clone);

    let keep = kept(&outer);

    assert_eq!(keep.worktree, clone);
    let text = keep.to_string();
    assert_eq!(text, below(&clone, UNCOMMITTED));
    assert!(!text.contains("still in use"), "{text}");
}

#[test]
fn an_unpushed_commit_in_the_folder_itself_keeps_it_without_naming_a_work_tree() {
    let s = Scratch::new();
    let clone = s.root.join("clone");
    s.origin.clone_to(&clone);
    make_unpushed(&clone);

    let keep = kept(&clone);

    assert_eq!(keep.worktree, clone);
    assert_eq!(keep.to_string(), UNPUSHED);
}

#[test]
fn a_repository_with_commits_and_no_upstream_is_kept_with_its_own_reason() {
    let s = Scratch::new();
    let repo = s.root.join("local-only");
    fs::create_dir_all(&repo).unwrap();
    repos::git(&repo, &["init", "-q", "-b", "main"]);
    repos::git(&repo, &["commit", "-q", "--allow-empty", "-m", "only here"]);

    assert_eq!(
        kept(&repo).to_string(),
        "has no upstream and commits on no remote branch"
    );
}

/// The order the work tree is judged in is the order the reasons are given in:
/// uncommitted changes first.
#[test]
fn uncommitted_changes_are_reported_before_unpushed_commits() {
    let s = Scratch::new();
    let clone = s.root.join("clone");
    s.origin.clone_to(&clone);
    make_unpushed(&clone);
    make_dirty(&clone);

    assert_eq!(kept(&clone).to_string(), UNCOMMITTED);
}

#[test]
fn the_search_reaches_two_levels_below_the_folder_and_no_further() {
    let s = Scratch::new();

    let two = s.root.join("two");
    let two_clone = two.join("a/b");
    s.origin.clone_to(&two_clone);
    make_dirty(&two_clone);
    assert_eq!(kept(&two).worktree, two_clone);

    let three = s.root.join("three");
    let three_clone = three.join("a/b/c");
    s.origin.clone_to(&three_clone);
    make_dirty(&three_clone);
    assert!(
        keep_and_report_unsaved_work(&three).is_none(),
        "a repository three levels down is outside the search"
    );
}

/// A linked worktree has a `.git` that is a file, which is what a run's
/// worktrees look like.
#[test]
fn a_linked_worktree_with_a_git_file_is_judged() {
    let s = Scratch::new();
    let main = s.root.join("main-checkout");
    s.origin.clone_to(&main);
    let outer = s.root.join("outer");
    let linked = outer.join("wt");
    fs::create_dir_all(&outer).unwrap();
    repos::git_paths(
        &main,
        &["worktree", "add", "-q", "-b", "wt-branch"],
        &[&linked],
    );
    assert!(linked.join(".git").is_file());
    assert!(keep_and_report_unsaved_work(&outer).is_none(), "clean");

    make_dirty(&linked);

    let keep = kept(&outer);
    assert_eq!(keep.worktree, linked);
    assert_eq!(keep.to_string(), below(&linked, UNCOMMITTED));
}

/// When several work trees qualify, the answer does not depend on the order
/// the file system lists them in: shallowest first, then by path.
#[test]
fn the_first_work_tree_with_unsaved_work_is_reported_in_a_fixed_order() {
    let s = Scratch::new();
    let outer = s.root.join("outer");
    let deep = outer.join("x/deep");
    let b = outer.join("b");
    let a = outer.join("a");
    for clone in [&deep, &b, &a] {
        s.origin.clone_to(clone);
    }
    make_unpushed(&deep);
    make_dirty(&b);
    make_dirty(&a);

    // `a` and `b` are both a level shallower than `deep`; `a` sorts first.
    assert_eq!(kept(&outer).worktree, a);
    fs::remove_file(a.join("unsaved.txt")).unwrap();
    assert_eq!(kept(&outer).worktree, b);
    fs::remove_file(b.join("unsaved.txt")).unwrap();
    assert_eq!(kept(&outer).worktree, deep);
}

/// A `.git` that says nothing about being a repository is not one: an empty
/// folder, or one with no `HEAD`, is ignored, so a stray one cannot keep a
/// folder for ever.
#[test]
fn a_git_folder_without_a_head_is_not_a_repository() {
    let s = Scratch::new();
    let empty = s.root.join("empty");
    fs::create_dir_all(empty.join(".git")).unwrap();
    assert!(keep_and_report_unsaved_work(&empty).is_none());

    let no_head = s.root.join("no-head");
    fs::create_dir_all(no_head.join(".git")).unwrap();
    fs::write(no_head.join(".git/config"), "[core]\n").unwrap();
    assert!(keep_and_report_unsaved_work(&no_head).is_none());
}

/// The other direction, which is the dangerous one: a `.git` that looks like
/// a repository and that Git refuses is kept, never read as "not a
/// repository" and deleted.
#[test]
fn a_git_file_pointing_nowhere_is_kept_because_it_cannot_be_checked() {
    let s = Scratch::new();
    let dir = s.root.join("broken");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(".git"), "gitdir: /nonexistent\n").unwrap();

    let keep = kept(&dir);

    assert_eq!(keep.worktree, dir);
    let text = keep.to_string();
    assert!(text.starts_with(CANNOT_CHECK), "{text}");
    assert!(text.len() > CANNOT_CHECK.len(), "a detail follows: {text}");
}

#[test]
fn a_corrupt_git_folder_with_a_head_is_kept_because_it_cannot_be_checked() {
    let s = Scratch::new();
    let dir = s.root.join("corrupt");
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::write(dir.join(".git/HEAD"), "garbage\n").unwrap();

    let text = kept(&dir).to_string();

    assert!(text.starts_with(CANNOT_CHECK), "{text}");
}

#[test]
fn a_broken_git_file_below_the_folder_is_kept_and_names_its_folder() {
    let s = Scratch::new();
    let outer = s.root.join("outer");
    let child = outer.join("child");
    fs::create_dir_all(&child).unwrap();
    fs::write(child.join(".git"), "gitdir: /nonexistent\n").unwrap();

    let keep = kept(&outer);

    assert_eq!(keep.worktree, child);
    assert!(
        keep.to_string()
            .starts_with(&format!("work tree {child:?}: {CANNOT_CHECK}")),
        "{keep}"
    );
}

/// `trim()` or a lossy decode would turn `clone ` into `clone`, or one with a
/// byte that is not UTF-8 into a different name; the answer would then be a
/// different folder than the one asked about, read as "not this repository",
/// and the work in it would be deleted.
#[test]
fn a_dirty_clone_whose_name_ends_in_a_space_is_kept() {
    let s = Scratch::new();
    let outer = s.root.join("outer");
    let clone = outer.join("clone ");
    s.origin.clone_to(&clone);
    make_dirty(&clone);

    assert_eq!(kept(&outer).worktree, clone);
}

#[test]
#[cfg_attr(
    target_vendor = "apple",
    ignore = "APFS refuses a file name that is not valid UTF-8 (EILSEQ), so the input cannot be made"
)]
fn a_dirty_clone_whose_name_is_not_utf8_is_kept() {
    let s = Scratch::new();
    let outer = s.root.join("outer");
    let clone = outer.join(crate::test_support::non_utf8_name::non_utf8(b"clone\xff"));
    s.origin.clone_to(&clone);
    make_dirty(&clone);

    assert_eq!(kept(&outer).worktree, clone);
}

/// A name with a newline, a terminal escape or a right-to-left override
/// cannot forge a second line, send an escape sequence or reorder what is
/// shown, and the text is the same everywhere it is shown (stderr, the
/// table, JSON).
#[test]
fn a_hostile_folder_name_cannot_break_the_message_into_lines_or_send_escapes() {
    let s = Scratch::new();
    let outer = s.root.join("outer");
    let clone = outer.join("we\nird\u{1b}[2J\u{202e}name");
    s.origin.clone_to(&clone);
    make_dirty(&clone);

    let text = kept(&outer).to_string();

    assert!(!text.chars().any(needs_escape), "{text:?}");
    assert!(text.starts_with("work tree \""), "{text}");
    assert!(text.ends_with(&format!(": {UNCOMMITTED}")), "{text}");
}

/// Git's own error names the `.git` it refused, and so carries the folder's
/// name with it, escape included. The detail after `cannot check for unpushed
/// commits:` is escaped the same way as a path: controls, and the format and
/// separator characters that reorder or hide the text around them.
#[test]
fn a_git_error_that_carries_an_unsafe_character_is_escaped_too() {
    let s = Scratch::new();
    let dir = s.root.join("broken\u{1b}[31m\u{202e}\u{200b}-escape");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(".git"), "gitdir: /nonexistent\n").unwrap();

    let text = kept(&dir).to_string();

    assert!(text.starts_with(CANNOT_CHECK), "{text}");
    assert!(
        text.len() > CANNOT_CHECK.len(),
        "Git's error follows the prefix: {text:?}"
    );
    assert!(!text.chars().any(needs_escape), "{text:?}");
    assert!(
        text.contains("\\u{202e}"),
        "the override is written out: {text}"
    );
}

/// Git refuses to run in a repository another user owns, and a nested one
/// planted by another user in a folder this one owns could name a filter to
/// run. Not owned by this user means kept, with Git never asked.
#[test]
fn a_clean_clone_owned_by_another_user_is_kept_without_asking_git() {
    let s = Scratch::new();
    let clone = s.root.join("clone");
    s.origin.clone_to(&clone);
    assert!(
        judge_unsaved_work(&clone, euid()).is_none(),
        "control: owned by this user and clean"
    );

    let foreign = euid().wrapping_add(1);
    let keep = judge_unsaved_work(&clone, foreign).expect("another user's folder is kept");

    let text = keep.to_string();
    assert!(text.starts_with(CANNOT_CHECK), "{text}");
    assert!(text.contains("not owned by the current user"), "{text}");
}

#[test]
fn a_foreign_user_matters_only_for_a_folder_that_has_a_git_entry() {
    let s = Scratch::new();
    let plain = s.root.join("plain");
    fs::create_dir_all(plain.join("a")).unwrap();

    assert!(judge_unsaved_work(&plain, euid().wrapping_add(1)).is_none());
}

/// An unreadable folder cannot be shown to hold no `.git`. Kept, because
/// "I could not look" is not "there is nothing".
#[test]
fn a_folder_that_cannot_be_opened_is_kept_not_assumed_empty() {
    if euid() == 0 {
        eprintln!(
            "a_folder_that_cannot_be_opened_is_kept_not_assumed_empty: running as root, which \
             ignores folder modes; this run does not exercise it"
        );
        return;
    }
    let s = Scratch::new();

    // The `.git` is inside a folder directly inside it, at mode 000: the
    // probe for the `.git` fails.
    let child_case = s.root.join("child-locked");
    let child = child_case.join("child");
    s.origin.clone_to(&child);
    make_dirty(&child);
    let _unlock_child = Unlock(vec![child.clone()]);
    fs::set_permissions(&child, fs::Permissions::from_mode(0o000)).unwrap();
    let text = kept(&child_case).to_string();
    assert!(text.contains(CANNOT_CHECK), "{text}");

    // The `.git` is inside a folder two levels inside it, at mode 000.
    let grand_case = s.root.join("grandchild-locked");
    let middle = grand_case.join("a");
    let grandchild = middle.join("b");
    s.origin.clone_to(&grandchild);
    make_dirty(&grandchild);
    let _unlock_grand = Unlock(vec![grandchild.clone()]);
    fs::set_permissions(&grandchild, fs::Permissions::from_mode(0o000)).unwrap();
    let text = kept(&grand_case).to_string();
    assert!(text.contains(CANNOT_CHECK), "{text}");

    // The folder that would hold the work tree cannot be listed.
    let list_case = s.root.join("listing-locked");
    let unlistable = list_case.join("a");
    let hidden = unlistable.join("b");
    s.origin.clone_to(&hidden);
    make_dirty(&hidden);
    let _unlock_list = Unlock(vec![unlistable.clone(), hidden.clone()]);
    fs::set_permissions(&unlistable, fs::Permissions::from_mode(0o000)).unwrap();
    let text = kept(&list_case).to_string();
    assert!(text.contains(CANNOT_CHECK), "{text}");
}

/// Cleanup does not follow a link: a link to a dirty repository is neither
/// judged nor counted, and a folder that is itself a link is not judged
/// (removal refuses a link anyway).
#[test]
fn links_are_never_followed() {
    let s = Scratch::new();
    let dirty = s.root.join("dirty-elsewhere");
    s.origin.clone_to(&dirty);
    make_dirty(&dirty);

    let outer = s.root.join("outer");
    fs::create_dir_all(&outer).unwrap();
    symlink(&dirty, outer.join("link-to-dirty")).unwrap();
    assert!(keep_and_report_unsaved_work(&outer).is_none());

    let linked_folder = s.root.join("the-folder-is-a-link");
    symlink(&dirty, &linked_folder).unwrap();
    assert!(keep_and_report_unsaved_work(&linked_folder).is_none());
}

/// Eight work trees are judged; the ninth keeps the folder, because nine
/// checks of up to a minute each are more than a run's end should spend.
#[test]
fn eight_clean_work_trees_are_all_judged_and_a_ninth_keeps_the_folder() {
    let s = Scratch::new();

    let at_cap = s.root.join("at-cap");
    for i in 0..MAX_WORK_TREES {
        s.origin.clone_to(&at_cap.join(format!("w{i}")));
    }
    assert!(
        keep_and_report_unsaved_work(&at_cap).is_none(),
        "eight clean work trees keep nothing"
    );

    let over = s.root.join("over-cap");
    for i in 0..=MAX_WORK_TREES {
        s.origin.clone_to(&over.join(format!("w{i}")));
    }
    let keep = kept(&over);
    assert_eq!(keep.worktree, over);
    assert_eq!(
        keep.to_string(),
        format!("{CANNOT_CHECK}more work trees than can be checked")
    );
}

/// Only a `.git` that passes the filter counts toward the cap: an empty
/// `.git` folder is ignored, and a link is not followed.
#[test]
fn entries_that_are_ignored_do_not_count_toward_the_cap() {
    let s = Scratch::new();
    let dirty = s.root.join("dirty-elsewhere");
    s.origin.clone_to(&dirty);
    make_dirty(&dirty);

    let outer = s.root.join("outer");
    for i in 0..MAX_WORK_TREES {
        s.origin.clone_to(&outer.join(format!("w{i}")));
    }
    for i in 0..6 {
        fs::create_dir_all(outer.join(format!("stray{i}/.git"))).unwrap();
    }
    for i in 0..3 {
        symlink(&dirty, outer.join(format!("link{i}"))).unwrap();
    }

    assert!(keep_and_report_unsaved_work(&outer).is_none());
}

// ----- issue #2785: where the judgement is made ---------------------------------

/// A planned `run-tmp` that holds unsaved work becomes a skip while planning,
/// so nothing downstream acts on it, and the report names the work tree.
#[test]
fn run_tmp_with_a_dirty_clone_is_skipped_when_planned_and_survives_apply() {
    let fx = Fx::new();
    let s = Scratch::new();
    let run_dir = fx.run(&[]);
    let runtime = run_dir.join("runtime");
    let clone = runtime.join("tmp/amplihack-rs");
    s.origin.clone_to(&clone);
    make_dirty(&clone);
    let settings = fx.settings(&[Category::RunTmp]);

    let items = collect(&settings).unwrap();

    let item = items
        .iter()
        .find(|i| i.path == runtime)
        .unwrap_or_else(|| panic!("no item for the runtime folder: {items:#?}"));
    assert!(matches!(item.op, Op::Skip), "{item:?}");
    assert_eq!(item.reason, below(&clone, UNCOMMITTED));
    assert!(
        !items.iter().any(|i| matches!(i.op, Op::Fail)),
        "{items:#?}"
    );

    let entries = apply_items(items, &settings, true);
    let e = entry(&entries, &runtime);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert_eq!(e.reason, below(&clone, UNCOMMITTED));
    assert!(clone.join("unsaved.txt").is_file(), "{entries:#?}");
    assert!(
        entries.iter().all(|e| e.action != Action::Failed),
        "keeping a folder never fails the sweep: {entries:#?}"
    );
}

/// The second line of defence: work that appears between planning and
/// removal is still seen, and refused as a skip, not a failure.
#[test]
fn run_tmp_that_gains_unsaved_work_after_planning_is_refused_when_applied() {
    let fx = Fx::new();
    let s = Scratch::new();
    let run_dir = fx.run(&[]);
    let runtime = run_dir.join("runtime");
    let clone = runtime.join("tmp/amplihack-rs");
    s.origin.clone_to(&clone);
    let settings = fx.settings(&[Category::RunTmp]);

    let items = collect(&settings).unwrap();
    assert!(
        items
            .iter()
            .any(|i| i.path == runtime && matches!(i.op, Op::RemoveRunTmp { .. })),
        "planned for removal while clean: {items:#?}"
    );
    make_unpushed(&clone);
    let entries = apply_items(items, &settings, true);

    let e = entry(&entries, &runtime);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert_eq!(e.reason, below(&clone, UNPUSHED));
    assert!(clone.join(".git").is_dir() && runtime.is_dir());
    assert!(entries.iter().all(|e| e.action != Action::Failed));
}

/// The refusals above are not vacuous: a clean, pushed clone does not keep
/// the folder, and it is removed as before.
#[test]
fn run_tmp_holding_only_a_clean_pushed_clone_is_removed_as_before() {
    let fx = Fx::new();
    let s = Scratch::new();
    let run_dir = fx.run(&[]);
    let runtime = run_dir.join("runtime");
    s.origin.clone_to(&runtime.join("tmp/amplihack-rs"));
    let settings = fx.settings(&[Category::RunTmp]);

    let entries = run_sweep(&settings, true).unwrap();

    assert_eq!(entry(&entries, &runtime).action, Action::Removed);
    assert!(!runtime.exists());
}

/// A recorded target in the legacy format is a plain folder; if it holds a
/// work tree with unsaved work it is skipped when planned.
#[test]
fn legacy_run_target_that_is_a_dirty_work_tree_is_skipped_when_planned() {
    let fx = Fx::new();
    let s = Scratch::new();
    let target = fx.work.targets_dir().join("t1");
    s.origin.clone_to(&target);
    make_dirty(&target);
    fx.run(&[&target]);
    let settings = fx.settings(&[Category::RunTarget]);

    let items = collect(&settings).unwrap();

    let item = items
        .iter()
        .find(|i| i.path == target)
        .unwrap_or_else(|| panic!("no item for the target: {items:#?}"));
    assert!(matches!(item.op, Op::Skip), "{item:?}");
    assert_eq!(item.reason, UNCOMMITTED);
    let entries = apply_items(items, &settings, true);
    assert_eq!(entry(&entries, &target).action, Action::Skipped);
    assert!(target.join("unsaved.txt").is_file());
    assert!(entries.iter().all(|e| e.action != Action::Failed));
}

#[test]
fn legacy_run_target_that_is_a_clean_work_tree_is_removed_as_before() {
    let fx = Fx::new();
    let s = Scratch::new();
    let target = fx.work.targets_dir().join("t1");
    s.origin.clone_to(&target);
    fx.run(&[&target]);
    let settings = fx.settings(&[Category::RunTarget]);

    let entries = run_sweep(&settings, true).unwrap();

    assert_eq!(entry(&entries, &target).action, Action::Removed);
    assert!(!target.exists());
}

/// `untagged-target` has no plan-time judgement; the apply-time arm that every
/// `RemoveDir` shares keeps it.
#[test]
fn untagged_target_holding_a_dirty_clone_is_kept_when_applied() {
    let fx = Fx::new();
    let s = Scratch::new();
    let stray = fx.work.targets_dir().join("stray");
    let clone = stray.join("checkout");
    s.origin.clone_to(&clone);
    make_dirty(&clone);
    let settings = fx.settings(&[Category::UntaggedTarget]);

    let entries = run_sweep(&settings, true).unwrap();

    let e = entry(&entries, &stray);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert_eq!(e.reason, below(&clone, UNCOMMITTED));
    assert!(clone.join("unsaved.txt").is_file());
    assert!(entries.iter().all(|e| e.action != Action::Failed));
}

#[test]
fn untagged_target_holding_a_clean_pushed_clone_is_removed_as_before() {
    let fx = Fx::new();
    let s = Scratch::new();
    let stray = fx.work.targets_dir().join("stray");
    s.origin.clone_to(&stray.join("checkout"));
    let settings = fx.settings(&[Category::UntaggedTarget]);

    let entries = run_sweep(&settings, true).unwrap();

    assert_eq!(entry(&entries, &stray).action, Action::Removed);
    assert!(!stray.exists());
}

/// A kept folder is a skip in the report, whatever kind of reason it is for,
/// so the command's exit status, which counts `failed` entries only, cannot
/// change. A broken `.git` is the case that could be mistaken for an error.
#[test]
fn keeping_a_folder_that_cannot_be_checked_is_a_skip_and_never_a_failure() {
    let fx = Fx::new();
    let run_dir = fx.run(&[]);
    let runtime = run_dir.join("runtime");
    let broken = runtime.join("tmp/broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join(".git"), "gitdir: /nonexistent\n").unwrap();
    let settings = fx.settings(&[Category::RunTmp]);

    let items = collect(&settings).unwrap();
    assert!(
        !items.iter().any(|i| matches!(i.op, Op::Fail)),
        "{items:#?}"
    );
    let entries = apply_items(items, &settings, true);

    let e = entry(&entries, &runtime);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert!(
        e.reason
            .starts_with(&format!("work tree {broken:?}: {CANNOT_CHECK}")),
        "{e:?}"
    );
    assert!(entries.iter().all(|e| e.action != Action::Failed));
    assert!(broken.join(".git").is_file());
}
