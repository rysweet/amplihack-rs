//! #2849 item 1: the merge-ready loop driver's output is kept in the PR's
//! state folder, and a hand-off that says `UNREADABLE` names it.
//!
//! Contract: docs/reference/autodrive-merge-ready-round.md#kept-driver-output.
//!
//! - `autodrive_merge_loop_run.sh` keeps everything the loop driver writes
//!   (standard output and standard error together) in
//!   `<state_dir>/merge-ready-driver.err`, mode `0600`, replaced at the start
//!   of each run of the tool. The step's own standard error carries the same
//!   bytes as before, and its standard output is still only the loop line.
//! - When the loop result is `UNREADABLE`, the loop line's `reason` names the
//!   kept file (bare name first, then its absolute path) and says whether
//!   `merge-ready-result.json` is absent, unreadable or says a loop result
//!   (at most 64 characters of `[A-Za-z0-9_-]`). The tool never reads the kept
//!   file, so no line the driver printed is in the reason: the hand-off agent
//!   reads the file itself. `step-02a` carries the reason through to the
//!   hand-off.
//! - The copy is evidence, not a condition. No exit status, no loop line other
//!   than the `UNREADABLE` one, and no decision changes; a copy that cannot
//!   be made is not an error, and the reason then says so. A copy that stopped
//!   part-way says so too. Both are said once on the step's standard error, on
//!   every ending, not only `UNREADABLE`.
//! - The copy does not rest on a program the host supplies. A driver that
//!   writes more than a pipe buffer loses nothing from the copy or the log,
//!   and a `tee` on `PATH` that fails changes neither.
//! - The kept file is written only through a descriptor opened once, under
//!   umask 077, in a state folder the tool has checked: a link at its name is
//!   replaced, never followed, and the driver does not inherit the descriptor.
//!
//! A child of issue_2575_agentic_merge_ready.rs: it runs the real
//! `autodrive_merge_loop_run.sh` and the real `step-02` and `step-02a`
//! commands from the copied bundle, with the stub loop driver (`FAKE_LOOP_DRIVER`)
//! standing in for `autodrive_loop.sh`. Every assertion is on files left on
//! disk, on the step's exit status and on the JSON it emits; no source text of
//! the tool or the document is read. Nothing reaches GitHub or an agent.
//!
//! The agent that writes the hand-off comment is not run; `step-02a`'s
//! `reason` is its `handoff_reason` input.

use super::*;
use std::fs;

const NAME: &str = "merge-ready-driver.err";
const RESULT_NAME: &str = "merge-ready-result.json";

/// The wording the document pins for the output clause of the reason. Kept in
/// one place so a wording change is one edit.
const NOT_KEPT: &str = "its output could not be kept";

/// What the step says on its standard error when the copy was not made, or
/// was made only in part. The reason carries neither on an ending other than
/// `UNREADABLE`, so the log is the only place they appear there.
const NOT_KEPT_NOTICE: &str = "NOTICE: the loop driver output could not be kept";
const PARTIAL_NOTICE: &str = "NOTICE: only part of the loop driver output could be kept";

const SCHEMA_PREFIX: &str = "loop result UNREADABLE: ";

fn kept(fx: &Fx) -> PathBuf {
    fx.state.join(NAME)
}

fn result_path(fx: &Fx) -> PathBuf {
    fx.state.join(RESULT_NAME)
}

/// The output clause for a copy that was made whole. It ends at the path, with
/// nothing after it: the reason joins its clauses with `; `, so a stray space
/// or a trailing remark shows up in [`expected_reason`].
fn kept_clause(fx: &Fx) -> String {
    format!("its output was kept in {NAME} at {}", kept(fx).display())
}

/// The wording for a copy the step could not finish writing.
fn partial_clause(fx: &Fx) -> String {
    format!(
        "only part of its output could be kept, in {NAME} at {}",
        kept(fx).display()
    )
}

/// The whole `UNREADABLE` reason for a driver that exited `rc`, given the
/// output clause and what became of the result file (`is absent`,
/// `is unreadable` or `says DONE`). Every word of it is the tool's own, so it
/// is pinned whole: a driver's words or a stray character in it is a difference
/// this comparison reports.
fn expected_reason(fx: &Fx, rc: &str, clause: &str, state: &str) -> String {
    format!(
        "the loop driver exited {rc} without a readable {RESULT_NAME}; {clause}; \
         {RESULT_NAME} {state} at {}",
        result_path(fx).display()
    )
}

/// Driver output that no reason may carry: it is in the kept file and in the
/// log, and the hand-off agent reads the file itself.
fn assert_no_driver_text(text: &str, words: &[&str]) {
    for word in words {
        assert!(!text.contains(word), "driver text {word:?} reached: {text}");
    }
}

fn case(rc: &'static str, result: Option<&'static str>, stderr: Option<&str>) -> LoopCase {
    LoopCase {
        rc,
        result,
        printed: None,
        stderr: stderr.map(str::to_string),
    }
}

/// The loop line `step-02` printed.
fn loop_line(out: &Output) -> Value {
    last_json(out, "loop_result")
}

fn loop_reason(out: &Output) -> String {
    loop_line(out)["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("no reason in the loop line:\n{}", show(out)))
        .to_string()
}

/// `step-02a`'s reason: the text the hand-off receives as `handoff_reason`.
fn outcome_reason(classified: &Output) -> String {
    last_json(classified, "outcome")["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("no reason in the outcome:\n{}", show(classified)))
        .to_string()
}

/// Runs `step-02` with a driver that exits 1 with no result file and writes
/// `stderr`; returns the loop line's reason and the step. The hand-off step
/// is a second process, so the tests that do not read the hand-off's own
/// reason stop here and [`unreadable`] adds it for those that do.
fn unreadable_line(fx: &Fx, stderr: Option<&str>) -> (String, Output) {
    let out = loop_step_02(fx, &case("1", None, stderr));
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    (loop_reason(&out), out)
}

/// [`unreadable_line`], carried on through `step-02a`: the loop line's
/// reason, the hand-off reason and step-02.
fn unreadable(fx: &Fx, stderr: Option<&str>) -> (String, String, Output) {
    let (why, out) = unreadable_line(fx, stderr);
    let classified = loop_outcome_step(fx, &out);
    assert_eq!(classified.status.code(), Some(0), "{}", show(&classified));
    assert_eq!(
        last_json(&classified, "outcome")["outcome"],
        json!("handoff")
    );
    (why, outcome_reason(&classified), out)
}

/// `loop_step` with extra stub-driver fixture files set for the one run.
fn with_extras(fx: &Fx, extras: &[(&str, &str)], case: &LoopCase) -> (Output, Option<Output>) {
    for (name, value) in extras {
        fx.set(name, value);
    }
    let r = loop_step(fx, case);
    for (name, _) in extras {
        let _ = fs::remove_file(fx.fake.join(name));
    }
    r
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn stderr_of(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

// ===========================================================================
// The acceptance case from #2849: a stub driver exits 1 after writing to
// standard error. The hand-off names the kept file and carries none of its
// text, and the state folder still holds it once the run is over.
// ===========================================================================

#[test]
fn unreadable_handoff_names_the_kept_file_and_carries_none_of_its_text() {
    let Some(fx) = loop_fixture() else { return };
    let first = "INFO: round 1 started";
    let last = "ERROR: AMPLIHACK_MAX_DEPTH changed from '3' to '4' inside the loop.";
    let (why, handoff_why, out) = unreadable(&fx, Some(&format!("{first}\n{last}\n")));

    // The step ended with exit 0 and the loop result it always gave.
    assert_eq!(loop_line(&out)["loop_result"], json!("UNREADABLE"));

    // The reason is the tool's own words, whole: the opening it has always
    // had, the kept file named by its bare name and then its absolute path,
    // and what became of the result file. Nothing follows the path but the
    // `; ` that joins the next clause.
    assert_eq!(
        why,
        expected_reason(&fx, "1", &kept_clause(&fx), "is absent"),
        "{}",
        show(&out)
    );
    assert!(
        why.find(NAME).unwrap() < why.find(&*kept(&fx).to_string_lossy()).unwrap(),
        "the bare name comes before the path: {why}"
    );

    // The hand-off's input is that reason and the schema prefix, no more.
    assert_eq!(handoff_why, format!("{SCHEMA_PREFIX}{why}"));
    assert!(handoff_why.len() <= 1000, "{}", handoff_why.len());

    // Nothing the driver wrote, from either line, is in either reason, and the
    // step's standard output is still only the loop line. The text is in the
    // kept file.
    for text in [&why, &handoff_why] {
        assert_no_driver_text(text, &["round 1 started", "AMPLIHACK_MAX_DEPTH"]);
    }
    assert!(
        !stdout(&out).contains(first) && !stdout(&out).contains(last),
        "{}",
        show(&out)
    );

    // The state folder still holds the evidence once both steps have ended,
    // byte for byte, and the log has it too.
    let file = kept(&fx);
    let meta = fs::symlink_metadata(&file).expect("the kept file exists after the run");
    assert!(meta.file_type().is_file(), "a regular file, not a link");
    assert_eq!(mode(&file), 0o600, "private, like every state file");
    assert_eq!(read(&file), format!("{first}\n{last}\n"));
    assert!(
        stderr_of(&out).contains(&format!("{first}\n{last}\n")),
        "{}",
        show(&out)
    );
}

#[test]
fn what_the_driver_prints_reaches_the_log_unchanged_and_in_order() {
    let Some(fx) = loop_fixture() else { return };
    let out = loop_step_02(
        &fx,
        &LoopCase {
            rc: "1",
            result: Some("STUCK"),
            printed: Some("driver says hello\n".into()),
            stderr: Some("driver says goodbye, cut off mid-wo".into()),
        },
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    // Standard output and standard error share one stream, in the order the
    // driver wrote them. A last line the driver ended without a newline keeps
    // its own bytes, with nothing added to them.
    assert_eq!(
        read(&kept(&fx)),
        "driver says hello\ndriver says goodbye, cut off mid-wo"
    );
    // The step's standard error has the same bytes, so the run log is unchanged.
    assert!(
        stderr_of(&out).contains("driver says hello\ndriver says goodbye, cut off mid-wo"),
        "{}",
        show(&out)
    );
    // The step's standard output is still only the loop line.
    let lines: Vec<String> = stdout(&out)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    assert_eq!(lines.len(), 1, "{}", show(&out));
    assert!(!lines[0].contains("driver says"), "{}", lines[0]);
    assert_eq!(
        serde_json::from_str::<Value>(&lines[0]).unwrap()["loop_result"],
        json!("STUCK")
    );
}

#[test]
fn a_driver_that_outtalks_a_pipe_buffer_loses_nothing_from_either_place() {
    let Some(fx) = loop_fixture() else { return };
    // A round writes the tail of its own log to the driver's standard error,
    // so the output of a real loop is tens of kilobytes, not one line. The
    // copy and the log must both hold every byte of it, run after run: an
    // external duplicator that gives up part-way through anything larger than
    // one pipe buffer takes the bytes it has already read with it, and the
    // copy and the log lose the output together. That is worse than keeping
    // no copy at all, because the log alone used to be enough.
    //
    // Eight runs, because such a loss is a race and shows up in a fraction of
    // them, not in all. (uutils coreutils 0.12.0, which Ubuntu 25.10 installs
    // as the default `tee`, aborts on roughly one run in ten.)
    let filler: String = (0..1500)
        .map(|i| format!("[round-3] {i:04} considering head 4fe286b3 against base main\n"))
        .collect();
    let last = "ERROR: the head moved while the round was running";
    let written = format!("{filler}{last}\n");
    assert!(
        written.len() > 64 * 1024,
        "past a 64 KiB pipe buffer: {}",
        written.len()
    );

    for run in 1..=8 {
        let (why, out) = unreadable_line(&fx, Some(&written));
        assert_eq!(
            read(&kept(&fx)),
            written,
            "run {run}: the copy is not the driver's output"
        );
        assert!(
            stderr_of(&out).contains(&written),
            "run {run}: the log lost the driver's output"
        );
        // So the reason makes the whole-copy claim, rather than falling back
        // to the part-way wording, and carries none of the driver's words.
        assert_eq!(
            why,
            expected_reason(&fx, "1", &kept_clause(&fx), "is absent"),
            "run {run}"
        );
        assert_no_driver_text(&why, &[last, "considering head"]);
        assert!(
            !stderr_of(&out).contains(PARTIAL_NOTICE),
            "run {run}: {}",
            show(&out)
        );
    }
}

#[test]
fn a_tee_that_fails_on_path_changes_nothing_about_the_copy_or_the_log() {
    let Some(fx) = loop_fixture() else { return };
    // The deterministic half of the case above. The copy is the step's own
    // account of a driver that died, so it cannot rest on a program the host
    // supplies: a `tee` that fails, or is missing, or copies only part of
    // what it reads, must leave the kept file and the log exactly as they
    // would be without it. A host `tee` is not hypothetical: Ubuntu 25.10
    // installs the uutils one, which aborts part-way through a copy larger
    // than a pipe buffer, so here it fails outright, which an external
    // duplicator could not survive and this step must.
    let dir = fx.root.join("failing-tee");
    fs::create_dir_all(&dir).unwrap();
    write_executable(&dir.join("tee"), "#!/bin/sh\nexit 1\n", 0o755);
    let written = "INFO: round 1 started\nERROR: the driver gave up\n";

    stage_loop_driver(&fx, &case("1", None, Some(written)));
    let mut c = fx.bash();
    c.arg("-c").arg(&loop_commands().0).env(
        "PATH",
        format!(
            "{}:{}:{}",
            dir.display(),
            fx.bin.display(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
        ),
    );
    loop_env(&mut c, &fx, &loop_preflight(&fx));
    let out = c
        .output()
        .expect("run merge loop step-02 with a failing tee");

    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(loop_line(&out)["loop_result"], json!("UNREADABLE"));
    assert_eq!(read(&kept(&fx)), written, "{}", show(&out));
    assert!(stderr_of(&out).contains(written), "{}", show(&out));
    let why = loop_reason(&out);
    assert_eq!(
        why,
        expected_reason(&fx, "1", &kept_clause(&fx), "is absent"),
        "{}",
        show(&out)
    );
    assert!(!stderr_of(&out).contains(PARTIAL_NOTICE), "{}", show(&out));
}

// ===========================================================================
// Nothing else changes: every exit status, and every loop line that is not
// the UNREADABLE one, is what it was before the output was kept.
// ===========================================================================

#[test]
fn every_driver_ending_is_unchanged_and_keeps_its_output() {
    let Some(fx) = loop_fixture() else { return };
    let stub = |r: &str| {
        format!(
            r#"{{"loop":"merge-ready","loop_result":"{r}","round_label":"round-2","reason":"fixture stop"}}"#
        )
    };
    let refusal = r#"{"loop":"merge-ready","loop_result":"TERMINAL_REFUSAL","round_label":"unknown","reason":"the merge-ready loop exited 79"}"#;
    // Driver exit, its result file, the step's exit, and the loop line the
    // step prints. `None` is the UNREADABLE line, whose reason is this
    // change's one new text and is asserted on throughout the rest of this
    // file; every other line must come out byte for byte as it did before.
    //
    // The exit status is the driver's, with the copy in the pipeline. (A copy
    // that itself fails is `a_failing_copy_never_changes_...` below; with a
    // copy that works, the step's `pipefail` makes the pipeline's status the
    // driver's anyway.)
    for (n, (rc, result, exit, printed)) in [
        ("79", None, 79, Some(refusal.to_string())),
        ("79", Some("DONE"), 79, Some(refusal.to_string())),
        ("1", Some("USAGE_LIMIT"), 1, Some(stub("USAGE_LIMIT"))),
        ("75", Some("ORPHANED"), 75, Some(stub("ORPHANED"))),
        ("2", None, 2, None),
        ("3", None, 3, None),
        ("0", Some("DONE"), 0, Some(stub("DONE"))),
        ("0", Some("WAITING"), 0, Some(stub("WAITING"))),
        ("0", None, 0, None),
        ("1", Some("STUCK"), 0, Some(stub("STUCK"))),
        (
            "1",
            Some("BUDGET_EXHAUSTED"),
            0,
            Some(stub("BUDGET_EXHAUSTED")),
        ),
        ("1", Some("HANDOFF"), 0, Some(stub("HANDOFF"))),
        ("1", None, 0, None),
    ]
    .into_iter()
    .enumerate()
    {
        // A marker no other run writes, so the kept file is this run's alone.
        let marker = format!("marker-{n}-for-driver-exit-{rc}\n");
        let out = loop_step_02(&fx, &case(rc, result, Some(&marker)));
        assert_eq!(
            out.status.code(),
            Some(exit),
            "driver {rc}/{result:?}: {}",
            show(&out)
        );
        assert_eq!(
            fs::read_to_string(kept(&fx)).ok().as_deref(),
            Some(marker.as_str()),
            "driver {rc}/{result:?} keeps its output, and only this run's"
        );
        // Where there is a line other than UNREADABLE it is byte-identical,
        // which is also what keeps the driver's output out of it.
        if let Some(line) = printed {
            assert_eq!(
                stdout(&out).trim_end(),
                line,
                "driver {rc}/{result:?}: {}",
                show(&out)
            );
        }
    }
}

// ===========================================================================
// What the reason says about the driver's output: where it is, and nothing of
// what it says. The kept file is the only place the words are.
// ===========================================================================

#[test]
fn a_driver_that_wrote_nothing_still_names_its_empty_file() {
    let Some(fx) = loop_fixture() else { return };
    let (why, handoff_why, _) = unreadable(&fx, None);
    // The same clause as for any other output, with nothing after the path.
    assert_eq!(
        why,
        expected_reason(&fx, "1", &kept_clause(&fx), "is absent")
    );
    assert_eq!(handoff_why, format!("{SCHEMA_PREFIX}{why}"));
    assert_eq!(read(&kept(&fx)), "", "kept, and empty");
}

#[test]
fn the_reason_is_printable_ascii_and_the_file_keeps_the_raw_bytes() {
    let Some(fx) = loop_fixture() else { return };
    // An escape sequence and a non-ASCII character in one line: the reason
    // holds neither, and the file holds both.
    let raw = "caf\u{e9} \u{1b}[31mRAWMARK\u{1b}[0m done";
    let (why, handoff_why, out) = unreadable(&fx, Some(&format!("{raw}\n")));
    assert_eq!(
        why,
        expected_reason(&fx, "1", &kept_clause(&fx), "is absent"),
        "{}",
        show(&out)
    );
    for text in [&why, &handoff_why] {
        assert!(text.bytes().all(|b| (0x20..=0x7e).contains(&b)), "{text:?}");
        assert_no_driver_text(text, &["RAWMARK"]);
    }
    // The evidence file is the driver's output, byte for byte.
    assert_eq!(read(&kept(&fx)), format!("{raw}\n"));
}

#[test]
fn a_long_state_path_is_cut_from_the_end_and_never_before_the_file_name() {
    let Some(mut fx) = loop_fixture() else { return };
    // The only text in the reason that can grow is the state folder's path,
    // which it names twice. Two 200-character folder names take the whole past
    // 900 characters, so the cap has something to cut.
    let deep = fx.root.join("d".repeat(200)).join("e".repeat(200));
    private_dir(&deep);
    fx.state = deep;
    let (why, handoff_why, _) = unreadable(&fx, Some("some output\n"));

    let whole = expected_reason(&fx, "1", &kept_clause(&fx), "is absent");
    assert!(whole.len() > 900, "nothing to cut: {}", whole.len());
    // Cut at 900 from the end: the result-file clause goes first, and the
    // bare file name, which comes before any path, is still there.
    assert_eq!(why, &whole[..900], "{why}");
    assert!(why.contains(NAME), "{why}");
    // The prefix step-02a adds still fits inside its own 1000-byte cut.
    assert_eq!(handoff_why, format!("{SCHEMA_PREFIX}{why}"));
    assert!(handoff_why.len() <= 1000, "{}", handoff_why.len());
}

// ===========================================================================
// What the reason says about merge-ready-result.json, and that the file (or
// the partial one) is still in the state folder.
// ===========================================================================

#[test]
fn a_partial_result_file_is_named_as_unreadable_and_stays_in_the_state_folder() {
    let Some(fx) = loop_fixture() else { return };
    let partial = "{\"loop\":\"merge-ready\",\"loop_result\":";
    let (out, classified) = with_extras(
        &fx,
        &[("loop_result_raw", partial)],
        &case("1", None, Some("driver died while writing its result\n")),
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let why = loop_reason(&out);
    assert_eq!(loop_line(&out)["loop_result"], json!("UNREADABLE"));
    assert!(
        why.contains(&format!(
            "{RESULT_NAME} is unreadable at {}",
            result_path(&fx).display()
        )),
        "{why}"
    );
    assert!(why.contains(&kept_clause(&fx)), "{why}");
    assert!(outcome_reason(&classified.unwrap()).contains("is unreadable at"));
    // The partial file the driver left is kept, byte for byte.
    assert_eq!(read(&result_path(&fx)), partial);
}

#[test]
fn a_done_result_after_a_failed_exit_says_what_it_says() {
    let Some(fx) = loop_fixture() else { return };
    let out = loop_step_02(
        &fx,
        &case("1", Some("DONE"), Some("exited 1 after writing DONE\n")),
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(loop_line(&out)["loop_result"], json!("UNREADABLE"));
    let why = loop_reason(&out);
    assert!(
        why.contains(&format!(
            "{RESULT_NAME} says DONE at {}",
            result_path(&fx).display()
        )),
        "{why}"
    );
    assert!(result_path(&fx).is_file(), "the result file is kept");
}

// ===========================================================================
// Each run of the tool starts clean: the kept output and the result file
// describe the same run.
// ===========================================================================

#[test]
fn each_run_replaces_the_previous_runs_output() {
    let Some(fx) = loop_fixture() else { return };
    let out = loop_step_02(&fx, &case("1", Some("STUCK"), Some("first run marker\n")));
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(read(&kept(&fx)), "first run marker\n");
    assert!(result_path(&fx).is_file());

    // A second run whose driver writes nothing leaves nothing of the first:
    // an empty kept file and no result file, never a mixture.
    let (why, _) = unreadable_line(&fx, None);
    assert_eq!(read(&kept(&fx)), "", "{why}");
    assert!(!result_path(&fx).exists());
    assert!(why.contains(&format!("{RESULT_NAME} is absent")), "{why}");
}

// ===========================================================================
// The kept file is never written through a link, and whatever was at its name
// is replaced by a private file.
// ===========================================================================

#[test]
fn a_planted_link_at_the_kept_name_is_replaced_never_followed() {
    let Some(fx) = loop_fixture() else { return };
    let victim = fx.root.join("victim.txt");
    fs::write(&victim, "precious\n").unwrap();
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o644)).unwrap();
    symlink(&victim, kept(&fx)).unwrap();

    let (why, _) = unreadable_line(&fx, Some("the driver's own output\n"));

    assert_eq!(
        read(&victim),
        "precious\n",
        "the link's target is untouched"
    );
    assert_eq!(mode(&victim), 0o644);
    let meta = fs::symlink_metadata(kept(&fx)).unwrap();
    assert!(
        meta.file_type().is_file(),
        "a fresh regular file replaced the link"
    );
    assert_eq!(mode(&kept(&fx)), 0o600);
    assert_eq!(read(&kept(&fx)), "the driver's own output\n");
    assert!(why.contains(&kept_clause(&fx)), "{why}");
}

#[test]
fn a_link_to_a_missing_target_does_not_create_the_target() {
    let Some(fx) = loop_fixture() else { return };
    let target = fx.root.join("not-there-yet.txt");
    symlink(&target, kept(&fx)).unwrap();
    let _ = unreadable_line(&fx, Some("output\n"));
    assert!(!target.exists(), "nothing was written through the link");
    assert!(
        fs::symlink_metadata(kept(&fx))
            .unwrap()
            .file_type()
            .is_file()
    );
}

#[test]
fn an_existing_loose_file_is_replaced_by_a_private_one() {
    let Some(fx) = loop_fixture() else { return };
    fs::write(kept(&fx), "old evidence from an older run\n").unwrap();
    fs::set_permissions(kept(&fx), fs::Permissions::from_mode(0o644)).unwrap();
    let _ = unreadable_line(&fx, Some("new evidence\n"));
    assert_eq!(read(&kept(&fx)), "new evidence\n");
    assert_eq!(mode(&kept(&fx)), 0o600, "the old mode is not kept");
}

// ===========================================================================
// A copy that cannot be made is not an error: the driver still runs, the log
// is complete, the exit status is the driver's, and the reason says so.
// ===========================================================================

/// Restores a folder's mode so the temporary tree can always be removed.
struct Restore(PathBuf);

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
    }
}

/// True when the process can still write into `dir`, as root can into a
/// read-only folder; the cases that rely on a write failing are then skipped,
/// the way `jq_available()` skips the cases that need jq.
fn can_write_into(dir: &Path) -> bool {
    let probe = dir.join(".write-probe");
    let wrote = fs::write(&probe, b"x").is_ok();
    let _ = fs::remove_file(&probe);
    wrote
}

fn assert_not_kept(why: &str, fx: &Fx) {
    assert!(why.contains(NOT_KEPT), "{why}");
    assert!(!why.contains("was kept"), "never claims a file: {why}");
    assert!(!why.contains(NAME), "names no file: {why}");
    // The result-file clause does not depend on the copy.
    assert!(
        why.contains(&format!(
            "{RESULT_NAME} is absent at {}",
            result_path(fx).display()
        )),
        "{why}"
    );
}

#[test]
fn an_unwritable_state_folder_changes_neither_the_exit_nor_the_log() {
    let Some(fx) = loop_fixture() else { return };
    fs::set_permissions(&fx.state, fs::Permissions::from_mode(0o500)).unwrap();
    let _restore = Restore(fx.state.clone());
    if can_write_into(&fx.state) {
        eprintln!("SKIP: this process can write into a read-only folder (running as root)");
        return;
    }
    let driver_line = "the driver's last line, which only the log has\n";

    // A driver that ends without a result: exit 0 and an UNREADABLE line.
    let (out, classified) = loop_step(&fx, &case("1", None, Some(driver_line)));
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(loop_line(&out)["loop_result"], json!("UNREADABLE"));
    let why = loop_reason(&out);
    assert_not_kept(&why, &fx);
    assert!(!why.contains("the driver's last line"), "{why}");
    assert!(outcome_reason(&classified.unwrap()).contains(NOT_KEPT));
    assert!(
        stderr_of(&out).contains(driver_line),
        "the log is complete: {}",
        show(&out)
    );
    assert!(!kept(&fx).exists(), "no file was made");

    // The driver's exit status still passes through, whatever the copy does.
    for (rc, expected) in [("79", 79), ("75", 75), ("3", 3)] {
        let out = loop_step_02(&fx, &case(rc, None, Some(driver_line)));
        assert_eq!(
            out.status.code(),
            Some(expected),
            "driver {rc}: {}",
            show(&out)
        );
        assert!(stderr_of(&out).contains(driver_line), "{}", show(&out));
    }
}

#[test]
fn a_missing_state_folder_is_not_created_and_not_claimed() {
    let Some(fx) = loop_fixture() else { return };
    fs::remove_dir(&fx.state).unwrap();
    let (out, classified) = loop_step(&fx, &case("1", None, Some("output with nowhere to go\n")));
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_not_kept(&loop_reason(&out), &fx);
    assert_eq!(classified.unwrap().status.code(), Some(0));
    assert!(
        !fx.state.exists(),
        "the tool never creates the state folder"
    );
    assert!(stderr_of(&out).contains("output with nowhere to go"));
}

#[test]
fn a_state_folder_that_is_a_link_is_not_written_through() {
    let Some(mut fx) = loop_fixture() else { return };
    let real = fx.root.join("real-state");
    private_dir(&real);
    let link = fx.root.join("linked-state");
    symlink(&real, &link).unwrap();
    fx.state = link;
    let out = loop_step_02(&fx, &case("1", None, Some("output behind a link\n")));
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_not_kept(&loop_reason(&out), &fx);
    assert!(
        !real.join(NAME).exists(),
        "nothing was written through the link"
    );
    assert!(stderr_of(&out).contains("output behind a link"));
}

#[test]
fn a_directory_at_the_kept_name_is_left_alone_and_the_run_goes_on() {
    let Some(fx) = loop_fixture() else { return };
    fs::create_dir(kept(&fx)).unwrap();
    let out = loop_step_02(&fx, &case("1", None, Some("output next to a directory\n")));
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_not_kept(&loop_reason(&out), &fx);
    assert!(kept(&fx).is_dir(), "the directory is still there");
    assert!(stderr_of(&out).contains("output next to a directory"));
}

/// #2849: a kept name that cannot be opened at all still changes nothing.
/// The descriptor is opened once, before the driver starts, and falls back to
/// `/dev/null`, so the writer always runs. The failure this pins is what
/// happens when it does not: a writer that is skipped leaves the driver
/// writing into a pipe with no reader, so it dies of SIGPIPE on its first
/// line and the step exits 141 with the driver's own status and its whole
/// output lost. A directory at the kept name is the one unopenable state a
/// test can hold with the state folder still ours and writable, because
/// `rm -f` cannot clear it; it exercises the `/dev/null` fallback, not the
/// open itself, which cannot fail separately now that the create and the
/// append are one operation.
#[test]
fn an_unopenable_kept_name_never_takes_the_drivers_exit_or_its_output() {
    let Some(fx) = loop_fixture() else { return };
    fs::create_dir(kept(&fx)).unwrap();
    let out = loop_step_02(&fx, &case("7", None, Some("first line\nlast line\n")));
    assert_eq!(
        out.status.code(),
        Some(7),
        "the driver's own exit passes through, never 141: {}",
        show(&out)
    );
    let err = stderr_of(&out);
    for line in ["first line", "last line"] {
        assert!(err.contains(line), "{line} reached the log: {}", show(&out));
    }
    assert!(err.contains(NOT_KEPT_NOTICE), "{}", show(&out));
}

/// #2849: `POSIXLY_CORRECT` in the environment makes bash enter POSIX mode at
/// startup, and a redirection error on `exec` ends a POSIX-mode shell outright
/// rather than returning 1. Without the guard around the open, a kept name
/// that cannot be opened would end the step there, silently and before the
/// driver ran: the copy deciding the run again, by another route. The mode is
/// turned off for the open and restored after it.
#[test]
fn posix_mode_does_not_let_an_unopenable_kept_name_end_the_step() {
    let Some(fx) = loop_fixture() else { return };
    fs::create_dir(kept(&fx)).unwrap();
    stage_loop_driver(&fx, &case("7", None, Some("first line\nlast line\n")));
    let mut c = fx.bash();
    c.arg("-c")
        .arg(&loop_commands().0)
        .env("POSIXLY_CORRECT", "1");
    loop_env(&mut c, &fx, &loop_preflight(&fx));
    let out = c.output().expect("run merge loop step-02 in POSIX mode");
    assert_eq!(
        out.status.code(),
        Some(7),
        "the driver ran and its own exit passed through: {}",
        show(&out)
    );
    let err = stderr_of(&out);
    for line in ["first line", "last line"] {
        assert!(err.contains(line), "{line} reached the log: {}", show(&out));
    }
    assert!(err.contains(NOT_KEPT_NOTICE), "{}", show(&out));
}

/// #2849: the copy's descriptor belongs to the step, not to the driver. The
/// pipeline closes it on the driver's side, so neither the driver nor an
/// agent it starts can write into, truncate or read the evidence file.
#[test]
fn the_driver_does_not_inherit_the_copys_descriptor() {
    let Some(fx) = loop_fixture() else { return };
    if !PathBuf::from("/proc/self/fd").is_dir() {
        eprintln!("SKIP: this host has no /proc/self/fd");
        return;
    }
    let out = loop_step_02(&fx, &case("0", Some("DONE"), None));
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert!(kept(&fx).is_file(), "the copy was made: {}", show(&out));
    let seen = fs::read_to_string(fx.fake.join("loop-fd4")).expect("the driver reported its fds");
    assert_eq!(
        seen.trim(),
        "closed",
        "the driver inherited the copy's descriptor: {}",
        show(&out)
    );
}

/// #2849: the umask of 077 that opens the copy is restored before the driver
/// starts. The driver runs under the umask the step was started with, so a file
/// it creates is not made private by the copy's open.
#[test]
fn the_driver_runs_under_the_steps_own_umask_not_the_copys() {
    let Some(fx) = loop_fixture() else { return };
    stage_loop_driver(&fx, &case("0", Some("DONE"), None));
    let mut c = fx.bash();
    c.arg("-c")
        .arg("umask 022; exec bash -c \"$STEP\"")
        .env("STEP", &loop_commands().0);
    loop_env(&mut c, &fx, &loop_preflight(&fx));
    let out = c.output().expect("run merge loop step-02 under umask 022");
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(
        read(&fx.fake.join("loop-umask")).trim(),
        "0022",
        "the driver's umask: {}",
        show(&out)
    );
    assert_eq!(mode(&kept(&fx)), 0o600, "{}", show(&out));
}

/// Runs `step-02` under a 2 KiB limit on file size, with the signal that limit
/// raises ignored, so a write past it fails the way a full disk does. The
/// stub driver ends with `case`. `None` when this shell cannot set the limit.
fn step02_under_file_limit(fx: &Fx, case: &LoopCase) -> Option<Output> {
    stage_loop_driver(fx, case);
    let mut c = fx.bash();
    c.arg("-c")
        .arg("ulimit -f 2 || exit 77; trap '' XFSZ; exec bash -c \"$STEP\"")
        .env("STEP", &loop_commands().0);
    loop_env(&mut c, fx, &loop_preflight(fx));
    let out = c.output().expect("run step-02 under a file size limit");
    if out.status.code() == Some(77) {
        eprintln!("SKIP: this shell cannot limit file size");
        return None;
    }
    Some(out)
}

#[test]
fn a_write_failure_part_way_is_reported_as_a_partial_copy() {
    let Some(fx) = loop_fixture() else { return };
    let filler: String = (0..400)
        .map(|i| format!("filler line number {i:04}\n"))
        .collect();
    let driver_output = format!("{filler}FINAL-MARKER-LINE the driver's very last line\n");
    let Some(out) = step02_under_file_limit(&fx, &case("1", None, Some(&driver_output))) else {
        return;
    };

    // The driver's exit still decides the step.
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(loop_line(&out)["loop_result"], json!("UNREADABLE"));
    // Standard error stays complete.
    assert!(
        stderr_of(&out).contains("FINAL-MARKER-LINE the driver's very last line\n"),
        "the log lost the driver's last line"
    );
    // The kept file stopped at the limit: it holds the start, not the end.
    let kept_text = read(&kept(&fx));
    assert!(
        !kept_text.is_empty() && kept_text.len() <= 2048,
        "{}",
        kept_text.len()
    );
    assert!(kept_text.contains("filler line number 0000"));
    assert!(!kept_text.contains("FINAL-MARKER-LINE"));
    // The reason says only part of the output was kept, because the file
    // stops short of the driver's last line and a whole-copy claim would be
    // one the run cannot make. The name and the absolute path stay, because
    // the truncated file is still the evidence to open. Nothing follows the
    // path but the next clause.
    let why = loop_reason(&out);
    assert_eq!(
        why,
        expected_reason(&fx, "1", &partial_clause(&fx), "is absent"),
        "{}",
        show(&out)
    );
    assert!(!why.contains("was kept"), "no claim of a whole copy: {why}");
    // The log says it too, after the driver's own output.
    assert!(stderr_of(&out).contains(PARTIAL_NOTICE), "{}", show(&out));
}

#[test]
fn a_copy_that_was_not_made_is_said_in_the_log_on_every_ending() {
    let Some(mut fx) = loop_fixture() else { return };
    // A state folder reached through a link: the tool refuses to write the
    // copy through it, while the driver still writes its result file there,
    // so the run ends `DONE`. Only an `UNREADABLE` reason used to carry the
    // loss, so a `DONE`, `STUCK` or `HANDOFF` run lost the copy in silence.
    let real = fx.root.join("real-state-for-notice");
    private_dir(&real);
    let link = fx.root.join("linked-state-for-notice");
    symlink(&real, &link).unwrap();
    fx.state = link;

    let out = loop_step_02(&fx, &case("0", Some("DONE"), Some("all good\n")));
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(loop_line(&out)["loop_result"], json!("DONE"));
    assert!(
        stderr_of(&out).contains(NOT_KEPT_NOTICE),
        "the log says the copy was not made: {}",
        show(&out)
    );
    // Still not a check: the loop line is the one it always was, the notice
    // is not on standard output, and nothing was written through the link.
    assert!(!stdout(&out).contains("NOTICE"), "{}", show(&out));
    assert!(!stdout(&out).contains("kept"), "{}", show(&out));
    assert!(
        !real.join(NAME).exists(),
        "nothing written through the link"
    );
}

#[test]
fn a_failing_copy_never_changes_the_drivers_exit_or_its_loop_line() {
    let Some(fx) = loop_fixture() else { return };
    // The copy's writes fail here, under a file-size limit, so the writer ends
    // non-zero after the driver has finished. The step's `pipefail` would hand
    // that 1 to a plain `$?`: a finished DONE would read as UNREADABLE, and a
    // 79, 75 or 3 would be lost. The status read is the driver's own.
    let overflow: String = (0..400)
        .map(|i| format!("filler line number {i:04}\n"))
        .collect();
    for (rc, result, expected_exit, expected_result) in [
        ("0", Some("DONE"), 0, Some("DONE")),
        ("0", Some("WAITING"), 0, Some("WAITING")),
        ("1", Some("STUCK"), 0, Some("STUCK")),
        ("1", Some("USAGE_LIMIT"), 1, Some("USAGE_LIMIT")),
        ("75", Some("ORPHANED"), 75, Some("ORPHANED")),
        ("79", None, 79, Some("TERMINAL_REFUSAL")),
        ("3", None, 3, None),
        ("2", None, 2, None),
    ] {
        let Some(out) = step02_under_file_limit(&fx, &case(rc, result, Some(&overflow))) else {
            return;
        };
        assert_eq!(
            out.status.code(),
            Some(expected_exit),
            "driver {rc}/{result:?}: {}",
            show(&out)
        );
        if let Some(r) = expected_result {
            assert_eq!(
                loop_line(&out)["loop_result"],
                json!(r),
                "driver {rc}/{result:?}: {}",
                show(&out)
            );
        }
    }
}
