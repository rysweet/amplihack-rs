//! Issue #1979: a crusty verdict with only minor findings ends the crusty
//! loop, and each minor finding is filed as a follow-up issue before READY.
//!
//! The contract is `docs/reference/auto-drive-to-merge.md`, section "Minor
//! findings end the crusty loop (#1979)":
//!
//! ```text
//! autodrive_followups.sh classify --record FILE --state-dir DIR   (concerns on stdin)
//!     -> "MINOR_ONLY" or nothing; writes <record>.minor and empties <record>.findings
//! autodrive_followups.sh collect  --state-dir DIR
//!     -> appends the terminal round's <record>.minor to followups.jsonl
//! autodrive_followups.sh file     --state-dir DIR --pr N [--task-id ID] [--task-text-file F]
//!     -> one `gh issue create` per id without a row in followups.tsv
//! autodrive_followups.sh check    --state-dir DIR --pr N
//!     -> "followups-ok <count>" (exit 0), or the failing token on stdout (exit 1)
//! ```
//!
//! Layers:
//!
//! 1. The tool, run directly with a fake `gh` (no network) and scratch under
//!    `$TMPDIR` only.
//! 2. The real recipe step bodies (crusty-round step-03 and step-06,
//!    crusty-loop step-03, merge-loop step-02b), extracted from the YAML and
//!    run in bash with a fake `amplihack` and the fake `gh`.
//! 3. `autodrive_crusty_final` accepting a `MINOR_ONLY` terminal record.
//! 4. Static guards: shared rules file, budgets, wording, wiring.
//!
//! The loop helper (`--also-clean-token`) and the merge gate are exercised
//! by `amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh`, which
//! cargo runs through `run_auto_drive_contract_shell_test`.
//!
//! Every behavioural test skips, with a note, when `jq` is not installed.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

use crate::support::auto_drive_light::{
    FAKE_AMPLIHACK, jq_available, private_dir, private_file, read, repo_root, show, stderr, stdout,
    step_command, tools_dir,
};

const TOOL: &str = "autodrive_followups.sh";
const RULES: &str = "autodrive_finding_rules.sh";
const PR: &str = "7";
const ZW: char = '\u{200b}';

fn skip() -> bool {
    if !jq_available() {
        eprintln!("skip: `jq` not available on PATH");
        return true;
    }
    false
}

/// A fake `gh`. Each call appends its argv (joined by U+001F) to
/// `$FAKE_LOG/gh.log`, and copies a `--body-file` to `$FAKE_LOG/body-<n>.md`.
///
/// - `pr view`      `{"number":7,"url":$FAKE_GH_PR_URL,"body":$FAKE_GH_PR_BODY}`,
///   with `--jq` applied by the real `jq`.
/// - `issue create` prints `https://github.com/o/r/issues/<101, 102, ...>`.
///   `$FAKE_GH_CREATE_EXIT` makes every create fail with that code;
///   `$FAKE_GH_CREATE_OK` lets only that many creates succeed first.
/// - anything else  nothing, exit 0.
const FAKE_GH: &str = r#"#!/usr/bin/env bash
set -u
log="${FAKE_LOG:?}"
n=$(( $(wc -l < "$log/gh.log" 2>/dev/null || echo 0) + 1 ))
( IFS=$'\x1f'; printf '%s\n' "$*" ) >> "$log/gh.log"
args=("$@"); jqf=""
for ((i=0; i<${#args[@]}; i++)); do
  case "${args[$i]}" in
    --jq|-q) jqf="${args[$((i+1))]:-}" ;;
    --body-file) cp -- "${args[$((i+1))]}" "$log/body-$n.md" 2>/dev/null ;;
  esac
done
emit() { if [ -n "$jqf" ]; then printf '%s' "$1" | jq -r "$jqf"; else printf '%s\n' "$1"; fi; }
case "${1:-} ${2:-}" in
  "pr view")
    jq -cn --arg u "${FAKE_GH_PR_URL-https://github.com/o/r/pull/7}" --arg b "${FAKE_GH_PR_BODY:-}" \
      '{number:7,url:$u,body:$b}' > "$log/pr.json"
    emit "$(cat "$log/pr.json")" ;;
  "issue create")
    made=$(grep -c . "$log/created" 2>/dev/null || true); made=${made:-0}
    if [ -n "${FAKE_GH_CREATE_EXIT:-}" ] || { [ -n "${FAKE_GH_CREATE_OK:-}" ] && [ "$made" -ge "$FAKE_GH_CREATE_OK" ]; }; then
      echo "HTTP 403: You have exceeded a secondary rate limit" >&2
      exit "${FAKE_GH_CREATE_EXIT:-1}"
    fi
    echo x >> "$log/created"
    printf 'https://github.com/o/r/issues/%s\n' "$((101 + made))" ;;
esac
exit 0
"#;

/// A scratch drive: a private state dir, a private TMPDIR, the fakes.
struct Drive {
    tmp: tempfile::TempDir,
    state: PathBuf,
    tmpdir: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

impl Drive {
    fn new() -> Self {
        // tempfile honours $TMPDIR; no literal /tmp path (#1978).
        let tmp = tempfile::Builder::new()
            .prefix("issue-1979-")
            .tempdir()
            .unwrap();
        let state = tmp.path().join("state");
        private_dir(&state);
        let tmpdir = tmp.path().join("private-tmp");
        private_dir(&tmpdir);
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let bin = tmp.path().join("fake-bin");
        let log = tmp.path().join("fake-log");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&log).unwrap();
        crate::support::exec_stub::write_executable(&bin.join("gh"), FAKE_GH, 0o755);
        crate::support::exec_stub::write_executable(&bin.join("amplihack"), FAKE_AMPLIHACK, 0o755);
        Drive {
            tmp,
            state,
            tmpdir,
            home,
            bin,
            log,
        }
    }

    fn p(&self, name: &str) -> PathBuf {
        self.state.join(name)
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.p(name)).unwrap_or_default()
    }

    fn path_env(&self) -> std::ffi::OsString {
        std::env::join_paths(
            std::iter::once(self.bin.clone()).chain(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            )),
        )
        .unwrap()
    }

    /// Runs `bash -c script` (or `bash args...`) isolated, fakes first on PATH.
    fn bash(
        &self,
        script: Option<&str>,
        args: &[&OsStr],
        env: &[(&str, &OsStr)],
        stdin: &str,
    ) -> Output {
        use std::io::Write;
        let mut c = crate::isolated_amplihack::isolated("bash");
        if let Some(s) = script {
            c.arg("-c").arg(s).arg("bash");
        }
        c.args(args)
            .current_dir(self.tmp.path())
            .env("HOME", &self.home)
            .env("PATH", self.path_env())
            .env("FAKE_LOG", &self.log)
            .env("TMPDIR", &self.tmpdir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("AMPLIHACK_CLAUDE_PROFILE_ROOT")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("BASH_ENV")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            c.env(k, v);
        }
        let mut child = c.spawn().expect("spawn bash");
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).ok();
        child.wait_with_output().expect("wait for bash")
    }

    fn tool(&self, args: &[&str], env: &[(&str, &str)], stdin: &str) -> Output {
        let t = tools_dir().join(TOOL);
        let mut argv: Vec<&OsStr> = vec![t.as_os_str()];
        argv.extend(args.iter().map(OsStr::new));
        let env: Vec<(&str, &OsStr)> = env.iter().map(|(k, v)| (*k, OsStr::new(*v))).collect();
        self.bash(None, &argv, &env, stdin)
    }

    fn record(&self, n: u32) -> PathBuf {
        self.p(&format!("crusty-round-{n}.json"))
    }

    fn classify(&self, n: u32, concerns: &str) -> Output {
        let rec = self.record(n);
        let st = self.state.clone();
        self.tool(
            &[
                "classify",
                "--record",
                rec.to_str().unwrap(),
                "--state-dir",
                st.to_str().unwrap(),
            ],
            &[],
            concerns,
        )
    }

    fn collect(&self) -> Output {
        let st = self.state.clone();
        self.tool(&["collect", "--state-dir", st.to_str().unwrap()], &[], "")
    }

    fn file(&self, extra: &[&str], env: &[(&str, &str)]) -> Output {
        let st = self.state.clone();
        let mut a = vec!["file", "--state-dir", st.to_str().unwrap(), "--pr", PR];
        a.extend_from_slice(extra);
        // Borrow-safe copy of the argument list.
        let a: Vec<String> = a.into_iter().map(String::from).collect();
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        self.tool(&a, env, "")
    }

    fn check(&self) -> Output {
        let st = self.state.clone();
        self.tool(
            &["check", "--state-dir", st.to_str().unwrap(), "--pr", PR],
            &[],
            "",
        )
    }

    fn gh_calls(&self) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.log.join("gh.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.split('\u{1f}').map(String::from).collect())
            .collect()
    }

    fn creates(&self) -> Vec<Vec<String>> {
        self.gh_calls()
            .into_iter()
            .filter(|c| {
                c.first().map(String::as_str) == Some("issue")
                    && c.get(1).map(String::as_str) == Some("create")
            })
            .collect()
    }

    fn bodies(&self) -> Vec<String> {
        let mut v: Vec<(usize, String)> = std::fs::read_dir(&self.log)
            .unwrap()
            .filter_map(|e| {
                let e = e.ok()?;
                let name = e.file_name().into_string().ok()?;
                let n = name
                    .strip_prefix("body-")?
                    .strip_suffix(".md")?
                    .parse()
                    .ok()?;
                Some((n, std::fs::read_to_string(e.path()).ok()?))
            })
            .collect();
        v.sort();
        v.into_iter().map(|(_, b)| b).collect()
    }

    fn tsv_rows(&self) -> Vec<Vec<String>> {
        self.read("followups.tsv")
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.split('\t').map(String::from).collect())
            .collect()
    }

    /// What autodrive_loop.sh does after a round: the record, a copy in
    /// crusty-latest.json, and a manifest row with the record's blob hash.
    fn loop_writes_round(&self, n: u32, verdict: &str, count: usize) {
        let label = format!("round-{n}");
        let name = format!("crusty-round-{n}.json");
        let sha = "a".repeat(40);
        let rec = format!(
            "{{\"crusty_verdict\":\"{verdict}\",\"concern_count\":{count},\"commits_this_round\":0,\"head_sha\":\"{sha}\",\"reviewed_head_sha\":\"{sha}\",\"round_label\":\"{label}\",\"test_signal\":\"\",\"ci_signal\":\"\"}}\n"
        );
        private_file(&self.p(&name), &rec);
        private_file(&self.p("crusty-latest.json"), &rec);
        let hash = blob_hash(&self.p(&name));
        let mut manifest = self.read("crusty-records.tsv");
        manifest.push_str(&format!("{label}\t{name}\t{hash}\n"));
        private_file(&self.p("crusty-records.tsv"), &manifest);
    }

    /// A `.minor` file beside round `n`'s record.
    fn minor(&self, n: u32, findings: &[(&str, &str, &str)]) {
        let body: String = findings
            .iter()
            .map(|(id, s, e)| format!("{}\n", json!({"id": id, "summary": s, "evidence": e})))
            .collect();
        private_file(&self.p(&format!("crusty-round-{n}.json.minor")), &body);
    }

    /// The follow-up set as `collect` leaves it.
    fn set(&self, findings: &[(&str, &str, &str)]) {
        let body: String = findings
            .iter()
            .map(|(id, s, e)| format!("{}\n", json!({"id": id, "summary": s, "evidence": e})))
            .collect();
        private_file(&self.p("followups.jsonl"), &body);
    }

    /// A drive whose crusty loop ended on a MINOR_ONLY round with `findings`,
    /// collected into the set.
    fn minor_drive(&self, findings: &[(&str, &str, &str)]) {
        self.loop_writes_round(1, "MINOR_ONLY", findings.len());
        self.minor(1, findings);
        self.set(findings);
    }
}

fn blob_hash(p: &Path) -> String {
    // From the state dir, with GIT_DIR and friends scrubbed, as the loop hashes it.
    let o = amplihack_git::command_in(p.parent().expect("record has a parent"))
        .args(["hash-object", "--no-filters"])
        .arg(p)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git hash-object");
    assert!(o.status.success());
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn concern(id: &str, severity: &str, summary: &str) -> Value {
    json!({"id": id, "severity": severity, "summary": summary, "evidence": format!("{id}.rs line 1")})
}

fn lines_of(s: &str) -> Vec<Value> {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON: {l}: {e}")))
        .collect()
}

fn two_minor() -> Value {
    json!([
        concern("naming-1", "minor", "Rename tmp_rec to record_copy"),
        concern("docs-typo-2", "minor", "Fix a typo in the header comment"),
    ])
}

/// Asserts classify did not decide MINOR_ONLY: empty stdout, no `.minor`.
fn assert_not_minor(d: &Drive, o: &Output, why: &str) {
    assert!(
        stdout(o).trim().is_empty(),
        "{why}: stdout must be empty\n{}",
        show(o)
    );
    assert!(
        !d.p("crusty-round-1.json.minor").exists(),
        "{why}: no .minor file may be written\n{}",
        show(o)
    );
}

// ===========================================================================
// 1. classify
// ===========================================================================

#[test]
fn classify_all_minor_prints_minor_only_and_writes_minor_file() {
    if skip() {
        return;
    }
    let d = Drive::new();
    // step-03 has already written the ids; classify must empty the file.
    private_file(
        &d.p("crusty-round-1.json.findings"),
        "naming-1\ndocs-typo-2\n",
    );
    let o = d.classify(1, &two_minor().to_string());
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(stdout(&o).trim(), "MINOR_ONLY", "{}", show(&o));
    let rows = lines_of(&d.read("crusty-round-1.json.minor"));
    assert_eq!(rows.len(), 2, "one line per finding");
    assert_eq!(rows[0]["id"], "naming-1");
    assert_eq!(rows[0]["summary"], "Rename tmp_rec to record_copy");
    assert_eq!(rows[1]["id"], "docs-typo-2");
    assert!(rows[0].get("evidence").is_some());
    assert_eq!(
        d.read("crusty-round-1.json.findings"),
        "",
        "the .findings file is emptied so minor ids never reach resolved-concerns.txt"
    );
    let mode = std::fs::metadata(d.p("crusty-round-1.json.minor"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, ".minor is private (umask 077): {mode:o}");
}

#[test]
fn classify_any_major_or_blocking_finding_is_not_minor_only() {
    if skip() {
        return;
    }
    for other in ["major", "blocking"] {
        let d = Drive::new();
        let c = json!([
            concern("naming-1", "minor", "Rename x"),
            concern("bug-2", other, "Wrong result")
        ]);
        let o = d.classify(1, &c.to_string());
        assert_not_minor(&d, &o, &format!("[minor, {other}]"));
    }
}

#[test]
fn classify_unknown_missing_or_miscased_severity_counts_as_major() {
    if skip() {
        return;
    }
    let cases = [
        json!([{"id": "a-1", "severity": "nit", "summary": "s", "evidence": "e"}]),
        json!([{"id": "a-1", "severity": "low", "summary": "s", "evidence": "e"}]),
        json!([{"id": "a-1", "severity": "Minor", "summary": "s", "evidence": "e"}]),
        json!([{"id": "a-1", "severity": "", "summary": "s", "evidence": "e"}]),
        json!([{"id": "a-1", "summary": "s", "evidence": "e"}]),
        json!([{"id": "a-1", "severity": null, "summary": "s", "evidence": "e"}]),
    ];
    for c in cases {
        let d = Drive::new();
        let o = d.classify(1, &c.to_string());
        assert_not_minor(&d, &o, &format!("severity case {c}"));
    }
}

#[test]
fn classify_fails_closed_on_bad_input() {
    if skip() {
        return;
    }
    for input in [
        "[]",
        "",
        "not json",
        "{\"id\":\"a\",\"severity\":\"minor\"}",
        "[\"minor\"]",
        "[1]",
        "[{\"id\":\"a\",\"severity\":\"minor\"}",
    ] {
        let d = Drive::new();
        let o = d.classify(1, input);
        assert_not_minor(&d, &o, &format!("input {input:?}"));
    }
}

#[test]
fn classify_multiple_json_values_on_stdin_is_not_minor_only() {
    if skip() {
        return;
    }
    let minor = json!([concern("naming-1", "minor", "Rename x")]);
    let blocking = json!([concern("bug-2", "blocking", "Crash on start")]);
    for input in [
        format!("{minor} {blocking}"),
        format!("{minor}\n{blocking}"),
        format!("{minor} {minor}"),
        format!("{minor} 1"),
    ] {
        let d = Drive::new();
        private_file(&d.p("crusty-round-1.json.findings"), "naming-1\nbug-2\n");
        let o = d.classify(1, &input);
        assert_not_minor(&d, &o, &format!("input {input:?}"));
        assert_eq!(
            d.read("crusty-round-1.json.findings"),
            "naming-1\nbug-2\n",
            ".findings is left as step-03 wrote it\n{}",
            show(&o)
        );
    }
}

#[test]
fn classify_invalid_json_logs_jq_error_and_stays_concerns() {
    if skip() {
        return;
    }
    let d = Drive::new();
    let o = d.classify(1, "[{\"id\":\"a\",\"severity\":\"minor\"}");
    assert_not_minor(&d, &o, "invalid JSON");
    let err = stderr(&o);
    assert!(
        err.contains("ERROR: classify: the concerns could not be read (jq rc=")
            && err.contains("the verdict stays CONCERNS"),
        "the jq failure is logged, not hidden\n{}",
        show(&o)
    );
    assert!(
        !err.contains("not every concern is a new minor finding"),
        "a jq failure is not reported as a CONCERNS decision\n{}",
        show(&o)
    );
}

#[test]
fn classify_rejects_unsafe_ids() {
    if skip() {
        return;
    }
    let long = "a".repeat(65);
    for id in [
        "has space",
        "semi;colon",
        "",
        "../up",
        long.as_str(),
        "$(id)",
    ] {
        let d = Drive::new();
        let o = d.classify(1, &json!([concern(id, "minor", "Rename x")]).to_string());
        assert_not_minor(&d, &o, &format!("id {id:?}"));
    }
    // 64 characters is still a valid id.
    let d = Drive::new();
    let ok = "b".repeat(64);
    let o = d.classify(1, &json!([concern(&ok, "minor", "Rename x")]).to_string());
    assert_eq!(
        stdout(&o).trim(),
        "MINOR_ONLY",
        "a 64-character id is valid\n{}",
        show(&o)
    );
}

#[test]
fn classify_rejects_minor_findings_with_security_keywords() {
    if skip() {
        return;
    }
    let cases = [
        // id keywords
        json!([{"id": "token-leak", "severity": "minor", "summary": "s", "evidence": "e"}]),
        json!([{"id": "auth-1", "severity": "minor", "summary": "s", "evidence": "e"}]),
        json!([{"id": "sec-3", "severity": "minor", "summary": "s", "evidence": "e"}]),
        // summary words
        json!([{"id": "n-1", "severity": "minor", "summary": "Logs the password on failure", "evidence": "e"}]),
        json!([{"id": "n-1", "severity": "minor", "summary": "Possible command injection", "evidence": "e"}]),
        // evidence words
        json!([{"id": "n-1", "severity": "minor", "summary": "Rename", "evidence": "credentials file is read twice"}]),
        json!([{"id": "n-1", "severity": "minor", "summary": "Rename", "evidence": "path traversal in join"}]),
    ];
    for c in cases {
        let d = Drive::new();
        let o = d.classify(1, &c.to_string());
        assert_not_minor(&d, &o, &format!("security case {c}"));
    }
    // Whole words only: "second" and "author" are not security words.
    let d = Drive::new();
    let c = json!([{"id": "n-1", "severity": "minor", "summary": "The second author line", "evidence": "e"}]);
    let o = d.classify(1, &c.to_string());
    assert_eq!(stdout(&o).trim(), "MINOR_ONLY", "{}", show(&o));
}

#[test]
fn classify_rejects_returning_ids() {
    if skip() {
        return;
    }
    // Listed in resolved-concerns.txt.
    let d = Drive::new();
    private_file(&d.p("resolved-concerns.txt"), "naming-1\n");
    let o = d.classify(
        2,
        &json!([concern("naming-1", "minor", "Rename x")]).to_string(),
    );
    assert!(
        stdout(&o).trim().is_empty(),
        "a resolved id returning is not minor-only\n{}",
        show(&o)
    );
    assert!(!d.p("crusty-round-2.json.minor").exists());

    // Listed in another round's .findings.
    let d = Drive::new();
    private_file(&d.p("crusty-round-1.json.findings"), "naming-1\n");
    let o = d.classify(
        2,
        &json!([concern("naming-1", "minor", "Rename x")]).to_string(),
    );
    assert!(
        stdout(&o).trim().is_empty(),
        "an id from an earlier round is not minor-only\n{}",
        show(&o)
    );
}

#[test]
fn classify_refuses_a_state_dir_that_is_not_private() {
    if skip() {
        return;
    }
    let d = Drive::new();
    std::fs::set_permissions(&d.state, std::fs::Permissions::from_mode(0o777)).unwrap();
    let o = d.classify(1, &two_minor().to_string());
    std::fs::set_permissions(&d.state, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_not_minor(&d, &o, "world-writable state dir");
}

#[test]
fn classify_clips_summary_and_evidence_to_200_characters() {
    if skip() {
        return;
    }
    let d = Drive::new();
    let long = "x".repeat(500);
    let c = json!([{"id": "n-1", "severity": "minor", "summary": long, "evidence": long}]);
    let o = d.classify(1, &c.to_string());
    assert_eq!(stdout(&o).trim(), "MINOR_ONLY", "{}", show(&o));
    let row = &lines_of(&d.read("crusty-round-1.json.minor"))[0];
    assert!(row["summary"].as_str().unwrap().chars().count() <= 200);
    assert!(row["evidence"].as_str().unwrap().chars().count() <= 200);
}

#[test]
fn bad_arguments_exit_2() {
    let d = Drive::new();
    for args in [
        vec![],
        vec!["nonsense"],
        vec!["check"],
        vec!["file", "--pr", "7"],
        vec!["check", "--state-dir"],
    ] {
        let o = d.tool(&args, &[], "");
        assert_eq!(o.status.code(), Some(2), "args {args:?}\n{}", show(&o));
    }
}

// ===========================================================================
// 2. collect
// ===========================================================================

#[test]
fn collect_reads_only_the_terminal_round() {
    if skip() {
        return;
    }
    let d = Drive::new();
    // Round 1 was minor-only with old-1, which round 2 no longer reports.
    d.loop_writes_round(1, "MINOR_ONLY", 1);
    d.minor(1, &[("old-1", "fixed later", "e")]);
    d.loop_writes_round(2, "MINOR_ONLY", 2);
    d.minor(
        2,
        &[("naming-1", "Rename", "e1"), ("docs-typo-2", "Typo", "e2")],
    );
    let o = d.collect();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let ids: Vec<String> = lines_of(&d.read("followups.jsonl"))
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        ids,
        vec!["naming-1", "docs-typo-2"],
        "only the round that ended the loop is filed"
    );
}

#[test]
fn collect_after_a_clean_round_adds_nothing() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.loop_writes_round(1, "MINOR_ONLY", 1);
    d.minor(1, &[("old-1", "fixed later", "e")]);
    d.loop_writes_round(2, "CLEAN", 0);
    let o = d.collect();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(
        lines_of(&d.read("followups.jsonl")).is_empty(),
        "a CLEAN end files nothing"
    );
}

#[test]
fn collect_refuses_a_record_that_no_longer_matches_the_manifest() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.loop_writes_round(1, "MINOR_ONLY", 1);
    d.minor(1, &[("naming-1", "Rename", "e")]);
    // An agent rewrote the record after the loop hashed it.
    let forged = d
        .read("crusty-round-1.json")
        .replace("\"concern_count\":1", "\"concern_count\":2");
    private_file(&d.record(1), &forged);
    let o = d.collect();
    assert_ne!(o.status.code(), Some(0), "{}", show(&o));
    assert!(
        lines_of(&d.read("followups.jsonl")).is_empty(),
        "nothing is collected from a modified record"
    );
}

#[test]
fn collect_fails_on_a_bad_minor_line_instead_of_dropping_it() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.loop_writes_round(1, "MINOR_ONLY", 2);
    private_file(
        &d.p("crusty-round-1.json.minor"),
        "{\"id\":\"naming-1\",\"summary\":\"s\",\"evidence\":\"e\"}\n{\"id\":\"bad id;\",\"summary\":\"s\",\"evidence\":\"e\"}\n",
    );
    let o = d.collect();
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(
        stderr(&o).contains("ERROR: followup-set-invalid"),
        "{}",
        show(&o)
    );
}

#[test]
fn collect_keeps_the_first_occurrence_of_an_id() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.set(&[("naming-1", "first summary", "e")]);
    d.loop_writes_round(1, "MINOR_ONLY", 2);
    d.minor(
        1,
        &[
            ("naming-1", "second summary", "e"),
            ("docs-typo-2", "Typo", "e"),
        ],
    );
    assert_eq!(d.collect().status.code(), Some(0));
    assert_eq!(d.collect().status.code(), Some(0), "collect is repeatable");
    let rows = lines_of(&d.read("followups.jsonl"));
    assert_eq!(rows.len(), 2, "deduplicated by id: {rows:?}");
    assert_eq!(rows[0]["summary"], "first summary");
}

// ===========================================================================
// 3. file
// ===========================================================================

fn three() -> [(&'static str, &'static str, &'static str); 3] {
    [
        (
            "naming-1",
            "Rename tmp_rec to record_copy",
            "autodrive_last_round.sh line 41",
        ),
        (
            "docs-typo-2",
            "Fix a typo in the header",
            "README.md line 3",
        ),
        (
            "style-3",
            "Join two short comment lines",
            "autodrive_loop.sh line 12",
        ),
    ]
}

fn arg_after<'a>(call: &'a [String], flag: &str) -> Option<&'a str> {
    call.iter()
        .position(|a| a == flag)
        .and_then(|i| call.get(i + 1))
        .map(String::as_str)
}

#[test]
fn file_creates_one_issue_per_minor_finding_naming_the_task() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&three());
    let o = d.file(&["--task-id", "1979"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let creates = d.creates();
    assert_eq!(
        creates.len(),
        3,
        "one gh issue create per finding: {creates:?}"
    );
    for (call, (_, summary, _)) in creates.iter().zip(three()) {
        assert_eq!(
            arg_after(call, "--repo"),
            Some("o/r"),
            "explicit --repo from the PR URL: {call:?}"
        );
        let title = arg_after(call, "--title").expect("--title");
        assert!(
            title.starts_with("Follow-up (#1979): "),
            "title names the task: {title}"
        );
        assert!(title.contains(summary), "{title}");
        assert!(title.chars().count() <= 80, "title clipped to 80: {title}");
        assert!(
            call.iter().any(|a| a == "--body-file"),
            "body goes through --body-file: {call:?}"
        );
        assert!(
            !call.iter().any(|a| a == "--label" || a == "--assignee"),
            "no labels or assignees: {call:?}"
        );
    }
    let bodies = d.bodies();
    assert_eq!(bodies.len(), 3);
    for (b, (id, _, _)) in bodies.iter().zip(three()) {
        assert!(b.contains("Task: #1979"), "{b}");
        assert!(
            b.contains(&format!("Finding: {id} (severity: minor)")),
            "{b}"
        );
        assert!(b.contains("PR #7"), "{b}");
        assert!(
            b.contains(&format!(
                "<!-- autodrive-followup task=#1979 finding={id} -->"
            )),
            "{b}"
        );
    }
    let rows = d.tsv_rows();
    assert_eq!(
        rows,
        vec![
            vec!["naming-1", "101", "#1979", "7"],
            vec!["docs-typo-2", "102", "#1979", "7"],
            vec!["style-3", "103", "#1979", "7"],
        ],
        "one tsv row per issue"
    );
    assert!(
        stdout(&o).contains("INFO: followup naming-1 -> #101 (task #1979, PR #7)")
            || stderr(&o).contains("INFO: followup naming-1 -> #101 (task #1979, PR #7)"),
        "{}",
        show(&o)
    );
    assert!(!d.p("followups.error").exists());
    let mode = std::fs::metadata(d.p("followups.tsv"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "followups.tsv is private: {mode:o}");
}

#[test]
fn file_rerun_files_nothing_new() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&three());
    assert_eq!(d.file(&["--task-id", "1979"], &[]).status.code(), Some(0));
    let o = d.file(&["--task-id", "1979"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(d.creates().len(), 3, "a rerun makes no new create call");
    assert_eq!(d.tsv_rows().len(), 3);
}

#[test]
fn file_fails_when_gh_cannot_create_the_issue() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&three());
    let o = d.file(&["--task-id", "1979"], &[("FAKE_GH_CREATE_EXIT", "1")]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(
        stderr(&o).contains("ERROR: followup-create-failed"),
        "{}",
        show(&o)
    );
    assert_eq!(d.read("followups.error").trim(), "followup-create-failed");
    assert!(d.tsv_rows().is_empty(), "no row without an issue");
    // The echoed gh stderr is one sanitised line.
    assert!(stderr(&o).contains("secondary rate limit"), "{}", show(&o));
}

#[test]
fn file_partial_failure_then_rerun_files_only_the_rest_and_clears_the_error() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&three());
    let o = d.file(&["--task-id", "1979"], &[("FAKE_GH_CREATE_OK", "1")]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert_eq!(
        d.tsv_rows().len(),
        1,
        "the issue that was created keeps its row"
    );
    assert!(d.p("followups.error").exists());
    let o = d.file(&["--task-id", "1979"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(d.tsv_rows().len(), 3);
    assert!(
        !d.p("followups.error").exists(),
        "a run that files every id removes followups.error"
    );
    let ok_creates = std::fs::read_to_string(d.log.join("created"))
        .unwrap()
        .lines()
        .count();
    assert_eq!(ok_creates, 3, "naming-1 was not filed twice");
}

#[test]
fn file_without_a_task_id_fails_and_files_nothing() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&three());
    let task = d.tmp.path().join("task.txt");
    std::fs::write(&task, "Tidy the loop; no issue reference here.").unwrap();
    let o = d.file(
        &["--task-text-file", task.to_str().unwrap()],
        &[(
            "FAKE_GH_PR_BODY",
            "A PR body with no closing keyword. See #12.",
        )],
    );
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(
        stderr(&o).contains("ERROR: followup-task-unresolved"),
        "{}",
        show(&o)
    );
    assert_eq!(d.read("followups.error").trim(), "followup-task-unresolved");
    assert!(d.creates().is_empty(), "no issue without a task id");
}

#[test]
fn file_rejects_invalid_task_ids() {
    if skip() {
        return;
    }
    let long = "y".repeat(65);
    for bad in ["#0", "0", "a b", "x;rm", "`id`", "$(id)", long.as_str()] {
        let d = Drive::new();
        d.minor_drive(&three()[..1]);
        let o = d.file(&["--task-id", bad], &[]);
        assert_eq!(o.status.code(), Some(1), "task id {bad:?}\n{}", show(&o));
        assert!(d.creates().is_empty(), "task id {bad:?} filed an issue");
    }
}

/// Arguments, environment, task text and the task id `file` must record.
type TaskCase = (
    Vec<&'static str>,
    Vec<(&'static str, &'static str)>,
    Option<&'static str>,
    &'static str,
);

#[test]
fn file_task_id_forms_and_fallbacks() {
    if skip() {
        return;
    }
    let cases: Vec<TaskCase> = vec![
        (vec!["--task-id", "#1979"], vec![], None, "#1979"),
        (
            vec!["--task-id", "plan/17.104"],
            vec![],
            None,
            "plan/17.104",
        ),
        // PR body, second in order.
        (
            vec![],
            vec![("FAKE_GH_PR_BODY", "Summary.\n\nCloses #1979\nFixes #5")],
            None,
            "#1979",
        ),
        // Task description, third in order.
        (vec![], vec![], Some("Implement this.\nResolves #42"), "#42"),
        // plan_task_id wins over the PR body.
        (
            vec!["--task-id", "plan/x"],
            vec![("FAKE_GH_PR_BODY", "Closes #1979")],
            None,
            "plan/x",
        ),
        // PR body wins over the task text.
        (
            vec![],
            vec![("FAKE_GH_PR_BODY", "Fixes #8")],
            Some("Resolves #42"),
            "#8",
        ),
    ];
    for (args, env, task_text, want) in cases {
        let d = Drive::new();
        d.minor_drive(&three()[..1]);
        let task = d.tmp.path().join("task.txt");
        let mut a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        if let Some(t) = task_text {
            std::fs::write(&task, t).unwrap();
            a.push("--task-text-file".into());
            a.push(task.to_str().unwrap().into());
        }
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        let o = d.file(&a, &env);
        assert_eq!(
            o.status.code(),
            Some(0),
            "case {args:?} {env:?} {task_text:?}\n{}",
            show(&o)
        );
        assert_eq!(
            d.tsv_rows()[0][2],
            want,
            "case {args:?} {env:?} {task_text:?}"
        );
        let title = arg_after(&d.creates()[0], "--title").unwrap().to_string();
        assert!(
            title.starts_with(&format!("Follow-up ({want}): ")),
            "{title}"
        );
    }
}

#[test]
fn file_with_an_empty_set_files_nothing_and_needs_no_task() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.loop_writes_round(1, "CLEAN", 0);
    let o = d.file(&[], &[]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "a CLEAN drive needs no task id\n{}",
        show(&o)
    );
    assert!(d.creates().is_empty());
    assert!(d.tsv_rows().is_empty());
}

#[test]
fn file_passes_exit_79_through_without_retry() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&three());
    let o = d.file(&["--task-id", "1979"], &[("FAKE_GH_CREATE_EXIT", "79")]);
    assert_eq!(o.status.code(), Some(79), "{}", show(&o));
    assert_eq!(
        d.creates().len(),
        1,
        "exactly one create call, never retried"
    );
}

#[test]
fn file_refuses_an_unparseable_pr_url() {
    if skip() {
        return;
    }
    for url in [
        "",
        "not-a-url",
        "https://example.com/o/r/pull/7",
        "https://github.com/o/r/pull/7; rm -rf x",
        "https://github.com/o/r/issues/7",
    ] {
        let d = Drive::new();
        d.minor_drive(&three()[..1]);
        let o = d.file(&["--task-id", "1979"], &[("FAKE_GH_PR_URL", url)]);
        assert_eq!(o.status.code(), Some(1), "url {url:?}\n{}", show(&o));
        assert!(
            stderr(&o).contains("ERROR: followup-repo-unresolved"),
            "url {url:?}\n{}",
            show(&o)
        );
        assert!(d.creates().is_empty(), "url {url:?} filed an issue");
    }
}

#[test]
fn file_body_neutralises_mentions_issue_links_and_urls() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&[(
        "naming-1",
        "Ask @octocat about #123",
        "see https://github.com/evil/repo and <script>|`x`",
    )]);
    let o = d.file(&["--task-id", "1979"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let body = &d.bodies()[0];
    assert!(
        !body.contains("@octocat"),
        "mention not neutralised: {body}"
    );
    assert!(body.contains(&format!("@{ZW}octocat")), "{body}");
    assert!(!body.contains("#123"), "issue link not neutralised: {body}");
    assert!(
        !body.contains("github.com/evil"),
        "url not neutralised: {body}"
    );
    assert!(!body.contains("<script>"), "HTML not escaped: {body}");
    assert!(
        body.contains("Task: #1979"),
        "the task line is the one real link: {body}"
    );
}

#[test]
fn file_title_keeps_plain_characters_as_one_argument() {
    if skip() {
        return;
    }
    let d = Drive::new();
    let summary = "a & b | `c` $(touch pwned) ; echo";
    d.minor_drive(&[("naming-1", summary, "e")]);
    let o = d.file(&["--task-id", "1979"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let call = &d.creates()[0];
    let title = arg_after(call, "--title").unwrap();
    assert_eq!(
        title,
        format!("Follow-up (#1979): {summary}"),
        "title is plain text, one argv element"
    );
    assert!(
        !title.contains("&amp;") && !title.contains("&#x"),
        "no HTML entities in the title: {title}"
    );
    assert!(
        !d.tmp.path().join("pwned").exists() && !d.state.join("pwned").exists(),
        "nothing was executed"
    );
}

#[test]
fn file_title_breaks_mentions_and_issue_references() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&[("naming-1", "Ask @user about #123 and GH-45", "e")]);
    let o = d.file(&["--task-id", "1979"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let title = arg_after(&d.creates()[0], "--title").unwrap().to_string();
    assert!(title.starts_with("Follow-up (#1979): "), "{title}");
    let summary = &title["Follow-up (#1979): ".len()..];
    for raw in ["@user", "#1", "GH-4"] {
        assert!(!summary.contains(raw), "{raw} left unbroken: {title}");
    }
    assert!(summary.contains(&format!("GH-{ZW}45")), "{title}");
    assert!(
        !title.contains("&amp;") && !title.contains("&#x"),
        "{title}"
    );
}

#[test]
fn cell_breaks_gh_issue_references() {
    let o = Command::new("bash")
        .arg("-c")
        .arg(r#". "$1/workflow_issue_tracking.sh" && . "$1/autodrive_finding_rules.sh" && cell 'see GH-45 and gh-7, not GHz-1'"#)
        .arg("bash")
        .arg(tools_dir())
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", show(&o));
    let out = stdout(&o);
    assert_eq!(
        out,
        format!("see GH-{ZW}45 and gh-{ZW}7, not GHz-1"),
        "{}",
        show(&o)
    );
}

#[test]
fn file_refuses_a_symlinked_tsv() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&three()[..1]);
    let elsewhere = d.tmp.path().join("elsewhere.tsv");
    std::fs::write(&elsewhere, "").unwrap();
    std::os::unix::fs::symlink(&elsewhere, d.p("followups.tsv")).unwrap();
    let o = d.file(&["--task-id", "1979"], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(stderr(&o).contains("followup-state-unsafe"), "{}", show(&o));
    assert!(d.creates().is_empty());
    assert_eq!(
        std::fs::read_to_string(&elsewhere).unwrap(),
        "",
        "the link target is not written"
    );
}

// ===========================================================================
// 4. check
// ===========================================================================

fn tsv(d: &Drive, rows: &[&str]) {
    let body: String = rows.iter().map(|r| format!("{r}\n")).collect();
    private_file(&d.p("followups.tsv"), &body);
}

#[test]
fn check_passes_when_every_finding_has_an_issue() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&three()[..2]);
    tsv(
        &d,
        &["naming-1\t101\t#1979\t7", "docs-typo-2\t102\t#1979\t7"],
    );
    let o = d.check();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(stdout(&o).trim(), "followups-ok 2");
    assert!(d.gh_calls().is_empty(), "check makes no network call");
}

#[test]
fn check_passes_a_clean_drive_with_no_set() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.loop_writes_round(1, "CLEAN", 0);
    let o = d.check();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(stdout(&o).trim(), "followups-ok 0");
}

#[test]
fn check_fails_on_a_missing_row() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&three()[..2]);
    tsv(&d, &["naming-1\t101\t#1979\t7"]);
    let o = d.check();
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(
        stdout(&o).contains("followup-missing:docs-typo-2"),
        "{}",
        show(&o)
    );
}

#[test]
fn check_fails_when_an_error_is_recorded() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.minor_drive(&three()[..1]);
    tsv(&d, &["naming-1\t101\t#1979\t7"]);
    private_file(&d.p("followups.error"), "followup-create-failed\n");
    let o = d.check();
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(
        stdout(&o).contains("followup-error-recorded"),
        "{}",
        show(&o)
    );
}

#[test]
fn check_fails_on_minor_only_with_an_empty_set() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.loop_writes_round(1, "MINOR_ONLY", 2);
    let o = d.check();
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(stdout(&o).contains("followup-set-empty"), "{}", show(&o));
}

#[test]
fn check_fails_on_malformed_rows() {
    if skip() {
        return;
    }
    for row in [
        "naming-1\t0\t#1979\t7",
        "naming-1\tabc\t#1979\t7",
        "naming-1\t101\t#1979",
        "naming-1\t101\t#1979\t7\textra",
        "naming-1\t101\t#1979\t8",
        "naming-1\t101\ta b\t7",
        "bad id\t101\t#1979\t7",
        "naming-1\t0101\t#1979\t7",
    ] {
        let d = Drive::new();
        d.minor_drive(&three()[..1]);
        tsv(&d, &[row]);
        let o = d.check();
        assert_eq!(o.status.code(), Some(1), "row {row:?}\n{}", show(&o));
        assert!(
            stdout(&o).contains("followup-row-malformed"),
            "row {row:?}\n{}",
            show(&o)
        );
    }
}

#[test]
fn check_refuses_unsafe_state_files() {
    if skip() {
        return;
    }
    // Group-writable tsv.
    let d = Drive::new();
    d.minor_drive(&three()[..1]);
    tsv(&d, &["naming-1\t101\t#1979\t7"]);
    std::fs::set_permissions(d.p("followups.tsv"), std::fs::Permissions::from_mode(0o620)).unwrap();
    let o = d.check();
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(stdout(&o).contains("followup-state-unsafe"), "{}", show(&o));

    // Symlinked tsv.
    let d = Drive::new();
    d.minor_drive(&three()[..1]);
    let elsewhere = d.tmp.path().join("forged.tsv");
    std::fs::write(&elsewhere, "naming-1\t101\t#1979\t7\n").unwrap();
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&elsewhere, d.p("followups.tsv")).unwrap();
    let o = d.check();
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(stdout(&o).contains("followup-state-unsafe"), "{}", show(&o));

    // World-writable set.
    let d = Drive::new();
    d.minor_drive(&three()[..1]);
    tsv(&d, &["naming-1\t101\t#1979\t7"]);
    std::fs::set_permissions(
        d.p("followups.jsonl"),
        std::fs::Permissions::from_mode(0o606),
    )
    .unwrap();
    let o = d.check();
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(stdout(&o).contains("followup-state-unsafe"), "{}", show(&o));
}

// ===========================================================================
// 5. autodrive_crusty_final accepts MINOR_ONLY (criterion 3)
// ===========================================================================

fn crusty_final(d: &Drive) -> Output {
    let helper = tools_dir().join("autodrive_state.sh");
    let script = format!(
        ". {:?} && autodrive_crusty_final \"$1\"",
        helper.display().to_string()
    );
    d.bash(Some(&script), &[d.state.as_os_str()], &[], "")
}

#[test]
fn crusty_final_accepts_a_minor_only_terminal_record() {
    let d = Drive::new();
    private_file(&d.p("phases.tsv"), "crusty-loop\t2026-11-05T00:00:00Z\n");
    d.loop_writes_round(1, "MINOR_ONLY", 2);
    let o = crusty_final(&d);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(stdout(&o).trim(), "a".repeat(40));
}

#[test]
fn crusty_final_still_rejects_a_modified_minor_only_record() {
    let d = Drive::new();
    private_file(&d.p("phases.tsv"), "crusty-loop\t2026-11-05T00:00:00Z\n");
    d.loop_writes_round(1, "CONCERNS", 2);
    // Rewritten to MINOR_ONLY after the loop hashed it.
    let forged = d
        .read("crusty-round-1.json")
        .replace("CONCERNS", "MINOR_ONLY");
    private_file(&d.record(1), &forged);
    private_file(&d.p("crusty-latest.json"), &forged);
    let o = crusty_final(&d);
    assert_ne!(o.status.code(), Some(0), "{}", show(&o));
    assert!(
        stdout(&o).contains("crusty-record-modified"),
        "{}",
        show(&o)
    );
}

#[test]
fn crusty_final_still_rejects_concerns() {
    let d = Drive::new();
    private_file(&d.p("phases.tsv"), "crusty-loop\t2026-11-05T00:00:00Z\n");
    d.loop_writes_round(1, "CONCERNS", 2);
    let o = crusty_final(&d);
    assert_ne!(o.status.code(), Some(0), "{}", show(&o));
    assert!(stdout(&o).contains("crusty-not-clean"), "{}", show(&o));
}

// ===========================================================================
// 6. Recipe step bodies
// ===========================================================================

const ROUND: &str = "autodrive-crusty-round";

fn step03(d: &Drive, review: &str) -> Output {
    let script = step_command(ROUND, "step-03-extract-crusty-verdict");
    let tools = tools_dir();
    let rec = d.record(1);
    let env: Vec<(&str, &OsStr)> = vec![
        ("RECIPE_VAR_crusty_review", OsStr::new(review)),
        ("AUTODRIVE_ROUND_RECORD", rec.as_os_str()),
        ("AUTODRIVE_ROUND_LABEL", OsStr::new("round-1")),
        ("AUTODRIVE_LAUNCH_TOOLS", tools.as_os_str()),
    ];
    d.bash(Some(&script), &[], &env, "")
}

fn last_json(o: &Output) -> Value {
    let s = stdout(o);
    let line = s
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON line\n{}", show(o)));
    serde_json::from_str(line).unwrap()
}

fn review(verdict: &str, concerns: &Value) -> String {
    format!(
        "My review.\n\n{}\n",
        json!({"crusty_verdict": verdict, "concerns": concerns})
    )
}

#[test]
fn round_step03_reports_minor_only_with_the_minor_count() {
    if skip() {
        return;
    }
    let d = Drive::new();
    let o = step03(&d, &review("CONCERNS", &two_minor()));
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let j = last_json(&o);
    assert_eq!(j["crusty_verdict"], "MINOR_ONLY", "{}", show(&o));
    assert_eq!(
        j["concern_count"],
        2,
        "the count is the number of minor findings, not 0: {}",
        show(&o)
    );
    assert_eq!(lines_of(&d.read("crusty-round-1.json.minor")).len(), 2);
    assert_eq!(d.read("crusty-round-1.json.findings"), "");
}

#[test]
fn round_step03_keeps_concerns_for_mixed_and_unknown_severities() {
    if skip() {
        return;
    }
    for concerns in [
        json!([
            concern("naming-1", "minor", "Rename"),
            concern("bug-2", "major", "Wrong")
        ]),
        json!([
            concern("naming-1", "minor", "Rename"),
            concern("bug-2", "blocking", "Crash")
        ]),
        json!([concern("naming-1", "nit", "Rename")]),
        json!([{"id": "naming-1", "summary": "Rename", "evidence": "e"}]),
        json!([]),
    ] {
        let d = Drive::new();
        let o = step03(&d, &review("CONCERNS", &concerns));
        let j = last_json(&o);
        assert_eq!(j["crusty_verdict"], "CONCERNS", "{concerns}\n{}", show(&o));
        assert!(!d.p("crusty-round-1.json.minor").exists(), "{concerns}");
    }
}

#[test]
fn round_step03_merges_concerns_given_as_a_string_of_two_arrays() {
    if skip() {
        return;
    }
    let d = Drive::new();
    let minor = json!([concern("naming-1", "minor", "Rename x")]);
    let blocking = json!([concern("bug-2", "blocking", "Crash on start")]);
    let o = step03(
        &d,
        &review("CONCERNS", &Value::String(format!("{minor} {blocking}"))),
    );
    let j = last_json(&o);
    assert_eq!(j["crusty_verdict"], "CONCERNS", "{}", show(&o));
    assert!(!d.p("crusty-round-1.json.minor").exists(), "{}", show(&o));
    assert_eq!(
        d.read("crusty-round-1.json.findings"),
        "naming-1\nbug-2\n",
        "the hidden blocking finding is kept\n{}",
        show(&o)
    );
}

#[test]
fn round_step03_normalises_concerns_with_jq_slurp() {
    let s03 = step_command(ROUND, "step-03-extract-crusty-verdict");
    assert!(
        s03.contains(
            r#"jq -cs 'if length > 0 and all(.[]; type == "array") then add else [] end'"#
        ),
        "step-03 merges every concerns array into one"
    );
}

#[test]
fn round_step03_ignores_a_minor_only_verdict_written_by_crusty() {
    if skip() {
        return;
    }
    // Crusty emits only CLEAN or CONCERNS; MINOR_ONLY from the agent is unrecognised.
    let d = Drive::new();
    let o = step03(&d, &review("MINOR_ONLY", &two_minor()));
    let j = last_json(&o);
    assert_eq!(j["verdict_source"], "unparseable_verdict", "{}", show(&o));
    assert_eq!(j["crusty_verdict"], "CONCERNS", "{}", show(&o));
}

#[test]
fn round_step03_without_the_tool_keeps_concerns() {
    if skip() {
        return;
    }
    let d = Drive::new();
    let empty_tools = d.tmp.path().join("no-tools");
    std::fs::create_dir_all(&empty_tools).unwrap();
    let script = step_command(ROUND, "step-03-extract-crusty-verdict");
    let rec = d.record(1);
    let rv = review("CONCERNS", &two_minor());
    let env: Vec<(&str, &OsStr)> = vec![
        ("RECIPE_VAR_crusty_review", OsStr::new(&rv)),
        ("AUTODRIVE_ROUND_RECORD", rec.as_os_str()),
        ("AUTODRIVE_LAUNCH_TOOLS", empty_tools.as_os_str()),
    ];
    let o = d.bash(Some(&script), &[], &env, "");
    assert_eq!(last_json(&o)["crusty_verdict"], "CONCERNS", "{}", show(&o));
}

#[test]
fn round_step06_records_minor_only() {
    if skip() {
        return;
    }
    let d = Drive::new();
    let script = step_command(ROUND, "step-06-write-round-record");
    let rec = d.record(1);
    let ctx = json!({"pr": "7", "head_sha": "a".repeat(40)}).to_string();
    let env: Vec<(&str, &OsStr)> = vec![
        (
            "RECIPE_VAR_crusty_verdict",
            OsStr::new(
                "{\"crusty_verdict\":\"MINOR_ONLY\",\"verdict_source\":\"crusty\",\"concern_count\":2}",
            ),
        ),
        ("RECIPE_VAR_crusty_round_context", OsStr::new(&ctx)),
        ("AUTODRIVE_ROUND_RECORD", rec.as_os_str()),
        ("AUTODRIVE_ROUND_LABEL", OsStr::new("round-1")),
    ];
    let o = d.bash(Some(&script), &[], &env, "");
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let r: Value = serde_json::from_str(d.read("crusty-round-1.json").trim()).unwrap();
    assert_eq!(r["crusty_verdict"], "MINOR_ONLY");
    assert_eq!(r["concern_count"], 2);
}

/// Runs crusty-loop step-03 on a drive whose loop ended DONE.
fn loop_step03(d: &Drive, plan_task_id: &str, env: &[(&str, &str)]) -> Output {
    let script = step_command("autodrive-crusty-loop", "step-03-record-crusty-phase");
    let root = repo_root();
    let tools = tools_dir();
    let pre = json!({"should_run": "true", "pr": PR, "state_dir": d.state}).to_string();
    let mut v: Vec<(&str, &OsStr)> = vec![
        ("RECIPE_VAR_crusty_loop_preflight", OsStr::new(&pre)),
        (
            "RECIPE_VAR_crusty_loop_result",
            OsStr::new("{\"loop_result\":\"DONE\"}"),
        ),
        ("RECIPE_VAR_plan_task_id", OsStr::new(plan_task_id)),
        (
            "RECIPE_VAR_task_description",
            OsStr::new("Implement the thing"),
        ),
        ("RECIPE_VAR_pr_number", OsStr::new(PR)),
        ("AMPLIHACK_HOME", root.as_os_str()),
        ("AUTODRIVE_LAUNCH_TOOLS", tools.as_os_str()),
        ("REPO_PATH", d.tmp.path().as_os_str()),
    ];
    v.extend(env.iter().map(|(k, val)| (*k, OsStr::new(*val))));
    d.bash(Some(&script), &[], &v, "")
}

fn marker_written(d: &Drive) -> bool {
    d.read("phases.tsv")
        .lines()
        .any(|l| l.starts_with("crusty-loop\t"))
}

#[test]
fn crusty_loop_step03_files_followups_before_the_marker() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.loop_writes_round(1, "MINOR_ONLY", 2);
    d.minor(1, &three()[..2]);
    let o = loop_step03(&d, "1979", &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(d.creates().len(), 2, "{}", show(&o));
    assert_eq!(d.tsv_rows().len(), 2);
    assert!(
        marker_written(&d),
        "the crusty-loop marker is written after filing\n{}",
        show(&o)
    );
    assert!(
        !d.read("resolved-concerns.txt").contains("naming-1"),
        "minor ids are not marked resolved"
    );
}

#[test]
fn crusty_loop_step03_writes_no_marker_when_filing_fails() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.loop_writes_round(1, "MINOR_ONLY", 2);
    d.minor(1, &three()[..2]);
    let o = loop_step03(&d, "1979", &[("FAKE_GH_CREATE_EXIT", "1")]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(
        !marker_written(&d),
        "no marker: the drive cannot reach READY\n{}",
        show(&o)
    );
}

#[test]
fn crusty_loop_step03_writes_no_marker_without_a_task_id() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.loop_writes_round(1, "MINOR_ONLY", 1);
    d.minor(1, &three()[..1]);
    let o = loop_step03(&d, "", &[]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert!(!marker_written(&d), "{}", show(&o));
    assert!(d.creates().is_empty());
}

#[test]
fn crusty_loop_step03_clean_files_nothing_and_needs_no_task() {
    if skip() {
        return;
    }
    let d = Drive::new();
    d.loop_writes_round(1, "CLEAN", 0);
    let o = loop_step03(&d, "", &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(d.creates().is_empty());
    assert!(marker_written(&d), "{}", show(&o));
}

#[test]
fn merge_loop_step02b_files_and_fails_the_step_on_error() {
    if skip() {
        return;
    }
    let script = step_command("autodrive-merge-loop", "step-02b-file-followups");
    let root = repo_root();
    let tools = tools_dir();
    for (fail, want_rc) in [(false, 0), (true, 1)] {
        let d = Drive::new();
        d.minor_drive(&three()[..1]);
        let pre = json!({"should_run": "true", "pr": PR, "state_dir": d.state}).to_string();
        let mut v: Vec<(&str, &OsStr)> = vec![
            ("RECIPE_VAR_merge_loop_preflight", OsStr::new(&pre)),
            ("RECIPE_VAR_plan_task_id", OsStr::new("1979")),
            (
                "RECIPE_VAR_task_description",
                OsStr::new("Implement the thing"),
            ),
            ("AMPLIHACK_HOME", root.as_os_str()),
            ("AUTODRIVE_LAUNCH_TOOLS", tools.as_os_str()),
            ("REPO_PATH", d.tmp.path().as_os_str()),
        ];
        if fail {
            v.push(("FAKE_GH_CREATE_EXIT", OsStr::new("1")));
        }
        let o = d.bash(Some(&script), &[], &v, "");
        assert_eq!(o.status.code(), Some(want_rc), "fail={fail}\n{}", show(&o));
        assert_eq!(d.tsv_rows().len(), if fail { 0 } else { 1 });
    }
}

// ===========================================================================
// 7. Static guards
// ===========================================================================

fn recipe_yaml(name: &str) -> serde_yaml::Value {
    serde_yaml::from_str(&read(&format!("amplifier-bundle/recipes/{name}.yaml"))).unwrap()
}

fn steps(name: &str) -> Vec<serde_yaml::Value> {
    recipe_yaml(name)["steps"].as_sequence().unwrap().clone()
}

fn step(name: &str, id: &str) -> serde_yaml::Value {
    steps(name)
        .into_iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("{name}: no step {id}"))
}

#[test]
fn new_files_exist_and_the_tool_is_executable() {
    let t = tools_dir().join(TOOL);
    let mode = std::fs::metadata(&t)
        .unwrap_or_else(|e| panic!("{}: {e}", t.display()))
        .permissions()
        .mode();
    assert!(mode & 0o111 != 0, "{TOOL} is executable");
    assert!(tools_dir().join(RULES).is_file(), "{RULES} exists");
}

#[test]
fn finding_rules_are_defined_once_and_sourced_by_both_rules() {
    let rules = read(&format!("amplifier-bundle/tools/{RULES}"));
    for def in ["KEYWORDS=", "TEXT_KEYWORDS=", "ZW=", "cell()"] {
        assert!(rules.contains(def), "{RULES} defines {def}");
    }
    for user in ["autodrive_light_round.sh", TOOL] {
        let body = read(&format!("amplifier-bundle/tools/{user}"));
        assert!(body.contains(RULES), "{user} sources {RULES}");
        assert!(
            !body.lines().any(|l| l.trim_start().starts_with("KEYWORDS=")
                || l.trim_start().starts_with("TEXT_KEYWORDS=")),
            "{user} must not define the keyword lists inline"
        );
        assert!(
            !body.lines().any(|l| l.trim_start().starts_with("cell()")),
            "{user} must not define cell() inline"
        );
    }
    // Sourcing has no side effects: nothing on stdout or stderr.
    let o = Command::new("bash")
        .arg("-c")
        .arg(format!(
            ". {:?}",
            tools_dir().join(RULES).display().to_string()
        ))
        .output()
        .unwrap();
    assert!(
        o.status.success() && o.stdout.is_empty() && o.stderr.is_empty(),
        "{}",
        show(&o)
    );
}

#[test]
fn light_recipe_copies_the_shared_rules_file() {
    assert!(read("amplifier-bundle/recipes/auto-drive-light.yaml").contains(RULES));
}

#[test]
fn every_touched_recipe_and_tool_stays_within_400_lines() {
    for f in [
        "amplifier-bundle/tools/autodrive_followups.sh",
        "amplifier-bundle/tools/autodrive_finding_rules.sh",
        "amplifier-bundle/tools/autodrive_loop.sh",
        "amplifier-bundle/tools/autodrive_crusty_rereview.sh",
        "amplifier-bundle/tools/autodrive_state.sh",
        "amplifier-bundle/tools/autodrive_merge_gate.sh",
        "amplifier-bundle/tools/autodrive_round_evidence.sh",
        "amplifier-bundle/recipes/autodrive-crusty-round.yaml",
        "amplifier-bundle/recipes/autodrive-crusty-loop.yaml",
        "amplifier-bundle/recipes/autodrive-merge-loop.yaml",
        "amplifier-bundle/recipes/auto-drive-to-merge.yaml",
    ] {
        let n = read(f).lines().count();
        assert!(n <= 400, "{f} has {n} lines; the budget is 400");
    }
}

#[test]
fn no_timeouts_or_numeric_caps_in_the_new_tool() {
    let body = read(&format!("amplifier-bundle/tools/{TOOL}"));
    for line in body.lines().filter(|l| !l.trim_start().starts_with('#')) {
        assert!(!line.contains("timeout "), "no timeout: {line}");
        assert!(
            !line.contains("max_") && !line.contains("MAX_"),
            "no numeric cap: {line}"
        );
        assert!(
            !line.contains("eval ") && !line.contains("sh -c"),
            "no eval or sh -c: {line}"
        );
        assert!(
            !line.contains("--no-verify") && !line.contains("--admin"),
            "no bypass flags: {line}"
        );
    }
}

#[test]
fn skill_copies_are_identical() {
    assert_eq!(
        read("amplifier-bundle/skills/auto-drive-to-merge/SKILL.md"),
        read("docs/claude/skills/auto-drive-to-merge/SKILL.md")
    );
}

#[test]
fn zero_concerns_wording_is_gone() {
    for f in [
        "amplifier-bundle/recipes/autodrive-crusty-loop.yaml",
        "amplifier-bundle/recipes/autodrive-crusty-round.yaml",
        "amplifier-bundle/recipes/auto-drive-to-merge.yaml",
        "amplifier-bundle/skills/auto-drive-to-merge/SKILL.md",
        "docs/claude/skills/auto-drive-to-merge/SKILL.md",
        "docs/reference/auto-drive-to-merge.md",
    ] {
        let body = read(f).to_lowercase();
        assert!(
            !body.contains("zero concerns"),
            "{f} still says 'zero concerns'"
        );
    }
}

#[test]
fn verdict_allow_lists_admit_minor_only() {
    let s06 = step_command(ROUND, "step-06-write-round-record");
    assert!(
        s06.contains("CLEAN|MINOR_ONLY|CONCERNS"),
        "step-06 allow-list"
    );
    // Step-04 and step-05 still run only on CONCERNS.
    for id in [
        "step-04-address-concerns",
        "step-05-verify-concerns-addressed",
    ] {
        let cond = step(ROUND, id)["condition"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(cond.contains("'CONCERNS'"), "{id} condition: {cond}");
    }
    let state = read("amplifier-bundle/tools/autodrive_state.sh");
    assert!(
        state.contains("(CLEAN|MINOR_ONLY|CONCERNS)"),
        "crusty_final well-formed regex"
    );
    assert!(
        state.contains("(CLEAN|MINOR_ONLY)"),
        "crusty_final terminal regex"
    );
    let ev = read("amplifier-bundle/tools/autodrive_round_evidence.sh");
    assert!(
        ev.contains("MINOR_ONLY"),
        "round evidence maps MINOR_ONLY to DONE_CLEAN"
    );
}

#[test]
fn round_step03_classifies_after_the_light_rule() {
    let s03 = step_command(ROUND, "step-03-extract-crusty-verdict");
    let light = s03.find("autodrive_light_steps.sh").expect("light line");
    let cls = s03.find("autodrive_followups.sh").expect("classify line");
    assert!(
        cls > light,
        "light mode keeps priority: classify runs after the light line"
    );
    assert!(
        s03.contains("AUTODRIVE_LAUNCH_TOOLS"),
        "loaded from the launch tools root"
    );
}

#[test]
fn only_the_crusty_loops_pass_the_second_clean_token() {
    let cl = step_command("autodrive-crusty-loop", "step-02-crusty-loop");
    assert!(
        cl.contains("--also-clean-token MINOR_ONLY")
            || cl.contains("--also-clean-token \"MINOR_ONLY\""),
        "crusty loop"
    );
    let rr = read("amplifier-bundle/tools/autodrive_crusty_rereview.sh");
    assert!(rr.contains("--also-clean-token"), "re-review");
    let ml = read("amplifier-bundle/recipes/autodrive-merge-loop.yaml");
    assert!(
        !ml.contains("--also-clean-token"),
        "the merge-ready loop passes no second token"
    );
    let rr_collect = rr.find(" collect").expect("re-review runs collect");
    let rr_done = rr
        .rfind("autodrive_record_crusty_loop_done")
        .expect("records done");
    assert!(
        rr_collect < rr_done,
        "collect runs before the loop is recorded done"
    );
}

#[test]
fn plan_task_id_is_an_input_and_reaches_both_loops() {
    for r in [
        "auto-drive-to-merge",
        "autodrive-crusty-loop",
        "autodrive-merge-loop",
    ] {
        let ctx = &recipe_yaml(r)["context"];
        assert!(
            ctx.get("plan_task_id").is_some(),
            "{r} declares plan_task_id"
        );
    }
    let top = read("amplifier-bundle/recipes/auto-drive-to-merge.yaml");
    for id in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let s = step("auto-drive-to-merge", id);
        let text = serde_yaml::to_string(&s).unwrap();
        assert!(
            text.contains("plan_task_id"),
            "{id} receives plan_task_id: {text}"
        );
    }
    assert!(
        top.contains("followups.tsv"),
        "the final summary lists follow-ups"
    );
}

#[test]
fn merge_loop_files_before_the_gate() {
    let ids: Vec<String> = steps("autodrive-merge-loop")
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    let b = ids
        .iter()
        .position(|i| i == "step-02b-file-followups")
        .expect("step-02b exists");
    let g = ids.iter().position(|i| i == "step-03-merge-gate").unwrap();
    assert!(b < g, "step-02b runs before the gate");
    let s = step("autodrive-merge-loop", "step-02b-file-followups");
    assert_eq!(s["type"].as_str(), Some("bash"));
    assert_eq!(
        s["condition"].as_str(),
        Some("merge_loop_preflight.should_run == 'true'")
    );
    assert!(
        s["command"]
            .as_str()
            .unwrap()
            .contains("autodrive_followups.sh")
    );
}

#[test]
fn merge_gate_runs_check_from_its_own_directory() {
    let g = read("amplifier-bundle/tools/autodrive_merge_gate.sh");
    assert!(
        g.contains("${GATE_HOME}/autodrive_followups.sh"),
        "check runs from GATE_HOME"
    );
    assert!(
        g.contains("ERROR: followups-unfiled"),
        "the gate prints the error itself"
    );
    assert!(
        g.contains("followups.tsv"),
        "followups.tsv joins the evidence bundle"
    );
}
