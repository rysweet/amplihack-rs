//! Issue #2333, acceptance A2: the completion dispatch reads the PR head
//! without GraphQL, and reports an unavailable read apart from a differing
//! head.
//!
//! Contract: docs/reference/autodrive-crusty-completion.md#head-check-outcomes
//! and #the-grammar-of-the-two-fields.
//!
//! The three outcomes are:
//!
//! - **Matches** — a route answered with the reviewed head. Objective
//!   completion as before, `evaluator_invoked` `false`, plus the additive
//!   `head_read` field `matches`.
//! - **Differs** — a route answered with another SHA. A dispatch error
//!   naming both commits; the word "unavailable" never appears in it.
//! - **Unavailable** — no route answered. The loop-health evaluator runs
//!   with `loop_head_read` in its context, so `evaluator_invoked` is `true`.
//!   Never `INVALID_DECISION`, so the merge loop is never stopped with
//!   `crusty-rereview-not-done`. This is the round-14 case of 12 Nov 2026,
//!   where the head matched and only the read failed.
//!
//! Built on the #2336 `Drive` fixture. `gh` answers every GraphQL-backed read
//! with GitHub's secondary-limit message, so a read that reached GraphQL
//! fails the test by failing the dispatch. The fake `git` answers
//! `ls-remote` and execs the real `git` for everything else.

use std::ffi::OsStr;
use std::process::Output;

use serde_json::{Value, json};

use crate::support::auto_drive_light::{private_file, show, stderr, stdout, tools_dir};

#[path = "support/autodrive_completion.rs"]
#[allow(dead_code, clippy::duplicate_mod)] // also loaded by issue_2336_autodrive_completion
mod fixture;
use fixture::{Drive, json_output};

/// Another valid head, for the `Differs` outcome.
const OTHER_SHA: &str = "b5864181b5864181b5864181b5864181b5864181";

/// The closed token set of docs/reference/autodrive-crusty-completion.md.
/// Both fields carry program-generated tokens and nothing else; no exit
/// status, `gh` stderr, branch name, check name or URL may appear.
const TOKENS: &[&str] = &[
    "read-failed",
    "origin-not-github",
    "pr-number-unresolved",
    "ambiguous-ref",
    "bad-sha",
];

struct Case {
    d: Drive,
    p: Value,
}

impl Case {
    /// A complete-able round (CLEAN, recorded at the current head), prepared
    /// with `--pr 7`, a `complete` decision written, `origin` set to the
    /// canonical GitHub URL, and the fake `git` installed.
    fn new() -> Self {
        Self::with_pr(Some("7"))
    }

    fn with_pr(pr: Option<&str>) -> Self {
        let d = Drive::new();
        d.set("review.json", r#"{"crusty_verdict":"CLEAN","concerns":[]}"#);
        d.record(1);
        let p = prepare(&d, 1, pr);
        d.decide(&p, "complete");
        d.git(
            &d.repo,
            &["remote", "set-url", "origin", "https://github.com/o/r"],
        );
        d.fakes.with_fake_git();
        Self { d, p }
    }

    fn dispatch(&self, env: &[(&str, &str)]) -> Output {
        let mut c = self.d.cmd(crate::support::amplihack_bin::amplihack_bin());
        c.args([
            "orch",
            "helper",
            "autodrive-completion-dispatch",
            "--private-state",
        ])
        .arg(self.p["private_state"].as_str().unwrap())
        .args(["--child-exit-code", "0"])
        // Every GraphQL-backed read is unavailable, as on 12 Nov 2026.
        .env("FAKE_GH_GRAPHQL_UNAVAILABLE", "1")
        .env("FAKE_BIN", &self.d.fakes.bin);
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn dispatched_marker(&self) -> bool {
        std::path::PathBuf::from(self.p["private_state"].as_str().unwrap())
            .with_extension("dispatched")
            .exists()
    }
}

/// `Drive::prepare` plus `--pr`, which carries the PR number into the
/// private state so the dispatch reads this PR's head (#2333 C5).
fn prepare(d: &Drive, n: usize, pr: Option<&str>) -> Value {
    let record = d.p(&format!("crusty-round-{n}.json"));
    let context = d.p("evaluator-context.json");
    private_file(
        &context,
        &json!({
            "loop_name":"auto-drive:crusty", "loop_round_label":format!("round-{n}"),
            "loop_history":"", "loop_repo_path":d.repo, "loop_agent_working_dir":d.review,
            "loop_baseline_ref":d.head(), "loop_child_exit_code":"0",
            "loop_findings_current":d.read(&format!("crusty-round-{n}.json.findings")),
            "loop_findings_previous":"", "loop_test_signal":"", "loop_test_signal_previous":"",
            "loop_ci_signal":"", "loop_ci_signal_previous":"", "loop_clean_token":"CLEAN",
            "loop_round_verdict":"CLEAN", "loop_last_round_output":"fixture review"
        })
        .to_string(),
    );
    let label = format!("round-{n}");
    let tools = tools_dir();
    let mut args: Vec<&OsStr> = vec![
        OsStr::new("--state-dir"),
        d.state.as_os_str(),
        OsStr::new("--round-label"),
        OsStr::new(&label),
        OsStr::new("--record"),
        record.as_os_str(),
        OsStr::new("--repo"),
        d.repo.as_os_str(),
        OsStr::new("--review-dir"),
        d.review.as_os_str(),
        OsStr::new("--launch-tools"),
        tools.as_os_str(),
        OsStr::new("--evaluator-context-file"),
        context.as_os_str(),
    ];
    if let Some(pr) = pr {
        args.push(OsStr::new("--pr"));
        args.push(OsStr::new(pr));
    }
    let o = d.helper("prepare", &args);
    assert!(o.status.success(), "prepare must accept --pr: {}", show(&o));
    json_output(&o)
}

/// The dispatch's result line. An unavailable read is not an error, so a
/// dispatch that printed nothing has already failed what #2333 asks for;
/// this says so rather than reporting a missing field.
fn line(o: &Output) -> Value {
    stdout(o)
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str(l).ok())
        .unwrap_or_else(|| {
            panic!("an unavailable head read is not an error; the dispatch must still report a result: {}", show(o))
        })
}

/// `head_read`, on the dispatch line:
/// `^(matches|unavailable:(<token>))$`.
fn valid_head_read(v: &str) -> bool {
    v == "matches"
        || v.strip_prefix("unavailable:")
            .is_some_and(|t| TOKENS.contains(&t))
}

/// `loop_head_read`, in the evaluator's context: the bare token, or empty.
fn valid_loop_head_read(v: &str) -> bool {
    v.is_empty() || TOKENS.contains(&v)
}

/// No GraphQL-backed read may appear on this path at all: `gh pr view`,
/// `gh pr checks` and `gh api graphql` are all gone from it (#2333 R1).
fn assert_no_graphql_read(d: &Drive) {
    for argv in d.fakes.gh_calls() {
        let joined = argv.join(" ");
        assert!(
            !(joined.starts_with("pr view")
                || joined.starts_with("pr checks")
                || joined.starts_with("api graphql")),
            "the head read must not use GraphQL: gh {joined}"
        );
    }
}

// ── A2 case 1: the head matches ─────────────────────────────────────────────

/// `git ls-remote` answers the reviewed head while every GraphQL read is
/// unavailable, so the clean round completes exactly as before. This is the
/// software-factory #272 round-5 case of 12 Nov 2026, which was thrown away.
#[test]
fn a_matching_head_read_without_graphql_completes_the_round() {
    let c = Case::new();
    let o = c.dispatch(&[("FAKE_GIT_LS_REMOTE", "answer")]);
    assert!(
        o.status.success(),
        "a clean round at the reviewed head completes: {}",
        show(&o)
    );
    let v = json_output(&o);
    assert_eq!(v["loop_result"], "DONE", "{}", show(&o));
    assert_eq!(v["source"], "objective-completion", "{}", show(&o));
    assert_eq!(v["evaluator_invoked"], false, "{}", show(&o));
    assert_eq!(v["head_read"], "matches", "{}", show(&o));
    assert!(
        c.d.evaluators().is_empty(),
        "a matching head does not run the evaluator"
    );
    assert!(c.dispatched_marker(), "completion writes the receipt");
    assert_no_graphql_read(&c.d);
    assert!(
        !c.d.fakes.ls_remote_calls().is_empty(),
        "route 1 is git ls-remote"
    );
}

/// Route 2: `ls-remote` answers no line, so the REST read answers instead.
/// Two routes to one fact, never a retry: neither is repeated.
#[test]
fn an_empty_ls_remote_falls_through_to_the_rest_read() {
    let c = Case::new();
    let o = c.dispatch(&[("FAKE_GIT_LS_REMOTE", "empty")]);
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(json_output(&o)["head_read"], "matches", "{}", show(&o));
    assert_no_graphql_read(&c.d);
    let rest: Vec<_> =
        c.d.fakes
            .gh_calls()
            .into_iter()
            .filter(|a| a.first().map(String::as_str) == Some("api"))
            .filter(|a| a.iter().any(|x| x.contains("repos/o/r/pulls/7")))
            .collect();
    assert_eq!(
        rest.len(),
        1,
        "route 2 is read once, never retried: {rest:?}"
    );
    assert_eq!(
        c.d.fakes.ls_remote_calls().len(),
        1,
        "route 1 is not repeated either"
    );
}

// ── A2 case 2: the head differs ─────────────────────────────────────────────

/// A route answered with another SHA, so the completion claim is stale. The
/// message names both commits, and never says "unavailable" (#2472).
#[test]
fn a_differing_head_fails_closed_and_never_reads_as_unavailable() {
    let c = Case::new();
    let o = c.dispatch(&[
        ("FAKE_GIT_LS_REMOTE", "other"),
        ("FAKE_GIT_OTHER_SHA", OTHER_SHA),
    ]);
    assert!(
        !o.status.success(),
        "a head that moved is an error: {}",
        show(&o)
    );
    let err = stderr(&o);
    assert!(
        err.contains(OTHER_SHA) && err.contains(&c.d.head()),
        "the message names both commits: {}",
        show(&o)
    );
    assert!(
        !err.to_lowercase().contains("unavailable"),
        "a differing head is not reported as unavailable: {}",
        show(&o)
    );
    assert!(
        c.d.evaluators().is_empty(),
        "a stale completion claim is not handed to the evaluator"
    );
    assert!(
        !c.dispatched_marker(),
        "no completion receipt for a stale claim"
    );
}

// ── A2 case 3: the round-14 case, the read was unavailable ──────────────────

/// 12 Nov 2026, about 23:2xZ: round 14 was CLEAN at `b5864181`, which was
/// also the PR's head on GitHub, and only the read failed. The dispatch must
/// hand the round to the loop-health evaluator, not reject it.
#[test]
fn an_unavailable_head_read_goes_to_the_evaluator_not_invalid_decision() {
    let c = Case::new();
    let o = c.dispatch(&[
        ("FAKE_GIT_LS_REMOTE", "fail"),
        ("FAKE_GH_FAIL", "repos/o/r/pulls/7"),
    ]);
    let v = line(&o);
    assert_eq!(v["evaluator_invoked"], true, "{}", show(&o));
    assert_eq!(v["head_read"], "unavailable:read-failed", "{}", show(&o));
    assert_ne!(
        v["loop_result"],
        "DONE",
        "an unconfirmed head does not complete: {}",
        show(&o)
    );
    assert_eq!(
        c.d.evaluators().len(),
        1,
        "the evaluator judges the round with the fault in front of it"
    );
    assert!(
        c.dispatched_marker(),
        "the evaluator path writes the receipt, as the Evaluate action does"
    );
    // The loop reads `evaluator_invoked`: `true` is never INVALID_DECISION,
    // so the merge loop is never stopped with `crusty-rereview-not-done`.
    let out = format!("{}{}", stdout(&o), stderr(&o));
    assert!(
        !out.contains("INVALID_DECISION") && !out.contains("crusty-rereview-not-done"),
        "{}",
        show(&o)
    );
    assert_no_graphql_read(&c.d);
}

/// The evaluator is told what failed, through `loop_head_read`, and the
/// round's own verdict is still carried.
#[test]
fn the_evaluator_is_told_the_head_read_failed() {
    let c = Case::new();
    let o = c.dispatch(&[
        ("FAKE_GIT_LS_REMOTE", "fail"),
        ("FAKE_GH_FAIL", "repos/o/r/pulls/7"),
    ]);
    assert_eq!(c.d.evaluators().len(), 1, "{}", show(&o));
    let ctx = c.d.evaluators()[0]["context"].clone();
    assert_eq!(ctx["loop_head_read"], "read-failed", "{ctx}");
    assert_eq!(
        ctx["loop_round_verdict"], "CLEAN",
        "the round's own verdict still stands: {ctx}"
    );
}

/// Every other caller is unaffected: a round that reads its head normally
/// and goes to the evaluator carries an empty `loop_head_read`.
#[test]
fn a_readable_head_leaves_loop_head_read_empty() {
    let d = Drive::new(); // the default fixture review is CONCERNS
    d.record(1);
    let p = prepare(&d, 1, Some("7"));
    d.decide(&p, "evaluate");
    d.git(
        &d.repo,
        &["remote", "set-url", "origin", "https://github.com/o/r"],
    );
    d.fakes.with_fake_git();
    let o = d
        .cmd(crate::support::amplihack_bin::amplihack_bin())
        .args([
            "orch",
            "helper",
            "autodrive-completion-dispatch",
            "--private-state",
        ])
        .arg(p["private_state"].as_str().unwrap())
        .args(["--child-exit-code", "0"])
        .env("FAKE_BIN", &d.fakes.bin)
        .output()
        .unwrap();
    assert_eq!(d.evaluators().len(), 1, "{}", show(&o));
    let ctx = d.evaluators()[0]["context"].clone();
    let head_read = ctx["loop_head_read"].as_str().unwrap_or("");
    assert_eq!(head_read, "", "an unfailed read says nothing: {ctx}");
    assert!(valid_loop_head_read(head_read), "{ctx}");
}

// ── The grammar of the two fields (S10, S11) ────────────────────────────────

/// Both fields carry closed tokens. A later change must not quietly widen
/// them to carry `gh` stderr, a rate-limit header, a branch name or a URL.
#[test]
fn both_fields_carry_only_closed_tokens() {
    for (mode, want) in [
        ("fail", "unavailable:read-failed"),
        ("ambiguous", "unavailable:ambiguous-ref"),
        ("bad-sha", "unavailable:bad-sha"),
    ] {
        let c = Case::new();
        let o = c.dispatch(&[
            ("FAKE_GIT_LS_REMOTE", mode),
            ("FAKE_GH_FAIL", "repos/o/r/pulls/7"),
        ]);
        let v = line(&o);
        let head_read = v["head_read"].as_str().unwrap_or_default();
        assert_eq!(head_read, want, "mode {mode}: {}", show(&o));
        assert!(valid_head_read(head_read), "mode {mode}: {head_read}");
        assert_eq!(v["evaluator_invoked"], true, "mode {mode}: {}", show(&o));
        assert_eq!(c.d.evaluators().len(), 1, "mode {mode}: {}", show(&o));
        let ctx = c.d.evaluators()[0]["context"].clone();
        let loop_head_read = ctx["loop_head_read"].as_str().unwrap_or_default();
        assert!(valid_loop_head_read(loop_head_read), "mode {mode}: {ctx}");
        assert_eq!(
            loop_head_read,
            want.trim_start_matches("unavailable:"),
            "the context carries the bare token: {ctx}"
        );
    }
}

/// The fault text `gh` and `git` print is never carried into the fields or
/// into the evaluator's context: it is attacker-influenceable by anyone who
/// can open a PR or name a workflow.
#[test]
fn read_failure_text_never_reaches_the_evaluator() {
    let c = Case::new();
    let o = c.dispatch(&[
        ("FAKE_GIT_LS_REMOTE", "fail"),
        ("FAKE_GH_FAIL", "repos/o/r/pulls/7"),
    ]);
    let v = line(&o);
    let line = v.to_string();
    assert_eq!(
        c.d.evaluators().len(),
        1,
        "an unavailable read runs the evaluator: {}",
        show(&o)
    );
    let ctx = c.d.evaluators()[0]["context"].to_string();
    for leak in [
        "could not read from remote repository",
        "HTTP 404",
        "secondary rate limit",
        "https://github.com/o/r",
        "quick/demo",
    ] {
        assert!(
            !line.contains(leak),
            "dispatch line carries `{leak}`: {line}"
        );
        assert!(
            !ctx.contains(leak),
            "evaluator context carries `{leak}`: {ctx}"
        );
    }
    assert!(o.status.code().is_some());
}

// ── Security: F1, the origin a round's own agent can rewrite ────────────────

/// F1: `git ls-remote origin` would take its answer from a config file the
/// round's own agent has been writing to. An `origin` that is not a GitHub
/// remote in an accepted form is `origin-not-github`, never `Matches` and
/// never `Differs`, and no read is made against it at all.
#[test]
fn an_origin_that_is_not_github_is_unavailable_not_a_match() {
    for url in [
        "https://attacker.example/o/r",
        "https://github.com.attacker.example/o/r",
        "file:///tmp/o/r",
        "ssh://git@github.com/o/r",
        "-ext::sh -c id",
    ] {
        let c = Case::new();
        c.d.git(&c.d.repo, &["remote", "set-url", "origin", url]);
        // The fake would answer the reviewed head for any URL, so a Matches
        // here would mean the outcome was forged through the config.
        let o = c.dispatch(&[("FAKE_GIT_LS_REMOTE", "answer")]);
        let v = line(&o);
        assert_eq!(
            v["head_read"],
            "unavailable:origin-not-github",
            "origin {url}: {}",
            show(&o)
        );
        assert_ne!(v["loop_result"], "DONE", "origin {url}: {}", show(&o));
        assert_eq!(v["evaluator_invoked"], true, "origin {url}: {}", show(&o));
        assert!(
            c.d.fakes.ls_remote_calls().is_empty(),
            "origin {url}: the remote is verified before it is read: {:?}",
            c.d.fakes.ls_remote_calls()
        );
    }
}

/// The verified `<owner>/<repo>` pair is used as a literal argument on both
/// routes, so a rewrite in the worktree's config cannot redirect either one.
#[test]
fn both_routes_use_the_canonical_github_url() {
    let c = Case::new();
    let o = c.dispatch(&[("FAKE_GIT_LS_REMOTE", "empty")]);
    assert!(o.status.success(), "{}", show(&o));
    let ls = c.d.fakes.ls_remote_calls();
    assert_eq!(ls.len(), 1, "{ls:?}");
    assert!(
        ls[0].contains(&"https://github.com/o/r".to_string())
            && ls[0].contains(&"refs/pull/7/head".to_string()),
        "ls-remote takes the canonical URL and the PR ref literally: {ls:?}"
    );
    assert!(
        !ls[0].contains(&"origin".to_string()),
        "never the config's `origin` alias: {ls:?}"
    );
    assert!(
        c.d.fakes
            .gh_calls()
            .iter()
            .any(|a| a.iter().any(|x| x.contains("repos/o/r/pulls/7"))),
        "route 2 names the verified owner and repository"
    );
}

// ── Security: F2, Unavailable must not become a way to complete ─────────────

/// The head read runs only after `evidence::validate` has accepted the
/// record. A record that fails validation still errors, whatever the read
/// does, so a broken read can never turn an invalid record into a wait.
#[test]
fn an_invalid_record_still_errors_when_the_read_is_unavailable() {
    let c = Case::new();
    // Break the binding between the record and the manifest.
    private_file(
        &c.d.p("crusty-records.tsv"),
        "round-1\tcrusty-round-1.json\t0\n",
    );
    let o = c.dispatch(&[
        ("FAKE_GIT_LS_REMOTE", "fail"),
        ("FAKE_GH_FAIL", "repos/o/r/pulls/7"),
    ]);
    assert!(
        !o.status.success(),
        "an invalid record is an error, not a wait: {}",
        show(&o)
    );
    assert!(
        c.d.evaluators().is_empty(),
        "an invalid record never reaches the evaluator"
    );
    assert!(!c.dispatched_marker(), "and never gets a receipt");
}

// ── Security: S14, the shell cannot supply loop_head_read ───────────────────

/// `loop_head_read` is not a member of `evaluator::FIELDS`. Dispatch inserts
/// it directly; a context key of that name supplied through the shell is
/// still refused as an unknown field, so the round's own agent cannot tell
/// the evaluator that GitHub was unreadable.
#[test]
fn a_shell_supplied_loop_head_read_is_refused() {
    let d = Drive::new();
    d.record(1);
    let record = d.p("crusty-round-1.json");
    let context = d.p("evaluator-context.json");
    private_file(
        &context,
        &json!({
            "loop_name":"auto-drive:crusty", "loop_round_label":"round-1",
            "loop_history":"", "loop_repo_path":d.repo, "loop_agent_working_dir":d.review,
            "loop_child_exit_code":"0", "loop_clean_token":"CLEAN",
            "loop_round_verdict":"CONCERNS",
            "loop_head_read":"read-failed"
        })
        .to_string(),
    );
    let tools = tools_dir();
    let o = d.helper(
        "prepare",
        &[
            OsStr::new("--state-dir"),
            d.state.as_os_str(),
            OsStr::new("--round-label"),
            OsStr::new("round-1"),
            OsStr::new("--record"),
            record.as_os_str(),
            OsStr::new("--repo"),
            d.repo.as_os_str(),
            OsStr::new("--review-dir"),
            d.review.as_os_str(),
            OsStr::new("--launch-tools"),
            tools.as_os_str(),
            OsStr::new("--evaluator-context-file"),
            context.as_os_str(),
        ],
    );
    assert!(
        !o.status.success(),
        "loop_head_read is not a declared evaluator field: {}",
        show(&o)
    );
    assert!(
        stderr(&o).contains("unknown evaluator context field"),
        "{}",
        show(&o)
    );
}

// ── S7: where the PR number comes from ──────────────────────────────────────

/// The PR number is typed and bounded. It is never accepted from the record,
/// from the evaluator context, or from any file the round's agent writes.
#[test]
fn prepare_refuses_a_pr_number_that_is_not_a_bounded_positive_integer() {
    let d = Drive::new();
    d.record(1);
    for bad in ["0", "-1", "7 ", "07x", "10000000000", "七", ""] {
        let o = prepare_rc(&d, bad);
        assert!(
            !o.status.success(),
            "--pr {bad:?} must be refused: {}",
            show(&o)
        );
    }
}

fn prepare_rc(d: &Drive, pr: &str) -> Output {
    let record = d.p("crusty-round-1.json");
    let context = d.p("evaluator-context.json");
    private_file(
        &context,
        &json!({"loop_name":"auto-drive:crusty","loop_round_label":"round-1"}).to_string(),
    );
    let tools = tools_dir();
    d.helper(
        "prepare",
        &[
            OsStr::new("--state-dir"),
            d.state.as_os_str(),
            OsStr::new("--round-label"),
            OsStr::new("round-1"),
            OsStr::new("--record"),
            record.as_os_str(),
            OsStr::new("--repo"),
            d.repo.as_os_str(),
            OsStr::new("--review-dir"),
            d.review.as_os_str(),
            OsStr::new("--launch-tools"),
            tools.as_os_str(),
            OsStr::new("--evaluator-context-file"),
            context.as_os_str(),
            OsStr::new("--pr"),
            OsStr::new(pr),
        ],
    )
}

/// A state file written before `pr` existed has none, and the head is then
/// resolved from the PR worktree's current branch. A branch name that is not
/// a plain ref is refused rather than sanitised, and gives the unavailable
/// outcome rather than an error (S6).
#[test]
fn a_state_without_a_pr_number_resolves_the_branch_or_reports_it_unresolved() {
    let c = Case::with_pr(None);
    c.d.git(&c.d.repo, &["checkout", "-q", "--detach"]);
    let o = c.dispatch(&[("FAKE_GIT_LS_REMOTE", "answer")]);
    let v = line(&o);
    assert_eq!(
        v["head_read"],
        "unavailable:pr-number-unresolved",
        "a detached worktree resolves no PR: {}",
        show(&o)
    );
    assert_eq!(v["evaluator_invoked"], true, "{}", show(&o));
    assert_ne!(v["loop_result"], "DONE", "{}", show(&o));
}

// ── S16: a failed read fails rather than hangs ──────────────────────────────

/// The head read must not wait for a credential that will not come. The
/// dispatch returns, whatever the read did.
#[test]
fn the_head_read_never_waits_for_a_credential() {
    let c = Case::new();
    let o = c.dispatch(&[
        ("FAKE_GIT_LS_REMOTE", "fail"),
        ("FAKE_GH_FAIL", "repos/o/r/pulls/7"),
        ("GIT_ASKPASS", "/bin/false"),
        ("SSH_ASKPASS", "/bin/false"),
    ]);
    assert!(o.status.code().is_some(), "the dispatch returned");
    assert_eq!(
        line(&o)["head_read"],
        "unavailable:read-failed",
        "{}",
        show(&o)
    );
}
