use super::{
    Dispatch,
    decision::{Action, Decision},
    evaluator, evidence, private_state, secure_fs,
};
use anyhow::{Result, ensure};

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
    let record = evidence::validate(
        &state,
        &snapshot,
        matches!(decision.action, Action::Complete),
    )?;
    let complete = matches!(
        record["crusty_verdict"].as_str(),
        Some("CLEAN" | "MINOR_ONLY")
    ) && record["reviewed_head_sha"] == record["head_sha"]
        && record["commits_this_round"].as_u64() == Some(0)
        && args.child_exit_code == 0;
    let (result, code) = match decision.action {
        Action::Complete => {
            // #2769: `--child-exit-code` is `ROUND_RC` in `autodrive_loop.sh`,
            // the exit status of whichever round that script ran: the Crusty
            // round or the merge-ready round. Dispatch is called on the Crusty
            // branch, so this arm sees the Crusty round's status. While the
            // #2663 outcome rule judged every `autodrive-*` sub-recipe by
            // whether it merged, that status was non-zero whatever the round
            // did and this check was constant-false, so the arm never fired.
            // Narrowing the rule to the two entry points makes the check
            // load-bearing: it now carries the round's real verdict, which is
            // meaningful only while every step in `autodrive-crusty-round.yaml`
            // is fatal. That is an assumption about the shipped recipe, not a
            // property enforced here: a step marked `continue_on_error` would
            // let a failed round exit 0 and complete the objective.
            // `tests/issue_2769_autodrive_outcome_scope.rs` holds the
            // end-to-end direction that matters here, with this dispatcher
            // stood in for by the fixture's own exit-status propagation: a
            // drive over a round whose own step fails stops at that round.
            // The check below is not itself exercised by a test, and the
            // recipe's step fatality is not enforced anywhere: #2775
            // tracks both.
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
            // #2333: GitHub's PR head is read once, over REST, and reported
            // with the result. It is evidence for the agents that follow, not
            // a condition of completion: the recording step reads it, the
            // merge-ready assessment and its criterion 3 judge whether the
            // review covers GitHub's head, and the merge itself is made with
            // `--match-head-commit`. See `evidence::RemoteHead`.
            let head = record["head_sha"].as_str().unwrap_or_default();
            let pr_head = evidence::remote_head(&state.repo, head);
            if let Some(fault) = pr_head.fault(head) {
                eprintln!("completion granted; PR head read for the next agent: {fault}");
            }
            (
                serde_json::json!({"loop":"crusty","loop_result":"DONE","round_label":state.round_label,
                "round_verdict":record["crusty_verdict"],"source":"objective-completion","evaluator_invoked":false,
                "pr_head":pr_head.evidence(head)}),
                0,
            )
        }
        Action::Evaluate => {
            // #2814: asking for convergence evaluation is never refused here.
            // This arm used to carry `ensure!(!complete)`, so a round the
            // next-action agent judged unfinished while this predicate called
            // it complete had no valid action at all, and a dispatch refusal
            // ends the whole drive with INVALID_DECISION. An unnecessary
            // evaluator call costs one invocation; a stopped drive costs the
            // run. The check that matters runs the other way round, in the
            // `Complete` arm above: a completion claim the evidence cannot
            // carry is still refused.
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
