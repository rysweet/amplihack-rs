//! Issue #1985: flags amplihack adds to a recipe agent step on Codex.
//! Issue #2317: the sandbox is `danger-full-access`, the same trust a Claude
//! step gets from `--dangerously-skip-permissions`; no network override.
//!
//! `codex_agent_step_args` is pure: it reads only the argv tail it is given.
//! These tests pin its exact output. Expected values are written out as
//! literals, not taken from the module's constants, so a change to the
//! injected flags has to change this file too.

use super::codex_agent_step::{
    SANDBOX_FLAGS, codex_agent_step_args, codex_agent_step_notice, exec_subcommand_index,
};

const SANDBOX: [&str; 2] = ["--sandbox", "danger-full-access"];
/// The `-c` value amplihack added before #2317. It must never be injected now.
const OLD_NETWORK: &str = "sandbox_workspace_write.network_access=true";
const HOOK_TRUST: &str = "--dangerously-bypass-hook-trust";
const APPROVALS_BYPASS: &str = "--dangerously-bypass-approvals-and-sandbox";

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// `head`, then the given injected tokens, then `tail`.
fn spliced(head: &[&str], injected: &[&str], tail: &[&str]) -> Vec<String> {
    head.iter()
        .chain(injected)
        .chain(tail)
        .map(|s| (*s).to_string())
        .collect()
}

fn injected() -> Vec<&'static str> {
    SANDBOX.iter().copied().chain([HOOK_TRUST]).collect()
}

fn count(args: &[String], token: &str) -> usize {
    args.iter().filter(|a| *a == token).count()
}

// ---- exec_subcommand_index ----

#[test]
fn exec_index_finds_first_positional_exec_or_alias() {
    let cases: &[(&[&str], Option<usize>)] = &[
        (&["exec"], Some(0)),
        (&["e", "-"], Some(0)),
        (&["exec", "resume", "abc"], Some(0)),
        (&["-m", "gpt", "exec"], Some(2)),
        (&["--model", "gpt", "exec"], Some(2)),
        (&["--model=gpt", "exec"], Some(1)),
        (&["-mgpt", "exec"], Some(1)),
        (&["-c", "x=1", "exec"], Some(2)),
        (&["--config", "x=1", "exec"], Some(2)),
        (&["-p", "ci", "exec"], Some(2)),
        (&["--profile", "ci", "exec"], Some(2)),
        (&["-C", "/work", "exec"], Some(2)),
        (&["--cd", "/work", "exec"], Some(2)),
        (&["-s", "read-only", "exec"], Some(2)),
        (&["--sandbox", "read-only", "exec"], Some(2)),
        (&["-a", "never", "exec"], Some(2)),
        (&["--ask-for-approval", "never", "exec"], Some(2)),
        (&["-i", "img.png", "exec"], Some(2)),
        (&["--image", "img.png", "exec"], Some(2)),
        (&["--add-dir", "/extra", "e"], Some(2)),
        (&["--enable", "feat", "exec"], Some(2)),
        (&["--disable", "feat", "exec"], Some(2)),
        (&["--local-provider", "ollama", "exec"], Some(2)),
        // A boolean root flag takes no value.
        (&["--oss", "exec"], Some(1)),
    ];
    for (argv, expected) in cases {
        assert_eq!(exec_subcommand_index(&v(argv)), *expected, "argv {argv:?}");
    }
}

#[test]
fn exec_index_is_none_without_an_exec_subcommand() {
    let cases: &[&[&str]] = &[
        &[],
        &["resume", "--last"],
        &["resume", "exec"],
        &["fix the failing test"],
        &["prompt", "exec"],
        &["-m", "gpt-5"],
        // `exec` only as the value of an option.
        &["-c", "exec"],
        &["-m", "exec"],
        &["--model", "exec"],
        &["-p", "e"],
        // `exec` only after the end of options.
        &["--", "exec"],
        &["-m", "gpt", "--", "exec", "-"],
        // Not the subcommand token.
        &["execute"],
        &["--exec"],
    ];
    for argv in cases {
        assert_eq!(exec_subcommand_index(&v(argv)), None, "argv {argv:?}");
    }
}

// ---- injection on agent steps ----

#[test]
fn plain_exec_gets_full_access_and_hook_trust_right_after_exec() {
    let input = v(&["exec", "--output-last-message", "/run/final-message", "-"]);
    let expected = spliced(
        &["exec"],
        &injected(),
        &["--output-last-message", "/run/final-message", "-"],
    );
    let out = codex_agent_step_args(&input);
    assert_eq!(out, expected);
    for flag in ["--sandbox", "danger-full-access", HOOK_TRUST] {
        assert_eq!(count(&out, flag), 1, "{flag} must appear once: {out:?}");
    }
    assert!(
        !out.iter()
            .any(|a| a.contains("workspace-write") || a.contains("sandbox_workspace_write")),
        "no workspace-write sandbox or network override may be injected: {out:?}"
    );
}

#[test]
fn exec_alias_e_gets_the_flags() {
    let out = codex_agent_step_args(&v(&["e", "-"]));
    assert_eq!(out, spliced(&["e"], &injected(), &["-"]));
}

#[test]
fn bare_exec_with_nothing_after_gets_the_flags() {
    let out = codex_agent_step_args(&v(&["exec"]));
    assert_eq!(out, spliced(&["exec"], &injected(), &[]));
}

#[test]
fn exec_resume_gets_the_flags_before_resume() {
    // codex-cli 0.166.0 rejects `--sandbox` after `resume`.
    let out = codex_agent_step_args(&v(&["exec", "resume", "sess-123", "-"]));
    assert_eq!(
        out,
        spliced(&["exec"], &injected(), &["resume", "sess-123", "-"])
    );
    let out = codex_agent_step_args(&v(&["exec", "resume", "--last"]));
    assert_eq!(out, spliced(&["exec"], &injected(), &["resume", "--last"]));
}

#[test]
fn root_options_before_exec_stay_in_front() {
    let out = codex_agent_step_args(&v(&["-m", "gpt-5", "-c", "x=1", "exec", "-"]));
    assert_eq!(
        out,
        spliced(&["-m", "gpt-5", "-c", "x=1", "exec"], &injected(), &["-"])
    );
}

#[test]
fn profile_is_not_a_sandbox_choice() {
    // S7: CLI flags outrank a profile's sandbox_mode; a profile that sets no
    // mode would otherwise fall back to the host config.
    for argv in [
        &["-p", "ci", "exec", "-"][..],
        &["--profile", "ci", "exec", "-"][..],
        &["exec", "-p", "ci", "-"][..],
    ] {
        let out = codex_agent_step_args(&v(argv));
        assert_eq!(
            count(&out, "danger-full-access"),
            1,
            "argv {argv:?}: {out:?}"
        );
        assert_eq!(count(&out, OLD_NETWORK), 0, "argv {argv:?}: {out:?}");
        assert_eq!(count(&out, HOOK_TRUST), 1, "argv {argv:?}: {out:?}");
    }
    let out = codex_agent_step_args(&v(&["-p", "ci", "exec", "-"]));
    assert_eq!(out, spliced(&["-p", "ci", "exec"], &injected(), &["-"]));
}

// ---- the caller's sandbox choice wins ----

#[test]
fn caller_sandbox_suppresses_sandbox_but_not_hook_trust() {
    let cases: &[&[&str]] = &[
        &["--sandbox", "read-only"],
        &["--sandbox", "danger-full-access"],
        &["-s", "danger-full-access"],
        &["-s", "read-only"],
        &["--sandbox=read-only"],
        &["-sread-only"],
        &["-sworkspace-write"],
        &["-sdanger-full-access"],
        &["-c", "sandbox_mode=read-only"],
        &["-csandbox_mode=read-only"],
        &["--config", "sandbox_mode=read-only"],
        &["--config=sandbox_mode=read-only"],
        // clap drops one `=` after a short option.
        &["-c=sandbox_mode=read-only"],
        &["-s=read-only"],
        // Codex trims the `-c` key, so spaces around `=` are allowed.
        &["-c", "sandbox_mode = \"read-only\""],
        &["--config", " sandbox_mode=read-only"],
        &[APPROVALS_BYPASS],
        &["--approve-for-me"],
        &["--yolo"],
    ];
    for choice in cases {
        let input = spliced(&["exec"], choice, &["-"]);
        let out = codex_agent_step_args(&input);
        let expected = spliced(&["exec", HOOK_TRUST], choice, &["-"]);
        assert_eq!(out, expected, "caller choice {choice:?}");
        assert_eq!(count(&out, HOOK_TRUST), 1, "caller choice {choice:?}");
        assert!(
            !out.iter().any(|a| a == OLD_NETWORK),
            "no network override with caller choice {choice:?}: {out:?}"
        );
    }
}

#[test]
fn root_level_caller_sandbox_before_exec_is_respected() {
    let out = codex_agent_step_args(&v(&["--sandbox", "read-only", "exec", "-"]));
    assert_eq!(out, v(&["--sandbox", "read-only", "exec", HOOK_TRUST, "-"]));
    let out = codex_agent_step_args(&v(&["-c", "sandbox_mode=read-only", "e", "-"]));
    assert_eq!(
        out,
        v(&["-c", "sandbox_mode=read-only", "e", HOOK_TRUST, "-"])
    );
}

#[test]
fn caller_sandbox_after_resume_is_respected() {
    let input = v(&["exec", "resume", "--last", "-c", "sandbox_mode=read-only"]);
    let out = codex_agent_step_args(&input);
    assert_eq!(
        out,
        v(&[
            "exec",
            HOOK_TRUST,
            "resume",
            "--last",
            "-c",
            "sandbox_mode=read-only"
        ])
    );
}

/// Decision 6 (#2317): a caller's own `sandbox_workspace_write.*` override is
/// not a sandbox choice. It passes through unchanged, full access and hook
/// trust are still added, and nothing is stripped or rewritten.
#[test]
fn caller_workspace_write_config_passes_through() {
    let input = v(&[
        "exec",
        "-c",
        "sandbox_workspace_write.network_access=false",
        "-",
    ]);
    let out = codex_agent_step_args(&input);
    assert_eq!(
        out,
        v(&[
            "exec",
            "--sandbox",
            "danger-full-access",
            HOOK_TRUST,
            "-c",
            "sandbox_workspace_write.network_access=false",
            "-",
        ])
    );
    for form in [
        &["--config", "sandbox_workspace_write.network_access=false"][..],
        &["--config=sandbox_workspace_write.network_access=false"][..],
        &["-csandbox_workspace_write.network_access=false"][..],
        &["-c=sandbox_workspace_write.network_access=false"][..],
        &["-c", "sandbox_workspace_write.network_access = false"][..],
        &["-c", "sandbox_workspace_write.writable_roots=[\"/x\"]"][..],
    ] {
        let input = spliced(&["exec"], form, &["-"]);
        let out = codex_agent_step_args(&input);
        assert_eq!(
            out,
            spliced(&["exec"], &injected(), &[form, &["-"][..]].concat()),
            "form {form:?}"
        );
        assert_eq!(count(&out, OLD_NETWORK), 0, "form {form:?}: {out:?}");
    }
}

/// #2317: the network override existed only for the workspace-write
/// sandbox. Full access needs none, so no `sandbox_workspace_write.*` token
/// is ever added, whatever the input.
#[test]
fn never_injects_network_override() {
    for input in every_input() {
        let out = codex_agent_step_args(&input);
        let overrides = |args: &[String]| {
            args.iter()
                .filter(|a| a.contains("sandbox_workspace_write"))
                .count()
        };
        assert_eq!(
            overrides(&out),
            overrides(&input),
            "network override added for {input:?}: {out:?}"
        );
    }
}

#[test]
fn caller_hook_trust_appears_once() {
    let out = codex_agent_step_args(&v(&["exec", HOOK_TRUST, "-"]));
    assert_eq!(
        out,
        spliced(&["exec"], &[SANDBOX[0], SANDBOX[1]], &[HOOK_TRUST, "-"])
    );
    assert_eq!(count(&out, HOOK_TRUST), 1);
}

#[test]
fn caller_choosing_everything_gets_nothing_added() {
    let input = v(&[
        "exec",
        "-s",
        "read-only",
        "-c",
        "sandbox_workspace_write.network_access=false",
        HOOK_TRUST,
        "-",
    ]);
    assert_eq!(codex_agent_step_args(&input), input);
}

// ---- detection is narrow (S2): no false positives ----

#[test]
fn sandbox_lookalikes_do_not_count_as_a_caller_choice() {
    let cases: &[&[&str]] = &[
        // Attached `-s` with something that is not a sandbox mode.
        &["exec", "--output-last-message", "-sfoo", "-"],
        &["exec", "-sfoo", "-"],
        &["exec", "-s=foo", "-"],
        // `--sandbox` or `-s` only as the value of another option.
        &["-c", "--sandbox", "exec", "-"],
        &["exec", "-m", "-s", "-"],
        &["exec", "-m", "--sandbox", "-"],
        // A -c override that is not `sandbox_mode=`.
        &["exec", "-c", "model=--sandbox=read-only", "-"],
        &["exec", "-c", "sandbox_mode_note=x", "-"],
        &["exec", "-c", "sandbox_mode_note = x", "-"],
        // A non-ASCII short option is passed through, not parsed.
        &["exec", "-é", "-"],
        &["exec", "-\u{2013}sandbox", "read-only", "-"],
        // After a bare `--`, these are prompt text.
        &["exec", "--", "--sandbox", "read-only"],
        &["exec", "--", "-c", "sandbox_mode=read-only"],
        &["exec", "--", APPROVALS_BYPASS],
    ];
    for argv in cases {
        let out = codex_agent_step_args(&v(argv));
        let idx = argv.iter().position(|a| *a == "exec").unwrap();
        assert_eq!(
            out,
            spliced(&argv[..=idx], &injected(), &argv[idx + 1..]),
            "argv {argv:?}"
        );
    }
}

#[test]
fn hook_trust_or_network_after_double_dash_is_prompt_text() {
    let out = codex_agent_step_args(&v(&["exec", "--", HOOK_TRUST]));
    assert_eq!(out, spliced(&["exec"], &injected(), &["--", HOOK_TRUST]));
    let input = v(&[
        "exec",
        "--",
        "-c",
        "sandbox_workspace_write.network_access=false",
    ]);
    let out = codex_agent_step_args(&input);
    assert_eq!(
        out,
        spliced(
            &["exec"],
            &injected(),
            &["--", "-c", "sandbox_workspace_write.network_access=false"],
        )
    );
    assert_eq!(count(&out, OLD_NETWORK), 0, "{out:?}");
}

// ---- interactive launches are unchanged ----

#[test]
fn argv_without_exec_is_returned_unchanged() {
    let cases: &[&[&str]] = &[
        &[],
        &["resume", "--last"],
        &["resume", "sess-123"],
        &["fix the failing test"],
        &["-m", "gpt-5"],
        &["-m", "gpt-5", "write a haiku"],
        &["-c", "exec"],
        &["-m", "exec"],
        &["--", "exec"],
        &["--", "exec", "-"],
        &["resume", "exec"],
        &["--sandbox", "read-only"],
        &[HOOK_TRUST],
        // A multi-byte character after `-` must not panic.
        &["-éxplain"],
        &["-\u{2013}sandbox", "read-only"],
    ];
    for argv in cases {
        assert_eq!(codex_agent_step_args(&v(argv)), v(argv), "argv {argv:?}");
    }
}

// ---- invariants over every case (S1) ----

fn every_input() -> Vec<Vec<String>> {
    let mut inputs: Vec<Vec<String>> = [
        &["exec", "-"][..],
        &["e", "-"],
        &["exec", "resume", "id", "-"],
        &["-p", "ci", "exec", "-"],
        &["exec", "--sandbox", "read-only", "-"],
        &["exec", "-s", "danger-full-access", "-"],
        &["exec", APPROVALS_BYPASS, "-"],
        &["exec", "--yolo", "-"],
        &[
            "exec",
            "-c",
            "sandbox_workspace_write.network_access=false",
            "-",
        ],
        &["exec", HOOK_TRUST, "-"],
        &["exec", "--", "--yolo"],
        &[],
        &["resume", "--last"],
        &["a prompt"],
        &[APPROVALS_BYPASS],
    ]
    .iter()
    .map(|a| v(a))
    .collect();
    inputs.push(v(&["exec", "--output-last-message", "/r/final", "-"]));
    // #2317: every other form of caller sandbox choice.
    for choice in [
        &["-sworkspace-write"][..],
        &["-sread-only"],
        &["--sandbox=read-only"],
        &["--sandbox", "danger-full-access"],
        &["-c", "sandbox_mode=read-only"],
        &["--approve-for-me"],
    ] {
        inputs.push(spliced(&["exec"], choice, &["-"]));
    }
    inputs.push(v(&["--sandbox", "read-only", "exec", "-"]));
    inputs
}

/// #2317: full access is now the injected sandbox, so it is no longer
/// forbidden. The approvals bypass, `--yolo` and any workspace-write token
/// are: none of them may be added, whole or embedded in a new token.
#[test]
fn never_adds_the_approvals_bypass_yolo_or_workspace_write() {
    for input in every_input() {
        let out = codex_agent_step_args(&input);
        for forbidden in [
            APPROVALS_BYPASS,
            "--yolo",
            "workspace-write",
            "sandbox_workspace_write",
        ] {
            assert_eq!(
                count(&out, forbidden),
                count(&input, forbidden),
                "{forbidden} added for input {input:?}: {out:?}"
            );
            assert!(
                !out.iter()
                    .any(|a| a.contains(forbidden) && !input.contains(a)),
                "{forbidden} embedded in a new token for {input:?}: {out:?}"
            );
        }
    }
}

/// Sandbox selections in an argv, counted up to a bare `--`: `--sandbox X`,
/// `--sandbox=X`, `-s X`, `-s<mode>`, `-c sandbox_mode=…` and the flags in
/// [`SANDBOX_FLAGS`]. Only used on [`every_input`], which holds no option
/// value that looks like one of these.
fn sandbox_selections(args: &[String]) -> usize {
    let mut n = 0;
    let mut prev = "";
    for arg in args.iter().map(String::as_str) {
        if arg == "--" {
            break;
        }
        let config_value = (prev == "-c" || prev == "--config").then_some(arg);
        let attached_config = arg
            .strip_prefix("--config=")
            .or_else(|| arg.strip_prefix("-c").filter(|_| arg.len() > 2));
        let is_sandbox_mode = |v: &str| {
            v.trim_start_matches('=')
                .trim_start()
                .starts_with("sandbox_mode")
        };
        if arg == "--sandbox"
            || arg == "-s"
            || arg.starts_with("--sandbox=")
            || ["read-only", "workspace-write", "danger-full-access"]
                .iter()
                .any(|m| arg == format!("-s{m}") || arg == format!("-s={m}"))
            || SANDBOX_FLAGS.contains(&arg)
            || config_value.is_some_and(is_sandbox_mode)
            || attached_config.is_some_and(is_sandbox_mode)
        {
            n += 1;
        }
        prev = arg;
    }
    n
}

/// #2317 (S3): amplihack adds exactly one sandbox selection when the caller
/// made none, and none when the caller made one. Codex (clap) rejects a
/// second `--sandbox`, so a second selection would stop the launch.
#[test]
fn at_most_one_sandbox_selection() {
    for input in every_input() {
        let out = codex_agent_step_args(&input);
        let before = sandbox_selections(&input);
        let after = sandbox_selections(&out);
        let expected = if exec_subcommand_index(&input).is_some() {
            before.max(1)
        } else {
            before
        };
        assert_eq!(
            after, expected,
            "input {input:?} ({before} selections) -> {out:?} ({after})"
        );
    }
}

#[test]
fn only_adds_tokens_and_keeps_the_caller_order() {
    for input in every_input() {
        let out = codex_agent_step_args(&input);
        assert!(out.len() >= input.len(), "{input:?} -> {out:?}");
        // The caller's tokens appear in the output as a subsequence, in order.
        let mut it = out.iter();
        for token in &input {
            assert!(
                it.any(|o| o == token),
                "caller token {token:?} lost or reordered: {input:?} -> {out:?}"
            );
        }
    }
}

#[test]
fn injection_is_idempotent() {
    for input in every_input() {
        let once = codex_agent_step_args(&input);
        let twice = codex_agent_step_args(&once);
        assert_eq!(once, twice, "input {input:?}");
    }
}

// ---- the sandbox-choice table matches codex-cli 0.166.0 ----

/// `codex exec --help` from codex-cli 0.166.0, the version the issue names.
const EXEC_HELP_0_166_0: &str = include_str!("fixtures/codex-exec-help-0.166.0.txt");

/// Hidden aliases Codex accepts but does not print in `--help`.
const HIDDEN_SANDBOX_ALIASES: &[&str] = &["--yolo"];

/// Each long option in the help text with its description.
fn help_entries(help: &str) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for line in help.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with('-') && indent <= 6 {
            let long = trimmed
                .split([' ', ','])
                .find(|t| t.starts_with("--"))
                .unwrap_or("")
                .to_string();
            entries.push((long, String::new()));
        } else if let Some(entry) = entries.last_mut() {
            entry.1.push_str(trimmed);
            entry.1.push(' ');
        }
    }
    entries
}

/// True when the description names the sandbox as a word (`sandbox`,
/// `sandboxing`), not inside a config key such as `sandbox_permissions`.
fn mentions_sandbox(description: &str) -> bool {
    description
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|word| word.to_ascii_lowercase().starts_with("sandbox") && !word.contains('_'))
}

#[test]
fn sandbox_flags_match_codex_0_166_0_exec_help() {
    let documented: Vec<String> = help_entries(EXEC_HELP_0_166_0)
        .into_iter()
        .filter(|(long, description)| {
            !long.is_empty() && long != "--sandbox" && mentions_sandbox(description)
        })
        .map(|(long, _)| long)
        .collect();
    assert!(
        documented.iter().any(|f| f == "--approve-for-me"),
        "help parser lost --approve-for-me: {documented:?}"
    );
    let mut expected: Vec<&str> = documented.iter().map(String::as_str).collect();
    expected.extend(HIDDEN_SANDBOX_ALIASES);
    expected.sort_unstable();
    let mut actual = SANDBOX_FLAGS.to_vec();
    actual.sort_unstable();
    assert_eq!(
        actual, expected,
        "SANDBOX_FLAGS must list every option that `codex exec --help` (0.166.0) \
         describes as choosing a sandbox, plus hidden aliases"
    );
}

#[test]
fn full_auto_is_not_a_codex_0_166_0_option() {
    assert!(!EXEC_HELP_0_166_0.contains("--full-auto"));
    assert!(!SANDBOX_FLAGS.contains(&"--full-auto"));
}

#[test]
fn approve_for_me_suppresses_the_injected_sandbox() {
    // codex-cli 0.166.0 rejects `--sandbox` with `--approve-for-me` (exit 2).
    for argv in [
        &["exec", "--approve-for-me", "-"][..],
        &["--approve-for-me", "exec", "-"][..],
        &["exec", "resume", "--last", "--approve-for-me"][..],
    ] {
        let out = codex_agent_step_args(&v(argv));
        assert_eq!(count(&out, "--sandbox"), 0, "argv {argv:?}: {out:?}");
        assert_eq!(
            count(&out, "danger-full-access"),
            0,
            "argv {argv:?}: {out:?}"
        );
        assert_eq!(count(&out, OLD_NETWORK), 0, "argv {argv:?}: {out:?}");
        assert_eq!(count(&out, HOOK_TRUST), 1, "argv {argv:?}: {out:?}");
    }
    let out = codex_agent_step_args(&v(&["exec", "--approve-for-me", "-"]));
    assert_eq!(out, v(&["exec", HOOK_TRUST, "--approve-for-me", "-"]));
}

// ---- codex_agent_step_notice (issue #2010) ----
//
// The expected lines are written out in full, so a change to the wording has
// to change this file and the reference table in
// `docs/reference/launch-flag-injection.md` together.

const NOTICE_ALL: &str = "amplihack: codex exec: added --sandbox danger-full-access, \
                          --dangerously-bypass-hook-trust (#1985)";
const NOTICE_SANDBOX_ONLY: &str =
    "amplihack: codex exec: added --sandbox danger-full-access (#1985)";

fn notice(argv: &[&str]) -> Option<String> {
    codex_agent_step_notice(&v(argv))
}

#[test]
fn notice_names_both_flags_when_caller_set_nothing() {
    for argv in [
        &["exec", "-"][..],
        &["e", "-"][..],
        &["exec", "--output-last-message", "/r/final", "-"][..],
        &["-m", "gpt", "exec", "-"][..],
        // A profile is not a sandbox choice (#1985).
        &["-p", "ci", "exec", "-"][..],
    ] {
        assert_eq!(notice(argv).as_deref(), Some(NOTICE_ALL), "argv {argv:?}");
    }
}

/// #2317: a caller's `sandbox_workspace_write.*` override changes nothing in
/// the line, and the line never mentions network access.
#[test]
fn notice_ignores_caller_workspace_write_config() {
    for argv in [
        &[
            "exec",
            "-c",
            "sandbox_workspace_write.network_access=false",
            "-",
        ][..],
        &[
            "exec",
            "--config",
            "sandbox_workspace_write.network_access=true",
            "-",
        ][..],
        &[
            "-c",
            " sandbox_workspace_write.network_access =false",
            "exec",
            "-",
        ][..],
    ] {
        assert_eq!(notice(argv).as_deref(), Some(NOTICE_ALL), "argv {argv:?}");
    }
}

#[test]
fn notice_omits_hook_trust_when_caller_passed_it() {
    for argv in [
        &["exec", HOOK_TRUST, "-"][..],
        &[HOOK_TRUST, "exec", "-"][..],
        &[
            "exec",
            "-c",
            "sandbox_workspace_write.network_access=false",
            HOOK_TRUST,
            "-",
        ][..],
    ] {
        assert_eq!(
            notice(argv).as_deref(),
            Some(NOTICE_SANDBOX_ONLY),
            "argv {argv:?}"
        );
    }
}

/// D1: a caller that chose a sandbox gets no line, even though the
/// hook-trust flag is still spliced into the argv (#2048 tracks reporting
/// that case).
#[test]
fn notice_is_none_when_caller_chose_sandbox() {
    for argv in [
        &["exec", "--sandbox", "read-only", "-"][..],
        &["exec", "--sandbox=read-only", "-"][..],
        &["exec", "-s", "read-only", "-"][..],
        &["exec", "-sread-only", "-"][..],
        &["-s", "danger-full-access", "exec", "-"][..],
        &["exec", "-c", "sandbox_mode=\"read-only\"", "-"][..],
        &["exec", "--approve-for-me", "-"][..],
        &["exec", APPROVALS_BYPASS, "-"][..],
        &["exec", "--yolo", "-"][..],
    ] {
        assert_eq!(notice(argv), None, "argv {argv:?}");
        let out = codex_agent_step_args(&v(argv));
        assert_eq!(
            count(&out, HOOK_TRUST),
            1,
            "hook trust must still be injected for {argv:?}: {out:?}"
        );
        let input = v(argv);
        assert_eq!(
            count(&out, "danger-full-access"),
            count(&input, "danger-full-access"),
            "full access must not be added over a caller choice: {argv:?}: {out:?}"
        );
        assert_eq!(
            count(&out, "--sandbox"),
            count(&input, "--sandbox"),
            "{argv:?}: {out:?}"
        );
        assert!(
            !out.iter()
                .any(|a| a.contains("workspace-write") && !input.contains(a)),
            "{argv:?}: {out:?}"
        );
    }
}

#[test]
fn notice_is_none_when_caller_chose_sandbox_and_hook_trust() {
    // Nothing is injected at all.
    let argv = ["exec", "--sandbox", "read-only", HOOK_TRUST, "-"];
    assert_eq!(codex_agent_step_args(&v(&argv)), v(&argv));
    assert_eq!(notice(&argv), None);
}

#[test]
fn notice_is_none_without_exec_subcommand() {
    for argv in [
        &[][..],
        &["resume", "--last"][..],
        &["a prompt"][..],
        &["-c", "exec"][..],
        &["-m", "exec"][..],
        &["--", "exec"][..],
        &["-"][..],
    ] {
        assert_eq!(notice(argv), None, "argv {argv:?}");
        assert_eq!(codex_agent_step_args(&v(argv)), v(argv), "argv {argv:?}");
    }
}

#[test]
fn notice_present_for_exec_resume() {
    for argv in [
        &["exec", "resume", "--last", "-"][..],
        &[
            "exec",
            "resume",
            "0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
            "-",
        ][..],
        &["exec", "--help"][..],
    ] {
        assert_eq!(notice(argv).as_deref(), Some(NOTICE_ALL), "argv {argv:?}");
    }
}

/// D3: `--sandbox` with no value is not a caller choice, so the flags are
/// added and the line is printed.
#[test]
fn notice_present_for_dangling_sandbox() {
    let argv = ["exec", "--sandbox"];
    assert_eq!(notice(&argv).as_deref(), Some(NOTICE_ALL));
    assert_eq!(
        codex_agent_step_args(&v(&argv)),
        v(&[
            "exec",
            "--sandbox",
            "danger-full-access",
            HOOK_TRUST,
            "--sandbox"
        ])
    );
}

/// S3: the line is built from fixed text only. Caller paths, `-c` values,
/// profile names, prompt text, escape sequences and multi-byte input never
/// appear in it.
#[test]
fn notice_ignores_argument_content() {
    let hostile = [
        "/home/someone/secret/final-message",
        "\u{1b}]0;pwned\u{7}",
        "line\namplihack: codex exec: added --yolo (#1985)",
        "ci-profile-name",
        "model-\u{e9}\u{4e2d}",
        "-\u{e9}",
    ];
    let argv = [
        "-p",
        hostile[3],
        "-m",
        hostile[4],
        "exec",
        "--output-last-message",
        hostile[0],
        "-c",
        "x=\u{1b}[31m",
        hostile[5],
        hostile[1],
        hostile[2],
    ];
    let line = notice(&argv).expect("exec without a sandbox choice gets a line");
    assert_eq!(line, NOTICE_ALL);
    assert!(!line.contains('\n') && !line.contains('\u{1b}'), "{line:?}");
    assert!(line.is_ascii(), "{line:?}");
}

/// The line is `Some` exactly when `--sandbox danger-full-access` is spliced in,
/// and it never names something the argv does not contain.
#[test]
fn notice_matches_the_injected_argv() {
    let mut inputs = every_input();
    inputs.push(v(&["exec", "--sandbox"]));
    inputs.push(v(&[
        "exec",
        "-c",
        "sandbox_workspace_write.network_access=false",
        HOOK_TRUST,
    ]));
    for input in inputs {
        let out = codex_agent_step_args(&input);
        let added = |token: &str| count(&out, token) > count(&input, token);
        let line = codex_agent_step_notice(&input);
        assert_eq!(
            line.is_some(),
            added("danger-full-access"),
            "input {input:?}: argv {out:?}, notice {line:?}"
        );
        let Some(line) = line else { continue };
        assert!(!line.contains("network"), "{input:?}: {line}");
        assert!(!line.contains("workspace-write"), "{input:?}: {line}");
        assert_eq!(
            line.contains(HOOK_TRUST),
            added(HOOK_TRUST),
            "{input:?}: {line}"
        );
        assert_eq!(line.lines().count(), 1, "{line:?}");
    }
}

/// S1, S4: full access is injected only on the `exec`/`e` path, and only
/// as the pair `--sandbox danger-full-access` directly after the subcommand.
#[test]
fn full_access_injected_only_after_exec() {
    for input in every_input() {
        let out = codex_agent_step_args(&input);
        if count(&out, "danger-full-access") > count(&input, "danger-full-access") {
            let index = exec_subcommand_index(&input)
                .unwrap_or_else(|| panic!("full access added without exec: {input:?} -> {out:?}"));
            assert_eq!(
                &out[index + 1..index + 3],
                &v(&SANDBOX)[..],
                "full access not spliced after exec: {input:?} -> {out:?}"
            );
        }
    }
}

#[test]
fn notice_is_pure_and_repeatable() {
    let argv = v(&["exec", "-"]);
    let before = argv.clone();
    assert_eq!(
        codex_agent_step_notice(&argv),
        codex_agent_step_notice(&argv)
    );
    assert_eq!(argv, before);
}
