//! Issue #2096 (reviewer issue 1): the stable execution path
//! `<work root>/exec/<identity>/` is reclaimed once its owner has ended, by
//! the next attempt with that identity and by `amplihack cleanup`, with the
//! same rules: the owner run has ended, no durable worktree uses its
//! repository, and a symlink there is never followed.
use super::*;
use std::os::unix::fs::PermissionsExt;

/// One successful attempt: its run folder and the stable path it used,
/// which it removed again at release.
fn first_attempt(f: &Fixture) -> (PathBuf, PathBuf) {
    f.push_base();
    let before = f.run_folders();
    succeeds(
        &f.command("first")
            .args(["-c", "issue_number=2096"])
            .output()
            .unwrap(),
    );
    let run = f.new_run_folder(&before);
    let stable = cwd(f, "first")
        .parent()
        .expect("the checkout lies in its storage")
        .to_path_buf();
    assert!(
        !stable.exists(),
        "{} was not released by its own attempt",
        stable.display()
    );
    (run, stable)
}

/// A relaunch with the same recovery identity.
fn relaunch(f: &Fixture, label: &str) -> Output {
    let out = f
        .command(label)
        .args(["-c", "issue_number=2096"])
        .output()
        .unwrap();
    succeeds(&out);
    out
}

fn cwd(f: &Fixture, label: &str) -> PathBuf {
    PathBuf::from(String::from_utf8(f.observed(label, "cwd")).unwrap())
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// What an attempt that claimed `stable` leaves when it dies or fails during
/// preparation: the path and its claim record naming `owner`.
fn stale_claim(stable: &Path, owner: &Path) {
    fs::create_dir(stable).unwrap();
    fs::set_permissions(stable, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        stable.join("claim.json"),
        serde_json::to_vec(&serde_json::json!({"schema_version": 1, "run_dir": owner})).unwrap(),
    )
    .unwrap();
}

/// A run folder whose `run.json` names this test process, which is running.
fn live_run_folder(f: &Fixture) -> PathBuf {
    let stat = fs::read_to_string("/proc/self/stat").unwrap();
    let ticks: u64 = stat[stat.rfind(')').unwrap() + 1..]
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    let dir = f.root.join("live-run");
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("run.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "run_id": "live-run",
            "pid": std::process::id(),
            "pid_start_ticks": ticks,
        }))
        .unwrap(),
    )
    .unwrap();
    dir
}

/// `amplihack cleanup --only run-exec`, as JSON.
fn cleanup(f: &Fixture, apply: bool) -> serde_json::Value {
    let mut c = crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
    f.environment(&mut c);
    c.env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
        .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
        .env("AMPLIHACK_WORK_ROOT", f.root.join("work"))
        .args(["cleanup", "--format", "json", "--only", "run-exec"]);
    if apply {
        c.arg("--apply");
    }
    let out = c.output().unwrap();
    succeeds(&out);
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "cleanup JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The report entry for `path`.
fn entry(report: &serde_json::Value, path: &Path) -> serde_json::Value {
    report["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("no entries in {report:#}"))
        .iter()
        .find(|e| e["path"].as_str() == path.to_str())
        .cloned()
        .unwrap_or_else(|| panic!("no entry for {} in {report:#}", path.display()))
}

#[test]
fn a_path_left_by_a_failed_attempt_is_reclaimed_by_the_next_attempt() {
    let f = Fixture::new();
    let (first_run, stable) = first_attempt(&f);
    // The attempt failed after its claim: a partial private repository and
    // the caller's copied input files are still there.
    stale_claim(&stable, &first_run);
    fs::create_dir_all(stable.join("repository.git/objects")).unwrap();
    fs::create_dir_all(stable.join("checkout")).unwrap();
    fs::write(stable.join("checkout/.env"), "TOKEN=copied\n").unwrap();

    let out = relaunch(&f, "second");
    assert_eq!(
        cwd(&f, "second"),
        stable.join("checkout"),
        "the relaunch reclaims the stable path: {}",
        stderr(&out)
    );
    assert!(!stderr(&out).contains("still held"), "{}", stderr(&out));
    assert!(!stable.exists(), "released again at the end of the attempt");
    let left: Vec<_> = fs::read_dir(stable.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(left.is_empty(), "nothing is left beside it: {left:?}");
}

#[test]
fn an_earlier_owner_that_is_still_running_keeps_the_path_and_the_relaunch_says_so() {
    let f = Fixture::new();
    let (_, stable) = first_attempt(&f);
    let live = live_run_folder(&f);
    stale_claim(&stable, &live);

    let out = relaunch(&f, "second");
    assert_ne!(cwd(&f, "second"), stable.join("checkout"));
    let text = stderr(&out);
    assert!(
        text.contains("still held") && text.contains("is still running"),
        "the fallback names its reason: {text}"
    );
    assert!(
        stable.join("claim.json").is_file(),
        "the owner's path stays"
    );

    let report = cleanup(&f, true);
    let kept = entry(&report, &stable);
    assert_eq!(kept["action"], "skipped", "{kept:#}");
    assert!(stable.join("claim.json").is_file());
}

#[test]
fn durable_worktrees_keep_the_path_until_they_are_removed() {
    let f = Fixture::new();
    let (first_run, stable) = first_attempt(&f);
    stale_claim(&stable, &first_run);
    // A durable worktree outside the storage, registered in its repository
    // as step 04 of default-workflow registers one.
    let durable = f.root.join("durable");
    fs::create_dir(&durable).unwrap();
    fs::write(durable.join(".git"), "gitdir: elsewhere\n").unwrap();
    let admin = stable.join("repository.git/worktrees/durable");
    fs::create_dir_all(&admin).unwrap();
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", durable.join(".git").display()),
    )
    .unwrap();

    let out = relaunch(&f, "second");
    assert_ne!(cwd(&f, "second"), stable.join("checkout"));
    assert!(
        stderr(&out).contains("durable worktree"),
        "{}",
        stderr(&out)
    );
    assert!(
        admin.join("gitdir").is_file(),
        "the relaunch removed nothing"
    );

    let kept = entry(&cleanup(&f, true), &stable);
    assert_eq!(kept["action"], "skipped", "{kept:#}");
    assert!(
        kept["reason"]
            .as_str()
            .unwrap()
            .contains("durable worktree"),
        "{kept:#}"
    );
    assert!(admin.join("gitdir").is_file(), "cleanup removed nothing");

    fs::remove_dir_all(&durable).unwrap();
    let removed = entry(&cleanup(&f, true), &stable);
    assert_eq!(removed["action"], "removed", "{removed:#}");
    assert!(!stable.exists());
}

#[test]
fn a_symlink_at_the_stable_path_is_refused_and_its_target_is_left_untouched() {
    let f = Fixture::new();
    let (_, stable) = first_attempt(&f);
    let elsewhere = f.root.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::write(elsewhere.join("sentinel"), "keep me\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, &stable).unwrap();

    let out = relaunch(&f, "second");
    assert_ne!(cwd(&f, "second"), stable.join("checkout"));
    assert!(
        stderr(&out).contains("not a real directory"),
        "{}",
        stderr(&out)
    );

    let kept = entry(&cleanup(&f, true), &stable);
    assert_eq!(kept["action"], "skipped", "{kept:#}");
    assert!(
        fs::symlink_metadata(&stable)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(elsewhere.join("sentinel")).unwrap(),
        "keep me\n"
    );
}

/// A launch refused after preparation (here, a context value embedding a
/// caller path; the S-AZ4 descriptor refusal takes the same path) releases
/// the checkout and finishes the recovery admission as Failure, so the next
/// launch does not resume it.
#[test]
fn a_launch_refused_after_preparation_finishes_the_admission_as_failure() {
    let f = Fixture::new();
    f.push_base();
    let before = f.run_folders();
    let out = f
        .command("refused")
        .args(["-c", "issue_number=2096", "-c"])
        .arg(format!("note=see {}/tracked", f.repo.display()))
        .output()
        .unwrap();
    assert!(!out.status.success(), "the launch must be refused");
    assert!(
        stderr(&out).contains("embeds a caller-checkout path"),
        "{}",
        stderr(&out)
    );
    assert!(!f.root.join("refused/cwd").exists(), "the runner started");
    let run = f.new_run_folder(&before);
    assert_eq!(recovery_outcome(&run), "failure");
    let report = json_file(&run.join("run.json"));
    assert_eq!(report["outcome"], "failure", "{report:#}");
    let exec = f.root.join("work/exec");
    let left: Vec<_> = match fs::read_dir(&exec) {
        Ok(entries) => entries.map(|e| e.unwrap().path()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => panic!("read {}: {e}", exec.display()),
    };
    assert!(left.is_empty(), "the stable path was released: {left:?}");
}

// ----- issue #2785: worktrees that hold work ----------------------------------
//
// A worktree registered in the storage's `repository.git` that lies inside
// the storage, including its own `checkout`, keeps the storage while it
// holds uncommitted changes or commits that are not on a remote. The reason
// names the discovered Git tree and what the existing checks found.

/// `git <args>` in `dir`, with the fixture's environment and an identity.
fn git_ok(f: &Fixture, dir: &Path, args: &[&str]) -> String {
    let mut all = vec![
        "-c",
        "user.name=Isolation Fixture",
        "-c",
        "user.email=fixture@example.invalid",
    ];
    all.extend_from_slice(args);
    String::from_utf8(f.git_in(dir, &all))
        .unwrap()
        .trim()
        .to_string()
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A private repository at `<stable>/repository.git`, cloned from a bare
/// origin outside the storage, with a fetch refspec so that a push updates
/// `origin/*`, as preparation leaves it.
fn private_repository(f: &Fixture, stable: &Path) -> PathBuf {
    let origin = f.root.join("origin.git");
    git_ok(
        f,
        &f.root,
        &[
            "clone",
            "-q",
            "--bare",
            path_str(&f.repo),
            path_str(&origin),
        ],
    );
    let repository = stable.join("repository.git");
    git_ok(
        f,
        &f.root,
        &[
            "clone",
            "-q",
            "--bare",
            path_str(&origin),
            path_str(&repository),
        ],
    );
    git_ok(
        f,
        &repository,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    git_ok(f, &repository, &["fetch", "-q", "origin"]);
    repository
}

#[test]
fn a_worktree_inside_the_storage_with_work_keeps_it_until_the_work_is_pushed() {
    let f = Fixture::new();
    let (first_run, stable) = first_attempt(&f);
    stale_claim(&stable, &first_run);
    let repository = private_repository(&f, &stable);
    let feat = stable.join("worktrees/feat");
    git_ok(
        &f,
        &repository,
        &["worktree", "add", "-q", "-b", "feat", path_str(&feat)],
    );
    let work = feat.join("step-07-tests.rs");
    fs::write(&work, "// written before the stop\n").unwrap();
    let named_worktree = format!("{feat:?}");

    let out = relaunch(&f, "second");
    assert_ne!(cwd(&f, "second"), stable.join("checkout"));
    let text = stderr(&out);
    assert!(
        text.contains("still held")
            && text.contains(&named_worktree)
            && text.contains("has uncommitted changes"),
        "the relaunch names the worktree and what it holds: {text}"
    );
    assert_eq!(
        fs::read_to_string(&work).unwrap(),
        "// written before the stop\n"
    );

    let kept = entry(&cleanup(&f, true), &stable);
    assert_eq!(kept["action"], "skipped", "{kept:#}");
    assert!(
        kept["reason"].as_str().unwrap().contains(&named_worktree)
            && kept["reason"]
                .as_str()
                .unwrap()
                .contains("has uncommitted changes"),
        "{kept:#}"
    );
    assert_eq!(
        fs::read_to_string(&work).unwrap(),
        "// written before the stop\n"
    );

    git_ok(&f, &feat, &["add", "step-07-tests.rs"]);
    git_ok(&f, &feat, &["commit", "-qm", "step 07"]);
    let sha = git_ok(&f, &feat, &["rev-parse", "HEAD"]);
    let kept = entry(&cleanup(&f, true), &stable);
    assert_eq!(kept["action"], "skipped", "{kept:#}");
    assert!(
        kept["reason"].as_str().unwrap().contains(&named_worktree)
            && kept["reason"]
                .as_str()
                .unwrap()
                .contains("commits on no remote branch"),
        "a commit on no remote still keeps it: {kept:#}"
    );
    assert_eq!(git_ok(&f, &feat, &["rev-parse", "HEAD"]), sha);
    assert_eq!(
        fs::read_to_string(&work).unwrap(),
        "// written before the stop\n"
    );

    git_ok(&f, &feat, &["push", "-q", "-u", "origin", "feat"]);
    let removed = entry(&cleanup(&f, true), &stable);
    assert_eq!(removed["action"], "removed", "{removed:#}");
    assert!(!stable.exists());
}

/// The primary checkout keeps storage while either its dirty file or its
/// unpushed commit remains. Cleaning and pushing makes storage removable.
#[test]
fn a_dirty_private_checkout_keeps_the_storage_until_clean_and_pushed() {
    let f = Fixture::new();
    let (first_run, stable) = first_attempt(&f);
    stale_claim(&stable, &first_run);
    let repository = private_repository(&f, &stable);
    let checkout = stable.join("checkout");
    git_ok(
        &f,
        &repository,
        &["worktree", "add", "-q", "--detach", path_str(&checkout)],
    );
    git_ok(
        &f,
        &checkout,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "the caller's own commit",
        ],
    );
    fs::write(checkout.join("copied-from-the-caller"), "dirty\n").unwrap();

    let sha = git_ok(&f, &checkout, &["rev-parse", "HEAD"]);
    let kept = entry(&cleanup(&f, true), &stable);
    assert_eq!(kept["action"], "skipped", "{kept:#}");
    assert!(
        kept["reason"].as_str().unwrap().contains("uncommitted"),
        "{kept:#}"
    );
    assert_eq!(git_ok(&f, &checkout, &["rev-parse", "HEAD"]), sha);
    assert!(checkout.join("copied-from-the-caller").is_file());
    git_ok(&f, &checkout, &["add", "copied-from-the-caller"]);
    git_ok(&f, &checkout, &["commit", "-qm", "saved checkout work"]);
    let kept = entry(&cleanup(&f, true), &stable);
    assert_eq!(kept["action"], "skipped", "{kept:#}");
    assert!(
        kept["reason"]
            .as_str()
            .unwrap()
            .contains("no remote branch"),
        "{kept:#}"
    );
    git_ok(
        &f,
        &checkout,
        &["push", "-q", "origin", "HEAD:refs/heads/saved-checkout"],
    );
    let removed = entry(&cleanup(&f, true), &stable);
    assert_eq!(removed["action"], "removed", "{removed:#}");
    assert!(!stable.exists());
}

/// At the end of a run, release keeps the private repository for a task
/// worktree linked to it, as before, and now names the worktree and the
/// work it holds. The run still succeeds.
#[test]
fn the_end_of_a_run_names_a_linked_worktree_that_holds_work() {
    let f = Fixture::new();
    f.push_base();
    let task = f.root.join("task-worktree");
    let out = f
        .command("task")
        .env("ISSUE_2785_TASK_WORKTREE", &task)
        .output()
        .unwrap();
    succeeds(&out);

    let text = stderr(&out);
    let line = text
        .lines()
        .find(|l| l.starts_with(&format!("amplihack: worktree {task:?} ")))
        .unwrap_or_else(|| panic!("no line names {}: {text}", task.display()));
    assert!(
        line.contains(" has uncommitted changes; it is linked to this run's private repository ")
            && line.ends_with(", which is kept for it"),
        "{line}"
    );
    assert!(
        task.join("uncommitted").is_file(),
        "the worktree and its work stay"
    );
}
