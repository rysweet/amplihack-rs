//! What a step says on its way out when it has met a usage limit and its
//! input cannot be given to a retry (review of PR #2992, issue #3036).
//!
//! One ending reaches here: the step holds no whole copy of its prompt, so
//! a retry would have nothing to read and no wait could give it any. Two
//! kinds of failure lead to it. A prompt that could not be read to its end
//! left the attempt in hand with less than the task that was sent, so the
//! step fails whether or not it meets a limit (`auto_switch::rerun::finish`
//! refuses to report such an attempt's success), and no retry can be given
//! more than it was. A copy that failed, or that a process out of file
//! descriptors could not set up at all, left the attempt with the whole
//! task but kept no whole copy of it, so the limit ends the step. The step
//! timeout no longer ends a limit wait at all (issue #3037): see `plan`.
//!
//! What the step writes has one job - hand the run what it needs to carry
//! on without this step's agent. Three things, in the order a reader of the
//! evidence needs them:
//!
//! 1. *why no retry can run, and that it is not the limit*, because
//!    `skipped reason=piped_prompt` does not say.
//! 2. *that this is a usage limit*, which the words of the sentence carry.
//!    The event line holds no limit marker of its own, so without the
//!    sentence the recipe runner's classifier answers `indeterminate` -
//!    "nothing in the evidence identifies the failure" - and a supervising
//!    loop reads that as terminal.
//! 3. *when the limit lifts* ([`resumes_at`]), in the one form that
//!    classifier's reset reader takes a time from, so the verdict it writes
//!    names the time instead of saying only "wait for the reset".
//!
//! Every sentence is built from static text and instants formatted from
//! integers, as every log line of this module is: no word of the harness's
//! output reaches any of them. The builders are pure so that
//! `tests_ending_evidence` can classify exactly what a step would write.

use super::events;

/// When the limit lifts, on its own line, for the layer that will have to
/// run this step again (review of PR #2992).
///
/// The ending below hands its step back to the recipe runner, which
/// classes it `usage_limit` from the words that ending uses and then looks
/// for a reset in the same evidence with
/// [`limit_patterns::reset_hint`](crate::commands::recipe::run::limit_patterns::reset_hint).
/// That reader takes everything after the first `resets ` on a line, so the
/// line carries the instant and nothing else: a supervising loop then knows
/// not only that the step stopped on a limit but when it is worth starting
/// again, which is the difference between resuming the run and abandoning
/// it.
///
/// Three answers, because the step knows three different things and must
/// not give one when it holds another:
///
/// - a reset still ahead of `now`: the time, in the form that reader takes.
/// - a reset already behind `now`: that it has passed, and the time, but
///   *not* in that form. `Waiting::PassedReset` waits on a reading rather
///   than on a time that has been and gone (issue #2857), and a wait that
///   runs to its own reset ends past it too. A loop told "it resets" with a
///   past time would schedule a resume for a moment gone by; told the reset
///   has passed, it knows to read the account instead. The step does not
///   promise the limit has lifted - it never re-ran - so the sentence says
///   only what the reset no longer tells anyone.
/// - no reset at all: that none is known. A capacity or login-change wait
///   has none by definition, a message may state none that parses, and the
///   inherited login may never have run under this step at all, so the
///   sentence covers all three without claiming which.
///
/// The instant is formatted from an integer, as every other line of this
/// module is, so no word of the harness's output reaches it.
fn resumes_at(reset: Option<i64>, now: i64) -> String {
    match reset {
        Some(r) if r > now => format!(
            "\namplihack: the limit resets {}",
            events::fmt_time(r, &chrono::Local)
        ),
        Some(r) => format!(
            "\namplihack: the reset this limit stated, {}, has already passed, so it does \
             not say when to start this step again; a reading of the account does.",
            events::fmt_time(r, &chrono::Local)
        ),
        None => "\namplihack: no reset is known for this limit, so nothing here can say \
                 when it lifts; a reading of the account is what says so."
            .to_string(),
    }
}

/// Says that a limit has ended this step because no retry could be given
/// its prompt, what has to happen instead, and when it is worth doing
/// (review of PR #2992).
///
/// `cause` is static text chosen by the supervisor from the shape of the
/// step's stdin, never built from what arrived on it.
///
/// The step cannot send itself the prompt again; the layer that holds it
/// can. So this says the one thing that layer still needs after being told
/// to re-send: when the limit lifts ([`resumes_at`]). Without it a caller
/// told to send the prompt again has no basis for choosing a moment, and
/// sending it at once walks back into the same limit (issue #3036).
///
/// Pure, so the lines it produces can be read by the test that pins what
/// the recipe runner makes of them (`tests_ending_evidence`).
pub(super) fn no_retry_note(cause: &'static str, reset: Option<i64>, now: i64) -> String {
    format!(
        "amplihack: this step met a usage limit and cannot be run again: {cause}. The \
         limit is not lifted, and no wait can lift it for this step, because a retry \
         would have no prompt to read; the attempt's own result is this step's \
         result, and this step has to be sent again. See \
         docs/reference/usage-limit-waits.md{}",
        resumes_at(reset, now)
    )
}

pub(super) fn no_retry_is_possible(cause: &'static str, reset: Option<i64>, now: i64) {
    eprintln!("{}", no_retry_note(cause, reset, now));
}
