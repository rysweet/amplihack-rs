//! Issue #3161: `recipe guide <PR URL>` reaches a live run whatever its work
//! root and state folder, and refuses, naming the session, when a live run
//! for the pull request cannot be reached.
//!
//! The drive here is started with its own state folder and an explicit work
//! root, neither of which is the sender's. The pinned runner stand-in blocks
//! on a FIFO between its two steps (no sleep, poll or timer: the FIFO open
//! is the rendezvous). The session log that names the run is written the way
//! a detached run's log is, with the pull request in its pointer.
//!
//! - (b) the guidance reaches the drive's own run folder, is seen once by the
//!   drive's next step, and `recipe guidance show <PR URL>` run under the
//!   drive's environment (the goal loop's read) lists the folder that
//!   `recipe guide` wrote (R7);
//! - (b2) a live run that cannot be reached makes the command refuse with
//!   exit 1, the session named, and nothing written;
//! - (b3) with no live run for the pull request, the pull request folder is
//!   written as before and the sender is told no run was found.
//!
//! The assertions compare paths, counts, exit codes and JSON fields.

#![cfg(unix)]

use crate::support::fifo_release;
use crate::support::recipe_guide::{
    BEGIN, GuideFixture, PR_URL, show, status_lines, stderr, stdout, written_lines,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const MARKER: &str = "MARKER-3161-merge-only-on-a-conflict";
const SESSION: &str = "build-3143-review-and-merge";

/// Two Claude steps with a block on `$RELEASE_FIFO` between them. The
/// launcher runs with the drive's own state folder.
const DRIVE: &str = r#"
AMPLIHACK_STATE_DIR="$DRIVE_STATE"; export AMPLIHACK_STATE_DIR
STEP=d.1 "$AMP_BIN" claude --subprocess-safe -- -p "drive step one" \
  > "$RECORD_DIR/out.d.1" 2> "$RECORD_DIR/err.d.1" || exit 11
read _ < "$RELEASE_FIFO"
STEP=d.2 "$AMP_BIN" claude --subprocess-safe -- -p "drive step two" \
  > "$RECORD_DIR/out.d.2" 2> "$RECORD_DIR/err.d.2" || exit 12
"#;

struct Drive {
    f: GuideFixture,
    live: fifo_release::Live,
    drive_state: PathBuf,
    drive_work: PathBuf,
    run_dir: PathBuf,
}

impl Drive {
    fn run_guidance(&self) -> PathBuf {
        self.run_dir.join("guidance")
    }

    /// `recipe guidance show <PR URL> --json` as the goal loop runs it: the
    /// drive's state folder and work root, no inherited folder list.
    fn goal_loop_show(&self) -> serde_json::Value {
        let mut c = self
            .f
            .amplihack(&["recipe", "guidance", "show", PR_URL, "--json"]);
        c.env("AMPLIHACK_STATE_DIR", &self.drive_state)
            .env("AMPLIHACK_WORK_ROOT", &self.drive_work)
            .env("AMPLIHACK_WORK_ROOT_EXPLICIT", "1")
            .env_remove("AMPLIHACK_RUN_GUIDANCE_DIRS");
        let o = self.f.run_with_stdin(c, None);
        assert!(o.status.success(), "{}", show(&o));
        serde_json::from_str(stdout(&o).trim()).unwrap_or_else(|e| panic!("{e}: {}", show(&o)))
    }
}

/// Starts the drive for `PR_URL` and returns once it is blocked between its
/// steps, with a session log whose pointer names the pull request.
fn start_drive() -> Drive {
    let f = GuideFixture::new();
    let drive_state = f.root.join("drive-state");
    let drive_work = f.root.join("drive-elsewhere").join("work");
    for d in [&drive_state, drive_work.parent().unwrap(), &drive_work] {
        crate::support::recipe_guide::private_dir(d);
    }
    let fifo = f.root.join("release.fifo");
    fifo_release::mkfifo(&fifo);
    let context = format!("pr_url={PR_URL}");
    let command = f.recipe_run_command(
        DRIVE,
        &["-c", &context],
        &[
            ("RELEASE_FIFO", fifo.to_str().unwrap()),
            ("DRIVE_STATE", drive_state.to_str().unwrap()),
            ("AMPLIHACK_STATE_DIR", drive_state.to_str().unwrap()),
            ("AMPLIHACK_WORK_ROOT", drive_work.to_str().unwrap()),
            ("AMPLIHACK_WORK_ROOT_EXPLICIT", "1"),
        ],
    );
    let live = fifo_release::start(command, &fifo, "amplihack recipe run");
    let run_dir = drive_work.join("runs").join(f.record("run_id.run").trim());
    let mut pointer = GuideFixture::pointer(f.record("corr_id.run").trim(), Some(&run_dir));
    pointer["pr_url"] = PR_URL.into();
    pointer["pr_number"] = "7".into();
    f.session_log_from_pointer(SESSION, &pointer);
    Drive {
        f,
        live,
        drive_state,
        drive_work,
        run_dir,
    }
}

fn mode_of(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o7777
}

// ---------------------------------------------------------------------------
// (b) fan-out to the live run, whatever its work root
// ---------------------------------------------------------------------------

#[test]
fn b_pr_url_guidance_reaches_a_drive_with_its_own_work_root() {
    let mut d = start_drive();
    // The run and the sender disagree about where the pull request folder is.
    assert_ne!(d.drive_state, d.f.state);
    assert!(
        !d.run_dir.starts_with(&d.f.work),
        "the drive's work root is not the sender's"
    );

    let o = d.f.guide_text(&[PR_URL], MARKER);
    assert!(o.status.success(), "{}", show(&o));
    let written = written_lines(&o);
    let folders: Vec<PathBuf> = written
        .iter()
        .map(|(p, _, _)| p.parent().unwrap().to_path_buf())
        .collect();
    assert!(
        folders.contains(&d.run_guidance()),
        "the live run's own folder was written: {folders:?}"
    );
    assert!(
        folders.contains(&d.f.pr_parent().join("guidance")),
        "the pull request folder was written too: {folders:?}"
    );
    assert_eq!(written.len(), 2, "{}", show(&o));
    assert_eq!(status_lines(&o), ["appended; not yet read"], "{}", show(&o));

    // R7: the goal loop's read resolves the folder the sender's write used.
    let seen = d.goal_loop_show();
    let listed: Vec<PathBuf> = seen["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| PathBuf::from(f["path"].as_str().unwrap()))
        .collect();
    assert!(listed.contains(&d.run_guidance()), "{listed:?}");
    assert_eq!(seen["entries"].as_array().unwrap().len(), 1, "{seen}");
    assert_eq!(seen["entries"][0]["read"], false, "{seen}");

    let run = d.live.finish();
    assert!(run.status.success(), "{}", fifo_release::describe(&run));

    // The drive's next turn sees the guidance once: a run folder entry and a
    // pull request folder entry are one entry.
    let p2 = d.f.record("prompt.d.2");
    assert!(p2.starts_with("drive step two\n\n"), "{p2:?}");
    assert_eq!(p2.matches(BEGIN).count(), 1, "{p2:?}");
    assert_eq!(p2.matches(MARKER).count(), 1, "{p2:?}");
    assert_eq!(d.f.record("prompt.d.1"), "drive step one");

    let after = d.goal_loop_show();
    assert_eq!(after["entries"].as_array().unwrap().len(), 1, "{after}");
    assert_eq!(after["entries"][0]["read"], true, "{after}");
}

// ---------------------------------------------------------------------------
// (b2) a live run that cannot be reached: refuse, name the session
// ---------------------------------------------------------------------------

#[test]
fn b2_an_unreachable_live_run_refuses_names_the_session_and_writes_nothing() {
    let mut d = start_drive();
    // A group- and world-writable run folder fails the trust rule the run's
    // guidance folder is read under.
    let original = mode_of(&d.run_dir);
    fs::set_permissions(&d.run_dir, fs::Permissions::from_mode(0o777)).unwrap();

    let o = d.f.guide_text(&[PR_URL], MARKER);
    fs::set_permissions(&d.run_dir, fs::Permissions::from_mode(original)).unwrap();

    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(stdout(&o).trim().is_empty(), "{}", show(&o));
    assert!(stderr(&o).contains(SESSION), "{}", show(&o));
    assert!(!stderr(&o).contains(MARKER), "{}", show(&o));
    // Nothing was created: no entry in the run, no pull request folder.
    assert!(d.f.entries(&d.run_guidance()).is_empty());
    assert!(!d.f.pr_parent().exists(), "{}", d.f.pr_parent().display());

    let run = d.live.finish();
    assert!(run.status.success(), "{}", fifo_release::describe(&run));
    assert!(!d.f.record("prompt.d.2").contains(MARKER));
}

// ---------------------------------------------------------------------------
// (b3) no live run: the pull request folder is written as before
// ---------------------------------------------------------------------------

#[test]
fn b3_with_no_live_run_the_pr_folder_is_written_and_the_sender_is_told() {
    let f = GuideFixture::new();
    let o = f.guide_text(&[PR_URL], MARKER);
    assert!(o.status.success(), "{}", show(&o));
    let written = written_lines(&o);
    assert_eq!(written.len(), 1, "{}", show(&o));
    assert_eq!(
        written[0].0.parent().unwrap(),
        f.pr_parent().join("guidance")
    );
    assert_eq!(written[0].2, "pr");
    let status = status_lines(&o);
    assert_eq!(status.len(), 1, "{}", show(&o));
    assert!(
        status[0].starts_with("appended; not yet read"),
        "{status:?}"
    );
    // The status carries a second clause that says no run was found.
    assert_eq!(status[0].matches(';').count(), 2, "{status:?}");
}

#[test]
fn b3_a_pointer_whose_run_is_not_live_does_not_stop_the_write() {
    let f = GuideFixture::new();
    // A run folder and a session log that name the pull request, with no
    // process record: the run has ended.
    let run_dir = f.make_run_in(&f.alt_work.clone(), "ended-run", "c-ended");
    let mut pointer = GuideFixture::pointer("c-ended", Some(&run_dir));
    pointer["pr_url"] = PR_URL.into();
    pointer["pr_number"] = "7".into();
    f.session_log_from_pointer("ended-session", &pointer);

    let o = f.guide_text(&[PR_URL], MARKER);
    assert!(o.status.success(), "{}", show(&o));
    let written = written_lines(&o);
    assert_eq!(written.len(), 1, "{}", show(&o));
    assert_eq!(written[0].2, "pr");
    assert!(f.entries(&run_dir.join("guidance")).is_empty());
}

#[test]
fn b3_an_unreadable_log_folder_is_reported_rather_than_read_as_no_live_run() {
    let f = GuideFixture::new();
    // A log folder anyone can write to fails the trust rule, so no run can
    // be looked up. The write still goes to the pull request folder, and the
    // sender is told the lookup did not happen.
    fs::create_dir_all(&f.recipe_logs).unwrap();
    fs::set_permissions(&f.recipe_logs, fs::Permissions::from_mode(0o777)).unwrap();
    let o = f.guide_text(&[PR_URL], MARKER);
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(written_lines(&o).len(), 1, "{}", show(&o));
    assert!(
        stderr(&o).contains("live runs could not be looked up"),
        "{}",
        show(&o)
    );
}
