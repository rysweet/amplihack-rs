//! Issue #1265, Option 3 — deliver amplihack's routing contract through
//! `--append-system-prompt`.
//!
//! # Why this channel
//!
//! amplihack delivers its routing instructions through a `UserPromptSubmit`
//! hook and `CLAUDE.md`. Both are *content* the agent reads; the base system
//! prompt is *the frame it reads them in*. When the base prompt carries a
//! directly contrary line — "Do not call the AgentTool unless the user
//! requested it", "Do not use workflows or deep-research unless the user
//! requested it" — the system prompt wins, the router is silently ignored, and
//! amplihack's central promise stops holding with no error and no warning.
//!
//! That is a delivery-channel problem, not a wording problem. No amount of
//! rewording a structurally outranked channel fixes it. `--append-system-prompt`
//! puts amplihack's contract at the same privilege level as the instruction it
//! has to overcome.
//!
//! # Where the bytes come from: the binary, not the filesystem
//!
//! The fragment is [`include_str!`]d from `amplifier-bundle/context/` at compile
//! time. It is 20 lines of static text that never varies per install, per user
//! or per repo, so there was never anything for a runtime read to discover —
//! only things for it to get wrong.
//!
//! Reading it from disk was tried first, and the read path is what made it
//! dangerous. The file has to be *staged* to `$HOME` before it can be read, so
//! it was added to `install::essential_files(Bundle)` — the list
//! `missing_framework_paths` uses to decide whether to restage. That single
//! listing armed a chain nobody wanted:
//!
//! 1. Every Bundle install in the wild lacks the file, so the gap is reported
//!    on the **first launch after upgrade**, for every user.
//! 2. `ensure_framework_installed` resolves a source with
//!    `find_bundled_framework_root`, whose second step **walks up from
//!    `current_dir()`** and accepts any ancestor carrying an
//!    `amplifier-bundle/` that passes a *shape* check — not a provenance or
//!    integrity check.
//! 3. The restage then copies `context/`, `agents/`, `skills/` and
//!    `tools/amplihack/*.sh` out of that root into `$HOME/.amplihack/.claude/`.
//! 4. This module reads the result and hands it to the agent at system-prompt
//!    privilege — where it inherits the fragment's own framing ("supersedes any
//!    earlier instruction", naming the guardrails it overrides) for free.
//!
//! So `git clone <fork> && cd <fork> && amplihack claude` wrote fork-authored
//! bytes into `$HOME` and injected them, permanently, into every later session
//! in every other repo. It is fair to say the cwd-sourced restage predates this
//! feature and already carried agent instructions and shell scripts. That is
//! true and it is the wrong half: before this listing existed the restage did
//! not *fire* on a healthy install. Supplying the trigger is the defect, and a
//! dormant channel and an armed one are not the same finding.
//!
//! Compiling the fragment in deletes the whole chain rather than guarding it —
//! there is no listing, so no trigger; no path, so no traversal question; no
//! read, so no TOCTOU, no size cap, no FIFO, no UTF-8 check, and no
//! integrity-at-read-time gap (A-8) to file as follow-up work. The bytes are
//! fixed at build time by the same review that ships the binary.
//!
//! # Security
//!
//! * The fragment's bytes appear in the process table and are visible to every
//!   user on the host. It carries operating instructions, never secrets, and
//!   the shipped file says so in its own header.
//! * `AMPLIHACK_NO_SYSTEM_PROMPT_APPEND` is UX, not a security boundary —
//!   anyone who can set it can already exec.

/// The routing contract itself, fixed at compile time.
///
/// `amplifier-bundle/context/` is the one copy: the bundle still ships the file
/// so `amplihack install` stages it where a human can read it, and so there is
/// exactly one place to edit. Nothing at runtime reads that copy — see the
/// module docs for why it used to, and what that cost.
pub(crate) const FRAGMENT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../amplifier-bundle/context/SYSTEM_PROMPT_APPEND.md"
));

/// Environment opt-out. Triggers on the exact value `"1"`, following the
/// `AMPLIHACK_COPILOT_NO_ALLOW_ALL` precedent.
pub(crate) const OPT_OUT_ENV: &str = "AMPLIHACK_NO_SYSTEM_PROMPT_APPEND";

/// The flags a user can supply to take over the append channel.
///
/// Two forms, each also accepted with a trailing `=value` — see
/// [`user_supplied_append_flag`], which is where the `=` handling lives.
pub(crate) const USER_FLAG_FORMS: &[&str] =
    &["--append-system-prompt", "--append-system-prompt-file"];

/// Map a launcher binary name onto its `AgentBinary` variant.
///
/// `AgentBinary` has exactly four variants, so `rusty` and `rustyclawd` need
/// this mapping to be answerable by the flag matrix at all — `run_rustyclawd`
/// delegates to `run_launch("claude", "claude", ...)`, so they are
/// claude-compatible front ends.
///
/// An unrecognised name returns `None` and injects nothing. `None` is the safe
/// default: an unknown binary must never receive a flag it may not accept.
pub(crate) fn agent_binary_for_name(
    binary_name: &str,
) -> Option<amplihack_launcher::flag_matrix::AgentBinary> {
    use amplihack_launcher::flag_matrix::AgentBinary;
    match binary_name {
        "claude" | "rusty" | "rustyclawd" => Some(AgentBinary::Claude),
        "copilot" => Some(AgentBinary::Copilot),
        "codex" => Some(AgentBinary::Codex),
        "amplifier" => Some(AgentBinary::Amplifier),
        _ => None,
    }
}

/// Should amplihack inject `--append-system-prompt` into this launch?
///
/// Pure — no I/O, and **no `std::env` reads inside**. This is a deliberate
/// divergence from its neighbours [`super::command::should_inject_copilot_allow_all`]
/// and `should_inject_copilot_remote`, which read the environment internally.
/// The read is hoisted to the call site here so the function is directly
/// testable without mutating process environment, which is `unsafe` under
/// edition 2024. Do not move it back inside.
///
/// True iff **all** of:
///
/// 1. [`agent_binary_for_name`] maps the name to a binary whose
///    `flags_for(..).supports_append_prompt` is true. The flag matrix is the
///    single source of truth — see the Amplifier note in
///    `docs/SYSTEM_PROMPT_APPEND.md`.
/// 2. `opt_out` is not `Some("1")`.
/// 3. The user supplied none of the four `--append-system-prompt*` forms.
///
/// There is no "is the fragment present?" condition: the fragment is
/// `include_str!`d into the binary, so it is present in every build. This gate
/// used to take a `fragment_present: bool` so the call site could decide
/// without paying for a file read — there is no file read now, and the
/// parameter was only ever passed `true`.
pub(crate) fn should_inject_system_prompt_append(
    binary_name: &str,
    extra_args: &[String],
    opt_out: Option<&str>,
) -> bool {
    if opt_out == Some("1") {
        return false;
    }
    let Some(binary) = agent_binary_for_name(binary_name) else {
        return false;
    };
    // The flag matrix is the source of truth, deliberately. `build_command_for_dir`'s
    // local `is_claude_compatible` also matches "amplifier" and governs
    // `--dangerously-skip-permissions` and `--model`; the matrix says
    // `supports_append_prompt == false` for Amplifier. The two disagree, the
    // matrix wins here, and `is_claude_compatible` is left alone — retargeting
    // those other two flags is a separate question. Do not "harmonize" them.
    if !amplihack_launcher::flag_matrix::flags_for(binary).supports_append_prompt {
        return false;
    }
    !user_supplied_append_flag(extra_args)
}

/// Did the caller already pass the flag themselves, in any accepted spelling?
///
/// Exact match on each form, or that form followed by `=`. Matching on a bare
/// prefix would make `--append-system-prompt-extra` and `--no-append-system-prompt`
/// false positives and silently disable the feature.
fn user_supplied_append_flag(extra_args: &[String]) -> bool {
    extra_args.iter().any(|arg| {
        USER_FLAG_FORMS.iter().any(|form| {
            arg == form
                || arg
                    .strip_prefix(form)
                    .is_some_and(|rest| rest.starts_with('='))
        })
    })
}

/// The routing contract, ready to hand to `--append-system-prompt`.
///
/// `Option` is kept because the gate treats "no fragment" as a first-class
/// answer and the call site is written around it. The empty case is now a build
/// mistake rather than a runtime one — an emptied bundle file would otherwise
/// inject a lone `--append-system-prompt ""`, which is noise at system-prompt
/// privilege — so it is checked here and pinned by
/// `the_compiled_in_fragment_is_not_empty_and_is_argv_sized`.
///
/// Borrows [`FRAGMENT`] rather than copying it. The bytes are `include_str!`d,
/// so they are already in the binary's read-only data for the whole process
/// lifetime; the only consumer hands them straight to `Command::arg`, which
/// takes `AsRef<OsStr>` and copies into the argv block regardless. Returning
/// `String` allocated a second copy of ~1 KiB on every launch to own bytes that
/// are already static and are never mutated.
pub(crate) fn installed_fragment() -> Option<&'static str> {
    (!FRAGMENT.trim().is_empty()).then_some(FRAGMENT)
}
