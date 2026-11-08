//! Issues #1802 and #1982: the light crusty round.
//!
//! Since #1982 light mode has no minor-finding rule of its own. The light-mode
//! marker only makes step-03 measure a moved head again; the verdict always
//! comes from the #1979 `classify` branch, and follow-ups are filed by
//! `autodrive_followups.sh collect` and `file`, as in auto-drive-to-merge.
//!
//! Four layers are tested:
//!
//! 1. `amplifier-bundle/tools/autodrive_light_round.sh check`, run directly:
//!
//!    ```text
//!    autodrive_light_round.sh check --record <record> --pr <N>
//!        --reviewed-head <sha> --repo <dir>        (no stdin)
//!    ```
//!
//!    The state directory is `dirname <record>`. The tool prints exactly one
//!    JSON line, `{"light_exit":"off|light|escalated"}` with an optional
//!    `reason`, exits 0 on any decision, exits 2 with empty stdout on bad
//!    arguments (the retired `defer` included), and passes exit 79 through.
//! 2. Step-03 and step-06 of `autodrive-crusty-round.yaml`, extracted from
//!    the live YAML and run in bash with a stand-in round tool. Without the
//!    `light-mode` marker their output is byte-identical to the output before
//!    #1802 (the golden tests were captured from the unmodified recipe).
//!    With the marker, step-03 calls `check` and never takes a verdict from
//!    it; step-06 has no light branch.
//! 3. The real tools (`autodrive_followups.sh`, `autodrive_followups_check.sh`,
//!    `autodrive_finding_rules.sh`, `autodrive_state.sh`,
//!    `workflow_issue_tracking.sh` and the light tools) copied into one tools
//!    directory, with a sentinel `gh` that exits 97 wherever step-03 must not
//!    call it: classification, the record, collect, file, the gate, both
//!    recurrence cases, the keyword predicate, escalation, and the absence
//!    of every retired light state file.
//! 4. Acceptance: a light `MINOR_ONLY` record is accepted by
//!    `autodrive_crusty_final` through the manifest.
//!
//! Every test skips, with a note, when `jq` is not installed.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};

use crate::support::auto_drive_light::{
    Fakes, LightRepo, called, fake_bin, git, jq_available, private_dir, private_file, repo_root,
    round_tool, run_bash, show, stderr, stdout, step_command, tools_dir,
};

const ROUND: &str = "autodrive-crusty-round";
const STEP03: &str = "step-03-extract-crusty-verdict";
const STEP06: &str = "step-06-write-round-record";

/// State files of the retired single-issue model; nothing may write them.
const RETIRED: &[&str] = &[
    "follow-up-issue",
    "follow-up-posted-ids",
    "follow-up-linked",
    "follow-up-failed",
];

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

    /// `check` at the commit the marker was written for.
    fn check(&self, env: &[(&str, &str)]) -> Output {
        self.check_at(&self.head.clone(), env, "")
    }

    fn check_at(&self, reviewed: &str, env: &[(&str, &str)], stdin: &str) -> Output {
        self.check_with(&round_tool(), reviewed, env, stdin)
    }

    fn check_with(&self, tool: &Path, reviewed: &str, env: &[(&str, &str)], stdin: &str) -> Output {
        let args: Vec<OsString> = vec![
            tool.into(),
            "check".into(),
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

/// The tool's stdout: exactly one line, a JSON object whose keys are
/// `light_exit` and, at most, `reason`.
fn result(o: &Output) -> Value {
    let out = stdout(o);
    assert!(out.ends_with('\n'), "one terminated line: {}", show(o));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one JSON line on stdout: {}",
        show(o)
    );
    let j: Value =
        serde_json::from_str(lines[0]).unwrap_or_else(|e| panic!("not JSON ({e}): {}", show(o)));
    let keys: Vec<&String> = j.as_object().expect("an object").keys().collect();
    assert!(
        keys.iter()
            .all(|k| k.as_str() == "light_exit" || k.as_str() == "reason"),
        "only light_exit and reason: {}",
        show(o)
    );
    j
}

/// `off`; `reason` is `None` for "no marker" and the token otherwise.
fn exit_off(o: &Output, reason: Option<&str>) -> Value {
    assert_eq!(o.status.code(), Some(0), "{}", show(o));
    let j = result(o);
    assert_eq!(j["light_exit"], "off", "{}", show(o));
    match reason {
        None => assert!(j.get("reason").is_none(), "no reason: {}", show(o)),
        Some(r) => assert_eq!(j["reason"], r, "{}", show(o)),
    }
    j
}

fn exit_light(o: &Output) {
    assert_eq!(o.status.code(), Some(0), "{}", show(o));
    assert_eq!(stdout(o), "{\"light_exit\":\"light\"}\n", "{}", show(o));
}

fn exit_escalated(o: &Output) -> String {
    assert_eq!(o.status.code(), Some(0), "{}", show(o));
    let j = result(o);
    assert_eq!(j["light_exit"], "escalated", "{}", show(o));
    let r = j["reason"].as_str().expect("a reason").to_string();
    assert!(r.starts_with("in-loop-"), "{}", show(o));
    r
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

/// Writes the `.findings` file step-03 writes before the light hook runs.
fn findings(e: &Env, ids: &[&str]) {
    let body: String = ids.iter().map(|i| format!("{i}\n")).collect();
    private_file(&e.rec(".findings"), &body);
}

fn state_names(e: &Env) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(&e.state)
        .unwrap()
        .map(|d| d.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

/// No retired light file, side file or temporary is in the state directory.
fn no_retired_files(e: &Env) {
    for name in state_names(e) {
        assert!(
            !RETIRED.contains(&name.as_str()),
            "{name} is a retired light state file"
        );
        assert!(
            !name.ends_with(".concerns.tsv") && !name.ends_with(".light.json"),
            "{name} is a retired light side file"
        );
        assert!(!name.starts_with('.'), "stray temporary {name}");
    }
}

/// `check` calls `gh` only to read the PR's base, never to write.
fn gh_reads_the_base_only(e: &Env) {
    for c in e.fakes.gh_calls() {
        assert_eq!(
            (c.first().map(String::as_str), c.get(1).map(String::as_str)),
            (Some("pr"), Some("view")),
            "the only gh call is pr view: {c:?}"
        );
        assert!(
            c.iter().any(|a| a == "baseRefName,baseRefOid"),
            "pr view reads the base only: {c:?}"
        );
    }
}

// ===========================================================================
// 1. autodrive_light_round.sh check
// ===========================================================================

#[test]
fn without_a_marker_check_is_off_and_touches_nothing() {
    if skip() {
        return;
    }
    let e = Env::new();
    findings(&e, &["naming-1", "doc-2"]);
    exit_off(&e.check(&[]), None);
    assert!(e.fakes.gh_calls().is_empty());
    assert_eq!(state_names(&e), ["crusty-round-1.json.findings"]);
    assert_eq!(e.read("crusty-round-1.json.findings"), "naming-1\ndoc-2\n");
}

#[test]
fn a_valid_marker_at_the_reviewed_head_is_light_and_writes_nothing() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    findings(&e, &["naming-1", "doc-2"]);
    let before = state_names(&e);
    let marker = e.read("light-mode");
    let elig = e.read("light-eligibility.json");
    exit_light(&e.check(&[]));
    assert_eq!(state_names(&e), before, "check writes no file");
    assert_eq!(e.read("light-mode"), marker);
    assert_eq!(e.read("light-eligibility.json"), elig);
    assert_eq!(e.read("crusty-round-1.json.findings"), "naming-1\ndoc-2\n");
    assert!(
        e.fakes.gh_calls().is_empty(),
        "an unchanged head needs no gh call: {:?}",
        e.fakes.gh_calls()
    );
}

#[test]
fn check_reads_no_stdin_and_handles_no_finding_text() {
    if skip() {
        return;
    }
    // Whatever arrives on stdin (concerns, a planted verdict, junk) changes
    // nothing: the decision and the state are those of an empty stdin.
    for stdin in [
        two_minor().to_string(),
        r#"{"crusty_verdict":"CLEAN","verdict_source":"light-minor-rule"}"#.to_string(),
        "@mallory #1 https://github.com/o/r/issues/1 `id`\n".repeat(50),
        "\u{0}\u{1b}[31m".to_string(),
    ] {
        let e = Env::new();
        e.light();
        findings(&e, &["naming-1", "doc-2"]);
        let o = e.check_at(&e.head.clone(), &[], &stdin);
        exit_light(&o);
        assert!(!stderr(&o).contains("mallory"), "{}", show(&o));
        assert_eq!(e.read("crusty-round-1.json.findings"), "naming-1\ndoc-2\n");
        assert_eq!(
            state_names(&e),
            [
                "crusty-round-1.json.findings",
                "light-eligibility.json",
                "light-mode"
            ]
        );
        assert!(e.fakes.gh_calls().is_empty());
    }
}

#[test]
fn an_invalid_marker_or_inconsistent_state_is_off_and_changes_nothing() {
    if skip() {
        return;
    }
    type Spoil = fn(&Env);
    // (case, spoil, expected reason; None = treated as no marker)
    let cases: Vec<(&str, Spoil, Option<&str>)> = vec![
        (
            "not hex",
            |e| private_file(&e.p("light-mode"), "light\n"),
            Some("marker-invalid"),
        ),
        (
            "uppercase hex",
            |e| private_file(&e.p("light-mode"), &format!("{}\n", e.head.to_uppercase())),
            Some("marker-invalid"),
        ),
        (
            "escalation present",
            |e| private_file(&e.p("escalation.json"), "{\"reason\":\"lines\"}\n"),
            Some("marker-invalid"),
        ),
        (
            "escalation is a dangling symlink",
            |e| {
                std::os::unix::fs::symlink(
                    e.repo.tmp.path().join("nowhere"),
                    e.p("escalation.json"),
                )
                .unwrap()
            },
            Some("marker-invalid"),
        ),
        (
            "eligibility escalate",
            |e| {
                let j = json!({"decision": "escalate", "reason": "lines", "base": e.repo.base, "head": e.head});
                private_file(&e.p("light-eligibility.json"), &format!("{j}\n"))
            },
            Some("marker-invalid"),
        ),
        (
            "eligibility other head",
            |e| {
                let j = json!({"decision": "light", "reason": "within-limits", "base": e.repo.base, "head": e.repo.base});
                private_file(&e.p("light-eligibility.json"), &format!("{j}\n"))
            },
            Some("marker-invalid"),
        ),
        (
            "eligibility missing",
            |e| std::fs::remove_file(e.p("light-eligibility.json")).unwrap(),
            Some("marker-invalid"),
        ),
        (
            "eligibility is a symlink",
            |e| {
                let real = e.repo.tmp.path().join("elig-elsewhere.json");
                std::fs::copy(e.p("light-eligibility.json"), &real).unwrap();
                std::fs::remove_file(e.p("light-eligibility.json")).unwrap();
                std::os::unix::fs::symlink(&real, e.p("light-eligibility.json")).unwrap();
            },
            Some("marker-invalid"),
        ),
        (
            "marker group-writable",
            |e| {
                std::fs::set_permissions(e.p("light-mode"), std::fs::Permissions::from_mode(0o620))
                    .unwrap()
            },
            Some("state-unsafe"),
        ),
        (
            "marker world-readable-writable",
            |e| {
                std::fs::set_permissions(e.p("light-mode"), std::fs::Permissions::from_mode(0o666))
                    .unwrap()
            },
            Some("state-unsafe"),
        ),
        (
            "eligibility group-writable",
            |e| {
                std::fs::set_permissions(
                    e.p("light-eligibility.json"),
                    std::fs::Permissions::from_mode(0o620),
                )
                .unwrap()
            },
            Some("state-unsafe"),
        ),
        (
            "state dir group-writable",
            |e| std::fs::set_permissions(&e.state, std::fs::Permissions::from_mode(0o770)).unwrap(),
            Some("state-unsafe"),
        ),
        (
            "marker is a symlink",
            |e| {
                let real = e.repo.tmp.path().join("elsewhere");
                std::fs::write(&real, format!("{}\n", e.head)).unwrap();
                std::fs::remove_file(e.p("light-mode")).unwrap();
                std::os::unix::fs::symlink(&real, e.p("light-mode")).unwrap();
            },
            None,
        ),
    ];
    for (name, spoil, reason) in cases {
        let e = Env::new();
        e.light();
        spoil(&e);
        findings(&e, &["naming-1", "doc-2"]);
        let before = state_names(&e);
        let o = e.check(&[]);
        let j = result(&o);
        assert_eq!(o.status.code(), Some(0), "{name}: {}", show(&o));
        assert_eq!(j["light_exit"], "off", "{name}: {}", show(&o));
        match reason {
            None => assert!(j.get("reason").is_none(), "{name}: {}", show(&o)),
            Some(r) => assert_eq!(j["reason"], r, "{name}: {}", show(&o)),
        }
        assert_eq!(state_names(&e), before, "{name}: nothing is written");
        assert_eq!(
            e.read("crusty-round-1.json.findings"),
            "naming-1\ndoc-2\n",
            "{name}"
        );
        assert!(e.fakes.gh_calls().is_empty(), "{name}");
    }
}

#[test]
fn planted_symlinks_beside_the_record_are_neither_followed_nor_touched() {
    if skip() {
        return;
    }
    // Retired light file names, planted as symlinks to files outside the
    // state directory: `check` neither reads, follows, writes nor removes them.
    let e = Env::new();
    e.light();
    let outside = e.repo.tmp.path().join("outside");
    private_dir(&outside);
    let mut planted = Vec::new();
    for name in [
        "crusty-round-1.json.light.json",
        "crusty-round-1.json.concerns.tsv",
        "crusty-round-1.json.findings",
        "follow-up-issue",
        "follow-up-posted-ids",
        "follow-up-failed",
    ] {
        let target = outside.join(name);
        private_file(&target, "precious\n");
        std::os::unix::fs::symlink(&target, e.p(name)).unwrap();
        planted.push((name, target));
    }
    e.repo.write(
        "docs/guide.md",
        "# guide\n\nA small fix.\nAnd an address commit.\n",
    );
    let moved = e.repo.commit("address");
    exit_light(&e.check(&[]));
    exit_light(&e.check_at(&moved, &[("FAKE_GH_HEAD", &moved)], ""));
    for (name, target) in planted {
        assert!(
            std::fs::symlink_metadata(e.p(name))
                .unwrap()
                .file_type()
                .is_symlink(),
            "{name} is left as it was"
        );
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "precious\n",
            "{name}: the target is not written"
        );
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
    exit_off(&e.check_at(&side, &[], ""), Some("marker-stale"));
    assert!(e.p("light-mode").exists(), "a stale marker is not removed");
    assert!(!e.p("escalation.json").exists());
    assert!(e.fakes.gh_calls().is_empty());
}

// --- re-measurement inside the loop ------------------------------------------

#[test]
fn a_moved_head_within_limits_is_re_measured_and_stays_light() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    findings(&e, &["naming-1", "doc-2"]);
    e.repo.write(
        "docs/guide.md",
        "# guide\n\nA small fix.\nAnd an address commit.\n",
    );
    let moved = e.repo.commit("address");
    exit_light(&e.check_at(&moved, &[("FAKE_GH_HEAD", &moved)], ""));
    assert_eq!(e.read("light-mode").trim(), moved);
    let elig: Value = serde_json::from_str(&e.read("light-eligibility.json")).unwrap();
    assert_eq!(elig["head"], moved.as_str());
    assert_eq!(elig["decision"], "light");
    assert_eq!(elig["base"], e.repo.base.as_str());
    assert_eq!(mode(&e.p("light-mode")), 0o600);
    assert_eq!(mode(&e.p("light-eligibility.json")), 0o600);
    assert_eq!(e.read("crusty-round-1.json.findings"), "naming-1\ndoc-2\n");
    gh_reads_the_base_only(&e);
    no_retired_files(&e);
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
    let o = e.check_at(&moved, &[("FAKE_GH_HEAD", &moved)], "");
    assert_eq!(exit_escalated(&o), "in-loop-lines");
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
    gh_reads_the_base_only(&e);
    no_retired_files(&e);

    // Every later round finds no marker.
    exit_off(&e.check_at(&moved, &[("FAKE_GH_HEAD", &moved)], ""), None);
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
    let reason = exit_escalated(&e.check_at(&moved, &[("FAKE_GH_HEAD", &moved)], ""));
    assert!(reason.starts_with("in-loop-fault-"), "{reason}");
    let esc: Value = serde_json::from_str(&e.read("escalation.json")).unwrap();
    assert_eq!(esc["reason"], reason.as_str());
    assert!(!e.p("light-mode").exists());
}

/// A tools directory holding a copy of the real round tool and its gh helper,
/// and a stand-in `autodrive_light_eligibility.sh` that prints
/// `AUTODRIVE_LIGHT_ELIGIBILITY=escalate reason=$FAKE_REASON` (exit 0), or,
/// with `$FAKE_FAULT` set, the fault line on stderr and exit 2.
fn tools_with_fake_eligibility(e: &Env) -> PathBuf {
    let t = e.repo.tmp.path().join("elig-tools");
    std::fs::create_dir_all(&t).unwrap();
    for f in ["autodrive_light_round.sh", "workflow_gh_retry.sh"] {
        std::fs::copy(tools_dir().join(f), t.join(f)).unwrap();
    }
    crate::support::exec_stub::write_executable(
        &t.join("autodrive_light_eligibility.sh"),
        r#"#!/usr/bin/env bash
if [ -n "${FAKE_FAULT:-}" ]; then
  printf 'AUTODRIVE_LIGHT_ELIGIBILITY=fault reason=%s\n' "$FAKE_FAULT" >&2
  exit 2
fi
printf 'AUTODRIVE_LIGHT_ELIGIBILITY=escalate reason=%s\n{"decision":"escalate"}\n' "$FAKE_REASON"
"#,
        0o755,
    );
    t
}

#[test]
fn the_escalation_reason_is_bounded() {
    if skip() {
        return;
    }
    // S2: only `^in-loop-[a-z][a-z-]{0,54}$` is written or printed; any other
    // reason becomes in-loop-fault-unknown.
    let a55 = "a".repeat(55);
    let a56 = "a".repeat(56);
    let f60 = "f".repeat(60);
    type ReasonCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, String);
    let cases: Vec<ReasonCase> = vec![
        (
            "plain",
            vec![("FAKE_REASON", "lines")],
            "in-loop-lines".into(),
        ),
        (
            "at the bound",
            vec![("FAKE_REASON", a55.as_str())],
            format!("in-loop-{a55}"),
        ),
        (
            "one over the bound",
            vec![("FAKE_REASON", a56.as_str())],
            "in-loop-fault-unknown".into(),
        ),
        (
            "leading dash",
            vec![("FAKE_REASON", "-lines")],
            "in-loop-fault-unknown".into(),
        ),
        (
            "quoted",
            vec![("FAKE_REASON", r#"li"nes"#)],
            "in-loop-fault-unknown".into(),
        ),
        (
            "uppercase",
            vec![("FAKE_REASON", "Lines")],
            "in-loop-fault-unknown".into(),
        ),
        (
            "fault token",
            vec![("FAKE_FAULT", "git-failed")],
            "in-loop-fault-git-failed".into(),
        ),
        (
            "overlong fault token",
            vec![("FAKE_FAULT", f60.as_str())],
            "in-loop-fault-unknown".into(),
        ),
    ];
    for (name, env, want) in cases {
        let e = Env::new();
        e.light();
        e.repo.write("docs/guide.md", "moved\n");
        let moved = e.repo.commit("moved");
        let tools = tools_with_fake_eligibility(&e);
        let mut env = env;
        env.push(("FAKE_GH_HEAD", &moved));
        let o = e.check_with(&tools.join("autodrive_light_round.sh"), &moved, &env, "");
        assert_eq!(exit_escalated(&o), want, "{name}: {}", show(&o));
        let esc: Value = serde_json::from_str(&e.read("escalation.json")).unwrap();
        assert_eq!(esc["reason"], want.as_str(), "{name}");
        assert!(!e.p("light-mode").exists(), "{name}");
    }
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
    exit_light(&e.check_at(
        &moved,
        &[("FAKE_GH_HEAD", &moved), ("FAKE_GH_BASE_OID", &up)],
        "",
    ));
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
    gh_reads_the_base_only(&e);
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
    exit_light(&e.check_at(
        &moved,
        &[("FAKE_GH_HEAD", &moved), ("FAKE_GH_BASE_OID", &up)],
        "",
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
    let o = e.check_at(
        &moved,
        &[("FAKE_GH_HEAD", &moved), ("FAKE_GH_BASE_OID", &up)],
        "",
    );
    assert_eq!(exit_escalated(&o), "in-loop-lines");
    let esc: Value = serde_json::from_str(&e.read("escalation.json")).unwrap();
    assert_eq!(esc["reason"], "in-loop-lines");
    assert_eq!(esc["base_sha"], up.as_str());
    assert!(!e.p("light-mode").exists());
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
        let mut env: Vec<(&str, &str)> = vec![("FAKE_GH_HEAD", &moved)];
        if name == "base name starts with -" {
            env.push(("FAKE_GH_BASE_OID", &up));
        }
        env.extend(extra.iter().map(|(k, v)| (*k, v.as_str())));
        let o = e.check_at(&moved, &env, "");
        assert_eq!(exit_escalated(&o), "in-loop-lines", "{name}: {}", show(&o));
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
    let o = e.check_at(
        &moved,
        &[
            ("FAKE_GH_HEAD", &moved),
            ("FAKE_GH_FAIL", "pr view"),
            ("FAKE_GH_EXIT_CODE", "79"),
        ],
        "",
    );
    assert_eq!(o.status.code(), Some(79), "{}", show(&o));
    assert!(stdout(&o).is_empty(), "no decision on 79: {}", show(&o));
    assert!(e.p("light-mode").exists());
    assert!(!e.p("escalation.json").exists());
    let left: Vec<_> = std::fs::read_dir(&e.tmpdir).unwrap().collect();
    assert!(left.is_empty(), "the EXIT trap runs on 79 too: {left:?}");
}

#[test]
fn no_retry_temporaries_are_left_behind() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    exit_light(&e.check(&[]));
    e.repo
        .write("docs/guide.md", "# guide\n\nA small fix.\nMore.\n");
    let moved = e.repo.commit("address");
    exit_light(&e.check_at(&moved, &[("FAKE_GH_HEAD", &moved)], ""));
    e.repo.write("docs/big.md", &"line\n".repeat(400));
    let big = e.repo.commit("big");
    exit_escalated(&e.check_at(&big, &[("FAKE_GH_HEAD", &big)], ""));
    let left: Vec<_> = std::fs::read_dir(&e.tmpdir).unwrap().collect();
    assert!(left.is_empty(), "TMPDIR must be clean: {left:?}");
    no_retired_files(&e);
}

// --- argument checks -------------------------------------------------------

#[test]
fn bad_arguments_exit_2_with_empty_stdout() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let rec: OsString = e.record.clone().into();
    let wt: OsString = e.repo.wt.clone().into();
    let h: OsString = e.head.clone().into();
    let base = |cmd: &str, pr: &str, head: &OsString, record: &OsString| -> Vec<OsString> {
        vec![
            round_tool().into(),
            cmd.into(),
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
    let mut extra = base("check", "7", &h, &rec);
    extra.extend(["--unknown".into(), "x".into()]);
    for args in [
        // The retired command, with otherwise valid arguments.
        base("defer", "7", &h, &rec),
        base("check", "7x", &h, &rec),
        base("check", "", &h, &rec),
        base("check", "12345678901", &h, &rec),
        base("check", "7", &"zz".into(), &rec),
        base("check", "7", &"-x".into(), &rec),
        base("check", "7", &h, &"relative/crusty-round-1.json".into()),
        base(
            "check",
            "7",
            &h,
            &format!("{}/../x/crusty-round-1.json", e.state.display()).into(),
        ),
        extra,
        vec![round_tool().into(), "nope".into()],
        vec![round_tool().into()],
    ] {
        let o = e.tool(&args, &[], &two_minor().to_string());
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", show(&o));
        assert!(stdout(&o).is_empty(), "{args:?}: {}", show(&o));
        assert_eq!(
            stderr(&o)
                .lines()
                .filter(|l| l.starts_with("ERROR:"))
                .count(),
            1,
            "{args:?}: one ERROR line: {}",
            show(&o)
        );
    }
    assert!(e.fakes.gh_calls().is_empty());
    assert!(e.p("light-mode").exists());
}

// ===========================================================================
// 2. step-03 and step-06 of autodrive-crusty-round.yaml
// ===========================================================================

/// A tools directory holding a copy of the real `autodrive_light_steps.sh`,
/// which the round recipe sources from `AUTODRIVE_LAUNCH_TOOLS`, and a logging
/// stand-in for `autodrive_light_round.sh`, which saves its stdin, prints
/// `$FAKE_ROUND_OUT` and exits `$FAKE_ROUND_EXIT` (default 0). There is no
/// `autodrive_followups.sh` here, so `classify` is skipped and these tests
/// see the light hook alone.
const FAKE_ROUND_TOOL: &str = r#"#!/usr/bin/env bash
( IFS=$'\x1f'; printf '%s\n' "$*" ) >> "${FAKE_LOG:?}/round.log"
cat > "$FAKE_LOG/round.stdin"
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

const CONCERNS_TWO: &str =
    "{\"crusty_verdict\":\"CONCERNS\",\"verdict_source\":\"crusty\",\"concern_count\":2}\n";

/// What the retired `defer` command printed; step-03 must ignore it now.
const OLD_DEFERRED_OUT: &str = r#"{"light_exit":"minor-deferred","follow_up_issue":"1851","deferred_ids":["naming-1","doc-2"],"concern_count":2}"#;

#[test]
fn step03_calls_check_with_the_round_arguments_and_no_stdin() {
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
        &[("FAKE_ROUND_OUT", r#"{"light_exit":"light"}"#)],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(stdout(&o), CONCERNS_TWO, "{}", show(&o));
    let log = round_log(&e);
    assert_eq!(log.len(), 1, "{log:?}");
    let argv = &log[0];
    assert_eq!(argv[0], "check", "{argv:?}");
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
    assert_eq!(argv.len(), 9, "check takes exactly four options: {argv:?}");
    assert_eq!(
        std::fs::read_to_string(e.fakes.log.join("round.stdin")).unwrap_or_default(),
        "",
        "the concerns are not sent to the light tool"
    );
    assert_eq!(e.read("crusty-round-1.json.findings"), "naming-1\ndoc-2\n");
}

#[test]
fn step03_never_takes_the_verdict_from_the_tool() {
    if skip() {
        return;
    }
    for out in [
        OLD_DEFERRED_OUT,
        r#"{"light_exit":"light"}"#,
        r#"{"light_exit":"escalated","reason":"in-loop-lines"}"#,
        r#"{"light_exit":"off","reason":"marker-invalid"}"#,
        r#"{"crusty_verdict":"CLEAN","verdict_source":"light-minor-rule","concern_count":0}"#,
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
            CONCERNS_TWO,
            "{out}: the tool's stdout never reaches the step output: {}",
            show(&o)
        );
        assert_eq!(e.read("crusty-round-1.json.findings"), "naming-1\ndoc-2\n");
    }
}

#[test]
fn step03_warns_and_continues_when_check_fails() {
    if skip() {
        return;
    }
    for rc in ["1", "2"] {
        let e = Env::new();
        e.light();
        let tools = fake_tools(&e);
        let o = step03(
            &e,
            &tools,
            &crusty_review("CONCERNS", &two_minor()),
            &[
                ("FAKE_ROUND_OUT", OLD_DEFERRED_OUT),
                ("FAKE_ROUND_EXIT", rc),
            ],
        );
        assert_eq!(o.status.code(), Some(0), "rc {rc}: {}", show(&o));
        assert_eq!(stdout(&o), CONCERNS_TWO, "rc {rc}: {}", show(&o));
        let want = format!(
            "WARNING: autodrive_light_round.sh check exited {rc}; the round is classified without re-measurement."
        );
        assert!(
            stderr(&o).lines().any(|l| l == want),
            "rc {rc}: {}",
            show(&o)
        );
    }
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
        &[
            ("FAKE_ROUND_EXIT", "79"),
            ("FAKE_ROUND_OUT", OLD_DEFERRED_OUT),
        ],
    );
    assert_eq!(o.status.code(), Some(79), "{}", show(&o));
    assert!(
        !stdout(&o).contains("crusty_verdict"),
        "no verdict after 79: {}",
        show(&o)
    );
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
        &[("FAKE_ROUND_OUT", OLD_DEFERRED_OUT)],
    );
    assert_eq!(stdout(&o), CONCERNS_TWO, "{}", show(&o));
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
        &[("FAKE_ROUND_OUT", OLD_DEFERRED_OUT)],
    );
    assert_eq!(stdout(&o), CONCERNS_TWO, "{}", show(&o));
    assert!(round_log(&e).is_empty());
}

#[test]
fn step03_checks_only_a_crusty_concerns_verdict() {
    if skip() {
        return;
    }
    // D3: a CLEAN round, or a round with no verdict, is not re-measured.
    let e = Env::new();
    e.light();
    let tools = fake_tools(&e);
    let o = step03(
        &e,
        &tools,
        &crusty_review("CLEAN", &json!([])),
        &[("FAKE_ROUND_OUT", OLD_DEFERRED_OUT)],
    );
    assert_eq!(
        stdout(&o),
        "{\"crusty_verdict\":\"CLEAN\",\"verdict_source\":\"crusty\",\"concern_count\":0}\n",
        "{}",
        show(&o)
    );
    let o = step03(&e, &tools, "", &[("FAKE_ROUND_OUT", OLD_DEFERRED_OUT)]);
    assert_eq!(
        stdout(&o),
        "{\"crusty_verdict\":\"CONCERNS\",\"verdict_source\":\"missing_verdict\",\"concern_count\":0}\n",
        "{}",
        show(&o)
    );
    assert!(round_log(&e).is_empty(), "{:?}", round_log(&e));
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
        &[("FAKE_ROUND_OUT", OLD_DEFERRED_OUT)],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(stdout(&o), CONCERNS_TWO, "{}", show(&o));
    assert!(
        stderr(&o).lines().any(|l| l
            == "WARNING: autodrive_light_steps.sh not found under AUTODRIVE_LAUNCH_TOOLS; the moved head is not re-measured."),
        "{}",
        show(&o)
    );
    assert!(round_log(&e).is_empty(), "the round tool is not called");
}

#[test]
fn step03_function_leaves_stdout_and_the_verdict_variables_unchanged() {
    if skip() {
        return;
    }
    for (out, rc, want_rc) in [
        (OLD_DEFERRED_OUT, "0", "0"),
        (
            r#"{"light_exit":"escalated","reason":"in-loop-lines"}"#,
            "0",
            "0",
        ),
        (OLD_DEFERRED_OUT, "1", "0"),
        (OLD_DEFERRED_OUT, "79", "79"),
    ] {
        let e = Env::new();
        e.light();
        let tools = fake_tools(&e);
        let ctx = round_context(&e);
        let script = format!(
            r#"AUTODRIVE_LAUNCH_TOOLS={tools:?}
. "$AUTODRIVE_LAUNCH_TOOLS/autodrive_light_steps.sh" || exit 98
VERDICT=CONCERNS; SOURCE=crusty; COUNT=2; RECORD={record:?}
CONCERNS='[{{"id":"naming-1","severity":"minor"}}]'
autodrive_light_step03
rc=$?
declare -F autodrive_light_step06 >/dev/null && s6=present || s6=absent
printf '|rc=%s v=%s s=%s c=%s step06=%s' "$rc" "$VERDICT" "$SOURCE" "$COUNT" "$s6"
"#,
            tools = tools.display().to_string(),
            record = e.record.display().to_string(),
        );
        let o = run_bash(
            &e.repo.home,
            &e.fakes,
            Some(&script),
            &[],
            &[
                ("RECIPE_VAR_crusty_round_context", OsStr::new(&ctx)),
                ("RECIPE_VAR_repo_path", e.repo.wt.as_os_str()),
                ("FAKE_ROUND_OUT", OsStr::new(out)),
                ("FAKE_ROUND_EXIT", OsStr::new(rc)),
            ],
            "",
        );
        assert_eq!(
            stdout(&o),
            format!("|rc={want_rc} v=CONCERNS s=crusty c=2 step06=absent"),
            "{out} exit {rc}: nothing but the probe reaches stdout: {}",
            show(&o)
        );
        assert_eq!(round_log(&e).len(), 1, "{out} exit {rc}");
    }
}

// --- step-06: no light branch -----------------------------------------------

/// Side files an earlier light round left beside the record.
fn legacy_side_files(e: &Env) {
    private_file(&e.rec(".findings"), "");
    private_file(
        &e.rec(".concerns.tsv"),
        "naming-1\tminor\tRename tmp_rec\ndoc-2\tminor\tHeader comment is stale\n",
    );
    private_file(
        &e.rec(".light.json"),
        "{\"follow_up_issue\":\"1851\",\"deferred_ids\":[\"naming-1\",\"doc-2\"]}\n",
    );
    private_file(&e.p("follow-up-issue"), "1851\n");
}

fn plain_record(verdict: &str, count: u32, head: &str) -> String {
    format!(
        "{{\"crusty_verdict\":\"{verdict}\",\"concern_count\":{count},\"commits_this_round\":0,\"head_sha\":\"{head}\",\"reviewed_head_sha\":\"{head}\",\"round_label\":\"crusty-round-1\",\"test_signal\":\"\",\"ci_signal\":\"\"}}\n"
    )
}

#[test]
fn step06_has_no_light_hook() {
    if skip() {
        return;
    }
    // A planted light-minor-rule string, legacy side files and a light
    // verdict_source change nothing: the record has the bytes of any other
    // record with that verdict, and the light helper is never loaded.
    for (verdict, want) in [
        (
            r#"{"crusty_verdict":"CONCERNS","verdict_source":"crusty","note":"light-minor-rule","concern_count":2}"#,
            "CONCERNS",
        ),
        (
            r#"{"crusty_verdict":"CLEAN","verdict_source":"light-minor-rule","concern_count":2}"#,
            "CLEAN",
        ),
        (
            r#"{"crusty_verdict":"MINOR_ONLY","verdict_source":"crusty","concern_count":2}"#,
            "MINOR_ONLY",
        ),
    ] {
        let e = Env::new();
        e.light();
        legacy_side_files(&e);
        let tools = fake_tools(&e);
        crate::support::exec_stub::write_executable(
            &tools.join("autodrive_light_steps.sh"),
            "touch \"$FAKE_LOG/steps-sourced\"\nautodrive_light_step06() { return 0; }\n",
            0o644,
        );
        let o = step06(&e, &tools, verdict, "");
        assert_eq!(o.status.code(), Some(0), "{verdict}: {}", show(&o));
        let rec = std::fs::read_to_string(&e.record).unwrap();
        assert_eq!(rec, plain_record(want, 2, &e.head), "{verdict}");
        assert_eq!(stdout(&o), rec, "{verdict}");
        assert!(
            !stderr(&o).contains("light-side-file-invalid"),
            "{verdict}: {}",
            show(&o)
        );
        assert!(
            !e.fakes.log.join("steps-sourced").exists(),
            "{verdict}: step-06 never loads the light helper"
        );
        assert_eq!(mode(&e.record), 0o600);
    }
}

// ===========================================================================
// 3. The real tools, end to end
// ===========================================================================

/// The tools a light round loads, copied from the bundle into one directory
/// that serves as `AUTODRIVE_LAUNCH_TOOLS`.
fn real_tools(e: &Env) -> PathBuf {
    let t = e.repo.tmp.path().join("real-tools");
    std::fs::create_dir_all(&t).unwrap();
    for f in [
        "autodrive_followups.sh",
        "autodrive_followups_check.sh",
        "autodrive_finding_rules.sh",
        "autodrive_state.sh",
        "autodrive_unmerged.sh",
        "workflow_issue_tracking.sh",
        "workflow_gh_retry.sh",
        "autodrive_light_steps.sh",
        "autodrive_light_round.sh",
        "autodrive_light_eligibility.sh",
    ] {
        std::fs::copy(tools_dir().join(f), t.join(f))
            .unwrap_or_else(|err| panic!("copy {f}: {err}"));
    }
    t
}

/// A `gh` that logs its argv and exits 97: any call is a failure.
const SENTINEL_GH: &str = r#"#!/usr/bin/env bash
( IFS=$'\x1f'; printf '%s\n' "$*" ) >> "${FAKE_LOG:?}/gh.log"
echo "sentinel gh: no gh call is expected here" >&2
exit 97
"#;

/// A `gh` for `file`: `pr view` answers PR 7 of o/r with an empty body,
/// `issue create` prints `https://github.com/o/r/issues/<101, 102, ...>`,
/// `issue comment` prints a comment URL. Every call is logged.
const FILING_GH: &str = r#"#!/usr/bin/env bash
set -u
log="${FAKE_LOG:?}"
( IFS=$'\x1f'; printf '%s\n' "$*" ) >> "$log/gh.log"
args=("$@"); jqf=""
for ((i=0; i<${#args[@]}; i++)); do
  case "${args[$i]}" in --jq|-q) jqf="${args[$((i+1))]:-}" ;; esac
done
emit() { if [ -n "$jqf" ]; then printf '%s' "$1" | jq -r "$jqf"; else printf '%s\n' "$1"; fi; }
case "${1:-} ${2:-}" in
  "pr view") emit '{"number":7,"url":"https://github.com/o/r/pull/7","body":"Refs #1982"}' ;;
  "issue create")
    made=$(grep -c . "$log/created" 2>/dev/null || true); made=${made:-0}
    echo x >> "$log/created"
    printf 'https://github.com/o/r/issues/%s\n' "$((101 + made))" ;;
  "issue comment") printf 'https://github.com/o/r/issues/%s#issuecomment-1\n' "${3:-0}" ;;
esac
exit 0
"#;

fn use_gh(e: &Env, body: &str) {
    crate::support::exec_stub::write_executable(&e.fakes.bin.join("gh"), body, 0o755);
}

/// Runs `autodrive_followups.sh <args>` from `tools`.
fn followups(e: &Env, tools: &Path, args: &[&str]) -> Output {
    let tool = tools.join("autodrive_followups.sh");
    let mut argv: Vec<OsString> = vec![tool.into()];
    argv.extend(args.iter().map(OsString::from));
    e.tool(&argv, &[], "")
}

fn state_arg(e: &Env) -> String {
    e.state.to_string_lossy().into_owned()
}

/// What autodrive_loop.sh does after a round: a copy in crusty-latest.json
/// and a manifest row with the record's blob hash.
fn loop_writes_round(e: &Env) {
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
}

const MINOR_ONLY_TWO: &str =
    "{\"crusty_verdict\":\"MINOR_ONLY\",\"verdict_source\":\"crusty\",\"concern_count\":2}\n";

/// A light round on an unchanged head whose findings are all minor, through
/// step-03 and step-06, with the sentinel `gh`. Returns the tools directory.
fn minor_only_light_round(e: &Env) -> PathBuf {
    e.light();
    let tools = real_tools(e);
    use_gh(e, SENTINEL_GH);
    let o = step03(e, &tools, &crusty_review("CONCERNS", &two_minor()), &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(stdout(&o), MINOR_ONLY_TWO, "{}", show(&o));
    let o6 = step06(e, &tools, MINOR_ONLY_TWO.trim(), "");
    assert_eq!(o6.status.code(), Some(0), "{}", show(&o6));
    assert!(
        e.fakes.gh_calls().is_empty(),
        "step-03 and step-06 make no gh call: {:?}",
        e.fakes.gh_calls()
    );
    tools
}

#[test]
fn real_tools_classify_an_all_minor_light_round_as_minor_only() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let tools = real_tools(&e);
    use_gh(&e, SENTINEL_GH);
    let o = step03(&e, &tools, &crusty_review("CONCERNS", &two_minor()), &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(stdout(&o), MINOR_ONLY_TWO, "{}", show(&o));
    let minor: Vec<Value> = e
        .read("crusty-round-1.json.minor")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let ids: Vec<&str> = minor.iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["naming-1", "doc-2"]);
    assert_eq!(e.read("crusty-round-1.json.findings"), "");
    assert!(e.p("crusty-round-1.json.severity").exists());
    assert_eq!(e.read("light-mode").trim(), e.head, "the marker stays");
    assert!(e.fakes.gh_calls().is_empty(), "{:?}", e.fakes.gh_calls());
    no_retired_files(&e);
}

#[test]
fn real_tools_a_light_round_with_a_major_finding_stays_concerns() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let tools = real_tools(&e);
    use_gh(&e, SENTINEL_GH);
    let concerns = json!([
        minor("naming-1", "Rename"),
        {"id": "real-bug", "severity": "major", "summary": "Wrong result", "evidence": "x"}
    ]);
    let o = step03(&e, &tools, &crusty_review("CONCERNS", &concerns), &[]);
    assert_eq!(stdout(&o), CONCERNS_TWO, "{}", show(&o));
    assert_eq!(
        e.read("crusty-round-1.json.findings"),
        "naming-1\nreal-bug\n"
    );
    assert!(!e.rec(".minor").exists());
    assert!(e.fakes.gh_calls().is_empty());
    no_retired_files(&e);
}

#[test]
fn real_tools_record_a_light_minor_only_round_with_the_plain_prefix() {
    if skip() {
        return;
    }
    let e = Env::new();
    minor_only_light_round(&e);
    let rec = std::fs::read_to_string(&e.record).unwrap();
    assert!(
        rec.starts_with("{\"crusty_verdict\":\"MINOR_ONLY\","),
        "{rec}"
    );
    assert_eq!(rec, plain_record("MINOR_ONLY", 2, &e.head));
    assert!(!rec.contains("verdict_source"), "{rec}");
}

/// Collects and files the follow-ups of a light MINOR_ONLY round for task
/// #1982 with the filing `gh`, and returns the gh calls `file` made.
fn collect_and_file(e: &Env, tools: &Path) -> Vec<Vec<String>> {
    loop_writes_round(e);
    let st = state_arg(e);
    let o = followups(e, tools, &["collect", "--state-dir", &st]);
    assert_eq!(o.status.code(), Some(0), "collect: {}", show(&o));
    use_gh(e, FILING_GH);
    let o = followups(
        e,
        tools,
        &[
            "file",
            "--state-dir",
            &st,
            "--pr",
            "7",
            "--task-id",
            "#1982",
        ],
    );
    assert_eq!(o.status.code(), Some(0), "file: {}", show(&o));
    assert_eq!(stdout(&o).trim(), "followups-ok 2", "{}", show(&o));
    e.fakes.gh_calls()
}

#[test]
fn real_tools_collect_and_file_make_one_issue_per_finding_and_one_parent_comment() {
    if skip() {
        return;
    }
    let e = Env::new();
    let tools = minor_only_light_round(&e);
    let calls = collect_and_file(&e, &tools);
    let first2 = |c: &Vec<String>| (c.first().cloned(), c.get(1).cloned());
    let creates: Vec<_> = calls
        .iter()
        .filter(|c| first2(c) == (Some("issue".into()), Some("create".into())))
        .collect();
    assert_eq!(creates.len(), 2, "{calls:?}");
    let comments: Vec<_> = calls
        .iter()
        .filter(|c| first2(c) == (Some("issue".into()), Some("comment".into())))
        .collect();
    assert_eq!(comments.len(), 1, "one parent comment: {calls:?}");
    assert_eq!(comments[0].get(2).map(String::as_str), Some("1982"));
    assert!(
        !calls
            .iter()
            .any(|c| first2(c) == (Some("pr".into()), Some("comment".into()))),
        "no PR comment: {calls:?}"
    );
    let rows: Vec<Vec<String>> = e
        .read("followups.tsv")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.split('\t').map(String::from).collect())
        .collect();
    assert_eq!(
        rows,
        [
            ["naming-1", "101", "#1982", "7"],
            ["doc-2", "102", "#1982", "7"]
        ]
    );
}

#[test]
fn real_tools_gate_blocks_a_light_minor_only_drive_until_filing() {
    if skip() {
        return;
    }
    let e = Env::new();
    let tools = minor_only_light_round(&e);
    loop_writes_round(&e);
    let st = state_arg(&e);
    let o = followups(&e, &tools, &["collect", "--state-dir", &st]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    // Before filing: no light exemption.
    let o = followups(&e, &tools, &["gate", "--state-dir", &st, "--pr", "7"]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(
        stderr(&o).contains("ERROR: followups-unfiled:"),
        "{}",
        show(&o)
    );
    let o = followups(&e, &tools, &["check", "--state-dir", &st, "--pr", "7"]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(e.fakes.gh_calls().is_empty(), "gate and check are offline");
    // After filing.
    collect_and_file(&e, &tools);
    let o = followups(&e, &tools, &["gate", "--state-dir", &st, "--pr", "7"]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(stdout(&o).starts_with("followups-ok 2"), "{}", show(&o));
    let o = followups(&e, &tools, &["check", "--state-dir", &st, "--pr", "7"]);
    assert_eq!(stdout(&o).trim(), "followups-ok 2", "{}", show(&o));
}

/// Runs step-03 with the real tools for round `n` of the same state dir.
fn real_round(e: &Env, tools: &Path, n: u32, concerns: &Value) -> Output {
    let rec = e.p(&format!("crusty-round-{n}.json"));
    let rec = rec.to_string_lossy().into_owned();
    step03(
        e,
        tools,
        &crusty_review("CONCERNS", concerns),
        &[("AUTODRIVE_ROUND_RECORD", &rec)],
    )
}

fn verdict_of(o: &Output) -> String {
    let j: Value = serde_json::from_str(stdout(o).trim())
        .unwrap_or_else(|err| panic!("one verdict line ({err}): {}", show(o)));
    j["crusty_verdict"].as_str().unwrap().to_string()
}

#[test]
fn real_tools_an_id_once_major_keeps_a_later_light_round_concerns() {
    if skip() {
        return;
    }
    // Recurrence, case 1 (decision 17.104): an id another round saw as major
    // stops a later all-minor round from being MINOR_ONLY.
    let e = Env::new();
    e.light();
    let tools = real_tools(&e);
    use_gh(&e, SENTINEL_GH);
    let r1 = json!([
        minor("naming-1", "Rename"),
        {"id": "bug-2", "severity": "major", "summary": "Wrong result", "evidence": "x"}
    ]);
    assert_eq!(verdict_of(&real_round(&e, &tools, 1, &r1)), "CONCERNS");
    let r2 = json!([
        minor("bug-2", "Now only cosmetic"),
        minor("naming-1", "Rename")
    ]);
    let o = real_round(&e, &tools, 2, &r2);
    assert_eq!(verdict_of(&o), "CONCERNS", "{}", show(&o));
    assert!(!e.p("crusty-round-2.json.minor").exists());
    assert!(e.fakes.gh_calls().is_empty());
}

#[test]
fn real_tools_a_minor_id_raised_again_stays_minor() {
    if skip() {
        return;
    }
    // Recurrence, case 2: an id seen only as minor before stays minor. The
    // retired light rule promoted every returning id to major.
    let e = Env::new();
    e.light();
    let tools = real_tools(&e);
    use_gh(&e, SENTINEL_GH);
    let r1 = json!([
        minor("naming-1", "Rename"),
        {"id": "bug-2", "severity": "major", "summary": "Wrong result", "evidence": "x"}
    ]);
    assert_eq!(verdict_of(&real_round(&e, &tools, 1, &r1)), "CONCERNS");
    private_file(&e.p("resolved-concerns.txt"), "bug-2\n");
    let o = real_round(&e, &tools, 2, &json!([minor("naming-1", "Rename")]));
    assert_eq!(verdict_of(&o), "MINOR_ONLY", "{}", show(&o));
    assert_eq!(
        e.read("crusty-round-2.json.minor").lines().count(),
        1,
        "{}",
        show(&o)
    );
    assert!(e.fakes.gh_calls().is_empty());
}

#[test]
fn real_tools_use_the_one_keyword_predicate() {
    if skip() {
        return;
    }
    for (id, summary, want) in [
        (
            "pr-body-testing-section",
            "Rename the heading",
            "MINOR_ONLY",
        ),
        ("section-1", "Rename the heading", "MINOR_ONLY"),
        (
            "head-commit-wrong-author-identity",
            "Fix the author",
            "MINOR_ONLY",
        ),
        ("sec-3", "Rename the heading", "CONCERNS"),
        ("auth-bypass", "Rename the heading", "CONCERNS"),
        ("naming-1", "The token is printed in the log", "CONCERNS"),
        ("naming-1", "Two tokens are cached", "CONCERNS"),
    ] {
        let e = Env::new();
        e.light();
        let tools = real_tools(&e);
        use_gh(&e, SENTINEL_GH);
        let o = real_round(&e, &tools, 1, &json!([minor(id, summary)]));
        assert_eq!(verdict_of(&o), want, "{id} / {summary}: {}", show(&o));
        assert!(e.fakes.gh_calls().is_empty());
    }
}

#[test]
fn real_tools_a_moved_head_over_the_limit_escalates_and_the_round_is_still_classified() {
    if skip() {
        return;
    }
    let e = Env::new();
    e.light();
    let tools = real_tools(&e);
    e.repo.write("docs/big.md", &"line\n".repeat(400));
    let moved = e.repo.commit("big address");
    let ctx = json!({"pr": "7", "head_sha": moved, "resolved_concerns": "", "round_label": "crusty-round-1"})
        .to_string();
    let o = step03(
        &e,
        &tools,
        &crusty_review("CONCERNS", &two_minor()),
        &[
            ("RECIPE_VAR_crusty_round_context", &ctx),
            ("FAKE_GH_HEAD", &moved),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(stdout(&o), MINOR_ONLY_TWO, "{}", show(&o));
    assert!(!e.p("light-mode").exists(), "the marker is removed");
    let esc: Value = serde_json::from_str(&e.read("escalation.json")).expect("escalation.json");
    assert_eq!(esc["reason"], "in-loop-lines");
    assert_eq!(esc["head_sha"], moved.as_str());
    gh_reads_the_base_only(&e);
    no_retired_files(&e);

    // The next round finds no marker and is classified without a check.
    use_gh(&e, SENTINEL_GH);
    let rec2 = e.p("crusty-round-2.json").to_string_lossy().into_owned();
    let o = step03(
        &e,
        &tools,
        &crusty_review("CONCERNS", &two_minor()),
        &[
            ("RECIPE_VAR_crusty_round_context", &ctx),
            ("AUTODRIVE_ROUND_RECORD", &rec2),
        ],
    );
    assert_eq!(stdout(&o), MINOR_ONLY_TWO, "{}", show(&o));
}

#[test]
fn real_tools_an_end_to_end_light_drive_leaves_no_retired_file() {
    if skip() {
        return;
    }
    let e = Env::new();
    let tools = minor_only_light_round(&e);
    let calls = collect_and_file(&e, &tools);
    no_retired_files(&e);
    for c in &calls {
        assert!(
            !c.iter().any(|a| a.contains("auto-drive-light:pr-")),
            "no light marker search: {c:?}"
        );
        assert!(
            !(c.first().map(String::as_str) == Some("pr")
                && c.get(1).map(String::as_str) == Some("comment")),
            "no PR comment: {c:?}"
        );
    }
    for name in state_names(&e) {
        assert!(!name.ends_with(".md"), "stray body file {name}");
    }
}

// ===========================================================================
// 4. Acceptance: a light MINOR_ONLY record reaches the merge phase
// ===========================================================================

/// Writes the loop's manifest row and `crusty-latest.json` for the record,
/// as `autodrive_loop.sh` does, then runs `autodrive_record_crusty_loop_done`
/// (the DONE path) and `autodrive_crusty_final`.
fn loop_done_and_final(e: &Env) -> Output {
    loop_writes_round(e);
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
fn acceptance_a_minor_only_light_round_reaches_the_merge_phase() {
    if skip() {
        return;
    }
    let e = Env::new();
    minor_only_light_round(&e);
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
        "minor ids are not recorded as resolved: {resolved}"
    );
}
