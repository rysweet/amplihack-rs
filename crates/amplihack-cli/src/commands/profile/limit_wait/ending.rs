//! The sentence a step writes when it has met a usage limit but ends
//! because its prompt was lost (issue #3036).
//!
//! The run learns of the limit as data, not from this sentence: the
//! supervisor reports every limit to the recipe runner as
//! `Outcome::UsageLimit` (issue #2334), the runner records it as the run's
//! `failure_cause`, and a relaunch resumes at the step with its whole
//! prompt (`issue_2911_prompt_loss_resumes`). The sentence is for the person
//! reading the log: that the prompt was lost, that the limit is not why the
//! step ended, and when the limit lifts. It is built from static text and
//! an instant formatted from an integer, so no word of the harness's output
//! reaches it.

use super::events;

/// When the limit lifts, as far as the step knows: a reset still ahead, a
/// reset already behind the clock (which says nothing about when to start
/// again), or none.
fn resumes_at(reset: Option<i64>, now: i64) -> String {
    match reset {
        Some(r) if r > now => format!(
            "\namplihack: the limit resets {}",
            events::fmt_time(r, &chrono::Local)
        ),
        Some(r) => format!(
            "\namplihack: the reset this limit stated, {}, has already passed, so it does \
             not say when to start this step again.",
            events::fmt_time(r, &chrono::Local)
        ),
        None => "\namplihack: no reset is known for this limit.".to_string(),
    }
}

/// The sentence, for a prompt lost to `cause` (static text, never built
/// from what arrived on the pipe).
fn prompt_lost_note(cause: &'static str, reset: Option<i64>, now: i64) -> String {
    format!(
        "amplihack: this step failed because its prompt was lost, not because of its usage \
         limit: {cause}. It fails whether or not it meets a limit, because its attempt was \
         not given the whole task. The limit is reported to the recipe runner as this step's \
         recorded cause, so a relaunch resumes here with the whole prompt. See \
         docs/reference/usage-limit-waits.md{}",
        resumes_at(reset, now)
    )
}

pub(super) fn the_prompt_was_lost(cause: &'static str, reset: Option<i64>, now: i64) {
    eprintln!("{}", prompt_lost_note(cause, reset, now));
}
