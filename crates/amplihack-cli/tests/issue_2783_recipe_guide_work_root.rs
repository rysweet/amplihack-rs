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
    GuideFixture, one_line_error, parse_appended, private_dir, show, stdout,
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
// A recorded path is untrusted
// ---------------------------------------------------------------------------

fn assert_refused_and_nothing_written(o: &std::process::Output, dir: &Path) {
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
    assert_refused_and_nothing_written(&o, &f.alt_work.join("runs").join(RUN_FOLDER));
}

#[test]
fn a_run_dir_whose_parent_is_not_named_runs_is_not_used() {
    let f = GuideFixture::new();
    let dir = f.root.join("not-runs").join(RUN_FOLDER);
    private_dir(&dir);
    crate::support::recipe_guide::private_file(&dir.join("run.json"), "{}");
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&dir)));
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_refused_and_nothing_written(&o, &dir);
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
    assert_refused_and_nothing_written(&o, &real);
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
    assert_refused_and_nothing_written(&o, &dir);
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
    assert_refused_and_nothing_written(&o, &run);
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
    assert_refused_and_nothing_written(&o, &real);
}

#[test]
fn a_run_dir_that_others_can_write_to_is_not_used() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    fs::set_permissions(&run, fs::Permissions::from_mode(0o777)).unwrap();
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(CORRELATION, Some(&run)));
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_refused_and_nothing_written(&o, &run);
}

#[test]
fn a_record_whose_run_id_disagrees_with_the_folder_run_json_is_not_used() {
    // A shadow log naming a real folder with another run's correlation id.
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    let other = "0e9d8c7b-6a5f-4e3d-8c2b-1a0f9e8d7c6b";
    f.session_log_from_pointer(SESSION, &GuideFixture::pointer(other, Some(&run)));
    let o = f.guide_without_work_root(&[SESSION], "text");
    assert_refused_and_nothing_written(&o, &run);
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
    assert_refused_and_nothing_written(&o, &run);
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
fn an_oversize_pointer_line_is_skipped_not_fatal() {
    let f = GuideFixture::new();
    let run = f.make_run_in_alt_work(RUN_FOLDER, CORRELATION);
    f.session_log_from_pointer(SESSION, &issue_pointer(&f, &run));
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
    // Per-field caps: a 20 KiB pr_url must not be held or matched, yet the
    // 20 KiB-bearing line (under the 32 KiB line limit) still yields its run_dir.
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
    assert_refused_and_nothing_written(&o, &run);
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
