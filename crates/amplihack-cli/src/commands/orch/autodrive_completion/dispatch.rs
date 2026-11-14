use super::{
    Dispatch,
    decision::{Action, Decision},
    evaluator, evidence, private_state, secure_fs,
};
use anyhow::{Result, bail, ensure};

pub fn run(args: Dispatch) -> Result<i32> {
    let state = private_state::load(&args.private_state)?;
    if matches!(args.child_exit_code, 79 | 75) {
        let result = if args.child_exit_code == 79 {
            "TERMINAL_POLICY_REFUSAL"
        } else {
            "ORPHANED"
        };
        println!(
            "{}",
            serde_json::json!({"loop":"crusty","loop_result":result,"evaluator_invoked":false})
        );
        return Ok(args.child_exit_code);
    }
    let decision: Decision = serde_json::from_slice(&secure_fs::read(
        secure_fs::Scope::of(&state.state_dir),
        &state.decision_file,
        16 * 1024,
    )?)?;
    decision.validate()?;
    let snapshot = private_state::snapshot(&args.private_state)?;
    ensure!(
        decision.run_id == state.run_id
            && decision.round_label == state.round_label
            && decision.round_record_hash == snapshot.record_hash,
        "decision identity or record hash mismatch"
    );
    let record = evidence::validate(&state, &snapshot)?;
    let complete = matches!(
        record["crusty_verdict"].as_str(),
        Some("CLEAN" | "MINOR_ONLY")
    );
    // The remote head is read only for a completion claim, after every record
    // check (#2333). Each outcome is matched by name: a difference fails
    // closed as before, an unavailable read is WAITING and never DONE.
    if complete {
        match evidence::head_check(&state)? {
            evidence::HeadCheck::Matches => {}
            evidence::HeadCheck::Differs => {
                bail!("current PR head differs from reviewed evidence")
            }
            evidence::HeadCheck::Unavailable(u) => {
                ensure!(
                    decision.action == Action::Complete && args.child_exit_code == 0,
                    "current PR head is unavailable and the decision cannot complete"
                );
                return Ok(waiting(&state, &u));
            }
        }
    }
    let (result, code) = match decision.action {
        Action::Complete => {
            ensure!(
                args.child_exit_code == 0,
                "unsuccessful child cannot complete the objective"
            );
            ensure!(complete, "completion claim has unresolved concerns");
            secure_fs::create(
                secure_fs::Scope::of(&state.state_dir),
                &args.private_state.with_extension("dispatched"),
                state.run_id.as_bytes(),
            )?;
            (
                serde_json::json!({"loop":"crusty","loop_result":"DONE","round_label":state.round_label,
                "round_verdict":record["crusty_verdict"],"source":"objective-completion","evaluator_invoked":false}),
                0,
            )
        }
        Action::Evaluate => {
            ensure!(
                !complete,
                "completed objective must select complete before convergence evaluation"
            );
            secure_fs::create(
                secure_fs::Scope::of(&state.state_dir),
                &args.private_state.with_extension("dispatched"),
                state.run_id.as_bytes(),
            )?;
            evaluator::run(&state, &record, args.child_exit_code)?
        }
    };
    // The dispatch result is iteration evidence only. Follow-up receipts and
    // phase completion belong to the finalization step, never to this adapter.
    println!("{result}");
    Ok(code)
}

/// WAITING on an unavailable head: no `.dispatched` marker, no evaluator, no
/// change to the sealed evidence, so the same round dispatches again later.
fn waiting(state: &private_state::State, u: &super::super::autodrive_gh::Unavailable) -> i32 {
    let target = match (u.rate_limited, u.reset_epoch) {
        (true, Some(e)) => format!("ratelimit-reset@{e}"),
        (true, None) => "ratelimit-reset@unknown".to_string(),
        (false, _) if u.reason.starts_with("pr-number-unresolved") => "read@pr-number".to_string(),
        (false, _) => "read@pr-head".to_string(),
    };
    eprintln!(
        "WARNING: the PR head is unavailable on every read path ({}); the round is WAITING, not complete",
        u.reason
    );
    println!(
        "{}",
        serde_json::json!({"loop":"crusty","loop_result":"WAITING","round_label":state.round_label,
            "reason":"the PR head could not be read on any path","wait_reason":"head-unavailable",
            "wait_target":target,"evaluator_invoked":false})
    );
    super::EXIT_WAITING
}
