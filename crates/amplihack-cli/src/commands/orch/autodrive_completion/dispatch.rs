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
    let record = evidence::validate(&state, &snapshot)?;
    let complete = matches!(
        record["crusty_verdict"].as_str(),
        Some("CLEAN" | "MINOR_ONLY")
    );
    let (result, code) = match decision.action {
        Action::Complete => {
            // #2769: `--child-exit-code` is `ROUND_RC` in `autodrive_loop.sh`,
            // the crusty round's own exit status. While the #2663 outcome rule
            // judged every `autodrive-*` sub-recipe by whether it merged, that
            // status was non-zero whatever the round did and this guard was
            // constant-false, so the arm never fired. Narrowing the rule to the
            // two entry points makes the guard load-bearing: it now carries the
            // round's real verdict, which holds only while every step in
            // `autodrive-crusty-round.yaml` is fatal. A step there marked
            // `continue_on_error` would let a failed round exit 0 and complete
            // the objective; `round_failure_is_fatal_tests` below pins that.
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

/// #2769: the `Action::Complete` guard reads the crusty round's own exit
/// status, so that status must be the round's verdict. Nothing in Rust can
/// enforce the recipe side of that, so pin it here: a step marked
/// `continue_on_error` would let a failed round exit 0 and complete the
/// objective.
#[cfg(test)]
mod round_failure_is_fatal_tests {
    #[test]
    fn no_crusty_round_step_tolerates_its_own_failure() {
        let recipe = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../amplifier-bundle/recipes/autodrive-crusty-round.yaml");
        let body =
            std::fs::read_to_string(&recipe).expect("the bundle ships autodrive-crusty-round.yaml");
        let tolerant: Vec<(usize, &str)> = body
            .lines()
            .enumerate()
            .filter(|(_, line)| line.trim_start().starts_with("continue_on_error"))
            .map(|(number, line)| (number + 1, line.trim()))
            .collect();
        assert!(
            tolerant.is_empty(),
            "every step in autodrive-crusty-round.yaml must be fatal so the round's \
             exit status is its verdict (#2769); found {tolerant:?}"
        );
    }
}
