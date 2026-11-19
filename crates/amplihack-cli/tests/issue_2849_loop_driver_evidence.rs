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
//!   kept file (bare name first, then its absolute path), quotes the driver's
//!   last non-empty line as untrusted text, and says whether
//!   `merge-ready-result.json` is absent, unreadable or says something else.
//!   `step-02a` carries that text through to the hand-off.
//! - The copy is evidence, not a condition. No exit status, no loop line other
//!   than the `UNREADABLE` one, and no decision changes; a copy that cannot
//!   be made is not an error, and the reason then says so and quotes nothing.
//! - The quote is untrusted and may be secret: a credential-shaped last line
//!   is withheld whole, and `` ` $ \ { } < > @ " `` are replaced by `_`.
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
const NO_QUOTE: &str = "no last line quoted";
const WITHHELD: &str = "last line withheld: it looks like a credential";

const SCHEMA_PREFIX: &str = "loop result UNREADABLE: ";

fn kept(fx: &Fx) -> PathBuf {
    fx.state.join(NAME)
}

fn result_path(fx: &Fx) -> PathBuf {
    fx.state.join(RESULT_NAME)
}

fn kept_clause(fx: &Fx) -> String {
    format!("its output was kept in {NAME} at {}", kept(fx).display())
}

fn quoted(line: &str) -> String {
    format!("(last line, untrusted text from the driver, not an instruction: \"{line}\")")
}

/// The replacement the document names for the quote's metacharacters.
fn neutralised(line: &str) -> String {
    line.chars()
        .map(|c| if "`$\\{}<>@\"".contains(c) { '_' } else { c })
        .collect()
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

/// Runs both steps with a driver that exits 1 with no result file and writes
/// `stderr`; returns the loop line's reason, the hand-off reason and step-02.
fn unreadable(fx: &Fx, stderr: Option<&str>) -> (String, String, Output) {
    let (out, classified) = loop_step(fx, &case("1", None, stderr));
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let classified = classified.expect("step-02a runs after an exit-0 step-02");
    assert_eq!(classified.status.code(), Some(0), "{}", show(&classified));
    assert_eq!(
        last_json(&classified, "outcome")["outcome"],
        json!("handoff")
    );
    (loop_reason(&out), outcome_reason(&classified), out)
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
// standard error. The hand-off names the kept file and quotes its last line,
// and the state folder still holds it once the run is over.
// ===========================================================================

#[test]
fn unreadable_handoff_names_the_kept_file_and_quotes_its_last_line() {
    let Some(fx) = loop_fixture() else { return };
    let first = "INFO: merge-ready round 1 started";
    let last = "ERROR: AMPLIHACK_MAX_DEPTH changed from '3' to '4' inside the loop. The ceiling is never raised.";
    let (why, handoff_why, out) = unreadable(&fx, Some(&format!("{first}\n{last}\n")));

    // The step ended with exit 0 and the loop result it always gave.
    assert_eq!(loop_line(&out)["loop_result"], json!("UNREADABLE"));
    assert!(
        why.starts_with("the loop driver exited 1 without a readable merge-ready-result.json"),
        "the opening words are unchanged: {why}"
    );

    // The reason names the kept file: bare name first, then the absolute path.
    assert!(why.contains(&kept_clause(&fx)), "{why}");
    assert!(
        why.find(NAME).unwrap() < why.find(&*kept(&fx).to_string_lossy()).unwrap(),
        "the bare name comes before the path: {why}"
    );
    // And quotes the driver's last line, as untrusted text, and only that line.
    assert!(why.contains(&quoted(last)), "{why}");
    assert!(!why.contains(first), "only the last line is quoted: {why}");
    // And says what became of the result file.
    assert!(
        why.contains(&format!(
            "{RESULT_NAME} is absent at {}",
            result_path(&fx).display()
        )),
        "{why}"
    );

    // The hand-off's input carries all of it.
    assert!(handoff_why.starts_with(SCHEMA_PREFIX), "{handoff_why}");
    assert!(handoff_why.contains(&kept_clause(&fx)), "{handoff_why}");
    assert!(handoff_why.contains(&quoted(last)), "{handoff_why}");
    assert!(handoff_why.len() <= 1000, "{}", handoff_why.len());

    // The state folder still holds the evidence once both steps have ended.
    let file = kept(&fx);
    let meta = fs::symlink_metadata(&file).expect("the kept file exists after the run");
    assert!(meta.file_type().is_file(), "a regular file, not a link");
    assert_eq!(mode(&file), 0o600, "private, like every state file");
    assert_eq!(read(&file), format!("{first}\n{last}\n"));
}

#[test]
fn what_the_driver_prints_reaches_the_log_unchanged_and_in_order() {
    let Some(fx) = loop_fixture() else { return };
    let (out, _) = loop_step(
        &fx,
        &LoopCase {
            rc: "1",
            result: Some("STUCK"),
            printed: Some("driver says hello\n".into()),
            stderr: Some("driver says goodbye\n".into()),
        },
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    // Standard output and standard error share one stream, in the order the
    // driver wrote them.
    assert_eq!(read(&kept(&fx)), "driver says hello\ndriver says goodbye\n");
    // The step's standard error has the same bytes, so the run log is unchanged.
    assert!(
        stderr_of(&out).contains("driver says hello\ndriver says goodbye\n"),
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

// ===========================================================================
// Nothing else changes: every exit status, and every loop line that is not
// the UNREADABLE one, is what it was before the output was kept.
// ===========================================================================

#[test]
fn every_driver_ending_keeps_its_output_and_its_exit_status() {
    let Some(fx) = loop_fixture() else { return };
    // The exit status is the driver's, with the copy in the pipeline. (A copy
    // that itself fails is the next test's case; with a copy that works, the
    // step's `pipefail` makes the pipeline's status the driver's anyway.)
    for (n, (rc, result, expected)) in [
        ("79", None, 79),
        ("79", Some("DONE"), 79),
        ("1", Some("USAGE_LIMIT"), 1),
        ("75", Some("ORPHANED"), 75),
        ("2", None, 2),
        ("3", None, 3),
        ("0", Some("DONE"), 0),
        ("0", Some("WAITING"), 0),
        ("0", None, 0),
        ("1", Some("STUCK"), 0),
        ("1", Some("BUDGET_EXHAUSTED"), 0),
        ("1", Some("HANDOFF"), 0),
        ("1", None, 0),
    ]
    .into_iter()
    .enumerate()
    {
        let marker = format!("marker-{n}-for-driver-exit-{rc}\n");
        let (out, _) = loop_step(&fx, &case(rc, result, Some(&marker)));
        assert_eq!(
            out.status.code(),
            Some(expected),
            "driver {rc}/{result:?}: {}",
            show(&out)
        );
        assert_eq!(
            fs::read_to_string(kept(&fx)).ok().as_deref(),
            Some(marker.as_str()),
            "driver {rc}/{result:?} keeps its output, and only this run's"
        );
    }
}

#[test]
fn loop_lines_other_than_unreadable_are_byte_identical() {
    let Some(fx) = loop_fixture() else { return };
    let stub = |r: &str| {
        format!(
            r#"{{"loop":"merge-ready","loop_result":"{r}","round_label":"round-2","reason":"fixture stop"}}"#
        )
    };
    let noise = Some("a last line that must not leak into any other loop line\n");
    for (rc, result, expected) in [
        ("0", Some("DONE"), stub("DONE")),
        ("0", Some("WAITING"), stub("WAITING")),
        ("1", Some("STUCK"), stub("STUCK")),
        ("1", Some("BUDGET_EXHAUSTED"), stub("BUDGET_EXHAUSTED")),
        ("1", Some("HANDOFF"), stub("HANDOFF")),
        ("1", Some("USAGE_LIMIT"), stub("USAGE_LIMIT")),
        ("75", Some("ORPHANED"), stub("ORPHANED")),
        (
            "79",
            None,
            r#"{"loop":"merge-ready","loop_result":"TERMINAL_REFUSAL","round_label":"unknown","reason":"the merge-ready loop exited 79"}"#
                .to_string(),
        ),
    ] {
        let (out, _) = loop_step(&fx, &case(rc, result, noise));
        assert_eq!(
            stdout(&out).trim_end(),
            expected,
            "driver {rc}/{result:?}: {}",
            show(&out)
        );
    }
}

// ===========================================================================
// What is quoted: the last non-empty line, one line, filtered.
// ===========================================================================

#[test]
fn the_last_non_empty_line_is_the_one_quoted() {
    let Some(fx) = loop_fixture() else { return };
    let earlier_secret = concat!("GH_TOKEN=", "abcdefghij1234567890");
    let (why, handoff_why, _) = unreadable(
        &fx,
        Some(&format!("{earlier_secret}\nsecond line\n\n   \n\t\n")),
    );
    assert!(why.contains(&quoted("second line")), "{why}");
    // An earlier line is not quoted, so an earlier credential never reaches
    // the reason, and the last line is not withheld for what came before it.
    for text in [&why, &handoff_why] {
        assert!(!text.contains("abcdefghij1234567890"), "{text}");
        assert!(!text.contains("withheld"), "{text}");
    }
}

#[test]
fn a_last_line_with_no_newline_is_quoted() {
    let Some(fx) = loop_fixture() else { return };
    let (why, _, _) = unreadable(&fx, Some("one\npartial line, cut off mid-wr"));
    assert!(
        why.contains(&quoted("partial line, cut off mid-wr")),
        "{why}"
    );
}

#[test]
fn ordinary_error_text_is_quoted_not_withheld() {
    let Some(fx) = loop_fixture() else { return };
    for line in [
        "error: failed to push some refs to origin",
        "ERROR: round 3 did not finish (exit 124)",
        "gh: not logged in; run gh auth login",
    ] {
        let (why, _, _) = unreadable(&fx, Some(&format!("{line}\n")));
        assert!(why.contains(&quoted(line)), "{why}");
        assert!(!why.contains("withheld"), "{why}");
    }
}

#[test]
fn a_driver_that_wrote_nothing_still_names_its_empty_file() {
    let Some(fx) = loop_fixture() else { return };
    let (why, handoff_why, _) = unreadable(&fx, None);
    for text in [&why, &handoff_why] {
        assert!(text.contains(&kept_clause(&fx)), "{text}");
        assert!(text.contains(NO_QUOTE), "nothing to quote: {text}");
        assert!(!text.contains("untrusted text"), "no quote slot: {text}");
    }
    assert_eq!(read(&kept(&fx)), "", "kept, and empty");
}

#[test]
fn the_quote_is_printable_ascii_and_the_file_keeps_the_raw_bytes() {
    let Some(fx) = loop_fixture() else { return };
    let raw = "caf\u{e9} \u{1b}[31mred\u{1b}[0m done";
    let (why, handoff_why, _) = unreadable(&fx, Some(&format!("{raw}\n")));
    // The escape byte and the non-ASCII character are dropped from the
    // quote (the same filter the hand-off reason always had) ...
    assert!(why.contains(&quoted("caf [31mred[0m done")), "{why}");
    for text in [&why, &handoff_why] {
        assert!(text.bytes().all(|b| (0x20..=0x7e).contains(&b)), "{text:?}");
    }
    // ... and the evidence file is the driver's output, unfiltered.
    assert_eq!(read(&kept(&fx)), format!("{raw}\n"));
}

#[test]
fn quote_metacharacters_are_neutralised_and_the_reason_stays_valid_json() {
    let Some(fx) = loop_fixture() else { return };
    let line = "run `id` $(id) ${HOME} {{secret}} <b>hi</b> @octocat say \"yes\" a\\b";
    let (why, handoff_why, out) = unreadable(&fx, Some(&format!("{line}\n")));
    let expected = neutralised(line);
    assert_eq!(
        expected,
        "run _id_ _(id) __HOME_ __secret__ _b_hi_/b_ _octocat say _yes_ a_b"
    );
    for text in [&why, &handoff_why] {
        assert!(text.contains(&quoted(&expected)), "{text}");
        // Nothing that could close the quote, expand, open a template or a
        // tag, or notify a user is left in the reason.
        for c in ['`', '$', '\\', '{', '}', '<', '>', '@'] {
            assert!(!text.contains(c), "{c:?} left in: {text}");
        }
        assert_eq!(
            text.matches('"').count(),
            2,
            "only the quote's own marks: {text}"
        );
    }
    // The loop line parsed as JSON above (last_json); it is one line.
    assert_eq!(stdout(&out).trim_end().lines().count(), 1, "{}", show(&out));
    // The kept file keeps the line as the driver wrote it.
    assert_eq!(read(&kept(&fx)), format!("{line}\n"));
}

#[test]
fn the_quote_is_capped_at_200_characters_and_the_whole_reason_at_900() {
    let Some(fx) = loop_fixture() else { return };
    let long = "x".repeat(1000);
    let (why, handoff_why, _) = unreadable(&fx, Some(&format!("{long}\n")));
    assert!(why.contains(&"x".repeat(200)), "{why}");
    assert!(!why.contains(&"x".repeat(201)), "{why}");
    assert!(why.len() <= 900, "{}", why.len());
    // The prefix step-02a adds still fits inside its own 1000-byte cut, and the
    // file name is still there.
    assert!(handoff_why.len() <= 1000, "{}", handoff_why.len());
    assert!(handoff_why.contains(NAME), "{handoff_why}");
    assert_eq!(read(&kept(&fx)), format!("{long}\n"), "evidence is not cut");
}

#[test]
fn a_huge_unterminated_line_is_bounded() {
    let Some(fx) = loop_fixture() else { return };
    let huge = "y".repeat(300_000);
    let (why, handoff_why, _) = unreadable(&fx, Some(&huge));
    assert!(why.contains(&"y".repeat(200)), "{why}");
    assert!(!why.contains(&"y".repeat(201)), "{why}");
    assert!(why.len() <= 900 && handoff_why.len() <= 1000);
    assert!(why.contains(&kept_clause(&fx)), "{why}");
    assert_eq!(fs::metadata(kept(&fx)).unwrap().len(), 300_000);
}

// ===========================================================================
// A credential-shaped last line is withheld whole. It stays in the kept file
// and in the log: only the quote in the reason is withheld.
// ===========================================================================

/// The shapes the document lists. Each is built at run time so that no
/// scanner sees a credential in this file. `(line, material)`: the material
/// is the part that must never reach the reason.
fn credential_lines() -> Vec<(String, String)> {
    let mut shapes: Vec<(String, String)> = Vec::new();
    let mut add = |line: String, material: &str| shapes.push((line, material.to_string()));
    let ghp = format!("{}{}", "ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4");
    add(format!("remote: auth with {ghp} rejected"), &ghp);
    let pat = format!("{}{}", "github_pat_", "11ABCDEFG0abcdefghijkl_MNOPQRSTUV");
    add(format!("remote: auth with {pat} rejected"), &pat);
    let sk = format!("{}{}", "sk-", "ant0api0abcdefghijklmnop12");
    add(format!("401 from the model API using {sk}"), &sk);
    let akia = format!("{}{}", "AKIA", "IOSFODNN7EXAMPLE");
    add(format!("aws: credentials {akia} are not valid"), &akia);
    let asia = format!("{}{}", "ASIA", "IOSFODNN7EXAMPLE");
    add(format!("aws: credentials {asia} are not valid"), &asia);
    let slack = format!("{}{}", "xoxb-", "1234567890-abcdefghij");
    add(format!("slack: {slack} was revoked"), &slack);
    let glpat = format!("{}{}", "glpat-", "abcdefghij0123456789");
    add(format!("gitlab: {glpat} was revoked"), &glpat);
    let npm = format!("{}{}", "npm_", "abcdefghijklmnopqrstuvwxyz0123456789");
    add(format!("npm ERR! auth {npm} rejected"), &npm);
    let aiza = format!("{}{}", "AIza", "SyAbcdefghijklmnopqrstuvwxyz012345678");
    add(format!("google: key {aiza} rejected"), &aiza);
    let jwt = format!(
        "eyJ{}.eyJ{}.{}",
        "hbGciOiJIUzI1NiJ9", "zdWIiOiIxMjM0NTY3ODkwIn0", "c2lnbmF0dXJlMTIzNDU2"
    );
    add(format!("session {jwt} expired"), &jwt);
    add(
        format!(
            "fatal: unable to access https://deploy:{}@git.example.com/r.git",
            "hunter2pass"
        ),
        "hunter2pass",
    );
    let pem = format!("{}{}", "-----BEGIN ", "PRIVATE KEY-----");
    add(format!("remote: {pem} rejected"), &pem);
    let rsa = format!("{}{}", "-----BEGIN RSA ", "PRIVATE KEY-----");
    add(format!("remote: {rsa} rejected"), &rsa);
    add(
        format!("curl -H Authorization: Bearer {}", "abc123def456ghi789"),
        "abc123def456ghi789",
    );
    add(
        format!("+ export GH_TOKEN={}", "notarealvalue123"),
        "notarealvalue123",
    );
    add(
        format!("client_secret: {}", "s3cr3tvalue99"),
        "s3cr3tvalue99",
    );
    add(
        format!("db password={}", "hunter2hunter2"),
        "hunter2hunter2",
    );
    add(
        format!("settings api_key: {}", "abcd1234efgh5678"),
        "abcd1234efgh5678",
    );
    shapes
}

#[test]
fn a_credential_shaped_last_line_is_withheld_from_both_reasons() {
    let Some(fx) = loop_fixture() else { return };
    for (line, material) in credential_lines() {
        let (out, classified) =
            loop_step(&fx, &case("1", None, Some(&format!("ok so far\n{line}\n"))));
        assert_eq!(out.status.code(), Some(0), "{}", show(&out));
        let why = loop_reason(&out);
        let handoff_why = outcome_reason(&classified.unwrap());
        for text in [&why, &handoff_why] {
            assert!(text.contains(WITHHELD), "shape {material:?}: {text}");
            // The file is still named: the evidence is there for a person.
            assert!(
                text.contains(&kept_clause(&fx)),
                "shape {material:?}: {text}"
            );
            assert!(
                !text.contains(&material),
                "shape {material:?} reached: {text}"
            );
            assert!(!text.contains(&line), "shape {material:?}: {text}");
            assert!(!text.contains("untrusted text"), "no quote slot: {text}");
        }
        // No output of the step other than the log carries it.
        assert!(!stdout(&out).contains(&material), "{}", stdout(&out));
        // The kept file is raw evidence for a person, so it still holds the line,
        // and so does the log.
        assert!(read(&kept(&fx)).contains(&line), "shape {material:?}");
        assert!(stderr_of(&out).contains(&line), "shape {material:?}");
    }
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
    let (out, _) = loop_step(
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
    let (out, _) = loop_step(&fx, &case("1", Some("STUCK"), Some("first run marker\n")));
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(read(&kept(&fx)), "first run marker\n");
    assert!(result_path(&fx).is_file());

    // A second run whose driver writes nothing leaves nothing of the first:
    // an empty kept file and no result file, never a mixture.
    let (why, _, _) = unreadable(&fx, None);
    assert_eq!(read(&kept(&fx)), "", "{why}");
    assert!(!result_path(&fx).exists());
    assert!(why.contains(&format!("{RESULT_NAME} is absent")), "{why}");
}

// ===========================================================================
// The kept file is never written through a link, and a link swapped in later
// is never read back.
// ===========================================================================

#[test]
fn a_planted_link_at_the_kept_name_is_replaced_never_followed() {
    let Some(fx) = loop_fixture() else { return };
    let victim = fx.root.join("victim.txt");
    fs::write(&victim, "precious\n").unwrap();
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o644)).unwrap();
    symlink(&victim, kept(&fx)).unwrap();

    let (why, _, _) = unreadable(&fx, Some("the driver's own output\n"));

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
    let _ = unreadable(&fx, Some("output\n"));
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
    let _ = unreadable(&fx, Some("new evidence\n"));
    assert_eq!(read(&kept(&fx)), "new evidence\n");
    assert_eq!(mode(&kept(&fx)), 0o600, "the old mode is not kept");
}

#[test]
fn a_link_swapped_in_after_the_driver_wrote_is_not_read_back() {
    let Some(fx) = loop_fixture() else { return };
    // While the driver runs, something replaces the kept file's name with a
    // link to a file outside the state folder.
    let canary = fx.root.join("outside.txt");
    fs::write(&canary, "CANARY-OUTSIDE-LINE not written by the driver\n").unwrap();
    let (out, classified) = with_extras(
        &fx,
        &[("loop_swap_to", &canary.to_string_lossy())],
        &case("1", None, Some("the driver's real last line\n")),
    );
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let why = loop_reason(&out);
    let handoff_why = outcome_reason(&classified.unwrap());
    for text in [&why, &handoff_why] {
        assert!(
            !text.contains("CANARY-OUTSIDE-LINE"),
            "read through the link: {text}"
        );
        assert!(text.contains(NAME), "{text}");
    }
    assert!(
        !stdout(&out).contains("CANARY-OUTSIDE-LINE"),
        "{}",
        show(&out)
    );
    assert_eq!(
        read(&canary),
        "CANARY-OUTSIDE-LINE not written by the driver\n"
    );
}

// ===========================================================================
// A copy that cannot be made is not an error: the driver still runs, the log
// is complete, the exit status is the driver's, and the reason says so and
// quotes nothing.
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
    assert!(!why.contains("untrusted text"), "quotes nothing: {why}");
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
        let (out, _) = loop_step(&fx, &case(rc, None, Some(driver_line)));
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
    let (out, _) = loop_step(&fx, &case("1", None, Some("output behind a link\n")));
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
    let (out, _) = loop_step(&fx, &case("1", None, Some("output next to a directory\n")));
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_not_kept(&loop_reason(&out), &fx);
    assert!(kept(&fx).is_dir(), "the directory is still there");
    assert!(stderr_of(&out).contains("output next to a directory"));
}

/// Runs `step-02` under a 2 KiB limit on file size, with the signal that limit
/// raises ignored, so a write past it fails the way a full disk does. The
/// stub driver ends with `case`. `None` when this shell cannot set the limit.
fn step02_under_file_limit(fx: &Fx, case: &LoopCase) -> Option<Output> {
    write_executable(
        &fx.tools().join("autodrive_loop.sh"),
        FAKE_LOOP_DRIVER,
        0o755,
    );
    for f in [
        "loop_stdout",
        "loop_stderr",
        "loop_result",
        "loop_result_raw",
        "loop_swap_to",
    ] {
        let _ = fs::remove_file(fx.fake.join(f));
    }
    let _ = fs::remove_file(result_path(fx));
    fx.set("loop_rc", case.rc);
    if let Some(r) = case.result {
        fx.set("loop_result", r);
    }
    if let Some(e) = &case.stderr {
        fx.set("loop_stderr", e);
    }
    let preflight = json!({
        "should_run": "true", "pr": PR, "pr_url": "", "pr_state": "OPEN",
        "branch": "feat/issue-2575", "head_sha": SHA_A, "base_branch": "main",
        "base_sha": SHA_B, "state_dir": fx.state, "repo": fx.repo,
        "reason": "fixture", "blockers": [], "attempts": []
    })
    .to_string();
    let mut c = fx.bash();
    c.arg("-c")
        .arg("ulimit -f 2 || exit 77; trap '' XFSZ; exec bash -c \"$STEP\"")
        .env("STEP", command_of("autodrive-merge-loop", "step-02"))
        .env("RECIPE_VAR_merge_loop_preflight", &preflight)
        .env("RECIPE_VAR_task_description", "issue 2849 fixture task")
        .env("RECIPE_VAR_autodrive_state_dir", &fx.state)
        .env("RECIPE_VAR_pr_number", PR)
        .env("RECIPE_VAR_no_merge", "false")
        .env("RECIPE_VAR_autodrive_dry_run", "false")
        .env("REPO_PATH", &fx.repo)
        .current_dir(&fx.repo)
        .stdin(Stdio::null());
    let out = c.output().expect("run step-02 under a file size limit");
    if out.status.code() == Some(77) {
        eprintln!("SKIP: this shell cannot limit file size");
        return None;
    }
    Some(out)
}

#[test]
fn a_write_failure_part_way_leaves_a_truncated_file_and_a_complete_log() {
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
    // The reason quotes the last line that was written, which is not the
    // driver's last line, and it still names the file.
    let why = loop_reason(&out);
    assert!(why.contains(&kept_clause(&fx)), "{why}");
    assert!(!why.contains("FINAL-MARKER-LINE"), "{why}");
    assert!(
        why.contains(&quoted("")[..40]),
        "something was quoted: {why}"
    );
}

#[test]
fn a_failing_copy_never_changes_the_drivers_exit_or_its_loop_line() {
    let Some(fx) = loop_fixture() else { return };
    // `tee` exits 1 here, after the driver has finished. The step's `pipefail`
    // would hand that 1 to a plain `$?`: a finished DONE would read as
    // UNREADABLE, and a 79, 75 or 3 would be lost. The status is the driver's.
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
