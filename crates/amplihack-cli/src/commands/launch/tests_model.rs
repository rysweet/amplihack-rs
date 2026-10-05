//! `--model` tests for issue #1527: the dotted-id rewrite of
//! AMPLIHACK_DEFAULT_MODEL, the warning for a dotted explicit `--model`, the
//! LiteLLM gateway's precedence, the stderr lines amplihack prints, and the
//! help text and reference pages that show them.
//!
//! They are kept apart from tests_command.rs, which covers the rest of
//! `build_command`. The environment helpers they share with the #1421 model
//! tests there are imported from it.

use super::command::{
    DEFAULT_MODEL, ModelArgs, ModelSelection, ModelSource, configured_default_model,
    explicit_model_warnings, is_claude_compatible_tool, model_args, model_selection_notice,
    normalize_dotted_claude_model_id,
};
use super::tests_command::{PROXY_ENV_VARS, model_arg_through_proxy, with_default_model_env};
use super::*;
use crate::binary_finder::BinaryInfo;
use crate::test_support::home_env_lock;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// Issue #1527: a dotted Claude model id in AMPLIHACK_DEFAULT_MODEL.
//
// Operators set the variable to a Claude id in the dotted spelling GitHub
// Copilot CLI uses (`claude-opus-5.5`). Claude model ids use hyphens
// (`claude-opus-5-5`); the issue records what Claude Code did with the dotted
// form. amplihack reads the variable only for claude-compatible launches, so it
// rewrites the one dot between major and minor version before passing it on,
// and says so on the stderr line added for #1421. An explicit `--model` is what
// the operator typed and is forwarded unchanged; when it is dotted, amplihack
// prints a warning naming the hyphenated spelling, because the launched tool
// may not report the problem itself.
// ---------------------------------------------------------------------------

/// The line the docs promise for a rewritten id
/// (docs/reference/environment-variables.md, AMPLIHACK_DEFAULT_MODEL).
const DOCUMENTED_REWRITE_NOTICE: &str = "amplihack: passing `--model claude-opus-5-5[1m]` to \
     `claude` (from AMPLIHACK_DEFAULT_MODEL, normalised from `claude-opus-5.5[1m]`: Claude \
     model ids use hyphens, not dots). Set AMPLIHACK_DEFAULT_MODEL to override it, or to an \
     empty value to let claude choose its own default model.";

/// The warning the docs promise for a dotted explicit `--model`
/// (docs/reference/environment-variables.md, "Explicit `--model`").
///
/// It states only what amplihack can vouch for, the spelling. What another
/// tool does with the dotted id changes with that tool's version, so it is not
/// in the line, the docs or this constant; issue #1527 has the report.
const DOCUMENTED_EXPLICIT_WARNING: &str = "amplihack: warning: passing `--model \
     claude-opus-5.5` to `claude` as typed, but Claude model ids use hyphens, not dots. \
     Use `--model claude-opus-5-5`.";

/// The line the docs promise on the LiteLLM gateway path
/// (docs/reference/environment-variables.md, AMPLIHACK_DEFAULT_MODEL).
const DOCUMENTED_GATEWAY_NOTICE: &str = "amplihack: passing `--model gateway-model` to \
     `claude` (from AMPLIHACK_LITELLM_MODEL). Set AMPLIHACK_LITELLM_MODEL to change it. \
     AMPLIHACK_DEFAULT_MODEL is not read while a LiteLLM gateway variable is set.";

fn selection(model: &str, source: ModelSource, normalised_from: Option<&str>) -> ModelSelection {
    ModelSelection {
        model: model.to_string(),
        source,
        normalised_from: normalised_from.map(str::to_string),
    }
}

/// A `BinaryInfo` for any tool name; `make_binary` is always `claude`.
fn make_named_binary(name: &str) -> BinaryInfo {
    BinaryInfo {
        name: name.to_string(),
        path: PathBuf::from(format!("/usr/bin/{name}")),
        version: Some("1.0.0".to_string()),
    }
}

/// The argv `build_command` produces for `binary_name` with
/// `AMPLIHACK_DEFAULT_MODEL` set to `env`, or unset for `None`.
///
/// Returns the argv instead of asserting inside the closure: the helper only
/// restores the environment when the closure returns, so an assertion that
/// fired in there would leave a dotted value set for the next test.
fn argv_with_default_model(env: Option<&str>, binary_name: &str, extra: &[&str]) -> Vec<String> {
    let extra: Vec<String> = extra.iter().map(|arg| arg.to_string()).collect();
    with_default_model_env(env, || {
        build_command(&make_named_binary(binary_name), false, false, false, &extra)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    })
}

/// The value after the first `--model`, if any.
fn model_after_flag(args: &[String]) -> Option<&str> {
    args.iter()
        .position(|arg| arg == "--model")
        .and_then(|at| args.get(at + 1))
        .map(String::as_str)
}

/// The rewrite's safety contract: the output is the input with its first `.`
/// (the one between major and minor) turned into `-`. Same length, one byte
/// changed, still `claude-` prefixed, so it cannot name some other model or
/// produce a token that reads as a flag.
fn assert_is_one_dot_to_hyphen_rewrite(input: &str, output: &str) {
    assert_eq!(
        output.len(),
        input.len(),
        "{input:?} -> {output:?} changed length"
    );
    let changed: Vec<(usize, u8, u8)> = input
        .bytes()
        .zip(output.bytes())
        .enumerate()
        .filter(|(_, (from, to))| from != to)
        .map(|(at, (from, to))| (at, from, to))
        .collect();
    assert_eq!(
        changed.len(),
        1,
        "{input:?} -> {output:?} must change exactly one byte, changed {changed:?}"
    );
    let (at, from, to) = changed[0];
    assert_eq!(
        (from, to),
        (b'.', b'-'),
        "{input:?} -> {output:?} must turn a `.` into a `-`"
    );
    assert_eq!(
        Some(at),
        input.find('.'),
        "{input:?} -> {output:?} must rewrite the dot between major and minor, \
         which is the first one"
    );
    assert!(
        output.starts_with("claude-"),
        "{input:?} -> {output:?} lost its `claude-` prefix"
    );
}

/// Issue #1527: each dotted spelling becomes the hyphenated id Claude Code
/// accepts. The docs table's rewritten rows are all here.
///
/// The version digits and the suffix are copied, never parsed.
/// `claude-opus-4.1-2025.08` keeps the dot in its suffix, because only the dot
/// between major and minor is rewritten and a suffix starting with `-` is
/// copied as it is. Leading zeros, and a digit run longer than any integer
/// type, survive unchanged; a scan that parsed the version into a number and
/// printed it back would get those wrong, or panic.
#[test]
fn test_normalize_dotted_claude_model_id_rewrites_dotted_ids() {
    let huge = "9".repeat(64);
    let huge_dotted = format!("claude-opus-{huge}.{huge}[1m]");
    let huge_hyphenated = format!("claude-opus-{huge}-{huge}[1m]");
    let cases = [
        ("claude-opus-5.5", "claude-opus-5-5"),
        ("claude-sonnet-4.5", "claude-sonnet-4-5"),
        ("claude-haiku-4.5", "claude-haiku-4-5"),
        ("claude-opus-5.5[1m]", "claude-opus-5-5[1m]"),
        ("claude-opus-4.1-20250805", "claude-opus-4-1-20250805"),
        ("claude-opus-4.1-2025.08", "claude-opus-4-1-2025.08"),
        ("claude-opus-05.05", "claude-opus-05-05"),
        (huge_dotted.as_str(), huge_hyphenated.as_str()),
    ];
    for (dotted, hyphenated) in cases {
        let got = normalize_dotted_claude_model_id(dotted);
        assert_eq!(got.as_deref(), Some(hyphenated), "rewriting {dotted:?}");
        assert_is_one_dot_to_hyphen_rewrite(dotted, hyphenated);
    }
}

/// Issue #1527: everything that is not exactly
/// `claude-<family>-<major>.<minor><suffix>` is left alone. No fuzzy matching,
/// no case-folding, no partial matches.
#[test]
fn test_normalize_dotted_claude_model_id_leaves_everything_else() {
    let unchanged = [
        // Already hyphenated, including amplihack's own default.
        "claude-opus-5-5",
        "claude-opus-5-5[1m]",
        "claude-opus-5[1m]",
        DEFAULT_MODEL,
        "claude-opus-4-1-20250805",
        // Not Claude ids: the dot is part of the real id.
        "gpt-5.1",
        "gemini-2.5-pro",
        // Aliases.
        "opus",
        "opus[1m]",
        "sonnet",
        // Legacy version-first ids are out of scope.
        "claude-3.5-sonnet",
        // A dot right after the minor version is not a valid suffix.
        "claude-opus-5.5.1",
        // Not lowercase.
        "Claude-Opus-5.5",
        "CLAUDE-OPUS-5.5",
        "claude-Opus-5.5",
        // A piece of the pattern is missing.
        "",
        "claude",
        "claude-",
        "claude-opus",
        "claude-opus-5",
        "claude-opus-5.",
        "claude-opus-.5",
        "claude--5.5",
        "claude-opus5.5",
        "claude-opus-4-5.1",
        // The suffix must be empty or start with `[` or `-`.
        "claude-opus-5.5x",
        "claude-opus-5.5]",
        "claude-opus-5.5 [1m]",
        // The pattern is anchored at the start. Callers trim before calling.
        "xclaude-opus-5.5",
        " claude-opus-5.5",
        "--model=claude-opus-5.5",
        // Non-ASCII look-alikes: only ASCII letters and digits count.
        "claude-op\u{fc}s-5.5",
        "claude-opus-\u{ff15}.\u{ff15}",
        "claude-opus-5\u{2024}5",
    ];
    for id in unchanged {
        assert_eq!(
            normalize_dotted_claude_model_id(id),
            None,
            "{id:?} must be passed through unchanged"
        );
    }
}

/// Issue #1527: the scanner is total. It never panics on any input, anything
/// it does rewrite obeys the one-byte contract, and its output is already in
/// final form, so rewriting twice is the same as rewriting once.
///
/// This is a sweep with no dependencies: every one-character insertion,
/// deletion and substitution of a few seed ids, over an alphabet that includes
/// the pattern's delimiters, multi-byte characters, whitespace and NUL.
#[test]
fn test_normalize_dotted_claude_model_id_is_total_and_idempotent() {
    let seeds = [
        "claude-opus-5.5",
        "claude-opus-5.5[1m]",
        "claude-opus-4.1-20250805",
        "claude-opus-5-5",
        "gpt-5.1",
    ];
    let alphabet = [
        '.', '-', '[', ']', 'a', 'Z', '0', '9', ' ', '\0', '\u{fc}', '\u{ff15}',
    ];
    let mut inputs: Vec<String> = Vec::new();
    for seed in seeds {
        let chars: Vec<char> = seed.chars().collect();
        inputs.push(seed.to_string());
        for at in 0..=chars.len() {
            for c in alphabet {
                let mut inserted = chars.clone();
                inserted.insert(at, c);
                inputs.push(inserted.into_iter().collect());
                if at < chars.len() {
                    let mut substituted = chars.clone();
                    substituted[at] = c;
                    inputs.push(substituted.into_iter().collect());
                }
            }
            if at < chars.len() {
                let mut deleted = chars.clone();
                deleted.remove(at);
                inputs.push(deleted.into_iter().collect());
            }
        }
    }

    let mut rewritten = 0;
    for input in &inputs {
        if let Some(output) = normalize_dotted_claude_model_id(input) {
            rewritten += 1;
            assert_is_one_dot_to_hyphen_rewrite(input, &output);
            assert_eq!(
                normalize_dotted_claude_model_id(&output),
                None,
                "{output:?} (rewritten from {input:?}) must not be rewritten again"
            );
        }
    }
    assert!(
        rewritten > seeds.len(),
        "the sweep must exercise the rewrite path, not only rejections; \
         {rewritten} of {} inputs were rewritten",
        inputs.len()
    );
}

/// Issue #1527: the selection records the rewrite, so the stderr line can name
/// both spellings. The original is stored trimmed, as the operator would
/// recognise it, not with the shell's surrounding whitespace.
#[test]
fn test_configured_default_model_records_the_rewrite() {
    let got = with_default_model_env(Some("  claude-opus-5.5[1m]  "), configured_default_model);
    assert_eq!(
        got,
        Some(selection(
            "claude-opus-5-5[1m]",
            ModelSource::DefaultModelEnv,
            Some("claude-opus-5.5[1m]"),
        ))
    );
}

/// Issue #1527: without a dotted Claude id, `configured_default_model` keeps
/// the #1421 behaviour. The source is decided where the variable is read, so
/// it cannot disagree with the value.
#[test]
fn test_configured_default_model_without_a_rewrite() {
    let unset = with_default_model_env(None, configured_default_model);
    let hyphenated = with_default_model_env(Some("claude-opus-5-5"), configured_default_model);
    let not_claude = with_default_model_env(Some(" gpt-5.1 "), configured_default_model);
    let empty = with_default_model_env(Some(""), configured_default_model);
    let blank = with_default_model_env(Some(" \t "), configured_default_model);

    assert_eq!(
        unset,
        Some(selection(DEFAULT_MODEL, ModelSource::BuiltInDefault, None)),
        "unset means amplihack's built-in default"
    );
    assert_eq!(
        hyphenated,
        Some(selection(
            "claude-opus-5-5",
            ModelSource::DefaultModelEnv,
            None
        )),
        "an already-hyphenated id is passed as set and is not reported as rewritten"
    );
    assert_eq!(
        not_claude,
        Some(selection("gpt-5.1", ModelSource::DefaultModelEnv, None)),
        "a dotted id that is not a Claude id is passed as set, trimmed"
    );
    assert_eq!(empty, None, "an empty value means no --model");
    assert_eq!(blank, None, "a whitespace-only value means no --model");
}

/// Issue #1527: a value that is not valid UTF-8 falls back to the built-in
/// default, labelled as such, even when its bytes look like a dotted Claude
/// id. It is never forwarded lossily or "repaired" into an id.
#[cfg(unix)]
#[test]
fn test_configured_default_model_non_utf8_falls_back_to_built_in_default() {
    use std::os::unix::ffi::OsStrExt;

    let got = with_default_model_env(None, || {
        // Set inside the helper so it restores the previous value afterwards.
        // Safety: the helper holds home_env_lock(), which serialises env access.
        unsafe {
            std::env::set_var(
                "AMPLIHACK_DEFAULT_MODEL",
                OsStr::from_bytes(b"claude-opus-5.5\xff"),
            )
        };
        configured_default_model()
    });
    assert_eq!(
        got,
        Some(selection(DEFAULT_MODEL, ModelSource::BuiltInDefault, None))
    );
}

/// Issue #1527, the reported bug: a dotted id reaches the command line as the
/// hyphenated id Claude Code accepts, trimmed and exactly once, for every
/// Claude-compatible tool, and the dotted spelling it rejects is nowhere in
/// argv.
#[test]
fn test_build_command_normalises_dotted_default_model() {
    let cases = [
        ("claude-opus-5.5[1m]", "claude-opus-5-5[1m]"),
        ("claude-opus-5.5", "claude-opus-5-5"),
        ("claude-sonnet-4.5", "claude-sonnet-4-5"),
        (" \tclaude-opus-5.5[1m]\n ", "claude-opus-5-5[1m]"),
    ];
    for (dotted, hyphenated) in cases {
        let args = argv_with_default_model(Some(dotted), "claude", &[]);
        assert_eq!(
            model_after_flag(&args),
            Some(hyphenated),
            "AMPLIHACK_DEFAULT_MODEL={dotted:?}, got: {args:?}"
        );
        assert_eq!(
            args.iter().filter(|arg| arg.starts_with("--model")).count(),
            1,
            "exactly one --model, got: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg.contains(dotted.trim())),
            "the dotted spelling Claude Code rejects must not reach argv, got: {args:?}"
        );
    }

    for tool in ["claude", "rusty", "rustyclawd", "amplifier"] {
        let args = argv_with_default_model(Some("claude-opus-5.5"), tool, &[]);
        assert_eq!(
            model_after_flag(&args),
            Some("claude-opus-5-5"),
            "`amplihack {tool}` is Claude-compatible and must get the rewrite, got: {args:?}"
        );
    }
}

/// Issue #1527: anything not of the documented dotted form reaches the tool
/// exactly as set, and only once. That covers dotted ids the rewrite must not
/// touch, and the hyphenated spellings the issue reports as working, `[1m]`
/// suffix and all: a rewrite that also touched hyphens or brackets would break
/// the ids that work today.
#[test]
fn test_build_command_passes_other_default_models_unchanged() {
    for id in [
        "gpt-5.1",
        "claude-3.5-sonnet",
        "claude-opus-5.5.1",
        "claude-opus-5-5",
        "claude-opus-5-5[1m]",
        "claude-opus-5[1m]",
        "claude-opus-4-1-20250805",
    ] {
        let args = argv_with_default_model(Some(id), "claude", &[]);
        assert_eq!(
            model_after_flag(&args),
            Some(id),
            "AMPLIHACK_DEFAULT_MODEL={id:?} must be passed unchanged, got: {args:?}"
        );
        assert_eq!(
            args.iter().filter(|arg| arg.starts_with("--model")).count(),
            1,
            "exactly one --model, got: {args:?}"
        );
    }
}

/// Issue #1527 (decision A2): only argv is rewritten. The child must inherit
/// AMPLIHACK_DEFAULT_MODEL exactly as the operator set it: the command sets no
/// override for it, removes nothing, and building the command leaves the
/// parent's value alone. Rewriting the environment would silently change the
/// operator's setting for every process below this one. Left alone, a nested
/// amplihack launch re-reads the dotted value and makes the same rewrite, with
/// the same stderr line naming the original spelling.
#[test]
fn test_build_command_leaves_the_childs_default_model_env_untouched() {
    let dotted = "claude-opus-5.5[1m]";
    let (overrides, parent_value_after_build) = with_default_model_env(Some(dotted), || {
        let cmd = build_command(&make_named_binary("claude"), false, false, false, &[]);
        let overrides: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .filter(|(key, _)| *key == OsStr::new("AMPLIHACK_DEFAULT_MODEL"))
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        (overrides, std::env::var("AMPLIHACK_DEFAULT_MODEL").ok())
    });
    assert!(
        overrides.is_empty(),
        "the child must inherit AMPLIHACK_DEFAULT_MODEL unmodified, but the command \
         overrides it: {overrides:?}"
    );
    assert_eq!(
        parent_value_after_build.as_deref(),
        Some(dotted),
        "building the command must not rewrite the parent's AMPLIHACK_DEFAULT_MODEL, \
         which every later child inherits"
    );
}

/// Issue #1527: an explicit `--model` is forwarded unchanged, in both forms,
/// even when it is dotted and AMPLIHACK_DEFAULT_MODEL is dotted too. The issue
/// asks that the operator's value be left alone. What amplihack adds for a
/// dotted one is a stderr warning, pinned by the
/// `test_explicit_model_warnings_*` and `test_model_args_*` tests, not a rewrite.
#[test]
fn test_build_command_explicit_dotted_model_is_not_normalised() {
    let env = Some("claude-opus-5.5[1m]");

    let spaced = argv_with_default_model(env, "claude", &["--model", "claude-opus-5.5"]);
    assert_eq!(
        spaced
            .iter()
            .filter(|arg| arg.starts_with("--model"))
            .count(),
        1,
        "the explicit --model must be the only one, got: {spaced:?}"
    );
    assert_eq!(
        model_after_flag(&spaced),
        Some("claude-opus-5.5"),
        "the explicit value must be forwarded byte for byte, got: {spaced:?}"
    );
    assert!(
        !spaced.iter().any(|arg| arg.contains("claude-opus-5-5")),
        "neither the explicit value nor the env value may be rewritten into argv, got: {spaced:?}"
    );

    let equals = argv_with_default_model(env, "claude", &["--model=claude-opus-5.5"]);
    let model_args: Vec<&str> = equals
        .iter()
        .filter(|arg| arg.starts_with("--model"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        model_args,
        vec!["--model=claude-opus-5.5"],
        "the explicit --model= form must be the only one and unchanged, got: {equals:?}"
    );
    assert!(
        !equals.iter().any(|arg| arg.contains("claude-opus-5-5")),
        "neither the explicit value nor the env value may be rewritten into argv, got: {equals:?}"
    );
}

/// Issue #1527: amplihack reads AMPLIHACK_DEFAULT_MODEL only for
/// claude-compatible launches. A dotted value must not start reaching Copilot
/// or Codex in either spelling.
#[test]
fn test_build_command_dotted_model_env_ignored_for_copilot_and_codex() {
    for tool in ["copilot", "codex"] {
        let args = argv_with_default_model(Some("claude-opus-5.5"), tool, &[]);
        assert!(
            !args.iter().any(|arg| arg.starts_with("--model")),
            "`amplihack {tool}` must not get a --model, got: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg.contains("claude-opus-5")),
            "`amplihack {tool}` must not get either spelling, got: {args:?}"
        );
    }
}

/// Issue #1527: the LiteLLM gateway routes on the model name, so its model is
/// an address and not a spelling to correct. A gateway that serves
/// `claude-opus-5.5` must be sent exactly that.
#[test]
fn test_proxy_model_is_never_normalised() {
    assert_eq!(
        model_arg_through_proxy(Some("claude-opus-5.5")).as_deref(),
        Some("claude-opus-5.5")
    );
}

/// Issue #1527: without a rewrite, the #1421 line is byte-for-byte what it was,
/// so nothing that reads stderr notices this change.
#[test]
fn test_model_selection_notice_is_unchanged_without_rewrite() {
    let documented = model_selection_notice(
        &selection("claude-sonnet-4-5", ModelSource::DefaultModelEnv, None),
        "claude",
    );
    assert_eq!(
        documented,
        "amplihack: passing `--model claude-sonnet-4-5` to `claude` (from \
         AMPLIHACK_DEFAULT_MODEL). Set AMPLIHACK_DEFAULT_MODEL to override it, or to an \
         empty value to let claude choose its own default model."
    );

    let built_in = model_selection_notice(
        &selection(DEFAULT_MODEL, ModelSource::BuiltInDefault, None),
        "rusty",
    );
    assert_eq!(
        built_in,
        format!(
            "amplihack: passing `--model {DEFAULT_MODEL}` to `rusty` (from amplihack's \
             built-in default). Set AMPLIHACK_DEFAULT_MODEL to override it, or to an empty \
             value to let rusty choose its own default model."
        )
    );
}

/// Issue #1527: on the LiteLLM gateway path the notice names
/// AMPLIHACK_LITELLM_MODEL as the variable to change. AMPLIHACK_DEFAULT_MODEL
/// is not read there, so advice to set it, or to empty it, would have no
/// effect.
#[test]
fn test_model_selection_notice_on_the_gateway_names_the_gateway_variable() {
    let got = model_selection_notice(
        &selection("gateway-model", ModelSource::LiteLlmGateway, None),
        "claude",
    );
    assert_eq!(got, DOCUMENTED_GATEWAY_NOTICE);
    assert!(
        !got.contains("Set AMPLIHACK_DEFAULT_MODEL"),
        "the gateway notice must not advise setting a variable that is not read: {got}"
    );
}

/// Issue #1527: a dotted Claude id given to `--model`, in either form, produces
/// one warning naming the hyphenated spelling. The value in the warning is the
/// one forwarded, and the suggestion keeps any suffix.
#[test]
fn test_explicit_model_warnings_name_the_hyphenated_spelling() {
    let args = |list: &[&str]| -> Vec<String> { list.iter().map(|a| a.to_string()).collect() };

    assert_eq!(
        explicit_model_warnings("claude", &args(&["--model", "claude-opus-5.5"]), false),
        vec![DOCUMENTED_EXPLICIT_WARNING.to_string()]
    );
    assert_eq!(
        explicit_model_warnings("claude", &args(&["--model=claude-opus-5.5"]), false),
        vec![DOCUMENTED_EXPLICIT_WARNING.to_string()],
        "the --model= form must give the same warning"
    );

    let suffixed = explicit_model_warnings(
        "rusty",
        &args(&["-p", "hello", "--model", "claude-opus-5.5[1m]"]),
        false,
    );
    assert_eq!(suffixed.len(), 1, "got: {suffixed:?}");
    assert!(
        suffixed[0].contains("passing `--model claude-opus-5.5[1m]` to `rusty` as typed")
            && suffixed[0].ends_with("Use `--model claude-opus-5-5[1m]`."),
        "got: {suffixed:?}"
    );
    assert!(
        !suffixed[0].contains("Claude Code"),
        "the warning must not name a tool other than the one launched: {suffixed:?}"
    );
}

/// Issue #1527: no warning for anything that is not a dotted Claude id, for a
/// `--model` with no value, or for arguments that only resemble `--model`.
#[test]
fn test_explicit_model_warnings_are_silent_otherwise() {
    let quiet: [&[&str]; 9] = [
        &[],
        &["--model", "claude-opus-5-5"],
        &["--model=claude-opus-5-5[1m]"],
        &["--model", "opus[1m]"],
        &["--model", "gpt-5.1"],
        &["--model", "claude-3.5-sonnet"],
        &["--model"],
        &["--model-config", "claude-opus-5.5"],
        &["-p", "claude-opus-5.5"],
    ];
    for list in quiet {
        let args: Vec<String> = list.iter().map(|a| a.to_string()).collect();
        assert_eq!(
            explicit_model_warnings("claude", &args, false),
            Vec::<String>::new(),
            "{args:?} must not produce a warning"
        );
    }
}

/// The tools amplihack treats as Claude Code, and so the only ones that get a
/// `--model` from it or the dotted `--model` warning.
const CLAUDE_COMPATIBLE_TOOLS: [&str; 4] = ["claude", "rusty", "rustyclawd", "amplifier"];

/// Every `AMPLIHACK_LITELLM_*` variable amplihack reads that is not one of the
/// three gateway variables in [`PROXY_ENV_VARS`]. `amplihack litellm`
/// verification reads these (commands/litellm/preflight.rs, and
/// `AMPLIHACK_LITELLM_TARGET` in amplihack-utils' litellm_proxy.rs). None of
/// them routes a launch through the gateway.
const NON_GATEWAY_LITELLM_VARS: [&str; 6] = [
    "AMPLIHACK_LITELLM_TELEMETRY_FILE",
    "AMPLIHACK_LITELLM_TELEMETRY_HMAC_KEY",
    "AMPLIHACK_LITELLM_EXPECTED_PROVIDER",
    "AMPLIHACK_LITELLM_EXPECTED_MODEL",
    "AMPLIHACK_LITELLM_EXPECTED_GATEWAY_IDENTITY",
    "AMPLIHACK_LITELLM_TARGET",
];

/// Puts back every variable [`model_args_with`] cleared, even when the call
/// under test panics, so a failure cannot leak a gateway variable into the
/// next test.
struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl Drop for RestoreEnv {
    fn drop(&mut self) {
        for (name, value) in self.0.drain(..) {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

/// [`model_args`] for `binary_name` and `extra` on a host where, of
/// `AMPLIHACK_DEFAULT_MODEL` and every `AMPLIHACK_LITELLM_*` variable amplihack
/// reads, exactly the ones in `env` are set.
fn model_args_with(env: &[(&str, &str)], binary_name: &str, extra: &[&str]) -> ModelArgs {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let names = PROXY_ENV_VARS
        .into_iter()
        .chain(NON_GATEWAY_LITELLM_VARS)
        .chain(["AMPLIHACK_DEFAULT_MODEL"]);
    let restore = RestoreEnv(names.map(|name| (name, std::env::var_os(name))).collect());
    for (name, _) in &restore.0 {
        unsafe { std::env::remove_var(name) };
    }
    for (name, value) in env {
        unsafe { std::env::set_var(name, value) };
    }
    let extra: Vec<String> = extra.iter().map(|arg| arg.to_string()).collect();
    let got = model_args(binary_name, &extra);
    drop(restore);
    got
}

/// The warning [`explicit_model_warnings`] gives `tool` for an explicit
/// `--model claude-opus-5.5`, built from the documented one.
fn explicit_warning_for(tool: &str) -> String {
    DOCUMENTED_EXPLICIT_WARNING.replace("to `claude`", &format!("to `{tool}`"))
}

/// Issue #1527 review: the warning's gate, as a pure function. Only a
/// Claude-compatible tool gets it, and not on the LiteLLM gateway path.
/// `amplihack copilot -- --model claude-opus-4.5` is Copilot's own spelling and
/// correct as typed; a dotted gateway route name may be correct too.
#[test]
fn test_explicit_model_warnings_only_for_claude_compatible_tools_off_the_gateway() {
    let dotted = ["--model".to_string(), "claude-opus-5.5".to_string()];
    for tool in CLAUDE_COMPATIBLE_TOOLS {
        assert!(is_claude_compatible_tool(tool), "{tool}");
        assert_eq!(
            explicit_model_warnings(tool, &dotted, false),
            vec![explicit_warning_for(tool)],
            "`amplihack {tool}` with a dotted --model must be warned"
        );
        assert_eq!(
            explicit_model_warnings(tool, &dotted, true),
            Vec::<String>::new(),
            "`amplihack {tool}` through the LiteLLM gateway must not be warned"
        );
    }
    for tool in ["copilot", "codex", "Claude", ""] {
        assert!(!is_claude_compatible_tool(tool), "{tool:?}");
        for through_gateway in [false, true] {
            assert_eq!(
                explicit_model_warnings(tool, &dotted, through_gateway),
                Vec::<String>::new(),
                "{tool:?} is not Claude-compatible and must never be warned \
                 (through_gateway = {through_gateway})"
            );
        }
    }
}

/// Issue #1527 review, wired to the environment: a dotted explicit `--model`
/// on a Claude-compatible tool, with no gateway variable set, gives exactly one
/// stderr line, the documented warning, and amplihack adds no `--model` of its
/// own. A dotted AMPLIHACK_DEFAULT_MODEL does not change that.
#[test]
fn test_model_args_warns_on_a_dotted_explicit_model() {
    for tool in CLAUDE_COMPATIBLE_TOOLS {
        for extra in [
            &["--model", "claude-opus-5.5"][..],
            &["--model=claude-opus-5.5"],
        ] {
            for env in [
                &[][..],
                &[("AMPLIHACK_DEFAULT_MODEL", "claude-opus-5.5[1m]")],
            ] {
                let got = model_args_with(env, tool, extra);
                assert_eq!(
                    got,
                    ModelArgs {
                        argv: Vec::new(),
                        stderr: vec![explicit_warning_for(tool)],
                    },
                    "`amplihack {tool}` with {extra:?} and {env:?}"
                );
                assert!(
                    !got.stderr[0].contains("Claude Code"),
                    "the warning must not name a tool other than `{tool}`: {:?}",
                    got.stderr
                );
            }
        }
    }
    assert_eq!(
        model_args_with(&[], "claude", &["--model", "claude-opus-5.5"]).stderr,
        vec![DOCUMENTED_EXPLICIT_WARNING.to_string()]
    );
}

/// Issue #1527 review: `amplihack copilot` and `amplihack codex` get neither a
/// `--model` nor any stderr line about one, whatever the operator passes and
/// whatever AMPLIHACK_DEFAULT_MODEL says. Telling a Copilot user to change
/// `claude-opus-4.5`, Copilot's own spelling, would be wrong.
#[test]
fn test_model_args_never_warns_copilot_or_codex() {
    for tool in ["copilot", "codex"] {
        for extra in [
            &["--model", "claude-opus-4.5"][..],
            &["--model=claude-opus-4.5"],
            &[],
        ] {
            for env in [&[][..], &[("AMPLIHACK_DEFAULT_MODEL", "claude-opus-5.5")]] {
                assert_eq!(
                    model_args_with(env, tool, extra),
                    ModelArgs::default(),
                    "`amplihack {tool}` with {extra:?} and {env:?}"
                );
            }
        }
    }
}

/// Issue #1527 review: on the LiteLLM gateway path, which any one of the three
/// gateway variables selects, a dotted explicit `--model` is a route name. It
/// is forwarded with no warning, and amplihack adds nothing.
#[test]
fn test_model_args_never_warns_on_the_gateway() {
    for gateway_var in PROXY_ENV_VARS {
        for tool in CLAUDE_COMPATIBLE_TOOLS {
            for extra in [
                &["--model", "claude-opus-5.5"][..],
                &["--model=claude-opus-5.5"],
            ] {
                assert_eq!(
                    model_args_with(&[(gateway_var, "claude-opus-5.5")], tool, extra),
                    ModelArgs::default(),
                    "`amplihack {tool}` with {extra:?} and only {gateway_var} set"
                );
            }
        }
    }
}

/// Issue #1527 review: only AMPLIHACK_LITELLM_ENDPOINT, _API_KEY and _MODEL
/// select the gateway, which is what the help text and docs say. Any one of
/// them alone does, and AMPLIHACK_DEFAULT_MODEL is then not read. Every other
/// `AMPLIHACK_LITELLM_*` variable leaves AMPLIHACK_DEFAULT_MODEL in force and
/// the dotted `--model` warning on.
#[test]
fn test_only_the_three_gateway_variables_select_the_gateway() {
    let pinned = [("AMPLIHACK_DEFAULT_MODEL", "claude-sonnet-4-5")];
    let pinned_notice = model_selection_notice(
        &selection("claude-sonnet-4-5", ModelSource::DefaultModelEnv, None),
        "claude",
    );
    for var in NON_GATEWAY_LITELLM_VARS {
        let env = [pinned[0], (var, "set")];
        assert_eq!(
            model_args_with(&env, "claude", &[]),
            ModelArgs {
                argv: vec!["--model".to_string(), "claude-sonnet-4-5".to_string()],
                stderr: vec![pinned_notice.clone()],
            },
            "{var} must not select the gateway: AMPLIHACK_DEFAULT_MODEL must still be read"
        );
        assert_eq!(
            model_args_with(&env, "claude", &["--model", "claude-opus-5.5"]).stderr,
            vec![DOCUMENTED_EXPLICIT_WARNING.to_string()],
            "{var} must not select the gateway: the dotted --model warning must stay on"
        );
    }
    for var in PROXY_ENV_VARS {
        let got = model_args_with(&[pinned[0], (var, "gateway-model")], "claude", &[]);
        let gateway_model = if var == amplihack_utils::litellm_proxy::MODEL_ENV {
            "gateway-model"
        } else {
            "amplihack-default"
        };
        assert_eq!(
            got,
            ModelArgs {
                argv: vec!["--model".to_string(), gateway_model.to_string()],
                stderr: vec![model_selection_notice(
                    &selection(gateway_model, ModelSource::LiteLlmGateway, None),
                    "claude",
                )],
            },
            "{var} alone must select the gateway, so AMPLIHACK_DEFAULT_MODEL is not read"
        );
    }
}

const STDERR_PROBE_BINARY_ENV: &str = "AMPLIHACK_TEST_1527_PROBE_BINARY";
const STDERR_PROBE_ARGS_ENV: &str = "AMPLIHACK_TEST_1527_PROBE_ARGS";

/// The child half of [`test_build_command_prints_exactly_the_model_args_stderr`],
/// not a test of its own. It is `#[ignore]`d, so `cargo test` and
/// `cargo nextest run` skip it and report it as ignored, not as a pass, and
/// nextest spawns no process for it. That test runs it in a child process with
/// `--ignored` and the probe variables set. The child builds the command for
/// the tool and arguments the variables name, so the parent can read what
/// really reached stderr. Without the variables there is nothing to build, so
/// it returns at once. That only happens when someone runs ignored tests by
/// hand (the same convention as `probe_spawn_claude` in amplihack-launcher's
/// root_sandbox_spawn.rs).
#[test]
#[ignore = "child half of test_build_command_prints_exactly_the_model_args_stderr"]
fn probe_build_command_stderr_for_issue_1527() {
    let Some(binary_name) = std::env::var_os(STDERR_PROBE_BINARY_ENV) else {
        return;
    };
    let extra: Vec<String> = std::env::var(STDERR_PROBE_ARGS_ENV)
        .unwrap_or_default()
        .split('\n')
        .filter(|arg| !arg.is_empty())
        .map(str::to_string)
        .collect();
    let binary = make_named_binary(&binary_name.to_string_lossy());
    build_command(&binary, false, false, false, &extra);
}

/// The `amplihack: ` lines `build_command` writes to the real stderr for
/// `binary_name` and `extra`, with exactly the variables in `env` set. Runs
/// the ignored [`probe_build_command_stderr_for_issue_1527`] with `--ignored`
/// in a child copy of this test binary, with a cleared environment, so neither
/// the developer's shell nor another test can change the answer. The
/// `1 passed` check fails if the probe is not run, for example because
/// `--ignored` was dropped.
fn real_build_command_stderr(
    env: &[(&str, &str)],
    binary_name: &str,
    extra: &[&str],
) -> Vec<String> {
    let module = module_path!()
        .split_once("::")
        .map_or(module_path!(), |(_, rest)| rest);
    let probe = format!("{module}::probe_build_command_stderr_for_issue_1527");
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            probe.as_str(),
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", "1")
        .env(STDERR_PROBE_BINARY_ENV, binary_name)
        .env(STDERR_PROBE_ARGS_ENV, extra.join("\n"))
        .envs(env.iter().copied());
    let output = child.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "the probe {probe} did not run exactly once and pass\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stderr
        .lines()
        .filter(|line| line.starts_with("amplihack: "))
        .map(str::to_string)
        .collect()
}

/// Issue #1527 review: what `build_command_for_dir` actually writes to stderr
/// for `--model` is what `model_args` returns, line for line, in a real
/// process. Without this, dropping the line that prints them, or printing a
/// warning `model_args` withheld, would pass every other test.
#[test]
fn test_build_command_prints_exactly_the_model_args_stderr() {
    assert_eq!(
        real_build_command_stderr(&[], "claude", &["--model", "claude-opus-5.5"]),
        vec![DOCUMENTED_EXPLICIT_WARNING.to_string()],
        "a dotted explicit --model on claude must print the documented warning, once"
    );
    assert_eq!(
        real_build_command_stderr(
            &[("AMPLIHACK_DEFAULT_MODEL", "claude-opus-5.5[1m]")],
            "claude",
            &[]
        ),
        vec![DOCUMENTED_REWRITE_NOTICE.to_string()],
        "a dotted AMPLIHACK_DEFAULT_MODEL must print the documented rewrite notice, once"
    );
    for tool in ["copilot", "codex"] {
        assert_eq!(
            real_build_command_stderr(
                &[("AMPLIHACK_DEFAULT_MODEL", "claude-opus-5.5")],
                tool,
                &["--model", "claude-opus-4.5"]
            ),
            Vec::<String>::new(),
            "`amplihack {tool}` must print nothing about --model"
        );
    }
    assert_eq!(
        real_build_command_stderr(
            &[
                (
                    amplihack_utils::litellm_proxy::ENDPOINT_ENV,
                    "https://gateway.example.com"
                ),
                (
                    amplihack_utils::litellm_proxy::API_KEY_ENV,
                    "gateway-secret"
                ),
                (amplihack_utils::litellm_proxy::MODEL_ENV, "claude-opus-5.5"),
            ],
            "claude",
            &["--model", "claude-opus-5.5"]
        ),
        Vec::<String>::new(),
        "a gateway launch with an explicit --model must print nothing about --model"
    );
}

const ENVIRONMENT_VARIABLES_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/reference/environment-variables.md"
));
const LAUNCH_FLAG_INJECTION_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/reference/launch-flag-injection.md"
));
const LAUNCHER_MODEL_CONFIGURATION_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/reference/LAUNCHER_MODEL_CONFIGURATION.md"
));

/// `amplihack claude --help`, as an operator sees it.
fn claude_long_help() -> String {
    use clap::CommandFactory;

    let mut cli = crate::Cli::command();
    let claude = cli
        .find_subcommand_mut("claude")
        .expect("`amplihack claude` should exist");
    let mut help = Vec::new();
    claude.write_long_help(&mut help).unwrap();
    String::from_utf8(help).unwrap()
}

/// The help text and every reference page that describes how amplihack picks
/// the model, by name.
fn model_help_and_pages() -> Vec<(&'static str, String)> {
    vec![
        ("`amplihack claude --help`", claude_long_help()),
        (
            "docs/reference/launch-flag-injection.md",
            LAUNCH_FLAG_INJECTION_MD.to_string(),
        ),
        (
            "docs/reference/environment-variables.md",
            ENVIRONMENT_VARIABLES_MD.to_string(),
        ),
        (
            "docs/reference/flag-matrix.md",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/reference/flag-matrix.md"
            ))
            .to_string(),
        ),
        (
            "docs/reference/LAUNCHER_MODEL_CONFIGURATION.md",
            LAUNCHER_MODEL_CONFIGURATION_MD.to_string(),
        ),
    ]
}

/// Issue #1527 review: the help text and reference pages an operator reads
/// about `--model` must name the default amplihack passes. Before this test
/// they still said amplihack passes no `--model` when AMPLIHACK_DEFAULT_MODEL
/// is unset, which stopped being true when [`DEFAULT_MODEL`] was introduced.
///
/// Both checks are positive, and each is about this variable alone. Every text
/// must show `--model <DEFAULT_MODEL>`. The variable's own entry in
/// environment-variables.md must give it on its `**Default:**` line. On main
/// that line read "unset — amplihack passes **no** `--model` at all", and no
/// page showed the flag. Both checks fail on that.
///
/// There is deliberately no ban on phrases such as "no built-in default".
/// environment-variables.md documents dozens of variables, and that phrase is
/// true of some of them, such as the required AMPLIHACK_LITELLM_MODEL. A
/// page-wide ban fails on a correct sentence about another variable and blames
/// DEFAULT_MODEL for it.
#[test]
fn test_model_help_and_docs_name_the_built_in_default() {
    let default_flag = format!("--model {DEFAULT_MODEL}");
    for (name, text) in model_help_and_pages() {
        assert!(
            text.contains(&default_flag),
            "{name} must name the built-in default `{default_flag}`"
        );
    }

    let heading = "### AMPLIHACK_DEFAULT_MODEL";
    let default_line = ENVIRONMENT_VARIABLES_MD
        .lines()
        .skip_while(|line| *line != heading)
        .skip(1)
        .take_while(|line| !line.starts_with("## ") && !line.starts_with("### "))
        .find(|line| line.starts_with("**Default:**"));
    assert!(
        default_line.is_some_and(|line| line.contains(&format!("`{DEFAULT_MODEL}`"))),
        "docs/reference/environment-variables.md, section `{heading}`: the \
         **Default:** line must give the built-in default `{DEFAULT_MODEL}`, \
         found {default_line:?}"
    );
}

/// Issue #1527 review: the stderr lines the docs show for `--model` are the ones
/// amplihack prints, in both directions.
///
/// Each line is rendered here by the code, not copied, and the `DOCUMENTED_*`
/// constants the other tests compare the code against must equal those
/// renderings. Then environment-variables.md must show every one of them, each
/// on a line of its own, and no page may show a full `--model` stderr line that
/// is not one of them. A change to the wording in the code fails here until the
/// docs say the same.
#[test]
fn test_documented_model_stderr_lines_are_the_ones_amplihack_prints() {
    let dotted = ["--model".to_string(), "claude-opus-5.5".to_string()];
    let rendered = [
        model_selection_notice(
            &selection("claude-sonnet-4-5", ModelSource::DefaultModelEnv, None),
            "claude",
        ),
        model_selection_notice(
            &selection(
                "claude-opus-5-5[1m]",
                ModelSource::DefaultModelEnv,
                Some("claude-opus-5.5[1m]"),
            ),
            "claude",
        ),
        model_selection_notice(
            &selection("gateway-model", ModelSource::LiteLlmGateway, None),
            "claude",
        ),
        explicit_model_warnings("claude", &dotted, false).concat(),
    ];
    assert_eq!(rendered[1], DOCUMENTED_REWRITE_NOTICE);
    assert_eq!(rendered[2], DOCUMENTED_GATEWAY_NOTICE);
    assert_eq!(rendered[3], DOCUMENTED_EXPLICIT_WARNING);

    for line in &rendered {
        assert!(
            ENVIRONMENT_VARIABLES_MD.lines().any(|doc| doc == line),
            "docs/reference/environment-variables.md must show, on a line of its \
             own, the stderr line amplihack prints:\n{line}"
        );
    }

    for (page, text) in [
        (
            "docs/reference/environment-variables.md",
            ENVIRONMENT_VARIABLES_MD,
        ),
        (
            "docs/reference/launch-flag-injection.md",
            LAUNCH_FLAG_INJECTION_MD,
        ),
        (
            "docs/reference/LAUNCHER_MODEL_CONFIGURATION.md",
            LAUNCHER_MODEL_CONFIGURATION_MD,
        ),
    ] {
        for doc in text.lines().map(str::trim).filter(|doc| {
            doc.starts_with("amplihack: passing `--model")
                || doc.starts_with("amplihack: warning: passing `--model")
        }) {
            assert!(
                rendered.iter().any(|line| line == doc),
                "{page} shows a stderr line amplihack does not print:\n{doc}\n\
                 amplihack prints:\n{}",
                rendered.join("\n")
            );
        }
    }
}

/// Issue #1527 review: only the three variables `proxy_requested()` checks
/// select the LiteLLM gateway (see
/// `test_only_the_three_gateway_variables_select_the_gateway`). An earlier
/// draft of the help text and LAUNCHER_MODEL_CONFIGURATION.md said any
/// `AMPLIHACK_LITELLM_*` variable did. That told an operator with only
/// telemetry variables set that their AMPLIHACK_DEFAULT_MODEL was ignored when
/// it was in use.
///
/// The check is positive: both texts must name each of the three. The draft
/// named neither AMPLIHACK_LITELLM_ENDPOINT nor AMPLIHACK_LITELLM_API_KEY in
/// either text, so this fails on it. There is no ban on the phrase "any
/// `AMPLIHACK_LITELLM_*`". It is true in other contexts:
/// docs/reference/security-recommendations.md uses it for the variables launch
/// setup subprocesses never receive.
#[test]
fn test_help_and_docs_name_each_gateway_variable() {
    for (name, text) in [
        ("`amplihack claude --help`", claude_long_help()),
        (
            "docs/reference/LAUNCHER_MODEL_CONFIGURATION.md",
            LAUNCHER_MODEL_CONFIGURATION_MD.to_string(),
        ),
    ] {
        for var in PROXY_ENV_VARS {
            assert!(
                text.contains(var),
                "{name} must name {var}, one of the variables that select the gateway"
            );
        }
    }
}

/// Issue #1527, wired together short of spawning a process: the selection a
/// dotted environment value produces renders as the line the docs promise.
#[test]
fn test_dotted_default_model_env_renders_the_documented_notice() {
    let got = with_default_model_env(Some("claude-opus-5.5[1m]"), configured_default_model)
        .expect("a dotted id is still a model to pass");
    assert_eq!(
        model_selection_notice(&got, "claude"),
        DOCUMENTED_REWRITE_NOTICE
    );
}
