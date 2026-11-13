//! Issue #2538: only an exact boolean `false` makes a pull request a
//! same-repository PR.
//!
//! `light-eligibility` in `auto-drive-light.yaml` reads
//! `gh pr view <PR> --json isCrossRepository` itself and stops with
//! `light-cross-repo` unless jq finds `.isCrossRepository == false`. The
//! refusal is a hard stop, never an `escalate` decision, and it comes before
//! any cached `light-eligibility.json` or `escalation.json` is reused.
//! `autodrive_open_pr_for_run_branch` (`autodrive_run_pr.sh`) applies the same
//! rule to the PRs it lists.
//!
//! Each reader is run for real, with a fake `gh` first on `PATH`, over the
//! same table: `true`, a missing field, `null`, the string `"false"`, empty
//! output and invalid JSON are refused; `false` is accepted.
//!
//! No agent, real `gh`, network or real `~/.amplihack` is used. Every test
//! skips, with a note, when `jq` is not installed.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Output;

use serde_json::{Value, json};

use crate::support::auto_drive_light::{
    Fakes, LightRepo, fake_bin, git, jq_available, private_dir, private_file, repo_root, run_bash,
    show, stderr, stdout, step_command, tools_dir,
};

const RECIPE: &str = "auto-drive-light";
const STEP: &str = "light-eligibility";

/// `gh pr view --json isCrossRepository` answers that must be refused.
const REFUSED_VIEW: &[(&str, &str)] = &[
    ("true", r#"{"isCrossRepository":true}"#),
    ("missing field", r#"{}"#),
    ("null", r#"{"isCrossRepository":null}"#),
    ("string \"false\"", r#"{"isCrossRepository":"false"}"#),
    ("empty output", ""),
    ("invalid JSON", "{this is not json"),
];

const ACCEPTED_VIEW: &str = r#"{"isCrossRepository":false}"#;

fn skip() -> bool {
    if !jq_available() {
        eprintln!("skip: `jq` not available on PATH");
        return true;
    }
    false
}

// --- (a) light-eligibility ---------------------------------------------------

/// A repository with one small commit after the base, a private state
/// directory, a private TMPDIR and the fakes.
struct Env {
    repo: LightRepo,
    state: PathBuf,
    tmpdir: PathBuf,
    fakes: Fakes,
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
        Env {
            repo,
            state,
            tmpdir,
            fakes,
            head,
        }
    }

    fn build_result(&self) -> String {
        json!({
            "pr": "7",
            "head_sha": self.head,
            "base_sha": self.repo.base,
            "state_dir": self.state.to_string_lossy(),
            "repo": self.repo.wt.to_string_lossy(),
        })
        .to_string()
    }

    /// A valid cached `light` decision for this head, as an earlier run
    /// would have left it.
    fn cached_light(&self) -> String {
        let elig = json!({
            "decision": "light", "reason": "within-limits",
            "base": self.repo.base, "head": self.head,
            "lines": 2, "new_modules": 0, "public_api": 0, "schema": 0, "recipe_contract": 0,
            "limits": {"lines": 300, "new_modules": 0, "public_api": 0, "schema": 0, "recipe_contract": 0}
        });
        let body = format!("{elig}\n");
        private_file(&self.state.join("light-eligibility.json"), &body);
        body
    }

    fn file(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.state.join(name)).ok()
    }

    /// Runs the live `light-eligibility` step. `view` is the raw `gh pr view`
    /// answer (`None` keeps the fake's default PR, which has `false`).
    fn run(&self, view: Option<&str>, env: &[(&str, &str)]) -> Output {
        let script = step_command(RECIPE, STEP);
        let root = repo_root();
        let br = self.build_result();
        let mut v: Vec<(&str, &OsStr)> = vec![
            ("AMPLIHACK_HOME", root.as_os_str()),
            ("RECIPE_VAR_build_result", OsStr::new(&br)),
            ("RECIPE_VAR_repo_path", self.repo.wt.as_os_str()),
            ("RECIPE_VAR_light_max_lines", OsStr::new("300")),
            ("RECIPE_VAR_light_max_new_modules", OsStr::new("0")),
            ("RECIPE_VAR_light_max_public_api", OsStr::new("0")),
            ("RECIPE_VAR_light_max_schema", OsStr::new("0")),
            ("RECIPE_VAR_light_max_recipe_contract", OsStr::new("0")),
            ("TMPDIR", self.tmpdir.as_os_str()),
            ("FAKE_GH_HEAD", OsStr::new(&self.head)),
        ];
        if let Some(raw) = view {
            v.push(("FAKE_GH_PR_VIEW", OsStr::new(raw)));
        }
        v.extend(env.iter().map(|(k, val)| (*k, OsStr::new(*val))));
        let mut wd_script = format!("cd {:?} || exit 99\n", self.repo.wt.display().to_string());
        wd_script.push_str(&script);
        run_bash(&self.repo.home, &self.fakes, Some(&wd_script), &[], &v, "")
    }

    /// The `gh pr view` calls that asked for `isCrossRepository`.
    fn cross_repo_reads(&self) -> usize {
        self.fakes
            .gh_calls()
            .iter()
            .filter(|c| {
                c.first().map(String::as_str) == Some("pr")
                    && c.get(1).map(String::as_str) == Some("view")
                    && c.iter().any(|a| a.contains("isCrossRepository"))
            })
            .count()
    }
}

/// Every `gh` call the step made is a read: `pr view` only.
fn assert_read_only(e: &Env, why: &str) {
    for c in e.fakes.gh_calls() {
        assert!(
            c.first().map(String::as_str) == Some("pr")
                && c.get(1).map(String::as_str) == Some("view"),
            "{why}: light-eligibility may only read the PR, but called gh {c:?}"
        );
    }
}

/// The refusal: a non-zero exit naming `light-cross-repo`, no measurement,
/// no decision on stdout and no state written.
fn assert_refused(e: &Env, o: &Output, why: &str) {
    assert_ne!(
        o.status.code(),
        Some(0),
        "{why}: must be refused\n{}",
        show(o)
    );
    assert!(
        stderr(o).contains("light-cross-repo"),
        "{why}: stderr must name light-cross-repo\n{}",
        show(o)
    );
    assert!(
        !stderr(o).contains("AUTODRIVE_LIGHT_ELIGIBILITY="),
        "{why}: no eligibility decision may be reported\n{}",
        show(o)
    );
    assert!(
        stdout(o).trim().is_empty(),
        "{why}: no decision may reach stdout, so nothing is escalated\n{}",
        show(o)
    );
    assert!(
        e.file("escalation.json").is_none(),
        "{why}: a refusal is never an escalation\n{}",
        show(o)
    );
    assert_read_only(e, why);
}

#[test]
fn light_eligibility_refuses_every_value_but_exact_false() {
    if skip() {
        return;
    }
    for (label, raw) in REFUSED_VIEW {
        let e = Env::new();
        let o = e.run(Some(raw), &[]);
        let why = format!("isCrossRepository {label}");
        assert_refused(&e, &o, &why);
        assert!(
            e.file("light-eligibility.json").is_none(),
            "{why}: light-eligibility.json must not be written\n{}",
            show(&o)
        );
        assert_eq!(
            e.cross_repo_reads(),
            1,
            "{why}: the step must read isCrossRepository with gh pr view exactly once\n{}",
            show(&o)
        );
    }
}

#[test]
fn light_eligibility_refuses_when_gh_pr_view_fails() {
    if skip() {
        return;
    }
    let e = Env::new();
    let o = e.run(None, &[("FAKE_GH_FAIL", "pr view")]);
    assert_refused(&e, &o, "gh pr view fails");
    assert!(e.file("light-eligibility.json").is_none(), "{}", show(&o));
    assert!(
        !stderr(&o).contains("HTTP 404"),
        "gh stderr is untrusted and is not echoed\n{}",
        show(&o)
    );
}

#[test]
fn light_eligibility_refuses_before_reusing_a_cached_decision() {
    if skip() {
        return;
    }
    for (label, raw) in REFUSED_VIEW {
        let e = Env::new();
        let cached = e.cached_light();
        let o = e.run(Some(raw), &[]);
        let why = format!("cached light decision, isCrossRepository {label}");
        assert_refused(&e, &o, &why);
        assert!(
            !stderr(&o).contains("reusing the eligibility decision"),
            "{why}: the cache must not be consulted before the check\n{}",
            show(&o)
        );
        assert_eq!(
            e.file("light-eligibility.json").as_deref(),
            Some(cached.as_str()),
            "{why}: the cached file is left untouched\n{}",
            show(&o)
        );
    }
}

#[test]
fn light_eligibility_refuses_a_cached_escalation_too() {
    if skip() {
        return;
    }
    // A previous escalation must not turn a fork PR into an escalate
    // decision that hands it to auto-drive-to-merge.
    let e = Env::new();
    private_file(
        &e.state.join("escalation.json"),
        &format!(
            "{}\n",
            json!({"reason": "over-limit", "base_sha": e.repo.base, "head_sha": e.head})
        ),
    );
    let o = e.run(Some(r#"{"isCrossRepository":true}"#), &[]);
    assert_ne!(o.status.code(), Some(0), "{}", show(&o));
    assert!(stderr(&o).contains("light-cross-repo"), "{}", show(&o));
    assert!(stdout(&o).trim().is_empty(), "{}", show(&o));
    assert!(e.file("light-eligibility.json").is_none(), "{}", show(&o));
}

#[test]
fn light_eligibility_accepts_exact_false_and_measures() {
    if skip() {
        return;
    }
    let e = Env::new();
    let o = e.run(Some(ACCEPTED_VIEW), &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(
        stderr(&o).contains("AUTODRIVE_LIGHT_ELIGIBILITY="),
        "the measurement must run\n{}",
        show(&o)
    );
    assert!(!stderr(&o).contains("light-cross-repo"), "{}", show(&o));
    let j: Value = serde_json::from_str(stdout(&o).trim())
        .unwrap_or_else(|err| panic!("stdout must be the decision JSON ({err})\n{}", show(&o)));
    assert!(
        j["decision"] == "light" || j["decision"] == "escalate",
        "decision: {j}"
    );
    assert_eq!(
        e.cross_repo_reads(),
        1,
        "the step must read isCrossRepository with gh pr view\n{}",
        show(&o)
    );
    assert_read_only(&e, "accepted");
    let view = e
        .fakes
        .gh_calls()
        .into_iter()
        .find(|c| c.iter().any(|a| a.contains("isCrossRepository")))
        .expect("pr view call");
    assert!(
        view.iter().any(|a| a == "7"),
        "the PR read must be the build_result PR: {view:?}"
    );
    assert!(
        !view
            .iter()
            .any(|a| a == "--jq" || a == "-q" || a.starts_with("--jq=")),
        "the value is compared by jq on the typed JSON, not through gh --jq: {view:?}"
    );
}

#[test]
fn light_eligibility_reads_the_pr_in_the_target_repository_not_gh_repo() {
    if skip() {
        return;
    }
    let e = Env::new();
    let o = e.run(Some(ACCEPTED_VIEW), &[("GH_REPO", "other/repo")]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let calls = e.fakes.gh_calls();
    let seen = e.fakes.gh_repo_seen();
    let idx = calls
        .iter()
        .position(|c| c.iter().any(|a| a.contains("isCrossRepository")))
        .unwrap_or_else(|| panic!("no isCrossRepository read\n{}", show(&o)));
    assert_eq!(
        seen.get(idx).map(String::as_str),
        Some("<unset>"),
        "GH_REPO must not choose the repository the PR number is looked up in"
    );
}

// --- (b) autodrive_run_pr.sh ---------------------------------------------------

const RUN_BRANCH: &str = "feat/run-2538";

/// A `gh` that answers `pr list` with `$FAKE_GH_PR_LIST` verbatim.
const LIST_GH: &str = r#"#!/usr/bin/env bash
( IFS=$'\x1f'; printf '%s\n' "$*" ) >> "${FAKE_LOG:?}/gh.log"
case "${1:-} ${2:-}" in
  "pr list") printf '%s' "${FAKE_GH_PR_LIST-[]}" ;;
esac
exit 0
"#;

fn run_pr_list(entry: Option<Value>, raw: Option<&str>) -> (Output, Fakes, LightRepo) {
    let repo = LightRepo::new();
    git(&repo.wt, &["checkout", "-q", "-b", RUN_BRANCH]);
    repo.write("docs/guide.md", "# guide\n\nrun work\n");
    let head = repo.commit("run work");
    let fakes = fake_bin(repo.tmp.path());
    crate::support::exec_stub::write_executable(&fakes.bin.join("gh"), LIST_GH, 0o755);
    let list = match (entry, raw) {
        (_, Some(r)) => r.to_string(),
        (Some(mut obj), None) => {
            obj["number"] = json!(7);
            obj["headRefName"] = json!(RUN_BRANCH);
            obj["headRefOid"] = json!(head);
            json!([obj]).to_string()
        }
        (None, None) => "[]".to_string(),
    };
    let tool = tools_dir().join("autodrive_run_pr.sh");
    let script = r#". "$1" && autodrive_open_pr_for_run_branch "$2" "$3""#;
    let o = run_bash(
        &repo.home,
        &fakes,
        Some(script),
        &[
            tool.as_os_str(),
            repo.wt.as_os_str(),
            OsStr::new(RUN_BRANCH),
        ],
        &[("FAKE_GH_PR_LIST", OsStr::new(&list))],
        "",
    );
    (o, fakes, repo)
}

#[test]
fn run_pr_lookup_refuses_every_value_but_exact_false() {
    if skip() {
        return;
    }
    let cases: Vec<(&str, Option<Value>, Option<&str>)> = vec![
        ("true", Some(json!({"isCrossRepository": true})), None),
        ("missing field", Some(json!({})), None),
        ("null", Some(json!({"isCrossRepository": null})), None),
        (
            "string \"false\"",
            Some(json!({"isCrossRepository": "false"})),
            None,
        ),
        ("empty output", None, Some("")),
        ("invalid JSON", None, Some("[{this is not json")),
    ];
    for (label, entry, raw) in cases {
        let (o, _fakes, _repo) = run_pr_list(entry, raw);
        assert_eq!(
            o.status.code(),
            Some(0),
            "{label}: never fails\n{}",
            show(&o)
        );
        assert!(
            stdout(&o).trim().is_empty(),
            "isCrossRepository {label}: no PR may be reported as the run's PR\n{}",
            show(&o)
        );
    }
}

#[test]
fn run_pr_lookup_accepts_exact_false() {
    if skip() {
        return;
    }
    let (o, fakes, _repo) = run_pr_list(Some(json!({"isCrossRepository": false})), None);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(stdout(&o).trim(), "7", "{}", show(&o));
    assert!(
        fakes
            .gh_calls()
            .iter()
            .any(|c| c.first().map(String::as_str) == Some("pr")
                && c.get(1).map(String::as_str) == Some("list")),
        "the lookup must ask gh"
    );
}
