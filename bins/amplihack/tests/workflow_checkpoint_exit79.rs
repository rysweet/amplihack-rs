//! Exit 79 at the two destination-is-head sites of
//! `workflow_publish_checkpoint.sh` (#1554, 17.126).
//!
//! At 918c256cf both sites read
//! `DESTINATION_IS_HEAD=$(...) || { reason=...; exit 1; }`, which turned a
//! terminal 79 from the destination read into 1. Each site is run here as it
//! stands in the wrapper, under `set -euo pipefail`, with `AMPLIHACK_BIN`
//! replaced by a shim that answers `destination-is-head` with a chosen status
//! and a `python3` sentinel that fails with 127. The status the snippet exits
//! with must be the status the engine returned.

use std::fs;
use std::path::PathBuf;
use std::process::Output;

use crate::workflow_checkpoint_support::*;

const PUBLISH: &str = "workflow_publish_checkpoint.sh";

/// Each destination-is-head site as a runnable block: the assignment (and a
/// preceding `rc=0` line, if any) through the line that exits with the
/// captured status, closed so every `if` it opened has its `fi`.
fn site_blocks() -> Vec<String> {
    let body = fs::read_to_string(tools_dir().join(PUBLISH)).unwrap();
    let lines: Vec<&str> = body.lines().collect();
    let mut blocks = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !(l.contains("destination-is-head") && l.contains("DESTINATION_IS_HEAD=")) {
            continue;
        }
        let start = if i > 0 && lines[i - 1].trim_end().ends_with("rc=0") {
            i - 1
        } else {
            i
        };
        let mut end = (i..lines.len())
            .find(|&j| lines[j].contains("exit \"$") || lines[j].contains("exit $"))
            .unwrap_or_else(|| panic!("site at line {} never exits with a status", i + 1));
        let opens = |s: &[&str]| {
            s.iter()
                .filter(|l| l.trim_start().starts_with("if "))
                .count()
        };
        let closes = |s: &[&str]| {
            s.iter()
                .filter(|l| l.trim() == "fi" || l.trim_end().ends_with("; fi"))
                .count()
        };
        while opens(&lines[start..=end]) > closes(&lines[start..=end]) && end + 1 < lines.len() {
            end += 1;
        }
        blocks.push(lines[start..=end].join("\n"));
    }
    blocks
}

/// Run one site with the engine answering `status` (and `stdout`).
fn run_site(block: &str, status_code: i32, stdout: &str) -> (Output, Sandbox, PathBuf) {
    let sb = Sandbox::new();
    let python_marker = sb.python3_marker_shim();
    let calls = sb.path().join("engine-calls");
    let shim = sb.shims().join("amplihack-engine");
    write_exec(
        &shim,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{calls}'\n\
             if [ \"$3\" = destination-is-head ]; then printf '{stdout}'; exit {status_code}; fi\n\
             exit 70\n",
            calls = calls.display(),
        ),
    );
    let script = format!(
        "set -euo pipefail\n\
         reason=\"\"\n\
         trap 'printf \"reason=%s\\n\" \"$reason\" >&2' EXIT\n\
         workflow_checkpoint() {{ \"$AMPLIHACK_BIN\" internal workflow-checkpoint \"$@\"; }}\n\
         workflow_checkpoint_python() {{ python3 -I - \"$@\" </dev/null; }}\n\
         {block}\n\
         printf 'reached:%s\\n' \"$DESTINATION_IS_HEAD\"\n"
    );
    let out = sb
        .command("bash")
        .arg("-c")
        .arg(script)
        .env("AMPLIHACK_BIN", &shim)
        .output()
        .unwrap();
    let _ = python_marker;
    (out, sb, calls)
}

#[test]
fn the_wrapper_has_exactly_two_destination_is_head_sites() {
    assert_eq!(site_blocks().len(), 2, "{:#?}", site_blocks());
}

/// A real 79 from one `destination-is-head` site's read stops the wrapper
/// with 79 (decision 17.126), through the engine and never python3.
fn assert_site_reports_a_terminal_79(site: usize) {
    let blocks = site_blocks();
    let block = &blocks[site - 1];
    let (out, sb, calls) = run_site(block, 79, "");
    assert_eq!(
        status(&out),
        79,
        "site {site}: 79 flattened\n{block}\nstderr: {}",
        stderr(&out)
    );
    assert!(
        fs::read_to_string(&calls)
            .unwrap_or_default()
            .contains("destination-is-head"),
        "site {site}: the engine shim was never called"
    );
    assert!(
        !sb.path().join("python3-was-called").exists(),
        "site {site}: python3 ran"
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("reached:"));
    assert!(
        stderr(&out).contains("reason=publication-destination-inspection-failed"),
        "site {site}: {}",
        stderr(&out)
    );
}

/// Allowlist row 17.126-dest-read-79-site-1.
#[test]
fn site_1_reports_a_terminal_79_from_the_destination_read_as_79() {
    assert_site_reports_a_terminal_79(1);
}

/// Allowlist row 17.126-dest-read-79-site-2.
#[test]
fn site_2_reports_a_terminal_79_from_the_destination_read_as_79() {
    assert_site_reports_a_terminal_79(2);
}

#[test]
fn other_destination_read_failures_keep_their_own_status() {
    for (n, block) in site_blocks().iter().enumerate() {
        for code in [1, 2, 78] {
            let (out, _sb, _calls) = run_site(block, code, "");
            assert_eq!(
                status(&out),
                code,
                "site {}: status {code} rewritten",
                n + 1
            );
            assert!(!String::from_utf8_lossy(&out.stdout).contains("reached:"));
        }
    }
}

#[test]
fn a_successful_destination_read_continues_with_its_answer() {
    for (n, block) in site_blocks().iter().enumerate() {
        for answer in ["true", "false"] {
            let (out, _sb, _calls) = run_site(block, 0, &format!("{answer}\\n"));
            assert_eq!(status(&out), 0, "site {}: {}", n + 1, stderr(&out));
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                format!("reached:{answer}\n"),
                "site {}",
                n + 1
            );
        }
    }
}

#[test]
fn no_destination_is_head_site_flattens_its_status_to_1() {
    for block in site_blocks() {
        assert!(!block.contains("exit 1"), "{block}");
        assert!(!block.contains("|| true"), "{block}");
        assert!(!block.contains("workflow_checkpoint_python"), "{block}");
    }
}
