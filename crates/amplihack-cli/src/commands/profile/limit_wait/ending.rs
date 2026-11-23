//! What a step says on its way out when it has met a usage limit but ends
//! because its prompt was lost (review of PR #2992, issue #3036).
//!
//! One ending reaches here, and it is a prompt-loss failure, not a limit
//! ending: the step's prompt came on a pipe and could not be read to its
//! end (the source failed, or the copier panicked part way through it). The
//! attempt in hand was given less than its task, so the step fails whether
//! or not it meets a limit (`auto_switch::rerun::finish` refuses to report
//! such an attempt's success), and no retry can be given more than it was.
//! The limit is not why the step ends; it is only why a retry would have
//! been needed. Every other way a copy used to be lost is gone: the copy is
//! kept in memory, so every byte the copier reads is a byte a retry is
//! given, and a step whose copy cannot be set up is not run at all
//! (`limit_wait::scratch`). There is therefore no step left that ends on a
//! limit for want of a copy, which is the case a capacity wait would have
//! helped. The step timeout no longer ends a limit wait either (issue
//! #3037): see `plan`.
//!
//! # How the run learns of the limit
//!
//! As data, not from these words. Every attempt that meets a limit is
//! reported to the recipe runner over its recovery channel as
//! `Outcome::UsageLimit` for this step's scope (issue #2334), by
//! `auto_switch::rerun::run`, before anything about a wait or this ending
//! is decided. The runner records that as a blockage of the step and, when
//! the step fails, writes it into the run's result as `failure_cause`
//! (`{"class":"usage_limit","origin":{...}}`). The run's classification
//! reads that field (`recipe::run::failure_class::classify_run_failure`),
//! and the run's recovery record is then `usage_limit`, so a relaunch
//! resumes at this step and the runner, which holds the whole prompt, sends
//! it again. The agentic layer that decides whether and when to relaunch
//! (`loop-health-evaluator`, auto-drive) reads the classification as JSON.
//! `tests_ending_evidence` pins that the verdict is the same with this
//! module's sentence removed from the evidence, and the integration test
//! `issue_2911_prompt_loss_resumes` runs the whole path on the pinned recipe
//! runner: the step ends `failed reason=prompt_cut_short` with no limit text
//! reaching the runner, the run's result carries the `usage_limit`
//! `failure_cause`, and the same command launched again resumes at this
//! step with the steps before it restored. No amplihack code relaunches a
//! run by itself; the layer that started it does, and needs nothing but the
//! same command.
//!
//! What the step writes here is for the person reading the log, in the
//! order they need it:
//!
//! 1. *that the prompt was lost, and that the limit is not why the step
//!    ends*, because `failed reason=prompt_cut_short` names the loss but
//!    not the limit.
//! 2. *that the limit still stands*, so a relaunch made before it lifts is
//!    expected to wait, as any step with its whole prompt does.
//! 3. *when the limit lifts* ([`resumes_at`]), the one thing the structured
//!    cause does not carry.
//!
//! Every sentence is built from static text and instants formatted from
//! integers, as every log line of this module is: no word of the harness's
//! output reaches any of them. The builders are pure so that
//! `tests_ending_evidence` can classify exactly what a step would write.

use super::events;

/// When the limit lifts, on its own line, for the layer that will have to
/// run this step again (review of PR #2992).
///
/// The run is classed `usage_limit` from the structured cause the
/// supervisor reported (see the module docs), not from this line. The
/// verdict's reasoning then quotes a reset if the evidence states one, read
/// with
/// [`limit_patterns::reset_hint`](crate::commands::recipe::run::limit_patterns::reset_hint),
/// which takes everything after the first `resets ` on a line. So this line
/// carries the instant and nothing else, and a person reading the verdict
/// is told when a relaunch will find room, not only that one is due.
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

/// Says that this step ends because its prompt was lost, not because of the
/// usage limit it also met, what has to happen instead, and when it is worth
/// doing (review of PR #2992).
///
/// `cause` is static text naming how the prompt was lost, never built from
/// what arrived on the pipe.
///
/// The step cannot send itself the prompt again; the runner that holds it
/// can, and learns that it must from the structured cause, not from this
/// sentence. A relaunch sent before the limit lifts is not wasted: the step
/// then has its whole prompt and waits for capacity like any other. This
/// says when the limit lifts ([`resumes_at`]) so a person can see when that
/// wait will end.
///
/// Pure, so the lines it produces can be read by the test that pins what
/// the recipe runner makes of them (`tests_ending_evidence`).
pub(super) fn prompt_lost_note(cause: &'static str, reset: Option<i64>, now: i64) -> String {
    format!(
        "amplihack: this step failed because its prompt was lost, not because of its usage \
         limit: {cause}. It fails whether or not it meets a limit, because its attempt was \
         not given the whole task, and no wait can earn it a retry, because a retry would \
         have no whole prompt to read. The limit is reported to the recipe runner as this \
         step's recorded cause, so a relaunch resumes here with the whole prompt, and waits \
         for capacity if the limit still stands. See \
         docs/reference/usage-limit-waits.md{}",
        resumes_at(reset, now)
    )
}

pub(super) fn the_prompt_was_lost(cause: &'static str, reset: Option<i64>, now: i64) {
    eprintln!("{}", prompt_lost_note(cause, reset, now));
}
