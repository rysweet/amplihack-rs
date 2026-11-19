//! Issue #2783 (first piece): `amplihack recipe guide` finds a run from what
//! the run recorded at start (the `run_dir` of its log pointer line), whatever
//! the caller's environment.
//!
//! Real binary, private 0700 `HOME`, `AMPLIHACK_STATE_DIR` and work roots (see
//! `support::recipe_guide`). Every guide call here runs with
//! `AMPLIHACK_WORK_ROOT` and `AMPLIHACK_WORK_ROOT_EXPLICIT` unset. Nothing
//! under the real `~/.claude` or `~/.amplihack` is touched.

#![cfg(unix)]

use crate::support::recipe_guide::{
    GuideFixture, one_line_error, parse_appended, private_dir, show, stderr, stdout,
};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

const RUN_FOLDER: &str = "3f1b7c2a-5d4e-4a90-8b61-7e2c9d0a4f18";
const CORRELATION: &str = "a7d4e1c9-0b36-4f25-9c8e-1d5f3b7a2e60";
const SESSION: &str = "amp-smart-orchestrator-myapp-20261118-205000";

fn guidance_of(run: &Path) -> std::path::PathBuf {
    run.join("guidance")
}

fn assert_lands_in(o: &std::process::Output, run: &Path) {
    assert!(o.status.success(), "{}", show(o));
    let (path, _, source) = parse_appended(o);
    assert_eq!(
        path.parent(),
        Some(guidance_of(run).as_path()),
        "{}",
        show(o)
    );
    assert_eq!(source, "run");
}

fn issue_pointer(f: &GuideFixture, run: &Path) -> serde_json::Value {
    let mut p = GuideFixture::pointer(CORRELATION, Some(run));
    p["issue_number"] = "2783".into();
    p["issue_url"] = "https://github.com/rysweet/amplihack-rs/issues/2783".into();
    let _ = f;
    p
}

// ---------------------------------------------------------------------------
// Resolution from the recorded run folder
// ---------------------------------------------------------------------------

#[test]
fn a_session_name_resolves_through_the_recorded_run_dir_under_a_non_default_work_root() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&run)));
    let o = f.guide_without_work_root(&[SESSION], "by session");
    assert_lands_in(&o, &run);
    assert_eq!(f.entries(&guidance_of(&run)).len(), 1);
    assert!(
        !f.runs().join(RUN_FOLDER).exists(),
        "wrote under the default root"
    );
}

#[test]
fn a_run_id_resolves_through_a_recorded_run_dir_under_a_non_default_work_root() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&run)));
    // The folder name and the correlation id are both run references.
    let o = f.guide_without_work_root(&[RUN_FOLDER], "by folder name");
    assert_lands_in(&o, &run);
    let o = f.guide_without_work_root(&[CORRELATION], "by correlation id");
    assert_lands_in(&o, &run);
    assert_eq!(f.entries(&guidance_of(&run)).len(), 2);
}

#[test]
fn a_recorded_issue_resolves_in_every_accepted_form() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &issue_pointer(&f, &run));
    for target in [
        "2783",
        "#2783",
        "https://github.com/rysweet/amplihack-rs/issues/2783",
    ] {
        let o = f.guide_without_work_root(&[target], "by issue");
        assert_lands_in(&o, &run);
    }
    assert_eq!(f.entries(&guidance_of(&run)).len(), 3);
}

#[test]
fn an_issue_in_the_pointer_is_not_matched_by_another_number() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &issue_pointer(&f, &run));
    let o = f.guide_without_work_root(&["2784"], "wrong issue");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(f.entries(&guidance_of(&run)).is_empty());
}

#[test]
fn the_recorded_folder_wins_over_the_environment_work_root() {
    let f = GuideFixture::new();
    let recorded = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    // The same folder name also exists under the environment's root.
    let env_run = f.make_run(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(
        SESSION,
        &GuideFixture::pointer(CORRELATION, Some(&recorded)),
    );
    let o = f.guide(&[SESSION, "-"], Some(b"which one"));
    assert_lands_in(&o, &recorded);
    assert!(f.entries(&guidance_of(&env_run)).is_empty());
}

#[test]
fn a_pointer_with_no_run_dir_still_resolves_through_the_environment() {
    let f = GuideFixture::new();
    let run = f.make_run(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, None));
    // The default work root is also where the environment points.
    let o = f.guide(&[SESSION, "-"], Some(b"by environment"));
    assert_lands_in(&o, &run);
    let o = f.guide(&[CORRELATION, "-"], Some(b"by environment"));
    assert_lands_in(&o, &run);
}

#[test]
fn a_pull_request_url_still_writes_only_to_the_per_pr_folder() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let mut p = GuideFixture::pointer(CORRELATION, Some(&run));
    p["pr_url"] = crate::support::recipe_guide::PR_URL.into();
    f.session_log_from_pointer(SESSION, &p);
    let o = f.guide_without_work_root(&[crate::support::recipe_guide::PR_URL], "for the pr");
    assert!(o.status.success(), "{}", show(&o));
    let (path, _, source) = parse_appended(&o);
    assert_eq!(source, "pr");
    assert!(path.starts_with(f.pr_parent()), "{}", show(&o));
    assert!(f.entries(&guidance_of(&run)).is_empty());
}

// ---------------------------------------------------------------------------
// A run that retried has one folder and several correlation ids (issue #2818)
// ---------------------------------------------------------------------------

/// The correlation id of a second attempt: a transient retry builds a fresh
/// one, which is the id `recipe stop` takes and the id the agent steps of
/// that attempt carry.
const RETRY_CORRELATION: &str = "c3e2a1b0-9f8e-4d7c-8b6a-5f4e3d2c1b0a";

/// A run that retried, its log holding one pointer line per attempt and its
/// `run.json` listing both, as `record_attempt_checked` writes them.
fn retried_run(f: &GuideFixture) -> std::path::PathBuf {
    let run = f.make_run_with_attempts(
        &f.alt_work.clone(),
        RUN_FOLDER,
        &[CORRELATION, RETRY_CORRELATION],
    );
    f.session_log_from_pointers(
        SESSION,
        &[
            GuideFixture::pointer(CORRELATION, Some(&run)),
            GuideFixture::pointer(RETRY_CORRELATION, Some(&run)),
        ],
    );
    run
}

#[test]
fn the_correlation_id_of_a_later_attempt_resolves_through_the_recorded_folder() {
    let f = GuideFixture::new();
    let run = retried_run(&f);
    // Only the first pointer line of a log is read, so the second attempt's
    // id is in no record; the folder every attempt shares is, and its
    // `run.json` lists that id.
    let o = f.guide_without_work_root(&[RETRY_CORRELATION], "by the running attempt");
    assert_lands_in(&o, &run);
    // The first attempt's id and the folder name still resolve as before.
    let o = f.guide_without_work_root(&[CORRELATION], "by the first attempt");
    assert_lands_in(&o, &run);
    let o = f.guide_without_work_root(&[RUN_FOLDER], "by folder name");
    assert_lands_in(&o, &run);
    assert_eq!(f.entries(&guidance_of(&run)).len(), 3);
}

#[test]
fn an_id_no_recorded_folder_records_is_not_found_and_the_line_names_what_was_read() {
    let f = GuideFixture::new();
    let run = retried_run(&f);
    let stranger = "7c6b5a49-3827-4615-9403-2e1d0c9b8a76";
    let o = f.guide_without_work_root(&[stranger], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let line = one_line_error(&o);
    assert!(line.contains(stranger), "{line}");
    assert!(
        line.contains("the attempts in run.json of 1 recorded run folder(s)"),
        "{line}"
    );
    assert!(f.entries(&guidance_of(&run)).is_empty());
}

/// A run whose folder has been cleaned up, addressed by the id of its
/// second attempt. The first stage has no record naming that id, so the
/// answer comes from the second stage -- which must report the same reason
/// the first stage reports for the first attempt's id, rather than fold a
/// folder it never opened into a count of `run.json` files it read
/// (issue #2818). The note's two counts say how far the sweep got.
#[test]
fn a_cleaned_up_folder_gives_its_reason_whichever_attempt_id_names_it() {
    let f = GuideFixture::new();
    let run = retried_run(&f);
    fs::remove_dir_all(&run).unwrap();

    let by_first = f.guide_without_work_root(&[CORRELATION], "text");
    let by_later = f.guide_without_work_root(&[RETRY_CORRELATION], "text");
    assert_eq!(by_first.status.code(), Some(1), "{}", show(&by_first));
    assert_eq!(by_later.status.code(), Some(1), "{}", show(&by_later));
    let first = one_line_error(&by_first);
    let later = one_line_error(&by_later);
    for line in [&first, &later] {
        assert!(line.contains("no longer exists"), "{line}");
        assert!(line.contains(run.to_str().unwrap()), "{line}");
        // The folder was never opened, so the note does not claim it was.
        assert!(
            line.contains("the attempts in run.json of 0 of 1 recorded run folder(s)"),
            "{line}"
        );
    }
    // Both stages reach the same folder; the reason is said once.
    assert_eq!(first.matches("no longer exists").count(), 1, "{first}");
}

// ---------------------------------------------------------------------------
// A recorded path is untrusted
// ---------------------------------------------------------------------------

fn assert_not_found_and_nothing_written(o: &std::process::Output, dir: &Path) {
    assert_eq!(o.status.code(), Some(1), "{}", show(o));
    assert!(stdout(o).is_empty(), "{}", show(o));
    one_line_error(o);
    assert!(
        !dir.join("guidance").exists(),
        "wrote into {}",
        dir.display()
    );
}

#[test]
fn a_relative_run_dir_is_not_used() {
    let f = GuideFixture::new();
    let mut p = GuideFixture::pointer(CORRELATION, None);
    p["run_dir"] = format!("elsewhere/work/runs/{RUN_FOLDER}").into();
    f.session_log_from_pointer(SESSION, &p);
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    one_line_error(&o);
}

#[test]
fn a_run_dir_with_a_dot_dot_component_is_not_used() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let sneaky = f
        .alt_work
        .join("runs")
        .join("x")
        .join("..")
        .join(RUN_FOLDER);
    let _ = run;
    let p = GuideFixture::pointer(CORRELATION, Some(&sneaky));
    f.session_log_from_pointer(SESSION, &p);
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_not_found_and_nothing_written(&o, &f.alt_work.join("runs").join(RUN_FOLDER));
}

#[test]
fn a_run_dir_whose_parent_is_not_named_runs_is_not_used() {
    let f = GuideFixture::new();
    let dir = f.root.join("not-runs").join(RUN_FOLDER);
    private_dir(&dir);
    crate::support::recipe_guide::private_file(&dir.join("run.json"), "{}");
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&dir)));
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_not_found_and_nothing_written(&o, &dir);
}

#[test]
fn a_run_dir_that_is_a_symbolic_link_is_not_used() {
    let f = GuideFixture::new();
    let real = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let link = f
        .alt_work
        .join("runs")
        .join("c5a1e0d2-8f47-4b3a-9e16-2a7d6c0b9f34");
    symlink(&real, &link).unwrap();
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&link)));
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_not_found_and_nothing_written(&o, &real);
}

#[test]
fn a_run_dir_without_a_run_json_is_not_used() {
    let f = GuideFixture::new();
    let dir = f.alt_work.join("runs").join(RUN_FOLDER);
    private_dir(&f.alt_work);
    private_dir(&f.alt_work.join("runs"));
    private_dir(&dir);
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&dir)));
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_not_found_and_nothing_written(&o, &dir);
}

#[test]
fn a_non_string_run_dir_is_treated_as_absent() {
    let f = GuideFixture::new();
    let run = f.make_run(RUN_FOLDER, CORRELATION);
    let mut p = GuideFixture::pointer(CORRELATION, None);
    p["run_dir"] = serde_json::json!(["not", "a", "string"]);
    f.session_log_from_pointer(SESSION, &p);
    let o = f.guide(&[SESSION, "-"], Some(b"fallback"));
    assert_lands_in(&o, &run);
}

// ---------------------------------------------------------------------------
// Not found
// ---------------------------------------------------------------------------

#[test]
fn an_unresolvable_target_names_what_was_looked_for_and_where() {
    let f = GuideFixture::new();
    f.make_run(RUN_FOLDER, CORRELATION);
    let o = f.guide_without_work_root(&["nightly-fix"], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let line = one_line_error(&o);
    assert!(line.starts_with("recipe guide: "), "{line}");
    assert!(line.contains("\"nightly-fix\""), "{line}");
    let session_log = f.recipe_logs.join("nightly-fix.log");
    assert!(line.contains(&session_log.display().to_string()), "{line}");
    assert!(
        line.contains(&f.recipe_logs.display().to_string()),
        "{line}"
    );
    assert!(line.contains(&f.runs().display().to_string()), "{line}");
    assert!(stdout(&o).is_empty());
    assert!(!f.runs().join(RUN_FOLDER).join("guidance").exists());
    assert!(!f.pr_parent().join("guidance").exists());
}

#[test]
fn a_recorded_run_dir_that_is_absent_is_named_in_the_not_found_line() {
    let f = GuideFixture::new();
    let gone = f.alt_work.join("runs").join(RUN_FOLDER);
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&gone)));
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let line = one_line_error(&o);
    assert!(line.contains(&gone.display().to_string()), "{line}");
    assert!(!gone.exists());
}

#[test]
fn control_characters_from_a_log_never_reach_the_message() {
    let f = GuideFixture::new();
    let mut p = GuideFixture::pointer(CORRELATION, None);
    p["run_dir"] = "/tmp/\u{1b}[31mred\u{9b}x/runs/y".into();
    f.session_log_from_pointer(SESSION, &p);
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(
        !err.contains('\u{1b}') && !err.contains('\u{9b}'),
        "{err:?}"
    );
    one_line_error(&o);
}

#[test]
fn a_session_log_named_by_the_operator_that_is_a_symlink_is_still_refused() {
    let f = GuideFixture::new();
    let target = f.root.join("elsewhere.log");
    crate::support::recipe_guide::private_file(&target, "x\n");
    private_dir(&f.recipe_logs);
    symlink(&target, f.recipe_logs.join(format!("{SESSION}.log"))).unwrap();
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    one_line_error(&o);
}

#[test]
fn a_symlink_planted_as_the_guidance_folder_is_not_followed() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let victim = f.root.join("victim");
    private_dir(&victim);
    symlink(&victim, run.join("guidance")).unwrap();
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&run)));
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert_eq!(fs::read_dir(&victim).unwrap().count(), 0);
}

// ---------------------------------------------------------------------------
// Help text
// ---------------------------------------------------------------------------

#[test]
fn guide_help_says_builds_and_drives_take_guidance_and_lists_the_target_forms() {
    let f = GuideFixture::new();
    let o = f.guide(&["--help"], None);
    assert!(o.status.success(), "{}", show(&o));
    let help = stdout(&o).to_lowercase();
    assert!(help.contains("build"), "{help}");
    assert!(help.contains("drive"), "{help}");
    for form in ["session", "run", "issue", "pull request"] {
        assert!(help.contains(form), "help omits {form}: {help}");
    }
    assert!(
        help.contains("quot"),
        "help should note #<number> needs quoting: {help}"
    );
}

// ---------------------------------------------------------------------------
// The record a real run leaves at start
// ---------------------------------------------------------------------------

#[test]
fn a_real_run_records_its_run_dir_in_the_pointer_and_guide_finds_it() {
    let f = GuideFixture::new();
    let alt = f.alt_work.display().to_string();
    let o = f.recipe_run(
        "",
        &[],
        &[
            ("AMPLIHACK_WORK_ROOT", alt.as_str()),
            ("AMPLIHACK_WORK_ROOT_EXPLICIT", "1"),
        ],
    );
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    let line = err
        .lines()
        .find(|l| l.starts_with("amplihack.recipe.log_pointer ") && l.contains("\"early\""))
        .unwrap_or_else(|| panic!("no early pointer line: {}", show(&o)));
    let v: serde_json::Value =
        serde_json::from_str(line.trim_start_matches("amplihack.recipe.log_pointer ")).unwrap();
    assert_eq!(v["schema_version"], 1);
    let run_dir = Path::new(
        v["run_dir"]
            .as_str()
            .expect("early pointer records run_dir"),
    );
    assert!(run_dir.starts_with(f.alt_work.join("runs")), "{run_dir:?}");
    assert!(run_dir.join("run.json").is_file());

    f.session_log_from_pointer(SESSION, &v);
    let o = f.guide_without_work_root(&[SESSION], "after the fact");
    assert_lands_in(&o, run_dir);
}

/// The runner fails once with an unambiguously transient transport fault and
/// succeeds when the wrapper runs it again, so the run really does retry.
const FLAKY_RUNNER: &str = r#"
attempts="$RECORD_DIR/runner.attempts"
n=$(( $(cat "$attempts" 2>/dev/null || echo 0) + 1 ))
printf '%s\n' "$n" > "$attempts"
if [ "$n" -eq 1 ]; then
  echo 'API Error: 529 Overloaded' >&2
  printf '%s\n' '{"recipe_name":"issue-2535-guide","success":false,"status":"failed","step_results":[],"context":{}}'
  exit 1
fi
"#;

/// The early pointer records of `stderr`, in the order they were written.
fn early_pointers(o: &std::process::Output) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(&o.stderr)
        .lines()
        .filter(|l| l.starts_with("amplihack.recipe.log_pointer "))
        .filter_map(|l| {
            serde_json::from_str::<serde_json::Value>(
                l.trim_start_matches("amplihack.recipe.log_pointer "),
            )
            .ok()
        })
        .filter(|v| v["event"] == "early")
        .collect()
}

/// A run that retried has one folder and a correlation id per attempt. The
/// index holds one record per log, so the second attempt's id -- the id
/// `recipe stop` takes and the id that attempt's agent steps carry -- is in
/// no record; the folder every attempt shares is, and its `run.json` lists
/// the id. Real run, real retry, real `run.json` (issue #2818).
///
/// The retry's backoff is a real `thread::sleep` of the planned delay, and
/// the default plan is 10 s with equal jitter -- 5 to 10 s of wall clock on
/// every run of this test, local and CI. `next_backoff` clamps the delay to
/// the budget that remains, so a one-second budget makes the wait one
/// second and leaves the control flow being tested untouched: the loop
/// still makes the second attempt, because the budget bounds the waiting
/// and not the attempts. What this test is about is the second attempt's
/// correlation id, not how long the wrapper waited before making it.
#[test]
fn a_real_retried_run_is_found_by_the_correlation_id_of_its_second_attempt() {
    let f = GuideFixture::new();
    let alt = f.alt_work.display().to_string();
    let o = f.recipe_run(
        FLAKY_RUNNER,
        &[],
        &[
            ("AMPLIHACK_WORK_ROOT", alt.as_str()),
            ("AMPLIHACK_WORK_ROOT_EXPLICIT", "1"),
            ("AMPLIHACK_RECIPE_TRANSIENT_BUDGET_SECS", "1"),
        ],
    );
    assert!(o.status.success(), "{}", show(&o));
    let pointers = early_pointers(&o);
    // Two early pointers is the retry itself: the second attempt happened,
    // under a budget that bounds only the wait before it.
    assert_eq!(
        pointers.len(),
        2,
        "one early pointer per attempt: {}",
        show(&o)
    );
    // The clamp is load-bearing for how long this test takes, so it is
    // asserted rather than assumed. The planned delay is 5 to 10 s before
    // the budget bounds it, so a run that printed anything else here would
    // be one that had stopped reading the budget and gone back to sleeping
    // for the real backoff.
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("Retrying in 1s."),
        "the one-second budget must clamp the backoff: {}",
        show(&o)
    );
    let ids: Vec<&str> = pointers
        .iter()
        .map(|p| p["run_id"].as_str().expect("every pointer names its run"))
        .collect();
    assert_ne!(
        ids[0], ids[1],
        "each attempt has a correlation id of its own"
    );
    let run_dir = Path::new(
        pointers[0]["run_dir"]
            .as_str()
            .expect("early pointer records run_dir"),
    );
    assert_eq!(
        pointers[1]["run_dir"].as_str(),
        Some(run_dir.to_str().unwrap()),
        "both attempts name the one folder"
    );

    // What the folder itself records: every attempt, in order.
    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(run_dir.join("run.json")).unwrap()).unwrap();
    let recorded: Vec<&str> = report["attempts"]
        .as_array()
        .expect("run.json lists attempts")
        .iter()
        .map(|a| a["correlation_id"].as_str().unwrap())
        .collect();
    assert_eq!(recorded, ids, "run.json lists both attempts");

    // The log a detached run leaves holds a pointer line per attempt; only
    // the first is read back, so the second id resolves through `run.json`.
    f.session_log_from_pointers(SESSION, &pointers);
    let o = f.guide_without_work_root(&[ids[1]], "by the id of the running attempt");
    assert_lands_in(&o, run_dir);
    let o = f.guide_without_work_root(&[ids[0]], "by the id of the first attempt");
    assert_lands_in(&o, run_dir);
    assert_eq!(f.entries(&guidance_of(run_dir)).len(), 2);
}

// ---------------------------------------------------------------------------
// Security: the index of records is untrusted input
// ---------------------------------------------------------------------------

use std::os::unix::fs::PermissionsExt;

const ISSUE_URL: &str = "https://github.com/rysweet/amplihack-rs/issues/2783";

fn assert_no_bidi_or_separators(text: &str) {
    for c in [
        '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}',
        '\u{2068}', '\u{2069}', '\u{2028}', '\u{2029}', '\u{1b}', '\u{9b}',
    ] {
        assert!(
            !text.contains(c),
            "U+{:04X} reached stderr: {text:?}",
            c as u32
        );
    }
}

#[test]
fn a_run_dir_with_empty_or_dot_segments_in_the_raw_string_is_not_used() {
    // `Path::components()` drops these segments, so only the raw string shows them.
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let base = run.display().to_string();
    let runs = f.alt_work.join("runs").display().to_string();
    for sneaky in [
        format!("{runs}//{RUN_FOLDER}"),
        format!("{runs}/./{RUN_FOLDER}"),
        format!("{base}/"),
    ] {
        let mut p = GuideFixture::pointer(CORRELATION, None);
        p["run_dir"] = sneaky.clone().into();
        f.session_log_from_pointer(SESSION, &p);
        let o = f.guide_without_work_root(&[SESSION], "text");
        assert_eq!(o.status.code(), Some(1), "{sneaky}: {}", show(&o));
        one_line_error(&o);
        assert!(!run.join("guidance").exists(), "{sneaky}");
    }
}

#[test]
fn a_run_dir_containing_the_replacement_character_is_not_used() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let mut p = GuideFixture::pointer(CORRELATION, None);
    p["run_dir"] = run
        .display()
        .to_string()
        .replace("elsewhere", "else\u{fffd}here")
        .into();
    f.session_log_from_pointer(SESSION, &p);
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_not_found_and_nothing_written(&o, &run);
}

#[test]
fn a_run_dir_whose_runs_folder_is_a_symbolic_link_is_not_used() {
    let f = GuideFixture::new();
    let real = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let linked_work = f.root.join("linked-work");
    private_dir(&linked_work);
    symlink(f.alt_work.join("runs"), linked_work.join("runs")).unwrap();
    let via = linked_work.join("runs").join(RUN_FOLDER);
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&via)));
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_not_found_and_nothing_written(&o, &real);
}

#[test]
fn a_run_dir_that_others_can_write_to_is_not_used() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    fs::set_permissions(&run, fs::Permissions::from_mode(0o777)).unwrap();
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&run)));
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_not_found_and_nothing_written(&o, &run);
}

/// A log planted beside the real ones, naming a run folder whose `run.json`
/// knows nothing of the id the pointer carries. The record is a claim about
/// somebody else's folder, so it is not used, whichever way it is addressed,
/// and the reason names the folder and the id.
#[test]
fn a_record_whose_run_id_disagrees_with_the_report_is_not_used() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let other = "0e9d8c7b-6a5f-4e3d-8c2b-1a0f9e8d7c6b";
    let mut pointer = issue_pointer(&f, &run);
    pointer["run_id"] = other.into();
    f.session_log_from_pointer(SESSION, &pointer);
    for target in [SESSION, other, RUN_FOLDER, ISSUE_URL, "2783"] {
        let o = f.guide_without_work_root(&[target], "recover the recorded run");
        assert_not_found_and_nothing_written(&o, &run);
        let line = one_line_error(&o);
        assert!(line.contains("does not record run"), "{target}: {line}");
        assert!(line.contains(other), "{target}: {line}");
    }
}

/// A report whose own `run_id` is not the folder it sits in names a run that
/// is not the one addressed, so the folder is not used.
#[test]
fn a_report_that_names_another_run_is_not_used() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let report = run.join("run.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
    let other = "0e9d8c7b-6a5f-4e3d-8c2b-1a0f9e8d7c6b";
    value["run_id"] = other.into();
    fs::write(&report, serde_json::to_string(&value).unwrap()).unwrap();
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&run)));
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_not_found_and_nothing_written(&o, &run);
    let line = one_line_error(&o);
    assert!(line.contains("names run"), "{line}");
    assert!(line.contains(other), "{line}");
}

/// A work root, `runs` folder or `run.json` that group or others can write
/// to is not used, and the reason is a line in the not-found message rather
/// than a warning beside a write that went ahead.
///
/// The folder is checked here and appended to later, and `append_entry`
/// rechecks only the run folder and the guidance folder beneath it. So a
/// user who can write to `runs` could rename the run folder aside between
/// the two and put their own in its place, and the entry would land in it
/// (CWE-367). Trying the next rule costs nothing; accepting this does.
#[test]
fn a_writable_work_root_runs_folder_or_report_is_not_used() {
    for (level, mode) in [
        ("root", 0o770),
        ("root", 0o707),
        ("runs", 0o770),
        ("runs", 0o707),
        ("run", 0o770),
        ("run.json", 0o660),
    ] {
        let f = GuideFixture::new();
        let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
        let path = match level {
            "root" => f.alt_work.clone(),
            "runs" => f.alt_work.join("runs"),
            "run" => run.clone(),
            _ => run.join("run.json"),
        };
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&run)));
        let o = f.guide_without_work_root(&[SESSION], "no write under a writable path");
        assert_not_found_and_nothing_written(&o, &run);
        let line = one_line_error(&o);
        assert!(
            line.contains(&path.display().to_string()),
            "{level} {mode:04o}: {line}"
        );
        assert!(
            line.contains("writable by group or others"),
            "{level} {mode:04o}: {line}"
        );
        // Nothing changed the permissions it read.
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            mode,
            "{level}"
        );
        fs::set_permissions(&f.alt_work, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn a_note_about_one_record_does_not_stop_a_later_record_from_matching() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let mut bad = GuideFixture::pointer("1b2c3d4e-0000-4000-8000-000000000001", None);
    bad["run_dir"] = "relative/runs/x".into();
    bad["issue_number"] = "2783".into();
    f.session_log_from_pointer("amp-bad-20261118-000001", &bad);
    f.session_log_from_pointer(SESSION, &issue_pointer(&f, &run));
    let o = f.guide_without_work_root(&["2783"], "still found");
    assert_lands_in(&o, &run);
}

#[test]
fn a_log_directory_others_can_write_to_is_not_scanned_and_says_so() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &issue_pointer(&f, &run));
    fs::set_permissions(&f.recipe_logs, fs::Permissions::from_mode(0o777)).unwrap();
    let o = f.guide_without_work_root(&[ISSUE_URL], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let line = one_line_error(&o);
    assert!(
        line.contains(&f.recipe_logs.display().to_string()),
        "{line}"
    );
    assert!(!run.join("guidance").exists());
    fs::set_permissions(&f.recipe_logs, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a_fifo_named_like_a_log_does_not_block_or_fail_the_lookup() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &issue_pointer(&f, &run));
    let fifo = f.recipe_logs.join("amp-fifo-20261118-000002.log");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    // The harness fails the test if this blocks; there is no timer here.
    let o = f.guide_without_work_root(&["2783"], "past the fifo");
    assert_lands_in(&o, &run);
}

#[test]
fn a_symbolic_link_named_like_a_log_is_skipped_by_the_scan() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let elsewhere = f.root.join("outside.log");
    crate::support::recipe_guide::private_file(
        &elsewhere,
        format!("amplihack.recipe.log_pointer {}\n", issue_pointer(&f, &run)),
    );
    private_dir(&f.recipe_logs);
    symlink(
        &elsewhere,
        f.recipe_logs.join("amp-link-20261118-000003.log"),
    )
    .unwrap();
    let o = f.guide_without_work_root(&["2783"], "text");
    assert_not_found_and_nothing_written(&o, &run);
}

#[test]
fn a_deeply_nested_pointer_line_is_skipped_not_fatal() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &issue_pointer(&f, &run));
    let depth = 5000;
    let nested = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
    private_dir(&f.recipe_logs);
    crate::support::recipe_guide::private_file(
        &f.recipe_logs.join("amp-deep-20261118-000004.log"),
        format!("amplihack.recipe.log_pointer {nested}\n"),
    );
    let o = f.guide_without_work_root(&["2783"], "past the nesting");
    assert_lands_in(&o, &run);
}

#[test]
fn a_pointer_line_over_the_old_size_limit_is_found() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let mut big = GuideFixture::pointer(CORRELATION, Some(&run));
    big["issue_number"] = "2783".into();
    big["cwd"] = "x".repeat(200_000).into();
    private_dir(&f.recipe_logs);
    crate::support::recipe_guide::private_file(
        &f.recipe_logs.join("amp-big-20261118-000005.log"),
        format!("amplihack.recipe.log_pointer {big}\n"),
    );
    let o = f.guide_without_work_root(&["2783"], "past the big line");
    assert_lands_in(&o, &run);
}

#[test]
fn a_long_recorded_field_does_not_prevent_a_match_on_another_field() {
    // Unknown fields do not interfere with recovery of run_dir.
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let mut p = issue_pointer(&f, &run);
    p["pr_url"] = "y".repeat(20 * 1024).into();
    f.session_log_from_pointer(SESSION, &p);
    let o = f.guide_without_work_root(&["2783"], "text");
    assert_lands_in(&o, &run);
}

#[test]
fn a_leading_plus_or_non_digits_in_a_recorded_issue_number_do_not_match() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let mut p = GuideFixture::pointer(CORRELATION, Some(&run));
    p["issue_number"] = "+2783".into();
    f.session_log_from_pointer(SESSION, &p);
    let o = f.guide_without_work_root(&["2783"], "text");
    assert_not_found_and_nothing_written(&o, &run);
}

#[test]
fn issue_urls_match_owner_and_repository_case_insensitively_and_never_by_substring() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &issue_pointer(&f, &run));
    let o = f.guide_without_work_root(
        &["https://github.com/RySweet/Amplihack-RS/issues/2783"],
        "case",
    );
    assert_lands_in(&o, &run);
    let o = f.guide_without_work_root(
        &["https://github.com/rysweet/amplihack-rs-fork/issues/2783"],
        "other repo",
    );
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert_eq!(f.entries(&guidance_of(&run)).len(), 1);
}

#[test]
fn bidirectional_and_line_separator_characters_never_reach_stderr() {
    let f = GuideFixture::new();
    // In the operator's target.
    let target = "a\u{202e}b\u{2066}c\u{2069}d\u{2028}e\u{2029}f";
    let o = f.guide_without_work_root(&[target], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    one_line_error(&o);
    assert_no_bidi_or_separators(&String::from_utf8_lossy(&o.stderr));
    // In a recorded value that a validator message embeds.
    let mut p = GuideFixture::pointer(CORRELATION, None);
    p["run_dir"] = "/tmp/\u{202e}x\u{2028}y\u{2029}z\u{2066}/runs/q".into();
    f.session_log_from_pointer(SESSION, &p);
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    one_line_error(&o);
    assert_no_bidi_or_separators(&String::from_utf8_lossy(&o.stderr));
}

#[test]
fn a_guidance_folder_swapped_for_a_symlink_after_acceptance_is_not_followed() {
    // The recorded run is accepted, then `guidance` is a symlink to a folder
    // outside the run: the write must be refused by the descriptor-relative
    // open, whichever check saw the path first.
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let victim = f.root.join("victim-after-acceptance");
    private_dir(&victim);
    symlink(&victim, run.join("guidance")).unwrap();
    f.session_log_from_pointer(SESSION, &issue_pointer(&f, &run));
    for target in [SESSION, "2783", ISSUE_URL] {
        let o = f.guide_without_work_root(&[target], "text");
        assert_eq!(o.status.code(), Some(1), "{target}: {}", show(&o));
    }
    assert_eq!(fs::read_dir(&victim).unwrap().count(), 0);
}

#[test]
fn a_bare_number_matching_several_runs_warns_on_stderr_and_keeps_stdout_unchanged() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &issue_pointer(&f, &run));
    let second_folder = "9c8b7a65-4d3e-4f21-a0b9-8c7d6e5f4a3b";
    let second_corr = "5e4d3c2b-1a09-4876-b5a4-c3d2e1f00a9b";
    let second = f.make_run_in_alt_work(second_folder, second_corr);
    let mut p = GuideFixture::pointer(second_corr, Some(&second));
    p["issue_number"] = "2783".into();
    p["issue_url"] = "https://github.com/someone/else/issues/2783".into();
    f.session_log_from_pointer("amp-other-20261118-205100", &p);
    let o = f.guide_without_work_root(&["2783"], "ambiguous");
    assert!(o.status.success(), "{}", show(&o));
    let (path, _, source) = parse_appended(&o);
    assert_eq!(source, "run");
    assert!(path.starts_with(f.alt_work.join("runs")), "{path:?}");
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(
        err.contains(RUN_FOLDER) || err.contains(second_folder),
        "warning should name the chosen run: {err}"
    );
}

#[test]
fn an_unset_home_is_reported_in_the_not_found_line_not_refused() {
    let f = GuideFixture::new();
    let mut c = f.amplihack(&["recipe", "guide", "nightly-fix", "-"]);
    c.env_remove("HOME")
        .env_remove("AMPLIHACK_WORK_ROOT")
        .env_remove("AMPLIHACK_WORK_ROOT_EXPLICIT");
    let o = f.run_with_stdin(c, Some(b"text"));
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let line = one_line_error(&o);
    assert!(line.to_lowercase().contains("home"), "{line}");
}

// ---------------------------------------------------------------------------
// Issue #2818: the session name a detached run was actually given
//
// `recipe detach` builds session names with `.take(80)` and may add a `-<n>`
// collision suffix on top, so it emits up to 85 characters. The lookup used
// to apply the 64 of the rule for a name tmux is asked to create, so every
// detached session named past 64 characters fell through the session rule,
// and fell through it silently. These exercise the real binary with both
// work-root variables unset, on names of exactly that shape.
// ---------------------------------------------------------------------------

/// The session name the drive in the original report ran under: the default
/// `amp-{recipe}-{repo}-{ts}` pattern, an auto-drive recipe and a per-PR
/// worktree. 65 characters, one past the creation rule.
const DRIVE_SESSION: &str = "amp-auto-drive-to-merge-amplihack-rs-drive-pr2814-20261119-000213";

/// A name at the 85 detach can emit: the 80 `session_name` truncates to,
/// plus the longest `-<n>` suffix `claim_session` adds on a collision.
const SESSION_AT_BOUND: &str =
    "amp-auto-drive-to-merge-amplihack-rs-drive-pr2814-20261119-000213-abcdefghijklmn-1000";

#[test]
fn the_sessions_these_tests_use_are_the_lengths_they_claim() {
    assert_eq!(DRIVE_SESSION.chars().count(), 65);
    assert_eq!(SESSION_AT_BOUND.chars().count(), 85);
    // The guard against a future edit trimming these back under the old
    // bound, which would leave the suite green and the bug alive.
    assert!(SESSION.chars().count() <= 64);
}

#[test]
fn a_sixty_five_character_drive_session_resolves_through_its_recorded_run_dir() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(
        DRIVE_SESSION,
        &GuideFixture::pointer(CORRELATION, Some(&run)),
    );
    let o = f.guide_without_work_root(&[DRIVE_SESSION], "guide the drive");
    assert_lands_in(&o, &run);
    assert_eq!(f.entries(&guidance_of(&run)).len(), 1);
    assert!(
        !f.runs().join(RUN_FOLDER).exists(),
        "wrote under the default root"
    );
}

#[test]
fn a_session_at_the_detach_bound_resolves_too() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(
        SESSION_AT_BOUND,
        &GuideFixture::pointer(CORRELATION, Some(&run)),
    );
    let o = f.guide_without_work_root(&[SESSION_AT_BOUND], "guide the drive");
    assert_lands_in(&o, &run);
}

/// A long session also resolves through the work root in the environment,
/// the second half of rule 3, so the bound is not fixed on one path only.
#[test]
fn a_sixty_five_character_session_with_no_recorded_run_dir_resolves_through_the_environment() {
    let f = GuideFixture::new();
    let run = f.make_run(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(DRIVE_SESSION, &GuideFixture::pointer(CORRELATION, None));
    let o = f.guide_without_work_root(&[DRIVE_SESSION], "guide the drive");
    assert_lands_in(&o, &run);
}

#[test]
fn a_long_session_whose_log_is_absent_names_the_log_it_looked_for() {
    let f = GuideFixture::new();
    f.make_run(RUN_FOLDER, CORRELATION);
    let o = f.guide_without_work_root(&[DRIVE_SESSION], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let line = one_line_error(&o);
    let log = f.recipe_logs.join(format!("{DRIVE_SESSION}.log"));
    assert!(line.contains(&log.display().to_string()), "{line}");
}

/// Past the bound the rule cannot run, and the not-found line says so
/// rather than listing only the places it did reach.
#[test]
fn a_name_past_the_detach_bound_says_the_session_rule_was_not_tried() {
    let f = GuideFixture::new();
    f.make_run(RUN_FOLDER, CORRELATION);
    let too_long = format!("{SESSION_AT_BOUND}-and-more");
    assert_eq!(too_long.chars().count(), 94);
    let o = f.guide_without_work_root(&[&too_long], "text");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let line = one_line_error(&o);
    assert!(line.contains("no session log ("), "{line}");
    assert!(line.contains("94 characters"), "{line}");
    assert!(line.contains("at most 85"), "{line}");
    // The miss is still a miss, and still names everywhere else it looked.
    assert!(
        line.contains(&f.recipe_logs.display().to_string()),
        "{line}"
    );
    assert!(stdout(&o).is_empty());
}

#[test]
fn a_target_that_could_never_be_a_session_name_says_which_character_stopped_it() {
    let f = GuideFixture::new();
    f.make_run(RUN_FOLDER, CORRELATION);
    for (target, needle) in [("runs/nightly", "\"/\""), ("amp:one", "\":\"")] {
        let o = f.guide_without_work_root(&[target], "text");
        assert_eq!(o.status.code(), Some(1), "{}", show(&o));
        let line = one_line_error(&o);
        assert!(line.contains("no session log ("), "{line}");
        assert!(line.contains(needle), "{target}: {line}");
    }
}

// ---------------------------------------------------------------------------
// The success path is escaped too (issue #2818, CWE-150)
// ---------------------------------------------------------------------------

/// An ancestor named with a terminal title sequence: user-owned, so every
/// trust and ownership check on the way down passes.
const HOSTILE_ANCESTOR: &str = "x\u{1b}]0;pwned\u{7}y";

/// Issue #2818: the success path prints the folder the entry was written to.
/// It used to print it raw, and only the failure path went through
/// `sanitize_line`, so a destination whose ancestor is named with a terminal
/// sequence reached the terminal as itself.
///
/// The destination here comes from `AMPLIHACK_WORK_ROOT`, which no check
/// reads the characters of, so the folder really is used: this pins the
/// escaping of the printed line, not a refusal standing in for it.
#[test]
fn the_success_line_escapes_a_control_character_in_the_folder_it_names() {
    let f = GuideFixture::new();
    let hostile_root = f.root.join(HOSTILE_ANCESTOR).join("work");
    let run = f.make_run_in(&hostile_root, RUN_FOLDER, CORRELATION);

    let mut c = f.amplihack(&["recipe", "guide", RUN_FOLDER, "-"]);
    c.env("AMPLIHACK_WORK_ROOT", &hostile_root);
    let o = f.run_with_stdin(c, Some(b"guide the run"));

    assert!(o.status.success(), "{}", show(&o));
    // The entry landed in the folder, so the line names it: nothing was
    // refused and no other folder stood in for it.
    assert_eq!(f.entries(&guidance_of(&run)).len(), 1, "{}", show(&o));
    let out = stdout(&o);
    assert_eq!(out.lines().count(), 1, "{}", show(&o));
    assert!(
        out.contains("\\u{001b}") && out.contains("\\u{0007}"),
        "{out}"
    );
    assert!(
        !out.contains('\u{1b}') && !out.contains('\u{7}'),
        "a terminal sequence reached stdout: {out:?}"
    );
    assert_no_bidi_or_separators(&out);
    assert_no_bidi_or_separators(&stderr(&o));
    // The contract of the line is otherwise unchanged.
    let (path, _, source) = parse_appended(&o);
    assert_eq!(source, "run");
    assert!(path.to_string_lossy().contains("\\u{001b}"), "{path:?}");
}

/// The same hostile ancestor, reached the other way: a record claims it.
/// `accept_recorded` declines to use the claim on its characters, before any
/// of the checks that read only the last two components and the work root.
/// The claim is not used rather than refused, so the rest of rule 3 still
/// reaches the real run through the work root in the environment
/// (issue #2809).
#[test]
fn a_recorded_run_folder_with_a_control_character_in_an_ancestor_is_passed_over() {
    let f = GuideFixture::new();
    let real = f.make_run(RUN_FOLDER, CORRELATION);
    let planted = f.root.join(HOSTILE_ANCESTOR).join("work");
    let claimed = f.make_run_in(&planted, RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&claimed)));

    let o = f.guide_without_work_root(&[SESSION], "text");

    assert_lands_in(&o, &real);
    assert!(
        !claimed.join("guidance").exists(),
        "wrote into the folder the record claimed"
    );
    assert_no_bidi_or_separators(&stdout(&o));
    assert_no_bidi_or_separators(&stderr(&o));
}

/// With nothing left to fall back to, the claim becomes a reason in the
/// not-found line, and the character that stopped it is named as an escape.
#[test]
fn a_rejected_hostile_run_folder_is_named_in_the_not_found_line_as_an_escape() {
    let f = GuideFixture::new();
    let planted = f.root.join(HOSTILE_ANCESTOR).join("work");
    let claimed = f.make_run_in(&planted, RUN_FOLDER, CORRELATION);
    // No run of that correlation id under the work root in the environment,
    // so the second half of rule 3 finds nothing either.
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&claimed)));

    let o = f.guide_without_work_root(&[SESSION], "text");

    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(stdout(&o).is_empty(), "{}", show(&o));
    let line = one_line_error(&o);
    assert!(line.contains("\\u{001b}"), "{line}");
    assert!(line.contains("in its path"), "{line}");
    assert_no_bidi_or_separators(&line);
    assert!(!claimed.join("guidance").exists());
}
