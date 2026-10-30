//! Issue #1714 crusty round 2: the PR-head fetch in the open-PR check must
//! never prompt or wait without limit, and must not run for a PR the view
//! already rules out.
//!
//! `GIT_TERMINAL_PROMPT=0` only stops git's own HTTPS credential prompt; ssh
//! reads a key passphrase or an unknown host key from /dev/tty, and stock
//! macOS has no `timeout(1)`. Here origin is an SSH remote
//! (`git@github.com:rysweet/fixture.git`) and a fake `ssh` first on PATH
//! records its arguments and exits 255, as a refused connection would. The
//! tests check that:
//! - git starts ssh with `BatchMode=yes` and the connection limits, so ssh
//!   fails instead of prompting, whether or not `timeout` is installed;
//! - a `core.sshCommand` the user configured is kept, with the options added;
//! - the failed fetch is no signal (fail closed), not an error;
//! - a closed PR whose head is missing locally never starts a fetch at all.

use std::fs;

use super::ancestor::{drop_pushed_branch, has_object, merge_main_on_pushed, prune, view_at};
use super::fixture::*;

const SSH_ORIGIN: &str = "git@github.com:rysweet/fixture.git";

const FAKE_SSH: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH_FIXTURE_DIR/ssh-calls.log"
exit 255
"#;

/// A fixture whose origin is an SSH remote, whose PR head M (a fast-forward
/// of the local HEAD) is not in the local object store, and whose `ssh` is
/// the recording fake.
fn ssh_fixture() -> (Fixture, String) {
    let fx = Fixture::new();
    let (_, pr_head) = merge_main_on_pushed(&fx);
    drop_pushed_branch(&fx);
    prune(&fx, &pr_head);
    fx.git(&["remote", "set-url", "origin", SSH_ORIGIN]);
    let ssh = fx.tmp.path().join("bin/ssh");
    fs::write(&ssh, FAKE_SSH).expect("write fake ssh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).expect("chmod ssh");
    }
    (fx, pr_head)
}

fn ssh_calls(fx: &Fixture) -> Vec<String> {
    fs::read_to_string(fx.tmp.path().join("ssh-calls.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn assert_fails_closed(outcome: &Outcome, why: &str) {
    assert!(
        !outcome.success(),
        "{why}: no signal\n{}",
        outcome.describe()
    );
    assert_eq!(
        outcome.stdout,
        "",
        "{why}: no stdout\n{}",
        outcome.describe()
    );
}

#[test]
fn the_ssh_fetch_runs_in_batch_mode_with_connection_limits() {
    let (fx, pr_head) = ssh_fixture();
    view_at(&fx, &pr_head, green_rollup());

    assert_fails_closed(&fx.signal(BRANCH, Some("7")), "refused ssh fetch");
    let calls = ssh_calls(&fx);
    assert_eq!(calls.len(), 1, "exactly one fetch attempt: {calls:?}");
    for option in [
        "-o BatchMode=yes",
        "-o ConnectTimeout=15",
        "-o ServerAliveInterval=15",
        "-o ServerAliveCountMax=2",
    ] {
        assert!(
            calls[0].contains(option),
            "ssh must be started with `{option}` so it fails instead of prompting: {}",
            calls[0]
        );
    }
    assert!(
        calls[0].contains("git@github.com"),
        "the fake ssh was the one git used: {}",
        calls[0]
    );
    assert!(!has_object(&fx, &pr_head), "nothing was fetched");
}

#[test]
fn a_configured_core_ssh_command_is_kept_with_batch_mode_added() {
    let (fx, pr_head) = ssh_fixture();
    let ssh = fx.tmp.path().join("bin/ssh");
    fx.git(&[
        "config",
        "core.sshCommand",
        &format!("{} -o IdentitiesOnly=yes", ssh.display()),
    ]);
    view_at(&fx, &pr_head, green_rollup());

    assert_fails_closed(&fx.signal(BRANCH, Some("7")), "refused ssh fetch");
    let calls = ssh_calls(&fx);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(
        calls[0].contains("-o IdentitiesOnly=yes") && calls[0].contains("-o BatchMode=yes"),
        "the user's ssh command is kept and made non-interactive: {}",
        calls[0]
    );
}

#[test]
fn a_pr_the_view_rules_out_never_starts_a_fetch() {
    let (fx, pr_head) = ssh_fixture();
    let rejected = [
        ("closed", "state", serde_json::json!("CLOSED")),
        ("from a fork", "isCrossRepository", serde_json::json!(true)),
        (
            "on another branch",
            "headRefName",
            serde_json::json!("other"),
        ),
        ("another number", "number", serde_json::json!(8)),
    ];
    for (name, key, value) in rejected {
        let mut view = open_pr(green_rollup());
        view["headRefOid"] = serde_json::json!(pr_head);
        view[key] = value;
        fx.gh_view(&view);
        assert_fails_closed(&fx.signal(BRANCH, Some("7")), name);
        assert!(
            ssh_calls(&fx).is_empty(),
            "{name}: a PR rejected by the view must not cost a fetch: {:?}",
            ssh_calls(&fx)
        );
    }
}
