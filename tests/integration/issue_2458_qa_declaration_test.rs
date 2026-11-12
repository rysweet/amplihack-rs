//! Issue #2458: the base branch's `.amplihack/qa.toml` may say how merge-ready
//! criterion 1 is met for a pull request that spans several areas.
//!
//! - `multi_area = "command"`: a passing qa.toml command meets criterion 1, and
//!   no gadugi scenario is required or written.
//! - `scenarios_ci_job = "<check-run name>"`: the scenarios run in CI; criterion
//!   1 reads that check-run at the head (autodrive_expected_checks.sh, 3rd
//!   argument).
//!
//! The declaration is read from the merge-base tree only. A qa.toml changed by
//! the pull request is refused (`changed-in-pr`) and the declaration is `none`,
//! so a pull request never sets the rule it is judged by (#2439). With no
//! declaration, today's fail-closed behaviour stays.
//!
//! Every case runs the real tools on a temporary git repository shaped like the
//! mistt-infra drives: a root package.json, a `scenarios/` folder holding JSON
//! and TypeScript scenarios (not gadugi YAML), a pull request that touches
//! three top-level folders, and a qa.toml command that exits 0. The tools run
//! under `env -i` with HOME, TMPDIR and git config inside the temporary
//! directory, so nothing reads the real ~/.claude, ~/.amplihack or profiles.
//! There are no timers and no network: `gh` is a fixture-serving fake.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SYSPATH: &str = "/usr/bin:/bin";
const JOB: &str = "Gadugi scenarios";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|p| p.join("amplifier-bundle/tools").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/tools")
}

fn tool(name: &str) -> PathBuf {
    workspace_root().join("amplifier-bundle/tools").join(name)
}

/// A temporary repository with a base commit (refs/remotes/origin/main) and a
/// pull-request commit on HEAD.
struct Fixture {
    tmp: tempfile::TempDir,
}

impl Fixture {
    fn root(&self) -> &Path {
        self.tmp.path()
    }
    fn repo(&self) -> PathBuf {
        self.root().join("repo")
    }
    fn home(&self) -> PathBuf {
        self.root().join("home")
    }
    fn stubs(&self) -> PathBuf {
        self.root().join("stubs")
    }
    fn calls(&self) -> PathBuf {
        self.root().join("calls")
    }
    fn calls_of(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.calls())
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with(&format!("{name}|")))
            .map(str::to_owned)
            .collect()
    }
}

fn git(fx: &Fixture, args: &[&str]) {
    let out = amplihack_git::command_in(fx.repo())
        .args(args)
        .env_clear()
        .env("PATH", SYSPATH)
        .env("HOME", fx.home())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn put(fx: &Fixture, rel: &str, body: &str) {
    let p = fx.repo().join(rel);
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(p, body).expect("write fixture file");
}

/// `commit` stages everything and commits (plumbing-free: no hooks are
/// installed in a fresh repository).
fn commit(fx: &Fixture, msg: &str) {
    git(fx, &["add", "-A"]);
    git(
        fx,
        &["commit", "-q", "--allow-empty", "--no-verify", "-m", msg],
    );
}

/// The issue's shape. `base_qa` is the qa.toml on the base branch (None: no
/// file); `pr_qa` is a qa.toml the pull request writes (None: left alone).
/// `pr_extra` are further paths the pull request changes.
fn fixture(base_qa: Option<&str>, pr_qa: Option<&str>, pr_extra: &[&str]) -> Fixture {
    let fx = Fixture {
        tmp: tempfile::tempdir().expect("tempdir"),
    };
    for d in [fx.repo(), fx.home(), fx.stubs()] {
        fs::create_dir_all(d).expect("mkdir");
    }
    fs::write(fx.calls(), "").expect("calls file");
    // Each stub appends "name|argv" to $CALLS and exits 0.
    for name in ["gadugi-test", "npm", "cargo", "pytest"] {
        let p = fx.stubs().join(name);
        fs::write(
            &p,
            format!("#!/bin/sh\nprintf '%s|%s\\n' '{name}' \"$*\" >> \"$CALLS\"\nexit 0\n"),
        )
        .expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).expect("chmod stub");
        }
    }
    git(&fx, &["init", "-q", "."]);
    put(
        &fx,
        "package.json",
        "{\"name\":\"infra\",\"private\":true}\n",
    );
    put(&fx, "scenarios/smoke.json", "{\"name\":\"smoke\"}\n");
    put(&fx, "scenarios/smoke.ts", "export const smoke = 1;\n");
    put(&fx, "web/a.txt", "1\n");
    put(&fx, "api/b.txt", "1\n");
    put(&fx, "infra/c.txt", "1\n");
    if let Some(q) = base_qa {
        put(&fx, ".amplihack/qa.toml", q);
    }
    commit(&fx, "base");
    git(&fx, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    for p in ["web/a.txt", "api/b.txt", "infra/c.txt"]
        .iter()
        .chain(pr_extra)
    {
        put(&fx, p, "2\n");
    }
    if let Some(q) = pr_qa {
        put(&fx, ".amplihack/qa.toml", q);
    }
    commit(&fx, "pr");
    fx
}

/// A scrubbed `env -i` command: HOME and TMPDIR in the fixture, stubs first
/// on PATH. No AUTODRIVE_QA_* is inherited; callers add what they test.
fn scrubbed(fx: &Fixture, program: &str) -> Command {
    let mut c = Command::new(program);
    c.env_clear()
        .env("HOME", fx.home())
        .env("TMPDIR", fx.root())
        .env("PATH", format!("{}:{SYSPATH}", fx.stubs().display()))
        .env("CALLS", fx.calls())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(fx.root());
    c
}

/// Runs autodrive_qa_evidence.sh against the merge base `main` and returns
/// the evidence record (its last stdout line).
fn evidence(fx: &Fixture, env: &[(&str, &str)]) -> Value {
    let mut c = scrubbed(fx, "bash");
    c.arg(tool("autodrive_qa_evidence.sh"))
        .env("REPO_PATH", fx.repo())
        .env("AUTODRIVE_QA_BASE", "main")
        .env("AUTODRIVE_ROUND_LABEL", "round-1");
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().expect("run the evidence tool");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout.lines().last().unwrap_or("");
    serde_json::from_str(last).unwrap_or_else(|e| {
        panic!(
            "the evidence record is not JSON ({e}): {last}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn field<'a>(ev: &'a Value, k: &str) -> &'a str {
    ev.get(k).and_then(Value::as_str).unwrap_or("<absent>")
}

fn expect(ev: &Value, case: &str, want: &[(&str, &str)]) {
    let bad: Vec<String> = want
        .iter()
        .filter(|(k, v)| field(ev, k) != *v)
        .map(|(k, v)| format!("{k}={:?} (want {v:?})", field(ev, k)))
        .collect();
    assert!(bad.is_empty(), "{case}: {}\nrecord: {ev}", bad.join(", "));
}

/// Calls `<function> <repo>` after sourcing autodrive_trust.sh (which loads
/// its own helpers), in a scrubbed shell. `base`: Some(v) sets
/// AUTODRIVE_QA_BASE=v (even empty); None leaves it unset (a preflight).
fn helper(fx: &Fixture, function: &str, base: Option<&str>, env: &[(&str, &str)]) -> Output {
    let mut c = scrubbed(fx, "bash");
    c.arg("-c")
        .arg(r#". "$1" || exit 90; "$2" "$3""#)
        .arg("_")
        .arg(tool("autodrive_trust.sh"))
        .arg(function)
        .arg(fx.repo());
    if let Some(b) = base {
        c.env("AUTODRIVE_QA_BASE", b);
    }
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().expect("run bash")
}

fn helper_line(fx: &Fixture, function: &str, base: Option<&str>, env: &[(&str, &str)]) -> String {
    let out = helper(fx, function, base, env);
    assert!(
        out.status.success(),
        "{function}: {out:?}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const QA_CMD: &str = "command = \"npm test\"\n";
const QA_MULTI: &str = "command = \"npm test\"\nmulti_area = \"command\"\n";
const QA_CI_JOB: &str = "command = \"npm test\"\nscenarios_ci_job = \"Gadugi scenarios\"\n";

// ---------------------------------------------------------------------------
// Criterion 1 in the evidence record.
// ---------------------------------------------------------------------------

/// (a) The issue's case with `multi_area = "command"` on the base branch: the
/// passing qa.toml command meets criterion 1; no gadugi scenario is needed or
/// run, and the record says which declaration was used.
#[cfg(unix)]
#[test]
fn a_base_multi_area_declaration_meets_criterion_1_with_the_passing_command() {
    let fx = fixture(Some(QA_MULTI), None, &[]);
    let ev = evidence(&fx, &[]);
    expect(
        &ev,
        "multi_area = command",
        &[
            ("qa_status", "PASS"),
            ("qa_command_source", "repo-file"),
            ("qa_command", "npm test"),
            ("qa_exit_code", "0"),
            ("gadugi_required", "false"),
            ("gadugi_status", "NOT_REQUIRED"),
            ("qa_criterion1_declaration", "multi-area-command"),
        ],
    );
    assert_eq!(
        fx.calls_of("npm"),
        vec!["npm|test".to_owned()],
        "the qa.toml command must run once"
    );
    assert!(
        fx.calls_of("gadugi-test").is_empty(),
        "no gadugi-test call under the declaration: {:?}",
        fx.calls_of("gadugi-test")
    );
}

/// (b) The same pull request with a qa.toml that declares nothing keeps
/// today's fail-closed result: gadugi is required and there are no YAML
/// scenarios, so the drive is FAIL / no-scenarios.
#[cfg(unix)]
#[test]
fn without_a_declaration_the_same_pull_request_still_needs_gadugi() {
    let fx = fixture(Some(QA_CMD), None, &[]);
    let ev = evidence(&fx, &[]);
    expect(
        &ev,
        "no declaration",
        &[
            ("qa_status", "FAIL"),
            ("qa_reason", "no-scenarios"),
            ("gadugi_required", "true"),
            ("gadugi_status", "NO_SCENARIOS"),
            ("qa_criterion1_declaration", "none"),
        ],
    );
}

/// (c) A declaration the pull request adds is never used: qa.toml changed in
/// the pull request is refused before it is read, the declaration is `none`
/// and gadugi stays required. Both shapes: the pull request adds the key to an
/// existing qa.toml, and the pull request adds qa.toml itself.
#[cfg(unix)]
#[test]
fn a_declaration_added_by_the_pull_request_is_ignored() {
    for (case, base) in [("key added", Some(QA_CMD)), ("file added", None)] {
        let fx = fixture(base, Some(QA_MULTI), &[]);
        let ev = evidence(&fx, &[]);
        expect(
            &ev,
            case,
            &[
                ("qa_status", "BLOCKED"),
                ("qa_reason", "qa-config-invalid"),
                ("gadugi_required", "true"),
                ("qa_criterion1_declaration", "none"),
            ],
        );
        assert!(
            field(&ev, "qa_summary").contains(".amplihack/qa.toml: changed-in-pr"),
            "{case}: the summary must name changed-in-pr: {ev}"
        );
        assert!(
            fx.calls_of("npm").is_empty(),
            "{case}: a qa.toml changed in the pull request must run nothing"
        );
        let decl = helper_line(&fx, "autodrive_qa_declaration", Some("main"), &[]);
        assert_eq!(decl, "none\n", "{case}: autodrive_qa_declaration");
        let req = helper_line(&fx, "autodrive_gadugi_required", Some("main"), &[]);
        assert_eq!(req, "true\n", "{case}: autodrive_gadugi_required");
    }
}

/// (B) recorded in the evidence: `scenarios_ci_job` lifts the local gadugi
/// requirement and names the job; the gate then reads that job (below).
#[cfg(unix)]
#[test]
fn a_base_ci_job_declaration_is_recorded_with_its_job_name() {
    let fx = fixture(Some(QA_CI_JOB), None, &[]);
    let ev = evidence(&fx, &[]);
    expect(
        &ev,
        "scenarios_ci_job",
        &[
            ("qa_status", "PASS"),
            ("qa_command_source", "repo-file"),
            ("gadugi_required", "false"),
            ("gadugi_status", "NOT_REQUIRED"),
            ("qa_criterion1_declaration", "ci-job:Gadugi scenarios"),
        ],
    );
    assert!(fx.calls_of("gadugi-test").is_empty());
}

/// Bad declarations are BLOCKED / qa-config-invalid with a token, never a
/// silent choice, and the rejected value is never printed.
#[cfg(unix)]
#[test]
fn an_invalid_declaration_blocks_with_a_token_and_prints_no_value() {
    let cases: &[(&str, &str)] = &[
        (
            "command = \"npm test\"\nmulti_area = \"yes\"\n",
            "unknown-value",
        ),
        (
            "command = \"npm test\"\nmulti_area = \"\"\n",
            "unknown-value",
        ),
        (
            "command = \"npm test\"\nscenarios_ci_job = \"\"\n",
            "unknown-value",
        ),
        (
            "command = \"npm test\"\nscenarios_ci_job = \"Gadugi $(id) scenarios\"\n",
            "unknown-value",
        ),
        (
            "command = \"npm test\"\nmulti_area = \"command\"\nscenarios_ci_job = \"Gadugi scenarios\"\n",
            "declaration-conflict",
        ),
        (
            "command = \"npm test\"\nmulti_area = \"command\"\nmulti_area = \"command\"\n",
            "duplicate-key",
        ),
        ("multi_area = \"command\"\n", "missing-command"),
        (
            "command = \"npm test\"\nmulti_areas = \"command\"\n",
            "unknown-key",
        ),
    ];
    for (body, token) in cases {
        let fx = fixture(Some(body), None, &[]);
        let ev = evidence(&fx, &[]);
        expect(
            &ev,
            body,
            &[
                ("qa_status", "BLOCKED"),
                ("qa_reason", "qa-config-invalid"),
                ("gadugi_required", "true"),
                ("qa_criterion1_declaration", "none"),
            ],
        );
        let summary = field(&ev, "qa_summary");
        assert!(
            summary.contains(&format!(".amplihack/qa.toml: {token}")),
            "{body:?}: want token {token}: {ev}"
        );
        for leaked in ["yes", "$(id)"] {
            assert!(
                !summary.contains(leaked),
                "{body:?}: the summary prints qa.toml content: {summary}"
            );
        }
        assert!(fx.calls_of("npm").is_empty(), "{body:?}: nothing may run");
    }
}

// ---------------------------------------------------------------------------
// The resolver: autodrive_qa_declaration and autodrive_gadugi_required.
// ---------------------------------------------------------------------------

/// One answer for the evidence tool, the gate and the preflights.
#[cfg(unix)]
#[test]
fn the_declaration_resolver_reads_the_merge_base_tree() {
    for (qa, want_decl, want_req) in [
        (Some(QA_MULTI), "multi-area-command", "false"),
        (Some(QA_CI_JOB), "ci-job:Gadugi scenarios", "false"),
        (Some(QA_CMD), "none", "true"),
        (None, "none", "true"),
    ] {
        let fx = fixture(qa, None, &[]);
        assert_eq!(
            helper_line(&fx, "autodrive_qa_declaration", Some("main"), &[]),
            format!("{want_decl}\n"),
            "qa.toml {qa:?}"
        );
        assert_eq!(
            helper_line(&fx, "autodrive_gadugi_required", Some("main"), &[]),
            format!("{want_req}\n"),
            "qa.toml {qa:?}"
        );
    }
}

/// The base qa.toml's declaration holds even when the working tree's copy
/// says something else: only the merge-base tree is read.
#[cfg(unix)]
#[test]
fn the_working_tree_copy_of_qa_toml_is_never_read() {
    let fx = fixture(Some(QA_CMD), None, &[]);
    put(&fx, ".amplihack/qa.toml", QA_MULTI); // modified, uncommitted
    assert_eq!(
        helper_line(&fx, "autodrive_qa_declaration", Some("main"), &[]),
        "none\n"
    );
    let ev = evidence(&fx, &[]);
    expect(
        &ev,
        "uncommitted declaration",
        &[
            ("gadugi_required", "true"),
            ("qa_criterion1_declaration", "none"),
        ],
    );
}

/// D7: an operator scenario directory or an env QA command makes the
/// declaration void; environment variables can only make the rule stricter.
#[cfg(unix)]
#[test]
fn env_overrides_void_the_declaration() {
    let fx = fixture(Some(QA_MULTI), None, &[]);
    for env in [
        ("AUTODRIVE_QA_SCENARIO_DIR", "scenarios"),
        ("AUTODRIVE_QA_COMMAND", "npm test"),
        ("AUTODRIVE_QA_COMMANDS", "npm test"),
    ] {
        assert_eq!(
            helper_line(&fx, "autodrive_qa_declaration", Some("main"), &[env]),
            "none\n",
            "{env:?}"
        );
        assert_eq!(
            helper_line(&fx, "autodrive_gadugi_required", Some("main"), &[env]),
            "true\n",
            "{env:?}"
        );
        let ev = evidence(&fx, &[env]);
        expect(
            &ev,
            env.0,
            &[
                ("gadugi_required", "true"),
                ("qa_criterion1_declaration", "none"),
            ],
        );
    }
}

/// S1: a set-but-empty AUTODRIVE_QA_BASE is an unknown merge base, not a
/// preflight: the declaration is `none` and gadugi stays required.
#[cfg(unix)]
#[test]
fn an_empty_merge_base_gives_no_declaration() {
    let fx = fixture(Some(QA_MULTI), None, &[]);
    assert_eq!(
        helper_line(&fx, "autodrive_qa_declaration", Some(""), &[]),
        "none\n"
    );
    assert_eq!(
        helper_line(&fx, "autodrive_gadugi_required", Some(""), &[]),
        "true\n"
    );
}

/// S3: a pull request that changes a workflow cannot rely on the CI job
/// declaration (it could change what the job runs). The command declaration
/// does not depend on CI and still holds.
#[cfg(unix)]
#[test]
fn a_workflow_change_voids_only_the_ci_job_declaration() {
    let wf = ".github/workflows/ci.yml";
    let fx = fixture(Some(QA_CI_JOB), None, &[wf]);
    assert_eq!(
        helper_line(&fx, "autodrive_qa_declaration", Some("main"), &[]),
        "none\n"
    );
    assert_eq!(
        helper_line(&fx, "autodrive_gadugi_required", Some("main"), &[]),
        "true\n"
    );
    let ev = evidence(&fx, &[]);
    expect(
        &ev,
        "ci-job with a workflow change",
        &[
            ("gadugi_required", "true"),
            ("qa_criterion1_declaration", "none"),
        ],
    );
    assert!(
        field(&ev, "qa_summary").contains("ci-job-workflow-changed"),
        "the summary must say why the CI job declaration was not used: {ev}"
    );

    let fx = fixture(Some(QA_MULTI), None, &[wf]);
    assert_eq!(
        helper_line(&fx, "autodrive_qa_declaration", Some("main"), &[]),
        "multi-area-command\n"
    );
}

/// D12 / S5: with no base at all (a preflight), the HEAD tree's declaration
/// may skip only the gadugi-test install demand; the report still refuses
/// to choose a command (invalid paths-unknown), so criterion 1 is decided
/// later against the merge base.
#[cfg(unix)]
#[test]
fn a_preflight_reads_the_head_declaration_but_the_report_still_needs_a_base() {
    let fx = fixture(Some(QA_MULTI), None, &[]);
    assert_eq!(
        helper_line(&fx, "autodrive_qa_declaration", None, &[]),
        "multi-area-command\n"
    );
    assert_eq!(
        helper_line(&fx, "autodrive_gadugi_required", None, &[]),
        "false\n"
    );
    let report = helper_line(&fx, "autodrive_qa_report", None, &[]);
    assert!(
        report.lines().any(|l| l == "invalid\tpaths-unknown"),
        "a report with no base must stay invalid/paths-unknown:\n{report}"
    );
    assert!(
        !report.lines().any(|l| l.starts_with("command\t")),
        "a report with no base must not choose a command:\n{report}"
    );
}

/// S6: a function inherited from the caller cannot stand in for the real
/// resolver. With autodrive_qa_sources.sh absent beside autodrive_trust.sh,
/// a pre-defined `autodrive_qa_declaration` that answers multi-area-command
/// must not lift the gadugi requirement.
#[cfg(unix)]
#[test]
fn an_inherited_declaration_function_never_lifts_the_requirement() {
    let fx = fixture(Some(QA_MULTI), None, &[]);
    let decoy = fx.root().join("tools");
    fs::create_dir_all(&decoy).expect("mkdir decoy tools");
    for f in ["autodrive_trust.sh", "autodrive_qa_scope.sh"] {
        fs::copy(tool(f), decoy.join(f)).expect("copy tool");
    }
    let out = scrubbed(&fx, "bash")
        .arg("-c")
        .arg(
            r#"autodrive_qa_declaration() { printf 'multi-area-command\n'; }
               _AD_SOURCES_OK=1
               . "$1" || exit 90; autodrive_gadugi_required "$2""#,
        )
        .arg("_")
        .arg(decoy.join("autodrive_trust.sh"))
        .arg(fx.repo())
        .env("AUTODRIVE_QA_BASE", "main")
        .output()
        .expect("run bash");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "true\n",
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------
// (B) at the gate: autodrive_expected_checks.sh <HEAD> <BASE> <JOB>.
// ---------------------------------------------------------------------------

const HEAD: &str = "3f9c2a7e1b4d5c6f708192a3b4c5d6e7f8091a2b";

/// Runs the expected-checks helper with the #2186 fixture set and fake `gh`
/// (expected-checks-fixtures.sh). `setup` is bash run against the fixture
/// directory `$FX` after the default green set is written. Returns
/// (exit code, last stdout line as JSON or Null, number of gh calls).
fn expected_checks(setup: &str, args: &[&str]) -> (Option<i32>, Value, usize) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixtures =
        workspace_root().join("amplifier-bundle/recipes/tests/expected-checks-fixtures.sh");
    let script = r#"
set -u
W="$1"; FIXTURES="$2"; HELPER="$3"; SETUP="$4"; shift 4
. "$FIXTURES"
mkdir -p "$W/bin"
ec_write_route "$W/route"; chmod +x "$W/route"
cat > "$W/bin/gh" <<'GH'
#!/usr/bin/env bash
echo "$*" >> "$GH_CALLS"
[ "${1:-}" = api ] && exec "$GH_CI_ROUTE" "$@"
exit 1
GH
chmod +x "$W/bin/gh"
FX="$W/fx"; ec_fixture_green "$FX"
eval "$SETUP"
: > "$W/calls"
env -u BASH_ENV -u ENV GH_CI_FIX="$FX" GH_CI_ROUTE="$W/route" GH_HEAD="$HEAD_SHA" GH_CALLS="$W/calls" \
  PATH="$W/bin:$PATH" /bin/bash -p "$HELPER" "$@" > "$W/out" 2> "$W/err" < /dev/null
rc=$?
tail -n 1 "$W/out"
echo "calls=$(wc -l < "$W/calls" | tr -d ' ')"
exit "$rc"
"#;
    let out = Command::new("bash")
        .arg("-c")
        .arg(script)
        .arg("_")
        .arg(tmp.path())
        .arg(fixtures)
        .arg(tool("autodrive_expected_checks.sh"))
        .arg(setup)
        .args(args)
        .env_clear()
        .env("PATH", SYSPATH)
        .env("HOME", tmp.path())
        .env("TMPDIR", tmp.path())
        .env("HEAD_SHA", HEAD)
        .output()
        .expect("run expected checks");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines: Vec<&str> = stdout.lines().collect();
    let calls = lines
        .pop()
        .and_then(|l| l.strip_prefix("calls="))
        .and_then(|n| n.parse().ok())
        .unwrap_or(usize::MAX);
    let verdict = lines
        .last()
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or(Value::Null);
    (out.status.code(), verdict, calls)
}

fn ci_status(v: &Value) -> &str {
    v.get("ci_status")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
}

fn missing_has(v: &Value, name: &str) -> bool {
    v.get("missing")
        .and_then(Value::as_array)
        .is_some_and(|m| m.iter().any(|x| x.as_str() == Some(name)))
}

/// The declared job is a check-run at the head: success passes, and only an
/// exact name counts.
#[cfg(unix)]
#[test]
fn the_declared_ci_job_passes_criterion_1_only_when_it_succeeded() {
    let add = |status: &str, conclusion: &str| {
        format!(r#"ec_job "$FX" 1001 "{JOB}" {status} {conclusion} "$HEAD_SHA""#)
    };
    let cases: Vec<(String, &str, i32)> = vec![
        (add("completed", "success"), "GREEN", 0),
        (add("completed", "failure"), "RED", 1),
        (add("completed", "cancelled"), "RED", 1),
        // S2: a skipped job ran no scenario, so it does not pass.
        (add("completed", "skipped"), "RED", 1),
        (add("in_progress", "null"), "PENDING", 1),
        (add("queued", "null"), "PENDING", 1),
    ];
    for (setup, want, rc) in &cases {
        let (code, v, _) = expected_checks(setup, &[HEAD, "main", JOB]);
        assert_eq!(
            (code, ci_status(&v)),
            (Some(*rc), *want),
            "setup `{setup}`: {v}"
        );
    }
    // The default green set without the job: everything else passes, but the
    // declared job is missing, and its name is in missing[].
    let (code, v, _) = expected_checks(":", &[HEAD, "main", JOB]);
    assert_eq!((code, ci_status(&v)), (Some(1), "MISSING"), "{v}");
    assert!(missing_has(&v, JOB), "missing[] must name the job: {v}");
    // A near name is not the declared job.
    let (code, v, _) = expected_checks(
        &format!(r#"ec_job "$FX" 1001 "{JOB} (linux)" completed success "$HEAD_SHA""#),
        &[HEAD, "main", JOB],
    );
    assert_eq!((code, ci_status(&v)), (Some(1), "MISSING"), "{v}");
}

/// S2 contrast: without the third argument a skipped job passes as before,
/// so the stricter rule applies only to the declared job.
#[cfg(unix)]
#[test]
fn without_a_declared_job_the_expected_checks_are_unchanged() {
    let setup = format!(r#"ec_job "$FX" 1001 "{JOB}" completed skipped "$HEAD_SHA""#);
    let (code, v, _) = expected_checks(&setup, &[HEAD, "main"]);
    assert_eq!((code, ci_status(&v)), (Some(0), "GREEN"), "{v}");
}

/// S4: a commit status with the job's name is not the job: only a check-run
/// meets the declaration.
#[cfg(unix)]
#[test]
fn a_commit_status_with_the_job_name_does_not_count() {
    let setup = format!(
        r#"printf '{{"state":"success","sha":"%s","total_count":1,"statuses":[{{"context":"{JOB}","state":"success"}}]}}\n' "$HEAD_SHA" > "$FX/status.json""#
    );
    let (code, v, _) = expected_checks(&setup, &[HEAD, "main", JOB]);
    assert_eq!((code, ci_status(&v)), (Some(1), "MISSING"), "{v}");
    assert!(missing_has(&v, JOB), "{v}");
}

/// S7/S8: a job name outside the whitelist is refused with exit 2 before any
/// `gh` call.
#[cfg(unix)]
#[test]
fn a_bad_job_name_is_refused_before_any_gh_call() {
    for bad in [
        "",
        "-x",
        "--paginate",
        "Gadugi \"scenarios\"",
        "Gadugi\tscenarios",
        "Gadugi\\scenarios",
        "Gadugi $HOME",
        "Gadugi `id`",
        &"x".repeat(129),
    ] {
        let (code, _, calls) = expected_checks(":", &[HEAD, "main", bad]);
        assert_eq!(code, Some(2), "job name {bad:?} must be refused");
        assert_eq!(calls, 0, "job name {bad:?}: gh was called first");
    }
}

// ---------------------------------------------------------------------------
// The merge-round agent's instructions.
// ---------------------------------------------------------------------------

/// step-04 tells the agent how criterion 1 is met: by the base-branch
/// declaration when there is one, and otherwise by gadugi scenarios written
/// without timers and without touching files outside the brief. The agent
/// never adds a declaration itself.
#[test]
fn merge_round_step_04_states_the_declaration_rule() {
    let recipe: serde_yaml::Value = serde_yaml::from_str(
        &fs::read_to_string(
            workspace_root().join("amplifier-bundle/recipes/autodrive-merge-round.yaml"),
        )
        .expect("read merge round"),
    )
    .expect("parse merge round");
    let prompt = recipe["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s["id"].as_str() == Some("step-04-address-blockers"))
        .and_then(|s| s["prompt"].as_str())
        .expect("step-04 prompt");
    let flat = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_ascii_lowercase();
    for needle in [
        "qa_criterion1_declaration",
        "multi_area",
        "scenarios_ci_job",
    ] {
        assert!(flat.contains(needle), "step-04 must mention `{needle}`");
    }
    for needle in ["timer", "sleep", "outside the brief", "base branch"] {
        assert!(lower.contains(needle), "step-04 must say `{needle}`");
    }
    assert!(
        lower.contains("where `gadugi_required` is `true`, write"),
        "step-04 still writes gadugi scenarios where they are required"
    );
}
