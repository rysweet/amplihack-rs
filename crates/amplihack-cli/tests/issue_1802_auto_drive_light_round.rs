//! Issue #1802: the minor-only rule of the light crusty round.
//!
//! Three layers are tested:
//!
//! 1. `amplifier-bundle/tools/autodrive_light_round.sh defer`, run directly:
//!
//!    ```text
//!    autodrive_light_round.sh defer --record <record> --pr <N>
//!        --reviewed-head <sha> --repo <dir>        (concerns JSON on stdin)
//!    ```
//!
//!    The state directory is `dirname <record>`. The tool always prints one
//!    JSON line with `light_exit` (`off`, `none` or `minor-deferred`), exits
//!    0 on any decision, exits 2 on bad arguments, and passes exit 79
//!    through.
//! 2. Step-03 and step-06 of `autodrive-crusty-round.yaml`, extracted from
//!    the live YAML and run in bash with a fake `amplihack` and a fake `gh`.
//!    Without the `light-mode` marker their output is byte-identical to the
//!    output before #1802 (the golden tests below were captured from the
//!    unmodified recipe).
//! 3. Acceptance test (a): a light CLEAN record is accepted by
//!    `autodrive_crusty_final` through the manifest, and
//!    `autodrive_record_crusty_loop_done` does not mark deferred ids resolved.
//!
//! Every test skips, with a note, when `jq` is not installed.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};

use crate::support::auto_drive_light::{
    Fakes, LightRepo, called, calls_of, fake_bin, git, jq_available, private_dir, private_file,
    repo_root, round_tool, run_bash, show, stderr, stdout, step_command, tools_dir,
};

const ROUND: &str = "autodrive-crusty-round";
const STEP03: &str = "step-03-extract-crusty-verdict";
const STEP06: &str = "step-06-write-round-record";
const MARKER: &str = "<!-- auto-drive-light:pr-7 -->";

fn skip() -> bool {
    if !jq_available() {
        eprintln!("skip: `jq` not available on PATH");
        return true;
    }
    false
}

/// A repository with one commit after the base, a private state directory,
/// a round record path inside it, and the fakes.
struct Env {
    repo: LightRepo,
    state: PathBuf,
    record: PathBuf,
    fakes: Fakes,
    tmpdir: PathBuf,
    head: String,
}

impl Env {
    fn new() -> Self {
        let repo = LightRepo::new();
        repo.write("docs/guide.md", "# guide\n\nA small fix.\n");
        let head = repo.commit("small fix");
        let state = repo.tmp.path().join("state");
        private_dir(&state);
        let tmpdir = repo.tmp.path().join("private-tmp");
        private_dir(&tmpdir);
        let fakes = fake_bin(repo.tmp.path());
        let record = state.join("crusty-round-1.json");
        Env {
            repo,
            state,
            record,
            fakes,
            tmpdir,
            head,
        }
    }

    /// The state the light recipe leaves after a `light` decision at `head`.
    fn light(&self) -> &Self {
        self.light_at(&self.head)
    }

    fn light_at(&self, head: &str) -> &Self {
        private_file(&self.state.join("light-mode"), &format!("{head}\n"));
        let elig = json!({
            "decision": "light", "reason": "within-limits",
            "base": self.repo.base, "head": head,
            "lines": 2, "new_modules": 0, "public_api": 0, "schema": 0, "recipe_contract": 0,
            "limits": {"lines": 300, "new_modules": 0, "public_api": 0, "schema": 0, "recipe_contract": 0}
        });
        private_file(
            &self.state.join("light-eligibility.json"),
            &format!("{elig}\n"),
        );
        self
    }

    fn p(&self, name: &str) -> PathBuf {
        self.state.join(name)
    }

    fn rec(&self, suffix: &str) -> PathBuf {
        PathBuf::from(format!("{}{suffix}", self.record.display()))
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.p(name)).unwrap_or_default()
    }

    fn defer(&self, concerns: &Value, env: &[(&str, &str)]) -> Output {
        self.defer_raw(&concerns.to_string(), &self.head.clone(), env)
    }

    fn defer_raw(&self, stdin: &str, reviewed: &str, env: &[(&str, &str)]) -> Output {
        let args: Vec<OsString> = vec![
            round_tool().into(),
            "defer".into(),
            "--record".into(),
            self.record.clone().into(),
            "--pr".into(),
            "7".into(),
            "--reviewed-head".into(),
            reviewed.into(),
            "--repo".into(),
            self.repo.wt.clone().into(),
        ];
        self.tool(&args, env, stdin)
    }

    fn tool(&self, args: &[OsString], env: &[(&str, &str)], stdin: &str) -> Output {
        let argv: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
        let root = repo_root();
        let mut e: Vec<(&str, &OsStr)> = vec![
            ("TMPDIR", self.tmpdir.as_os_str()),
            ("FAKE_GH_HEAD", OsStr::new(&self.head)),
            ("AMPLIHACK_HOME", root.as_os_str()),
        ];
        e.extend(env.iter().map(|(k, v)| (*k, OsStr::new(*v))));
        run_bash(&self.repo.home, &self.fakes, None, &argv, &e, stdin)
    }
}

/// The tool's single stdout line, parsed.
fn result(o: &Output) -> Value {
    let out = stdout(o);
    let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one JSON line on stdout: {}",
        show(o)
    );
    serde_json::from_str(lines[0]).unwrap_or_else(|e| panic!("not JSON ({e}): {}", show(o)))
}

fn exit_none(o: &Output, reason: &str) -> Value {
    assert_eq!(o.status.code(), Some(0), "{}", show(o));
    let j = result(o);
    assert_eq!(j["light_exit"], "none", "{}", show(o));
    assert_eq!(j["reason"], reason, "{}", show(o));
    j
}

fn deferred(o: &Output) -> Value {
    assert_eq!(o.status.code(), Some(0), "{}", show(o));
    let j = result(o);
    assert_eq!(j["light_exit"], "minor-deferred", "{}", show(o));
    j
}

fn minor(id: &str, summary: &str) -> Value {
    json!({"id": id, "severity": "minor", "summary": summary, "evidence": format!("{id}.rs line 1")})
}

fn two_minor() -> Value {
    json!([
        minor("naming-1", "Rename tmp_rec"),
        minor("doc-2", "Header comment is stale")
    ])
}

fn mode(p: &Path) -> u32 {
    std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

/// Writes the `.findings` file step-03 writes before it calls the tool.
fn findings(e: &Env, ids: &[&str]) {
    let body: String = ids.iter().map(|i| format!("{i}\n")).collect();
    private_file(&e.rec(".findings"), &body);
}

fn no_issue_created(e: &Env) {
    assert!(
        !called(&e.fakes.gh_calls(), "issue", "create"),
        "no issue may be created: {:?}",
        e.fakes.gh_calls()
    );
}

// ===========================================================================
// 1. autodrive_light_round.sh defer
// ===========================================================================

#[test]
fn minor_only_concerns_are_deferred_to_a_new_follow_up_issue() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    findings(&e, &["naming-1", "doc-2"]);
    let j = deferred(&e.defer(&two_minor(), &[]));
    assert_eq!(j["follow_up_issue"], "1851");
    assert_eq!(j["deferred_ids"], json!(["naming-1", "doc-2"]));
    assert_eq!(j["concern_count"], 2);

    // Side files.
    assert_eq!(std::fs::read(e.rec(".findings")).unwrap(), b"");
    let tsv = std::fs::read_to_string(e.rec(".concerns.tsv")).unwrap();
    let rows: Vec<Vec<&str>> = tsv.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(rows.len(), 2, "{tsv}");
    assert_eq!(&rows[0][..2], ["naming-1", "minor"]);
    assert_eq!(&rows[1][..2], ["doc-2", "minor"]);
    let side: Value =
        serde_json::from_str(&std::fs::read_to_string(e.rec(".light.json")).unwrap()).unwrap();
    assert_eq!(
        side,
        json!({"follow_up_issue": "1851", "deferred_ids": ["naming-1", "doc-2"]})
    );
    assert_eq!(e.read("follow-up-issue").trim(), "1851");
    assert!(e.p("follow-up-linked").is_file());
    let posted: Vec<String> = e
        .read("follow-up-posted-ids")
        .lines()
        .map(String::from)
        .collect();
    assert!(posted.contains(&"naming-1".into()) && posted.contains(&"doc-2".into()));
    for f in [
        e.rec(".concerns.tsv"),
        e.rec(".light.json"),
        e.p("follow-up-issue"),
        e.p("follow-up-posted-ids"),
    ] {
        assert_eq!(mode(&f), 0o600, "{} must be private", f.display());
    }

    // gh: one issue created through --body-file, with the fixed title.
    let calls = e.fakes.gh_calls();
    let creates = calls_of(&calls, "issue", "create");
    assert_eq!(creates.len(), 1, "{calls:?}");
    let c = creates[0];
    assert!(c.iter().any(|a| a == "--body-file"), "{c:?}");
    assert!(!c.iter().any(|a| a == "--body" || a == "-b"), "{c:?}");
    let ti = c
        .iter()
        .position(|a| a == "--title" || a == "-t")
        .expect("--title");
    assert_eq!(c[ti + 1], "Follow-up: minor review concerns from PR #7");
    assert!(
        !c.iter().any(|a| a == "--label" || a == "-l"),
        "no label unless it exists"
    );
    let body = &e.fakes.bodies()[0];
    assert_eq!(body.matches(MARKER).count(), 1, "{body}");
    assert!(body.contains("naming-1") && body.contains("doc-2"));
    assert!(
        body.contains("| id | severity | summary | evidence |"),
        "{body}"
    );

    // gh: the PR is linked once with the fixed text.
    let comments = calls_of(&calls, "pr", "comment");
    assert_eq!(comments.len(), 1, "{calls:?}");
    let text = comments[0].join(" ") + &e.fakes.bodies().join(" ");
    assert!(
        text.contains("Minor review concerns deferred to #1851."),
        "{text}"
    );
    assert!(comments[0].iter().any(|a| a == "7"));

    // The body file is removed after a successful create.
    assert!(!e.p("follow-up-body.md").exists());
}

#[test]
fn the_follow_up_label_is_added_only_when_it_exists() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    deferred(&e.defer(
        &two_minor(),
        &[("FAKE_GH_LABELS", r#"[{"name":"bug"},{"name":"follow-up"}]"#)],
    ));
    let calls = e.fakes.gh_calls();
    let c = calls_of(&calls, "issue", "create")[0];
    let li = c
        .iter()
        .position(|a| a == "--label" || a == "-l")
        .expect("--label");
    assert_eq!(c[li + 1], "follow-up");
}

#[test]
fn a_major_or_blocking_concern_takes_the_normal_path() {
    if skip() {
        return;
    }
    for sev in ["major", "blocking"] {
        let e = Env::new();
        e.light();
        findings(&e, &["naming-1", "real-bug"]);
        let concerns = json!([
            minor("naming-1", "Rename"),
            {"id": "real-bug", "severity": sev, "summary": "Wrong result", "evidence": "x"}
        ]);
        exit_none(&e.defer(&concerns, &[]), "not-minor-only");
        assert_eq!(
            e.read("crusty-round-1.json.findings"),
            "naming-1\nreal-bug\n"
        );
        assert!(!e.rec(".light.json").exists());
        no_issue_created(&e);
    }
}

#[test]
fn unknown_or_missing_severity_is_major() {
    if skip() {
        return;
    }
    for c in [
        json!({"id": "a-1", "summary": "no severity"}),
        json!({"id": "a-1", "severity": "", "summary": "empty"}),
        json!({"id": "a-1", "severity": "Minor", "summary": "case differs"}),
        json!({"id": "a-1", "severity": "nit", "summary": "unknown"}),
        json!({"id": "a-1", "severity": 1, "summary": "not a string"}),
    ] {
        let e = Env::new();
        e.light();
        exit_none(
            &e.defer(&json!([minor("naming-1", "x"), c]), &[]),
            "not-minor-only",
        );
        no_issue_created(&e);
    }
}

#[test]
fn a_missing_or_malformed_id_is_major() {
    if skip() {
        return;
    }
    for c in [
        json!({"severity": "minor", "summary": "no id"}),
        json!({"id": "", "severity": "minor", "summary": "empty id"}),
        json!({"id": "has space", "severity": "minor", "summary": "x"}),
        json!({"id": "a;rm -rf /", "severity": "minor", "summary": "x"}),
        json!({"id": "x".repeat(65), "severity": "minor", "summary": "too long"}),
        json!({"id": "nl\nid", "severity": "minor", "summary": "x"}),
    ] {
        let e = Env::new();
        e.light();
        exit_none(&e.defer(&json!([c]), &[]), "not-minor-only");
        no_issue_created(&e);
    }
}

#[test]
fn ids_containing_a_security_keyword_are_major() {
    if skip() {
        return;
    }
    for id in [
        "auth-check",
        "sec-1",
        "section-1",
        "permalink-2",
        "author-name",
        "TOKEN-leak",
        "xss-1",
        "csrf",
        "cve-2026-1",
        "Vuln.2",
        "unsafe-block",
        "privilege-drop",
        "secret-in-log",
        "credential-1",
        "sql-inject",
    ] {
        let e = Env::new();
        e.light();
        exit_none(&e.defer(&json!([minor(id, "x")]), &[]), "not-minor-only");
        no_issue_created(&e);
    }
}

#[test]
fn a_security_word_in_the_summary_or_evidence_is_major() {
    if skip() {
        return;
    }
    let cases = [
        json!({"id": "c3", "severity": "minor", "summary": "Possible SQL injection in the query builder", "evidence": "db.rs line 4"}),
        json!({"id": "c3", "severity": "minor", "summary": "Leaks the API Token to the log", "evidence": "log.rs"}),
        json!({"id": "c3", "severity": "minor", "summary": "Tidy this", "evidence": "the path is not sanitised before use"}),
        json!({"id": "c3", "severity": "minor", "summary": "Missing authorization check", "evidence": null}),
        json!({"id": "c3", "severity": "minor", "summary": "Insecure default", "evidence": "x"}),
    ];
    for c in cases {
        let e = Env::new();
        e.light();
        exit_none(&e.defer(&json!([c]), &[]), "not-minor-only");
        no_issue_created(&e);
    }
}

#[test]
fn common_words_in_the_summary_do_not_count_as_security_words() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let c = json!([{"id": "c3", "severity": "minor",
        "summary": "The second author line repeats the section title",
        "evidence": "permalink in README.md line 2"}]);
    deferred(&e.defer(&c, &[]));
}

#[test]
fn without_a_follow_up_label_the_issue_is_created_without_one() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let j = deferred(&e.defer(&two_minor(), &[("FAKE_GH_LABELS", r#"[{"name":"bug"}]"#)]));
    assert_eq!(j["light_exit"], "minor-deferred");
    let calls = e.fakes.gh_calls();
    let c = calls_of(&calls, "issue", "create")[0];
    assert!(
        !c.iter().any(|a| a == "--label" || a == "-l"),
        "no label may be passed: {c:?}"
    );
    // Both ids are recorded as posted, from an empty start.
    assert_eq!(e.read("follow-up-posted-ids"), "naming-1\ndoc-2\n");
}

#[test]
fn a_failed_issue_list_is_logged_before_the_duplicate_check_is_skipped() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let o = e.defer(&two_minor(), &[("FAKE_GH_FAIL", "issue list")]);
    assert!(
        stderr(&o).contains("WARNING: light round: issue list failed; duplicate check skipped"),
        "{}",
        show(&o)
    );
}

#[test]
fn an_empty_concerns_array_is_not_minor_only() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    exit_none(&e.defer(&json!([]), &[]), "no-concerns");
    no_issue_created(&e);
    assert!(e.fakes.gh_calls().is_empty());
}

#[test]
fn a_returning_id_is_promoted_to_major() {
    if skip() {
        return;
    }
    // Recorded as resolved by an earlier DONE crusty loop.
    let e = Env::new();
    e.light();
    private_file(&e.p("resolved-concerns.txt"), "naming-1\n");
    exit_none(&e.defer(&two_minor(), &[]), "not-minor-only");
    no_issue_created(&e);

    // Raised in an earlier round whose step-04 ran (non-empty .findings).
    let e = Env::new();
    e.light();
    private_file(&e.p("crusty-round-0.json"), "{}\n");
    private_file(&e.p("crusty-round-0.json.findings"), "doc-2\n");
    exit_none(&e.defer(&two_minor(), &[]), "not-minor-only");
    no_issue_created(&e);
}

#[test]
fn a_deferred_id_raised_again_stays_minor_and_is_not_posted_again() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    // An earlier light round deferred both ids: its .findings is empty.
    private_file(&e.p("crusty-round-0.json"), "{}\n");
    private_file(&e.p("crusty-round-0.json.findings"), "");
    private_file(
        &e.p("crusty-round-0.json.concerns.tsv"),
        "naming-1\tminor\tRename\ndoc-2\tminor\tStale\n",
    );
    private_file(&e.p("follow-up-issue"), "1851\n");
    private_file(&e.p("follow-up-linked"), "1851\n");
    private_file(&e.p("follow-up-posted-ids"), "doc-2\nnaming-1\n");
    let j = deferred(&e.defer(&two_minor(), &[]));
    assert_eq!(j["follow_up_issue"], "1851");
    let calls = e.fakes.gh_calls();
    assert!(!called(&calls, "issue", "create"), "{calls:?}");
    assert!(
        !called(&calls, "issue", "comment"),
        "nothing new to post: {calls:?}"
    );
    assert!(
        !called(&calls, "pr", "comment"),
        "the PR is linked once: {calls:?}"
    );
}

#[test]
fn a_confirming_round_posts_only_new_ids_to_the_existing_issue() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    private_file(&e.p("follow-up-issue"), "1851\n");
    private_file(&e.p("follow-up-linked"), "1851\n");
    private_file(&e.p("follow-up-posted-ids"), "naming-1\n");
    let j = deferred(&e.defer(&two_minor(), &[]));
    assert_eq!(j["follow_up_issue"], "1851");
    let calls = e.fakes.gh_calls();
    assert!(!called(&calls, "issue", "create"), "{calls:?}");
    assert!(!called(&calls, "pr", "comment"), "{calls:?}");
    let comments = calls_of(&calls, "issue", "comment");
    assert_eq!(comments.len(), 1, "one comment for the new ids: {calls:?}");
    assert!(comments[0].iter().any(|a| a == "1851"));
    assert!(comments[0].iter().any(|a| a == "--body-file"));
    let body = e.fakes.bodies().pop().unwrap();
    assert!(body.contains("doc-2"), "{body}");
    assert!(!body.contains("naming-1"), "already posted: {body}");
    let posted = e.read("follow-up-posted-ids");
    assert!(posted.lines().any(|l| l == "doc-2") && posted.lines().any(|l| l == "naming-1"));
}

#[test]
fn a_recorded_issue_number_that_is_not_digits_is_never_used() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    private_file(&e.p("follow-up-issue"), "12; touch /tmp/x\n");
    let j = deferred(&e.defer(&two_minor(), &[]));
    assert_eq!(j["follow_up_issue"], "1851");
    for c in e.fakes.gh_calls() {
        assert!(!c.join(" ").contains("touch"), "{c:?}");
    }
    assert_eq!(e.read("follow-up-issue").trim(), "1851");
}

#[test]
fn an_open_issue_found_by_search_is_reused_when_written_by_this_login() {
    if skip() {
        return;
    }
    let body = format!("{MARKER}\nMinor review concerns raised by crusty on PR #7.\n");
    let hit_cli =
        json!([{"number": 1900, "state": "OPEN", "author": {"login": "me"}, "body": body}]);
    let hit_api = json!([{"number": 1900, "state": "open", "user": {"login": "me"}, "body": body}]);
    let view = json!({"number": 1900, "state": "OPEN", "author": {"login": "me"}, "body": body});
    let api_view = json!({"number": 1900, "state": "open", "user": {"login": "me"}, "body": body});
    let env = |login: &str| -> Vec<(String, String)> {
        vec![
            (
                "FAKE_GH_SEARCH".into(),
                hit_cli
                    .to_string()
                    .replace("\"me\"", &format!("\"{login}\"")),
            ),
            (
                "FAKE_GH_SEARCH_API".into(),
                hit_api
                    .to_string()
                    .replace("\"me\"", &format!("\"{login}\"")),
            ),
            (
                "FAKE_GH_ISSUE_VIEW".into(),
                view.to_string().replace("\"me\"", &format!("\"{login}\"")),
            ),
            (
                "FAKE_GH_ISSUE_API".into(),
                api_view
                    .to_string()
                    .replace("\"me\"", &format!("\"{login}\"")),
            ),
        ]
    };

    let e = Env::new();
    e.light();
    let v = env("me");
    let refs: Vec<(&str, &str)> = v.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let j = deferred(&e.defer(&two_minor(), &refs));
    assert_eq!(j["follow_up_issue"], "1900");
    no_issue_created(&e);
    assert_eq!(e.read("follow-up-issue").trim(), "1900");

    // Written by someone else: not reused.
    let e = Env::new();
    e.light();
    let v = env("mallory");
    let refs: Vec<(&str, &str)> = v.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let j = deferred(&e.defer(&two_minor(), &refs));
    assert_eq!(j["follow_up_issue"], "1851");
}

#[test]
fn issue_cells_are_sanitised_and_the_marker_appears_once() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let nasty = format!(
        "{MARKER} ping @someone about #12 | col `code` <b>&\u{7}{}",
        "x".repeat(500)
    );
    let concerns =
        json!([{"id": "style-1", "severity": "minor", "summary": nasty, "evidence": "@other #34"}]);
    deferred(&e.defer(&concerns, &[]));
    let body = e.fakes.bodies()[0].clone();
    assert_eq!(body.matches(MARKER).count(), 1, "{body}");
    assert!(
        !body.contains("@someone") && !body.contains("@other"),
        "{body}"
    );
    assert!(!body.contains("#12") && !body.contains("#34"), "{body}");
    assert!(!body.contains("<b>"), "{body}");
    assert!(!body.contains('\u{7}'), "control characters are removed");
    assert!(
        !body.contains(&"x".repeat(201)),
        "cells are truncated to 200 characters"
    );
    let row = body.lines().find(|l| l.contains("style-1")).expect("row");
    // id, severity, summary, evidence: the escaped `|` adds no column.
    assert_eq!(
        row.matches(" | ").count() - row.matches("\\|").count(),
        3,
        "{row}"
    );
}

#[test]
fn full_github_urls_in_cells_are_broken() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let concerns = json!([{"id": "style-1", "severity": "minor",
        "summary": "see https://github.com/o/r/issues/12",
        "evidence": "and HTTPS://GitHub.com/o/r/pull/34"}]);
    deferred(&e.defer(&concerns, &[]));
    let body = e.fakes.bodies()[0].clone();
    assert!(
        !body.contains("github.com/o/r/issues/12"),
        "the URL is not left whole: {body}"
    );
    assert!(!body.contains("GitHub.com/o/r/pull/34"), "{body}");
    assert!(body.contains("github.com/\u{200b}o/r/issues/12"), "{body}");
}

#[test]
fn junk_lines_in_posted_ids_do_not_stop_a_post() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    private_file(&e.p("follow-up-issue"), "1851\n");
    private_file(&e.p("follow-up-linked"), "1851\n");
    // A forged line that is not an id, and one that names doc-2 with junk.
    private_file(
        &e.p("follow-up-posted-ids"),
        "naming-1\ndoc-2 extra\n../doc-2\n",
    );
    deferred(&e.defer(&two_minor(), &[]));
    let body = e.fakes.bodies().pop().unwrap();
    assert!(body.contains("doc-2"), "doc-2 is posted: {body}");
    assert!(!body.contains("naming-1"), "{body}");
    let posted = e.read("follow-up-posted-ids");
    assert_eq!(posted, "naming-1\ndoc-2\n", "junk is not written back");
}

#[test]
fn a_failure_to_record_follow_up_failed_is_reported() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    findings(&e, &["naming-1", "doc-2"]);
    std::fs::create_dir(e.p("follow-up-failed")).unwrap();
    let o = e.defer(&two_minor(), &[("FAKE_GH_FAIL", "issue create")]);
    exit_none(&o, "follow-up-failed");
    assert!(
        stderr(&o).contains("ERROR: light round: follow-up-failed could not be written"),
        "{}",
        show(&o)
    );
}

#[test]
fn a_gh_failure_keeps_the_normal_path_and_truncates_nothing() {
    if skip() {
        return;
    }
    for fail in ["issue create", "pr comment", "issue list", "api user"] {
        let e = Env::new();
        e.light();
        findings(&e, &["naming-1", "doc-2"]);
        let o = e.defer(&two_minor(), &[("FAKE_GH_FAIL", fail)]);
        if fail == "issue list" || fail == "api user" {
            // The search fallback failing must not stop a create.
            let j = result(&o);
            assert!(
                j["light_exit"] == "minor-deferred"
                    || (j["light_exit"] == "none" && j["reason"] == "follow-up-failed"),
                "{fail}: {}",
                show(&o)
            );
            if j["light_exit"] == "minor-deferred" {
                continue;
            }
        } else {
            exit_none(&o, "follow-up-failed");
        }
        assert_eq!(
            e.read("crusty-round-1.json.findings"),
            "naming-1\ndoc-2\n",
            "{fail}"
        );
        assert!(!e.rec(".light.json").exists(), "{fail}");
        assert!(e.p("follow-up-failed").is_file(), "{fail}");
        let f = e.read("follow-up-failed");
        assert!(
            !f.contains("HTTP"),
            "follow-up-failed carries no free text: {f}"
        );
    }
}

#[test]
fn an_unparseable_issue_url_is_a_follow_up_failure() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    findings(&e, &["naming-1", "doc-2"]);
    exit_none(
        &e.defer(&two_minor(), &[("FAKE_GH_NEW_ISSUE", "12;evil")]),
        "follow-up-failed",
    );
    assert!(!e.p("follow-up-issue").exists() || !e.read("follow-up-issue").contains("evil"));
}

#[test]
fn exit_79_from_gh_is_passed_through() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let o = e.defer(
        &two_minor(),
        &[
            ("FAKE_GH_FAIL", "issue create"),
            ("FAKE_GH_EXIT_CODE", "79"),
        ],
    );
    assert_eq!(o.status.code(), Some(79), "{}", show(&o));
    let creates = calls_of(&e.fakes.gh_calls(), "issue", "create").len();
    assert_eq!(creates, 1, "exit 79 is never retried");
}

#[test]
fn no_retry_temporaries_or_body_files_are_left_behind() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    deferred(&e.defer(&two_minor(), &[]));
    let left: Vec<_> = std::fs::read_dir(&e.tmpdir).unwrap().collect();
    assert!(left.is_empty(), "TMPDIR must be clean: {left:?}");
    for entry in std::fs::read_dir(&e.state).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(!name.starts_with('.'), "stray temporary {name}");
        assert!(!name.ends_with(".md"), "stray body file {name}");
    }
}

// --- marker validation -----------------------------------------------------

#[test]
fn without_a_marker_the_tool_is_off_and_touches_nothing() {
    if skip() {
        return;
    }
    let e = Env::new();
    findings(&e, &["naming-1", "doc-2"]);
    let o = e.defer(&two_minor(), &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(result(&o)["light_exit"], "off");
    assert!(e.fakes.gh_calls().is_empty());
    let names: Vec<String> = std::fs::read_dir(&e.state)
        .unwrap()
        .map(|d| d.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["crusty-round-1.json.findings"]);
}

#[test]
fn an_invalid_marker_or_inconsistent_state_takes_the_normal_path() {
    if skip() {
        return;
    }
    type Spoil = fn(&Env);
    let cases: Vec<(&str, Spoil)> = vec![
        ("not hex", |e| private_file(&e.p("light-mode"), "light\n")),
        ("uppercase hex", |e| {
            private_file(&e.p("light-mode"), &format!("{}\n", e.head.to_uppercase()))
        }),
        ("escalation present", |e| {
            private_file(&e.p("escalation.json"), "{\"reason\":\"lines\"}\n")
        }),
        ("eligibility escalate", |e| {
            let j = json!({"decision": "escalate", "reason": "lines", "base": e.repo.base, "head": e.head});
            private_file(&e.p("light-eligibility.json"), &format!("{j}\n"))
        }),
        ("eligibility other head", |e| {
            let j = json!({"decision": "light", "reason": "within-limits", "base": e.repo.base, "head": e.repo.base});
            private_file(&e.p("light-eligibility.json"), &format!("{j}\n"))
        }),
        ("eligibility missing", |e| {
            std::fs::remove_file(e.p("light-eligibility.json")).unwrap()
        }),
        ("marker world-readable-writable", |e| {
            std::fs::set_permissions(e.p("light-mode"), std::fs::Permissions::from_mode(0o666))
                .unwrap()
        }),
        ("eligibility group-writable", |e| {
            std::fs::set_permissions(
                e.p("light-eligibility.json"),
                std::fs::Permissions::from_mode(0o620),
            )
            .unwrap()
        }),
        ("marker is a symlink", |e| {
            let real = e.repo.tmp.path().join("elsewhere");
            std::fs::write(&real, format!("{}\n", e.head)).unwrap();
            std::fs::remove_file(e.p("light-mode")).unwrap();
            std::os::unix::fs::symlink(&real, e.p("light-mode")).unwrap();
        }),
    ];
    for (name, spoil) in cases {
        let e = Env::new();
        e.light();
        spoil(&e);
        findings(&e, &["naming-1", "doc-2"]);
        let o = e.defer(&two_minor(), &[]);
        assert_eq!(o.status.code(), Some(0), "{name}: {}", show(&o));
        let j = result(&o);
        assert!(
            (j["light_exit"] == "none"
                && (j["reason"] == "marker-invalid" || j["reason"] == "state-unsafe"))
                || (name == "marker is a symlink" && j["light_exit"] == "off"),
            "{name}: {}",
            show(&o)
        );
        assert_eq!(
            e.read("crusty-round-1.json.findings"),
            "naming-1\ndoc-2\n",
            "{name}"
        );
        no_issue_created(&e);
    }
}

#[test]
fn a_reviewed_head_that_does_not_descend_from_the_marker_head_is_stale() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    git(&e.repo.wt, &["checkout", "-q", "-b", "side", &e.repo.base]);
    e.repo.write("docs/other.md", "x\n");
    let side = e.repo.commit("side");
    findings(&e, &["naming-1", "doc-2"]);
    exit_none(
        &e.defer_raw(&two_minor().to_string(), &side, &[]),
        "marker-stale",
    );
    assert!(e.p("light-mode").exists(), "a stale marker is not removed");
    no_issue_created(&e);
}

// --- re-measurement inside the loop ------------------------------------------

#[test]
fn a_moved_head_within_limits_is_re_measured_and_stays_light() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    e.repo.write(
        "docs/guide.md",
        "# guide\n\nA small fix.\nAnd an address commit.\n",
    );
    let moved = e.repo.commit("address");
    let j = deferred(&e.defer_raw(
        &two_minor().to_string(),
        &moved,
        &[("FAKE_GH_HEAD", &moved)],
    ));
    assert_eq!(j["follow_up_issue"], "1851");
    assert_eq!(e.read("light-mode").trim(), moved);
    let elig: Value = serde_json::from_str(&e.read("light-eligibility.json")).unwrap();
    assert_eq!(elig["head"], moved.as_str());
    assert_eq!(elig["decision"], "light");
    assert_eq!(elig["base"], e.repo.base.as_str());
    assert_eq!(mode(&e.p("light-mode")), 0o600);
    assert_eq!(mode(&e.p("light-eligibility.json")), 0o600);
}

#[test]
fn a_moved_head_over_a_limit_escalates_inside_the_loop() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    e.repo.write("docs/big.md", &"line\n".repeat(400));
    let moved = e.repo.commit("big address");
    findings(&e, &["naming-1", "doc-2"]);
    exit_none(
        &e.defer_raw(
            &two_minor().to_string(),
            &moved,
            &[("FAKE_GH_HEAD", &moved)],
        ),
        "re-measured-escalate",
    );
    assert!(
        !e.p("light-mode").exists(),
        "the marker is removed on escalation"
    );
    let esc: Value = serde_json::from_str(&e.read("escalation.json")).expect("escalation.json");
    assert_eq!(esc["reason"], "in-loop-lines");
    assert_eq!(esc["head_sha"], moved.as_str());
    assert_eq!(esc["base_sha"], e.repo.base.as_str());
    assert_eq!(mode(&e.p("escalation.json")), 0o600);
    assert_eq!(e.read("crusty-round-1.json.findings"), "naming-1\ndoc-2\n");
    no_issue_created(&e);

    // Every later round finds no marker.
    let o = e.defer_raw(
        &two_minor().to_string(),
        &moved,
        &[("FAKE_GH_HEAD", &moved)],
    );
    assert_eq!(result(&o)["light_exit"], "off", "{}", show(&o));
}

#[test]
fn a_re_measurement_fault_escalates_with_a_fault_reason() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    // Point the recorded base at a commit that is not an ancestor.
    git(&e.repo.wt, &["checkout", "-q", "-b", "side", &e.repo.base]);
    e.repo.write("docs/side.md", "x\n");
    let side = e.repo.commit("side");
    git(&e.repo.wt, &["checkout", "-q", "main"]);
    let j = json!({"decision": "light", "reason": "within-limits", "base": side, "head": e.head,
                   "limits": {"lines": 300, "new_modules": 0, "public_api": 0, "schema": 0, "recipe_contract": 0}});
    private_file(&e.p("light-eligibility.json"), &format!("{j}\n"));
    e.repo.write("docs/guide.md", "moved\n");
    let moved = e.repo.commit("moved");
    exit_none(
        &e.defer_raw(
            &two_minor().to_string(),
            &moved,
            &[("FAKE_GH_HEAD", &moved)],
        ),
        "re-measured-escalate",
    );
    let esc: Value = serde_json::from_str(&e.read("escalation.json")).unwrap();
    assert!(
        esc["reason"]
            .as_str()
            .unwrap()
            .starts_with("in-loop-fault-"),
        "{esc}"
    );
    assert!(!e.p("light-mode").exists());
}

/// Pushes a 400-line commit to `origin/main` from a second clone, so the
/// working repository does not hold it yet, and returns its SHA.
fn upstream_commit(e: &Env) -> String {
    let other = e.repo.tmp.path().join("other");
    let origin = e.repo.tmp.path().join("origin.git");
    git(
        e.repo.tmp.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    std::fs::write(other.join("upstream.md"), "line\n".repeat(400)).unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-q", "-m", "upstream"]);
    git(&other, &["push", "-q", "origin", "HEAD:main"]);
    git(&other, &["rev-parse", "HEAD"])
}

/// Merges `origin/main` into the PR branch and returns the merge commit.
fn merge_upstream(e: &Env) -> String {
    git(&e.repo.wt, &["fetch", "-q", "origin"]);
    git(
        &e.repo.wt,
        &["merge", "-q", "--no-edit", "--no-ff", "origin/main"],
    );
    e.repo.head()
}

#[test]
fn a_moved_head_that_merged_the_base_branch_is_measured_from_the_new_merge_base() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let up = upstream_commit(&e);
    let moved = merge_upstream(&e);
    findings(&e, &["naming-1", "doc-2"]);
    let j = deferred(&e.defer_raw(
        &two_minor().to_string(),
        &moved,
        &[("FAKE_GH_HEAD", &moved), ("FAKE_GH_BASE_OID", &up)],
    ));
    assert_eq!(j["follow_up_issue"], "1851");
    assert_eq!(e.read("light-mode").trim(), moved);
    let elig: Value = serde_json::from_str(&e.read("light-eligibility.json")).unwrap();
    assert_eq!(elig["decision"], "light");
    assert_eq!(elig["head"], moved.as_str());
    assert_eq!(
        elig["base"],
        up.as_str(),
        "the new base is recorded, so the next round's reuse check uses it"
    );
    assert!(
        called(&e.fakes.gh_calls(), "pr", "view"),
        "{:?}",
        e.fakes.gh_calls()
    );
}

#[test]
fn a_base_tip_missing_locally_is_fetched_before_the_merge_base() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let up = upstream_commit(&e);
    e.repo.write(
        "docs/guide.md",
        "# guide\n\nA small fix.\nAnd an address commit.\n",
    );
    let moved = e.repo.commit("address");
    findings(&e, &["naming-1", "doc-2"]);
    deferred(&e.defer_raw(
        &two_minor().to_string(),
        &moved,
        &[("FAKE_GH_HEAD", &moved), ("FAKE_GH_BASE_OID", &up)],
    ));
    git(&e.repo.wt, &["cat-file", "-e", &format!("{up}^{{commit}}")]);
    let elig: Value = serde_json::from_str(&e.read("light-eligibility.json")).unwrap();
    // The PR did not merge the base, so the merge-base is still the pinned base.
    assert_eq!(elig["base"], e.repo.base.as_str());
}

#[test]
fn the_pr_s_own_change_over_a_limit_still_escalates_after_a_base_merge() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let up = upstream_commit(&e);
    merge_upstream(&e);
    e.repo.write("docs/big.md", &"line\n".repeat(400));
    let moved = e.repo.commit("big address");
    findings(&e, &["naming-1", "doc-2"]);
    exit_none(
        &e.defer_raw(
            &two_minor().to_string(),
            &moved,
            &[("FAKE_GH_HEAD", &moved), ("FAKE_GH_BASE_OID", &up)],
        ),
        "re-measured-escalate",
    );
    let esc: Value = serde_json::from_str(&e.read("escalation.json")).unwrap();
    assert_eq!(esc["reason"], "in-loop-lines");
    assert_eq!(esc["base_sha"], up.as_str());
    assert!(!e.p("light-mode").exists());
    no_issue_created(&e);
}

#[test]
fn an_unreadable_or_invalid_base_falls_back_to_the_pinned_base_and_escalates() {
    if skip() {
        return;
    }
    let cases: Vec<(&str, Vec<(&str, String)>)> = vec![
        ("gh fails", vec![("FAKE_GH_FAIL", "pr view".into())]),
        ("no base oid", vec![]),
        (
            "base oid not hex",
            vec![("FAKE_GH_BASE_OID", "not-a-sha".into())],
        ),
        (
            "base oid unknown",
            vec![("FAKE_GH_BASE_OID", "ab".repeat(20))],
        ),
        (
            "base name starts with -",
            vec![("FAKE_GH_BASE_NAME", "-main".into())],
        ),
    ];
    for (name, extra) in cases {
        let e = Env::new();
        e.light();
        let up = upstream_commit(&e);
        let moved = merge_upstream(&e);
        findings(&e, &["naming-1", "doc-2"]);
        let mut env: Vec<(&str, &str)> = vec![("FAKE_GH_HEAD", &moved)];
        if name == "base name starts with -" {
            env.push(("FAKE_GH_BASE_OID", &up));
        }
        env.extend(extra.iter().map(|(k, v)| (*k, v.as_str())));
        let o = e.defer_raw(&two_minor().to_string(), &moved, &env);
        exit_none(&o, "re-measured-escalate");
        assert!(
            stderr(&o).contains("measuring from the pinned base"),
            "{name}: {}",
            show(&o)
        );
        let esc: Value = serde_json::from_str(&e.read("escalation.json")).unwrap();
        assert_eq!(esc["reason"], "in-loop-lines", "{name}");
        assert_eq!(esc["base_sha"], e.repo.base.as_str(), "{name}");
    }
}

#[test]
fn exit_79_while_reading_the_base_is_passed_through() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    e.repo.write("docs/guide.md", "moved\n");
    let moved = e.repo.commit("moved");
    let o = e.defer_raw(
        &two_minor().to_string(),
        &moved,
        &[
            ("FAKE_GH_HEAD", &moved),
            ("FAKE_GH_FAIL", "pr view"),
            ("FAKE_GH_EXIT_CODE", "79"),
        ],
    );
    assert_eq!(o.status.code(), Some(79), "{}", show(&o));
    assert!(e.p("light-mode").exists());
}

// --- argument checks -------------------------------------------------------

#[test]
fn bad_arguments_exit_2() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let rec: OsString = e.record.clone().into();
    let wt: OsString = e.repo.wt.clone().into();
    let h: OsString = e.head.clone().into();
    let base = |pr: &str, head: &OsString, record: &OsString| -> Vec<OsString> {
        vec![
            round_tool().into(),
            "defer".into(),
            "--record".into(),
            record.clone(),
            "--pr".into(),
            pr.into(),
            "--reviewed-head".into(),
            head.clone(),
            "--repo".into(),
            wt.clone(),
        ]
    };
    for args in [
        base("7x", &h, &rec),
        base("", &h, &rec),
        base("12345678901", &h, &rec),
        base("7", &"zz".into(), &rec),
        base("7", &"-x".into(), &rec),
        base("7", &h, &"relative/crusty-round-1.json".into()),
        base(
            "7",
            &h,
            &format!("{}/../x/crusty-round-1.json", e.state.display()).into(),
        ),
        vec![round_tool().into(), "nope".into()],
        vec![round_tool().into()],
    ] {
        let o = e.tool(&args, &[], &two_minor().to_string());
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", show(&o));
    }
    assert!(e.fakes.gh_calls().is_empty());
}

#[test]
fn concerns_that_are_not_a_json_array_take_the_normal_path() {
    if skip() {
        return;
    }
    for input in ["", "not json", "{\"id\":\"a\"}", "[1,2]"] {
        let e = Env::new();
        e.light();
        let o = e.defer_raw(input, &e.head.clone(), &[]);
        assert_eq!(o.status.code(), Some(0), "{input:?}: {}", show(&o));
        assert_ne!(result(&o)["light_exit"], "minor-deferred", "{input:?}");
        no_issue_created(&e);
    }
}

// ===========================================================================
// 2. step-03 and step-06 of autodrive-crusty-round.yaml
// ===========================================================================

/// A tools directory holding a copy of the real `autodrive_light_steps.sh`,
/// which the round recipe sources from `AUTODRIVE_LAUNCH_TOOLS`, and a logging
/// stand-in for `autodrive_light_round.sh`, which prints `$FAKE_ROUND_OUT` and exits
/// `$FAKE_ROUND_EXIT` (default 0). With `$FAKE_ROUND_TRUNCATE` set it empties
/// the record's `.findings`, as the real tool does on `minor-deferred`.
const FAKE_ROUND_TOOL: &str = r#"#!/usr/bin/env bash
( IFS=$'\x1f'; printf '%s\n' "$*" ) >> "${FAKE_LOG:?}/round.log"
cat > "$FAKE_LOG/round.stdin"
if [ -n "${FAKE_ROUND_TRUNCATE:-}" ]; then
  while [ $# -gt 0 ]; do [ "$1" = --record ] && : > "$2.findings"; shift; done
fi
printf '%s\n' "${FAKE_ROUND_OUT:-}"
exit "${FAKE_ROUND_EXIT:-0}"
"#;

fn fake_tools(e: &Env) -> PathBuf {
    let t = e.repo.tmp.path().join("fake-tools");
    std::fs::create_dir_all(&t).unwrap();
    std::fs::copy(
        tools_dir().join("autodrive_light_steps.sh"),
        t.join("autodrive_light_steps.sh"),
    )
    .unwrap();
    crate::support::exec_stub::write_executable(
        &t.join("autodrive_light_round.sh"),
        FAKE_ROUND_TOOL,
        0o755,
    );
    t
}

fn round_log(e: &Env) -> Vec<Vec<String>> {
    std::fs::read_to_string(e.fakes.log.join("round.log"))
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('\u{1f}').map(String::from).collect())
        .collect()
}

fn crusty_review(verdict: &str, concerns: &Value) -> String {
    format!(
        "Here is my review.\n\n{}\n",
        json!({"crusty_verdict": verdict, "concerns": concerns})
    )
}

fn round_context(e: &Env) -> String {
    json!({"pr": "7", "head_sha": e.head, "resolved_concerns": "", "round_label": "crusty-round-1"})
        .to_string()
}

fn run_step(e: &Env, id: &str, tools: &Path, env: &[(&str, &str)]) -> Output {
    let script = step_command(ROUND, id);
    let ctx = round_context(e);
    let root = repo_root();
    let mut v: Vec<(&str, &OsStr)> = vec![
        ("AUTODRIVE_ROUND_RECORD", e.record.as_os_str()),
        ("AUTODRIVE_ROUND_LABEL", OsStr::new("crusty-round-1")),
        ("AUTODRIVE_LAUNCH_TOOLS", tools.as_os_str()),
        ("RECIPE_VAR_crusty_round_context", OsStr::new(&ctx)),
        ("REPO_PATH", e.repo.wt.as_os_str()),
        ("RECIPE_VAR_repo_path", e.repo.wt.as_os_str()),
        ("RECIPE_VAR_pr_number", OsStr::new("7")),
        ("PR_NUMBER", OsStr::new("7")),
        ("TMPDIR", e.tmpdir.as_os_str()),
        ("FAKE_GH_HEAD", OsStr::new(&e.head)),
        ("AMPLIHACK_HOME", root.as_os_str()),
    ];
    v.extend(env.iter().map(|(k, val)| (*k, OsStr::new(*val))));
    let mut wd_script = format!("cd {:?} || exit 99\n", e.repo.wt.display().to_string());
    wd_script.push_str(&script);
    run_bash(&e.repo.home, &e.fakes, Some(&wd_script), &[], &v, "")
}

fn step03(e: &Env, tools: &Path, review: &str, env: &[(&str, &str)]) -> Output {
    let mut v = vec![("RECIPE_VAR_crusty_review", review)];
    v.extend_from_slice(env);
    run_step(e, STEP03, tools, &v)
}

fn step06(e: &Env, tools: &Path, verdict: &str, fix: &str) -> Output {
    run_step(
        e,
        STEP06,
        tools,
        &[
            ("RECIPE_VAR_crusty_verdict", verdict),
            ("RECIPE_VAR_crusty_fix_evidence", fix),
        ],
    )
}

fn state_names(e: &Env) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(&e.state)
        .unwrap()
        .map(|d| d.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

// --- golden: no marker, output as before #1802 ------------------------------

#[test]
fn golden_step03_without_marker_is_unchanged_and_never_calls_the_light_tool() {
    if skip() {
        return;
    }
    let e = Env::new();
    let tools = fake_tools(&e);
    let concerns = json!([
        {"id": "naming-1", "severity": "minor", "summary": "x"},
        {"id": "real-bug", "severity": "major", "summary": "y"}
    ]);
    let o = step03(&e, &tools, &crusty_review("CONCERNS", &concerns), &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(
        stdout(&o),
        "{\"crusty_verdict\":\"CONCERNS\",\"verdict_source\":\"crusty\",\"concern_count\":2}\n"
    );
    assert_eq!(
        e.read("crusty-round-1.json.findings"),
        "naming-1\nreal-bug\n"
    );
    assert_eq!(state_names(&e), ["crusty-round-1.json.findings"]);
    assert!(
        round_log(&e).is_empty(),
        "the light tool must not run without a marker"
    );
    assert!(e.fakes.gh_calls().is_empty());

    // All-minor without a marker is still CONCERNS.
    let e = Env::new();
    let tools = fake_tools(&e);
    let o = step03(&e, &tools, &crusty_review("CONCERNS", &two_minor()), &[]);
    assert_eq!(
        stdout(&o),
        "{\"crusty_verdict\":\"CONCERNS\",\"verdict_source\":\"crusty\",\"concern_count\":2}\n"
    );
    assert!(round_log(&e).is_empty());

    // CLEAN and missing verdicts.
    let e = Env::new();
    let tools = fake_tools(&e);
    let o = step03(&e, &tools, &crusty_review("CLEAN", &json!([])), &[]);
    assert_eq!(
        stdout(&o),
        "{\"crusty_verdict\":\"CLEAN\",\"verdict_source\":\"crusty\",\"concern_count\":0}\n"
    );
    let o = step03(&e, &tools, "", &[]);
    assert_eq!(
        stdout(&o),
        "{\"crusty_verdict\":\"CONCERNS\",\"verdict_source\":\"missing_verdict\",\"concern_count\":0}\n"
    );
    assert!(round_log(&e).is_empty());
}

#[test]
fn golden_step06_without_light_source_writes_the_same_bytes() {
    if skip() {
        return;
    }
    let e = Env::new();
    let tools = fake_tools(&e);
    findings(&e, &["naming-1", "real-bug"]);
    let post = "a".repeat(40);
    let fix = json!({"base_sha": e.head, "head_sha": post, "commits": 1, "dirty_worktree": "false", "git_observed": "true"}).to_string();
    let o = step06(
        &e,
        &tools,
        r#"{"crusty_verdict":"CONCERNS","verdict_source":"crusty","concern_count":2}"#,
        &fix,
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let want = format!(
        "{{\"crusty_verdict\":\"CONCERNS\",\"concern_count\":2,\"commits_this_round\":1,\"head_sha\":\"{post}\",\"reviewed_head_sha\":\"{}\",\"round_label\":\"crusty-round-1\",\"test_signal\":\"\",\"ci_signal\":\"\"}}\n",
        e.head
    );
    assert_eq!(std::fs::read_to_string(&e.record).unwrap(), want);
    assert_eq!(stdout(&o), want);
    assert_eq!(mode(&e.record), 0o600);
    assert_eq!(
        state_names(&e),
        ["crusty-round-1.json", "crusty-round-1.json.findings"]
    );

    let e = Env::new();
    let o = step06(
        &e,
        &tools,
        r#"{"crusty_verdict":"CLEAN","verdict_source":"crusty","concern_count":0}"#,
        "",
    );
    let want = format!(
        "{{\"crusty_verdict\":\"CLEAN\",\"concern_count\":0,\"commits_this_round\":0,\"head_sha\":\"{h}\",\"reviewed_head_sha\":\"{h}\",\"round_label\":\"crusty-round-1\",\"test_signal\":\"\",\"ci_signal\":\"\"}}\n",
        h = e.head
    );
    assert_eq!(
        std::fs::read_to_string(&e.record).unwrap(),
        want,
        "{}",
        show(&o)
    );
    assert_eq!(
        state_names(&e),
        ["crusty-round-1.json", "crusty-round-1.json.findings"]
    );
}

// --- step-03 with a marker (stand-in tool) ----------------------------------

const DEFERRED_OUT: &str = r#"{"light_exit":"minor-deferred","follow_up_issue":"1851","deferred_ids":["naming-1","doc-2"],"concern_count":2}"#;

#[test]
fn step03_turns_a_minor_deferred_result_into_a_light_clean_verdict() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let tools = fake_tools(&e);
    let o = step03(
        &e,
        &tools,
        &crusty_review("CONCERNS", &two_minor()),
        &[
            ("FAKE_ROUND_OUT", DEFERRED_OUT),
            ("FAKE_ROUND_TRUNCATE", "1"),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(
        stdout(&o),
        "{\"crusty_verdict\":\"CLEAN\",\"verdict_source\":\"light-minor-rule\",\"concern_count\":2}\n"
    );
    let log = round_log(&e);
    assert_eq!(log.len(), 1, "{log:?}");
    let argv = &log[0];
    assert_eq!(argv[0], "defer");
    let arg = |flag: &str| {
        let i = argv
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} in {argv:?}"));
        argv[i + 1].clone()
    };
    assert_eq!(arg("--record"), e.record.to_string_lossy());
    assert_eq!(arg("--pr"), "7");
    assert_eq!(arg("--reviewed-head"), e.head);
    assert_eq!(
        std::fs::canonicalize(arg("--repo")).unwrap(),
        std::fs::canonicalize(&e.repo.wt).unwrap()
    );
    let seen: Value =
        serde_json::from_str(&std::fs::read_to_string(e.fakes.log.join("round.stdin")).unwrap())
            .expect("concerns reach the tool as JSON on stdin");
    assert_eq!(seen, two_minor());
}

#[test]
fn step03_keeps_the_crusty_verdict_when_the_rule_does_not_apply() {
    if skip() {
        return;
    }
    for out in [
        r#"{"light_exit":"none","reason":"not-minor-only"}"#,
        r#"{"light_exit":"none","reason":"follow-up-failed"}"#,
        r#"{"light_exit":"off"}"#,
        "garbage",
        "",
    ] {
        let e = Env::new();
        e.light();
        let tools = fake_tools(&e);
        let o = step03(
            &e,
            &tools,
            &crusty_review("CONCERNS", &two_minor()),
            &[("FAKE_ROUND_OUT", out)],
        );
        assert_eq!(o.status.code(), Some(0), "{out}: {}", show(&o));
        assert_eq!(
            stdout(&o),
            "{\"crusty_verdict\":\"CONCERNS\",\"verdict_source\":\"crusty\",\"concern_count\":2}\n",
            "{out}"
        );
        assert_eq!(e.read("crusty-round-1.json.findings"), "naming-1\ndoc-2\n");
    }
}

#[test]
fn step03_fails_safe_when_the_tool_fails() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let tools = fake_tools(&e);
    let o = step03(
        &e,
        &tools,
        &crusty_review("CONCERNS", &two_minor()),
        &[("FAKE_ROUND_OUT", DEFERRED_OUT), ("FAKE_ROUND_EXIT", "1")],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(
        stdout(&o).starts_with("{\"crusty_verdict\":\"CONCERNS\","),
        "a failed tool never yields CLEAN: {}",
        show(&o)
    );
}

#[test]
fn step03_passes_exit_79_through() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let tools = fake_tools(&e);
    let o = step03(
        &e,
        &tools,
        &crusty_review("CONCERNS", &two_minor()),
        &[("FAKE_ROUND_EXIT", "79")],
    );
    assert_eq!(o.status.code(), Some(79), "{}", show(&o));
}

#[test]
fn step03_ignores_a_marker_that_is_not_a_regular_file() {
    if skip() {
        return;
    }
    let e = Env::new();
    let tools = fake_tools(&e);
    let real = e.repo.tmp.path().join("elsewhere");
    std::fs::write(&real, format!("{}\n", e.head)).unwrap();
    std::os::unix::fs::symlink(&real, e.p("light-mode")).unwrap();
    let o = step03(
        &e,
        &tools,
        &crusty_review("CONCERNS", &two_minor()),
        &[("FAKE_ROUND_OUT", DEFERRED_OUT)],
    );
    assert!(
        stdout(&o).starts_with("{\"crusty_verdict\":\"CONCERNS\","),
        "{}",
        show(&o)
    );
    assert!(
        round_log(&e).is_empty(),
        "a symlinked marker is not light mode"
    );

    let e = Env::new();
    let tools = fake_tools(&e);
    std::fs::create_dir(e.p("light-mode")).unwrap();
    let o = step03(
        &e,
        &tools,
        &crusty_review("CONCERNS", &two_minor()),
        &[("FAKE_ROUND_OUT", DEFERRED_OUT)],
    );
    assert!(
        stdout(&o).starts_with("{\"crusty_verdict\":\"CONCERNS\","),
        "{}",
        show(&o)
    );
    assert!(round_log(&e).is_empty());
}

#[test]
fn step03_leaves_a_crusty_clean_verdict_alone_in_light_mode() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let tools = fake_tools(&e);
    let o = step03(
        &e,
        &tools,
        &crusty_review("CLEAN", &json!([])),
        &[("FAKE_ROUND_OUT", DEFERRED_OUT)],
    );
    assert_eq!(
        stdout(&o),
        "{\"crusty_verdict\":\"CLEAN\",\"verdict_source\":\"crusty\",\"concern_count\":0}\n",
        "{}",
        show(&o)
    );
}

// --- step-06 with the light source ------------------------------------------

const LIGHT_VERDICT: &str =
    r#"{"crusty_verdict":"CLEAN","verdict_source":"light-minor-rule","concern_count":2}"#;

fn light_side_files(e: &Env) {
    private_file(&e.rec(".findings"), "");
    private_file(
        &e.rec(".concerns.tsv"),
        "naming-1\tminor\tRename tmp_rec\ndoc-2\tminor\tHeader comment is stale\n",
    );
    private_file(
        &e.rec(".light.json"),
        "{\"follow_up_issue\":\"1851\",\"deferred_ids\":[\"naming-1\",\"doc-2\"]}\n",
    );
}

fn light_record(head: &str) -> String {
    format!(
        "{{\"crusty_verdict\":\"CLEAN\",\"concern_count\":2,\"commits_this_round\":0,\"head_sha\":\"{head}\",\"reviewed_head_sha\":\"{head}\",\"round_label\":\"crusty-round-1\",\"test_signal\":\"\",\"ci_signal\":\"\",\"verdict_source\":\"light-minor-rule\",\"follow_up_issue\":\"1851\",\"deferred_ids\":[\"naming-1\",\"doc-2\"]}}\n"
    )
}

#[test]
fn step06_writes_the_light_clean_record() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    light_side_files(&e);
    let tools = fake_tools(&e);
    let o = step06(&e, &tools, LIGHT_VERDICT, "");
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let rec = std::fs::read_to_string(&e.record).unwrap();
    assert_eq!(rec, light_record(&e.head));
    assert!(rec.starts_with("{\"crusty_verdict\":\"CLEAN\","));
    assert_eq!(rec.matches("\"reviewed_head_sha\":").count(), 1);
    assert_eq!(rec.lines().count(), 1);
    assert_eq!(mode(&e.record), 0o600);
}

#[test]
fn step06_writes_concerns_when_the_side_file_is_invalid() {
    if skip() {
        return;
    }
    type Spoil = fn(&Env);
    let cases: Vec<(&str, Spoil)> = vec![
        ("missing side file", |e| {
            std::fs::remove_file(e.rec(".light.json")).unwrap()
        }),
        ("extra key", |e| {
            private_file(
                &e.rec(".light.json"),
                "{\"follow_up_issue\":\"1851\",\"deferred_ids\":[\"naming-1\",\"doc-2\"],\"crusty_verdict\":\"CLEAN\"}\n",
            )
        }),
        ("issue not digits", |e| {
            private_file(
                &e.rec(".light.json"),
                "{\"follow_up_issue\":\"18\\\"51\",\"deferred_ids\":[\"naming-1\",\"doc-2\"]}\n",
            )
        }),
        ("issue too long", |e| {
            private_file(
                &e.rec(".light.json"),
                "{\"follow_up_issue\":\"12345678901\",\"deferred_ids\":[\"naming-1\",\"doc-2\"]}\n",
            )
        }),
        ("id not in tsv", |e| {
            private_file(
                &e.rec(".light.json"),
                "{\"follow_up_issue\":\"1851\",\"deferred_ids\":[\"naming-1\",\"other-9\"]}\n",
            )
        }),
        ("tsv has a major row", |e| {
            private_file(
                &e.rec(".concerns.tsv"),
                "naming-1\tminor\tx\ndoc-2\tminor\ty\nreal-bug\tmajor\tz\n",
            )
        }),
        ("findings not empty", |e| {
            private_file(&e.rec(".findings"), "naming-1\n")
        }),
        ("side file loose mode", |e| {
            std::fs::set_permissions(e.rec(".light.json"), std::fs::Permissions::from_mode(0o666))
                .unwrap()
        }),
        ("side file symlink", |e| {
            let real = e.repo.tmp.path().join("side.json");
            std::fs::write(
                &real,
                "{\"follow_up_issue\":\"1851\",\"deferred_ids\":[\"naming-1\",\"doc-2\"]}\n",
            )
            .unwrap();
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::remove_file(e.rec(".light.json")).unwrap();
            std::os::unix::fs::symlink(&real, e.rec(".light.json")).unwrap();
        }),
    ];
    for (name, spoil) in cases {
        let e = Env::new();
        e.light();
        light_side_files(&e);
        spoil(&e);
        let tools = fake_tools(&e);
        let o = step06(&e, &tools, LIGHT_VERDICT, "");
        assert_eq!(o.status.code(), Some(0), "{name}: {}", show(&o));
        let rec = std::fs::read_to_string(&e.record).unwrap();
        assert!(
            rec.starts_with("{\"crusty_verdict\":\"CONCERNS\","),
            "{name}: {rec}"
        );
        assert!(
            !rec.contains("verdict_source"),
            "{name}: no light suffix: {rec}"
        );
        assert!(
            stderr(&o).contains("WARNING: light-side-file-invalid"),
            "{name}: {}",
            show(&o)
        );
    }
}

#[test]
fn step03_loads_the_helper_from_the_launch_tools_only() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let tools = fake_tools(&e);
    std::fs::remove_file(tools.join("autodrive_light_steps.sh")).unwrap();
    // AMPLIHACK_HOME (the repository root) still holds the helper.
    let o = step03(
        &e,
        &tools,
        &crusty_review("CONCERNS", &two_minor()),
        &[("FAKE_ROUND_OUT", DEFERRED_OUT)],
    );
    assert!(
        stdout(&o).starts_with("{\"crusty_verdict\":\"CONCERNS\","),
        "{}",
        show(&o)
    );
    assert!(
        stderr(&o).contains("autodrive_light_steps.sh not found under AUTODRIVE_LAUNCH_TOOLS"),
        "{}",
        show(&o)
    );
    assert!(round_log(&e).is_empty(), "the round tool is not called");
}

#[test]
fn step06_rejects_a_verdict_that_names_the_rule_but_has_another_source() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    light_side_files(&e);
    let tools = fake_tools(&e);
    let verdict = r#"{"crusty_verdict":"CLEAN","verdict_source":"crusty","note":"light-minor-rule","concern_count":0}"#;
    let o = step06(&e, &tools, verdict, "");
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let rec = std::fs::read_to_string(&e.record).unwrap();
    assert!(
        rec.starts_with("{\"crusty_verdict\":\"CONCERNS\","),
        "{rec}"
    );
    assert!(!rec.contains("deferred_ids"), "{rec}");
    assert!(
        stderr(&o).contains("WARNING: light-side-file-invalid"),
        "{}",
        show(&o)
    );

    // The function itself: non-zero, with an empty SUFFIX.
    let helper = tools_dir().join("autodrive_light_steps.sh");
    let script = format!(
        ". {:?}; VERDICT=CLEAN; RECORD={:?}; SUFFIX=x; autodrive_light_step06 \"$1\"; printf 'rc=%s suffix=[%s]' \"$?\" \"$SUFFIX\"",
        helper.display().to_string(),
        e.record.display().to_string()
    );
    let argv = [
        OsStr::new("-c"),
        OsStr::new(&script),
        OsStr::new("x"),
        OsStr::new(verdict),
    ];
    let o = run_bash(&e.repo.home, &e.fakes, None, &argv, &[], "");
    assert_eq!(stdout(&o), "rc=1 suffix=[]", "{}", show(&o));
}

// ===========================================================================
// 3. Acceptance test (a): light CLEAN record reaches the merge phase
// ===========================================================================

/// Writes the loop's manifest row and `crusty-latest.json` for the record,
/// as `autodrive_loop.sh` does, then runs `autodrive_record_crusty_loop_done`
/// (the DONE path) and `autodrive_crusty_final`.
fn loop_done_and_final(e: &Env) -> Output {
    let hash = git(
        &e.state,
        &["hash-object", "--no-filters", e.record.to_str().unwrap()],
    );
    private_file(
        &e.p("crusty-records.tsv"),
        &format!("crusty-round-1\tcrusty-round-1.json\t{hash}\n"),
    );
    std::fs::copy(&e.record, e.p("crusty-latest.json")).unwrap();
    std::fs::set_permissions(
        e.p("crusty-latest.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let helper = tools_dir().join("autodrive_state.sh");
    let script = ". \"$1\" || exit 98\nautodrive_record_crusty_loop_done \"$2\"\nautodrive_crusty_final \"$2\"\n";
    run_bash(
        &e.repo.home,
        &e.fakes,
        Some(script),
        &[helper.as_os_str(), e.state.as_os_str()],
        &[("TMPDIR", e.tmpdir.as_os_str())],
        "",
    )
}

#[test]
fn acceptance_a_minor_only_round_reaches_the_merge_phase() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    // The real tool, loaded from the tool root.
    let tools = tools_dir();
    let o = step03(&e, &tools, &crusty_review("CONCERNS", &two_minor()), &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(
        stdout(&o),
        "{\"crusty_verdict\":\"CLEAN\",\"verdict_source\":\"light-minor-rule\",\"concern_count\":2}\n"
    );
    let verdict = stdout(&o).trim().to_string();
    let o6 = step06(&e, &tools, &verdict, "");
    assert_eq!(o6.status.code(), Some(0), "{}", show(&o6));
    assert_eq!(
        std::fs::read_to_string(&e.record).unwrap(),
        light_record(&e.head)
    );
    assert_eq!(e.read("follow-up-issue").trim(), "1851");

    let f = loop_done_and_final(&e);
    assert_eq!(f.status.code(), Some(0), "{}", show(&f));
    assert_eq!(
        stdout(&f).trim(),
        e.head,
        "autodrive_crusty_final accepts the record"
    );
    let phases = e.read("phases.tsv");
    assert!(
        phases.lines().any(|l| l.starts_with("crusty-loop\t")),
        "{phases}"
    );
    let resolved = e.read("resolved-concerns.txt");
    assert!(
        !resolved.contains("naming-1") && !resolved.contains("doc-2"),
        "deferred ids are not recorded as resolved: {resolved}"
    );
}

#[test]
fn acceptance_b_a_major_concern_runs_the_normal_address_path() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let concerns = json!([
        minor("naming-1", "Rename"),
        {"id": "real-bug", "severity": "major", "summary": "Wrong result", "evidence": "x"}
    ]);
    let o = step03(&e, &tools_dir(), &crusty_review("CONCERNS", &concerns), &[]);
    assert_eq!(
        stdout(&o),
        "{\"crusty_verdict\":\"CONCERNS\",\"verdict_source\":\"crusty\",\"concern_count\":2}\n",
        "{}",
        show(&o)
    );
    assert_eq!(
        e.read("crusty-round-1.json.findings"),
        "naming-1\nreal-bug\n"
    );
    no_issue_created(&e);
}
