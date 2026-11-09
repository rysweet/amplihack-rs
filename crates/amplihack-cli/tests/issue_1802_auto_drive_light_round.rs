//! Issues #1802 and #1982: a crusty round of an auto-drive-light run.
//!
//! Since #1982 light mode adds nothing inside the crusty and merge loops. The
//! change is measured once, by `light-eligibility` before the loops, where the
//! result decides what runs; a crusty round never measures it again, and has
//! no light hook, marker or tool. Minor findings are classified by the #1979
//! `classify` branch and filed by `autodrive_followups.sh collect` and `file`,
//! as in auto-drive-to-merge.
//!
//! Three layers are tested:
//!
//! 1. Step-03 and step-06 of `autodrive-crusty-round.yaml`, extracted from
//!    the live YAML and run in bash. Their output is byte-identical to the
//!    output before #1802 (the golden tests were captured from the unmodified
//!    recipe), and a light state directory, a leftover `light-mode` marker or
//!    a planted light tool changes none of it.
//! 2. The real tools (`autodrive_followups.sh`, `autodrive_followups_check.sh`,
//!    `autodrive_finding_rules.sh`, `autodrive_state.sh` and
//!    `workflow_issue_tracking.sh`) copied into one tools directory, with a
//!    sentinel `gh` that exits 97 wherever step-03 must not call it:
//!    classification, the record, collect, file, the gate, both recurrence
//!    cases, the keyword predicate, a head that grew past the light limits,
//!    and the absence of every retired light state file.
//! 3. Acceptance: a light `MINOR_ONLY` record is accepted by
//!    `autodrive_crusty_final` through the manifest.
//!
//! Every test skips, with a note, when `jq` is not installed.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};

use crate::support::auto_drive_light::{
    Fakes, LightRepo, fake_bin, git, jq_available, private_dir, private_file, read, repo_root,
    run_bash, show, stderr, stdout, step_command, tools_dir,
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

/// The in-loop light tools #1982 removed.
const RETIRED_TOOLS: &[&str] = &["autodrive_light_round.sh", "autodrive_light_steps.sh"];

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

    /// The state `light-eligibility` leaves after a `light` decision at the
    /// head: `light-eligibility.json`, and nothing a round reads.
    fn light(&self) -> &Self {
        let elig = json!({
            "decision": "light", "reason": "within-limits",
            "base": self.repo.base, "head": self.head,
            "lines": 2, "new_modules": 0, "public_api": 0, "schema": 0, "recipe_contract": 0,
            "limits": {"lines": 300, "new_modules": 0, "public_api": 0, "schema": 0, "recipe_contract": 0}
        });
        private_file(
            &self.state.join("light-eligibility.json"),
            &format!("{elig}\n"),
        );
        self
    }

    /// A `light-mode` marker as an auto-drive-light run before #1982 left it.
    fn leftover_marker(&self) -> &Self {
        private_file(&self.state.join("light-mode"), &format!("{}\n", self.head));
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

// ===========================================================================
// 1. Step-03 and step-06 of the shared round
// ===========================================================================

/// The planted light tools write this file if anything runs or sources them.
const CANARY: &str = "light-tool-ran";

/// A tools directory for the round with no `autodrive_followups.sh`, so
/// `classify` is skipped, and a stand-in for each retired light tool that
/// touches `$FAKE_LOG/light-tool-ran` if a round ever runs or sources it.
fn fake_tools(e: &Env) -> PathBuf {
    let t = e.repo.tmp.path().join("fake-tools");
    std::fs::create_dir_all(&t).unwrap();
    for f in RETIRED_TOOLS {
        crate::support::exec_stub::write_executable(
            &t.join(f),
            format!("#!/usr/bin/env bash\ntouch \"${{FAKE_LOG:?}}/{CANARY}\"\n"),
            0o755,
        );
    }
    t
}

fn light_tool_ran(e: &Env) -> bool {
    e.fakes.log.join(CANARY).exists()
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
    assert!(!light_tool_ran(&e), "no light tool runs");
    assert!(e.fakes.gh_calls().is_empty());

    // All-minor without a marker is still CONCERNS.
    let e = Env::new();
    let tools = fake_tools(&e);
    let o = step03(&e, &tools, &crusty_review("CONCERNS", &two_minor()), &[]);
    assert_eq!(
        stdout(&o),
        "{\"crusty_verdict\":\"CONCERNS\",\"verdict_source\":\"crusty\",\"concern_count\":2}\n"
    );
    assert!(!light_tool_ran(&e));

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
    assert!(!light_tool_ran(&e));
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

const CONCERNS_TWO: &str =
    "{\"crusty_verdict\":\"CONCERNS\",\"verdict_source\":\"crusty\",\"concern_count\":2}\n";

#[test]
fn step03_ignores_a_light_state_dir_and_a_leftover_marker() {
    if skip() {
        return;
    }
    // A light run's state directory, with or without the `light-mode` file an
    // earlier version wrote, gives the same verdict and state as any other
    // round: nothing is measured, no light tool runs and gh is never called.
    let concerns = json!([
        minor("naming-1", "Rename"),
        {"id": "real-bug", "severity": "major", "summary": "Wrong result", "evidence": "x"}
    ]);
    for (verdict, list, want) in [
        ("CONCERNS", concerns.clone(), CONCERNS_TWO),
        ("CONCERNS", two_minor(), CONCERNS_TWO),
        (
            "CLEAN",
            json!([]),
            "{\"crusty_verdict\":\"CLEAN\",\"verdict_source\":\"crusty\",\"concern_count\":0}\n",
        ),
    ] {
        for marker in [false, true] {
            let e = Env::new();
            e.light();
            if marker {
                e.leftover_marker();
            }
            let before = state_names(&e);
            let tools = fake_tools(&e);
            let o = step03(&e, &tools, &crusty_review(verdict, &list), &[]);
            assert_eq!(o.status.code(), Some(0), "{verdict}/{marker}: {}", show(&o));
            assert_eq!(stdout(&o), want, "{verdict}/{marker}: {}", show(&o));
            assert!(
                !light_tool_ran(&e),
                "{verdict}/{marker}: no light tool runs"
            );
            assert!(e.fakes.gh_calls().is_empty(), "{verdict}/{marker}");
            let mut after = state_names(&e);
            after.retain(|n| n != "crusty-round-1.json.findings");
            assert_eq!(
                after, before,
                "{verdict}/{marker}: only .findings is written"
            );
            if marker {
                assert_eq!(
                    e.read("light-mode").trim(),
                    e.head,
                    "the leftover is left alone"
                );
            }
        }
    }
}

#[test]
fn the_round_recipe_has_no_light_hook_and_the_in_loop_tools_are_gone() {
    let round = read("amplifier-bundle/recipes/autodrive-crusty-round.yaml");
    for gone in [
        "light-mode",
        "autodrive_light",
        "light-minor-rule",
        "light_exit",
    ] {
        assert!(
            !round.contains(gone),
            "autodrive-crusty-round.yaml still mentions {gone}"
        );
    }
    for f in RETIRED_TOOLS {
        assert!(
            !tools_dir().join(f).exists(),
            "{f} was removed by #1982 and must not come back"
        );
    }
    for rel in [
        "amplifier-bundle/recipes/auto-drive-light.yaml",
        "amplifier-bundle/recipes/autodrive-crusty-loop.yaml",
        "amplifier-bundle/recipes/autodrive-merge-loop.yaml",
        "amplifier-bundle/recipes/auto-drive-to-merge.yaml",
    ] {
        let body = read(rel);
        for f in RETIRED_TOOLS {
            assert!(!body.contains(f), "{rel} still names {f}");
        }
        assert!(
            !body.contains("light-escalated"),
            "{rel} still has light-escalated"
        );
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
        e.light().leftover_marker();
        legacy_side_files(&e);
        let tools = fake_tools(&e);
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
        assert!(!light_tool_ran(&e), "{verdict}: no light tool runs");
        assert_eq!(mode(&e.record), 0o600);
    }
}

// ===========================================================================
// 2. The real tools, end to end
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
fn real_tools_a_head_grown_past_the_light_limits_is_not_measured_in_the_loop() {
    if skip() {
        return;
    }
    // #1982: the round neither measures the moved head nor escalates; the
    // verdict comes from classify alone, and gh is never called (a sentinel
    // gh exits 97), so there is no base read and no fetch per round.
    let e = Env::new();
    e.light().leftover_marker();
    let tools = real_tools(&e);
    use_gh(&e, SENTINEL_GH);
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
    assert!(e.fakes.gh_calls().is_empty(), "{:?}", e.fakes.gh_calls());
    assert!(!e.p("escalation.json").exists(), "no in-loop escalation");
    assert_eq!(
        e.read("light-mode").trim(),
        e.head,
        "the leftover is not rewritten"
    );
    let elig: Value = serde_json::from_str(&e.read("light-eligibility.json")).unwrap();
    assert_eq!(
        elig["head"],
        e.head.as_str(),
        "light-eligibility.json is not rewritten"
    );
    assert!(!stderr(&o).contains("light round"), "{}", show(&o));
    no_retired_files(&e);
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
// 3. Acceptance: a light MINOR_ONLY record reaches the merge phase
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
