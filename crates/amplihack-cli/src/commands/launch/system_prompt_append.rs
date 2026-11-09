//! Issue #1265, Option 3 — deliver amplihack's routing contract at
//! system-prompt privilege, through `--append-system-prompt-file <path>`
//! (issue #2030).
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
//! rewording a structurally outranked channel fixes it. Appending to the system
//! prompt puts amplihack's contract at the same privilege level as the
//! instruction it has to overcome.
//!
//! # Why a file, not argv text (issue #2030)
//!
//! The contract used to be passed as `--append-system-prompt <text>`, so its
//! text sat in `/proc/<pid>/cmdline` of every session. One stray
//! `pkill -f 'tail -f /dev/null'` matched a phrase in it and ended 34 sessions
//! across the factory. Any pattern-based kill or `pgrep -f` that matches a
//! phrase in the contract hits every session. The contract is therefore
//! written to `<amplihack home>/private/system-prompt-append.md` (see
//! [`ensure_contract_file`]) and only that path is passed. The flag needs
//! Claude Code [`MIN_APPEND_PROMPT_FILE_VERSION`]; an older CLI fails the launch
//! loudly ([`require_append_prompt_file_capability`]) and never falls back to
//! argv.
//!
//! # Where the bytes come from: the binary, not the filesystem
//!
//! The fragment is [`include_str!`]d from `amplifier-bundle/context/` at compile
//! time. It is static text that never varies per install, per user or per repo,
//! so there is nothing for a runtime read to discover — only things for it to
//! get wrong.
//!
//! Reading it from disk was tried first, and the read path is what made it
//! dangerous. The file had to be *staged* to `$HOME` before it could be read,
//! so it was added to `install::essential_files(Bundle)` — the list
//! `missing_framework_paths` uses to decide whether to restage. That listing
//! armed a chain: a restage on the first launch after upgrade, a source found
//! by walking up from `current_dir()` with only a shape check (issue #1275
//! narrowed it), and the result injected at system-prompt privilege. So
//! `git clone <fork> && cd <fork> && amplihack claude` wrote fork-authored
//! bytes into `$HOME` and injected them into every later session.
//!
//! The contract file written for issue #2030 does not reopen that chain. It is
//! written *from* the compiled-in bytes at each launch that injects them, it is
//! never read back as a source of content (only a bounded byte comparison
//! decides whether to rewrite it), and it is not listed in
//! `install::essential_files` or the bundle staging tree.
//!
//! # Security
//!
//! * Threat model: other uids, and any process that scans argv. A same-uid
//!   adversary can already read and write everything this user owns and is
//!   out of scope.
//! * No contract text appears in any agent's argv; only the path of the
//!   contract file does. The path holds no secret.
//! * [`ensure_contract_file`] uses `lstat` and never follows a symlink. The
//!   `private/` directory must be a real directory owned by the current user
//!   with mode 0700 (repaired when it is ours). The file is replaced unless it
//!   is a regular 0600 file owned by us with exactly the compiled-in bytes.
//!   Replacement writes a `create_new` + `O_NOFOLLOW` 0600 temporary file with
//!   a random suffix, `fsync`s it and renames it over the target, so a planted
//!   symlink is replaced rather than written through, and concurrent launches
//!   never see a partial file.
//! * Residual TOCTOU: between the check and Claude opening the file, a process
//!   of the same uid could swap it. That process is already inside the threat
//!   model's excluded set.
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

/// Should amplihack inject the contract (`--append-system-prompt-file`) into
/// this launch?
///
/// Pure — no I/O, and **no `std::env` reads inside**. This is a deliberate
/// divergence from its neighbour [`super::command::should_inject_copilot_allow_all`],
/// which reads the environment internally.
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
/// The launch path also skips injection for a `rustyclawd` executable, whose
/// support for `--append-system-prompt-file` is unconfirmed; see
/// [`contract_injection_applies`].
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
    // The flag matrix is the source of truth, deliberately. `is_claude_compatible_tool`
    // in command.rs also matches "amplifier" and governs
    // `--dangerously-skip-permissions` (in `build_command_for_dir`) and `--model`
    // (in `model_args`); the matrix says `supports_append_prompt == false` for
    // Amplifier. The two disagree, the matrix wins here, and
    // `is_claude_compatible_tool` is left alone: retargeting those other two
    // flags is a separate question. Do not "harmonize" them.
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

/// The routing contract, ready to write to the contract file.
///
/// `Option` is kept because the empty case is a build mistake: an emptied
/// bundle file would otherwise inject an empty system-prompt append, which is
/// noise at system-prompt privilege. It is pinned by
/// `the_compiled_in_fragment_is_not_empty`.
pub(crate) fn installed_fragment() -> Option<&'static str> {
    (!FRAGMENT.trim().is_empty()).then_some(FRAGMENT)
}

// Issue #2030: the contract file and the version gate live in
// `contract_file`, which writes these compiled-in bytes to a 0600 file. They
// are re-exported here so callers keep one import path; this module itself
// still never reads anything from disk.
pub(crate) use super::contract_file::{
    CONTRACT_PATH_MISSING, contract_home, contract_injection_applies, ensure_contract_file,
    probe_claude_version, require_append_prompt_file_capability,
    require_prompt_file_flag_capability,
};
#[cfg(test)]
pub(crate) use super::contract_file::{
    CONTRACT_RELATIVE_PATH, MIN_APPEND_PROMPT_FILE_VERSION, ensure_contract_file_as,
    parse_claude_version,
};
