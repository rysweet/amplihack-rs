//! Issue #2391: the `autodrive_minor_findings` recipe context value.
//!
//! - `address` (the default; also empty or unset): an all-minor Crusty review
//!   stays CONCERNS, the address and verify steps run, and the Crusty loop
//!   continues until CLEAN or until loop-health evaluation stops it. Nothing
//!   is filed as a follow-up.
//! - `follow_up`: the behaviour before #2391. An all-minor review is
//!   MINOR_ONLY, it ends the loop, and the finalizer files one linked
//!   follow-up issue per minor finding.
//! - Any other value is treated as `address`, with one WARNING line from each
//!   step that normalises the value (crusty-loop step-02 and the re-review).
//!
//! The contract is `docs/reference/auto-drive-to-merge.md`, section
//! "Minor findings: address or follow up (#1979, #2391)".
//!
//! Layers:
//!
//! 1. Round step-03, extracted from the YAML, run with each value.
//! 2. Crusty-loop step-02 and `autodrive_crusty_rereview.sh`, run against a
//!    stub `autodrive_loop.sh` that records its argv, to check what each
//!    entry point passes on.
//! 3. The real loop, follow-up tools, state helpers and Rust completion
//!    helper, through the #2336 recipe boundary double (no agent, no network).
//! 4. Wiring: every composed recipe declares and passes the setting.
//!
//! No sleeps, timeouts or clock-based assertions. HOME, state and logs live in
//! a per-test temporary directory; no credential file is read.

use std::path::PathBuf;
use std::process::Output;

use serde_json::{Value, json};

use crate::issue_2336_autodrive_completion::fixture::{Drive, json_output};
use crate::support::auto_drive_light::{recipe, show, stderr, stdout, step_command, tools_dir};

const ROUND: &str = "autodrive-crusty-round";
const KEY: &str = "autodrive_minor_findings";
const WARNING_PREFIX: &str = "WARNING: autodrive_minor_findings='";
const WARNING_SUFFIX: &str = "' is not recognised; using 'address'";

/// Values that mean `address`, with no WARNING.
const ADDRESS_VALUES: [Option<&str>; 4] = [None, Some(""), Some("address"), Some(" address ")];
/// Values that mean `follow_up`, with no WARNING.
const FOLLOW_UP_VALUES: [&str; 3] = ["follow_up", " follow_up ", "\tfollow_up\t"];
/// Values that are not recognised: `address`, with one WARNING.
const UNKNOWN_VALUES: [&str; 5] = ["ignore", "follow-up", "FOLLOW_UP", "Address", "follow_up2"];

fn two_minor_review() -> String {
    json!({"crusty_verdict":"CONCERNS","concerns":[
        {"id":"docs-typo","severity":"minor","summary":"Typo in example","evidence":"README.md:4"},
        {"id":"naming-2","severity":"minor","summary":"Rename tmp to record","evidence":"lib.rs:9"}
    ]})
    .to_string()
}

fn issue_creates(d: &Drive) -> usize {
    d.fakes
        .gh_calls()
        .into_iter()
        .filter(|c| {
            c.first().map(String::as_str) == Some("issue")
                && c.get(1).map(String::as_str) == Some("create")
        })
        .count()
}

fn warning_lines(o: &Output) -> Vec<String> {
    stderr(o)
        .lines()
        .filter(|l| l.contains("autodrive_minor_findings") && l.contains("not recognised"))
        .map(str::to_owned)
        .collect()
}

// ===========================================================================
// 1. Round step-03
// ===========================================================================

/// Runs round step-03 on `review` with the setting as the runner exports it.
fn step03(d: &Drive, review: &str, value: Option<&str>) -> Output {
    let script = step_command(ROUND, "step-03-extract-crusty-verdict");
    let mut c = d.script(&script);
    c.env_remove("AUTODRIVE_MINOR_FINDINGS")
        .env("RECIPE_VAR_crusty_review", review)
        .env("AUTODRIVE_ROUND_RECORD", d.p("crusty-round-1.json"))
        .env("AUTODRIVE_ROUND_LABEL", "round-1")
        .env("AUTODRIVE_LAUNCH_TOOLS", tools_dir());
    if let Some(v) = value {
        c.env("AUTODRIVE_MINOR_FINDINGS", v);
    }
    c.output().unwrap()
}

fn verdict_line(o: &Output) -> Value {
    json_output(o)
}

#[test]
fn round_step03_keeps_an_all_minor_review_as_concerns_by_default() {
    for value in [None, Some(""), Some("address")] {
        let d = Drive::new();
        let o = step03(&d, &two_minor_review(), value);
        assert!(o.status.success(), "{value:?}: {}", show(&o));
        let v = verdict_line(&o);
        assert_eq!(
            v["crusty_verdict"],
            "CONCERNS",
            "{value:?}: minor findings are addressed, not classified away: {}",
            show(&o)
        );
        assert_eq!(v["verdict_source"], "crusty", "{value:?}: {}", show(&o));
        assert_eq!(v["concern_count"], 2, "{value:?}: {}", show(&o));
        assert_eq!(
            d.read("crusty-round-1.json.findings"),
            "docs-typo\nnaming-2\n",
            "{value:?}: the ids stay in .findings for the recurring-findings signal"
        );
        for sidecar in ["crusty-round-1.json.minor", "crusty-round-1.json.severity"] {
            assert!(
                !d.p(sidecar).exists(),
                "{value:?}: no {sidecar} under address: {}",
                show(&o)
            );
        }
        assert!(
            warning_lines(&o).is_empty(),
            "{value:?}: the round step never warns about the setting: {}",
            show(&o)
        );
    }
}

#[test]
fn round_step03_classifies_an_all_minor_review_only_under_follow_up() {
    let d = Drive::new();
    let o = step03(&d, &two_minor_review(), Some("follow_up"));
    assert!(o.status.success(), "{}", show(&o));
    let v = verdict_line(&o);
    assert_eq!(v["crusty_verdict"], "MINOR_ONLY", "{}", show(&o));
    assert_eq!(v["concern_count"], 2, "{}", show(&o));
    assert!(d.p("crusty-round-1.json.minor").is_file(), "{}", show(&o));
    assert!(
        d.p("crusty-round-1.json.severity").is_file(),
        "{}",
        show(&o)
    );
    assert_eq!(d.read("crusty-round-1.json.findings"), "");
}

#[test]
fn round_step03_keeps_major_findings_as_concerns_under_either_value() {
    let review = json!({"crusty_verdict":"CONCERNS","concerns":[
        {"id":"docs-typo","severity":"minor","summary":"Typo","evidence":"README.md:4"},
        {"id":"wrong-result","severity":"major","summary":"Wrong value","evidence":"lib.rs:4"}
    ]})
    .to_string();
    for value in [None, Some("follow_up")] {
        let d = Drive::new();
        let o = step03(&d, &review, value);
        assert_eq!(
            verdict_line(&o)["crusty_verdict"],
            "CONCERNS",
            "{value:?}: {}",
            show(&o)
        );
        assert!(!d.p("crusty-round-1.json.minor").exists(), "{value:?}");
    }
}

#[test]
fn round_step03_never_runs_or_echoes_an_unrecognised_value() {
    // Normalisation and the WARNING belong to step-02 and the re-review; the
    // round compares with follow_up only and never prints the raw value.
    let d = Drive::new();
    let marker = d.tmp.path().join("pwned");
    for value in [
        "$(touch \"$FIXTURE/pwned\")".to_owned(),
        "`touch \"$FIXTURE/pwned\"`".to_owned(),
        "follow_up; touch \"$FIXTURE/pwned\"".to_owned(),
        "FOLLOW_UP".to_owned(),
        "follow-up".to_owned(),
    ] {
        let o = step03(&d, &two_minor_review(), Some(&value));
        assert_eq!(
            verdict_line(&o)["crusty_verdict"],
            "CONCERNS",
            "{value:?} is not follow_up: {}",
            show(&o)
        );
        assert!(!marker.exists(), "{value:?} was executed");
        assert!(
            !stderr(&o).contains(value.as_str()) && !stdout(&o).contains(value.as_str()),
            "{value:?}: the round never echoes the raw value: {}",
            show(&o)
        );
    }
}

// ===========================================================================
// 2. What crusty-loop step-02 and the re-review pass on
// ===========================================================================

/// `autodrive_loop.sh` double: records its argv (NUL-separated) and reports DONE.
const LOOP_STUB: &str = r#"#!/usr/bin/env bash
printf '%s\0' "$@" > "$STUB_LOG/loop-argv"
printf '{"loop":"crusty","loop_result":"DONE","round_label":"round-1","round_verdict":"CLEAN"}\n'
exit 0
"#;

/// A tool tree with a stub loop driver. Step-02 takes its loop helper and
/// round recipe from `$AMPLIHACK_HOME`; the re-review takes every tool from
/// beside itself, so the real re-review script is copied in next to the stubs.
struct StubTree {
    home: PathBuf,
    tools: PathBuf,
    log: PathBuf,
}

fn stub_tree(d: &Drive) -> StubTree {
    let home = d.tmp.path().join("stub-home");
    let tools = home.join("amplifier-bundle/tools");
    let recipes = home.join("amplifier-bundle/recipes");
    let log = d.tmp.path().join("stub-log");
    for dir in [&tools, &recipes, &log] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(recipes.join("autodrive-crusty-round.yaml"), "name: stub\n").unwrap();
    let write = |name: &str, body: &str| {
        crate::support::exec_stub::write_executable(&tools.join(name), body, 0o755);
    };
    write("autodrive_loop.sh", LOOP_STUB);
    write(
        "autodrive_worktree_guard.sh",
        "#!/usr/bin/env bash\nprintf '%s\\n' \"$2\"\n",
    );
    write(
        "autodrive_pr_worktree.sh",
        "#!/usr/bin/env bash\nprintf '%s\\n' \"$3\"\n",
    );
    write(
        "autodrive_round_evidence.sh",
        "#!/usr/bin/env bash\nprintf '{\"rereview\":\"true\",\"range\":\"code\"}\\n'\n",
    );
    for name in [
        "autodrive_state.sh",
        "autodrive_trust.sh",
        "autodrive_followups.sh",
    ] {
        write(name, "#!/usr/bin/env bash\n:\n");
    }
    write(
        "autodrive_crusty_finalize.sh",
        "#!/usr/bin/env bash\nexit 0\n",
    );
    for real in ["recipe_context_value.sh", "autodrive_crusty_rereview.sh"] {
        write(
            real,
            &std::fs::read_to_string(tools_dir().join(real)).unwrap(),
        );
    }
    StubTree { home, tools, log }
}

fn loop_argv(t: &StubTree) -> Vec<String> {
    std::fs::read(t.log.join("loop-argv"))
        .map(|b| {
            String::from_utf8_lossy(&b)
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Runs crusty-loop step-02 against the stub loop. `value` is the caller's
/// RECIPE_VAR_autodrive_minor_findings (`None`: unset).
fn step02(d: &Drive, value: Option<&str>) -> (Output, Vec<String>) {
    let t = stub_tree(d);
    let script = step_command("autodrive-crusty-loop", "step-02-crusty-loop");
    let mut c = d.script(&script);
    c.env_remove("AUTODRIVE_MINOR_FINDINGS")
        .env("AMPLIHACK_HOME", &t.home)
        .env("STUB_LOG", &t.log)
        .env("REPO_PATH", &d.review)
        .env(
            "RECIPE_VAR_crusty_loop_preflight",
            json!({"should_run":"true","repo":d.repo,"pr":"7","state_dir":d.state}).to_string(),
        )
        .env("RECIPE_VAR_task_description", "Fix issue #2391")
        .env("PR_URL", "https://github.com/o/r/pull/7");
    if let Some(v) = value {
        c.env("RECIPE_VAR_autodrive_minor_findings", v);
    }
    let o = c.output().unwrap();
    let argv = loop_argv(&t);
    (o, argv)
}

/// Runs the real re-review script, copied beside the stub loop.
fn rereview(d: &Drive, value: Option<&str>) -> (Output, Vec<String>) {
    let t = stub_tree(d);
    let mut c = d.cmd("bash");
    c.arg(t.tools.join("autodrive_crusty_rereview.sh"))
        .arg("--repo")
        .arg(&d.repo)
        .arg("--review-dir")
        .arg(&d.review)
        .arg("--state-dir")
        .arg(&d.state)
        .args(["-c", "pr_number=7", "-c", "plan_task_id=#2391"]);
    if let Some(v) = value {
        c.arg("-c").arg(format!("{KEY}={v}"));
    }
    c.arg("--context-from-file")
        .arg(format!("task_description={}", d.task.display()))
        .env_remove("AUTODRIVE_MINOR_FINDINGS")
        .env("STUB_LOG", &t.log);
    let o = c.output().unwrap();
    let argv = loop_argv(&t);
    (o, argv)
}

fn has_second_clean_token(argv: &[String]) -> bool {
    argv.windows(2)
        .any(|w| w[0] == "--also-clean-token" && w[1] == "MINOR_ONLY")
}

/// The values of every `--context autodrive_minor_findings=...` pair.
fn forwarded(argv: &[String]) -> Vec<String> {
    argv.windows(2)
        .filter(|w| w[0] == "--context")
        .filter_map(|w| w[1].strip_prefix(&format!("{KEY}=")).map(str::to_owned))
        .collect()
}

/// Checks one run of a normalising entry point against the expected
/// canonical value and warning count.
fn assert_forwarding(
    who: &str,
    value: Option<&str>,
    o: &Output,
    argv: &[String],
    canonical: &str,
    warnings: usize,
) {
    assert!(o.status.success(), "{who} {value:?}: {}", show(o));
    assert!(
        !argv.is_empty(),
        "{who} {value:?}: loop not started: {}",
        show(o)
    );
    assert_eq!(
        has_second_clean_token(argv),
        canonical == "follow_up",
        "{who} {value:?}: --also-clean-token MINOR_ONLY only under follow_up; argv {argv:?}"
    );
    assert!(
        !argv.iter().any(|a| a == "--also-clean-token") || canonical == "follow_up",
        "{who} {value:?}: no second clean token of any kind under address; argv {argv:?}"
    );
    assert_eq!(
        forwarded(argv),
        [canonical],
        "{who} {value:?}: the canonical value is forwarded to the round exactly once"
    );
    assert_eq!(
        warning_lines(o).len(),
        warnings,
        "{who} {value:?}: WARNING lines: {}",
        show(o)
    );
}

#[test]
fn step02_passes_no_second_clean_token_by_default() {
    for value in ADDRESS_VALUES {
        let d = Drive::new();
        let (o, argv) = step02(&d, value);
        assert_forwarding("step-02", value, &o, &argv, "address", 0);
    }
}

#[test]
fn step02_passes_minor_only_as_a_clean_token_under_follow_up() {
    for value in FOLLOW_UP_VALUES {
        let d = Drive::new();
        let (o, argv) = step02(&d, Some(value));
        assert_forwarding("step-02", Some(value), &o, &argv, "follow_up", 0);
    }
}

#[test]
fn step02_treats_an_unknown_value_as_address_with_one_warning() {
    for value in UNKNOWN_VALUES {
        let d = Drive::new();
        let (o, argv) = step02(&d, Some(value));
        assert_forwarding("step-02", Some(value), &o, &argv, "address", 1);
        let line = &warning_lines(&o)[0];
        assert_eq!(
            line,
            &format!("{WARNING_PREFIX}{value}{WARNING_SUFFIX}"),
            "{}",
            show(&o)
        );
    }
}

#[test]
fn rereview_passes_minor_only_as_a_clean_token_only_under_follow_up() {
    for value in ADDRESS_VALUES {
        let d = Drive::new();
        let (o, argv) = rereview(&d, value);
        assert_forwarding("re-review", value, &o, &argv, "address", 0);
    }
    for value in FOLLOW_UP_VALUES {
        let d = Drive::new();
        let (o, argv) = rereview(&d, Some(value));
        assert_forwarding("re-review", Some(value), &o, &argv, "follow_up", 0);
    }
    for value in UNKNOWN_VALUES {
        let d = Drive::new();
        let (o, argv) = rereview(&d, Some(value));
        assert_forwarding("re-review", Some(value), &o, &argv, "address", 1);
        assert_eq!(
            warning_lines(&o)[0],
            format!("{WARNING_PREFIX}{value}{WARNING_SUFFIX}"),
            "{}",
            show(&o)
        );
    }
}

#[test]
fn an_unknown_value_cannot_run_commands_or_forge_log_lines() {
    let hostile = [
        "$(touch \"$STUB_LOG/pwned\")",
        "`touch \"$STUB_LOG/pwned\"`",
        "x\nWARNING: forged line",
        "red\u{1b}[31mtext",
        "%s%s%n",
    ];
    for value in hostile {
        for who in ["step-02", "re-review"] {
            let d = Drive::new();
            let (o, argv) = if who == "step-02" {
                step02(&d, Some(value))
            } else {
                rereview(&d, Some(value))
            };
            assert_forwarding(who, Some(value), &o, &argv, "address", 1);
            assert!(
                !d.tmp.path().join("stub-log/pwned").exists(),
                "{who} {value:?} was executed"
            );
            let err = stderr(&o);
            assert!(
                !err.lines().any(|l| l.starts_with("WARNING: forged")),
                "{who} {value:?}: a value cannot start its own log line: {}",
                show(&o)
            );
            assert!(
                !err.contains('\u{1b}'),
                "{who} {value:?}: control characters are stripped: {}",
                show(&o)
            );
            let line = &warning_lines(&o)[0];
            assert!(
                line.starts_with(WARNING_PREFIX) && line.ends_with(WARNING_SUFFIX),
                "{who} {value:?}: {line:?}"
            );
            if value == "%s%s%n" {
                assert!(
                    line.contains("%s%s%n"),
                    "{who}: the value is printed as data: {line:?}"
                );
            }
        }
    }
}

// ===========================================================================
// 3. The loop, end to end through the recipe boundary double
// ===========================================================================

/// Rounds after the first use `after-review.json` and `after-action`: the
/// address step of round 1 fixed what Crusty found.
fn address_then_clean(d: &Drive) {
    let inner = std::fs::read_to_string(d.tmp.path().join("round.sh")).unwrap();
    d.set("round-inner.sh", &inner);
    d.set(
        "round.sh",
        r#"if [ "${AUTODRIVE_ROUND_LABEL:-}" != round-1 ] && [ -f "$FIXTURE/after-review.json" ]; then
  cp -- "$FIXTURE/after-review.json" "$FIXTURE/review.json"
  cp -- "$FIXTURE/after-action" "$FIXTURE/action"
fi
exec bash "$FIXTURE/round-inner.sh"
"#,
    );
    d.set(
        "after-review.json",
        r#"{"crusty_verdict":"CLEAN","concerns":[]}"#,
    );
    d.set("after-action", "complete");
}

#[test]
fn default_address_loops_past_an_all_minor_round_until_clean() {
    for value in [None, Some("address")] {
        let d = Drive::new();
        d.minor_findings(value);
        d.set("review.json", &two_minor_review());
        // An unresolved review is not complete: the agent asks for evaluation,
        // and loop-health sees progress still possible.
        d.set("action", "evaluate");
        d.set("eval-verdict", "CONTINUE");
        address_then_clean(&d);
        let o = d.run_loop();
        assert!(
            o.status.success(),
            "{value:?}: the loop ends DONE on CLEAN: {}",
            show(&o)
        );
        let v = json_output(&o);
        assert_eq!(v["loop_result"], "DONE", "{value:?}: {}", show(&o));
        assert_eq!(v["round_verdict"], "CLEAN", "{value:?}: {}", show(&o));
        assert_eq!(
            d.rounds(),
            ["round-1", "round-2"],
            "{value:?}: an all-minor round does not end the loop"
        );
        let r1: Value = serde_json::from_str(&d.read("crusty-round-1.json")).unwrap();
        assert_eq!(r1["crusty_verdict"], "CONCERNS", "{value:?}");
        assert!(
            !d.p("crusty-round-1.json.minor").exists(),
            "{value:?}: nothing is set aside as a follow-up"
        );
        let evals = d.evaluators();
        assert_eq!(evals.len(), 1, "{value:?}: round 1 reached the evaluator");
        let argv = evals[0]["argv"].as_array().unwrap();
        assert!(
            argv.iter().any(|a| a == "loop_round_verdict=CONCERNS"),
            "{value:?}: {argv:?}"
        );
        // The phase finalizer files nothing: there is no minor set.
        let o = d.finalize(false, false);
        assert!(o.status.success() && d.marked(), "{value:?}: {}", show(&o));
        assert_eq!(issue_creates(&d), 0, "{value:?}: no follow-up issue");
        assert!(d.read("followups.tsv").trim().is_empty(), "{value:?}");
    }
}

#[test]
fn default_address_never_completes_on_an_unaddressed_minor_round() {
    // The completion agent may not stop on minor findings under address; if
    // nothing changes, loop-health evaluation stops the loop (no cap).
    let d = Drive::new();
    d.minor_findings(None);
    d.set("review.json", &two_minor_review());
    d.set("action", "evaluate");
    d.set("eval-verdict", "STUCK");
    let o = d.run_loop();
    assert!(!o.status.success(), "{}", show(&o));
    let v = json_output(&o);
    assert_eq!(v["loop_result"], "STUCK", "{}", show(&o));
    assert_ne!(v["round_verdict"], "MINOR_ONLY", "{}", show(&o));
    assert_eq!(d.evaluators().len(), 1);
    assert!(!d.p("crusty-round-1.json.minor").exists());
    assert_eq!(issue_creates(&d), 0);
    assert!(!d.marked());
}

#[test]
fn follow_up_ends_on_minor_only_and_files_each_follow_up() {
    let d = Drive::new();
    d.minor_findings(Some("follow_up"));
    d.set("review.json", &two_minor_review());
    let o = d.run_loop();
    assert!(o.status.success(), "{}", show(&o));
    let v = json_output(&o);
    assert_eq!(v["loop_result"], "DONE", "{}", show(&o));
    assert_eq!(v["round_verdict"], "MINOR_ONLY", "{}", show(&o));
    assert_eq!(d.rounds(), ["round-1"], "MINOR_ONLY ends the loop");
    assert!(d.evaluators().is_empty(), "completion needs no evaluator");
    assert!(d.p("crusty-round-1.json.minor").is_file());
    let o = d.finalize(false, false);
    assert!(o.status.success() && d.marked(), "{}", show(&o));
    assert_eq!(issue_creates(&d), 2, "one follow-up per minor finding");
    let tsv = d.read("followups.tsv");
    for id in ["docs-typo", "naming-2"] {
        assert!(tsv.contains(&format!("{id}\t")), "{id} recorded: {tsv}");
    }
}

#[test]
fn an_unknown_value_runs_the_loop_as_address() {
    let d = Drive::new();
    d.minor_findings(Some("ignore"));
    d.set("review.json", &two_minor_review());
    d.set("action", "evaluate");
    d.set("eval-verdict", "STUCK");
    let o = d.run_loop();
    assert_eq!(json_output(&o)["loop_result"], "STUCK", "{}", show(&o));
    assert!(!d.p("crusty-round-1.json.minor").exists());
    assert_eq!(
        warning_lines(&o),
        [format!("{WARNING_PREFIX}ignore{WARNING_SUFFIX}")],
        "{}",
        show(&o)
    );
}

#[test]
fn merge_ready_rereview_addresses_minor_findings_by_default() {
    for value in [None, Some(""), Some("address")] {
        let d = Drive::new();
        d.enable_rereview();
        d.minor_findings(value);
        d.set("action", "evaluate");
        d.set("eval-verdict", "STUCK");
        let o = d.rereview(false);
        assert!(
            !o.status.success(),
            "{value:?}: an all-minor re-review is not done under address: {}",
            show(&o)
        );
        assert_eq!(
            json_output(&o)["before_round_result"],
            "crusty-rereview-not-done",
            "{value:?}: {}",
            show(&o)
        );
        let latest: Value = serde_json::from_str(&d.read("crusty-latest.json")).unwrap();
        assert_eq!(latest["crusty_verdict"], "CONCERNS", "{value:?}");
        assert_eq!(issue_creates(&d), 0, "{value:?}: nothing filed");
        assert!(!d.marked(), "{value:?}: criterion 3 is not met");
    }
}

#[test]
fn merge_ready_rereview_under_follow_up_files_and_completes() {
    let d = Drive::new();
    d.enable_rereview();
    d.minor_findings(Some("follow_up"));
    let o = d.rereview(false);
    assert!(o.status.success() && d.marked(), "{}", show(&o));
    assert_eq!(
        json_output(&o)["before_round_result"],
        "crusty-rereview-done",
        "{}",
        show(&o)
    );
    assert!(d.read("followups.tsv").contains("docs-typo\t"));
    assert!(d.evaluators().is_empty());
}

// ===========================================================================
// 4. Wiring through the composed recipes
// ===========================================================================

fn step_of(recipe_name: &str, id: &str) -> serde_yaml::Value {
    recipe(recipe_name)["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("{recipe_name}: no step {id}"))
        .clone()
}

#[test]
fn every_composed_recipe_declares_the_setting_with_address_as_the_default() {
    assert_eq!(
        recipe("auto-drive-to-merge")["context"][KEY].as_str(),
        Some("address"),
        "auto-drive-to-merge declares the default"
    );
    for r in [
        "autodrive-crusty-loop",
        "autodrive-crusty-round",
        "autodrive-merge-loop",
        "auto-drive-light",
    ] {
        assert_eq!(
            recipe(r)["context"][KEY].as_str(),
            Some(""),
            "{r} declares {KEY} as empty (empty means address; #1561)"
        );
    }
}

#[test]
fn entry_recipes_pass_the_setting_to_each_loop() {
    for (r, ids) in [
        (
            "auto-drive-to-merge",
            &["autodrive-crusty-loop", "autodrive-merge-loop"][..],
        ),
        (
            "auto-drive-light",
            &["light-escalate", "light-crusty-loop", "light-merge-loop"][..],
        ),
    ] {
        for id in ids {
            assert_eq!(
                step_of(r, id)["context"][KEY].as_str(),
                Some("{{autodrive_minor_findings}}"),
                "{r}/{id} passes {KEY}"
            );
        }
    }
}

// No test runs the round recipe through the runner, so this is the only check
// that keeps step-04 and step-05 running under `address`: an all-minor round
// is CONCERNS there, and these steps need no new condition.
#[test]
fn round_address_steps_still_run_on_concerns() {
    for id in [
        "step-04-address-concerns",
        "step-05-verify-concerns-addressed",
    ] {
        let cond = step_of(ROUND, id)["condition"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(cond, "crusty_verdict.crusty_verdict == 'CONCERNS'", "{id}");
    }
}
