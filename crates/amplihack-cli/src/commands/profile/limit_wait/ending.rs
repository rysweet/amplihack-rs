//! What a step says on its way out when a bound that is not the limit ends
//! it while the limit still stands (review of PR #2992).
//!
//! Two endings reach here, and #2911 removed neither: the step's own
//! timeout cutting a limit wait (issue #3037), and a prompt no retry can be
//! given (issue #3036). Neither is a limit being refused and neither is work
//! that failed, so what the step writes has one job - hand the run what it
//! needs to carry on without this step's agent.
//!
//! Three things, in the order a reader of the evidence needs them:
//!
//! 1. *which bound ended the step, and that it is not the limit*, because
//!    `failed reason=step_timeout` and `skipped reason=piped_prompt` do not
//!    say.
//! 2. *that this is a usage limit*, which the words of each sentence carry.
//!    The event lines hold no limit marker of their own, so without the
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

use super::STEP_TIMEOUT_ENV;
use super::events;

/// When the limit lifts, on its own line, for the layer that will have to
/// run this step again (review of PR #2992).
///
/// Both endings below hand their step back to the recipe runner, which
/// classes it `usage_limit` from the words those endings use and then looks
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

/// Says that the step's own timeout, and not the limit, ended the step
/// here, and when the limit lifts (review of PR #2992).
///
/// The wait's deadline is pulled back to `AMPLIHACK_STEP_TIMEOUT` less
/// [`super::plan::STEP_TIMEOUT_LEAD`] because the recipe runner kills the step at
/// that timeout: a wait that ran past it would be killed mid-sleep, with no
/// line saying why and no attempt result passed up. Ending just before it
/// is what lets the step report. So this is the step's own bound, applied
/// by the layer that owns it, and not a budget this module keeps for
/// limits - but it does mean a limit wait is cut short by it, which the
/// `failed reason=step_timeout` event alone does not say.
///
/// Two remedies, not one. The operator's is to raise the step timeout or
/// leave it unset. The run's is to start the step again once the limit has
/// lifted, and [`resumes_at`] is what tells it when; raising a number is
/// the only remedy a step that could not say that leaves anyone
/// (`docs/reference/usage-limit-waits.md`, issue #3037).
///
/// Pure, so the lines it produces can be read by the test that pins what
/// the recipe runner makes of them (`tests_ending_evidence`).
pub(super) fn step_timeout_note(reset: Option<i64>, now: i64) -> String {
    format!(
        "amplihack: this step's own timeout (${STEP_TIMEOUT_ENV}) ended a usage-limit \
         wait before the limit lifted, so the step ends here with the limit's result. \
         The timeout is the step's bound and not a budget for limits: raise it, or \
         leave it unset, to let a limit wait run longer, or start this step again once \
         the limit has lifted. See docs/reference/usage-limit-waits.md{}",
        resumes_at(reset, now)
    )
}

pub(super) fn the_step_timeout_cut_the_wait(reset: Option<i64>, now: i64) {
    eprintln!("{}", step_timeout_note(reset, now));
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
/// Pure, for the same reason [`step_timeout_note`] is.
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
