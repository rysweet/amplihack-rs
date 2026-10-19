//! The literal marker tables `failure_class` classifies with (issue #1267).
//!
//! Held apart from the classification logic so each file stays inside the
//! brick budget (#1437). Every entry is lowercase: the classifier lowercases
//! the evidence before matching. Precedence between the tables is decided in
//! `failure_class::classify_failure_text`, not by their order here.

/// The literal line a structural guard prints when it refuses to spawn
/// (`terminal_block` in `execute.rs`, issues #1326/#1327/#1329/#1332).
///
/// Matched as the whole phrase, never as the bare word `BLOCKED_TERMINAL`.
/// Issue #1437: the bare word appears in a run transcript whenever a
/// *descendant* was refused and the agent then did exactly what the refusal
/// told it to do — "complete this step inline and return your result" — so
/// matching the word alone reported an already-handled refusal as this run's
/// terminating cause.
pub(super) const STRUCTURAL_REFUSAL_MARKER: &str = "blocked_terminal orchestration_unavailable";

/// Markers that only a transport-level fault produces. Every entry is a status
/// line or a socket-level error string — none of them is something a compiler,
/// a test runner, or an agent's prose would emit as its own terminal message.
///
/// Note what is deliberately absent: 401/403/404. An auth rejection or an
/// unknown-model answer is a *stable* server answer, not an overloaded one, and
/// retrying it just burns budget. Those are classified `Environmental` below.
pub(super) const TRANSIENT_MARKERS: &[&str] = &[
    // Anthropic / OpenAI style overload and rate-limit envelopes.
    "api error: 529",
    "529 overloaded",
    "overloaded_error",
    "api error: 503",
    "503 service unavailable",
    "api error: 502",
    "502 bad gateway",
    "api error: 500",
    "500 internal server error",
    "504 gateway timeout",
    "api error: 429",
    "429 too many requests",
    "rate_limit_error",
    "rate limit exceeded",
    "server_error",
    "service temporarily unavailable",
    // Transport / socket level.
    "connection reset by peer",
    "connection closed before message completed",
    "econnreset",
    "etimedout",
    "epipe",
    "socket hang up",
    "tls handshake timed out",
    "temporary failure in name resolution",
    "upstream connect error",
    "request timed out",
];

/// Markers that mean the environment cannot support the work as configured.
/// Retrying the identical invocation cannot help — something has to change
/// first — so these are terminal for the mechanical retry.
pub(super) const ENVIRONMENTAL_MARKERS: &[&str] = &[
    "api error: 401",
    "401 unauthorized",
    "authentication_error",
    "invalid api key",
    "invalid x-api-key",
    "api error: 403",
    "403 forbidden",
    "permission_error",
    // Issue #1421: a model the API does not know. This is the most misleading
    // failure the runner sees — it looks like an API blip, so it used to fall
    // through to `Indeterminate` ("nothing in the evidence identifies the
    // failure") when in fact the evidence names the exact problem. It is
    // environmental by definition: retrying the identical call with the same
    // model can never succeed, something has to change first.
    //
    // Issue #1437: `model_not_found` is the same condition under another
    // provider's error type, so it is listed here too rather than as an
    // `agent_api` rejection; and these 404s win over `AGENT_API_MARKERS`.
    "api error: 404",
    "404 not found",
    "not_found_error",
    "model_not_found",
    "credit balance is too low",
    "command not found",
    "no such file or directory",
    "permission denied",
    "no space left on device",
    "disk quota exceeded",
    "gh auth login",
    "not logged in",
];

/// Markers for a repository-selection precondition failure (issue #1435).
///
/// `workflow-worktree` refuses a repository with no `origin` remote before
/// anything tries to fetch from it (issue #1323, merged as #1374) — a workflow
/// that will eventually push cannot proceed without one. That refusal is
/// correct, and its message names both the cause and the remedy:
///
/// ```text
/// ERROR: '/home/me/src' is a git repository but has no 'origin' remote, so this
///        workflow cannot resolve a base ref or push its work.
///        This looks like a multi-repository workspace. Repositories found:
///          /home/me/src/amplihack-rs
///        Point repo_path at the one to work in (issue #1323).
/// ```
///
/// The classifier used to answer `indeterminate` — "no transport, environmental,
/// or work marker found" — about that. It is `Environmental` by this taxonomy's
/// own definition: the environment cannot support the work as configured, and
/// retrying the identical call cannot help because the identical `repo_path`
/// will be refused identically. A human has to pick a repository or add an
/// origin.
///
/// These are held apart from `ENVIRONMENTAL_MARKERS` for one reason: the
/// verdict quotes the remedy back (see `FailureVerdict::reasoning`), and that
/// only makes sense when the deciding signal came from this table.
///
/// Every entry is a literal from the precondition's own output. Note what is
/// deliberately absent: bare `repository`, bare `origin`, and git's own
/// `fatal: not a git repository` / `does not appear to be a git repository`.
/// Those are everyday git noise; a marker broad enough to catch them would
/// reclassify ordinary failures, which is worse than the bug being fixed.
pub(super) const REPO_SELECTION_MARKERS: &[&str] = &[
    "has no 'origin' remote",
    "point repo_path at",
    "add an 'origin' remote",
];

/// Markers that mean the work itself failed. These win over everything: a run
/// whose evidence says a test failed is not retried because a 529 also appears
/// somewhere in the same tail.
pub(super) const WORK_MARKERS: &[&str] = &[
    "test result: failed",
    "test failed",
    "tests failed",
    "assertion failed",
    "assertionerror",
    "panicked at",
    "error[e",
    "could not compile",
    "compilation failed",
    "compilation error",
    "traceback (most recent call last)",
    "syntaxerror",
    "merge conflict",
    "hook failed",
    // Terminal policy refusals from the recursion / width guards (#1326,
    // #1327, #1332). These are FINAL answers by construction; retrying into a
    // guard is precisely the behaviour those guards exist to stop. Matched as
    // the guard's whole literal line, not as the bare words — see
    // `STRUCTURAL_REFUSAL_MARKER` (#1437).
    STRUCTURAL_REFUSAL_MARKER,
];

/// Markers for an account usage / session limit (issue #1390).
///
/// These are deliberately NOT in `TRANSIENT_MARKERS`. A 429 or an overload
/// envelope clears in seconds and is worth retrying inside the budget. A usage
/// limit clears when the stated reset time arrives, which is typically hours
/// away -- retrying it inside a minutes-long budget cannot succeed.
///
/// Note what is absent: a bare "rate limit". That phrase belongs to the
/// short-lived envelopes above and must keep its existing meaning.
///
/// Issue #1546: "weekly limit" and "spend limit" are listed explicitly,
/// because "monthly limit" does not match "monthly spend limit". "daily
/// limit" is deliberately absent too: provider and GitHub API daily request
/// quotas use that phrase and clear on their own, so listing it would stop
/// retrying them. List order does not pick the reported signal --
/// `first_match` reports the marker that appears earliest in the text.
pub(super) const USAGE_LIMIT_MARKERS: &[&str] = &[
    "session limit",
    "usage limit",
    "usage_limit_reached",
    "quota exceeded",
    "monthly limit",
    "weekly limit",
    "spend limit",
    "credit balance is too low",
];

/// Markers for an agent transport / model-endpoint rejection (issue #1437).
///
/// These name a request that never reached a verdict because the endpoint
/// refused it outright — a 400 envelope, an agent-side 404, a session that no
/// longer exists. They are deliberately NOT in `TRANSIENT_MARKERS` (the
/// identical request gets the identical rejection, so retrying inside the
/// mechanical budget cannot succeed) and NOT in `WORK_MARKERS` (nothing about
/// the work failed, and nothing declined to do it).
///
/// Note what is absent:
///
/// - `api error: 404`, `404 not found`, `not_found_error`, `model_not_found`:
///   an unknown model is `Environmental` (#1421), and those win over this
///   table.
/// - the generic `amplihack copilot failed (exit 1)` / `amplihack claude failed
///   (exit 1)` wrapper. That string is present on EVERY failed agent step,
///   including one whose real cause is a failing test, so matching it would
///   relabel most work failures. `Indeterminate` stays meaningful.
pub(super) const AGENT_API_MARKERS: &[&str] = &[
    "execution failed: 400",
    "execution failed: 404",
    "the resource you requested was not found",
    "api error: 400",
    "400 bad request",
    "invalid_request_error",
    "session not found",
    "conversation not found",
];
