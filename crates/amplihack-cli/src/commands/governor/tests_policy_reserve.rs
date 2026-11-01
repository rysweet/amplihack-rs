//! Issue #1811: step 4 (the reserve) for nested starts. Only a start under
//! a running `recipe run`'s slot skips the reserve; one under an
//! interactive session's slot is new work and is held like a top-level
//! start. See "Nesting" in docs/reference/capacity-governor.md.

use super::model::{Class, DecisionKind, ReasonCode, Start};
use super::policy::decide;
use super::tests_policy::NOW;
use super::tests_policy::fixture::{budget, check, inputs};

/// A recipe admitted above the reserve that crosses it part-way through
/// must finish: its agent steps run under the recipe's slot and are not
/// held at the reserve, so the run is not abandoned with its points already
/// spent.
#[test]
fn step4_reserve_does_not_hold_agent_steps_of_a_running_recipe() {
    let mut i = inputs(vec![budget("alpha", true, 0.0, 95.0)], Class::Ordinary);
    i.request.start = Start::Agent;
    i.request.parent = Some(Start::Recipe);
    i.request.deadline = NOW;
    check(&decide(&i, NOW), DecisionKind::Admit, ReasonCode::Nested);
    // The same start at top level is held.
    i.request.parent = None;
    check(&decide(&i, NOW), DecisionKind::Wait, ReasonCode::Reserve);
}

/// A sub-recipe started by a step of a running recipe is part of that
/// recipe's plan and finishes with it.
#[test]
fn step4_reserve_does_not_hold_a_sub_recipe_of_a_running_recipe() {
    let mut i = inputs(vec![budget("alpha", true, 0.0, 95.0)], Class::Ordinary);
    i.request.start = Start::Recipe;
    i.request.parent = Some(Start::Recipe);
    check(&decide(&i, NOW), DecisionKind::Admit, ReasonCode::Nested);
}

/// Crusty round 3 on #1831: an interactive session admitted above the
/// reserve can keep starting `recipe run`s after the weekly remainder falls
/// below it. Each is new work and must wait at the reserve, or the reserve
/// only gates the start of the session itself.
#[test]
fn step4_reserve_holds_a_recipe_run_started_under_a_session() {
    let mut i = inputs(vec![budget("alpha", true, 0.0, 95.0)], Class::Ordinary);
    i.request.start = Start::Recipe;
    i.request.parent = Some(Start::Agent);
    check(&decide(&i, NOW), DecisionKind::Wait, ReasonCode::Reserve);
}

/// The same holds for an agent launch started directly from the session.
#[test]
fn step4_reserve_holds_an_agent_launch_started_under_a_session() {
    let mut i = inputs(vec![budget("alpha", true, 0.0, 95.0)], Class::Ordinary);
    i.request.start = Start::Agent;
    i.request.parent = Some(Start::Agent);
    check(&decide(&i, NOW), DecisionKind::Wait, ReasonCode::Reserve);
}

/// A protected class under a session may still spend the reserve.
#[test]
fn step4_protected_work_under_a_session_spends_the_reserve() {
    for class in [Class::Coordination, Class::Recovery, Class::Review] {
        let mut i = inputs(vec![budget("alpha", true, 0.0, 95.0)], class);
        i.request.start = Start::Recipe;
        i.request.parent = Some(Start::Agent);
        check(&decide(&i, NOW), DecisionKind::Admit, ReasonCode::Nested);
    }
}
