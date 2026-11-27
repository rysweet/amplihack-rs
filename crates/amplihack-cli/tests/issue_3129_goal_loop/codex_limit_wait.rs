//! Issue #2952: a goal-loop turn whose agent step meets the Codex credits
//! line waits for the limit to clear, then the loop starts its next turn.
//!
//! The run is the real binary, the pinned runner and the real recipes, with
//! the stand-in gh and the stub agent of `support`. The stub prints the
//! credits line on stderr on its first call and exits 1. The step's limit
//! wait has no reset to wait for, so it ends only on an event: here a login
//! change in `$HOME/.claude`, made once the wait has written a `progress`
//! line and so holds its baseline. The wait's `start` and `end` lines are
//! read from the run's recipe log. Nothing depends on a wall-clock window;
//! the only bound is the shared test ceiling.

#![cfg(target_os = "linux")]

use super::support::{Harness, MERGE_WHEN_GREEN, describe};
use crate::issue_1618_fixture::{TEST_NOW, WAIT_PREFIX, toggle_login};
use crate::support::wait::{self, CEILING};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Instant;

const CREDITS_ERROR: &str = "ERROR: Your workspace is out of credits. Add credits to continue.";

/// Every regular file under `dir` whose text holds `needle`, skipping the
/// bundle copy and the bare origin.
fn files_holding(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            let name = entry.file_name();
            if name != "trusted" && name != "origin.git" && name != ".git" {
                found.extend(files_holding(&path, needle));
            }
        } else if kind.is_file()
            && fs::read(&path).is_ok_and(|b| String::from_utf8_lossy(&b).contains(needle))
        {
            found.push(path);
        }
    }
    found
}

/// The wait's own lines, without their prefix, from every file under `dir`.
fn wait_lines(dir: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    for path in files_holding(dir, WAIT_PREFIX) {
        let text = String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned();
        lines.extend(
            text.lines()
                .filter_map(|l| l.split_once(WAIT_PREFIX).map(|(_, rest)| rest.to_string())),
        );
    }
    lines
}

#[test]
fn a_turn_that_meets_the_codex_credits_line_waits_then_the_next_turn_starts() {
    let h = Harness::new();
    let login = h.home.join(".claude");
    fs::create_dir_all(&login).unwrap();
    h.policy(&format!(
        r#"
if [ ! -e "$STUB_LOG/limited" ]; then
  : > "$STUB_LOG/limited"
  echo "{CREDITS_ERROR}" >&2
  exit 1
fi
if [ ! -e "$STUB_LOG/t1" ]; then
  : > "$STUB_LOG/t1"
  echo '{{"action":"continue","args":{{}},"why":"The limit cleared and I carried on."}}'
  exit 0
fi
{MERGE_WHEN_GREEN}
"#
    ));
    let mut c = h.drive_command("auto-drive-to-merge", &["no_merge=false"]);
    let stdout_path = h.base.join("drive.stdout");
    let stderr_path = h.base.join("drive.stderr");
    c.env("AMPLIHACK_RECIPE_LOG", h.base.join("recipe-log.txt"))
        .env("TZ", "America/Los_Angeles")
        .env("AMPLIHACK_GOVERNOR", "off")
        .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
        .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW", TEST_NOW.to_string())
        .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "1000000")
        .stdout(Stdio::from(fs::File::create(&stdout_path).unwrap()))
        .stderr(Stdio::from(fs::File::create(&stderr_path).unwrap()));
    let start = Instant::now();
    let mut child = wait::spawn_once("the goal loop", &mut c);

    // The event is made once the wait reports progress, so it is seen as a
    // change and not taken in as the wait's baseline.
    let mut exited = None;
    wait::wait_until_with_ceiling("the turn's limit wait to report progress", CEILING, || {
        if let Ok(Some(status)) = child.try_wait() {
            exited = Some(status);
            return true;
        }
        wait_lines(&h.base)
            .iter()
            .any(|l| l.starts_with("progress "))
    });
    if exited.is_none() {
        toggle_login(&login);
    }
    let status = match exited {
        Some(s) => s,
        None => wait::wait_child_with_ceiling(
            "the goal loop",
            &mut child,
            CEILING.saturating_sub(start.elapsed()),
        ),
    };
    let out = Output {
        status,
        stdout: fs::read(&stdout_path).unwrap(),
        stderr: fs::read(&stderr_path).unwrap(),
    };

    assert!(
        exited.is_none(),
        "the loop ended before its turn's limit wait reported progress: {}",
        describe(&out)
    );
    // The wait's `start` and `end` lines, as the run's recipe log keeps them.
    let log = fs::read_to_string(h.base.join("recipe-log.txt")).unwrap_or_default();
    let waits: Vec<&str> = log
        .lines()
        .filter_map(|l| l.strip_prefix(WAIT_PREFIX))
        .collect();
    assert_eq!(
        waits,
        [
            "waiting_for_capacity reason=credits account=unknown until=none",
            "start reason=usage_limit wait=capacity account=unknown reset=none until=none bound=none",
            "end reason=login changed",
        ],
        "one wait, on the credits line, ended by the login change\n{}",
        describe(&out)
    );
    assert_eq!(
        h.loop_end()["end"],
        json!("merged"),
        "{}\n{}",
        h.loop_end(),
        describe(&out)
    );
    assert_eq!(h.turns(), vec!["001", "002"], "{}", describe(&out));
    assert_eq!(
        h.result("001")["outcome"],
        json!("continued"),
        "the limit was waited out inside turn 1, which then decided"
    );
    let agent_turns: Vec<String> = fs::read_to_string(h.base.join("agent-turns.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| Path::new(l).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        agent_turns,
        ["001", "001", "002"],
        "turn 1's agent ran again after the wait, and turn 2 followed"
    );
    assert_eq!(
        h.reading("002")["previous"]["action"],
        json!("continue"),
        "turn 2 followed the decision turn 1 made after the wait"
    );
    assert_eq!(h.merge_calls().len(), 1, "{:?}", h.calls());
}
