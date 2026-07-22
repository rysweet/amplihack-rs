//! TDD (RED): daemon strategy selection + idempotent plan (D5).
//!
//! Contract — `amplihack_cli::commands::signal::daemon`:
//!
//! * `choose_strategy(systemd_user_available)` prefers `systemd --user`, falling
//!   back to `nohup`.
//! * `plan_daemon(systemd_available, already_running, endpoint)` is pure and
//!   idempotent: when the loopback JSON-RPC daemon is already running it plans a
//!   no-op (`needs_start() == false`); otherwise it plans a start using the
//!   chosen strategy and carries the resolved loopback endpoint.
#![cfg(feature = "signal")]

use amplihack_cli::commands::signal::daemon::{DaemonStrategy, choose_strategy, plan_daemon};

#[test]
fn prefers_systemd_user_when_available() {
    assert_eq!(choose_strategy(true), DaemonStrategy::SystemdUser);
}

#[test]
fn falls_back_to_nohup_without_systemd() {
    assert_eq!(choose_strategy(false), DaemonStrategy::Nohup);
}

#[test]
fn already_running_is_a_noop_plan() {
    let plan = plan_daemon(true, true, "127.0.0.1:7583");
    assert!(
        !plan.needs_start(),
        "an already-running daemon must not be restarted"
    );
    assert!(plan.already_running);
    assert_eq!(plan.endpoint, "127.0.0.1:7583");
}

#[test]
fn not_running_plans_a_start_with_chosen_strategy() {
    let plan = plan_daemon(false, false, "127.0.0.1:9100");
    assert!(plan.needs_start());
    assert_eq!(plan.strategy, DaemonStrategy::Nohup);
    assert_eq!(plan.endpoint, "127.0.0.1:9100");

    let plan = plan_daemon(true, false, "127.0.0.1:9100");
    assert_eq!(plan.strategy, DaemonStrategy::SystemdUser);
    assert!(plan.needs_start());
}

#[test]
fn planning_twice_is_stable_when_running() {
    let a = plan_daemon(true, true, "127.0.0.1:7583");
    let b = plan_daemon(true, true, "127.0.0.1:7583");
    assert_eq!(a, b, "idempotent planning yields identical no-op plans");
}
