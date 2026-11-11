//! Review regressions: private preparation under ordinary umasks and dry-run receipts.

use super::{done_record, fixture::Drive};
use crate::support::auto_drive_light::{private_file, show};
use serde_json::json;

#[cfg(unix)]
#[test]
fn shell_preflight_makes_existing_0755_state_usable_by_completion() {
    use crate::support::auto_drive_light::tools_dir;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    let d = Drive::new();
    let record = d.record(1);
    let evidence = std::fs::read(&record).unwrap();
    std::fs::set_permissions(&d.state, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = d
        .cmd("bash")
        .args([
            "-c",
            "source \"$1/autodrive_state.sh\" && autodrive_private_dir \"$2\"",
            "state-preflight",
        ])
        .arg(tools_dir())
        .arg(&d.state)
        .output()
        .unwrap();
    assert!(o.status.success(), "shell preflight: {}", show(&o));

    let prepared = d.prepare(1, "");
    assert!(Path::new(prepared["private_state"].as_str().unwrap()).is_file());
    assert_eq!(
        std::fs::metadata(&d.state).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(std::fs::read(record).unwrap(), evidence);
}

#[cfg(unix)]
#[test]
fn completion_preparation_and_evaluation_are_private_under_caller_umasks() {
    use super::fixture::json_output;
    use crate::support::amplihack_bin::amplihack_bin;
    use crate::support::auto_drive_light::tools_dir;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    for mask in ["000", "022", "077"] {
        let d = Drive::new();
        d.set("review.json", r#"{"crusty_verdict":"CONCERNS","concerns":[{"id":"wrong-value","severity":"major","summary":"Wrong value","evidence":"lib.rs:2"}]}"#);
        d.set("eval-verdict", "CONTINUE");
        let record = d.record(1);
        // Set the mask only in the helper's subprocess, never the test harness.
        let command = |op: &str| {
            let mut c = d.cmd("bash");
            c.args([
                "-c",
                "umask \"$1\"; shift; exec \"$@\"",
                "completion-umask",
                mask,
            ])
            .arg(amplihack_bin())
            .args(["orch", "helper", op]);
            c
        };
        let o = command("autodrive-completion-prepare")
            .arg("--state-dir")
            .arg(&d.state)
            .args(["--round-label", "round-1"])
            .arg("--record")
            .arg(record)
            .arg("--repo")
            .arg(&d.repo)
            .arg("--review-dir")
            .arg(&d.review)
            .arg("--launch-tools")
            .arg(tools_dir())
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "prepare under umask {mask}: {}",
            show(&o)
        );
        let prepared = json_output(&o);
        let state = Path::new(prepared["private_state"].as_str().unwrap());
        let driver = state.parent().unwrap();
        let root = driver.parent().unwrap();
        for path in [root, driver, &root.join("output")] {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700,
                "{} must be private under umask {mask}",
                path.display()
            );
        }
        d.decide(&prepared, "evaluate");
        let o = command("autodrive-completion-dispatch")
            .arg("--private-state")
            .arg(state)
            .args(["--child-exit-code", "0"])
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "evaluate under umask {mask}: {}",
            show(&o)
        );
        assert_eq!(json_output(&o)["loop_result"], "CONTINUE");
        assert_eq!(
            d.evaluators().len(),
            1,
            "private evaluator context must reach the evaluator"
        );
    }
}

#[test]
fn clean_dry_run_finishes_without_github_calls() {
    let d = Drive::new();
    d.set("review.json", r#"{"crusty_verdict":"CLEAN","concerns":[]}"#);
    d.record(1);
    private_file(
        &d.p("crusty-result.json"),
        &json!({
            "loop":"crusty", "loop_result":"DONE", "round_label":"round-1",
            "round_verdict":"CLEAN", "source":"objective-completion", "evaluator_invoked":false
        })
        .to_string(),
    );
    let calls = d.fakes.gh_calls();
    let o = d.finalize(true, false);
    assert!(
        o.status.success() && d.marked(),
        "clean dry run: {}",
        show(&o)
    );
    assert_eq!(
        d.fakes.gh_calls(),
        calls,
        "dry runs must skip filing entirely"
    );
}

#[test]
fn dry_run_accepts_existing_receipts_without_github_calls() {
    let d = Drive::new();
    done_record(&d);
    let o = d.finalize(false, false);
    assert!(
        o.status.success() && d.marked(),
        "baseline receipts: {}",
        show(&o)
    );
    let receipts = d.read("followups.tsv");
    let parent = d.read("followups.parent");
    assert!(!receipts.is_empty() && !parent.is_empty());
    let calls = d.fakes.gh_calls();
    let o = d.finalize(true, false);
    assert!(
        o.status.success() && d.marked(),
        "recorded dry run: {}",
        show(&o)
    );
    assert_eq!(d.read("followups.tsv"), receipts);
    assert_eq!(d.read("followups.parent"), parent);
    assert_eq!(
        d.fakes.gh_calls(),
        calls,
        "dry runs only verify local receipts"
    );
}

#[test]
fn dry_run_refuses_missing_issue_or_parent_receipts_and_clears_stale_completion() {
    for receipt in ["followups.tsv", "followups.parent"] {
        let d = Drive::new();
        done_record(&d);
        let o = d.finalize(false, false);
        assert!(o.status.success() && d.marked(), "baseline: {}", show(&o));
        std::fs::remove_file(d.p(receipt)).unwrap();
        let calls = d.fakes.gh_calls();
        let o = d.finalize(true, false);
        assert!(
            !o.status.success() && !d.marked(),
            "missing {receipt}: {}",
            show(&o)
        );
        assert_eq!(
            d.fakes.gh_calls(),
            calls,
            "dry runs cannot repair missing receipts"
        );
    }
}
