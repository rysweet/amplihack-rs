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
    // check (#2333). Each outcome is matched by name: a head that moved fails
    // closed as before, while a read that did not answer leaves the head
    // unconfirmed and is judged by the loop-health evaluator, with the fault
    // in front of it. It is not an invalid decision, and it is not a
    // completion either.
    let mut head_unavailable = None;
    if complete {
        match evidence::head_check(&state)? {
            evidence::HeadCheck::Matches => {}
            evidence::HeadCheck::Differs { local, remote } => bail!(
                "current PR head {remote} differs from the reviewed evidence at {local}; \
                 the completion claim does not cover the branch as it now stands"
            ),
            evidence::HeadCheck::Unavailable(u) => {
                // The guard the Complete arm applies, kept ahead of the
                // evaluator so an unreadable head cannot route a decision
                // past a check it would otherwise have failed.
                ensure!(
                    decision.action == Action::Complete && args.child_exit_code == 0,
                    "the PR head could not be read and the decision cannot complete"
                );
                head_unavailable = Some(u);
            }
        }
    }
    if let Some(u) = head_unavailable {
        eprintln!(
            "WARNING: the PR head could not be read on either route ({}); the round goes to the loop-health evaluator rather than completing",
            u.token()
        );
        secure_fs::create(
            secure_fs::Scope::of(&state.state_dir),
            &args.private_state.with_extension("dispatched"),
            state.run_id.as_bytes(),
        )?;
        let (mut result, code) =
            evaluator::run(&state, &record, args.child_exit_code, Some(u.token()))?;
        result["head_read"] = serde_json::json!(format!("unavailable:{}", u.token()));
        println!("{result}");
        return Ok(code);
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
                "round_verdict":record["crusty_verdict"],"source":"objective-completion","evaluator_invoked":false,
                "head_read":"matches"}),
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
            evaluator::run(&state, &record, args.child_exit_code, None)?
        }
    };
    // The dispatch result is iteration evidence only. Follow-up receipts and
    // phase completion belong to the finalization step, never to this adapter.
    println!("{result}");
    Ok(code)
}
