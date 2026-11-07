//! Issue #1983: a run started with claude stays in claude, and a run started
//! with codex stays in codex, unless the user explicitly asks for another CLI.
//!
//! Every test starts the real `amplihack` binary under a cleared environment
//! and asserts on what fake `claude` and `codex` scripts recorded when they
//! ran, so each check crosses a real process boundary. No real CLI runs, and
//! no test touches `~/.claude`, `~/.claude.json`, `~/.codex` or a credential
//! file. See `docs/testing/CLI_STAYS_PUT_TESTS.md` for the level table.

#![cfg(unix)]

#[path = "issue_1983_cli_stays_put/explicit_change.rs"]
mod explicit_change;
#[path = "issue_1983_cli_stays_put/explicit_launch.rs"]
mod explicit_launch;
#[path = "issue_1983_cli_stays_put/fixture.rs"]
mod fixture;
#[path = "issue_1983_cli_stays_put/nested.rs"]
mod nested;
#[path = "issue_1983_cli_stays_put/record.rs"]
mod record;
#[path = "issue_1983_cli_stays_put/script.rs"]
mod script;
#[path = "issue_1983_cli_stays_put/steps.rs"]
mod steps;
#[path = "issue_1983_cli_stays_put/subprocess_safe.rs"]
mod subprocess_safe;
#[path = "issue_1983_cli_stays_put/workstreams.rs"]
mod workstreams;
